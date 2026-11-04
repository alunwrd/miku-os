// Checksums against external values. Compute and verify could share a mistake
// and still round-trip, so vectors come from the CRC-32C specification and the
// ext4 on-disk format

use extfs::ext4::crc32c::{self, SUPERBLOCK_CSUM_OFFSET};
use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;

// The primitive

/// The CRC-32C (Castagnoli) check value: the CRC of ASCII "123456789" is
/// 0xE3069283
#[test]
fn crc32c_matches_the_published_check_value() {
    assert_eq!(crc32c::crc32c(0, b"123456789"), 0xE306_9283);
}

/// The empty message has no effect on the register
#[test]
fn crc32c_of_nothing_is_the_seed() {
    assert_eq!(crc32c::crc32c_raw(0, b""), 0);
    assert_eq!(crc32c::crc32c_raw(!0, b""), !0);
}

/// What ext4 depends on: summing in pieces matches summing whole. `crc32c_raw`
/// must not invert, since the inverting form cannot chain
#[test]
fn crc32c_raw_chains() {
    let whole = crc32c::crc32c_raw(!0, b"the quick brown fox");
    let a = crc32c::crc32c_raw(!0, b"the quick ");
    let split = crc32c::crc32c_raw(a, b"brown fox");
    assert_eq!(whole, split);
}

/// The two forms differ by one inversion at each end
#[test]
fn the_two_forms_differ_by_their_inversions() {
    for msg in [b"".as_slice(), b"x", b"123456789", b"the quick brown fox"] {
        assert_eq!(crc32c::crc32c(!0, msg), !crc32c::crc32c_raw(0, msg));
        assert_eq!(crc32c::crc32c_raw(!0, msg), !crc32c::crc32c(0, msg));
    }
}

/// ext4 metadata uses the raw form; the wrong one verifies only against itself
#[test]
fn the_forms_are_not_interchangeable() {
    let msg = b"the quick brown fox";
    assert_ne!(crc32c::crc32c(!0, msg), crc32c::crc32c_raw(!0, msg));
}

/// CRC16 as the legacy GDT_CSUM feature uses it
#[test]
fn crc16_matches_its_check_value() {
    // The IBM/ARC CRC-16 of "123456789" with seed 0 is 0xBB3D
    assert_eq!(crc32c::crc16(0, b"123456789"), 0xBB3D);
}

// The superblock

/// `s_checksum` is the last field of the 1024-byte superblock; 0xFE is
/// `s_desc_size` and half of `s_default_mount_opts`
#[test]
fn superblock_checksum_lives_at_0x3fc() {
    assert_eq!(SUPERBLOCK_CSUM_OFFSET, 1020);
    assert_eq!(SUPERBLOCK_CSUM_OFFSET, 0x3FC);
}

/// Covers everything before itself, and nothing after
#[test]
fn superblock_checksum_covers_everything_before_it() {
    let mut sb = [0u8; 1024];
    for (i, b) in sb.iter_mut().enumerate() {
        *b = i as u8;
    }
    let base = crc32c::ext4_superblock_csum(&sb);

    // A byte inside the covered range changes it
    sb[1019] ^= 0xFF;
    assert_ne!(crc32c::ext4_superblock_csum(&sb), base);
    sb[1019] ^= 0xFF;

    // The checksum field itself does not count
    sb[1020] ^= 0xFF;
    sb[1023] ^= 0xFF;
    assert_eq!(crc32c::ext4_superblock_csum(&sb), base);
}

/// Writing the checksum disturbs no other field, such as `s_desc_size` at 254
/// and the low half of `s_default_mount_opts` at 256
#[test]
fn stamping_the_superblock_leaves_other_fields_alone() {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).unwrap();

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    // Forces the feature on so the stamp runs
    let ro = fs.superblock.feature_ro_compat();
    fs.superblock
        .write_u32(100, ro | extfs::structs::FEATURE_RO_COMPAT_METADATA_CSUM);

    let desc_size = fs.superblock.desc_size();
    let mount_opts = fs.superblock.default_mount_opts();
    let blocks = fs.superblock.blocks_count();

    fs.update_superblock_csum();

    assert_eq!(fs.superblock.desc_size(), desc_size, "s_desc_size clobbered");
    assert_eq!(
        fs.superblock.default_mount_opts(),
        mount_opts,
        "s_default_mount_opts clobbered"
    );
    assert_eq!(fs.superblock.blocks_count(), blocks);
    assert!(fs.verify_superblock_csum(), "what we stamped must verify");
}

/// And a single flipped bit anywhere in the superblock must be caught
#[test]
fn superblock_checksum_catches_a_flipped_bit() {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).unwrap();

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let ro = fs.superblock.feature_ro_compat();
    fs.superblock
        .write_u32(100, ro | extfs::structs::FEATURE_RO_COMPAT_METADATA_CSUM);
    fs.update_superblock_csum();
    assert!(fs.verify_superblock_csum());

    // Corrupt the free block count, the way a torn write would
    let free = fs.superblock.free_blocks_count();
    fs.superblock.write_u32(12, free ^ 1);
    assert!(
        !fs.verify_superblock_csum(),
        "a flipped bit in a covered field must fail verification"
    );
}

// Seeds

/// Without INCOMPAT_CSUM_SEED the seed is the UUID folded into ~0; with it, the
/// stored value is used verbatim, for volumes whose UUID changed
#[test]
fn csum_seed_honours_the_stored_value() {
    let uuid = [0x11u8; 16];
    let derived = crc32c::ext4_csum_seed(&uuid, None);
    let stored = crc32c::ext4_csum_seed(&uuid, Some(0xDEAD_BEEF));
    assert_eq!(stored, 0xDEAD_BEEF);
    assert_ne!(derived, stored);
    assert_eq!(derived, crc32c::crc32c_raw(!0, &uuid));
}

/// Different volumes give different checksums for the same bytes, so a copied
/// block does not validate
#[test]
fn different_uuids_give_different_seeds() {
    let a = crc32c::ext4_csum_seed(&[0xAAu8; 16], None);
    let b = crc32c::ext4_csum_seed(&[0xBBu8; 16], None);
    assert_ne!(a, b);
}

// Group descriptors and inodes: the field is skipped, not zeroed

/// The checksum field is skipped from its own coverage; zeroing gives another
/// answer
#[test]
fn group_desc_checksum_ignores_its_own_field() {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).unwrap();

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let ro = fs.superblock.feature_ro_compat();
    fs.superblock
        .write_u32(100, ro | extfs::structs::FEATURE_RO_COMPAT_METADATA_CSUM);

    let base = fs.compute_group_desc_csum(0);
    // Scribbling on the field does not move the value
    fs.groups[0].write_u16(30, 0x5A5A);
    assert_eq!(fs.compute_group_desc_csum(0), base);

    // But a real field must move it
    fs.groups[0].write_u16(12, 0x1234);
    assert_ne!(fs.compute_group_desc_csum(0), base);
}

/// Each group's checksum is keyed by its number, so a moved descriptor does not
/// validate
#[test]
fn group_desc_checksum_is_keyed_by_group_number() {
    // ext2 at 1 KiB blocks, so 64 MiB is several groups. At ext4's 4 KiB a
    // 32768-block group covers the image
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).unwrap();

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let ro = fs.superblock.feature_ro_compat();
    fs.superblock
        .write_u32(100, ro | extfs::structs::FEATURE_RO_COMPAT_METADATA_CSUM);

    // Make two descriptors byte-identical apart from where they sit
    assert!(fs.groups.len() >= 2, "test needs a multi-group image");
    let copy = fs.groups[0].data;
    fs.groups[1].data = copy;
    assert_ne!(
        fs.compute_group_desc_csum(0),
        fs.compute_group_desc_csum(1),
        "identical descriptors in different groups must not share a checksum"
    );
}

#[test]
fn stamped_group_desc_verifies() {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).unwrap();

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let ro = fs.superblock.feature_ro_compat();
    fs.superblock
        .write_u32(100, ro | extfs::structs::FEATURE_RO_COMPAT_METADATA_CSUM);

    fs.update_group_desc_csum(0);
    assert!(fs.verify_group_desc_csum(0));

    fs.groups[0].write_u32(0, 0xDEAD);
    assert!(!fs.verify_group_desc_csum(0), "a changed descriptor must fail");
}

// The two crc32c implementations
//
// One folds eight bytes at a time through a table; the other is the CPU's
// SSE4.2 instruction. Which runs depends on the machine, so they must be
// bit-identical.

use extfs::ext4::crc32c::{crc32c_raw, crc32c_raw_table};

/// Deterministic byte stream, for a reproducible failure
fn stream(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 24) as u8
        })
        .collect()
}

#[test]
fn the_two_implementations_agree() {
    // Every length past two eight-byte steps, then larger ones: tails are where
    // a wide loop goes wrong
    for len in 0..40 {
        let data = stream(len, 0x1234_5678);
        assert_eq!(
            crc32c_raw(0, &data),
            crc32c_raw_table(0, &data),
            "length {len}"
        );
    }
    for len in [63usize, 64, 65, 255, 256, 1024, 4096, 4097] {
        let data = stream(len, len as u32);
        assert_eq!(
            crc32c_raw(0xDEAD_BEEF, &data),
            crc32c_raw_table(0xDEAD_BEEF, &data),
            "length {len}"
        );
    }
}

/// Both forms chain the same way: one call and two calls over the same bytes
/// must agree
#[test]
fn both_forms_chain() {
    let data = stream(1000, 99);
    for split in [0usize, 1, 7, 8, 9, 500, 999, 1000] {
        let (a, b) = data.split_at(split);
        assert_eq!(
            crc32c_raw(crc32c_raw(0, a), b),
            crc32c_raw(0, &data),
            "split at {split}"
        );
        assert_eq!(
            crc32c_raw_table(crc32c_raw_table(0, a), b),
            crc32c_raw_table(0, &data),
            "split at {split}"
        );
    }
}
