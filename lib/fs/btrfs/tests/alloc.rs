// Real addresses and the books afterward
//
// `tests/trans.rs` uses a `Scratch` allocator at 1 TiB, past every chunk. Here
// addresses come from the volume's metadata block groups, written into the free
// space tree and block group items before sealing.
//
//   the pin rule        a block copy-on-write freed is the crash fallback
//   the super stripes   three superblock mirrors sit inside block groups, free in the tree. An allocator
//                       believing the tree places a node where a commit writes a superblock. On the
//                       512 MiB fixture, mirror 1 at 64 MiB falls in the metadata chunk's first stripe,
//                       at logical 58720256, mid largest free run
//   the run arithmetic  an allocation mid-run leaves two runs, as `FREE_SPACE_INFO` must count;
//                       the free amount is right either way
//   `used`              allocated bytes in the block group item, cross-checked by `btrfs check` against
//                       the extent tree and the superblock's `bytes_used`
//
// No early returns: a fixture unable to give what a test needs asserts, since a
// return would pass as a skip.

mod common;

use btrfs::commit::{commit_superblocks, mirror_count, write_block};
use btrfs::error::BtrfsError;
use btrfs::space::*;
use btrfs::structs::*;
use btrfs::trans::{BlockAlloc, Trans};
use btrfs::Volume;
use common::{Image, IMG_BYTES};
use fshost::{BlockHost, HostError};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

// The host

/// A writable image file, written through so `btrfs check` sees what this crate
/// did
struct FileHost {
    file: std::fs::File,
    sectors: u64,
}

impl FileHost {
    fn open(path: &Path) -> Self {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("cannot open image for writing");
        let sectors = file.metadata().expect("cannot stat image").len() / 512;
        FileHost { file, sectors }
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
}

impl BlockHost for FileHost {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        let n = self.seek_to(lba, count)?.min(buf.len());
        self.file
            .read_exact(&mut buf[..n])
            .map_err(|_| HostError::Io)
    }
    fn write_sectors(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        let n = self.seek_to(lba, count)?;
        if buf.len() < n {
            return Err(HostError::Io);
        }
        self.file.write_all(&buf[..n]).map_err(|_| HostError::Io)
    }
    fn write_sectors_sync(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.write_sectors(lba, count, buf)
    }
    fn write_sectors_barrier(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.write_sectors(lba, count, buf)
    }
    fn flush(&mut self) -> Result<(), HostError> {
        self.file.sync_data().map_err(|_| HostError::Io)
    }
    fn total_sectors(&self) -> u64 {
        self.sectors
    }
}

// The fixture

/// An image and a volume open on it. `mkfs -r` with a directory gives more than
/// one free run in the metadata group and more than one fs tree leaf, avoiding
/// every easy branch
fn fixture(tag: &str) -> Image {
    let mut stage = std::env::temp_dir();
    stage.push(format!("btrfs-alloc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).expect("cannot make the staging directory");
    for i in 0..200 {
        std::fs::write(stage.join(format!("f{i:04}")), b"hello").expect("cannot stage a file");
    }
    let img = Image::new(tag, Some(&stage)).expect(
        "btrfs-progs must be present - `the_external_judges_are_here` says what to do about it",
    );
    let _ = std::fs::remove_dir_all(&stage);
    img
}

fn open(img: &Image) -> Volume<FileHost> {
    let mut vol = Volume::open(FileHost::open(&img.path), 0).expect("the volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");
    vol
}

/// Where the three trees this half writes live on a given volume
struct Roots {
    fst: u64,
    bg: u64,
    root_tree: u64,
    fs: u64,
}

fn roots(vol: &mut Volume<FileHost>) -> Roots {
    let bg_id = if vol.geometry.compat_ro & FEATURE_COMPAT_RO_BLOCK_GROUP_TREE != 0 {
        BLOCK_GROUP_TREE_OBJECTID
    } else {
        EXTENT_TREE_OBJECTID
    };
    Roots {
        fst: vol
            .find_root(FREE_SPACE_TREE_OBJECTID)
            .expect("a default volume has a free space tree")
            .bytenr,
        bg: vol.find_root(bg_id).expect("the block groups live somewhere").bytenr,
        root_tree: vol.geometry.root,
        fs: vol.fs_root().expect("the fs tree must be findable").bytenr,
    }
}

/// What one transaction did, kept after the transaction is gone
struct Ran {
    allocated: Vec<(u64, u64)>,
    pinned: Vec<(u64, u64)>,
    excluded: Vec<(u64, u64)>,
    /// `used` per block group, after everything
    used: Vec<(u64, u64)>,
    bytes_used: u64,
    /// Every block the transaction sealed
    sealed: Vec<u64>,
}

/// Opens a transaction, changes one thing, settles the space, and with `commit`
/// writes it all. The change copies the fs tree's path to its first key,
/// allocating only that path's blocks
fn run(img: &Image, commit: bool) -> Result<Ran, BtrfsError> {
    let mut vol = open(img);
    let r = roots(&mut vol);
    let g = vol.geometry;
    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();
    let generation = g.generation + 1;

    let alloc = allocator_for(&mut vol)?;
    let mut trans = Trans::new(alloc, g.nodesize, g.sectorsize, fsid, generation);

    {
        let nodesize = g.nodesize as usize;
        let v = &mut vol;
        let mut read = move |logical: u64| {
            let mut buf = vec![0u8; nodesize];
            v.read_logical(logical, &mut buf)?;
            Ok(buf)
        };

        // The change: a path copy through a multi-level tree
        trans.cow_path(r.fs, &Key { objectid: 0, kind: 0, offset: 0 }, &mut read)?;
        settle(&mut trans, r.fst, r.bg, r.root_tree, &mut read)?;
    }

    let mut allocated = Vec::new();
    let mut pinned = Vec::new();
    for c in trans.alloc.changes() {
        if c.alloc {
            allocated.push((c.start, c.len));
        } else {
            pinned.push((c.start, c.len));
        }
    }
    let excluded = trans.alloc.excluded().to_vec();
    let used: Vec<(u64, u64)> = trans
        .alloc
        .view()
        .groups
        .iter()
        .map(|g| (g.start, g.used))
        .collect();
    let bytes_used = trans.alloc.bytes_used();

    let blocks = trans.seal()?;
    let sealed: Vec<u64> = blocks.iter().map(|(at, _)| *at).collect();

    if commit {
        let new_root = trans
            .new_root_of(r.root_tree)
            .expect("the root tree must have moved: its items were rewritten");
        let level = blocks
            .iter()
            .find(|(at, _)| *at == new_root)
            .map(|(_, d)| d[btrfs::node::header::LEVEL])
            .expect("the new root tree block must be one this transaction sealed");

        for (at, data) in &blocks {
            write_block(&mut vol, *at, data)?;
        }

        // The three superblock fields this half owns; `commit_superblocks` does
        // the rest
        put64(&mut vol.superblock.data, sb::ROOT, new_root);
        vol.superblock.data[sb::ROOT_LEVEL] = level;
        put64(&mut vol.superblock.data, sb::BYTES_USED, bytes_used);
        reseal_super(&mut vol.superblock);
        commit_superblocks(&mut vol, generation)?;
    }

    Ok(Ran { allocated, pinned, excluded, used, bytes_used, sealed })
}

/// Restores the superblock's checksum over the changed fields, since
/// `commit_superblocks` checks what it is given first. The commit re-seals per
/// mirror afterward
fn reseal_super(sb: &mut btrfs::Superblock) {
    let csum = sb.computed_csum();
    sb.data[sb::CSUM..sb::CSUM + 4].copy_from_slice(&csum.to_le_bytes());
}

fn put64(buf: &mut [u8], at: usize, v: u64) {
    buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn overlaps(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.0 + b.1 && b.0 < a.0 + a.1
}

// The allocator

/// Makes an absent toolchain loud: every fixture here uses `mkfs.btrfs`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}

/// Each address is in a metadata block group, nodesize aligned, inside a run
/// the free space tree called free
#[test]
fn what_is_handed_out_is_metadata_space_that_was_free() {
    let img = fixture("where");
    let mut vol = open(&img);
    let space = read_free_space(&mut vol).expect("the free space must read");
    let nodesize = vol.geometry.nodesize as u64;
    drop(vol);

    let ran = run(&img, false).expect("the transaction must settle");
    assert!(
        ran.allocated.len() >= 2,
        "this fixture must allocate more than one block or nothing below is being tested: {:?}",
        ran.allocated
    );

    for &(at, len) in &ran.allocated {
        assert_eq!(len, nodesize, "a tree block is one nodesize");
        assert_eq!(at % nodesize, 0, "{at} is not nodesize aligned");
        let g = space
            .groups
            .iter()
            .find(|g| at >= g.start && at < g.end())
            .unwrap_or_else(|| panic!("{at} is in no block group at all"));
        assert!(
            g.is_metadata(),
            "{at} came out of a {} group, not a metadata one",
            g.flags
        );
        assert!(
            g.runs.iter().any(|&(rs, rl)| at >= rs && at + len <= rs + rl),
            "{at} was not inside any free run of its group: {:?}",
            g.runs
        );
    }
}

/// Rule 1: a block copy-on-write freed may not be reused before commit.
/// `Trans::check_fresh` refuses one too, so a violation fails here and in the
/// transaction
#[test]
fn a_block_this_transaction_pinned_is_never_handed_out_again() {
    let img = fixture("pin");
    let ran = run(&img, false).expect("the transaction must settle");

    assert!(
        !ran.pinned.is_empty(),
        "nothing was pinned, so this case would pass without testing anything"
    );
    for &p in &ran.pinned {
        for &a in &ran.allocated {
            assert!(
                !overlaps(a, p),
                "{a:?} was handed out over pinned {p:?} - the old tree lives there until the commit"
            );
        }
    }
}

/// `exclude_super_stripes`: the three mirrors are free in the tree and must not
/// be allocated. On 512 MiB, mirror 1 is at 64 MiB on the device, the metadata
/// chunk's first stripe starts at 38797312, so it falls at logical 58720256,
/// inside the group's largest free run, where first fit goes
#[test]
fn a_superblock_mirror_is_never_handed_out() {
    let img = fixture("super");
    let mut vol = open(&img);
    let space = read_free_space(&mut vol).expect("the free space must read");
    drop(vol);

    let ran = run(&img, false).expect("the transaction must settle");

    assert_eq!(
        ran.excluded.len(),
        1,
        "a 512 MiB volume has exactly one superblock mirror inside a chunk; \
         this fixture must keep it or the case proves nothing: {:?}",
        ran.excluded
    );
    let stripe = ran.excluded[0];
    assert_eq!(stripe.1, 65536, "a super stripe is BTRFS_STRIPE_LEN long");

    // It is inside free space, so a trusting allocator would hand it out
    assert!(
        space
            .groups
            .iter()
            .any(|g| g.runs.iter().any(|&(rs, rl)| overlaps(stripe, (rs, rl)))),
        "the excluded stripe {stripe:?} is not inside any free run - this case \
         has stopped testing the thing it names"
    );

    for &a in &ran.allocated {
        assert!(
            !overlaps(a, stripe),
            "{a:?} was allocated over superblock mirror stripe {stripe:?}"
        );
    }
}

/// `BTRFS_SUPER_MIRROR_MAX` is 3. A fourth excludes a range no superblock uses;
/// a missing third costs a filesystem on a large volume
#[test]
fn the_mirror_count_is_three_and_only_those_that_fit_are_excluded() {
    assert_eq!(SUPER_MIRRORS.len(), 3, "BTRFS_SUPER_MIRROR_MAX is 3");
    assert_eq!(mirror_count(IMG_BYTES), 2, "a 512 MiB volume has two mirrors");

    let img = fixture("mirrors");
    let vol = open(&img);
    let chunks: Vec<_> = vol.chunks.iter().cloned().collect();
    drop(vol);

    // The metadata chunk at each mirror count. Only the 64 MiB one is inside,
    // so the answer moves once between primary and primary plus first backup
    let meta = chunks
        .iter()
        .find(|c| c.is_metadata())
        .expect("a volume has a metadata chunk");
    let end = meta.start + meta.length;
    let counts: Vec<usize> = (0..=3)
        .map(|n| super_stripes(meta, end, n).len())
        .collect();
    assert_eq!(counts, vec![0, 0, 1, 1], "mirror 1 is the only one inside this chunk");
}

/// One address, once
#[test]
fn the_same_address_is_never_handed_out_twice() {
    let img = fixture("twice");
    let ran = run(&img, false).expect("the transaction must settle");
    let mut seen = HashSet::new();
    for &(at, _) in &ran.allocated {
        assert!(seen.insert(at), "{at} was handed out twice");
    }
    assert_eq!(seen.len(), ran.allocated.len());
}

/// No chunk is allocated: a metadata group with no room refuses
#[test]
fn an_allocator_with_no_room_refuses_instead_of_growing_the_volume() {
    let img = fixture("full");
    let mut vol = open(&img);
    let mut space = read_free_space(&mut vol).expect("the free space must read");
    let nodesize = vol.geometry.nodesize;
    let mirrors = mirror_count(IMG_BYTES);

    // Take every metadata run away, leaving the data and system groups alone
    for g in space.groups.iter_mut() {
        if g.is_metadata() {
            g.runs.clear();
        }
    }
    assert!(
        space.groups.iter().any(|g| g.is_data() && !g.runs.is_empty()),
        "the data group must still have room, or this proves only that the volume is full"
    );

    let mut alloc = Allocator::new(space, &vol.chunks, nodesize, mirrors)
        .expect("the allocator must build");
    assert_eq!(
        alloc.alloc(nodesize as u64),
        Err(BtrfsError::Unsupported),
        "with no metadata room the answer is the unimplemented chunk allocator, \
         not a data block and not a panic"
    );
}

/// A data extent is another request, and a metadata group would be the wrong
/// chunk
#[test]
fn an_allocation_that_is_not_one_tree_block_is_refused() {
    let img = fixture("size");
    let mut vol = open(&img);
    let mut alloc = allocator_for(&mut vol).expect("the allocator must build");
    let nodesize = vol.geometry.nodesize as u64;

    assert_eq!(alloc.alloc(nodesize / 2), Err(BtrfsError::InvalidArg));
    assert_eq!(alloc.alloc(nodesize * 2), Err(BtrfsError::InvalidArg));
    assert!(alloc.alloc(nodesize).is_ok(), "one nodesize is the request that works");
}

// ==================================================== the free space tree

/// The whole loop judged by this crate's reader on the committed volume.
/// `GroupScan::finish` checks the `FREE_SPACE_INFO` run count against runs
/// found, and the sum of free runs against `length - used`
#[test]
fn the_space_reads_back_after_a_transaction() {
    let img = fixture("readback");

    let mut vol = open(&img);
    let before = read_free_space(&mut vol).expect("the free space must read before");
    drop(vol);

    let ran = run(&img, true).expect("the transaction must settle and commit");

    let mut vol = open(&img);
    let after = read_free_space(&mut vol).expect(
        "the free space tree this transaction wrote must read back - a failure here is \
         the run count or the length-used sum, and `GroupScan::finish` says which",
    );

    assert_eq!(after.groups.len(), before.groups.len());
    for (a, b) in after.groups.iter().zip(before.groups.iter()) {
        assert_eq!(a.start, b.start);
        assert_eq!(a.length, b.length);
        assert_eq!(
            a.used,
            ran.used
                .iter()
                .find(|(s, _)| *s == a.start)
                .expect("every group was accounted")
                .1,
            "group at {}: the block group item does not say what the allocator counted",
            a.start
        );
    }

    // Allocating more than was pinned leaves less free space by the difference
    let took: u64 = ran.allocated.iter().map(|&(_, l)| l).sum();
    let gave: u64 = ran.pinned.iter().map(|&(_, l)| l).sum();
    assert_eq!(
        after.total_free(BLOCK_GROUP_METADATA) + took,
        before.total_free(BLOCK_GROUP_METADATA) + gave,
        "the metadata free space moved by something other than what was allocated and pinned"
    );
    assert_eq!(
        ran.bytes_used,
        after.groups.iter().map(|g| g.used).sum::<u64>(),
        "the superblock's bytes_used is the sum of every group's used"
    );
}

/// Each sealed block is used in the committed tree and each pinned block free,
/// as `btrfs check` compares between the free space and extent trees
#[test]
fn what_was_written_is_used_and_what_was_pinned_is_free() {
    let img = fixture("cover");
    let ran = run(&img, true).expect("the transaction must settle and commit");

    let mut vol = open(&img);
    let after = read_free_space(&mut vol).expect("the free space must read back");

    let free_at = |at: u64, len: u64| {
        after
            .groups
            .iter()
            .any(|g| g.runs.iter().any(|&(rs, rl)| at >= rs && at + len <= rs + rl))
    };

    assert!(!ran.sealed.is_empty(), "a transaction that sealed nothing tests nothing");
    for &at in &ran.sealed {
        assert!(
            !free_at(at, 1),
            "{at} holds a block this transaction wrote and the free space tree calls it free"
        );
    }
    assert!(!ran.pinned.is_empty(), "nothing was pinned");
    for &(at, len) in &ran.pinned {
        assert!(
            free_at(at, len),
            "{at} was freed by copy-on-write and the free space tree still calls it used"
        );
    }
}

/// An allocation mid-run leaves a run each side, and `FREE_SPACE_INFO` must say
/// two for one. Driven directly: first fit allocates at a run's start, case 2
/// of `remove_free_space_extent`
#[test]
fn a_run_cut_in_the_middle_becomes_two_runs_and_is_counted_as_two() {
    let img = fixture("split");
    let mut vol = open(&img);
    let r = roots(&mut vol);
    let g = vol.geometry;
    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();

    let space = read_free_space(&mut vol).expect("the free space must read");
    let group = space
        .groups
        .iter()
        .find(|g| g.is_metadata())
        .expect("a metadata group")
        .clone();
    // The largest run, and a nodesize well inside it
    let (rs, rl) = *group
        .runs
        .iter()
        .max_by_key(|&&(_, l)| l)
        .expect("the metadata group has free runs");
    assert!(
        rl > 4 * g.nodesize as u64,
        "the run picked is too small to have a middle: {rl}"
    );
    let cut = rs + 2 * g.nodesize as u64;
    let runs_before = group.runs.len();

    let alloc = allocator_for(&mut vol).expect("the allocator must build");
    let mut trans = Trans::new(alloc, g.nodesize, g.sectorsize, fsid, g.generation + 1);

    let after_runs;
    let after_count;
    {
        let nodesize = g.nodesize as usize;
        let v = &mut vol;
        let mut read = move |logical: u64| {
            let mut buf = vec![0u8; nodesize];
            v.read_logical(logical, &mut buf)?;
            Ok(buf)
        };

        // One range, straight into the tree, with nothing else going on
        remove_free_space(
            &mut trans,
            r.fst,
            &group,
            cut,
            g.nodesize as u64,
            &mut read,
        )
        .expect("cutting a run in the middle must work");

        let (items, count) = free_space_items(&trans, r.fst, &group, &mut read);
        after_runs = items;
        after_count = count;
        let _ = r.bg;
        let _ = r.root_tree;
    }

    assert_eq!(
        after_runs.len(),
        runs_before + 1,
        "cutting a run in the middle must leave one more run than there was: {after_runs:?}"
    );
    assert!(
        after_runs.contains(&(rs, cut - rs)),
        "the piece before the cut is missing: {after_runs:?}"
    );
    assert!(
        after_runs.contains(&(cut + g.nodesize as u64, rs + rl - cut - g.nodesize as u64)),
        "the piece after the cut is missing: {after_runs:?}"
    );
    assert_eq!(
        after_count as usize, after_runs.len(),
        "the FREE_SPACE_INFO count is the one check that catches a split nobody counted"
    );
}

/// Returning a range next to a run ending at its start makes one run, and the
/// count drops. Adjacent runs fail a joined allocation, and the kernel's
/// checker refuses the leaf
#[test]
fn a_range_given_back_between_two_runs_merges_into_one() {
    let img = fixture("merge");
    let mut vol = open(&img);
    let r = roots(&mut vol);
    let g = vol.geometry;
    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();
    let node = g.nodesize as u64;

    let space = read_free_space(&mut vol).expect("the free space must read");
    let group = space
        .groups
        .iter()
        .find(|g| g.is_metadata())
        .expect("a metadata group")
        .clone();
    let (rs, rl) = *group
        .runs
        .iter()
        .max_by_key(|&&(_, l)| l)
        .expect("the metadata group has free runs");
    assert!(rl > 4 * node, "the run picked is too small: {rl}");
    let cut = rs + 2 * node;

    let alloc = allocator_for(&mut vol).expect("the allocator must build");
    let mut trans = Trans::new(alloc, g.nodesize, g.sectorsize, fsid, g.generation + 1);

    let runs;
    let count;
    {
        let nodesize = g.nodesize as usize;
        let v = &mut vol;
        let mut read = move |logical: u64| {
            let mut buf = vec![0u8; nodesize];
            v.read_logical(logical, &mut buf)?;
            Ok(buf)
        };

        // Cut the middle out and return it: the tree must be where it started,
        // case 4 of `add_free_space_extent`, with a neighbor each side
        remove_free_space(&mut trans, r.fst, &group, cut, node, &mut read)
            .expect("the cut must work");
        add_free_space(&mut trans, r.fst, &group, cut, node, &mut read)
            .expect("giving it back must work");

        let (items, c) = free_space_items(&trans, r.fst, &group, &mut read);
        runs = items;
        count = c;
    }

    assert!(
        runs.contains(&(rs, rl)),
        "the run was not put back whole - the two halves did not merge: {runs:?}"
    );
    assert_eq!(runs.len(), group.runs.len(), "and no run was gained or lost");
    assert_eq!(count as usize, runs.len(), "nor did the count drift from the items");
}

/// Allocating and freeing one address leaves the books unchanged.
/// `Trans::discard_block`'s path, where the simplification in `space.rs` shows:
/// the range leaves the tree and returns
#[test]
fn a_block_allocated_and_freed_leaves_used_where_it_was() {
    let img = fixture("cancel");
    let mut vol = open(&img);
    let nodesize = vol.geometry.nodesize as u64;
    let mut alloc = allocator_for(&mut vol).expect("the allocator must build");

    let before = alloc.bytes_used();
    let at = alloc.alloc(nodesize).expect("one block must be free");
    assert_eq!(alloc.bytes_used(), before + nodesize);
    alloc.free(at, nodesize);
    assert_eq!(alloc.bytes_used(), before, "used must come back to where it started");

    // Still not given out again, as it is pinned
    let next = alloc.alloc(nodesize).expect("another block must be free");
    assert_ne!(next, at, "a freed block is pinned until the commit, however it was freed");
}

// Reading the tree back
//
// Helpers that read the free space tree as the transaction has it, before any
// commit

/// Every `FREE_SPACE_EXTENT` of one group, and the count `FREE_SPACE_INFO`
/// states
fn free_space_items<A: BlockAlloc>(
    trans: &Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    read: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>,
) -> (Vec<(u64, u64)>, u32) {
    let mut runs = Vec::new();
    let mut count = None;

    walk(trans, fst_root, read, &mut |k: Key, data: &[u8]| {
        if k.objectid == group.start
            && k.kind == FREE_SPACE_INFO_KEY
            && k.offset == group.length
        {
            count = Some(u32::from_le_bytes(data[0..4].try_into().unwrap()));
        }
        if k.kind == FREE_SPACE_EXTENT_KEY
            && k.objectid >= group.start
            && k.objectid < group.end()
        {
            runs.push((k.objectid, k.offset));
        }
    });

    runs.sort();
    (runs, count.expect("the group must have a free space info item"))
}

/// Walk a tree as the transaction sees it
fn walk<A: BlockAlloc>(
    trans: &Trans<A>,
    root: u64,
    read: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>,
    each: &mut dyn FnMut(Key, &[u8]),
) {
    fn go<A: BlockAlloc>(
        trans: &Trans<A>,
        at: u64,
        read: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>,
        each: &mut dyn FnMut(Key, &[u8]),
        depth: usize,
    ) {
        assert!(depth < 16, "a tree that deep is a loop");
        let node = trans.node_at(at, read).expect("every block of the tree must parse");
        if node.is_leaf() {
            for slot in 0..node.nritems {
                let k = node.key_at(slot).unwrap();
                let d = node.item_data(slot).unwrap().to_vec();
                each(k, &d);
            }
            return;
        }
        for slot in 0..node.nritems {
            go(trans, node.blockptr_at(slot).unwrap(), read, each, depth + 1);
        }
    }
    go(trans, trans.new_root_of(root).unwrap_or(root), read, each, 0);
}


// The bitmap form
//
// `mkfs.btrfs` will not produce one, as a fresh volume has too few free runs,
// so `common::into_bitmaps` re-encodes a real one

/// A volume with a bitmap free space tree. A second file, since `Image`'s
/// `Drop` cleans the temporary directory
fn bitmap_fixture(tag: &str) -> Image {
    let img = fixture(tag);
    let bytes = std::fs::read(&img.path).expect("cannot read the image");
    let rewritten = common::into_bitmaps(&bytes);
    let path = img.path.with_extension("bitmaps.img");
    std::fs::write(&path, &rewritten).expect("cannot write the bitmap image");
    drop(img);
    Image { path }
}

/// Each group of the fixture is in bitmap form, and this reader agrees with
/// `btrfs check`. Everything below depends on it
#[test]
fn the_bitmap_fixture_is_really_in_bitmap_form() {
    let img = bitmap_fixture("bm-fixture");
    let dump = img.btrfs(&["inspect-internal", "dump-tree", "-t", "10"]);
    assert!(
        dump.contains("FREE_SPACE_BITMAP"),
        "the fixture is not in bitmap form:\n{dump}"
    );
    assert!(
        !dump.contains("FREE_SPACE_EXTENT"),
        "the fixture is in both forms at once:\n{dump}"
    );
    let check = img.btrfs(&["check"]);
    assert!(
        check.contains("no error found"),
        "btrfs check rejected the bitmap fixture before anything was written to it:\n{check}"
    );
}

/// The same whole loop as `the_space_reads_back_after_a_transaction` in the
/// other representation. A wrong bit order is caught by `GroupScan::finish`
/// twice
#[test]
fn the_space_reads_back_after_a_transaction_on_a_bitmap_volume() {
    let img = bitmap_fixture("bm-readback");

    let mut vol = open(&img);
    let before = read_free_space(&mut vol).expect("the bitmap free space must read before");
    drop(vol);

    let ran = run(&img, true).expect("the transaction must settle and commit");

    let mut vol = open(&img);
    let after = read_free_space(&mut vol).expect(
        "the bitmaps this transaction wrote must read back - a failure here is the bit \
         order, the run count, or the length-used sum",
    );

    let took: u64 = ran.allocated.iter().map(|&(_, l)| l).sum();
    let gave: u64 = ran.pinned.iter().map(|&(_, l)| l).sum();
    assert_eq!(
        after.total_free(BLOCK_GROUP_METADATA) + took,
        before.total_free(BLOCK_GROUP_METADATA) + gave,
    );
    for (a, b) in after.groups.iter().zip(before.groups.iter()) {
        assert_eq!(a.start, b.start);
        assert_eq!(
            a.used,
            ran.used.iter().find(|(s, _)| *s == a.start).unwrap().1,
            "group at {}: the block group item does not say what the allocator counted",
            a.start
        );
    }
}

/// The bit order, directly: one sector taken out of a bitmap group clears the
/// bit at `index % 8` of its byte, not `7 - index % 8`. The byte is read back
/// and compared, since reversal keeps the free total
#[test]
fn a_bitmap_clears_the_bit_of_the_sector_and_not_its_mirror() {
    let img = bitmap_fixture("bm-bit");
    let mut vol = open(&img);
    let r = roots(&mut vol);
    let g = vol.geometry;
    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();
    let sector = g.sectorsize as u64;

    let space = read_free_space(&mut vol).expect("the free space must read");
    let group = space
        .groups
        .iter()
        .find(|g| g.is_metadata())
        .expect("a metadata group")
        .clone();
    let (rs, rl) = *group.runs.iter().max_by_key(|&&(_, l)| l).unwrap();
    assert!(rl > 16 * sector, "the run picked is too small: {rl}");

    // A sector whose bit index is not its own mirror: index 3 mirrors to 4
    let at = rs + 3 * sector;
    let want = ((at - group.start) / sector) as usize;
    assert_ne!(want % 8, 7 - want % 8, "pick a sector whose bit is not self-mirrored");

    let alloc = allocator_for(&mut vol).expect("the allocator must build");
    let mut trans = Trans::new(alloc, g.nodesize, g.sectorsize, fsid, g.generation + 1);

    let (before, after) = {
        let nodesize = g.nodesize as usize;
        let v = &mut vol;
        let mut read = move |logical: u64| {
            let mut buf = vec![0u8; nodesize];
            v.read_logical(logical, &mut buf)?;
            Ok(buf)
        };
        let before = bitmap_bytes(&trans, r.fst, group.start, &mut read);
        remove_free_space(&mut trans, r.fst, &group, at, sector, &mut read)
            .expect("taking one sector out of a bitmap group must work");
        let after = bitmap_bytes(&trans, r.fst, group.start, &mut read);
        (before, after)
    };

    assert_eq!(before.len(), after.len(), "a bitmap item never changes size");
    let changed: Vec<usize> = (0..before.len() * 8)
        .filter(|i| (before[i / 8] >> (i % 8) & 1) != (after[i / 8] >> (i % 8) & 1))
        .collect();
    assert_eq!(
        changed,
        vec![want],
        "exactly the bit of the allocated sector must change, least significant bit \
         first within its byte - byte {} went 0x{:02x} -> 0x{:02x}",
        want / 8,
        before[want / 8],
        after[want / 8]
    );
}

/// The bitmap item covering the start of a group, as bytes
fn bitmap_bytes<A: BlockAlloc>(
    trans: &Trans<A>,
    fst_root: u64,
    group_start: u64,
    read: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>,
) -> Vec<u8> {
    let mut found = None;
    walk(trans, fst_root, read, &mut |k: Key, data: &[u8]| {
        if k.kind == FREE_SPACE_BITMAP_KEY && k.objectid == group_start {
            found = Some(data.to_vec());
        }
    });
    found.expect("the group must have a bitmap item starting at its first byte")
}

// External judges

/// btrfs-progs reading our bytes: each `FREE_SPACE_EXTENT` in the committed
/// tree, as `btrfs inspect-internal dump-tree` prints it, against this crate's
/// reading. `tests/space.rs` does this for a `mkfs` volume; this is for one
/// written here
#[test]
fn btrfs_progs_reads_back_the_free_space_tree_we_wrote() {
    let img = fixture("progs");
    run(&img, true).expect("the transaction must settle and commit");

    let mut vol = open(&img);
    let ours = read_free_space(&mut vol).expect("our own reader must read it back");
    drop(vol);

    let dump = img.btrfs(&["inspect-internal", "dump-tree", "-t", "10"]);
    assert!(
        !dump.contains("ERROR") && !dump.contains("corrupt"),
        "btrfs-progs would not read the free space tree we wrote:\n{dump}"
    );

    let theirs = progs_free_runs(&dump);
    let mine: Vec<(u64, u64)> = ours
        .groups
        .iter()
        .flat_map(|g| g.runs.iter().copied())
        .collect();
    assert!(!mine.is_empty(), "a volume with no free space at all tests nothing");
    assert_eq!(
        theirs, mine,
        "btrfs-progs and this crate read different free runs out of the tree this \
         crate wrote"
    );

    // And the counts, which are the half a run list cannot disagree about
    for (start, count) in progs_extent_counts(&dump) {
        let g = ours
            .groups
            .iter()
            .find(|g| g.start == start)
            .unwrap_or_else(|| panic!("btrfs-progs saw a group at {start} that we did not"));
        assert_eq!(
            g.runs.len(),
            count,
            "group at {start}: the FREE_SPACE_INFO count we wrote is not the number of \
             runs we wrote"
        );
    }
}

/// `btrfs check` on a volume this half committed alone, with the extent tree
/// left behind. `run` leaves each moved block's `METADATA_ITEM` and
/// `TREE_BLOCK_REF` at the old address; `tests/extent.rs` judges both. Here
/// btrfs check may complain only at the addresses this transaction moved
#[test]
fn btrfs_check_blames_the_free_space_tree_only_where_the_extent_tree_is_behind() {
    let img = fixture("check");
    let ran = run(&img, true).expect("the transaction must settle and commit");

    let out = img.btrfs(&["check"]);
    let phase = between(&out, "checking free space tree", "] checking fs roots");

    let touched: Vec<(u64, u64)> = ran
        .allocated
        .iter()
        .chain(ran.pinned.iter())
        .copied()
        .collect();
    assert!(!touched.is_empty());

    let mut complaints = 0;
    for line in phase.lines().map(str::trim).filter(|l| !l.is_empty()) {
        complaints += 1;
        let numbers = numbers_in(line);
        assert!(
            !numbers.is_empty(),
            "btrfs check said something about the free space tree that names no address, \
             so this case cannot tell whose fault it is:\n{line}\n--- whole check ---\n{out}"
        );
        assert!(
            numbers.iter().any(|&n| {
                touched.iter().any(|&(s, l)| n >= s && n < s + l)
                    || ran.used.iter().any(|&(s, _)| n == s)
            }),
            "btrfs check blames the free space tree at an address this transaction never \
             touched:\n{line}\nallocated {:?}\npinned {:?}\n--- whole check ---\n{out}",
            ran.allocated,
            ran.pinned
        );
    }
    assert!(
        complaints > 0,
        "btrfs check found nothing to say about the free space tree. That would be \
         excellent news and it would mean this case has stopped describing reality - \
         read the doc comment and rewrite it rather than deleting the assert:\n{out}"
    );
}

// Reading a dump

/// Every `FREE_SPACE_EXTENT` key in a `dump-tree` of the free space tree, as
/// `(start, length)` in key order
fn progs_free_runs(dump: &str) -> Vec<(u64, u64)> {
    dump.lines()
        .filter(|l| l.contains("FREE_SPACE_EXTENT"))
        .map(|l| {
            let n = key_numbers(l);
            assert_eq!(n.len(), 2, "cannot read a free space extent key out of: {l}");
            (n[0], n[1])
        })
        .collect()
}

/// `(block group start, extent count)` for each `FREE_SPACE_INFO` in a dump
fn progs_extent_counts(dump: &str) -> Vec<(u64, usize)> {
    let lines: Vec<&str> = dump.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !l.contains("FREE_SPACE_INFO") {
            continue;
        }
        let start = key_numbers(l)[0];
        let body = lines
            .get(i + 1)
            .expect("a FREE_SPACE_INFO item is printed over two lines");
        assert!(body.contains("extent count"), "unexpected dump shape: {body}");
        let count = numbers_in(body)[0] as usize;
        out.push((start, count));
    }
    out
}

/// The two numbers of a `key (objectid TYPE offset)` as `dump-tree` prints it.
/// The whole line also has the slot index and item offset
fn key_numbers(line: &str) -> Vec<u64> {
    let a = line.find("key (").expect("a dump line with an item key");
    let rest = &line[a + 5..];
    let b = rest.find(')').expect("an unterminated key");
    numbers_in(&rest[..b])
}

/// The text between two markers, or "" if the first is absent
fn between<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let Some(a) = text.find(from) else { return "" };
    let rest = &text[a + from.len()..];
    let rest = &rest[rest.find('\n').map(|n| n + 1).unwrap_or(rest.len())..];
    match rest.find(to) {
        Some(b) => &rest[..rest[..b].rfind('\n').map(|n| n + 1).unwrap_or(b)],
        None => rest,
    }
}

/// Every run of digits in a line as numbers; `dump-tree` and `check` print
/// addresses inline among words
fn numbers_in(line: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in line.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(n) = cur.parse() {
                out.push(n);
            }
            cur.clear();
        }
    }
    if let Ok(n) = cur.parse::<u64>() {
        out.push(n);
    }
    out
}

// The bitmap form end to end

/// Both halves over a bitmap free space tree, and `btrfs check`. The driver is
/// written out since the one in `tests/extent.rs` is private; passes, settle,
/// and the bound come from the two modules
fn commit_both_halves(img: &Image) -> String {
    let mut vol = open(img);
    let g = vol.geometry;
    let generation = g.generation + 1;
    let root_tree = g.root;
    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();

    let ext = btrfs::extent::ExtentTree::new(
        vol.find_root(EXTENT_TREE_OBJECTID).expect("an extent tree").bytenr,
        g.incompat,
    )
    .expect("a volume mkfs made must be supported");
    let r = roots(&mut vol);

    let alloc = allocator_for(&mut vol).expect("the allocator must build");
    let mut trans = Trans::new(alloc, g.nodesize, g.sectorsize, fsid, generation);

    let (sealed, used) = {
        let nodesize = g.nodesize as usize;
        let v = &mut vol;
        let mut read = move |logical: u64| {
            let mut buf = vec![0u8; nodesize];
            v.read_logical(logical, &mut buf)?;
            Ok(buf)
        };

        trans
            .cow_path(r.fs, &Key { objectid: 0, kind: 0, offset: 0 }, &mut read)
            .expect("the change must apply");

        let mut state = btrfs::extent::ExtentSettle::new();
        let mut settled = false;
        for _ in 0..btrfs::extent::MAX_SETTLE_PASSES {
            let mut changed = trans
                .extent_pass(&ext, &mut state, &mut read)
                .expect("an extent pass");
            let before = trans.dirty_count();
            settle(&mut trans, r.fst, r.bg, root_tree, &mut read)
                .expect("the free space half must settle");
            changed += trans.dirty_count() - before;
            if changed == 0 {
                settled = true;
                break;
            }
        }
        assert!(settled, "the two halves together did not settle");
        let used = trans.alloc.bytes_used();
        (trans.seal().expect("every block must seal"), used)
    };

    let new_root_tree = trans.new_root_of(root_tree).expect("the root tree moved");
    for (at, data) in &sealed {
        write_block(&mut vol, *at, data).expect("a sealed block must write");
    }
    let level = vol.read_node(new_root_tree).expect("the new root tree must read").level;
    put64(&mut vol.superblock.data, sb::ROOT, new_root_tree);
    vol.superblock.data[sb::ROOT_LEVEL] = level;
    put64(&mut vol.superblock.data, sb::BYTES_USED, used);
    reseal_super(&mut vol.superblock);
    commit_superblocks(&mut vol, generation).expect("the superblocks must commit");
    drop(vol);

    img.btrfs(&["check"])
}

/// The gate on the form `mkfs` does not produce. A reversed byte is a
/// permutation, which this reader can miss; btrfs-progs compares its own
/// picture from the extent tree
#[test]
fn btrfs_check_reports_a_bitmap_volume_both_halves_committed_clean() {
    let img = bitmap_fixture("bm-judge");
    let before = img.btrfs(&["check"]);
    assert!(
        before.contains("no error found"),
        "the bitmap fixture was already broken before anything was written:\n{before}"
    );

    let out = commit_both_halves(&img);
    assert!(
        out.contains("no error found") && !out.contains("ERROR"),
        "btrfs check did not call the committed bitmap volume clean:\n{out}"
    );
}

/// The same exclusion as an allocator meets it: first fit reaches the mirror's
/// address after about 1700 blocks and must step over it
#[test]
fn an_allocator_that_walks_the_whole_group_steps_over_the_mirror() {
    let img = fixture("walk");
    let mut vol = open(&img);
    let nodesize = vol.geometry.nodesize as u64;
    let mut alloc = allocator_for(&mut vol).expect("the allocator must build");

    let stripe = *alloc
        .excluded()
        .first()
        .expect("this volume has a superblock mirror inside a chunk");

    let mut handed = Vec::new();
    let mut crossed = false;
    for _ in 0..100_000 {
        match alloc.alloc(nodesize) {
            Ok(at) => {
                assert!(
                    !overlaps((at, nodesize), stripe),
                    "allocation {at} landed on superblock mirror stripe {stripe:?} \
                     after {} blocks",
                    handed.len()
                );
                if at > stripe.0 + stripe.1 {
                    crossed = true;
                }
                handed.push(at);
            }
            // The declared limit: the group is used up and no chunk is made
            Err(BtrfsError::Unsupported) => break,
            Err(e) => panic!("the allocator failed with {e:?} after {} blocks", handed.len()),
        }
    }
    assert!(
        crossed,
        "the allocator never got as far as the mirror at {stripe:?}, so nothing was stepped \
         over - it handed out {} blocks ending at {:?}",
        handed.len(),
        handed.last()
    );
    // What was given out is the group minus the mirror
    let excluded: u64 = alloc.excluded().iter().map(|&(_, l)| l).sum();
    let free_before = read_free_space(&mut vol)
        .expect("the free space must read")
        .total_free(BLOCK_GROUP_METADATA);
    let alignment_loss = free_before - excluded - handed.len() as u64 * nodesize;
    assert!(
        alignment_loss < nodesize * 4,
        "the allocator left {alignment_loss} bytes of the metadata group unused, which is \
         more than nodesize alignment can explain"
    );
}
