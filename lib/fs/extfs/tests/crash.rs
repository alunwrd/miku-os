// The journal, tested by cutting the power
//
// `MemDisk` models a device with a volatile write cache; `crash_at(n)` cuts the
// power after the n-th write. The image is then mounted, replaying the journal,
// and handed to `e2fsck`. The sweep covers every cut point in the operation.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SECTORS: usize = 64 * 1024;
const BLOCK_SIZE: u32 = 1024;

fn have_e2fsck() -> bool {
    Command::new("e2fsck")
        .arg("-V")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty())
        .unwrap_or(false)
}

fn dump(disk: &mut MemDisk, tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("extfs-crash-{tag}-{}.img", std::process::id()));
    let mut f = std::fs::File::create(&path).expect("cannot create image file");
    f.write_all(disk.image_flushed()).expect("cannot write image");
    f.sync_all().ok();
    path
}

fn cleanup(path: &Path) {
    if std::env::var_os("EXTFS_KEEP_IMAGES").is_some() {
        eprintln!("kept image: {}", path.display());
        return;
    }
    let _ = std::fs::remove_file(path);
}

fn fsck(path: &Path) -> (i32, String) {
    let out = Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg(path)
        .output()
        .expect("e2fsck did not run");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), report)
}

/// What `e2fsck` actually complained about, without its running commentary
fn complaint(report: &str) -> String {
    let mut lines: Vec<&str> = report
        .lines()
        .map(|l| l.trim())
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("e2fsck ")
                && !l.starts_with("Pass ")
                && !l.starts_with("Fix? ")
                && !l.starts_with("Clear? ")
                && !l.starts_with("Salvage? ")
                && !l.starts_with("Free blocks count")
                && !l.starts_with("Free inodes count")
                && !l.contains("files (")
                && !l.contains("FILE SYSTEM WAS MODIFIED")
                && !l.contains("WARNING: Filesystem still has errors")
        })
        .collect();
    lines.truncate(2);
    if lines.is_empty() {
        return String::from("(no message)");
    }
    lines.join(" / ")
}

/// A journalled filesystem with `setup` already durably on it
fn prepared<F>(setup: F) -> MemDisk
where
    F: FnOnce(&mut MikuFS<&mut MemDisk>),
{
    prepared_on(FsType::Ext4, true, setup)
}

/// The same on a filesystem of a given flavor, optionally with no directory
/// index, so a directory grows the long way
fn prepared_on<F>(kind: FsType, dir_index: bool, setup: F) -> MemDisk
where
    F: FnOnce(&mut MikuFS<&mut MemDisk>),
{
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    params.block_size = BLOCK_SIZE;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");
    let mut disk = if dir_index {
        disk
    } else {
        let mut image = disk.image_flushed().to_vec();
        let at = 1024 + 92; // s_feature_compat
        let compat = u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]]);
        image[at..at + 4].copy_from_slice(&(compat & !0x0020).to_le_bytes());
        MemDisk::from_image(image)
    };
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.activate().expect("activate failed");
        assert!(fs.journal_active, "the point of this file is the journal");
        setup(&mut fs);
        fs.sync().unwrap();
    }
    disk
}

/// Writes the operation takes uninterrupted, including the mount, as a crash
/// during recovery counts
fn write_span<F>(base: &MemDisk, op: &F) -> u64
where
    F: Fn(&mut MikuFS<&mut MemDisk>),
{
    let mut disk = MemDisk::from_image(base.image().to_vec());
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.activate().expect("activate failed");
        op(&mut fs);
        fs.sync().unwrap();
    }
    disk.write_ops
}

/// Runs the operation with the power cut after `n` writes, then replays and
/// returns the image.
///
/// The disk is not in volatile mode: with a volatile cache the crash would lose
/// the whole operation, consistent for free. Here each write reaches the medium
/// in order and the cut truncates the stream, leaving a prefix, as a journal
/// must repair
fn cut_at<F>(base: &MemDisk, n: u64, op: &F) -> (MemDisk, bool, u32)
where
    F: Fn(&mut MikuFS<&mut MemDisk>),
{
    let mut disk = MemDisk::from_image(base.image().to_vec());
    disk.crash_at(n);
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        // Any of these may be cut half-done; none may panic, and results mean
        // nothing on a stopped disk.
        //
        // The sync is inside the window so the sweep covers the checkpoint: the
        // journal is committed and metadata is on its way to its real location,
        // the case replay is for
        let _ = fs.activate();
        op(&mut fs);
        let _ = fs.sync();
    }
    let crashed = disk.did_crash();
    disk.power_on();
    let replayed;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("will not mount after the crash");
        replayed = fs.activate().expect("replay failed").0;
        fs.sync().unwrap();
    }
    (disk, crashed, replayed)
}

/// Cuts the power at every point of an operation and requires the filesystem to
/// return whole at each. `verdict` gets the recovered filesystem and can
/// require all-or-nothing
fn sweep<F, V>(label: &str, base: &MemDisk, op: F, verdict: V)
where
    F: Fn(&mut MikuFS<&mut MemDisk>),
    V: Fn(&mut MikuFS<&mut MemDisk>) -> Result<(), String>,
{
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let span = write_span(base, &op);
    assert!(span > 4, "{label}: the operation writes almost nothing ({span})");

    let mut broken = Vec::new();
    let mut cut_short = 0u64;
    for n in 0..span {
        let (mut disk, crashed, replayed) = cut_at(base, n, &op);
        if crashed {
            cut_short += 1;
        }

        let seen = {
            let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
            verdict(&mut fs)
        };
        let path = dump(&mut disk, &format!("{label}-{n}"));
        let (code, report) = fsck(&path);
        if code != 0 {
            broken.push(format!(
                "cut {n} (replayed {replayed}): e2fsck: {}",
                complaint(&report)
            ));
            cleanup(&path);
            continue;
        }
        cleanup(&path);
        if let Err(why) = seen {
            broken.push(format!("cut {n} (replayed {replayed}): {why}"));
        }
    }

    assert!(
        broken.is_empty(),
        "{label}: {} of {span} cut points left the filesystem damaged:\n  {}",
        broken.len(),
        broken.join("\n  ")
    );
    // Guards against a sweep where the power never went out
    assert!(
        cut_short > span / 2,
        "{label}: only {cut_short} of {span} cut points reached the disk at all"
    );
    eprintln!("{label}: {span} cut points, {cut_short} of them interrupted a write");
}

/// A file is there or not. "There" means a name, the inode it names, and an
/// inode saying it is a file with one link
fn all_or_nothing(fs: &mut MikuFS<&mut MemDisk>, dir: u32, name: &str) -> Result<(), String> {
    let found = match fs.ext2_lookup_in_dir(dir, name) {
        Ok(v) => v,
        Err(e) => return Err(format!("looking for {name}: {e:?}")),
    };
    let Some(ino) = found else { return Ok(()) };
    let inode = match fs.read_inode(ino) {
        Ok(i) => i,
        Err(e) => return Err(format!("{name} points at inode {ino}, which will not read: {e:?}")),
    };
    if inode.links_count() == 0 {
        return Err(format!("{name} points at inode {ino}, which has no links"));
    }
    if !inode.is_regular() {
        return Err(format!("{name} points at inode {ino}, which is not a file"));
    }
    Ok(())
}

#[test]
fn creating_a_file() {
    let base = prepared(|fs| {
        fs.ext3_create_dir(2, "d", 0o755).unwrap();
    });
    let dir = {
        let mut disk = MemDisk::from_image(base.image().to_vec());
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.ext2_lookup_in_dir(2, "d").unwrap().unwrap()
    };
    sweep(
        "create",
        &base,
        |fs| {
            let _ = fs.ext3_create_file(dir, "new.txt", 0o644);
        },
        |fs| all_or_nothing(fs, dir, "new.txt"),
    );
}

#[test]
fn deleting_a_file() {
    let base = prepared(|fs| {
        let dir = fs.ext3_create_dir(2, "d", 0o755).unwrap();
        let ino = fs.ext3_create_file(dir, "doomed.txt", 0o644).unwrap();
        fs.ext3_write_file(ino, b"contents worth losing", 0).unwrap();
    });
    let dir = {
        let mut disk = MemDisk::from_image(base.image().to_vec());
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.ext2_lookup_in_dir(2, "d").unwrap().unwrap()
    };
    sweep(
        "delete",
        &base,
        |fs| {
            let _ = fs.ext3_delete_file(dir, "doomed.txt");
        },
        |fs| all_or_nothing(fs, dir, "doomed.txt"),
    );
}

/// The name of the directory `setup` made, looked up on a fresh mount
fn dir_of(base: &MemDisk, name: &str) -> u32 {
    let mut disk = MemDisk::from_image(base.image().to_vec());
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    fs.ext2_lookup_in_dir(2, name).unwrap().unwrap()
}

/// Each name is there or not, none half
fn each_all_or_nothing(
    fs: &mut MikuFS<&mut MemDisk>,
    dir: u32,
    names: &[String],
) -> Result<(), String> {
    for name in names {
        all_or_nothing(fs, dir, name)?;
    }
    Ok(())
}

/// Adding to a directory big enough to split leaves and rewrite the index,
/// changing three blocks at once: old leaf, new leaf, and index node above
#[test]
fn splitting_an_htree_leaf() {
    let base = prepared(|fs| {
        let dir = fs.ext3_create_dir(2, "d", 0o755).unwrap();
        for i in 0..400 {
            fs.ext3_create_file(dir, &format!("f{i:05}"), 0o644).unwrap();
        }
        let inode = fs.read_inode(dir).unwrap();
        assert!(fs.dir_is_indexed(&inode), "the directory has to be indexed");
    });
    let dir = dir_of(&base, "d");
    let names: Vec<String> = (0..12).map(|i| format!("added-{i:03}")).collect();
    let created = names.clone();
    sweep(
        "split",
        &base,
        move |fs| {
            for name in &created {
                let _ = fs.ext3_create_file(dir, name, 0o644);
            }
        },
        |fs| each_all_or_nothing(fs, dir, &names),
    );
}

/// The same with no index, where the directory grows through the indirect
/// chain. Those indirect blocks are the file's block map
#[test]
fn growing_a_linear_directory() {
    let base = prepared_on(FsType::Ext3, false, |fs| {
        assert!(!fs.superblock.has_dir_index());
        let dir = fs.ext3_create_dir(2, "d", 0o755).unwrap();
        // Just short of the twelve direct pointers, so the added names need an
        // indirect block
        for i in 0..760 {
            fs.ext3_create_file(dir, &format!("f{i:05}"), 0o644).unwrap();
        }
        let inode = fs.read_inode(dir).unwrap();
        assert!(
            inode.size() >= 12 * BLOCK_SIZE as u64,
            "the directory is {} bytes - the test never reaches the indirect blocks",
            inode.size()
        );
    });
    let dir = dir_of(&base, "d");
    let names: Vec<String> = (0..12).map(|i| format!("added-{i:03}")).collect();
    let created = names.clone();
    sweep(
        "grow",
        &base,
        move |fs| {
            for name in &created {
                let _ = fs.ext3_create_file(dir, name, 0o644);
            }
        },
        |fs| each_all_or_nothing(fs, dir, &names),
    );
}

/// An extended attribute in its own block, which is metadata
#[test]
fn setting_an_extended_attribute() {
    let base = prepared(|fs| {
        let dir = fs.ext3_create_dir(2, "d", 0o755).unwrap();
        fs.ext3_create_file(dir, "tagged", 0o644).unwrap();
    });
    let dir = dir_of(&base, "d");
    sweep(
        "xattr",
        &base,
        move |fs| {
            let Ok(Some(ino)) = fs.ext2_lookup_in_dir(dir, "tagged") else { return };
            let _ = fs.ext3_set_xattr(ino, 1, "selinux", &[b'x'; 96]);
        },
        |fs| all_or_nothing(fs, dir, "tagged"),
    );
}

/// A journal written here, replayed by `e2fsck`. Leaves a committed, unchecked
/// transaction on the disk for e2fsprogs' reader, which checks descriptor
/// blocks, tags, sequence numbers, and the journal superblock against JBD2
#[test]
fn e2fsck_replays_a_journal_we_wrote() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let base = prepared(|fs| {
        fs.ext3_create_dir(2, "d", 0o755).unwrap();
    });
    let dir = dir_of(&base, "d");

    let mut disk = MemDisk::from_image(base.image().to_vec());
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.activate().unwrap();
        for i in 0..8 {
            fs.ext3_create_file(dir, &format!("logged-{i}"), 0o644).unwrap();
        }
        // The log goes to the medium; the blocks stay in the cache and die with
        // it, as a power cut between commit and checkpoint
        fs.force_commit().unwrap();
    }

    let path = dump(&mut disk, "dirtylog");
    let out = Command::new("e2fsck")
        .args(["-f", "-y"])
        .arg(&path)
        .output()
        .expect("e2fsck did not run");
    let report = String::from_utf8_lossy(&out.stdout).into_owned()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(
        report.contains("ecovering journal"),
        "e2fsck did not find a journal to replay - the test proves nothing:\n{report}"
    );

    // Whatever it had to do to replay, the result must then be clean
    let (code, second) = fsck(&path);
    assert_eq!(
        code, 0,
        "after e2fsck replayed our journal the filesystem is still wrong:\n{report}\n---\n{second}"
    );

    // and the names the transaction carried are there, in e2fsprogs' reading
    let listing = Command::new("debugfs")
        .arg("-R")
        .arg("ls -l /d")
        .arg(&path)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    for i in 0..8 {
        assert!(
            listing.contains(&format!("logged-{i}")),
            "logged-{i} did not survive the replay:\n{listing}"
        );
    }
    cleanup(&path);
}

/// A journal with unimplemented features is left alone. The tag stride depends
/// on these bits, and a wrong stride writes the log's contents over other
/// blocks
#[test]
fn a_journal_we_cannot_read_is_not_replayed() {
    let base = prepared(|fs| {
        fs.ext3_create_dir(2, "d", 0o755).unwrap();
    });
    let dir = dir_of(&base, "d");

    // A real dirty journal, then one unimplemented feature bit
    let mut disk = MemDisk::from_image(base.image().to_vec());
    let journal_sb_block;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.activate().unwrap();
        journal_sb_block = fs.journal_block_to_disk(0).unwrap();
        for i in 0..4 {
            fs.ext3_create_file(dir, &format!("logged-{i}"), 0o644).unwrap();
        }
        fs.force_commit().unwrap();
    }
    let mut image = disk.image_flushed().to_vec();

    // Sanity: as it stands, this journal replays
    {
        let mut clean = MemDisk::from_image(image.clone());
        let mut fs = MikuFS::open(&mut clean, 0, false).unwrap();
        let (replayed, _) = fs.activate().unwrap();
        assert!(replayed > 0, "the test needs a journal that would replay");
        assert!(fs.ext2_lookup_in_dir(dir, "logged-0").unwrap().is_some());
    }

    // JBD2_FEATURE_INCOMPAT_FAST_COMMIT, not implemented. `s_feature_incompat`
    // is at offset 40 of the journal superblock, big endian like the rest of
    // JBD2
    let at = journal_sb_block as usize * BLOCK_SIZE as usize + 40;
    let current = u32::from_be_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]]);
    image[at..at + 4].copy_from_slice(&(current | 0x0020).to_be_bytes());

    let mut disk = MemDisk::from_image(image);
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("it still has to mount");
    let (replayed, _) = fs.activate().expect("an unreadable journal is not a mount failure");
    assert_eq!(replayed, 0, "a journal we cannot read must not be replayed");
    assert!(
        fs.reader.read_only,
        "a volume whose journal was not replayed must not be written to"
    );
    // Still readable: only the write is refused
    assert!(fs.ext2_lookup_in_dir(2, "d").unwrap().is_some());
}

/// Every byte of `name` up to its size is `byte`, or there is no `name`
fn reads_as(fs: &mut MikuFS<&mut MemDisk>, name: &str, byte: u8) -> Result<(), String> {
    all_or_nothing(fs, 2, name)?;
    let Some(ino) = fs.ext2_lookup_in_dir(2, name).map_err(|e| format!("{e:?}"))? else {
        return Ok(());
    };
    let inode = fs.read_inode(ino).map_err(|e| format!("{e:?}"))?;
    let mut buf = vec![0u8; inode.size() as usize];
    fs.read_file(&inode, 0, &mut buf).map_err(|e| format!("reading {name}: {e:?}"))?;
    match buf.iter().position(|&b| b != byte) {
        Some(at) => Err(format!("{name} byte {at} of {} is {:#x}, not {byte:#x}", buf.len(), buf[at])),
        None => Ok(()),
    }
}

/// Sixteen pages of `byte`, as the page cache's write-out hands them
fn pages_of(byte: u8) -> Vec<Vec<u8>> {
    (0..16).map(|_| vec![byte; 4096]).collect()
}

/// Whole blocks go past the block cache, and a new block's data precedes its
/// extent: after a power cut the file shows what was written or nothing, never
/// a deleted file's bytes
#[test]
fn a_growing_file_never_shows_old_data() {
    let base = prepared(|fs| {
        let old = fs.ext3_create_file(2, "old.bin", 0o644).unwrap();
        fs.ext3_write_file(old, &vec![0xAA; 64 * 1024], 0).unwrap();
        fs.sync().unwrap();
        fs.ext3_delete_file(2, "old.bin").unwrap();
    });
    let pages = pages_of(0x55);
    sweep(
        "grow",
        &base,
        |fs| {
            let Ok(ino) = fs.ext3_create_file(2, "new.bin", 0o644) else { return };
            let parts: Vec<&[u8]> = pages.iter().map(|p| p.as_slice()).collect();
            let _ = fs.ext3_write_parts(ino, &parts, 0);
        },
        |fs| reads_as(fs, "new.bin", 0x55),
    );
}

/// A file deleted and a new one written in one transaction. Until the delete
/// commits, a crash returns the old file's blocks, so new data must not be on
/// them: the old file returns whole or not at all
#[test]
fn a_delete_and_a_write_in_one_transaction() {
    let base = prepared(|fs| {
        let keep = fs.ext3_create_file(2, "keep.bin", 0o644).unwrap();
        fs.ext3_write_file(keep, &vec![0xAA; 64 * 1024], 0).unwrap();
    });
    let pages = pages_of(0x55);
    sweep(
        "delete+write",
        &base,
        |fs| {
            if fs.ext3_delete_file(2, "keep.bin").is_err() {
                return;
            }
            let Ok(ino) = fs.ext3_create_file(2, "new.bin", 0o644) else { return };
            let parts: Vec<&[u8]> = pages.iter().map(|p| p.as_slice()).collect();
            let _ = fs.ext3_write_parts(ino, &parts, 0);
        },
        |fs| {
            reads_as(fs, "keep.bin", 0xAA)?;
            reads_as(fs, "new.bin", 0x55)
        },
    );
}
