// The checker, checked
//
// A checker can miss things or report faults on a healthy volume. So two
// halves: the first gives it a sound filesystem and requires silence. The
// second breaks one thing and asks in order:
//
//   1. does `e2fsck` agree the injury is real?
//   2. does our check name it?
//   3. after our repair, does `e2fsck` call the volume clean?
//
// Skipped when e2fsprogs is not installed.

use extfs::fsck::{FsckOptions, FsckReport, Problem};
use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::structs::*;
use extfs::xattr::XATTR_INDEX_USER;
use extfs::{MemDisk, MikuFS};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SECTORS: usize = 64 * 1024;

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-V")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty())
        .unwrap_or(false)
}

fn dump(disk: &mut MemDisk, tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("extfs-fsck-{tag}-{}.img", std::process::id()));
    let mut f = std::fs::File::create(&path).expect("cannot create image");
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

/// `e2fsck -f -n`: checks everything, changes nothing
fn e2fsck(path: &Path) -> (i32, String) {
    let out = Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg(path)
        .output()
        .expect("e2fsck did not run");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), report)
}

fn complaints(r: &FsckReport) -> String {
    let mut out = String::new();
    for f in r.examples() {
        out.push_str(&format!("  {}{}\n", f.problem, if f.fixed { " [fixed]" } else { "" }));
    }
    if r.findings.truncated() > 0 {
        out.push_str(&format!("  ... and {} more\n", r.findings.truncated()));
    }
    out
}

fn fresh(kind: FsType, block_size: u32) -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    params.block_size = block_size;
    if kind == FsType::Ext4 {
        params.inode_size = 256;
    }
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

/// Build a filesystem, do something to it, unmount it cleanly
fn prepared<F>(kind: FsType, block_size: u32, setup: F) -> MemDisk
where
    F: FnOnce(&mut MikuFS<&mut MemDisk>),
{
    let mut disk = fresh(kind, block_size);
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        fs.activate().expect("activate failed");
        setup(&mut fs);
        fs.sync().expect("sync failed");
    }
    disk
}

/// Check a disk with our own checker, on a fresh mount
fn check(disk: &mut MemDisk) -> FsckReport {
    let mut fs = MikuFS::open(disk, 0, false).expect("mount failed");
    fs.activate().expect("activate failed");
    fs.fsck(FsckOptions::check()).expect("the check could not run")
}

fn repair(disk: &mut MemDisk) -> FsckReport {
    let mut fs = MikuFS::open(disk, 0, false).expect("mount failed");
    fs.activate().expect("activate failed");
    let r = fs.fsck(FsckOptions::repair()).expect("the repair could not run");
    fs.sync().expect("sync failed");
    r
}

fn must_be_clean(disk: &mut MemDisk, what: &str) {
    let r = check(disk);
    assert!(r.checked, "{what}: the check refused to run");
    assert_eq!(r.errors(), 0, "{what}: our check invented faults:\n{}", complaints(&r));
}

// Half one: a sound filesystem, and silence

#[test]
fn a_fresh_filesystem_is_clean() {
    for kind in [FsType::Ext2, FsType::Ext3, FsType::Ext4] {
        for bs in [1024u32, 4096] {
            let mut disk = fresh(kind, bs);
            must_be_clean(&mut disk, &format!("fresh {} at {} byte blocks", kind.name(), bs));
        }
    }
}

#[test]
fn a_filesystem_in_use_is_clean() {
    for kind in [FsType::Ext2, FsType::Ext3, FsType::Ext4] {
        let mut disk = prepared(kind, 1024, |fs| {
            let d = fs.ext3_create_dir(EXT2_ROOT_INO, "work", 0o755).unwrap();
            for i in 0..40 {
                let name = format!("file{i:03}");
                let ino = fs.ext3_create_file(d, &name, 0o644).unwrap();
                let body = vec![b'x'; 100 * (i + 1)];
                fs.ext3_write_file(ino, &body, 0).unwrap();
            }
            let sub = fs.ext3_create_dir(d, "deeper", 0o755).unwrap();
            let f = fs.ext3_create_file(sub, "leaf", 0o644).unwrap();
            fs.ext3_write_file(f, b"hello", 0).unwrap();
            // A hard link, for a link count above one
            fs.ext2_hardlink(d, "leaf-again", f).unwrap();
        });
        must_be_clean(&mut disk, &format!("{} in use", kind.name()));
    }
}

/// A directory big enough to index, whose index blocks must not read as entries
#[test]
fn an_indexed_directory_is_clean() {
    let mut disk = prepared(FsType::Ext4, 1024, |fs| {
        let d = fs.ext3_create_dir(EXT2_ROOT_INO, "many", 0o755).unwrap();
        for i in 0..400 {
            fs.ext3_create_file(d, &format!("entry-{i:04}"), 0o644).unwrap();
        }
    });
    must_be_clean(&mut disk, "an indexed directory");
}

/// Files with unusual block placement: an extent tree, a hole, an unwritten
/// extent, data in the inode, and an attribute block
#[test]
fn the_awkward_shapes_are_clean() {
    let mut disk = prepared(FsType::Ext4, 1024, |fs| {
        let big = fs.ext3_create_file(EXT2_ROOT_INO, "big", 0o644).unwrap();
        let body = vec![b'q'; 200 * 1024];
        fs.ext3_write_file(big, &body, 0).unwrap();

        let sparse = fs.ext3_create_file(EXT2_ROOT_INO, "sparse", 0o644).unwrap();
        fs.ext3_write_file(sparse, b"start", 0).unwrap();
        fs.ext3_write_file(sparse, b"far away", 400 * 1024).unwrap();

        let reserved = fs.ext3_create_file(EXT2_ROOT_INO, "reserved", 0o644).unwrap();
        fs.ext4_fallocate(reserved, 0, 64 * 1024).unwrap();

        let tagged = fs.ext3_create_file(EXT2_ROOT_INO, "tagged", 0o644).unwrap();
        fs.ext3_set_xattr(tagged, XATTR_INDEX_USER, "note", b"an attribute long enough to need a block of its own, several times over, so that it cannot live inside the inode").unwrap();
    });
    must_be_clean(&mut disk, "extents, holes, unwritten space, inline data and xattrs");
}

/// The filesystem from `tests/crash.rs`, with a journal replayed on the way in
#[test]
fn a_replayed_journal_leaves_nothing_to_find() {
    let mut disk = prepared(FsType::Ext3, 1024, |fs| {
        fs.ext3_create_dir(EXT2_ROOT_INO, "before", 0o755).unwrap();
    });
    {
        // Committed, not checkpointed, no clean unmount: the next mount has a
        // log to replay
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.activate().unwrap();
        let d = fs.ext3_create_dir(EXT2_ROOT_INO, "after", 0o755).unwrap();
        fs.ext3_create_file(d, "inside", 0o644).unwrap();
        fs.force_commit().unwrap();
    }
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let (replayed, _) = fs.activate().unwrap();
        assert!(replayed > 0, "the point of this case is a replay that happened");
        fs.sync().unwrap();
    }
    must_be_clean(&mut disk, "a filesystem whose journal was replayed");
}

/// Made by e2fsprogs
#[test]
fn a_foreign_filesystem_is_clean() {
    if !have("mke2fs") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let mut path = std::env::temp_dir();
    path.push(format!("extfs-fsck-foreign-{}.img", std::process::id()));
    let ok = Command::new("mke2fs")
        .args(["-q", "-F", "-t", "ext4", "-b", "1024", "-I", "256"])
        .arg(&path)
        .arg("32768")
        .status()
        .expect("mke2fs did not run");
    assert!(ok.success(), "mke2fs failed");

    let image = std::fs::read(&path).expect("cannot read the image");
    cleanup(&path);
    let mut disk = MemDisk::from_image(image);
    must_be_clean(&mut disk, "an image mke2fs wrote");
}

// Half two: one injury at a time
//
// Each case goes through `injured`, requiring all three steps, so a case that
// breaks nothing does not pass quietly.

/// Build a filesystem, break one thing, leave it cleanly unmounted
fn injured<S, B>(kind: FsType, setup: S, break_it: B) -> MemDisk
where
    S: FnOnce(&mut MikuFS<&mut MemDisk>),
    B: FnOnce(&mut MikuFS<&mut MemDisk>),
{
    let mut disk = prepared(kind, 1024, setup);
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        fs.activate().expect("activate failed");
        break_it(&mut fs);
        fs.sync().expect("sync failed");
        fs.mark_clean_unmount();
        fs.flush_all_dirty_metadata().expect("flush failed");
    }
    disk
}

/// Leaves the volume as an unmount would, so `e2fsck` judges the filesystem
fn unmount(disk: &mut MemDisk) {
    let mut fs = MikuFS::open(disk, 0, false).expect("mount failed");
    fs.activate().expect("activate failed");
    fs.sync().expect("sync failed");
    fs.mark_clean_unmount();
    fs.flush_all_dirty_metadata().expect("flush failed");
}

/// The three questions, in order
fn three_steps<W>(tag: &str, mut disk: MemDisk, want: W)
where
    W: Fn(&Problem) -> bool,
{
    if !have("e2fsck") {
        eprintln!("e2fsprogs not installed - skipping {tag}");
        return;
    }

    // 1. Is the injury real? `e2fsck -n` exits zero for some harmless faults,
    //    such as a wrong free count, while still printing them, so the printed
    //    text is the answer: with `-n` each problem appears as a question it
    //    answers itself
    let path = dump(&mut disk, tag);
    let (_, report) = e2fsck(&path);
    cleanup(&path);
    assert!(
        report.contains("? no"),
        "{tag}: e2fsck finds nothing wrong, so this case is not testing anything:\n{report}"
    );

    //   2. does our check name it?
    let seen = check(&mut disk);
    assert!(seen.checked, "{tag}: our check refused to run");
    assert!(
        seen.saw(&want),
        "{tag}: e2fsck saw a fault and we did not name it.\ne2fsck said:\n{report}\nwe said:\n{}",
        complaints(&seen)
    );

    // 3. does our repair satisfy somebody else's checker?
    let fixed = repair(&mut disk);
    assert!(fixed.fixed > 0, "{tag}: nothing was repaired:\n{}", complaints(&fixed));
    unmount(&mut disk);

    let path = dump(&mut disk, &format!("{tag}-repaired"));
    let (code, report) = e2fsck(&path);
    cleanup(&path);
    assert!(
        code == 0 && !report.contains("? no"),
        "{tag}: e2fsck still has something to say about what our repair left:\n{report}"
    );

    let again = check(&mut disk);
    assert_eq!(
        again.errors(), 0,
        "{tag}: our own check still finds something after repairing:\n{}",
        complaints(&again)
    );
}

/// A file with one block, and the block it uses
fn one_file(fs: &mut MikuFS<&mut MemDisk>, name: &str) -> (u32, u32) {
    let ino = fs.ext3_create_file(EXT2_ROOT_INO, name, 0o644).unwrap();
    fs.ext3_write_file(ino, b"contents", 0).unwrap();
    let block = fs.get_file_block_any(ino, 0).unwrap();
    (ino, block)
}

#[test]
fn the_superblock_free_block_count_is_wrong() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            let wrong = fs.superblock.free_blocks_count() + 500;
            fs.superblock.write_u32(12, wrong);
            fs.superblock_dirty = true;
        },
    );
    three_steps("sb-free-blocks", disk, |p| {
        matches!(p, Problem::SuperFreeBlocksWrong { .. })
    });
}

#[test]
fn a_group_free_inode_count_is_wrong() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            let wrong = fs.groups[0].free_inodes() - 7;
            fs.groups[0].write_u16(14, wrong);
            fs.update_group_desc_csum(0);
            fs.flush_group_desc(0).unwrap();
        },
    );
    three_steps("group-free-inodes", disk, |p| {
        matches!(p, Problem::GroupFreeInodesWrong { .. })
    });
}

/// The bitmap says a block is free while a file uses it, so the next allocation
/// gives it away
#[test]
fn a_live_block_is_marked_free() {
    let mut victim = 0u32;
    let disk = injured(
        FsType::Ext4,
        |fs| {
            let (_, block) = one_file(fs, "a");
            assert_ne!(block, 0);
        },
        |fs| {
            let ino = fs.resolve_path("/a").unwrap();
            victim = fs.get_file_block_any(ino, 0).unwrap();
            fs.set_bitmap_bit(fs.groups[0].block_bitmap(), block_index(fs, victim), false)
                .unwrap();
            fs.update_block_bitmap_csum(0).unwrap();
        },
    );
    three_steps("block-marked-free", disk, |p| {
        matches!(p, Problem::BlockBitmapWrong { .. })
    });
}

/// The other direction: a block nobody owns, marked in use
#[test]
fn a_block_nobody_owns_is_marked_used() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            let free = first_free_block(fs);
            fs.set_bitmap_bit(fs.groups[0].block_bitmap(), block_index(fs, free), true)
                .unwrap();
            fs.update_block_bitmap_csum(0).unwrap();
            fs.update_group_free_blocks(0, -1).unwrap();
            fs.update_superblock_free_blocks(-1).unwrap();
        },
    );
    three_steps("leaked-block", disk, |p| {
        matches!(p, Problem::BlockBitmapWrong { .. })
    });
}

/// Two files pointing at one block
#[test]
fn two_inodes_claim_one_block() {
    let disk = injured(
        FsType::Ext2,
        |fs| {
            one_file(fs, "a");
            one_file(fs, "b");
        },
        |fs| {
            let a = fs.resolve_path("/a").unwrap();
            let b = fs.resolve_path("/b").unwrap();
            let shared = fs.get_file_block_any(a, 0).unwrap();
            let orphaned = fs.get_file_block_any(b, 0).unwrap();
            let mut inode = fs.read_inode(b).unwrap();
            inode.set_block(0, shared);
            fs.write_inode(b, &inode).unwrap();
            // The block b owned now belongs to nobody, part of the same injury
            let _ = orphaned;
        },
    );
    three_steps("cross-linked", disk, |p| {
        matches!(p, Problem::BlockClaimedTwice { .. })
    });
}

#[test]
fn a_block_pointer_leaves_the_filesystem() {
    let disk = injured(
        FsType::Ext2,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            let a = fs.resolve_path("/a").unwrap();
            let past_the_end = fs.superblock.blocks_count() + 9;
            let mut inode = fs.read_inode(a).unwrap();
            inode.set_block(0, past_the_end);
            fs.write_inode(a, &inode).unwrap();
        },
    );
    three_steps("pointer-out-of-range", disk, |p| {
        matches!(p, Problem::BlockOutOfRange { .. })
    });
}

#[test]
fn a_link_count_is_too_high() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            let a = fs.resolve_path("/a").unwrap();
            let mut inode = fs.read_inode(a).unwrap();
            inode.set_links_count(4);
            fs.write_inode(a, &inode).unwrap();
        },
    );
    three_steps("links-too-high", disk, |p| {
        matches!(p, Problem::LinkCountWrong { counted: 1, .. })
    });
}

/// Too low loses data: the last name to go frees an inode other names point at
#[test]
fn a_link_count_is_too_low() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            let (ino, _) = one_file(fs, "a");
            fs.ext2_hardlink(EXT2_ROOT_INO, "b", ino).unwrap();
            fs.ext2_hardlink(EXT2_ROOT_INO, "c", ino).unwrap();
        },
        |fs| {
            let a = fs.resolve_path("/a").unwrap();
            let mut inode = fs.read_inode(a).unwrap();
            inode.set_links_count(1);
            fs.write_inode(a, &inode).unwrap();
        },
    );
    three_steps("links-too-low", disk, |p| {
        matches!(p, Problem::LinkCountWrong { counted: 3, .. })
    });
}

/// A file whose name is gone, with blocks still claimed and unreachable
#[test]
fn a_file_has_no_name_left() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            fs.remove_dir_entry(EXT2_ROOT_INO, "a").unwrap();
        },
    );
    three_steps("unattached-file", disk, |p| {
        matches!(p, Problem::InodeUnattached { .. })
    });
}

/// An entry pointing at an inode that has been freed underneath it
#[test]
fn an_entry_points_at_a_free_inode() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            let a = fs.resolve_path("/a").unwrap();
            let blank = fs.fresh_inode();
            fs.write_inode(a, &blank).unwrap();
            fs.free_inode(a).unwrap();
        },
    );
    three_steps("entry-to-free-inode", disk, |p| {
        matches!(p, Problem::DirEntryFreeInode { .. })
    });
}

/// `..` pointing elsewhere, with two directories' link counts wrong too
#[test]
fn dot_dot_points_at_the_wrong_directory() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            let a = fs.ext3_create_dir(EXT2_ROOT_INO, "outer", 0o755).unwrap();
            fs.ext3_create_dir(a, "inner", 0o755).unwrap();
            fs.ext3_create_dir(EXT2_ROOT_INO, "elsewhere", 0o755).unwrap();
        },
        |fs| {
            let inner = fs.resolve_path("/outer/inner").unwrap();
            let elsewhere = fs.resolve_path("/elsewhere").unwrap();
            let inode = fs.read_inode(inner).unwrap();
            let phys = fs.get_file_block(&inode, 0).unwrap();
            let mut buf = vec![0u8; fs.block_size as usize];
            fs.read_block_into(phys, &mut buf).unwrap();
            buf[12..16].copy_from_slice(&elsewhere.to_le_bytes());
            fs.write_dir_head(inner, &inode, phys, &mut buf).unwrap();
        },
    );
    three_steps("dotdot-wrong", disk, |p| {
        matches!(p, Problem::DirDotDotWrong { .. })
    });
}

/// A record length running off its block's end
#[test]
fn a_directory_record_runs_off_the_block() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            let d = fs.ext3_create_dir(EXT2_ROOT_INO, "dir", 0o755).unwrap();
            for i in 0..6 {
                fs.ext3_create_file(d, &format!("f{i}"), 0o644).unwrap();
            }
        },
        |fs| {
            let d = fs.resolve_path("/dir").unwrap();
            let inode = fs.read_inode(d).unwrap();
            let phys = fs.get_file_block(&inode, 0).unwrap();
            let mut buf = vec![0u8; fs.block_size as usize];
            fs.read_block_into(phys, &mut buf).unwrap();
            // Third record, past `.` and `..`, claiming more space than the
            // block has
            let at = 12 + 12;
            buf[at + 4..at + 6].copy_from_slice(&4000u16.to_le_bytes());
            fs.write_dir_block(d, phys, &mut buf).unwrap();
        },
    );
    three_steps("rec-len-off-the-end", disk, |p| {
        matches!(p, Problem::DirBadRecLen { .. })
    });
}

/// An inode marked in use with nothing in it, never allocatable, with a wrong
/// free count
#[test]
fn an_unused_inode_is_marked_in_use() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            one_file(fs, "a");
        },
        |fs| {
            let free = fs.superblock.free_inodes_count();
            let ino = fs.alloc_inode(0).unwrap();
            // Allocated in the bitmap and never filled in
            let _ = free;
            let _ = ino;
        },
    );
    three_steps("inode-marked-in-use", disk, |p| {
        matches!(p, Problem::InodeBitmapWrong { .. })
    });
}

/// A subtree unlinked from its parent, intact and unreachable
#[test]
fn a_subtree_is_cut_off_from_the_root() {
    let disk = injured(
        FsType::Ext4,
        |fs| {
            let a = fs.ext3_create_dir(EXT2_ROOT_INO, "lost", 0o755).unwrap();
            let b = fs.ext3_create_dir(a, "deeper", 0o755).unwrap();
            fs.ext3_create_file(b, "leaf", 0o644).unwrap();
        },
        |fs| {
            fs.remove_dir_entry(EXT2_ROOT_INO, "lost").unwrap();
        },
    );
    three_steps("cut-off-subtree", disk, |p| {
        matches!(p, Problem::DirDisconnected { .. })
    });
}

/// Which bit of a group-0 bitmap block stands for this block
fn block_index(fs: &MikuFS<&mut MemDisk>, block: u32) -> u32 {
    block - fs.superblock.first_data_block()
}

/// A block in group 0 that nothing is using
fn first_free_block(fs: &mut MikuFS<&mut MemDisk>) -> u32 {
    let b = fs.alloc_block(0).expect("no free block for the test");
    fs.free_block(b).unwrap();
    b
}

/// An index built by e2fsprogs. The check finds index nodes by descending and
/// comparing each node's `limit` with what its size holds; a mistake would
/// report every node as damaged
#[test]
fn an_index_e2fsprogs_built_is_clean() {
    if !have("mke2fs") || !have("debugfs") || !have("e2fsck") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let mut path = std::env::temp_dir();
    path.push(format!("extfs-fsck-theirindex-{}.img", std::process::id()));
    let count = 500usize;
    let ok = Command::new("mke2fs")
        .args(["-q", "-t", "ext4", "-b", "1024", "-I", "256"])
        .args(["-O", "dir_index,metadata_csum"])
        .args(["-N", &format!("{}", count + 4096)])
        .args(["-F", path.to_str().unwrap(), &format!("{}", 8192 + count * 8)])
        .status()
        .expect("mke2fs did not run");
    assert!(ok.success(), "mke2fs failed");

    let mut script = String::from("mkdir /many\n");
    for i in 0..count {
        script.push_str(&format!("mkdir /many/name-{i:05}\n"));
    }
    let script_path = path.with_extension("cmds");
    std::fs::write(&script_path, script).unwrap();
    let ok = Command::new("debugfs").args(["-w", "-f"]).arg(&script_path).arg(&path)
        .output().expect("debugfs did not run");
    assert!(ok.status.success(), "debugfs failed");
    let _ = std::fs::remove_file(&script_path);

    // pass 3A is where e2fsprogs builds the index
    let ok = Command::new("e2fsck").args(["-fDy"]).arg(&path).output().expect("e2fsck did not run");
    assert!(ok.status.code().unwrap_or(-1) & !1 == 0, "e2fsck -D failed");

    let image = std::fs::read(&path).expect("cannot read the image");
    cleanup(&path);
    let mut disk = MemDisk::from_image(image);
    must_be_clean(&mut disk, "a directory index e2fsprogs built");
}

// Special files

/// Device nodes, FIFOs, and sockets. e2fsprogs judges the device number
/// encoding in `i_block`
#[test]
fn special_files_are_created_and_our_check_accepts_them() {
    for kind in [FsType::Ext2, FsType::Ext3, FsType::Ext4] {
        let bs = if kind == FsType::Ext4 { 4096 } else { 1024 };
        let mut disk = prepared(kind, bs, |fs| {
            fs.ext3_mknod(2, "null", FT_CHRDEV, 0o666, 1, 3).unwrap();
            fs.ext3_mknod(2, "sda", FT_BLKDEV, 0o660, 8, 0).unwrap();
            fs.ext3_mknod(2, "pipe", FT_FIFO, 0o644, 0, 0).unwrap();
            fs.ext3_mknod(2, "sock", FT_SOCK, 0o644, 0, 0).unwrap();
            // A minor past 255 has to take the other encoding
            fs.ext3_mknod(2, "big", FT_CHRDEV, 0o600, 511, 4096).unwrap();
        });

        {
            let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
            fs.activate().unwrap();

            let ino = fs.ext2_lookup_in_dir(2, "null").unwrap().unwrap();
            let inode = fs.read_inode(ino).unwrap();
            assert!(inode.is_chardev(), "{kind:?}: null is a character device");
            assert_eq!(inode.device_number(), (1, 3), "{kind:?}: /dev/null is 1:3");
            assert_eq!(inode.blocks(), 0, "{kind:?}: a device node owns no blocks");
            assert_eq!(inode.size(), 0, "{kind:?}: and has no size");

            let ino = fs.ext2_lookup_in_dir(2, "sda").unwrap().unwrap();
            assert!(fs.read_inode(ino).unwrap().is_blockdev());
            assert_eq!(fs.read_inode(ino).unwrap().device_number(), (8, 0));

            let ino = fs.ext2_lookup_in_dir(2, "pipe").unwrap().unwrap();
            assert!(fs.read_inode(ino).unwrap().is_fifo(), "{kind:?}: a FIFO");

            let ino = fs.ext2_lookup_in_dir(2, "sock").unwrap().unwrap();
            assert!(fs.read_inode(ino).unwrap().is_socket(), "{kind:?}: a socket");

            let ino = fs.ext2_lookup_in_dir(2, "big").unwrap().unwrap();
            assert_eq!(
                fs.read_inode(ino).unwrap().device_number(),
                (511, 4096),
                "{kind:?}: a minor past 255 needs the wide encoding"
            );
        }

        must_be_clean(&mut disk, &format!("{kind:?}: after mknod"));
    }
}

/// `debugfs` prints the device number as Linux would read it
#[test]
fn e2fsprogs_reads_our_device_numbers() {
    if !have("e2fsck") || !have("debugfs") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = prepared(FsType::Ext4, 4096, |fs| {
        fs.ext3_mknod(2, "null", FT_CHRDEV, 0o666, 1, 3).unwrap();
        fs.ext3_mknod(2, "sda", FT_BLKDEV, 0o660, 8, 0).unwrap();
        fs.ext3_mknod(2, "big", FT_CHRDEV, 0o600, 511, 4096).unwrap();
    });

    let path = dump(&mut disk, "mknod");
    let (code, report) = e2fsck(&path);

    let out = Command::new("debugfs")
        .args(["-R", "stat null"])
        .arg(&path)
        .output()
        .expect("debugfs did not run");
    let null_stat = String::from_utf8_lossy(&out.stdout).into_owned();

    let out = Command::new("debugfs")
        .args(["-R", "stat big"])
        .arg(&path)
        .output()
        .expect("debugfs did not run");
    let big_stat = String::from_utf8_lossy(&out.stdout).into_owned();

    cleanup(&path);

    assert_eq!(code, 0, "e2fsck refused our device nodes:\n{report}");
    assert!(
        null_stat.contains("Device major/minor number: 01:03")
            || null_stat.contains("Device major/minor number: 1:3"),
        "debugfs read /dev/null as something else:\n{null_stat}"
    );
    assert!(
        big_stat.contains("511:4096") || big_stat.contains("1ff:1000"),
        "debugfs read the wide device number as something else:\n{big_stat}"
    );
}

/// Removing a device node frees the inode and nothing else. `i_block[0]` holds
/// 1:3, as a block number 259, metadata on a small volume
#[test]
fn deleting_a_device_node_frees_no_blocks() {
    let mut disk = prepared(FsType::Ext3, 1024, |fs| {
        fs.ext3_mknod(2, "null", FT_CHRDEV, 0o666, 1, 3).unwrap();
    });

    let free_with_node = {
        let fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.superblock.free_blocks_count()
    };

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.activate().unwrap();
        fs.ext3_delete_file(2, "null").unwrap();
        fs.sync().unwrap();
    }

    let free_after = {
        let fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.superblock.free_blocks_count()
    };
    assert_eq!(
        free_after, free_with_node,
        "removing a device node handed blocks back that it never held"
    );

    must_be_clean(&mut disk, "after removing a device node");
}
