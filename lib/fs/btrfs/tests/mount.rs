// Superblocks that should be refused
//
// `check_superblock` is pure, so each case takes a real `mkfs.btrfs` one,
// changes one thing, and asks, as `tree-checker.c` guards the kernel against
// bad bytes

use btrfs::structs::*;
use btrfs::{check_superblock, MountReject, Superblock};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tool(name: &str) -> Option<PathBuf> {
    let vendored = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../refsrc/btrfs-progs")
        .join(name);
    if vendored.exists() {
        return Some(vendored);
    }
    let out = Command::new(name).arg("--version").output().ok()?;
    out.status.success().then(|| PathBuf::from(name))
}

/// One real superblock, made once for the process and cloned per case, since
/// these run in parallel. Real, so checks are not against a shape already
/// believed.
///
/// `None` means the tool is not installed, only; an `mkfs` failing panics
fn good_superblock() -> Option<Superblock> {
    static ONCE: std::sync::OnceLock<Option<Superblock>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let mkfs = tool("mkfs.btrfs")?;
        let mut path = std::env::temp_dir();
        path.push(format!("btrfs-mount-{}.img", std::process::id()));

        let f = std::fs::File::create(&path).expect("cannot create image");
        f.set_len(512 * 1024 * 1024).expect("cannot size image");
        drop(f);

        let out = Command::new(mkfs)
            .arg("-q")
            .arg(&path)
            .output()
            .expect("mkfs.btrfs did not run");
        assert!(
            out.status.success(),
            "mkfs.btrfs failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        let mut f = std::fs::File::open(&path).expect("cannot open image");
        f.seek(SeekFrom::Start(SUPER_INFO_OFFSET)).expect("cannot seek");
        let mut sb = Superblock::zeroed();
        f.read_exact(&mut sb.data).expect("cannot read superblock");
        let _ = std::fs::remove_file(&path);
        Some(sb)
    })
    .clone()
}

/// Sets a value and fixes the checksum, so only the case under test is wrong
fn poke_u64(sb: &mut Superblock, off: usize, val: u64) {
    sb.data[off..off + 8].copy_from_slice(&val.to_le_bytes());
    reseal(sb);
}
fn poke_u32(sb: &mut Superblock, off: usize, val: u32) {
    sb.data[off..off + 4].copy_from_slice(&val.to_le_bytes());
    reseal(sb);
}
fn poke_u8(sb: &mut Superblock, off: usize, val: u8) {
    sb.data[off] = val;
    reseal(sb);
}
fn reseal(sb: &mut Superblock) {
    let c = sb.computed_csum();
    sb.data[0..4].copy_from_slice(&c.to_le_bytes());
    sb.data[4..CSUM_SIZE].fill(0);
}

macro_rules! sb_or_skip {
    () => {
        match good_superblock() {
            Some(sb) => sb,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
}

#[test]
fn the_real_thing_passes() {
    let sb = sb_or_skip!();
    let geo = check_superblock(&sb, Some(SUPER_INFO_OFFSET))
        .expect("the image we just made must validate");
    assert_eq!(geo.sectorsize, 4096);
}

#[test]
fn bad_magic_is_refused() {
    let mut sb = sb_or_skip!();
    poke_u64(&mut sb, sb::MAGIC, 0xDEAD_BEEF);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::BadMagic(0xDEAD_BEEF))
    ));
}

/// The checksum is checked before anything trusts these bytes
#[test]
fn a_broken_checksum_is_refused() {
    let mut sb = sb_or_skip!();
    sb.data[sb::GENERATION] ^= 0xFF; // not resealed
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::BadSuperblockChecksum)
    ));
}

/// crc32c on a sha256 volume would reject every block, so the algorithm comes
/// first
#[test]
fn another_checksum_algorithm_is_refused_by_name() {
    for (ty, name) in [
        (CSUM_TYPE_XXHASH, "xxhash"),
        (CSUM_TYPE_SHA256, "sha256"),
        (CSUM_TYPE_BLAKE2, "blake2"),
    ] {
        let mut sb = sb_or_skip!();
        sb.data[sb::CSUM_TYPE..sb::CSUM_TYPE + 2].copy_from_slice(&ty.to_le_bytes());
        reseal(&mut sb);
        match check_superblock(&sb, None) {
            Err(MountReject::UnknownChecksumType(got)) => assert_eq!(got, ty, "{name}"),
            other => panic!("{name} must be refused by name, got {other:?}"),
        }
    }
}

/// Each refused incompat bit, one at a time, named in the rejection
#[test]
fn each_unsupported_incompat_bit_is_named() {
    for (bit, what) in [
        (FEATURE_INCOMPAT_COMPRESS_LZO, "lzo"),
        (FEATURE_INCOMPAT_COMPRESS_ZSTD, "zstd"),
        (FEATURE_INCOMPAT_RAID56, "raid56"),
        (FEATURE_INCOMPAT_ZONED, "zoned"),
        (FEATURE_INCOMPAT_EXTENT_TREE_V2, "extent-tree-v2"),
        (FEATURE_INCOMPAT_RAID_STRIPE_TREE, "raid-stripe-tree"),
        (FEATURE_INCOMPAT_REMAP_TREE, "remap-tree"),
        (FEATURE_INCOMPAT_MIXED_GROUPS, "mixed-groups"),
        (FEATURE_INCOMPAT_METADATA_UUID, "metadata-uuid"),
    ] {
        let mut sb = sb_or_skip!();
        let cur = sb.incompat_flags();
        poke_u64(&mut sb, sb::INCOMPAT_FLAGS, cur | bit);
        match check_superblock(&sb, None) {
            Err(MountReject::UnsupportedIncompat(got)) => {
                assert_eq!(got, bit, "{what}: the value must be the missing bit alone")
            }
            other => panic!("{what} must be refused, got {other:?}"),
        }
    }
}

/// Compression: an extent's bytes are not its contents, and raw reads look like
/// data
#[test]
fn compression_is_refused_rather_than_misread() {
    let mut sb = sb_or_skip!();
    let cur = sb.incompat_flags();
    poke_u64(&mut sb, sb::INCOMPAT_FLAGS, cur | FEATURE_INCOMPAT_COMPRESS_ZSTD);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::UnsupportedIncompat(_))
    ));
}

/// Read-only-compat bits are all accepted, as this driver reads only. A future
/// bit is not
#[test]
fn an_unknown_read_only_bit_is_still_refused() {
    let mut sb = sb_or_skip!();
    let cur = sb.compat_ro_flags();
    poke_u64(&mut sb, sb::COMPAT_RO_FLAGS, cur | (1 << 40));
    match check_superblock(&sb, None) {
        Err(MountReject::UnsupportedCompatRo(got)) => assert_eq!(got, 1 << 40),
        other => panic!("an undefined ro bit must be refused, got {other:?}"),
    }
}

/// The kernel's rule: block-group-tree needs free-space-tree and no-holes
#[test]
fn block_group_tree_without_its_companions_is_refused() {
    let mut sb = sb_or_skip!();
    let cur = sb.incompat_flags();
    poke_u64(&mut sb, sb::INCOMPAT_FLAGS, cur & !FEATURE_INCOMPAT_NO_HOLES);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::InconsistentFeatures)
    ));
}

#[test]
fn absurd_block_sizes_are_refused() {
    for bad in [0u32, 1, 3, 512, 2048, 131_072, u32::MAX] {
        let mut sb = sb_or_skip!();
        poke_u32(&mut sb, sb::SECTORSIZE, bad);
        assert!(
            matches!(check_superblock(&sb, None), Err(MountReject::BadBlockSize(_))),
            "sectorsize {bad} must be refused"
        );
    }
}

/// `nodesize` has a second copy in a long-unused field, which the kernel still
/// compares
#[test]
fn a_nodesize_that_disagrees_with_its_own_copy_is_refused() {
    let mut sb = sb_or_skip!();
    poke_u32(&mut sb, sb::UNUSED_LEAFSIZE, 8192);
    match check_superblock(&sb, None) {
        Err(MountReject::LeafsizeMismatch(leaf, node)) => {
            assert_eq!(leaf, 8192);
            assert_eq!(node, 16384);
        }
        other => panic!("a leafsize mismatch must be refused, got {other:?}"),
    }
}

/// A level past the format's maximum recurses too far
#[test]
fn a_tree_deeper_than_the_format_allows_is_refused() {
    for off in [sb::ROOT_LEVEL, sb::CHUNK_ROOT_LEVEL, sb::LOG_ROOT_LEVEL] {
        for bad in [MAX_LEVEL, MAX_LEVEL + 1, 200, 255] {
            let mut sb = sb_or_skip!();
            poke_u8(&mut sb, off, bad);
            assert!(
                matches!(check_superblock(&sb, None), Err(MountReject::BadLevel(_))),
                "level {bad} at offset {off} must be refused"
            );
        }
    }
}

/// A root off a sector boundary reads from the middle of something
#[test]
fn an_unaligned_tree_root_is_refused() {
    let mut sb = sb_or_skip!();
    let root = sb.root();
    poke_u64(&mut sb, sb::ROOT, root + 1);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::UnalignedRoot(_))
    ));
}

#[test]
fn a_volume_with_no_devices_is_refused() {
    let mut sb = sb_or_skip!();
    poke_u64(&mut sb, sb::NUM_DEVICES, 0);
    assert!(matches!(check_superblock(&sb, None), Err(MountReject::NoDevices)));
}

/// The device must belong to the superblock's filesystem
#[test]
fn a_device_belonging_to_another_filesystem_is_refused() {
    let mut sb = sb_or_skip!();
    sb.data[DEV_ITEM_FSID] ^= 0xFF;
    reseal(&mut sb);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::ForeignDevice)
    ));
}

// The bootstrap array
//
// Parsing a variable-length structure from disk, where a missing bound reads
// past the end

#[test]
fn a_sys_chunk_array_longer_than_its_field_is_refused() {
    let mut sb = sb_or_skip!();
    poke_u32(&mut sb, sb::SYS_CHUNK_ARRAY_SIZE_OFF, SYS_CHUNK_ARRAY_SIZE as u32 + 1);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::BadSysChunkArray)
    ));
}

#[test]
fn a_sys_chunk_array_too_small_for_one_chunk_is_refused() {
    for small in [0u32, 1, 16, (DISK_KEY_SIZE + CHUNK_SIZE) as u32 - 1] {
        let mut sb = sb_or_skip!();
        poke_u32(&mut sb, sb::SYS_CHUNK_ARRAY_SIZE_OFF, small);
        assert!(
            matches!(check_superblock(&sb, None), Err(MountReject::BadSysChunkArray)),
            "sys_array_size {small} must be refused"
        );
    }
}

/// A `num_stripes` big enough to run past the array: the length comes from the
/// disk
#[test]
fn a_stripe_count_that_overruns_the_array_is_refused() {
    for stripes in [u16::MAX, 1000, 60] {
        let mut sb = sb_or_skip!();
        let at = sb::SYS_CHUNK_ARRAY + DISK_KEY_SIZE + chunk::NUM_STRIPES;
        sb.data[at..at + 2].copy_from_slice(&stripes.to_le_bytes());
        reseal(&mut sb);
        assert!(
            matches!(check_superblock(&sb, None), Err(MountReject::BadSysChunkArray)),
            "num_stripes {stripes} must be refused"
        );
    }
}

/// Zero stripes: no copies, and its size arithmetic underflows
#[test]
fn a_chunk_with_no_stripes_is_refused() {
    let mut sb = sb_or_skip!();
    let at = sb::SYS_CHUNK_ARRAY + DISK_KEY_SIZE + chunk::NUM_STRIPES;
    sb.data[at..at + 2].copy_from_slice(&0u16.to_le_bytes());
    reseal(&mut sb);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::BadSysChunkArray)
    ));
}

/// The array holds only system chunks, to map the chunk tree's blocks
#[test]
fn a_non_system_chunk_in_the_bootstrap_array_is_refused() {
    let mut sb = sb_or_skip!();
    let at = sb::SYS_CHUNK_ARRAY + DISK_KEY_SIZE + chunk::TYPE;
    sb.data[at..at + 8].copy_from_slice(&BLOCK_GROUP_DATA.to_le_bytes());
    reseal(&mut sb);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::BadSysChunkArray)
    ));
}

#[test]
fn a_bootstrap_entry_that_is_not_a_chunk_item_is_refused() {
    let mut sb = sb_or_skip!();
    sb.data[sb::SYS_CHUNK_ARRAY + 8] = 1; // the key's type byte
    reseal(&mut sb);
    assert!(matches!(
        check_superblock(&sb, None),
        Err(MountReject::BadSysChunkArray)
    ));
}
