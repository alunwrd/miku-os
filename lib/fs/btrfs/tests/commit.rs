// The commit judged by btrfs-progs
//
// Cases end with `btrfs check` or `dump-super`, as this crate reading its own
// superblock is a closed loop.
//
// A superblock's `generation` must equal that of the tree root it names, or a
// reader reports "parent transid verify failed". So the tests play the commit's
// first half: roll the root block's generation, seal it, `write_block` it, then
// `commit_superblocks`. A real commit writes the new root to a new address;
// these overwrite in place, as allocation needs the other half. The crash tests
// note that cost.

mod common;

use btrfs::commit::{commit_superblocks, mirror_count, seal_block, write_block};
use btrfs::node::{header, HEADER_FLAG_WRITTEN};
use btrfs::space::read_free_space;
use btrfs::structs::*;
use btrfs::{check_superblock, BtrfsError, Superblock, Volume};
use common::{Image, IMG_BYTES};
use fshost::{BlockHost, HostError, MemDisk};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

// The host

/// What the driver asked of the disk, in order. The order is invisible in the
/// bytes afterward, so the host records it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Write(u64, u32),
    Sync(u64, u32),
    Barrier(u64, u32),
    Flush,
}

/// A writable image file, written through so `btrfs check` sees what the driver
/// did. Refuses a transfer past the end, since a sparse file accepts a write at
/// 256 GiB
struct FileHost {
    file: std::fs::File,
    sectors: u64,
    log: Vec<Op>,
}

impl FileHost {
    fn open(path: &Path) -> Self {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("cannot open image for writing");
        let sectors = file.metadata().expect("cannot stat image").len() / 512;
        FileHost { file, sectors, log: Vec::new() }
    }

    fn seek_to(&mut self, lba: u64, count: u32) -> Result<usize, HostError> {
        let end = lba.checked_add(count as u64).ok_or(HostError::Io)?;
        if end > self.sectors {
            return Err(HostError::Io);
        }
        self.file
            .seek(SeekFrom::Start(lba * 512))
            .map_err(|_| HostError::Io)?;
        Ok(count as usize * 512)
    }

    fn put(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        let n = self.seek_to(lba, count)?;
        if buf.len() < n {
            return Err(HostError::Io);
        }
        self.file.write_all(&buf[..n]).map_err(|_| HostError::Io)
    }

    /// Each write landing on a superblock mirror, in order. Mirrors are the
    /// only 4 KiB writes here, so the filter uses length and address
    fn super_writes(&self) -> Vec<Op> {
        self.log
            .iter()
            .copied()
            .filter(|op| match op {
                Op::Write(lba, c) | Op::Sync(lba, c) | Op::Barrier(lba, c) => {
                    *c == 8 && SUPER_MIRRORS.contains(&(lba * 512))
                }
                Op::Flush => false,
            })
            .collect()
    }
}

impl BlockHost for FileHost {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        let n = self.seek_to(lba, count)?;
        let n = n.min(buf.len());
        self.file.read_exact(&mut buf[..n]).map_err(|_| HostError::Io)
    }
    fn write_sectors(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.log.push(Op::Write(lba, count));
        self.put(lba, count, buf)
    }
    fn write_sectors_sync(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.log.push(Op::Sync(lba, count));
        self.put(lba, count, buf)
    }
    fn write_sectors_barrier(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.log.push(Op::Barrier(lba, count));
        self.put(lba, count, buf)
    }
    fn flush(&mut self) -> Result<(), HostError> {
        self.log.push(Op::Flush);
        self.file.sync_data().map_err(|_| HostError::Io)
    }
    fn total_sectors(&self) -> u64 {
        self.sectors
    }
}

// The fixture

struct Fixture {
    img: Image,
    vol: Volume<FileHost>,
    /// The generation the volume had when `mkfs` finished
    was: u64,
    /// The generation these tests commit
    now: u64,
    root: u64,
}

/// A volume with its tree root rolled to the next generation and written: the
/// commit's first half, played by the test
fn staged(tag: &str) -> Option<Fixture> {
    let img = Image::new(tag, None)?;
    let host = FileHost::open(&img.path);
    let mut vol = Volume::open(host, 0).expect("a fresh volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");

    let was = vol.geometry.generation;
    let now = was + 1;
    let root = vol.geometry.root;

    let mut block = vol.read_node(root).expect("the root must read").bytes().to_vec();
    block[header::GENERATION..header::GENERATION + 8].copy_from_slice(&now.to_le_bytes());
    seal_block(&mut block, root).expect("a rolled root must seal");
    write_block(&mut vol, root, &block).expect("the new root must write");

    Some(Fixture { img, vol, was, now, root })
}

macro_rules! fx {
    ($tag:expr) => {
        match staged($tag) {
            Some(f) => f,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
}

/// Generation of the primary on the medium. A refused commit must leave it
fn on_disk_generation(img: &Image) -> u64 {
    img.read_super().generation()
}

fn assert_clean(out: &str, what: &str) {
    assert!(
        out.contains("no error found") && !out.contains("ERROR"),
        "btrfs check rejected the volume after {what}:\n{out}"
    );
}

// The judgement

/// Commit, then btrfs-progs. `btrfs check` reads the primary and follows every
/// tree, comparing each block with its parent's transaction. `--super 1` does
/// the same from the first backup, the only external check of the backups
#[test]
fn btrfs_progs_accepts_what_we_committed() {
    let mut f = fx!("judge");
    commit_superblocks(&mut f.vol, f.now).expect("a staged volume must commit");

    assert_clean(&f.img.btrfs(&["check"]), "a commit");
    assert_clean(&f.img.btrfs(&["check", "--super", "1"]), "a commit, read from backup 1");

    let dump = f.img.btrfs(&["inspect-internal", "dump-super"]);
    assert!(
        dump.contains(&format!("generation\t\t{}", f.now)),
        "dump-super must show the generation we committed ({}):\n{dump}",
        f.now
    );
    assert!(
        !dump.contains(&format!("generation\t\t{}", f.was)),
        "and not the one we replaced ({}):\n{dump}",
        f.was
    );
}

/// Each existing mirror has the new generation, its own address, and a checksum
/// covering it there, read back from the file
#[test]
fn every_mirror_that_fits_is_written_whole() {
    let mut f = fx!("mirrors");
    commit_superblocks(&mut f.vol, f.now).expect("a staged volume must commit");

    let want = mirror_count(IMG_BYTES);
    assert_eq!(want, 2, "a 512 MiB image has the primary and one backup");

    for at in SUPER_MIRRORS.iter().copied().take(want) {
        let sb = f.img.read_super_at(at);
        assert!(sb.csum_matches(), "the copy at {at} does not match its own checksum");
        assert_eq!(sb.bytenr(), at, "the copy at {at} says it lives at {}", sb.bytenr());
        assert_eq!(sb.generation(), f.now, "the copy at {at} is at the wrong generation");
        check_superblock(&sb, Some(at))
            .unwrap_or_else(|e| panic!("the copy at {at} is not a superblock we would mount: {e:?}"));
    }
}

/// A mirror not fitting the device is not written, and nothing goes past the
/// end: the file did not grow, the host got no out-of-range sector, and each
/// write landed inside the device
#[test]
fn a_mirror_past_the_end_is_never_written() {
    let mut f = fx!("fit");
    commit_superblocks(&mut f.vol, f.now).expect("a staged volume must commit");

    let len = std::fs::metadata(&f.img.path).expect("cannot stat image").len();
    assert_eq!(len, IMG_BYTES, "the image grew - something was written past its end");

    let sectors = f.vol.host_mut().total_sectors();
    for op in f.vol.host_mut().log.clone() {
        if let Op::Write(lba, c) | Op::Sync(lba, c) | Op::Barrier(lba, c) = op {
            assert!(
                lba + c as u64 <= sectors,
                "{op:?} runs past the end of a {sectors}-sector device"
            );
        }
    }
    for at in SUPER_MIRRORS.iter().copied().skip(2) {
        assert!(
            !f.vol.host_mut().log.iter().any(|op| matches!(
                op,
                Op::Write(lba, _) | Op::Sync(lba, _) | Op::Barrier(lba, _) if lba * 512 == at
            )),
            "mirror at {at} was written to a 512 MiB device"
        );
    }
}

/// The fit rule at sizes no image uses. `>=`, as the kernel refuses a mirror
/// ending at the last byte
#[test]
fn the_fit_rule_counts_the_way_the_kernel_does() {
    assert_eq!(mirror_count(0), 0);
    assert_eq!(mirror_count(SUPER_MIRRORS[0] + SUPER_INFO_SIZE as u64), 0, "exactly touching is out");
    assert_eq!(mirror_count(SUPER_MIRRORS[0] + SUPER_INFO_SIZE as u64 + 1), 1);
    assert_eq!(mirror_count(64 * 1024 * 1024), 1, "the backup at 64 MiB does not fit in 64 MiB");
    assert_eq!(mirror_count(512 * 1024 * 1024), 2);
    assert_eq!(mirror_count(255 * (1u64 << 30)), 2, "mirror 2 is at 256 GiB");
    assert_eq!(mirror_count(300 * (1u64 << 30)), 3);

    // Three on a device of any size: `BTRFS_SUPER_MIRROR_MAX`. A fourth at 1
    // PiB would land in space `exclude_super_stripes` did not reserve
    assert_eq!(mirror_count(u64::MAX), 3, "BTRFS_SUPER_MIRROR_MAX is 3");
    assert_eq!(SUPER_MIRRORS.len(), 3, "and there is no fourth mirror to write");

    // Stopping at the first misfit is correct only because offsets ascend,
    // asserted here: `commit_superblocks` takes a prefix of `SUPER_MIRRORS`
    assert!(
        SUPER_MIRRORS.windows(2).all(|w| w[0] < w[1]),
        "the mirror offsets must ascend: {SUPER_MIRRORS:?}"
    );
}

/// The order: `barrier_all_devices`, the primary with FUA, the backups lazily.
/// The flush makes tree blocks stable before anything names them, and the FUA
/// makes the transaction a single moment
#[test]
fn the_barrier_comes_before_the_supers() {
    let mut f = fx!("order");
    f.vol.host_mut().log.clear();
    commit_superblocks(&mut f.vol, f.now).expect("a staged volume must commit");

    let log = f.vol.host_mut().log.clone();
    let first_super = log
        .iter()
        .position(|op| !matches!(op, Op::Flush))
        .expect("the commit wrote something");
    assert!(
        log[..first_super].contains(&Op::Flush),
        "no flush before the first superblock write: {log:?}"
    );

    let supers = f.vol.host_mut().super_writes();
    assert_eq!(supers.len(), 2, "a 512 MiB image takes two mirrors: {supers:?}");
    assert_eq!(
        supers[0],
        Op::Barrier(SUPER_MIRRORS[0] / 512, 8),
        "the primary copy goes down FUA and first"
    );
    assert_eq!(
        supers[1],
        Op::Write(SUPER_MIRRORS[1] / 512, 8),
        "and the backup goes down lazy, which is what the reference chooses"
    );
}

// Writing blocks

/// A tree block written back unchanged is still one, to btrfs-progs and this
/// reader
#[test]
fn a_block_written_back_unchanged_is_still_itself() {
    let mut f = fx!("rewrite");
    commit_superblocks(&mut f.vol, f.now).expect("a staged volume must commit");

    let again = f.vol.read_node(f.root).expect("the root must read").bytes().to_vec();
    write_block(&mut f.vol, f.root, &again).expect("a block already on the volume must write");

    f.vol.read_node(f.root).expect("and must still read back");
    assert_clean(&f.img.btrfs(&["check"]), "rewriting the root block");
}

/// Both copies of a DUP block are written: a reader takes whichever passes, so
/// a half-written mirrored block would give two answers
#[test]
fn both_copies_of_a_dup_block_are_written() {
    let f = fx!("dup");

    let chunk = f.vol.chunks.find(f.root).expect("the root is in a chunk");
    let copies = chunk.profile.copies();
    assert_eq!(copies, 2, "mkfs makes metadata DUP on one device - this test needs that");

    let nodesize = f.vol.geometry.nodesize as usize;
    let places: Vec<_> = (0..copies)
        .map(|m| f.vol.chunks.map(f.root, m).expect("every copy maps"))
        .collect();
    assert_ne!(places[0].offset, places[1].offset, "DUP puts the copies in different places");

    // The fixture already wrote a rolled root. Both copies must carry it
    for p in &places {
        let mut got = vec![0u8; nodesize];
        f.img.read_at(p.offset, &mut got);
        let gen = u64_at(&got, header::GENERATION).expect("a node has a generation");
        assert_eq!(gen, f.now, "the copy at {} was not written", p.offset);
        assert_eq!(
            u32_at(&got, header::CSUM).unwrap(),
            fshost::crc32c::crc32c(0, &got[CSUM_SIZE..]),
            "the copy at {} does not match its own checksum",
            p.offset
        );
    }
}

/// `seal_block` writes the address, then the checksum covering it. The sum
/// covers byte 32 on and `bytenr` is at byte 48, so a sum computed first fails
/// on read, as `write_dev_supers` and `write_one_eb` avoid.
///
/// The second half re-seals for another address, the only case where the order
/// shows
#[test]
fn a_sealed_block_carries_the_address_its_checksum_covers() {
    let mut block = vec![0u8; 16384];
    block[header::FLAGS..header::FLAGS + 8].copy_from_slice(&HEADER_FLAG_WRITTEN.to_le_bytes());

    for at in [65536u64, 1 << 30, (1 << 30) + 16384] {
        seal_block(&mut block, at).expect("sealing must work");
        assert_eq!(u64_at(&block, header::BYTENR).unwrap(), at, "the address was not written");
        assert_eq!(
            u32_at(&block, header::CSUM).unwrap(),
            fshost::crc32c::crc32c(0, &block[CSUM_SIZE..]),
            "a block sealed for {at} does not match its own checksum"
        );
    }

    // `WRITTEN` is the caller's: `seal_block` cannot know the block is filled.
    // `Transaction::seal` in `trans.rs` sets it, and `write_block` refuses a
    // block with it clear
    let mut fresh = vec![0u8; 16384];
    seal_block(&mut fresh, 65536).expect("sealing must work");
    assert_eq!(
        u64_at(&fresh, header::FLAGS).unwrap() & HEADER_FLAG_WRITTEN,
        0,
        "seal_block set WRITTEN on a block nobody said was finished"
    );

    let mut tiny = vec![0u8; 64];
    assert_eq!(
        seal_block(&mut tiny, 65536),
        Err(BtrfsError::InvalidArg),
        "there is no header to seal in 64 bytes"
    );
}

/// A block with a wrong checksum or address is refused, not repaired, which
/// would make it look right at another address
#[test]
fn write_block_refuses_a_block_it_would_have_to_fix() {
    let mut f = fx!("refuse");
    let good = f.vol.read_node(f.root).expect("the root must read").bytes().to_vec();

    let mut stale = good.clone();
    stale[header::GENERATION] ^= 0xFF;
    assert_eq!(
        write_block(&mut f.vol, f.root, &stale),
        Err(BtrfsError::ChecksumMismatch),
        "an edited block that was not re-sealed must be refused"
    );

    // Sealed for elsewhere: the sum is right and the address wrong
    let elsewhere = f.root + f.vol.geometry.nodesize as u64;
    let mut misplaced = good.clone();
    seal_block(&mut misplaced, elsewhere).expect("sealing must work");
    assert_eq!(
        write_block(&mut f.vol, f.root, &misplaced),
        Err(BtrfsError::Corrupt),
        "a block that says it lives somewhere else must be refused"
    );

    // Sealed, right address, not marked written. `seal_block` leaves the flag,
    // and `parse_node` notices, as `tree-checker.c` does
    let mut unwritten = good.clone();
    let flags = u64_at(&unwritten, header::FLAGS).unwrap() & !HEADER_FLAG_WRITTEN;
    unwritten[header::FLAGS..header::FLAGS + 8].copy_from_slice(&flags.to_le_bytes());
    seal_block(&mut unwritten, f.root).expect("sealing must work");
    assert_eq!(
        write_block(&mut f.vol, f.root, &unwritten),
        Err(BtrfsError::Corrupt),
        "a block whose WRITTEN flag is clear is one this crate would refuse to read back"
    );

    let mut short = good.clone();
    short.truncate(512);
    assert_eq!(
        write_block(&mut f.vol, f.root, &short),
        Err(BtrfsError::InvalidArg),
        "a block that is not a node is not a tree block"
    );

    // Nothing reached the medium: both copies hold what the fixture staged,
    // byte for byte
    let nodesize = f.vol.geometry.nodesize as usize;
    let copies = f.vol.chunks.find(f.root).expect("the root is in a chunk").profile.copies();
    for m in 0..copies {
        let p = f.vol.chunks.map(f.root, m).expect("every copy maps");
        let mut got = vec![0u8; nodesize];
        f.img.read_at(p.offset, &mut got);
        assert_eq!(got, good, "a refused write reached the copy at {}", p.offset);
    }
}

// the refusals

#[test]
fn the_commit_refuses_a_generation_that_does_not_move() {
    let mut f = fx!("backwards");
    let was = f.was;
    assert_eq!(
        commit_superblocks(&mut f.vol, was),
        Err(BtrfsError::InvalidArg),
        "the same generation twice is not a new transaction"
    );
    assert_eq!(
        commit_superblocks(&mut f.vol, was - 1),
        Err(BtrfsError::InvalidArg),
        "and a generation that goes backwards makes every new block fail its transid check"
    );
    assert_eq!(on_disk_generation(&f.img), was, "a refused commit wrote a superblock anyway");

    // Committing forward still works, so the refusal left nothing half done
    commit_superblocks(&mut f.vol, f.now).expect("forward must still work");
    assert_clean(&f.img.btrfs(&["check"]), "two refusals and a commit");
}

#[test]
fn the_commit_refuses_an_outstanding_tree_log() {
    let mut f = fx!("log");
    // A log root this driver can neither write nor replay; only the field being
    // set matters
    let fake = f.vol.geometry.root;
    f.vol.superblock.data[sb::LOG_ROOT..sb::LOG_ROOT + 8].copy_from_slice(&fake.to_le_bytes());
    let csum = f.vol.superblock.computed_csum();
    f.vol.superblock.data[sb::CSUM..sb::CSUM + 4].copy_from_slice(&csum.to_le_bytes());

    assert_eq!(
        commit_superblocks(&mut f.vol, f.now),
        Err(BtrfsError::Unsupported),
        "a superblock still pointing at a tree log must not be committed"
    );
    assert_eq!(on_disk_generation(&f.img), f.was, "a refused commit wrote a superblock anyway");
}

#[test]
fn the_commit_refuses_a_superblock_that_stopped_being_one() {
    let mut f = fx!("corrupt");
    // A field the volume could not have: a nodesize not a power of two, with
    // the checksum fixed up
    f.vol.superblock.data[sb::NODESIZE..sb::NODESIZE + 4].copy_from_slice(&5000u32.to_le_bytes());
    let csum = f.vol.superblock.computed_csum();
    f.vol.superblock.data[sb::CSUM..sb::CSUM + 4].copy_from_slice(&csum.to_le_bytes());

    assert_eq!(
        commit_superblocks(&mut f.vol, f.now),
        Err(BtrfsError::Corrupt),
        "the reference calls this unexpected superblock corruption before writing it"
    );
    assert_eq!(on_disk_generation(&f.img), f.was, "a refused commit wrote a superblock anyway");
}

// Crash consistency

/// A volume on a `MemDisk` with a commit's first half staged, writing nothing
/// yet
fn on_memdisk(pristine: &[u8]) -> MemDisk {
    MemDisk::from_image(pristine.to_vec())
}

/// A crash before the supers leaves the old volume, per btrfs-progs. The power
/// goes on the first write, so nothing reached the medium, and the in-place
/// artifice at the top does not apply. `btrfs check` must pass at the old
/// generation
#[test]
fn a_crash_before_anything_leaves_the_old_volume() {
    let img = match Image::new("crash-early", None) {
        Some(i) => i,
        None => {
            eprintln!("btrfs-progs not available - skipping");
            return;
        }
    };
    let pristine = std::fs::read(&img.path).expect("cannot read image");
    let mut disk = on_memdisk(&pristine);

    let medium = {
        let mut vol = Volume::open(&mut disk, 0).expect("a fresh volume must open");
        vol.load_chunk_tree().expect("the chunk tree must load");
        let was = vol.geometry.generation;
        let root = vol.geometry.root;
        let mut block = vol.read_node(root).expect("the root must read").bytes().to_vec();
        block[header::GENERATION..header::GENERATION + 8]
            .copy_from_slice(&(was + 1).to_le_bytes());
        seal_block(&mut block, root).expect("sealing must work");

        vol.host_mut().set_volatile(true);
        vol.host_mut().crash_at(0);

        // Both complete and report success, as a machine without power reports
        // nothing. `MemDisk` keeps running so the test can continue
        let _ = write_block(&mut vol, root, &block);
        let _ = commit_superblocks(&mut vol, was + 1);
        assert!(vol.host_mut().did_crash(), "the cut point must have been reached");
        vol.host_mut().crash();
        vol.host_mut().image().to_vec()
    };

    assert_eq!(medium, pristine, "a crash at the first write must leave every byte alone");
    std::fs::write(&img.path, &medium).expect("cannot write medium back");
    assert_clean(&img.btrfs(&["check"]), "a crash before the first write");
}

/// A crash between the barrier and supers leaves the old volume, per
/// btrfs-progs. The power goes after `barrier_all_devices` and before the
/// primary, so the disk holds the old filesystem plus one durable unreferenced
/// block.
///
/// The staged block goes to an address `read_free_space` calls free, as a real
/// commit does, so the old volume is untouched and `btrfs check` applies,
/// unlike the sweep below.
///
/// Without the barrier, the block would still be in the device's cache
#[test]
fn a_crash_between_the_barrier_and_the_supers_leaves_the_old_volume() {
    let img = match Image::new("crash-barrier", None) {
        Some(i) => i,
        None => {
            eprintln!("btrfs-progs not available - skipping");
            return;
        }
    };
    let pristine = std::fs::read(&img.path).expect("cannot read image");
    let mut disk = on_memdisk(&pristine);

    let (medium, was, spare, places) = {
        let mut vol = Volume::open(&mut disk, 0).expect("a fresh volume must open");
        vol.load_chunk_tree().expect("the chunk tree must load");
        let was = vol.geometry.generation;
        let root = vol.geometry.root;
        let nodesize = vol.geometry.nodesize as u64;

        // Somewhere the old volume does not reference, as a real commit's first
        // half uses
        let space = read_free_space(&mut vol).expect("a fresh volume has a free space model");
        let spare = space
            .find_free(BLOCK_GROUP_METADATA, nodesize, nodesize)
            .expect("a fresh volume has a free metadata node somewhere");
        assert_ne!(spare, root, "the staged block must not land on the root");

        let mut block = vol.read_node(root).expect("the root must read").bytes().to_vec();
        block[header::GENERATION..header::GENERATION + 8]
            .copy_from_slice(&(was + 1).to_le_bytes());
        seal_block(&mut block, spare).expect("sealing must work");

        // The cut point is the write after the staged block's copies: the
        // primary. `flush` is not a write, so this lands in the window
        let copies = vol.chunks.find(spare).expect("the spare is in a chunk").profile.copies();
        // Where those copies land on the device, found before giving up the
        // volume
        let places: Vec<u64> = (0..copies)
            .map(|m| vol.chunks.map(spare, m).expect("every copy maps").offset)
            .collect();
        vol.host_mut().set_volatile(true);
        vol.host_mut().crash_at(copies as u64);

        write_block(&mut vol, spare, &block).expect("the staged block must write");
        let _ = commit_superblocks(&mut vol, was + 1);
        assert!(vol.host_mut().did_crash(), "the cut point must have been reached");
        vol.host_mut().crash();
        (vol.host_mut().image().to_vec(), was, spare, places)
    };

    // The barrier happened, so each copy survived the cache loss. By
    // generation, not checksum: free metadata space holds earlier `mkfs` trees
    // that checksum and carry this `bytenr`. Nothing on the volume was written
    // at `was + 1`
    for off in &places {
        let got = &medium[*off as usize..*off as usize + 16384];
        assert_eq!(
            u32_at(got, header::CSUM).unwrap(),
            fshost::crc32c::crc32c(0, &got[CSUM_SIZE..]),
            "the copy of the staged block {spare} at {off} is not a whole block"
        );
        assert_eq!(
            u64_at(got, header::BYTENR).unwrap(),
            spare,
            "the copy at {off} is not a block that belongs at {spare}"
        );
        assert_eq!(
            u64_at(got, header::GENERATION).unwrap(),
            was + 1,
            "the copy of the staged block {spare} at {off} did not reach stable media - \
             there was no barrier"
        );
    }

    // And nothing names it. Every mirror still carries the old transaction
    for at in SUPER_MIRRORS.iter().copied().take(mirror_count(IMG_BYTES)) {
        let sb = read_super(&medium, at);
        assert_eq!(sb.generation(), was, "the mirror at {at} moved before the power went");
    }

    std::fs::write(&img.path, &medium).expect("cannot write medium back");
    assert_clean(&img.btrfs(&["check"]), "a crash between the barrier and the supers");

    let dump = img.btrfs(&["inspect-internal", "dump-super"]);
    assert!(
        dump.contains(&format!("generation\t\t{was}")),
        "dump-super must still show the generation we started from ({was}):\n{dump}"
    );
}

/// The new generation is not visible before its blocks. The cut sweeps every
/// write of the transaction. At each:
///
///   * the primary on the medium is whole: a 4 KiB write happened or did not;
///   * if it names the new generation, each copy of the new tree block is on
///     the medium, what
///     `barrier_all_devices` buys;
///   * if it names the old, so does each backup.
///
/// No `btrfs check` here: the new root overwrites the old, a state a real
/// commit does not produce, which progs calls a transid error. The ordering is
/// visible without it
#[test]
fn the_new_generation_is_never_visible_before_its_blocks() {
    let img = match Image::new("crash-sweep", None) {
        Some(i) => i,
        None => {
            eprintln!("btrfs-progs not available - skipping");
            return;
        }
    };
    let pristine = std::fs::read(&img.path).expect("cannot read image");

    // Four writes: two copies of the rolled root, two superblock mirrors. The
    // sweep goes one past the end to cover no cut, then a power loss with the
    // cache full
    for cut in 0..=5u64 {
        let mut disk = on_memdisk(&pristine);
        let (medium, was, root, places) = {
            let mut vol = Volume::open(&mut disk, 0).expect("a fresh volume must open");
            vol.load_chunk_tree().expect("the chunk tree must load");
            let was = vol.geometry.generation;
            let root = vol.geometry.root;
            let copies = vol.chunks.find(root).expect("the root is in a chunk").profile.copies();
            let places: Vec<u64> = (0..copies)
                .map(|m| vol.chunks.map(root, m).expect("every copy maps").offset)
                .collect();

            let mut block = vol.read_node(root).expect("the root must read").bytes().to_vec();
            block[header::GENERATION..header::GENERATION + 8]
                .copy_from_slice(&(was + 1).to_le_bytes());
            seal_block(&mut block, root).expect("sealing must work");

            vol.host_mut().set_volatile(true);
            vol.host_mut().crash_at(cut);
            let _ = write_block(&mut vol, root, &block);
            let _ = commit_superblocks(&mut vol, was + 1);
            let reached = vol.host_mut().did_crash();
            // The cache goes with the power regardless
            vol.host_mut().crash();

            if !reached {
                // Nothing was interrupted, so the primary survived the cache
                // loss, only if written FUA
                let sb = read_super(vol.host_mut().image(), SUPER_MIRRORS[0]);
                assert_eq!(
                    sb.generation(),
                    was + 1,
                    "cut {cut}: the primary superblock did not survive the power going out, \
                     so it was not written to stable media"
                );
                // The backup did not, as the reference writes it lazily and
                // accepts this window
                let backup = read_super(vol.host_mut().image(), SUPER_MIRRORS[1]);
                assert_eq!(
                    backup.generation(),
                    was,
                    "cut {cut}: the backup mirror survived, so it was not written lazily"
                );
            }
            (vol.host_mut().image().to_vec(), was, root, places)
        };

        let primary = read_super(&medium, SUPER_MIRRORS[0]);
        check_superblock(&primary, Some(SUPER_MIRRORS[0])).unwrap_or_else(|e| {
            panic!("cut {cut}: the primary superblock on the medium is not whole: {e:?}")
        });

        let gen = primary.generation();
        assert!(
            gen == was || gen == was + 1,
            "cut {cut}: the primary is at generation {gen}, which is neither transaction"
        );

        if gen == was + 1 {
            for off in &places {
                let got = &medium[*off as usize..*off as usize + 16384];
                assert_eq!(
                    u32_at(got, header::CSUM).unwrap(),
                    fshost::crc32c::crc32c(0, &got[CSUM_SIZE..]),
                    "cut {cut}: the copy of root {root} at {off} is not intact"
                );
                assert_eq!(
                    u64_at(got, header::GENERATION).unwrap(),
                    was + 1,
                    "cut {cut}: the superblock names generation {} but the block at {off} is \
                     still the old one - the barrier did not happen",
                    was + 1
                );
            }
        } else {
            let backup = read_super(&medium, SUPER_MIRRORS[1]);
            assert_eq!(
                backup.generation(),
                was,
                "cut {cut}: the primary is old and a backup is new - the primary is written first"
            );
        }
    }
}

fn read_super(medium: &[u8], at: u64) -> Superblock {
    let mut sb = Superblock::zeroed();
    sb.data
        .copy_from_slice(&medium[at as usize..at as usize + SUPER_INFO_SIZE]);
    sb
}

/// A caller editing the superblock need not re-checksum it: the checksum it
/// arrives with covers previous contents, and `seal_super` recomputes per
/// mirror
#[test]
fn a_caller_that_edits_the_superblock_need_not_re_checksum_it() {
    let mut f = fx!("nocsum");

    // As a real commit: points the superblock at a new root, leaving the
    // checksum stale
    let root = f.root;
    f.vol.superblock.data[sb::ROOT..sb::ROOT + 8].copy_from_slice(&root.to_le_bytes());
    f.vol.superblock.data[sb::CSUM..sb::CSUM + 4].copy_from_slice(&[0, 0, 0, 0]);
    assert!(
        !f.vol.superblock.csum_matches(),
        "this case is worthless unless the checksum really is wrong going in"
    );

    commit_superblocks(&mut f.vol, f.now).expect("a stale checksum must not refuse the commit");

    // And what landed is a volume, judged by somebody else
    assert_clean(&f.img.btrfs(&["check"]), "a commit with a stale incoming checksum");
    assert!(f.img.read_super().csum_matches(), "the written copy must checksum");
}

/// The judges are present. Externally judged cases skip without `btrfs-progs`,
/// so this one fails, once per file. See `common::require_tools`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}
