// Filesystems judged by `e2fsck`
//
// Other tests are this code agreeing with itself, blind to a mistake compute
// and verify share: a checksum at the wrong offset, a field summed that should
// be skipped, a deterministic wrong hash.
//
// Skipped when e2fsprogs is not installed.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};
use std::io::Write;
use std::process::Command;

const SECTORS: usize = 128 * 1024;

fn have_e2fsck() -> bool {
    Command::new("e2fsck")
        .arg("-V")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty())
        .unwrap_or(false)
}

/// Write the image somewhere e2fsck can read it
fn dump(disk: &mut MemDisk, name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("extfs-test-{name}-{}.img", std::process::id()));
    let mut f = std::fs::File::create(&path).expect("cannot create image file");
    f.write_all(disk.image_flushed()).expect("cannot write image");
    f.sync_all().ok();
    path
}

/// Removes the image unless EXTFS_KEEP_IMAGES is set, for keeping evidence
fn cleanup(path: &std::path::Path) {
    if std::env::var_os("EXTFS_KEEP_IMAGES").is_some() {
        eprintln!("kept image: {}", path.display());
        return;
    }
    let _ = std::fs::remove_file(path);
}

/// `e2fsck -f -n`: forces a full check, changing nothing, and returns its
/// report. Status 0 is clean, bit 0 (1) means corrected, bit 2 (4) uncorrected.
/// With `-n` nothing is corrected, so any non-zero is a complaint
fn fsck(path: &std::path::Path) -> (i32, String) {
    let out = Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg(path)
        .output()
        .expect("e2fsck did not run");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), report)
}

fn check_our_mkfs(kind: FsType, block_size: u32, inode_size: u32) {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    params.block_size = block_size;
    params.inode_size = inode_size;
    mkfs(&mut disk, &params).expect("mkfs failed");

    let path = dump(&mut disk, kind.name());
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert_eq!(
        code, 0,
        "e2fsck rejects the {} image we produced (exit {code}):\n{report}",
        kind.name()
    );
}

#[test]
fn e2fsck_accepts_our_ext2() {
    check_our_mkfs(FsType::Ext2, 1024, 128);
}

#[test]
fn e2fsck_accepts_our_ext3() {
    check_our_mkfs(FsType::Ext3, 1024, 128);
}

#[test]
fn e2fsck_accepts_our_ext4() {
    check_our_mkfs(FsType::Ext4, 4096, 256);
}

/// Also after use
#[test]
fn e2fsck_accepts_a_filesystem_we_wrote_to() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        let dir = fs.ext2_create_dir(2, "work", 0o755).expect("mkdir failed");
        for i in 0..8 {
            let name = format!("file{i}.txt");
            let ino = fs
                .ext2_create_file(dir, &name, 0o644)
                .unwrap_or_else(|e| panic!("create {name}: {e:?}"));
            let body = format!("contents of {name}\n").repeat(40);
            fs.ext2_write_file(ino, body.as_bytes(), 0)
                .unwrap_or_else(|e| panic!("write {name}: {e:?}"));
        }
        fs.sync().expect("sync failed");
    }

    let path = dump(&mut disk, "written");
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert_eq!(
        code, 0,
        "e2fsck rejects a filesystem after we wrote to it (exit {code}):\n{report}"
    );
}

/// Truncation touches block bitmaps, group counters, and the inode's block
/// count. A mistake in any reads correctly and is inconsistent
#[test]
fn e2fsck_accepts_a_filesystem_we_truncated() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        // A file large enough to reach indirect blocks at a 1 KiB block size
        let ino = fs.ext2_create_file(2, "big.bin", 0o644).unwrap();
        let body: Vec<u8> = (0..400_000usize).map(|i| (i % 251) as u8).collect();
        fs.ext2_write_file(ino, &body, 0).unwrap();

        // Shrink it hard, then part way, then grow it sparsely
        fs.ext3_truncate_to(ino, 100_000).unwrap();
        fs.ext3_truncate_to(ino, 3_333).unwrap();
        fs.ext3_truncate_to(ino, 500_000).unwrap();

        // And one truncated to nothing
        let empty = fs.ext2_create_file(2, "empty.bin", 0o644).unwrap();
        fs.ext2_write_file(empty, &body[..50_000], 0).unwrap();
        fs.ext3_truncate_to(empty, 0).unwrap();

        fs.sync().expect("sync failed");
    }

    let path = dump(&mut disk, "truncated");
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert_eq!(
        code, 0,
        "e2fsck rejects a filesystem after truncation (exit {code}):\n{report}"
    );
}

/// `dumpe2fs -h`, for asking what features an image really declares
fn features(path: &std::path::Path) -> String {
    let out = Command::new("dumpe2fs")
        .arg("-h")
        .arg(path)
        .output()
        .expect("dumpe2fs did not run");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with("Filesystem features:"))
        .unwrap_or("")
        .to_string()
}

/// Judges the whole checksum chain: superblock, each group descriptor, both
/// bitmaps per group, each inode, and each directory block, each keyed
/// differently. Writes afterward reveal a checksum never updated
#[test]
fn e2fsck_accepts_our_metadata_csum_ext4() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        assert!(
            fs.superblock.has_metadata_csum(),
            "ext4 must be formatted with metadata_csum"
        );

        // Enough entries to push a directory past its first block, so the tail
        // record is kept on a block allocated here
        let dir = fs.ext2_create_dir(2, "csum", 0o755).expect("mkdir failed");
        for i in 0..64 {
            let name = format!("entry-{i:03}-with-a-longish-name.txt");
            let ino = fs
                .ext2_create_file(dir, &name, 0o644)
                .unwrap_or_else(|e| panic!("create {name}: {e:?}"));
            let body = format!("{name}\n").repeat(20);
            fs.ext2_write_file(ino, body.as_bytes(), 0)
                .unwrap_or_else(|e| panic!("write {name}: {e:?}"));
        }
        // A delete frees blocks and inodes, touching both bitmap checksums and
        // the group descriptor
        fs.ext2_delete_file(dir, "entry-007-with-a-longish-name.txt")
            .expect("delete failed");
        fs.sync().expect("sync failed");
    }

    let path = dump(&mut disk, "metadata-csum");
    let feats = features(&path);
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert!(
        feats.contains("metadata_csum"),
        "the image must really declare it: {feats}"
    );
    assert_eq!(
        code, 0,
        "e2fsck rejects our metadata_csum ext4 (exit {code}):\n{report}"
    );
}

/// Forces an extent tree deep enough for a node on disk, and checks its tail
/// record. A root in the inode holds four entries, and a straight write gets
/// one extent, so growing files in turn interleaves blocks until the tree grows
/// into a block ending in a checksum e2fsck verifies
#[test]
fn e2fsck_accepts_an_extent_tree_we_grew() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    // 1 KiB blocks hold 84 extents per node, so 400 scattered blocks per file
    // reach three levels
    params.block_size = 1024;
    mkfs(&mut disk, &params).expect("mkfs failed");

    let deepest;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        assert!(fs.superblock.has_metadata_csum());

        let bs = fs.block_size as usize;
        let mut inos = Vec::new();
        for i in 0..6 {
            inos.push(
                fs.ext4_create_file(2, &format!("interleaved-{i}.bin"), 0o644)
                    .expect("create failed"),
            );
        }
        // Round-robin one block at a time, so no file gets a contiguous run
        let block: Vec<u8> = (0..bs).map(|i| (i % 251) as u8).collect();
        for round in 0..400u64 {
            for &ino in &inos {
                fs.ext4_write_file(ino, &block, round * bs as u64)
                    .expect("write failed");
            }
        }

        deepest = inos
            .iter()
            .map(|&ino| {
                let inode = fs.read_inode(ino).unwrap();
                fs.extent_root(&inode).unwrap().depth
            })
            .max()
            .unwrap();

        // Reads one back, running the verification side
        let mut back = vec![0u8; bs];
        let inode = fs.read_inode(inos[0]).unwrap();
        let n = fs.read_file(&inode, 0, &mut back).expect("read failed");
        assert_eq!(&back[..n], &block[..n]);

        fs.sync().expect("sync failed");
    }

    assert!(
        deepest >= 2,
        "the interleaving did not push any file out of its inode - this test \
         proves nothing at depth {deepest}"
    );

    let path = dump(&mut disk, "extent-tree");
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert_eq!(
        code, 0,
        "e2fsck rejects an extent tree we grew (exit {code}):\n{report}"
    );
}

/// A filesystem made by `mke2fs`, written here, and judged by e2fsck. Its
/// layout has none of this code's assumptions, and a 1 KiB block size places
/// directory tails and the inode table where local mkfs does not
#[test]
fn e2fsck_accepts_a_foreign_filesystem_we_wrote_to() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let mke2fs = Command::new("mke2fs").arg("-V").output();
    if mke2fs.is_err() {
        eprintln!("mke2fs not installed - skipping");
        return;
    }

    let mut path = std::env::temp_dir();
    path.push(format!("extfs-test-foreign-{}.img", std::process::id()));
    let blocks = 16384; // 16 MiB at 1 KiB blocks

    let made = Command::new("mke2fs")
        .args([
            "-q", "-F", "-t", "ext4", "-b", "1024", "-I", "256",
            // Few enough inodes that filling group 0 spills into group 1, which
            // mke2fs marks INODE_UNINIT
            "-N", "128",
            "-O", "metadata_csum,extent,^has_journal,^64bit",
        ])
        .arg(&path)
        .arg(blocks.to_string())
        .status()
        .expect("mke2fs did not run");
    assert!(made.success(), "mke2fs failed");

    let image = std::fs::read(&path).expect("cannot read the image mke2fs made");
    let mut disk = MemDisk::from_image(image);

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("cannot mount a real ext4");
        assert!(fs.superblock.has_metadata_csum());
        assert_eq!(fs.block_size, 1024);

        assert!(
            fs.groups[1].flags_gd() & extfs::structs::EXT4_BG_INODE_UNINIT != 0,
            "the sample must have a group whose bitmaps were never written"
        );

        let dir = fs.ext2_create_dir(2, "ours", 0o755).expect("mkdir failed");
        for i in 0..60 {
            let name = format!("f{i:02}-written-by-mikuos.txt");
            let ino = fs
                .ext2_create_file(dir, &name, 0o644)
                .unwrap_or_else(|e| panic!("create {name}: {e:?}"));
            let body = format!("{name}\n").repeat(30);
            fs.ext2_write_file(ino, body.as_bytes(), 0)
                .unwrap_or_else(|e| panic!("write {name}: {e:?}"));
        }
        fs.ext2_delete_file(dir, "f05-written-by-mikuos.txt")
            .expect("delete failed");
        assert_eq!(
            fs.groups[1].flags_gd() & extfs::structs::EXT4_BG_INODE_UNINIT,
            0,
            "allocating out of that group must have initialised it"
        );
        fs.sync().expect("sync failed");
    }

    std::fs::write(&path, disk.image_flushed()).expect("cannot write the image back");
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert_eq!(
        code, 0,
        "e2fsck rejects a real ext4 after we wrote to it (exit {code}):\n{report}"
    );
}

/// Reserved, unwritten extents judged from outside, where the unwritten bit
/// matters
#[test]
fn e2fsck_accepts_reserved_space() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    params.block_size = 1024;
    mkfs(&mut disk, &params).expect("mkfs failed");

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        let bs = fs.block_size as usize;

        let untouched = fs.ext4_create_file(2, "untouched.bin", 0o644).unwrap();
        fs.ext4_fallocate(untouched, 0, 256 * 1024).expect("fallocate failed");

        // One written into, so its extent splits three ways: unwritten,
        // written, unwritten
        let partly = fs.ext4_create_file(2, "partly.bin", 0o644).unwrap();
        fs.ext4_fallocate(partly, 0, 256 * 1024).expect("fallocate failed");
        for round in [1u64, 17, 99, 200] {
            let body = vec![round as u8; bs];
            fs.ext4_write_file(partly, &body, round * bs as u64).unwrap();
        }
        fs.sync().expect("sync failed");
    }

    let path = dump(&mut disk, "fallocate");
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert_eq!(
        code, 0,
        "e2fsck rejects reserved space (exit {code}):\n{report}"
    );
}

/// Extended attributes are in their own block, which on metadata_csum has a
/// checksum keyed by block number, the one exception among ext4's checksums
#[test]
fn e2fsck_accepts_our_extended_attributes() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        assert!(fs.superblock.has_metadata_csum());
        let ino = fs.ext4_create_file(2, "tagged.bin", 0o644).unwrap();
        fs.ext4_write_file(ino, b"some contents\n", 0).unwrap();
        fs.set_xattr(ino, extfs::xattr::XATTR_INDEX_USER, "colour", b"teal")
            .expect("setxattr failed");
        fs.set_xattr(ino, extfs::xattr::XATTR_INDEX_USER, "note", b"written by mikuos")
            .expect("setxattr failed");
        let mut back = [0u8; 64];
        let n = fs
            .get_xattr(ino, extfs::xattr::XATTR_INDEX_USER, "colour", &mut back)
            .expect("getxattr failed");
        assert_eq!(&back[..n], b"teal");
        fs.sync().expect("sync failed");
    }

    let path = dump(&mut disk, "xattr");
    let (code, report) = fsck(&path);
    cleanup(&path);

    assert_eq!(
        code, 0,
        "e2fsck rejects our extended attributes (exit {code}):\n{report}"
    );
}

/// Deleting a file with attributes returns the attribute block too, as it is
/// not in the block map
#[test]
fn e2fsck_accepts_a_deleted_file_that_had_attributes() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");

    let free_before;
    let free_after;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        free_before = fs.superblock.free_blocks_count();
        let ino = fs.ext4_create_file(2, "tagged.bin", 0o644).unwrap();
        fs.ext4_write_file(ino, b"contents\n", 0).unwrap();
        fs.set_xattr(ino, extfs::xattr::XATTR_INDEX_USER, "colour", b"teal")
            .unwrap();
        fs.ext4_delete_file(2, "tagged.bin").expect("delete failed");
        free_after = fs.superblock.free_blocks_count();
        fs.sync().expect("sync failed");
    }

    assert_eq!(
        free_before, free_after,
        "deleting a file with attributes leaked {} blocks",
        free_before as i64 - free_after as i64
    );

    let path = dump(&mut disk, "xattr-deleted");
    let (code, report) = fsck(&path);
    cleanup(&path);
    assert_eq!(code, 0, "e2fsck rejects the image (exit {code}):\n{report}");
}

// The backup superblocks

/// `e2fsck -b <backup>`: checks using a backup superblock, the recovery path
/// when the front of the disk is damaged. Any report is about a superblock
/// written here
fn fsck_from_backup(path: &std::path::Path, backup: u32, block_size: u32) -> (i32, String) {
    let out = Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg("-b")
        .arg(backup.to_string())
        .arg("-B")
        .arg(block_size.to_string())
        .arg(path)
        .output()
        .expect("e2fsck did not run");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), report)
}

#[test]
fn e2fsck_accepts_our_backup_superblocks() {
    if !have_e2fsck() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    // At 4 KiB a group spans 128 MiB, so the ext4 case needs several times the
    // 64 MiB used elsewhere, or there is no backup
    for (kind, block_size, sectors) in [
        (FsType::Ext2, 1024u32, SECTORS),
        (FsType::Ext4, 4096u32, 1024 * 1024usize),
    ] {
        let mut disk = MemDisk::new(sectors);
        let mut params = MkfsParams::new(kind, 0);
        params.total_sectors = sectors as u32;
        params.block_size = block_size;
        mkfs(&mut disk, &params).expect("mkfs failed");

        // The backup in group 1, which sparse_super always keeps
        let (backup_block, groups) = {
            let fs = MikuFS::open(&mut disk, 0, false).unwrap();
            (
                fs.superblock.first_data_block() + fs.blocks_per_group,
                fs.group_count,
            )
        };
        assert!(groups >= 2, "{kind:?}: this case needs more than one group");

        // Moves the volume away from its mkfs state, then leaves it as the
        // kernel does at umount
        {
            let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
            let dir = fs.ext3_create_dir(2, "things", 0o755).unwrap();
            for i in 0..64 {
                let mut name = String::from("f");
                name.push_str(&i.to_string());
                let ino = fs.ext3_create_file(dir, &name, 0o644).unwrap();
                fs.ext3_write_file(ino, &[b'y'; 4096], 0).unwrap();
            }
            fs.finish_unmount().unwrap();
        }

        let path = dump(&mut disk, &format!("backup-{kind:?}"));

        let (primary_code, primary_report) = fsck(&path);
        let (code, report) = fsck_from_backup(&path, backup_block, block_size);
        cleanup(&path);

        assert_eq!(
            primary_code, 0,
            "{kind:?}: the primary is not clean, so the backup proves nothing:\n{primary_report}"
        );
        assert_eq!(
            code, 0,
            "{kind:?}: e2fsck refused the backup superblock at block {backup_block}:\n{report}"
        );
    }
}
