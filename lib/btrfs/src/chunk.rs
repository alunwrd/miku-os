// Chunks: where a logical address is
//
// btrfs addresses everything in its own logical space, with the translation to
// devices in the chunk tree. No other tree can be read before this works. A
// mapping wrong by a stripe reads another valid block, surfacing as a checksum
// failure elsewhere.
//
// From `btrfs_max_io_len` and `set_io_stripe` in `fs/btrfs/volumes.c`:
//
//     offset        = logical - chunk.start
//     stripe_nr     = offset >> 16          (BTRFS_STRIPE_LEN is 64 KiB)
//     stripe_offset = offset & 0xFFFF
//     physical      = stripes[i].offset + stripe_offset + (stripe_nr << 16)
//
// For each accepted profile, `stripes[i]` covers the whole chunk and `i` picks
// a copy, so these collapse to `physical = stripes[i].offset + offset`. Valid
// only because striping profiles are refused.

use crate::error::BtrfsError;
use crate::structs::*;
use alloc::vec::Vec;

/// `BTRFS_STRIPE_LEN`; the kernel's chunk checker refuses anything else, as the
/// value is baked into every chunk
pub const STRIPE_LEN: u64 = 65536;

/// The chunk length the kernel calls bogus, to catch bitflips
pub const MAX_CHUNK_LEN: u64 = (u32::MAX as u64) * STRIPE_LEN;

/// One copy of a chunk on one device
#[derive(Clone, Copy, Debug)]
pub struct Stripe {
    pub devid: u64,
    /// Byte offset on that device where this copy of the chunk starts
    pub offset: u64,
}

/// How the copies of a chunk are arranged
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    /// One copy, one device
    Single,
    /// Two copies on one device. `mkfs.btrfs` uses this for metadata and system
    /// chunks on a single disk
    Dup,
    /// Two, three or four copies on separate devices. A reader takes any one
    Raid1(u16),
}

impl Profile {
    /// How many copies of every byte there are
    pub fn copies(self) -> u16 {
        match self {
            Profile::Single => 1,
            Profile::Dup => 2,
            Profile::Raid1(n) => n,
        }
    }
}

/// A chunk, as this driver needs it
#[derive(Clone, Debug)]
pub struct Chunk {
    /// First logical address this chunk covers
    pub start: u64,
    pub length: u64,
    /// The whole type word, profile bits included
    pub ty: u64,
    pub profile: Profile,
    pub stripes: Vec<Stripe>,
}

impl Chunk {
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.length)
    }

    pub fn contains(&self, logical: u64) -> bool {
        logical >= self.start && logical < self.end()
    }

    pub fn is_system(&self) -> bool {
        self.ty & BLOCK_GROUP_SYSTEM != 0
    }
    pub fn is_metadata(&self) -> bool {
        self.ty & BLOCK_GROUP_METADATA != 0
    }
    pub fn is_data(&self) -> bool {
        self.ty & BLOCK_GROUP_DATA != 0
    }
}

/// Which profile a type word describes, or why it cannot be used.
///
/// Refused:
///
///   RAID0, RAID10  stripes are pieces, not copies, with a `stripe_index` from a modulo
///                  the collapse above does not do. Mirror arithmetic would read the wrong
///                  64 KiB, valid data of another address
///   RAID5, RAID6   parity, needing unimplemented reconstruction. Also refused at mount
///                  by the feature bit
fn profile_of(ty: u64) -> Result<Profile, BtrfsError> {
    let bits = ty & BLOCK_GROUP_PROFILE_MASK;
    match bits {
        0 => Ok(Profile::Single),
        BLOCK_GROUP_DUP => Ok(Profile::Dup),
        BLOCK_GROUP_RAID1 => Ok(Profile::Raid1(2)),
        BLOCK_GROUP_RAID1C3 => Ok(Profile::Raid1(3)),
        BLOCK_GROUP_RAID1C4 => Ok(Profile::Raid1(4)),
        _ => Err(BtrfsError::Unsupported),
    }
}

/// Stripes a profile must have, from `valid_stripe_count` and
/// `btrfs_raid_array` in the kernel. Exact
fn required_stripes(p: Profile) -> u16 {
    match p {
        Profile::Single => 1,
        Profile::Dup => 2,
        Profile::Raid1(n) => n,
    }
}

/// Reads and checks one chunk item. `logical` is the key's offset, the chunk's
/// first address, passed with the item.
///
/// Checks are `btrfs_check_chunk_valid` in `fs/btrfs/tree-checker.c`.
/// `num_stripes` is bounds-checked before it sizes anything
pub fn parse_chunk(
    buf: &[u8],
    at: usize,
    logical: u64,
    sectorsize: u32,
) -> Result<Chunk, BtrfsError> {
    let length = u64_at(buf, at + chunk::LENGTH)?;
    let stripe_len = u64_at(buf, at + chunk::STRIPE_LEN)?;
    let ty = u64_at(buf, at + chunk::TYPE)?;
    let chunk_sectorsize = u32_at(buf, at + chunk::SECTOR_SIZE)?;
    let num_stripes = u16_at(buf, at + chunk::NUM_STRIPES)?;
    let sub_stripes = u16_at(buf, at + chunk::SUB_STRIPES)?;

    if num_stripes == 0 {
        return Err(BtrfsError::Corrupt);
    }
    // The address the chunk starts at has to be readable at all
    if logical % sectorsize as u64 != 0 {
        return Err(BtrfsError::Corrupt);
    }
    if chunk_sectorsize != sectorsize {
        return Err(BtrfsError::Corrupt);
    }
    if length == 0 || length % sectorsize as u64 != 0 {
        return Err(BtrfsError::Corrupt);
    }
    // A chunk whose end wraps would `contains()` everything
    if logical.checked_add(length).is_none() {
        return Err(BtrfsError::Corrupt);
    }
    if length >= MAX_CHUNK_LEN {
        return Err(BtrfsError::Corrupt);
    }
    if stripe_len != STRIPE_LEN {
        return Err(BtrfsError::Corrupt);
    }

    // Exactly one type bit, and zero or one profile bit
    let type_bits = ty & (BLOCK_GROUP_DATA | BLOCK_GROUP_SYSTEM | BLOCK_GROUP_METADATA);
    if type_bits == 0 {
        return Err(BtrfsError::Corrupt);
    }
    let profile_bits = ty & BLOCK_GROUP_PROFILE_MASK;
    if profile_bits.count_ones() > 1 {
        return Err(BtrfsError::Corrupt);
    }
    // A system chunk also holding data or metadata is not a btrfs shape, and
    // the bootstrap array relies on the system bit
    if ty & BLOCK_GROUP_SYSTEM != 0 && ty & (BLOCK_GROUP_METADATA | BLOCK_GROUP_DATA) != 0 {
        return Err(BtrfsError::Corrupt);
    }
    // Without MIXED_GROUPS, refused at mount, data and metadata do not share a
    // chunk
    if ty & BLOCK_GROUP_METADATA != 0 && ty & BLOCK_GROUP_DATA != 0 {
        return Err(BtrfsError::Corrupt);
    }

    let profile = profile_of(ty)?;
    if num_stripes != required_stripes(profile) {
        return Err(BtrfsError::Corrupt);
    }
    // `sub_stripes` applies only to RAID10, refused above
    if sub_stripes != 1 {
        return Err(BtrfsError::Corrupt);
    }

    let mut stripes = Vec::new();
    stripes
        .try_reserve_exact(num_stripes as usize)
        .map_err(|_| BtrfsError::OutOfMemory)?;
    for i in 0..num_stripes as usize {
        let s = at + chunk::STRIPES + i * STRIPE_SIZE;
        stripes.push(Stripe {
            devid: u64_at(buf, s + stripe::DEVID)?,
            offset: u64_at(buf, s + stripe::OFFSET)?,
        });
    }

    Ok(Chunk { start: logical, length, ty, profile, stripes })
}

/// Where a logical address is on a device
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Physical {
    pub devid: u64,
    pub offset: u64,
    /// Bytes contiguous on the device from here. A longer read must split,
    /// keeping a caller inside the chunk
    pub len: u64,
}

/// Every known chunk in logical order, a sorted `Vec` with binary search:
/// hundreds at most, built once at mount
#[derive(Default)]
pub struct ChunkMap {
    chunks: Vec<Chunk>,
}

impl ChunkMap {
    pub const fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
    pub fn iter(&self) -> core::slice::Iter<'_, Chunk> {
        self.chunks.iter()
    }

    /// Adds a chunk, keeping order and refusing an overlap, since the lookup
    /// would then depend on insertion order. A chunk at the same address is
    /// accepted as a repeat: the bootstrap array and chunk tree describe the
    /// same system chunks
    pub fn insert(&mut self, chunk: Chunk) -> Result<(), BtrfsError> {
        let pos = self.chunks.partition_point(|c| c.start < chunk.start);

        if let Some(existing) = self.chunks.get(pos) {
            if existing.start == chunk.start {
                if existing.length != chunk.length {
                    return Err(BtrfsError::Corrupt);
                }
                return Ok(());
            }
            if chunk.end() > existing.start {
                return Err(BtrfsError::Corrupt);
            }
        }
        if pos > 0 {
            let before = &self.chunks[pos - 1];
            if before.end() > chunk.start {
                return Err(BtrfsError::Corrupt);
            }
        }

        self.chunks
            .try_reserve(1)
            .map_err(|_| BtrfsError::OutOfMemory)?;
        self.chunks.insert(pos, chunk);
        Ok(())
    }

    pub fn find(&self, logical: u64) -> Option<&Chunk> {
        let pos = self.chunks.partition_point(|c| c.start <= logical);
        let c = self.chunks.get(pos.checked_sub(1)?)?;
        c.contains(logical).then_some(c)
    }

    /// Translates a logical address to a device location. `mirror` picks the
    /// copy, from zero. SINGLE has copy 0; DUP has both on one device at
    /// different offsets. The returned `len` is contiguous from there, at most
    /// the chunk's remainder
    pub fn map(&self, logical: u64, mirror: u16) -> Result<Physical, BtrfsError> {
        let chunk = self.find(logical).ok_or(BtrfsError::NotFound)?;
        self.map_in(chunk, logical, mirror)
    }

    pub fn map_in(&self, chunk: &Chunk, logical: u64, mirror: u16) -> Result<Physical, BtrfsError> {
        if !chunk.contains(logical) {
            return Err(BtrfsError::NotFound);
        }
        let stripe = chunk
            .stripes
            .get(mirror as usize)
            .ok_or(BtrfsError::InvalidArg)?;

        let offset = logical - chunk.start;
        // Every profile here gives each stripe the whole chunk, so the note at
        // the top reduces to one addition. `profile_of` refuses striping
        // profiles
        let physical = stripe
            .offset
            .checked_add(offset)
            .ok_or(BtrfsError::Corrupt)?;

        Ok(Physical {
            devid: stripe.devid,
            offset: physical,
            len: chunk.end() - logical,
        })
    }
}

/// Builds the first chunk map from the superblock's bootstrap array, since the
/// chunk tree's own blocks need mapping
pub fn map_from_sys_array(
    sb: &crate::mount::Superblock,
    sectorsize: u32,
) -> Result<ChunkMap, BtrfsError> {
    let arr = sb.sys_chunk_array();
    let declared = arr.len();
    let mut map = ChunkMap::new();

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

        let c = parse_chunk(arr, cur, key.offset, sectorsize)?;
        // The array maps the chunk tree, which lives in system chunks
        if !c.is_system() {
            return Err(BtrfsError::Corrupt);
        }
        map.insert(c)?;
        cur += item;
    }

    if map.is_empty() {
        return Err(BtrfsError::Corrupt);
    }
    Ok(map)
}
