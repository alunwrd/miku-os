// What is free, and where
//
// Two places answer: the block group item says how much of a chunk is used, and
// the free space tree says which parts. `GroupScan::finish` holds them together
// with two checks catching different mistakes:
//
//   run count    `FREE_SPACE_INFO` counted the runs when written. Catches a bitmap decoded with each
//                byte's bits reversed: the same sectors come out free, but run boundaries move
//   length-used  the sum of free runs against what the block group item says is unused. Catches a
//                decode with the wrong amount: wrong granularity, a shifted window, an item read past its end
//
// Two representations:
//
//   FREE_SPACE_EXTENT   one item per run. The item is empty: `objectid` is the start, `offset` the length
//   FREE_SPACE_BITMAP   one bit per sector over the key's range, least significant bit first per byte.
//                       A group converts when it has more runs than items are worth
//
// Block groups live in their own tree since 6.x (`BLOCK_GROUP_TREE`, compat_ro
// bit 3, a `mkfs.btrfs` default), before that in the extent tree picked out by
// key type. `SpaceInfo::from_block_group_tree` records which.
//
// Checks are `check_block_group_item`, `check_free_space_info`,
// `check_free_space_extent`, and `check_free_space_bitmap` from
// `tree-checker.c`; counting is `load_free_space_extents` and
// `load_free_space_bitmaps` from `free-space-tree.c`. Offsets from the compiler
// over `btrfs_tree.h`: `btrfs_block_group_item` is 24 bytes packed,
// `btrfs_free_space_info` 8.

use crate::error::BtrfsError;
use crate::structs::*;
use crate::volume::Volume;
use alloc::vec::Vec;
use fshost::BlockHost;

// key types

pub const BLOCK_GROUP_ITEM_KEY: u8 = 192;
pub const FREE_SPACE_INFO_KEY: u8 = 198;
pub const FREE_SPACE_EXTENT_KEY: u8 = 199;
pub const FREE_SPACE_BITMAP_KEY: u8 = 200;

// Tree ids

pub const EXTENT_TREE_OBJECTID: u64 = 2;
pub const FREE_SPACE_TREE_OBJECTID: u64 = 10;
pub const BLOCK_GROUP_TREE_OBJECTID: u64 = 11;
/// Every block group item's `chunk_objectid`, constant since the format began
pub const FIRST_CHUNK_TREE_OBJECTID: u64 = 256;

// Items

/// Field offsets in `struct btrfs_block_group_item`
pub mod bgi {
    pub const USED: usize = 0;
    pub const CHUNK_OBJECTID: usize = 8;
    pub const FLAGS: usize = 16;
    /// The whole struct, the exact item size
    pub const SIZE_OF: usize = 24;
    /// `btrfs_block_group_item_v2`, written by a `REMAP_TREE` volume, refused
    /// at mount; lets this layer report an unimplemented format
    pub const V2_SIZE_OF: usize = 36;
}

/// Field offsets in `struct btrfs_free_space_info`
pub mod fsi {
    pub const EXTENT_COUNT: usize = 0;
    pub const FLAGS: usize = 4;
    pub const SIZE_OF: usize = 8;
}

/// The group's free space is written as bitmaps
pub const FREE_SPACE_USING_BITMAPS: u32 = 1 << 0;
pub const FREE_SPACE_FLAGS_MASK: u32 = FREE_SPACE_USING_BITMAPS;

/// Most bits in one bitmap item: 256 bytes, 2048 sectors. The key states the
/// range and the item size is checked against it
pub const FREE_SPACE_BITMAP_SIZE: usize = 256;

// The model

/// One block group: a chunk's logical space, how much is used, and the free
/// runs. `runs` is empty until the free space tree is walked; a group with no
/// free space info is an error
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeSpace {
    pub start: u64,
    pub length: u64,
    /// Bytes allocated out of this group, as the block group item counts them
    pub used: u64,
    /// The whole flags word: type and profile bits, the same as the chunk's
    pub flags: u64,
    /// The free runs, as `(start, length)`, ascending and non-overlapping
    pub runs: Vec<(u64, u64)>,
}

impl FreeSpace {
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.length)
    }

    pub fn is_data(&self) -> bool {
        self.flags & BLOCK_GROUP_DATA != 0
    }
    pub fn is_metadata(&self) -> bool {
        self.flags & BLOCK_GROUP_METADATA != 0
    }
    pub fn is_system(&self) -> bool {
        self.flags & BLOCK_GROUP_SYSTEM != 0
    }

    /// How much is free by the runs. `GroupScan::finish` required this to match
    /// the arithmetic
    pub fn free(&self) -> u64 {
        self.runs.iter().map(|&(_, len)| len).sum()
    }
}

/// Every block group on the volume, with what is free in each
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceInfo {
    pub sectorsize: u32,
    /// Where block group items were found: `BLOCK_GROUP_TREE` by default, the
    /// extent tree before 6.x or with the feature off
    pub from_block_group_tree: bool,
    /// In logical order, as the block group tree keeps them
    pub groups: Vec<FreeSpace>,
}

impl SpaceInfo {
    /// Free space in groups matching `kind_mask`, a mask of `BLOCK_GROUP_DATA`,
    /// `_METADATA`, and `_SYSTEM`
    pub fn total_free(&self, kind_mask: u64) -> u64 {
        self.groups
            .iter()
            .filter(|g| g.flags & kind_mask != 0)
            .map(|g| g.free())
            .sum()
    }

    /// First `len` free bytes in a group of the requested kind, aligned to
    /// `align`, as a logical address. First fit over groups in logical order
    /// and runs in address order, a function of the state alone. The address
    /// must still be mapped; on DUP or RAID1 one logical byte costs more than
    /// one device byte
    pub fn find_free(&self, kind_mask: u64, len: u64, align: u64) -> Option<u64> {
        // Zero length or a non-power-of-two alignment is a caller's error
        if len == 0 || align == 0 || !align.is_power_of_two() {
            return None;
        }

        for g in self.groups.iter().filter(|g| g.flags & kind_mask != 0) {
            for &(start, run_len) in &g.runs {
                let at = start.checked_add(align - 1)? & !(align - 1);
                let end = start.checked_add(run_len)?;
                if at >= end {
                    continue;
                }
                if end - at >= len {
                    return Some(at);
                }
            }
        }
        None
    }
}

// the bg item

/// Reads and checks one `BLOCK_GROUP_ITEM`. The key has its placement:
/// `objectid` the start, `offset` the length. `check_block_group_item` in
/// `tree-checker.c`, minus parts needing a mounted state, and refusing mixed
/// data and metadata, a `MIXED_GROUPS` volume refused at mount
pub fn parse_block_group_item(
    key: &Key,
    item: &[u8],
    sectorsize: u32,
) -> Result<FreeSpace, BtrfsError> {
    if item.len() == bgi::V2_SIZE_OF {
        return Err(BtrfsError::Unsupported);
    }
    if item.len() != bgi::SIZE_OF {
        return Err(BtrfsError::Corrupt);
    }

    let start = key.objectid;
    let length = key.offset;
    if length == 0 {
        return Err(BtrfsError::Corrupt);
    }
    // A group has to start and end somewhere a sector can be read
    if start % sectorsize as u64 != 0 || length % sectorsize as u64 != 0 {
        return Err(BtrfsError::Corrupt);
    }
    // A group whose end wraps would contain every address there is
    if start.checked_add(length).is_none() {
        return Err(BtrfsError::Corrupt);
    }

    let used = u64_at(item, bgi::USED)?;
    let chunk_objectid = u64_at(item, bgi::CHUNK_OBJECTID)?;
    let flags = u64_at(item, bgi::FLAGS)?;

    if chunk_objectid != FIRST_CHUNK_TREE_OBJECTID {
        return Err(BtrfsError::Corrupt);
    }
    // More used than there is. Everything below subtracts one from the other
    if used > length {
        return Err(BtrfsError::Corrupt);
    }

    // Zero or one profile bit. Two would describe a chunk laid out two ways
    if (flags & BLOCK_GROUP_PROFILE_MASK).count_ones() > 1 {
        return Err(BtrfsError::Corrupt);
    }

    let ty = flags & (BLOCK_GROUP_DATA | BLOCK_GROUP_METADATA | BLOCK_GROUP_SYSTEM);
    if ty == (BLOCK_GROUP_DATA | BLOCK_GROUP_METADATA) {
        // Mixed groups, which the mount check refuses by its incompat bit
        return Err(BtrfsError::Unsupported);
    }
    if ty != BLOCK_GROUP_DATA && ty != BLOCK_GROUP_METADATA && ty != BLOCK_GROUP_SYSTEM {
        return Err(BtrfsError::Corrupt);
    }

    Ok(FreeSpace { start, length, used, flags, runs: Vec::new() })
}

// the free space

/// One group's free space, assembled per item. A type since bitmap runs span
/// items, and a test can feed it a hostile item
pub struct GroupScan {
    group: FreeSpace,
    sectorsize: u64,
    /// What the info item said to expect, checked at the end
    expected: u32,
    counted: u32,
    bitmaps: bool,
    /// A run of set bits not yet ended, and its start
    pending: Option<u64>,
    /// End of the last bitmap item, where a pending run stops if the next does
    /// not continue it
    covered_to: u64,
}

impl GroupScan {
    /// Begins a group from `FREE_SPACE_INFO`: `check_free_space_info`, plus the
    /// kernel's `ASSERT` on the key. The item is keyed by its group; another
    /// range means the walk and list have separated
    pub fn new(
        group: FreeSpace,
        key: &Key,
        info: &[u8],
        sectorsize: u32,
    ) -> Result<Self, BtrfsError> {
        if key.kind != FREE_SPACE_INFO_KEY {
            return Err(BtrfsError::Corrupt);
        }
        if key.objectid != group.start || key.offset != group.length {
            return Err(BtrfsError::Corrupt);
        }
        if info.len() != fsi::SIZE_OF {
            return Err(BtrfsError::Corrupt);
        }

        let expected = u32_at(info, fsi::EXTENT_COUNT)?;
        let flags = u32_at(info, fsi::FLAGS)?;
        if flags & !FREE_SPACE_FLAGS_MASK != 0 {
            return Err(BtrfsError::Corrupt);
        }
        // No more runs than sectors: a run is at least one sector and two
        // cannot touch
        if expected as u64 > group.length / sectorsize as u64 {
            return Err(BtrfsError::Corrupt);
        }

        Ok(GroupScan {
            covered_to: group.start,
            group,
            sectorsize: sectorsize as u64,
            expected,
            counted: 0,
            bitmaps: flags & FREE_SPACE_USING_BITMAPS != 0,
            pending: None,
        })
    }

    /// Take one `FREE_SPACE_EXTENT` or `FREE_SPACE_BITMAP` item
    pub fn item(&mut self, key: &Key, item: &[u8]) -> Result<(), BtrfsError> {
        // The flags name this group's representation, and another kind's item
        // disagrees; the kernel refuses the leaf
        let expected_kind = if self.bitmaps {
            FREE_SPACE_BITMAP_KEY
        } else {
            FREE_SPACE_EXTENT_KEY
        };
        if key.kind != expected_kind {
            return Err(BtrfsError::Corrupt);
        }

        let start = key.objectid;
        let len = key.offset;
        if len == 0 || len % self.sectorsize != 0 || start % self.sectorsize != 0 {
            return Err(BtrfsError::Corrupt);
        }
        let end = start.checked_add(len).ok_or(BtrfsError::Corrupt)?;
        // `validate_free_space_key`: an item's range stays inside the group
        if start < self.group.start || end > self.group.end() {
            return Err(BtrfsError::Corrupt);
        }

        if !self.bitmaps {
            // The run is in the key; the item carries no bytes
            if !item.is_empty() {
                return Err(BtrfsError::Corrupt);
            }
            return self.push_run(start, len);
        }

        // The item holds exactly one bit per sector of the key's range
        let bits = len / self.sectorsize;
        let want = bits.checked_add(7).ok_or(BtrfsError::Corrupt)? / 8;
        if item.len() as u64 != want {
            return Err(BtrfsError::Corrupt);
        }

        // A run spans items only where they continue each other; bytes between
        // are undescribed
        if self.pending.is_some() && start != self.covered_to {
            self.close_pending()?;
        }

        for i in 0..bits {
            let byte = u8_at(item, (i / 8) as usize)?;
            let set = byte >> (i % 8) & 1 != 0;
            let at = start + i * self.sectorsize;
            match (self.pending, set) {
                (None, true) => self.pending = Some(at),
                (Some(from), false) => {
                    self.pending = None;
                    self.push_run(from, at - from)?;
                }
                _ => {}
            }
        }
        self.covered_to = end;
        Ok(())
    }

    /// Ends the open run at the last byte a bitmap covered. The kernel ends it
    /// at the group's end; these match when bitmaps cover the group, and this
    /// invents no free space from a gap
    fn close_pending(&mut self) -> Result<(), BtrfsError> {
        if let Some(from) = self.pending.take() {
            let to = self.covered_to;
            if to <= from {
                return Err(BtrfsError::Corrupt);
            }
            self.push_run(from, to - from)?;
        }
        Ok(())
    }

    fn push_run(&mut self, start: u64, len: u64) -> Result<(), BtrfsError> {
        if len == 0 || len % self.sectorsize != 0 || start % self.sectorsize != 0 {
            return Err(BtrfsError::Corrupt);
        }
        let end = start.checked_add(len).ok_or(BtrfsError::Corrupt)?;
        if start < self.group.start || end > self.group.end() {
            return Err(BtrfsError::Corrupt);
        }
        // Ascending and disjoint, or `total_free` counts a byte twice and
        // `find_free` gives it out twice
        if let Some(&(last_start, last_len)) = self.group.runs.last() {
            if start < last_start + last_len {
                return Err(BtrfsError::Corrupt);
            }
        }

        self.counted = self.counted.checked_add(1).ok_or(BtrfsError::Corrupt)?;
        self.group
            .runs
            .try_reserve(1)
            .map_err(|_| BtrfsError::OutOfMemory)?;
        self.group.runs.push((start, len));
        Ok(())
    }

    /// Close the group and check that it describes itself consistently
    pub fn finish(mut self) -> Result<FreeSpace, BtrfsError> {
        self.close_pending()?;

        // The info item counted the runs. A mismatch means items lost or
        // gained, or bitmaps decoded into the wrong number of runs, as with
        // reversed bits, which leave the sum unchanged
        if self.counted != self.expected {
            return Err(BtrfsError::Corrupt);
        }

        // The two halves agree: what is not used is free. Catches a wrong
        // amount: wrong granularity, or a shifted window. A reversed byte has
        // the same popcount, so it passes this and the count above catches it
        let mut sum: u64 = 0;
        for &(_, len) in &self.group.runs {
            sum = sum.checked_add(len).ok_or(BtrfsError::Corrupt)?;
        }
        if sum != self.group.length - self.group.used {
            return Err(BtrfsError::Corrupt);
        }

        Ok(self.group)
    }
}

// Reading

/// Reads every block group and what is free in it. Block groups first, as free
/// space items mean something only against their group, then the free space
/// tree. Both are keyed by group start, so the second walk consumes the list in
/// order, and a mismatched info item shows the trees disagree
pub fn read_free_space<H: BlockHost>(vol: &mut Volume<H>) -> Result<SpaceInfo, BtrfsError> {
    let sectorsize = vol.geometry.sectorsize;
    let compat_ro = vol.geometry.compat_ro;

    // A `mkfs.btrfs` volume since 6.x keeps block groups in their own tree
    // (compat_ro bit 3); older ones in the extent tree, told apart by type byte
    let from_block_group_tree = compat_ro & FEATURE_COMPAT_RO_BLOCK_GROUP_TREE != 0;
    let bg_tree = if from_block_group_tree {
        vol.find_root(BLOCK_GROUP_TREE_OBJECTID)?.bytenr
    } else {
        vol.find_root(EXTENT_TREE_OBJECTID)?.bytenr
    };

    let mut groups: Vec<FreeSpace> = Vec::new();
    vol.for_each_item(bg_tree, &mut |k, data| {
        if k.kind != BLOCK_GROUP_ITEM_KEY {
            // Only in the extent tree, where the rest concerns extents
            return Ok(true);
        }
        let g = parse_block_group_item(&k, data, sectorsize)?;
        groups.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
        groups.push(g);
        Ok(true)
    })?;
    if groups.is_empty() {
        return Err(BtrfsError::Corrupt);
    }

    // Each group is a chunk, which says where its addresses are. A group
    // without one, or covering another range, is space nobody could read or
    // write
    for g in &groups {
        let chunk = vol.chunks.find(g.start).ok_or(BtrfsError::Corrupt)?;
        if chunk.start != g.start || chunk.length != g.length {
            return Err(BtrfsError::Corrupt);
        }
        if chunk.ty != g.flags {
            return Err(BtrfsError::Corrupt);
        }
    }

    // Without the free space tree there is no answer. The v1 space cache is a
    // different structure, in the root tree
    if compat_ro & FEATURE_COMPAT_RO_FREE_SPACE_TREE == 0 {
        return Err(BtrfsError::Unsupported);
    }
    // Last written by something not maintaining it, so the kernel rebuilds it
    // at the next mount
    if compat_ro & FEATURE_COMPAT_RO_FREE_SPACE_TREE_VALID == 0 {
        return Err(BtrfsError::Unsupported);
    }

    let fst = vol.find_root(FREE_SPACE_TREE_OBJECTID)?.bytenr;

    let mut done: Vec<FreeSpace> = Vec::new();
    done.try_reserve_exact(groups.len())
        .map_err(|_| BtrfsError::OutOfMemory)?;
    let mut scan: Option<GroupScan> = None;
    let mut next = 0usize;

    vol.for_each_item(fst, &mut |k, data| {
        match k.kind {
            FREE_SPACE_INFO_KEY => {
                if let Some(s) = scan.take() {
                    done.push(s.finish()?);
                }
                let g = groups.get(next).ok_or(BtrfsError::Corrupt)?;
                next += 1;
                scan = Some(GroupScan::new(g.clone(), &k, data, sectorsize)?);
            }
            FREE_SPACE_EXTENT_KEY | FREE_SPACE_BITMAP_KEY => {
                scan.as_mut().ok_or(BtrfsError::Corrupt)?.item(&k, data)?;
            }
            // The free space tree holds these three key types only
            _ => return Err(BtrfsError::Corrupt),
        }
        Ok(true)
    })?;
    if let Some(s) = scan.take() {
        done.push(s.finish()?);
    }

    // A group with no free space info cannot allocate; the kernel says so too
    if done.len() != groups.len() {
        return Err(BtrfsError::Corrupt);
    }

    Ok(SpaceInfo { sectorsize, from_block_group_tree, groups: done })
}

// Handing space out and keeping the books
//
// Three places record one fact:
//
//   the block allocator   addresses this transaction may use
//   the free space tree   addresses the next mount may use
//   the block group item  bytes in use per chunk
//
// One transaction writes them, and `btrfs check` cross-checks them against the
// extent tree.
//
// The pinning rule
//
// `BlockAlloc::free` is called for a block copy-on-write stopped referencing.
// It is not free: the old tree, what a crash falls back to, points at it. No
// path below returns a range to `view`, so a freed address cannot come out of
// `alloc`. `Trans::check_fresh` also refuses one, for another allocator.
//
// `btrfs_free_tree_block` splits on `HEADER_FLAG_WRITTEN`, returning a
// just-allocated unwritten block at once. This allocator does not, as
// `BlockAlloc::free` lacks the flag; a few nodesizes (`Trans::discard_block`'s)
// stay unusable until commit. The books balance: allocate then free removes and
// restores the range, and `used` moves equally.
//
// Absent
//
// No chunk is allocated. With no room in a metadata block group this returns
// `Unsupported`.
//
// Neither representation is converted. The kernel converts when free runs cost
// more as items than bits (`btrfs_set_free_space_tree_thresholds`, and the two
// branches ending `update_free_space_extent_count`). Both are modified here,
// and a change crossing the threshold is `Unsupported`, computed from the same
// two thresholds.
//
// Only metadata is allocated: `alloc` gives one nodesize at a time from
// `BLOCK_GROUP_METADATA` groups.
//
// The superblock's `bytes_used` is not written here. `bytes_used()` gives the
// number, and the caller puts it in the superblock beside the root tree's
// address; `commit_superblocks` writes neither. `btrfs_update_block_group`
// moves the item and counter together; a caller forgetting the second gets
// `found N bytes used` from `btrfs check`

use crate::chunk::{Chunk, ChunkMap, STRIPE_LEN};
use crate::trans::{BlockAlloc, ReadBlock, Trans};

/// Sectors one `FREE_SPACE_BITMAP` item's bits describe:
/// `BTRFS_FREE_SPACE_BITMAP_BITS`, one of the two inputs to the conversion
/// thresholds
pub const FREE_SPACE_BITMAP_BITS: u64 = FREE_SPACE_BITMAP_SIZE as u64 * 8;

/// One range this transaction took from or returned to free space. `alloc`:
/// `true` means it left the free space tree and `used` rose. Recorded order is
/// apply order
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Change {
    pub start: u64,
    pub len: u64,
    pub alloc: bool,
}

/// Where a superblock mirror lands in a chunk, as a logical range:
/// `exclude_super_stripes` in `block-group.c`.
///
///   * the first 64 KiB of the device is nobody's, so a group starting below
///     `BTRFS_SUPER_INFO_OFFSET`
///     loses that part;
///   * each of the three mirrors (`BTRFS_SUPER_MIRROR_MAX` is 3) maps from its
///     device offset into every
///     stripe of the chunk covering it.
///
/// The reverse of `ChunkMap::map_in`, one subtraction for accepted profiles:
/// `logical = chunk.start + (physical - stripe.offset)`.
///
/// These ranges stay free in the free space tree: the kernel keeps them in
/// `excluded_extents` and counts them in `bytes_super`. So this list steers
/// placement only
pub fn super_stripes(chunk: &Chunk, group_end: u64, mirrors: usize) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();

    if chunk.start < SUPER_INFO_OFFSET {
        out.push((chunk.start, SUPER_INFO_OFFSET - chunk.start));
    }

    for &at in SUPER_MIRRORS.iter().take(mirrors) {
        for stripe in &chunk.stripes {
            let end = match stripe.offset.checked_add(chunk.length) {
                Some(e) => e,
                None => continue,
            };
            if at < stripe.offset || at >= end {
                continue;
            }
            let logical = chunk.start + (at - stripe.offset);
            if logical >= group_end {
                continue;
            }
            // `min(stripe_len, block_group_end - logical)`, the reference's
            // clamp: a mirror at a chunk's end is excluded only as far as the
            // chunk goes
            let len = STRIPE_LEN.min(group_end - logical);
            out.push((logical, len));
        }
    }
    out
}

/// Takes `[start, start + len)` out of an ascending, disjoint run list. Returns
/// the bytes present. A partial hit is an error for an allocation, not for a
/// superblock stripe
fn subtract(runs: &mut Vec<(u64, u64)>, start: u64, len: u64) -> Result<u64, BtrfsError> {
    let end = start.checked_add(len).ok_or(BtrfsError::InvalidArg)?;
    let mut out: Vec<(u64, u64)> = Vec::new();
    out.try_reserve(runs.len() + 1)
        .map_err(|_| BtrfsError::OutOfMemory)?;
    let mut removed = 0u64;

    for &(rs, rl) in runs.iter() {
        let re = rs.checked_add(rl).ok_or(BtrfsError::Corrupt)?;
        if re <= start || rs >= end {
            out.push((rs, rl));
            continue;
        }
        removed += re.min(end) - rs.max(start);
        if rs < start {
            out.push((rs, start - rs));
        }
        if re > end {
            out.push((end, re - end));
        }
    }

    *runs = out;
    Ok(removed)
}

/// The block allocator: addresses from the volume's metadata block groups, and
/// the ledger for the free space tree and block group items.
///
///   `view.groups[i].runs`   placement: free, minus superblock mirrors and everything given out,
///                           never plus anything pinned. What `alloc` searches
///   `view.groups[i].used`   accounting: the block group item's running value, moved by each
///                           `alloc` and `free`
///
/// The free space tree is changed item by item from `log`
pub struct Allocator {
    view: SpaceInfo,
    /// What each block group item on the medium says, so an unmoved `used` is
    /// not rewritten
    written_used: Vec<u64>,
    nodesize: u32,
    /// Superblock mirror ranges carved from `view`, kept so a test can ask
    excluded: Vec<(u64, u64)>,
    /// Everything that happened, in order
    log: Vec<Change>,
    /// How much of `log` the free space tree has already been told about
    applied: usize,
    /// `BlockAlloc::free` cannot return an error, so one is kept for the next
    /// call that can
    broken: Option<BtrfsError>,
}

impl Allocator {
    /// Builds one over a volume's space, excluding superblock mirrors.
    /// `mirrors` is how many fit: `commit::mirror_count`
    pub fn new(
        space: SpaceInfo,
        chunks: &ChunkMap,
        nodesize: u32,
        mirrors: usize,
    ) -> Result<Self, BtrfsError> {
        let mut view = space;
        let mut written_used: Vec<u64> = Vec::new();
        written_used
            .try_reserve_exact(view.groups.len())
            .map_err(|_| BtrfsError::OutOfMemory)?;
        let mut excluded: Vec<(u64, u64)> = Vec::new();

        for g in view.groups.iter_mut() {
            written_used.push(g.used);
            let chunk = chunks.find(g.start).ok_or(BtrfsError::Corrupt)?;
            for (at, len) in super_stripes(chunk, g.end(), mirrors) {
                subtract(&mut g.runs, at, len)?;
                excluded.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
                excluded.push((at, len));
            }
        }

        Ok(Allocator {
            view,
            written_used,
            nodesize,
            excluded,
            log: Vec::new(),
            applied: 0,
            broken: None,
        })
    }

    /// The volume's space for this transaction. `runs` is the placement view,
    /// not the free space tree
    pub fn view(&self) -> &SpaceInfo {
        &self.view
    }

    /// Superblock mirror ranges carved from the placement view, as
    /// `(start, length)` logical
    pub fn excluded(&self) -> &[(u64, u64)] {
        &self.excluded
    }

    /// Everything handed out and everything pinned, in the order it happened
    pub fn changes(&self) -> &[Change] {
        &self.log
    }

    /// What the superblock's `bytes_used` must become: the sum of each group's
    /// `used`. `btrfs_update_block_group` moves both; here the caller does
    pub fn bytes_used(&self) -> u64 {
        self.view.groups.iter().map(|g| g.used).sum()
    }

    /// Which group holds this address
    fn group_of(&self, addr: u64) -> Option<usize> {
        self.view
            .groups
            .iter()
            .position(|g| addr >= g.start && addr < g.end())
    }

    /// Changes not yet in the free space tree, taken out so the caller applies
    /// them while the allocator records more. This makes the fixpoint in
    /// `settle` terminate
    fn take_pending(&mut self) -> Result<Vec<Change>, BtrfsError> {
        if let Some(e) = self.broken {
            return Err(e);
        }
        let mut out: Vec<Change> = Vec::new();
        out.try_reserve_exact(self.log.len() - self.applied)
            .map_err(|_| BtrfsError::OutOfMemory)?;
        out.extend_from_slice(&self.log[self.applied..]);
        self.applied = self.log.len();
        Ok(out)
    }

    fn record(&mut self, c: Change) {
        if self.log.try_reserve(1).is_err() {
            self.broken = Some(BtrfsError::OutOfMemory);
            return;
        }
        self.log.push(c);
    }
}

impl BlockAlloc for Allocator {
    fn alloc(&mut self, size: u64) -> Result<u64, BtrfsError> {
        if let Some(e) = self.broken {
            return Err(e);
        }
        // One tree block, one nodesize. A data extent is another request
        if size != self.nodesize as u64 {
            return Err(BtrfsError::InvalidArg);
        }

        // Nodesize-aligned, stricter than the format and what each btrfs
        // allocator does
        let at = self
            .view
            .find_free(BLOCK_GROUP_METADATA, size, self.nodesize as u64)
            // No chunk is allocated, so a metadata group with no room ends here
            .ok_or(BtrfsError::Unsupported)?;

        let g = self.group_of(at).ok_or(BtrfsError::Corrupt)?;
        // `find_free` said this range was whole and free; less means the list
        // and search disagree
        if subtract(&mut self.view.groups[g].runs, at, size)? != size {
            return Err(BtrfsError::Corrupt);
        }
        self.view.groups[g].used = self.view.groups[g]
            .used
            .checked_add(size)
            .ok_or(BtrfsError::Corrupt)?;

        self.record(Change { start: at, len: size, alloc: true });
        Ok(at)
    }

    fn free(&mut self, logical: u64, size: u64) {
        let g = match self.group_of(logical) {
            Some(g) => g,
            None => {
                // An address in no block group was not given out here and
                // cannot be accounted for
                self.broken = Some(BtrfsError::Corrupt);
                return;
            }
        };
        match self.view.groups[g].used.checked_sub(size) {
            Some(u) => self.view.groups[g].used = u,
            None => {
                self.broken = Some(BtrfsError::Corrupt);
                return;
            }
        }
        // Not back into `view.groups[g].runs`: rule 1, as the old tree points
        // here until the commit is on the medium
        self.record(Change { start: logical, len: size, alloc: false });
    }
}

/// An allocator over an open volume, with the mirror rule from the commit's
/// function. Here since it needs the free space, chunk map, and mirror count
pub fn allocator_for<H: BlockHost>(vol: &mut Volume<H>) -> Result<Allocator, BtrfsError> {
    let space = read_free_space(vol)?;
    let nodesize = vol.geometry.nodesize;

    // The pair `commit_superblocks` measures a mirror against: what the
    // filesystem recorded and what the device has, the lesser
    let recorded = u64_at(&vol.superblock.data, sb::DEV_ITEM + 8)?;
    let host_bytes = vol.host_mut().total_sectors().saturating_mul(512);
    let device_bytes = if host_bytes == 0 { recorded } else { recorded.min(host_bytes) };

    Allocator::new(space, &vol.chunks, nodesize, crate::commit::mirror_count(device_bytes))
}

// Finding items
//
// The free space tree changes while it is read, so a search must see the
// transaction's blocks. `Trans::node_at` and `Node::search` do this

/// Greatest key at or below `key`, with its item, as this transaction sees the
/// tree: `btrfs_search_prev_slot` in `free-space-tree.c`. A free run is named
/// by its start, so the item containing an address is the last at or before it.
///
/// One descent: a node's key is the smallest beneath its pointer, so
/// `Err(n) => n - 1` at each level enters a subtree whose first key is at or
/// below the target. `Err(0)` happens only at the root, meaning nothing is at
/// or before this key
fn search_prev<A: BlockAlloc>(
    trans: &Trans<A>,
    root: u64,
    key: &Key,
    read: ReadBlock<'_>,
) -> Result<Option<(Key, Vec<u8>)>, BtrfsError> {
    let mut logical = trans.new_root_of(root).unwrap_or(root);

    for _ in 0..=MAX_LEVEL as usize {
        let node = trans.node_at(logical, read)?;
        let slot = match node.search(key)? {
            Ok(exact) => exact,
            Err(0) => return Ok(None),
            Err(next) => next - 1,
        };
        if node.is_leaf() {
            let mut body: Vec<u8> = Vec::new();
            let data = node.item_data(slot)?;
            body.try_reserve_exact(data.len())
                .map_err(|_| BtrfsError::OutOfMemory)?;
            body.extend_from_slice(data);
            return Ok(Some((node.key_at(slot)?, body)));
        }
        logical = node.blockptr_at(slot)?;
    }
    Err(BtrfsError::Corrupt)
}

/// The item under exactly this key, or `None`
fn find_exact<A: BlockAlloc>(
    trans: &Trans<A>,
    root: u64,
    key: &Key,
    read: ReadBlock<'_>,
) -> Result<Option<Vec<u8>>, BtrfsError> {
    match search_prev(trans, root, key, read)? {
        Some((found, body)) if found == *key => Ok(Some(body)),
        _ => Ok(None),
    }
}

/// A key at the end of everything under `objectid`, so `search_prev` lands on
/// an item starting at that address. The reference writes
/// `key.type = (u8)-1; key.offset = (u64)-1`
fn last_at(objectid: u64) -> Key {
    Key { objectid, kind: u8::MAX, offset: u64::MAX }
}

// The free space info item

/// What a group's `FREE_SPACE_INFO` says: run count and representation
fn read_info<A: BlockAlloc>(
    trans: &Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    read: ReadBlock<'_>,
) -> Result<(u32, u32), BtrfsError> {
    let key = Key { objectid: group.start, kind: FREE_SPACE_INFO_KEY, offset: group.length };
    let body = find_exact(trans, fst_root, &key, read)?.ok_or(BtrfsError::Corrupt)?;
    if body.len() != fsi::SIZE_OF {
        return Err(BtrfsError::Corrupt);
    }
    let count = u32_at(&body, fsi::EXTENT_COUNT)?;
    let flags = u32_at(&body, fsi::FLAGS)?;
    if flags & !FREE_SPACE_FLAGS_MASK != 0 {
        return Err(BtrfsError::Corrupt);
    }
    Ok((count, flags))
}

/// Counts at which the kernel changes a group's representation:
/// `btrfs_set_free_space_tree_thresholds`. Above the high one an extent group
/// becomes bitmaps; below the low one a bitmap group becomes extents. The
/// hundred-run gap is its hysteresis. This driver refuses at the same points
fn thresholds(group_length: u64, sectorsize: u32) -> (u32, u32) {
    let bitmap_range = sectorsize as u64 * FREE_SPACE_BITMAP_BITS;
    let num_bitmaps = group_length.div_ceil(bitmap_range);
    let bitmap_size = ITEM_SIZE as u64 + FREE_SPACE_BITMAP_SIZE as u64;
    let total = num_bitmaps.saturating_mul(bitmap_size);
    let high = (total / ITEM_SIZE as u64).min(u32::MAX as u64) as u32;
    let low = if high > 100 { high - 100 } else { 0 };
    (high, low)
}

/// Moves a group's run count by `delta`, refusing a threshold crossing:
/// `update_free_space_extent_count` with a refusal for its conversions.
/// `delta == 0` short-circuits, as in the reference
fn bump_extent_count<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    delta: i64,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    if delta == 0 {
        return Ok(());
    }
    let (count, flags) = read_info(trans, fst_root, group, read)?;
    let next = i64::from(count)
        .checked_add(delta)
        .ok_or(BtrfsError::Corrupt)?;
    if next < 0 || next > u32::MAX as i64 {
        return Err(BtrfsError::Corrupt);
    }
    let next = next as u32;

    let (high, low) = thresholds(group.length, trans.sectorsize);
    let bitmaps = flags & FREE_SPACE_USING_BITMAPS != 0;
    if (!bitmaps && next > high) || (bitmaps && next < low) {
        // The kernel would convert here. A group in a form its info item does
        // not name would fail the kernel's checker
        return Err(BtrfsError::Unsupported);
    }

    let mut body: Vec<u8> = Vec::new();
    body.try_reserve_exact(fsi::SIZE_OF)
        .map_err(|_| BtrfsError::OutOfMemory)?;
    body.extend_from_slice(&next.to_le_bytes());
    body.extend_from_slice(&flags.to_le_bytes());
    let key = Key { objectid: group.start, kind: FREE_SPACE_INFO_KEY, offset: group.length };
    trans.update_item(fst_root, &key, &body, read)
}

// the extent form

/// Takes `[start, start + len)` out of a one-item-per-run tree:
/// `remove_free_space_extent`, with four cases of cutting a run. The run is in
/// the key, so a cut is a delete and up to two inserts.
///
/// The count moves before the items, unlike the reference, so a refused
/// threshold crossing leaves the item undeleted
fn remove_free_space_extent<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    start: u64,
    len: u64,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    let end = start.checked_add(len).ok_or(BtrfsError::InvalidArg)?;

    let (found, body) =
        search_prev(trans, fst_root, &last_at(start), read)?.ok_or(BtrfsError::Corrupt)?;
    if found.kind != FREE_SPACE_EXTENT_KEY || !body.is_empty() {
        return Err(BtrfsError::Corrupt);
    }
    let fs = found.objectid;
    let fe = fs.checked_add(found.offset).ok_or(BtrfsError::Corrupt)?;
    // The reference asserts this. An error here, guarding against giving out
    // the same bytes twice
    if start < fs || end > fe {
        return Err(BtrfsError::Corrupt);
    }

    // Case 1 removes a run; cases 2 and 3 leave one, no net change; case 4
    // leaves two
    let mut delta: i64 = -1;
    if start > fs {
        delta += 1;
    }
    if end < fe {
        delta += 1;
    }
    bump_extent_count(trans, fst_root, group, delta, read)?;

    trans.delete_item(fst_root, &found, read)?;
    if start > fs {
        let k = Key { objectid: fs, kind: FREE_SPACE_EXTENT_KEY, offset: start - fs };
        trans.insert_item(fst_root, &k, &[], read)?;
    }
    if end < fe {
        let k = Key { objectid: end, kind: FREE_SPACE_EXTENT_KEY, offset: fe - end };
        trans.insert_item(fst_root, &k, &[], read)?;
    }
    Ok(())
}

/// Returns `[start, start + len)`, merging with what it touches:
/// `add_free_space_extent`, with four cases. Touching runs must be one, or the
/// count is wrong and a later allocation of the joined length fails
fn add_free_space_extent<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    start: u64,
    len: u64,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    let end = start.checked_add(len).ok_or(BtrfsError::InvalidArg)?;
    if start < group.start || end > group.end() {
        return Err(BtrfsError::Corrupt);
    }

    let mut new = Key { objectid: start, kind: FREE_SPACE_EXTENT_KEY, offset: len };
    let mut delta: i64 = 1;
    let mut absorb: Vec<Key> = Vec::new();

    // The left neighbor, if this run does not start the group
    if start > group.start {
        if let Some((found, _)) = search_prev(trans, fst_root, &last_at(start - 1), read)? {
            if found.kind == FREE_SPACE_EXTENT_KEY {
                let fe = found
                    .objectid
                    .checked_add(found.offset)
                    .ok_or(BtrfsError::Corrupt)?;
                if fe > start {
                    // The neighbor overlaps what is being freed: two records
                    // contradict
                    return Err(BtrfsError::Corrupt);
                }
                if fe == start {
                    new.objectid = found.objectid;
                    new.offset += found.offset;
                    delta -= 1;
                    absorb.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
                    absorb.push(found);
                }
            } else if found.kind != FREE_SPACE_INFO_KEY {
                return Err(BtrfsError::Corrupt);
            }
        }
    }

    // The right neighbor, if it does not end the group
    if end < group.end() {
        if let Some((found, _)) = search_prev(trans, fst_root, &last_at(end), read)? {
            if found.kind == FREE_SPACE_EXTENT_KEY && found.objectid == end {
                new.offset += found.offset;
                delta -= 1;
                absorb.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
                absorb.push(found);
            }
        }
    }

    bump_extent_count(trans, fst_root, group, delta, read)?;
    for k in &absorb {
        trans.delete_item(fst_root, k, read)?;
    }
    trans.insert_item(fst_root, &new, &[], read)
}

// the bitmap form

/// The bitmap item covering `at`, as `(key, bytes)`
fn bitmap_at<A: BlockAlloc>(
    trans: &Trans<A>,
    fst_root: u64,
    at: u64,
    read: ReadBlock<'_>,
) -> Result<(Key, Vec<u8>), BtrfsError> {
    let (found, body) =
        search_prev(trans, fst_root, &last_at(at), read)?.ok_or(BtrfsError::Corrupt)?;
    if found.kind != FREE_SPACE_BITMAP_KEY {
        return Err(BtrfsError::Corrupt);
    }
    let end = found
        .objectid
        .checked_add(found.offset)
        .ok_or(BtrfsError::Corrupt)?;
    if at < found.objectid || at >= end {
        return Err(BtrfsError::Corrupt);
    }
    Ok((found, body))
}

/// Whether the sector at `at` is free: `btrfs_free_space_test_bit`. Least
/// significant bit first per byte, as in `GroupScan::item` and
/// `extent_buffer_test_bit`
fn test_bit(key: &Key, body: &[u8], at: u64, sectorsize: u64) -> Result<bool, BtrfsError> {
    let i = (at - key.objectid) / sectorsize;
    let byte = u8_at(body, (i / 8) as usize)?;
    Ok(byte >> (i % 8) & 1 != 0)
}

/// Sets or clears each bit of `[start, start + len)` across as many bitmap
/// items as it spans: `free_space_modify_bits` and its loop in
/// `modify_free_space_bitmap`. Each item is rewritten whole with `update_item`,
/// keeping its size, fixed by its key's range
fn modify_bits<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    start: u64,
    len: u64,
    set: bool,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    let sectorsize = trans.sectorsize as u64;
    let end = start.checked_add(len).ok_or(BtrfsError::InvalidArg)?;
    let mut at = start;

    while at < end {
        let (key, mut body) = bitmap_at(trans, fst_root, at, read)?;
        let item_end = key.objectid + key.offset;
        let stop = end.min(item_end);

        let mut b = at;
        while b < stop {
            let i = ((b - key.objectid) / sectorsize) as usize;
            let byte = body.get_mut(i / 8).ok_or(BtrfsError::Corrupt)?;
            if set {
                *byte |= 1 << (i % 8);
            } else {
                *byte &= !(1 << (i % 8));
            }
            b += sectorsize;
        }

        trans.update_item(fst_root, &key, &body, read)?;
        at = item_end;
    }
    Ok(())
}

/// Takes a range out of, or returns it to, a bitmap group:
/// `modify_free_space_bitmap`. Neighbors decide the run count:
///
///   removing  the range was in one run, lost unless free space is right before or after, each leaving a piece
///   adding    a new run, unless it touches a free neighbor on either side, each merging
fn modify_free_space_bitmap<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    start: u64,
    len: u64,
    remove: bool,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    let sectorsize = trans.sectorsize as u64;
    let end = start.checked_add(len).ok_or(BtrfsError::InvalidArg)?;
    if start < group.start || end > group.end() {
        return Err(BtrfsError::Corrupt);
    }

    let prev_set = if start > group.start {
        let at = start - sectorsize;
        let (k, body) = bitmap_at(trans, fst_root, at, read)?;
        test_bit(&k, &body, at, sectorsize)?
    } else {
        false
    };
    let next_set = if end < group.end() {
        let (k, body) = bitmap_at(trans, fst_root, end, read)?;
        test_bit(&k, &body, end, sectorsize)?
    } else {
        false
    };

    let mut delta: i64 = if remove { -1 } else { 1 };
    if remove {
        if prev_set {
            delta += 1;
        }
        if next_set {
            delta += 1;
        }
    } else {
        if prev_set {
            delta -= 1;
        }
        if next_set {
            delta -= 1;
        }
    }

    // Before the bits, so a threshold crossing is refused with the tree
    // untouched
    bump_extent_count(trans, fst_root, group, delta, read)?;
    modify_bits(trans, fst_root, start, len, !remove, read)
}

// Applying one

/// Takes `[start, start + len)` out of a group in its representation:
/// `__btrfs_remove_from_free_space_tree`. The info item says which form
pub fn remove_free_space<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    start: u64,
    len: u64,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    let (_, flags) = read_info(trans, fst_root, group, read)?;
    if flags & FREE_SPACE_USING_BITMAPS != 0 {
        modify_free_space_bitmap(trans, fst_root, group, start, len, true, read)
    } else {
        remove_free_space_extent(trans, fst_root, group, start, len, read)
    }
}

/// Returns `[start, start + len)` to a group in its representation:
/// `__btrfs_add_to_free_space_tree`. A pin becomes this as its transaction is
/// written
pub fn add_free_space<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    start: u64,
    len: u64,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    let (_, flags) = read_info(trans, fst_root, group, read)?;
    if flags & FREE_SPACE_USING_BITMAPS != 0 {
        modify_free_space_bitmap(trans, fst_root, group, start, len, false, read)
    } else {
        add_free_space_extent(trans, fst_root, group, start, len, read)
    }
}

fn apply_change<A: BlockAlloc>(
    trans: &mut Trans<A>,
    fst_root: u64,
    group: &FreeSpace,
    c: Change,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    if c.alloc {
        remove_free_space(trans, fst_root, group, c.start, c.len, read)
    } else {
        add_free_space(trans, fst_root, group, c.start, c.len, read)
    }
}

// The block groups

/// Writes each block group item whose `used` moved, and says if any did:
/// `btrfs_update_block_group`'s half reaching the medium. Rebuilt whole: `used`
/// is the running count, `chunk_objectid` constant, `flags` the chunk's type
/// word.
///
/// `bg_root` is `BLOCK_GROUP_TREE` since 6.x and the extent tree before, per
/// `SpaceInfo::from_block_group_tree`
fn write_block_groups(
    trans: &mut Trans<Allocator>,
    bg_root: u64,
    read: ReadBlock<'_>,
) -> Result<bool, BtrfsError> {
    let mut wrote = false;

    for i in 0..trans.alloc.view.groups.len() {
        let g = &trans.alloc.view.groups[i];
        if g.used == trans.alloc.written_used[i] {
            continue;
        }
        let (start, length, used, flags) = (g.start, g.length, g.used, g.flags);
        if used > length {
            return Err(BtrfsError::Corrupt);
        }

        let mut body: Vec<u8> = Vec::new();
        body.try_reserve_exact(bgi::SIZE_OF)
            .map_err(|_| BtrfsError::OutOfMemory)?;
        body.extend_from_slice(&used.to_le_bytes());
        body.extend_from_slice(&FIRST_CHUNK_TREE_OBJECTID.to_le_bytes());
        body.extend_from_slice(&flags.to_le_bytes());

        let key = Key { objectid: start, kind: BLOCK_GROUP_ITEM_KEY, offset: length };
        trans.update_item(bg_root, &key, &body, read)?;
        trans.alloc.written_used[i] = used;
        wrote = true;
    }
    Ok(wrote)
}

// The fixpoint

// The root items

/// Points each moved tree's `ROOT_ITEM` at its root's new place, and says if
/// any needed it. The tree comes from `header::OWNER` of the block, as
/// `Trans::new_block` copies it.
///
/// The root tree's move is skipped; the superblock names it, written last.
///
/// `generation_v2` is written with `generation` when the item has one.
/// btrfs-progs identifies a modern item by the two being equal
fn write_root_items(
    trans: &mut Trans<Allocator>,
    root_tree: u64,
    read: ReadBlock<'_>,
) -> Result<bool, BtrfsError> {
    let moved: Vec<(u64, u64)> = trans.moved_roots.iter().map(|(a, b)| (*a, *b)).collect();
    let generation = trans.generation;
    let mut wrote = false;

    for (_old, new) in moved {
        let block = trans.block_bytes(new, read)?;
        let owner = u64_at(&block, crate::node::header::OWNER)?;
        let level = u8_at(&block, crate::node::header::LEVEL)?;
        if owner == crate::fs_tree::ROOT_TREE_OBJECTID {
            continue;
        }

        // The highest `ROOT_ITEM` under this objectid. The offset is a
        // generation, so this searches for the last
        let probe = Key { objectid: owner, kind: crate::fs_tree::ROOT_ITEM_KEY, offset: u64::MAX };
        let (key, mut body) = match search_prev(trans, root_tree, &probe, read)? {
            Some(found) => found,
            None => return Err(BtrfsError::NotFound),
        };
        if key.objectid != owner || key.kind != crate::fs_tree::ROOT_ITEM_KEY {
            return Err(BtrfsError::NotFound);
        }
        if body.len() < crate::fs_tree::root_item::LEGACY_SIZE {
            return Err(BtrfsError::Corrupt);
        }
        if u64_at(&body, crate::fs_tree::root_item::BYTENR)? == new {
            continue;
        }

        let at = crate::fs_tree::root_item::BYTENR;
        body[at..at + 8].copy_from_slice(&new.to_le_bytes());
        let at = crate::fs_tree::root_item::GENERATION;
        body[at..at + 8].copy_from_slice(&generation.to_le_bytes());
        body[crate::fs_tree::root_item::LEVEL] = level;
        let v2 = crate::fs_tree::root_item::LEGACY_SIZE;
        if body.len() >= v2 + 8 {
            body[v2..v2 + 8].copy_from_slice(&generation.to_le_bytes());
        }

        trans.update_item(root_tree, &key, &body, read)?;
        wrote = true;
    }
    Ok(wrote)
}

// The fixpoint

/// Rounds `settle` allows, a bound. Each round applies changes from the last or
/// writes block group items, and both end: once a tree's path is this
/// transaction's, copy-on-write allocates nothing more (`Trans::cow_block`'s
/// rule 2)
const SETTLE_ROUNDS: usize = 32;

/// Writes everything the allocator did to the free space tree and block group
/// items, including what that caused.
///
/// Recording an allocation changes the free space tree, copying blocks,
/// allocating and pinning more. It terminates since a second change to a copied
/// tree costs no blocks.
///
/// Called once, after the transaction's change and before sealing. Three kinds
/// of work: free space tree, block group items, and each moved tree's
/// `ROOT_ITEM`, the last covering the extent tree too, through `moved_roots`
/// and each block's owner.
///
/// Not here: the extent tree's items and the superblock (`commit.rs`). The
/// chunk tree's root is also in the superblock, a second field nothing here
/// knows; nothing copies one.
///
/// On return, `trans.new_root_of(root_tree)` is the root tree's place and
/// `Allocator::bytes_used` the superblock's `bytes_used`, both for the caller
/// to write
pub fn settle(
    trans: &mut Trans<Allocator>,
    fst_root: u64,
    bg_root: u64,
    root_tree: u64,
    read: ReadBlock<'_>,
) -> Result<(), BtrfsError> {
    for _ in 0..SETTLE_ROUNDS {
        let pending = trans.alloc.take_pending()?;
        if !pending.is_empty() {
            for c in pending {
                let g = trans
                    .alloc
                    .group_of(c.start)
                    .ok_or(BtrfsError::Corrupt)?;
                // The group is cloned since applying borrows the transaction,
                // holding the allocator. Only `start`, `length`, and `flags`
                // are read, which do not move
                let g0 = &trans.alloc.view.groups[g];
                let group = FreeSpace {
                    start: g0.start,
                    length: g0.length,
                    used: g0.used,
                    flags: g0.flags,
                    runs: Vec::new(),
                };
                apply_change(trans, fst_root, &group, c, read)?;
            }
            continue;
        }
        if write_block_groups(trans, bg_root, read)? {
            continue;
        }
        if write_root_items(trans, root_tree, read)? {
            continue;
        }
        return Ok(());
    }
    Err(BtrfsError::Corrupt)
}
