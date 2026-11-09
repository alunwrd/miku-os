// Laying a filesystem down and reading it back: what `mkfs` writes is what
// `open` reads

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::mount::{check_superblock, MountReject};
use extfs::structs::*;
use extfs::{ExtHost, MemDisk, MikuFS};

/// 64 MiB: several block groups at a 1 KiB block size
const SECTORS: usize = 128 * 1024;

fn format(kind: FsType) -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

#[test]
fn mkfs_then_mount_ext2() {
    let mut disk = format(FsType::Ext2);
    let fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
    assert_eq!(fs.block_size, 1024);
    assert!(fs.group_count >= 2, "64 MiB at 1 KiB blocks is many groups");
    assert_eq!(fs.groups.len(), fs.group_count as usize);
}

#[test]
fn mkfs_then_mount_ext3() {
    let mut disk = format(FsType::Ext3);
    let fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
    assert!(fs.superblock.has_journal(), "ext3 must declare a journal");
}

#[test]
fn mkfs_then_mount_ext4() {
    let mut disk = format(FsType::Ext4);
    let fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
    assert_eq!(fs.block_size, 4096);
    assert!(fs.superblock.has_extents(), "ext4 must declare extents");
    assert!(fs.superblock.has_journal(), "ext4 must declare a journal");
}

/// The flavor reported at mount is what was laid down, as the mount log prints
#[test]
fn version_string_round_trips() {
    for (kind, want) in [
        (FsType::Ext2, "ext2"),
        (FsType::Ext3, "ext3"),
        (FsType::Ext4, "ext4"),
    ] {
        let mut disk = format(kind);
        let fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        assert_eq!(fs.superblock.fs_version_str(), want);
    }
}

// Hostile superblocks
//
// Not from any `mkfs`: from a corrupted sector, a truncated download, or a
// fuzzer. Each would reach code dividing by or indexing with the value

fn good_superblock() -> Superblock {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).unwrap();
    let fs = MikuFS::open(&mut disk, 0, false).unwrap();
    fs.superblock
}

#[test]
fn a_good_superblock_passes() {
    let sb = good_superblock();
    let geo = check_superblock(&sb).expect("the image we just made must validate");
    assert_eq!(geo.block_size, 1024);
    assert!(geo.writable());
}

#[test]
fn bad_magic_is_rejected() {
    let mut sb = good_superblock();
    sb.write_u16(56, 0xDEAD);
    assert!(matches!(
        check_superblock(&sb),
        Err(MountReject::BadMagic(0xDEAD))
    ));
}

/// `block_size` is `1024 << s_log_block_size`. At 22 the shift overflows a u32;
/// a release build masks the amount, so the mount would succeed with a nonsense
/// size
#[test]
fn absurd_log_block_size_is_rejected() {
    for log_bs in [7u32, 22, 31, 255] {
        let mut sb = good_superblock();
        sb.write_u32(24, log_bs);
        assert!(
            matches!(check_superblock(&sb), Err(MountReject::BadBlockSize(_))),
            "log_block_size {log_bs} must be refused"
        );
    }
}

/// `s_inodes_per_group` is the divisor in every inode-to-group path, starting
/// with the `read_inode` of orphan cleanup during mount
#[test]
fn zero_inodes_per_group_is_rejected() {
    let mut sb = good_superblock();
    sb.write_u32(40, 0);
    assert!(matches!(
        check_superblock(&sb),
        Err(MountReject::ZeroInodesPerGroup)
    ));
}

#[test]
fn zero_blocks_per_group_is_rejected() {
    let mut sb = good_superblock();
    sb.write_u32(32, 0);
    assert!(matches!(
        check_superblock(&sb),
        Err(MountReject::ZeroBlocksPerGroup)
    ));
}

/// More inodes in a group than its one-block bitmap has bits; believing it
/// scans past the bitmap's end
#[test]
fn inodes_per_group_beyond_the_bitmap_is_rejected() {
    let mut sb = good_superblock();
    sb.write_u32(40, 1024 * 8 + 1);
    assert!(check_superblock(&sb).is_err());
}

#[test]
fn unsupported_incompat_refuses_the_mount() {
    // meta_bg moves the group descriptor table, so reading is guesswork too
    let mut sb = good_superblock();
    let incompat = sb.feature_incompat();
    sb.write_u32(96, incompat | FEATURE_INCOMPAT_META_BG);
    match check_superblock(&sb) {
        Err(MountReject::UnsupportedIncompat(bits)) => {
            assert_eq!(bits, FEATURE_INCOMPAT_META_BG);
        }
        other => panic!("meta_bg must be refused, got {other:?}"),
    }
}

#[test]
fn encryption_refuses_the_mount() {
    let mut sb = good_superblock();
    let incompat = sb.feature_incompat();
    sb.write_u32(96, incompat | FEATURE_INCOMPAT_ENCRYPT);
    assert!(matches!(
        check_superblock(&sb),
        Err(MountReject::UnsupportedIncompat(_))
    ));
}

/// ro_compat: the layout is readable, so the volume mounts, but writing would
/// leave unmaintained structures inconsistent, so it comes up read-only
#[test]
fn unsupported_ro_compat_forces_read_only() {
    let mut sb = good_superblock();
    let ro = sb.feature_ro_compat();
    sb.write_u32(100, ro | FEATURE_RO_COMPAT_BIGALLOC);
    let geo = check_superblock(&sb).expect("bigalloc is readable, so it must still mount");
    assert!(!geo.writable(), "bigalloc must force read-only");
    assert_eq!(geo.unsupported_ro, FEATURE_RO_COMPAT_BIGALLOC);
}

#[test]
fn quota_forces_read_only() {
    let mut sb = good_superblock();
    let ro = sb.feature_ro_compat();
    sb.write_u32(100, ro | FEATURE_RO_COMPAT_QUOTA);
    let geo = check_superblock(&sb).unwrap();
    assert!(!geo.writable());
}

/// Local mkfs output must be inside the mask
#[test]
fn our_own_images_are_within_the_supported_masks() {
    for kind in [FsType::Ext2, FsType::Ext3, FsType::Ext4] {
        let mut disk = format(kind);
        let fs = MikuFS::open(&mut disk, 0, false)
            .unwrap_or_else(|e| panic!("{} must mount: {e:?}", kind.name()));
        assert!(
            !fs.reader.read_only,
            "{} must mount read-write, not be forced ro by its own feature bits",
            kind.name()
        );
    }
}

/// `large_dir` allows a three-level directory index; `DX_MAX_LEVELS` is 2, so
/// the mount is refused
#[test]
fn a_largedir_volume_is_refused_at_mount() {
    let mut sb = good_superblock();
    let incompat = sb.feature_incompat();
    sb.write_u32(96, incompat | FEATURE_INCOMPAT_LARGEDIR);
    match check_superblock(&sb) {
        Err(MountReject::UnsupportedIncompat(bits)) => {
            assert_eq!(bits, FEATURE_INCOMPAT_LARGEDIR);
        }
        other => panic!("large_dir must be refused, got {other:?}"),
    }
}

// Backup superblocks

/// `do_write_superblock` writes block 2 only. Backups must be current, or
/// `e2fsck -b` restores old feature bits
#[test]
fn the_backup_superblocks_follow_the_primary() {
    let mut disk = format(FsType::Ext2);

    let (first, bpg, groups, block_size) = {
        let fs = MikuFS::open(&mut disk, 0, false).unwrap();
        (
            fs.superblock.first_data_block(),
            fs.blocks_per_group,
            fs.group_count,
            fs.block_size,
        )
    };
    assert!(groups >= 2, "this case needs a group that keeps a backup");

    // Moves the free counters off their mkfs values, then leaves the volume as
    // the kernel does
    let free_before = {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let before = fs.superblock.free_blocks_count();
        for i in 0..64 {
            let mut name = String::from("f");
            name.push_str(&i.to_string());
            let ino = fs.ext2_create_file(2, &name, 0o644).unwrap();
            fs.ext2_write_file(ino, &[b'x'; 2048], 0).unwrap();
        }
        fs.finish_unmount().unwrap();
        before
    };

    let primary = {
        let fs = MikuFS::open(&mut disk, 0, false).unwrap();
        fs.superblock
    };
    assert_ne!(
        primary.free_blocks_count(),
        free_before,
        "the writes must have moved the free count, or this proves nothing"
    );

    // Read a backup straight off the medium, the way e2fsck would
    let mut found = 0;
    for g in 1..groups {
        if !extfs::mkfs::layout::group_has_sb(g) {
            continue;
        }
        let block = first + g * bpg;
        let lba = block * (block_size / 512);
        let mut raw = [0u8; 1024];
        disk.read_sectors(lba as u64, 2, &mut raw).unwrap();

        let mut backup = Superblock::zeroed();
        backup.data.copy_from_slice(&raw);

        assert_eq!(backup.magic(), EXT2_MAGIC, "group {g}: not a superblock");
        assert_eq!(
            backup.feature_incompat(),
            primary.feature_incompat(),
            "group {g}: the backup's feature bits lag the primary"
        );
        assert_eq!(
            backup.free_blocks_count(),
            primary.free_blocks_count(),
            "group {g}: the backup's free count lags the primary"
        );
        assert_eq!(
            backup.block_group_nr(),
            g as u16,
            "group {g}: e2fsck checks this field against the group it sits in"
        );
        found += 1;
    }
    assert!(found > 0, "no backup superblock was examined");
}
