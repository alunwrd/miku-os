// On-disk structures by offset
//
// Offsets and sizes came from the compiler: a C program including
// `refsrc/linux/include/uapi/linux/btrfs_tree.h` printed `offsetof` per field.
//
// Read through accessors over a byte slice, not `#[repr(packed)]` types: a
// packed struct invites unaligned references, and each field comes from a
// foreign disk and needs a bounds check.

use crate::error::BtrfsError;

// Superblock

/// `_BHRfS_M`, at byte 64 of the superblock
pub const BTRFS_MAGIC: u64 = 0x4D5F_5366_5248_425F;

/// The primary superblock is at 64 KiB, past partition tables and boot sectors
pub const SUPER_INFO_OFFSET: u64 = 65536;
pub const SUPER_INFO_SIZE: usize = 4096;

/// Superblock copies: `btrfs_sb_offset` in `fs/btrfs/disk-io.h`, 64 KiB, then
/// 16 KiB shifted left by 12 bits per mirror. A volume smaller than an offset
/// lacks that copy.
///
/// Three: `BTRFS_SUPER_MIRROR_MAX 3`, as `write_all_supers` and
/// `btrfs_read_dev_super` use. 1 PiB is not a copy; `exclude_super_stripes` in
/// `block-group.c` does not reserve it, so it is allocatable space
pub const SUPER_MIRRORS: [u64; 3] = [
    65536,           // 64 KiB
    67_108_864,      // 64 MiB
    274_877_906_944, // 256 GiB
];

pub const CSUM_SIZE: usize = 32;
pub const FSID_SIZE: usize = 16;
pub const UUID_SIZE: usize = 16;
pub const LABEL_SIZE: usize = 256;
pub const SYS_CHUNK_ARRAY_SIZE: usize = 2048;

/// Field offsets in `struct btrfs_super_block`
pub mod sb {
    pub const CSUM: usize = 0;
    pub const FSID: usize = 32;
    pub const BYTENR: usize = 48;
    pub const FLAGS: usize = 56;
    pub const MAGIC: usize = 64;
    pub const GENERATION: usize = 72;
    pub const ROOT: usize = 80;
    pub const CHUNK_ROOT: usize = 88;
    pub const LOG_ROOT: usize = 96;
    pub const TOTAL_BYTES: usize = 112;
    pub const BYTES_USED: usize = 120;
    pub const ROOT_DIR_OBJECTID: usize = 128;
    pub const NUM_DEVICES: usize = 136;
    pub const SECTORSIZE: usize = 144;
    pub const NODESIZE: usize = 148;
    /// Unused since the format began; the kernel checks it equals `nodesize`
    pub const UNUSED_LEAFSIZE: usize = 152;
    pub const STRIPESIZE: usize = 156;
    pub const SYS_CHUNK_ARRAY_SIZE_OFF: usize = 160;
    pub const CHUNK_ROOT_GENERATION: usize = 164;
    pub const COMPAT_FLAGS: usize = 172;
    pub const COMPAT_RO_FLAGS: usize = 180;
    pub const INCOMPAT_FLAGS: usize = 188;
    pub const CSUM_TYPE: usize = 196;
    pub const ROOT_LEVEL: usize = 198;
    pub const CHUNK_ROOT_LEVEL: usize = 199;
    pub const LOG_ROOT_LEVEL: usize = 200;
    pub const DEV_ITEM: usize = 201;
    pub const LABEL: usize = 299;
    pub const CACHE_GENERATION: usize = 555;
    pub const UUID_TREE_GENERATION: usize = 563;
    pub const METADATA_UUID: usize = 571;
    pub const SYS_CHUNK_ARRAY: usize = 811;
}

/// `dev_item.fsid`, from the superblock's start. The kernel checks it against
/// the metadata uuid
pub const DEV_ITEM_FSID: usize = sb::DEV_ITEM + 82;

// Features

pub const FEATURE_COMPAT_RO_FREE_SPACE_TREE: u64 = 1 << 0;
pub const FEATURE_COMPAT_RO_FREE_SPACE_TREE_VALID: u64 = 1 << 1;
pub const FEATURE_COMPAT_RO_VERITY: u64 = 1 << 2;
pub const FEATURE_COMPAT_RO_BLOCK_GROUP_TREE: u64 = 1 << 3;

pub const FEATURE_INCOMPAT_MIXED_BACKREF: u64 = 1 << 0;
pub const FEATURE_INCOMPAT_DEFAULT_SUBVOL: u64 = 1 << 1;
pub const FEATURE_INCOMPAT_MIXED_GROUPS: u64 = 1 << 2;
pub const FEATURE_INCOMPAT_COMPRESS_LZO: u64 = 1 << 3;
pub const FEATURE_INCOMPAT_COMPRESS_ZSTD: u64 = 1 << 4;
pub const FEATURE_INCOMPAT_BIG_METADATA: u64 = 1 << 5;
pub const FEATURE_INCOMPAT_EXTENDED_IREF: u64 = 1 << 6;
pub const FEATURE_INCOMPAT_RAID56: u64 = 1 << 7;
pub const FEATURE_INCOMPAT_SKINNY_METADATA: u64 = 1 << 8;
pub const FEATURE_INCOMPAT_NO_HOLES: u64 = 1 << 9;
pub const FEATURE_INCOMPAT_METADATA_UUID: u64 = 1 << 10;
pub const FEATURE_INCOMPAT_RAID1C34: u64 = 1 << 11;
pub const FEATURE_INCOMPAT_ZONED: u64 = 1 << 12;
pub const FEATURE_INCOMPAT_EXTENT_TREE_V2: u64 = 1 << 13;
pub const FEATURE_INCOMPAT_RAID_STRIPE_TREE: u64 = 1 << 14;
pub const FEATURE_INCOMPAT_SIMPLE_QUOTA: u64 = 1 << 16;
pub const FEATURE_INCOMPAT_REMAP_TREE: u64 = 1 << 17;

/// Superblock flags the kernel tolerates (`BTRFS_SUPER_FLAG_SUPP`,
/// `fs/btrfs/disk-io.c`)
pub const SUPER_FLAG_SUPP: u64 = (1 << 0)   // HEADER_FLAG_WRITTEN
    | (1 << 1)                              // HEADER_FLAG_RELOC
    | (1 << 2)                              // SUPER_FLAG_ERROR
    | (1 << 32)                             // SEEDING
    | (1 << 33)                             // METADUMP
    | (1 << 34); // METADUMP_V2

// csum and size

pub const CSUM_TYPE_CRC32: u16 = 0;
pub const CSUM_TYPE_XXHASH: u16 = 1;
pub const CSUM_TYPE_SHA256: u16 = 2;
pub const CSUM_TYPE_BLAKE2: u16 = 3;

pub const MAX_LEVEL: u8 = 8;
pub const MIN_BLOCKSIZE: u32 = 4096;
pub const MAX_METADATA_BLOCKSIZE: u32 = 65536;

// Block profiles

pub const BLOCK_GROUP_DATA: u64 = 1 << 0;
pub const BLOCK_GROUP_SYSTEM: u64 = 1 << 1;
pub const BLOCK_GROUP_METADATA: u64 = 1 << 2;
pub const BLOCK_GROUP_RAID0: u64 = 1 << 3;
pub const BLOCK_GROUP_RAID1: u64 = 1 << 4;
pub const BLOCK_GROUP_DUP: u64 = 1 << 5;
pub const BLOCK_GROUP_RAID10: u64 = 1 << 6;
pub const BLOCK_GROUP_RAID5: u64 = 1 << 7;
pub const BLOCK_GROUP_RAID6: u64 = 1 << 8;
pub const BLOCK_GROUP_RAID1C3: u64 = 1 << 9;
pub const BLOCK_GROUP_RAID1C4: u64 = 1 << 10;

/// Bits saying where copies are, as against what the chunk holds
pub const BLOCK_GROUP_PROFILE_MASK: u64 = BLOCK_GROUP_RAID0
    | BLOCK_GROUP_RAID1
    | BLOCK_GROUP_DUP
    | BLOCK_GROUP_RAID10
    | BLOCK_GROUP_RAID5
    | BLOCK_GROUP_RAID6
    | BLOCK_GROUP_RAID1C3
    | BLOCK_GROUP_RAID1C4;

// Keys and items

/// `struct btrfs_disk_key`: 17 bytes, the type byte between two u64s, so
/// nothing is aligned
pub const DISK_KEY_SIZE: usize = 17;
pub const ITEM_SIZE: usize = 25;
pub const KEY_PTR_SIZE: usize = 33;
pub const HEADER_SIZE: usize = 101;

pub const CHUNK_ITEM_KEY: u8 = 228;

/// `struct btrfs_chunk` with one stripe; each further stripe adds `STRIPE_SIZE`
pub const CHUNK_SIZE: usize = 80;
pub const STRIPE_SIZE: usize = 32;

/// Field offsets in `struct btrfs_chunk`
pub mod chunk {
    pub const LENGTH: usize = 0;
    pub const OWNER: usize = 8;
    pub const STRIPE_LEN: usize = 16;
    pub const TYPE: usize = 24;
    pub const IO_ALIGN: usize = 32;
    pub const IO_WIDTH: usize = 36;
    pub const SECTOR_SIZE: usize = 40;
    pub const NUM_STRIPES: usize = 44;
    pub const SUB_STRIPES: usize = 46;
    pub const STRIPES: usize = 48;
}

/// Field offsets in `struct btrfs_stripe`
pub mod stripe {
    pub const DEVID: usize = 0;
    pub const OFFSET: usize = 8;
    pub const DEV_UUID: usize = 16;
}

/// How many bytes a chunk item with this many stripes occupies
pub const fn chunk_item_size(num_stripes: u16) -> usize {
    CHUNK_SIZE + STRIPE_SIZE * (num_stripes as usize - 1)
}

// Reading
//
// Every read is bounds-checked and returns an error, as the bytes come from a
// foreign disk

#[inline]
pub fn u16_at(buf: &[u8], off: usize) -> Result<u16, BtrfsError> {
    let end = off.checked_add(2).ok_or(BtrfsError::Corrupt)?;
    let b = buf.get(off..end).ok_or(BtrfsError::Corrupt)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

#[inline]
pub fn u32_at(buf: &[u8], off: usize) -> Result<u32, BtrfsError> {
    let end = off.checked_add(4).ok_or(BtrfsError::Corrupt)?;
    let b = buf.get(off..end).ok_or(BtrfsError::Corrupt)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[inline]
pub fn u64_at(buf: &[u8], off: usize) -> Result<u64, BtrfsError> {
    let end = off.checked_add(8).ok_or(BtrfsError::Corrupt)?;
    let b = buf.get(off..end).ok_or(BtrfsError::Corrupt)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

#[inline]
pub fn u8_at(buf: &[u8], off: usize) -> Result<u8, BtrfsError> {
    buf.get(off).copied().ok_or(BtrfsError::Corrupt)
}

/// A key: `(objectid, type, offset)`, sorted in that order. The type between
/// the numbers lets btrfs range-scan everything about an object
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Key {
    pub objectid: u64,
    pub kind: u8,
    pub offset: u64,
}

impl Key {
    pub fn read(buf: &[u8], at: usize) -> Result<Self, BtrfsError> {
        Ok(Self {
            objectid: u64_at(buf, at)?,
            kind: u8_at(buf, at + 8)?,
            offset: u64_at(buf, at + 9)?,
        })
    }

    /// Lays a key down at `at`, in the 17 bytes `read` reads. Bounds-checked,
    /// since the buffer is a tree block sized by a superblock
    pub fn write(&self, buf: &mut [u8], at: usize) -> Result<(), BtrfsError> {
        let end = at.checked_add(DISK_KEY_SIZE).ok_or(BtrfsError::Corrupt)?;
        let b = buf.get_mut(at..end).ok_or(BtrfsError::Corrupt)?;
        b[0..8].copy_from_slice(&self.objectid.to_le_bytes());
        b[8] = self.kind;
        b[9..17].copy_from_slice(&self.offset.to_le_bytes());
        Ok(())
    }
}
