// Deciding if a volume can be opened, and how
//
// Shaped like `lib/fs/extfs/src/mount.rs`: a pure function over a superblock that
// describes the volume or names why it is refused, so bad superblocks can be
// tested in bulk.
//
// A bit in a supported mask means the layout is read correctly; an absent bit
// means it would be misread. Each absent bit has its reason below.

use crate::error::BtrfsError;
use crate::structs::*;

/// Incompat bits this driver reads correctly.
///
/// Absent:
///
///   COMPRESS_LZO     an extent's bytes are not its contents; needs two decompressors and
///   COMPRESS_ZSTD    a framing format
///   RAID56           parity reconstruction and a stripe layout not computed here
///   ZONED            the superblock lives elsewhere and writes are append-only
///   EXTENT_TREE_V2   a different extent tree, still changing in the kernel
///   RAID_STRIPE_TREE  mapping moves into a tree not read here
///   REMAP_TREE       another layer between logical and physical
///   MIXED_GROUPS     data and metadata share a chunk; type bits stop distinguishing
///   METADATA_UUID    tree blocks carry another uuid, needing one more field; untested
///   SIMPLE_QUOTA     accounting not maintained, harmless to a reader; untested
pub const INCOMPAT_SUPP: u64 = FEATURE_INCOMPAT_MIXED_BACKREF
    | FEATURE_INCOMPAT_DEFAULT_SUBVOL
    | FEATURE_INCOMPAT_BIG_METADATA
    | FEATURE_INCOMPAT_EXTENDED_IREF
    // Both mkfs defaults. SKINNY_METADATA shortens a metadata extent item's
    // key; NO_HOLES removes records for gaps, so a gap is an absent record
    | FEATURE_INCOMPAT_SKINNY_METADATA
    | FEATURE_INCOMPAT_NO_HOLES
    // RAID1C3/C4 are more mirrors; a reader taking the first copy is correct
    | FEATURE_INCOMPAT_RAID1C34;

/// Read-only-compat bits, accepted since a read-only mount is what they allow.
///
/// `BLOCK_GROUP_TREE` is needed: `mkfs.btrfs` sets it by default since v6.x. It
/// moves block group items into a tree a reader does not consult.
///
/// `VERITY`: its measurements are checked on write and read-for-verification,
/// neither done here
pub const COMPAT_RO_SUPP: u64 = FEATURE_COMPAT_RO_FREE_SPACE_TREE
    | FEATURE_COMPAT_RO_FREE_SPACE_TREE_VALID
    | FEATURE_COMPAT_RO_VERITY
    | FEATURE_COMPAT_RO_BLOCK_GROUP_TREE;

/// Why a volume was refused, naming the cause
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountReject {
    /// The eight bytes at offset 64 are not `_BHRfS_M`
    BadMagic(u64),
    /// `csum_type` names an algorithm other than crc32c, which would reject
    /// each block on a sound volume
    UnknownChecksumType(u16),
    /// The superblock does not match its checksum
    BadSuperblockChecksum,
    /// Incompat bits outside `INCOMPAT_SUPP`; the value is the unsupported
    /// subset
    UnsupportedIncompat(u64),
    /// Read-only-compat bits outside `COMPAT_RO_SUPP`
    UnsupportedCompatRo(u64),
    /// Superblock flags outside what the kernel tolerates
    UnsupportedFlags(u64),
    /// `sectorsize` or `nodesize` is not an in-range power of two
    BadBlockSize(u32),
    /// `nodesize` disagrees with the `leafsize` field that has to mirror it
    LeafsizeMismatch(u32, u32),
    /// A tree root is deeper than the format allows
    BadLevel(u8),
    /// A tree root is not on a sector boundary
    UnalignedRoot(u64),
    /// Zero devices, which describes nothing
    NoDevices,
    /// This copy says it lives elsewhere than where it was read: stale or
    /// deliberate
    WrongLocation { says: u64, read_from: u64 },
    /// The bootstrap chunk array is longer than its field, too short for one
    /// chunk, or describes a non-chunk
    BadSysChunkArray,
    /// `dev_item.fsid` names a different filesystem than the superblock does
    ForeignDevice,
    /// The kernel refuses `BLOCK_GROUP_TREE` without its two companion features
    InconsistentFeatures,
}

/// What a valid superblock says about the volume
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub sectorsize: u32,
    pub nodesize: u32,
    pub root: u64,
    pub root_level: u8,
    pub chunk_root: u64,
    pub chunk_root_level: u8,
    pub log_root: u64,
    pub total_bytes: u64,
    pub num_devices: u64,
    pub generation: u64,
    pub sys_chunk_array_size: u32,
    pub incompat: u64,
    pub compat_ro: u64,
}

/// A superblock as 4096 bytes, kept whole since the checksum covers the bytes
/// from 32 on
#[derive(Clone)]
pub struct Superblock {
    pub data: [u8; SUPER_INFO_SIZE],
}

impl Superblock {
    pub fn zeroed() -> Self {
        Self { data: [0u8; SUPER_INFO_SIZE] }
    }

    pub fn magic(&self) -> u64 {
        u64_at(&self.data, sb::MAGIC).unwrap_or(0)
    }
    pub fn bytenr(&self) -> u64 {
        u64_at(&self.data, sb::BYTENR).unwrap_or(0)
    }
    pub fn flags(&self) -> u64 {
        u64_at(&self.data, sb::FLAGS).unwrap_or(0)
    }
    pub fn generation(&self) -> u64 {
        u64_at(&self.data, sb::GENERATION).unwrap_or(0)
    }
    pub fn root(&self) -> u64 {
        u64_at(&self.data, sb::ROOT).unwrap_or(0)
    }
    pub fn chunk_root(&self) -> u64 {
        u64_at(&self.data, sb::CHUNK_ROOT).unwrap_or(0)
    }
    pub fn log_root(&self) -> u64 {
        u64_at(&self.data, sb::LOG_ROOT).unwrap_or(0)
    }
    pub fn total_bytes(&self) -> u64 {
        u64_at(&self.data, sb::TOTAL_BYTES).unwrap_or(0)
    }
    pub fn bytes_used(&self) -> u64 {
        u64_at(&self.data, sb::BYTES_USED).unwrap_or(0)
    }
    pub fn num_devices(&self) -> u64 {
        u64_at(&self.data, sb::NUM_DEVICES).unwrap_or(0)
    }
    pub fn sectorsize(&self) -> u32 {
        u32_at(&self.data, sb::SECTORSIZE).unwrap_or(0)
    }
    pub fn nodesize(&self) -> u32 {
        u32_at(&self.data, sb::NODESIZE).unwrap_or(0)
    }
    pub fn leafsize(&self) -> u32 {
        u32_at(&self.data, sb::UNUSED_LEAFSIZE).unwrap_or(0)
    }
    pub fn stripesize(&self) -> u32 {
        u32_at(&self.data, sb::STRIPESIZE).unwrap_or(0)
    }
    pub fn sys_chunk_array_size(&self) -> u32 {
        u32_at(&self.data, sb::SYS_CHUNK_ARRAY_SIZE_OFF).unwrap_or(0)
    }
    pub fn compat_ro_flags(&self) -> u64 {
        u64_at(&self.data, sb::COMPAT_RO_FLAGS).unwrap_or(0)
    }
    pub fn incompat_flags(&self) -> u64 {
        u64_at(&self.data, sb::INCOMPAT_FLAGS).unwrap_or(0)
    }
    pub fn csum_type(&self) -> u16 {
        u16_at(&self.data, sb::CSUM_TYPE).unwrap_or(0xFFFF)
    }
    pub fn root_level(&self) -> u8 {
        u8_at(&self.data, sb::ROOT_LEVEL).unwrap_or(0xFF)
    }
    pub fn chunk_root_level(&self) -> u8 {
        u8_at(&self.data, sb::CHUNK_ROOT_LEVEL).unwrap_or(0xFF)
    }
    pub fn log_root_level(&self) -> u8 {
        u8_at(&self.data, sb::LOG_ROOT_LEVEL).unwrap_or(0xFF)
    }
    pub fn fsid(&self) -> &[u8] {
        &self.data[sb::FSID..sb::FSID + FSID_SIZE]
    }
    pub fn metadata_uuid(&self) -> &[u8] {
        &self.data[sb::METADATA_UUID..sb::METADATA_UUID + FSID_SIZE]
    }
    pub fn dev_item_fsid(&self) -> &[u8] {
        &self.data[DEV_ITEM_FSID..DEV_ITEM_FSID + FSID_SIZE]
    }
    pub fn stored_csum(&self) -> &[u8] {
        &self.data[sb::CSUM..sb::CSUM + CSUM_SIZE]
    }

    /// The bootstrap chunk array, clipped to what the superblock says it
    /// filled; past `sys_chunk_array_size` are leftovers
    pub fn sys_chunk_array(&self) -> &[u8] {
        let n = (self.sys_chunk_array_size() as usize).min(SYS_CHUNK_ARRAY_SIZE);
        &self.data[sb::SYS_CHUNK_ARRAY..sb::SYS_CHUNK_ARRAY + n]
    }

    pub fn label(&self) -> &str {
        let raw = &self.data[sb::LABEL..sb::LABEL + LABEL_SIZE];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(LABEL_SIZE);
        core::str::from_utf8(&raw[..end]).unwrap_or("")
    }

    /// crc32c over everything after the checksum field: the standard pre- and
    /// post-inverted form, not ext4's raw one
    pub fn computed_csum(&self) -> u32 {
        fshost::crc32c::crc32c(0, &self.data[CSUM_SIZE..])
    }

    pub fn csum_matches(&self) -> bool {
        let stored = u32_at(&self.data, sb::CSUM).unwrap_or(0);
        // Only the first four bytes are used for crc32c; the rest of the
        // 32-byte field is zero
        stored == self.computed_csum()
    }

    pub fn has_incompat(&self, bit: u64) -> bool {
        self.incompat_flags() & bit != 0
    }
    pub fn has_compat_ro(&self, bit: u64) -> bool {
        self.compat_ro_flags() & bit != 0
    }
}

/// Judges a superblock read from `read_from`. Pure: no disk, allocation, or
/// clock. `read_from` is checked against the copy's own location; `None` skips
/// that
pub fn check_superblock(sb: &Superblock, read_from: Option<u64>) -> Result<Geometry, MountReject> {
    let magic = sb.magic();
    if magic != BTRFS_MAGIC {
        return Err(MountReject::BadMagic(magic));
    }

    // Checksum type first: later checks trust bytes it covers, and crc32c on a
    // sha256 volume rejects a sound disk
    let csum_type = sb.csum_type();
    if csum_type != CSUM_TYPE_CRC32 {
        return Err(MountReject::UnknownChecksumType(csum_type));
    }
    if !sb.csum_matches() {
        return Err(MountReject::BadSuperblockChecksum);
    }

    let flags = sb.flags();
    if flags & !SUPER_FLAG_SUPP != 0 {
        return Err(MountReject::UnsupportedFlags(flags & !SUPER_FLAG_SUPP));
    }

    let incompat = sb.incompat_flags();
    let unsupported = incompat & !INCOMPAT_SUPP;
    if unsupported != 0 {
        return Err(MountReject::UnsupportedIncompat(unsupported));
    }
    let compat_ro = sb.compat_ro_flags();
    let unsupported_ro = compat_ro & !COMPAT_RO_SUPP;
    if unsupported_ro != 0 {
        return Err(MountReject::UnsupportedCompatRo(unsupported_ro));
    }

    // The kernel's rule: block-group-tree needs free-space-tree and no-holes
    if compat_ro & FEATURE_COMPAT_RO_BLOCK_GROUP_TREE != 0
        && (compat_ro & FEATURE_COMPAT_RO_FREE_SPACE_TREE_VALID == 0
            || incompat & FEATURE_INCOMPAT_NO_HOLES == 0)
    {
        return Err(MountReject::InconsistentFeatures);
    }

    let sectorsize = sb.sectorsize();
    if !sectorsize.is_power_of_two()
        || sectorsize < MIN_BLOCKSIZE
        || sectorsize > MAX_METADATA_BLOCKSIZE
    {
        return Err(MountReject::BadBlockSize(sectorsize));
    }
    let nodesize = sb.nodesize();
    if !nodesize.is_power_of_two() || nodesize < sectorsize || nodesize > MAX_METADATA_BLOCKSIZE {
        return Err(MountReject::BadBlockSize(nodesize));
    }
    let leafsize = sb.leafsize();
    if leafsize != nodesize {
        return Err(MountReject::LeafsizeMismatch(leafsize, nodesize));
    }
    if !sb.stripesize().is_power_of_two() {
        return Err(MountReject::BadBlockSize(sb.stripesize()));
    }

    for level in [sb.root_level(), sb.chunk_root_level(), sb.log_root_level()] {
        if level >= MAX_LEVEL {
            return Err(MountReject::BadLevel(level));
        }
    }

    // A root off a sector boundary cannot be read
    for root in [sb.root(), sb.chunk_root(), sb.log_root()] {
        if root % sectorsize as u64 != 0 {
            return Err(MountReject::UnalignedRoot(root));
        }
    }

    if sb.num_devices() == 0 {
        return Err(MountReject::NoDevices);
    }

    // The device must belong to this filesystem. METADATA_UUID is not accepted,
    // so `fsid` is compared
    if sb.dev_item_fsid() != sb.fsid() {
        return Err(MountReject::ForeignDevice);
    }

    if let Some(at) = read_from {
        if sb.bytenr() != at {
            return Err(MountReject::WrongLocation { says: sb.bytenr(), read_from: at });
        }
    }

    check_sys_chunk_array(sb).map_err(|_| MountReject::BadSysChunkArray)?;

    Ok(Geometry {
        sectorsize,
        nodesize,
        root: sb.root(),
        root_level: sb.root_level(),
        chunk_root: sb.chunk_root(),
        chunk_root_level: sb.chunk_root_level(),
        log_root: sb.log_root(),
        total_bytes: sb.total_bytes(),
        num_devices: sb.num_devices(),
        generation: sb.generation(),
        sys_chunk_array_size: sb.sys_chunk_array_size(),
        incompat,
        compat_ro,
    })
}

/// Walks the bootstrap chunk array to confirm it is well formed. Each entry is
/// a key and a chunk; the key must say `CHUNK_ITEM`, and the chunk must be
/// SYSTEM, since this array maps only the chunk tree's blocks.
///
/// Bounds are checked before each read. Linux reads `num_stripes` first, safe
/// only with its fixed 2048-byte member
pub fn check_sys_chunk_array(sb: &Superblock) -> Result<(), BtrfsError> {
    let declared = sb.sys_chunk_array_size() as usize;
    if declared > SYS_CHUNK_ARRAY_SIZE {
        return Err(BtrfsError::Corrupt);
    }
    if declared < DISK_KEY_SIZE + CHUNK_SIZE {
        return Err(BtrfsError::Corrupt);
    }
    let arr = sb.sys_chunk_array();

    let mut cur = 0usize;
    while cur < declared {
        if cur + DISK_KEY_SIZE > declared {
            return Err(BtrfsError::Corrupt);
        }
        let key = Key::read(arr, cur)?;
        if key.kind != CHUNK_ITEM_KEY {
            return Err(BtrfsError::Corrupt);
        }
        cur += DISK_KEY_SIZE;

        // The fixed part must be present before `num_stripes` is read
        if cur + CHUNK_SIZE > declared {
            return Err(BtrfsError::Corrupt);
        }
        let num_stripes = u16_at(arr, cur + chunk::NUM_STRIPES)?;
        if num_stripes == 0 {
            return Err(BtrfsError::Corrupt);
        }
        let item = chunk_item_size(num_stripes);
        if cur + item > declared {
            return Err(BtrfsError::Corrupt);
        }
        let ty = u64_at(arr, cur + chunk::TYPE)?;
        if ty & BLOCK_GROUP_SYSTEM == 0 {
            return Err(BtrfsError::Corrupt);
        }
        cur += item;
    }
    Ok(())
}
