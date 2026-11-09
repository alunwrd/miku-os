// Which filesystem this is
//
// The flavor is inferred from feature bits, as e2fsprogs does: ext4 is a volume
// with a feature ext3 never had. Superblocks here came from
// `mke2fs -t ext2/ext3/ext4` images.

use extfs::structs::*;

const SB_EXT2: &[u8] = include_bytes!("data/sb-ext2.bin");
const SB_EXT3: &[u8] = include_bytes!("data/sb-ext3.bin");
const SB_EXT4: &[u8] = include_bytes!("data/sb-ext4.bin");
const SB_CSUM: &[u8] = include_bytes!("data/mke2fs-metadata-csum.bin");

fn sb(bytes: &[u8]) -> Superblock {
    let mut s = Superblock::zeroed();
    s.data.copy_from_slice(bytes);
    s
}

#[test]
fn the_three_reference_images_are_named_correctly() {
    assert_eq!(sb(SB_EXT2).fs_version_str(), "ext2");
    assert_eq!(sb(SB_EXT3).fs_version_str(), "ext3");
    assert_eq!(sb(SB_EXT4).fs_version_str(), "ext4");
}

#[test]
fn the_metadata_csum_image_is_ext4() {
    let s = sb(SB_CSUM);
    assert!(s.has_metadata_csum());
    assert_eq!(s.fs_version_str(), "ext4");
}

/// An ext4 feature outside extents, 64bit, and huge_file
#[test]
fn a_checksummed_volume_without_extents_is_still_ext4() {
    let mut s = sb(SB_EXT3);
    assert_eq!(s.fs_version_str(), "ext3");
    let ro = s.feature_ro_compat();
    s.write_u32(100, ro | FEATURE_RO_COMPAT_METADATA_CSUM);
    assert!(!s.has_extents() && !s.has_64bit());
    assert_eq!(
        s.fs_version_str(),
        "ext4",
        "a volume with metadata_csum is ext4 whatever else it has"
    );
}

#[test]
fn inline_data_alone_is_ext4_too() {
    let mut s = sb(SB_EXT3);
    let inc = s.feature_incompat();
    s.write_u32(96, inc | FEATURE_INCOMPAT_INLINE_DATA);
    assert_eq!(s.fs_version_str(), "ext4");
}

/// ext3 features stay ext3: a journal, recovery, and meta_bg predate ext4
#[test]
fn the_ext3_features_do_not_promote_it() {
    let mut s = sb(SB_EXT3);
    let inc = s.feature_incompat();
    s.write_u32(96, inc | FEATURE_INCOMPAT_RECOVER | FEATURE_INCOMPAT_META_BG);
    assert_eq!(s.fs_version_str(), "ext3");
}

/// An ext2 volume with nothing added is ext2 even when it is dirty
#[test]
fn ext2_stays_ext2() {
    let mut s = sb(SB_EXT2);
    assert!(!s.has_journal());
    s.write_u16(58, 2); // s_state: not cleanly unmounted
    assert_eq!(s.fs_version_str(), "ext2");
}

/// Local mkfs output names itself correctly too, in the superblock the mount
/// log and `df` read
#[test]
fn our_own_images_name_themselves() {
    use extfs::mkfs::{mkfs, FsType, MkfsParams};
    use extfs::{MemDisk, MikuFS};

    for (kind, want) in [
        (FsType::Ext2, "ext2"),
        (FsType::Ext3, "ext3"),
        (FsType::Ext4, "ext4"),
    ] {
        let mut disk = MemDisk::new(64 * 1024);
        let mut params = MkfsParams::new(kind, 0);
        params.total_sectors = 64 * 1024;
        mkfs(&mut disk, &params).expect("mkfs failed");
        let fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        assert_eq!(
            fs.superblock.fs_version_str(),
            want,
            "our {} image calls itself {}",
            want,
            fs.superblock.fs_version_str()
        );
    }
}
