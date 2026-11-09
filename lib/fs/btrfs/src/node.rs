// A tree block and its safety checks
//
// The one block parser: an internal node is keys and pointers, a leaf keys and
// data, both with a 101-byte header. A leaf:
//
//     +----------------+ 0
//     | header         |
//     +----------------+ 101
//     | item[0]        |   25 bytes: key, offset, size
//     | item[1]        |   growing forwards
//     | ...            |
//     +----------------+
//     |  free space    |
//     +----------------+
//     | ...            |
//     | data of [1]    |   growing backwards from the end
//     | data of [0]    |
//     +----------------+ nodesize
//
// `item.offset` is measured from the end of the header, so slot `i`'s bytes are
// at `101 + item[i].offset`. The two arrays must not meet.
//
// Checks are `__btrfs_check_leaf` and `__btrfs_check_node` from
// `fs/btrfs/tree-checker.c`. Once a block has passed, users need not repeat
// them

use crate::error::BtrfsError;
use crate::structs::*;
use alloc::vec::Vec;

/// Field offsets in `struct btrfs_header`
pub mod header {
    pub const CSUM: usize = 0;
    pub const FSID: usize = 32;
    pub const BYTENR: usize = 48;
    pub const FLAGS: usize = 56;
    pub const CHUNK_TREE_UUID: usize = 64;
    pub const GENERATION: usize = 80;
    pub const OWNER: usize = 88;
    pub const NRITEMS: usize = 96;
    pub const LEVEL: usize = 100;
}

/// Field offsets in `struct btrfs_item`, relative to the item's own start
pub mod item {
    pub const KEY: usize = 0;
    pub const OFFSET: usize = 17;
    pub const SIZE: usize = 21;
}

/// Field offsets in `struct btrfs_key_ptr`
pub mod key_ptr {
    pub const KEY: usize = 0;
    pub const BLOCKPTR: usize = 17;
    pub const GENERATION: usize = 25;
}

/// The block was written. The kernel refuses a node without this, as an
/// allocated unfilled block holds old contents
pub const HEADER_FLAG_WRITTEN: u64 = 1 << 0;
pub const HEADER_FLAG_RELOC: u64 = 1 << 1;

/// A tree block that passed every check, owning its bytes, as a node is read
/// once and walked many times
pub struct Node {
    data: Vec<u8>,
    pub bytenr: u64,
    pub generation: u64,
    pub owner: u64,
    pub level: u8,
    pub nritems: u32,
}

impl Node {
    pub fn is_leaf(&self) -> bool {
        self.level == 0
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// Space for item data in a leaf: everything after the header, where
    /// `item.offset` is measured from
    fn leaf_data_size(&self) -> u32 {
        (self.data.len() - HEADER_SIZE) as u32
    }

    // the keys

    /// Key at `slot`, for a node or leaf: different strides, both at the
    /// entry's start
    pub fn key_at(&self, slot: u32) -> Result<Key, BtrfsError> {
        if slot >= self.nritems {
            return Err(BtrfsError::InvalidArg);
        }
        let stride = if self.is_leaf() { ITEM_SIZE } else { KEY_PTR_SIZE };
        Key::read(&self.data, HEADER_SIZE + stride * slot as usize)
    }

    // an internal node

    /// Where the subtree under `slot` begins
    pub fn blockptr_at(&self, slot: u32) -> Result<u64, BtrfsError> {
        if self.is_leaf() || slot >= self.nritems {
            return Err(BtrfsError::InvalidArg);
        }
        let at = HEADER_SIZE + KEY_PTR_SIZE * slot as usize;
        u64_at(&self.data, at + key_ptr::BLOCKPTR)
    }

    /// Generation of that subtree. A pointer's generation newer than its tree
    /// cannot occur
    pub fn ptr_generation_at(&self, slot: u32) -> Result<u64, BtrfsError> {
        if self.is_leaf() || slot >= self.nritems {
            return Err(BtrfsError::InvalidArg);
        }
        let at = HEADER_SIZE + KEY_PTR_SIZE * slot as usize;
        u64_at(&self.data, at + key_ptr::GENERATION)
    }

    // a leaf

    /// Bytes of the item at `slot`. Bounds were checked at parse; returns a
    /// `Result` as the read primitive
    pub fn item_data(&self, slot: u32) -> Result<&[u8], BtrfsError> {
        if !self.is_leaf() || slot >= self.nritems {
            return Err(BtrfsError::InvalidArg);
        }
        let at = HEADER_SIZE + ITEM_SIZE * slot as usize;
        let off = u32_at(&self.data, at + item::OFFSET)? as usize;
        let size = u32_at(&self.data, at + item::SIZE)? as usize;
        let start = HEADER_SIZE + off;
        let end = start.checked_add(size).ok_or(BtrfsError::Corrupt)?;
        self.data.get(start..end).ok_or(BtrfsError::Corrupt)
    }

    pub fn item_size(&self, slot: u32) -> Result<u32, BtrfsError> {
        if !self.is_leaf() || slot >= self.nritems {
            return Err(BtrfsError::InvalidArg);
        }
        let at = HEADER_SIZE + ITEM_SIZE * slot as usize;
        u32_at(&self.data, at + item::SIZE)
    }

    // searching

    /// Slot for `key`, or where it would go. `Ok(slot)` is exact. `Err(slot)`
    /// is the first greater key; in an internal node the subtree is `slot - 1`,
    /// as a node's key is the smallest beneath its pointer. Binary search over
    /// keys the parse checked to be strictly increasing
    pub fn search(&self, key: &Key) -> Result<Result<u32, u32>, BtrfsError> {
        let mut lo = 0u32;
        let mut hi = self.nritems;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let k = self.key_at(mid)?;
            match k.cmp(key) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => return Ok(Ok(mid)),
            }
        }
        Ok(Err(lo))
    }
}

/// Reads a block already fetched into `data`. `expect` is the requested logical
/// address and `fsid` the filesystem, both checked against the block, which
/// exposes a wrong chunk mapping
pub fn parse_node(
    data: Vec<u8>,
    expect: u64,
    fsid: &[u8],
    sectorsize: u32,
) -> Result<Node, BtrfsError> {
    if data.len() < HEADER_SIZE + ITEM_SIZE {
        return Err(BtrfsError::Corrupt);
    }

    // Checksum first, as all after it trusts these bytes
    let stored = u32_at(&data, header::CSUM)?;
    let computed = fshost::crc32c::crc32c(0, &data[CSUM_SIZE..]);
    if stored != computed {
        return Err(BtrfsError::ChecksumMismatch);
    }

    if &data[header::FSID..header::FSID + FSID_SIZE] != fsid {
        return Err(BtrfsError::Corrupt);
    }
    let bytenr = u64_at(&data, header::BYTENR)?;
    if bytenr != expect {
        return Err(BtrfsError::Corrupt);
    }

    let flags = u64_at(&data, header::FLAGS)?;
    if flags & HEADER_FLAG_WRITTEN == 0 {
        return Err(BtrfsError::Corrupt);
    }

    let level = u8_at(&data, header::LEVEL)?;
    if level >= MAX_LEVEL {
        return Err(BtrfsError::Corrupt);
    }
    let nritems = u32_at(&data, header::NRITEMS)?;
    let generation = u64_at(&data, header::GENERATION)?;
    let owner = u64_at(&data, header::OWNER)?;

    let node = Node { data, bytenr, generation, owner, level, nritems };

    if node.is_leaf() {
        check_leaf(&node)?;
    } else {
        check_internal(&node, sectorsize)?;
    }
    Ok(node)
}

/// Parses a block a transaction is building. Skips the checksum, not computed
/// yet, and `WRITTEN`, deliberately clear. Shape is checked as for a read block
pub fn parse_node_in_flight(
    data: Vec<u8>,
    at: u64,
    sectorsize: u32,
) -> Result<Node, BtrfsError> {
    if data.len() < HEADER_SIZE + ITEM_SIZE {
        return Err(BtrfsError::Corrupt);
    }
    let bytenr = u64_at(&data, header::BYTENR)?;
    if bytenr != at {
        return Err(BtrfsError::Corrupt);
    }
    let level = u8_at(&data, header::LEVEL)?;
    if level >= MAX_LEVEL {
        return Err(BtrfsError::Corrupt);
    }
    let nritems = u32_at(&data, header::NRITEMS)?;
    let generation = u64_at(&data, header::GENERATION)?;
    let owner = u64_at(&data, header::OWNER)?;

    let node = Node { data, bytenr, generation, owner, level, nritems };
    if node.is_leaf() {
        check_leaf(&node)?;
    } else {
        check_internal(&node, sectorsize)?;
    }
    Ok(node)
}

/// Each key is strictly above the last, or a binary search reads wrong
fn check_key_order(node: &Node) -> Result<(), BtrfsError> {
    let mut prev = Key::default();
    for slot in 0..node.nritems {
        let k = node.key_at(slot)?;
        // The first key must beat `(0,0,0)` too: no valid key type is zero
        if k <= prev {
            return Err(BtrfsError::Corrupt);
        }
        prev = k;
    }
    Ok(())
}

fn check_internal(node: &Node, sectorsize: u32) -> Result<(), BtrfsError> {
    if node.level == 0 || node.level >= MAX_LEVEL {
        return Err(BtrfsError::Corrupt);
    }
    // An internal node with no children is a dead end in the middle of a tree
    let max = (node.data.len() - HEADER_SIZE) / KEY_PTR_SIZE;
    if node.nritems == 0 || node.nritems as usize > max {
        return Err(BtrfsError::Corrupt);
    }
    check_key_order(node)?;

    for slot in 0..node.nritems {
        let ptr = node.blockptr_at(slot)?;
        if ptr == 0 {
            return Err(BtrfsError::Corrupt);
        }
        if ptr % sectorsize as u64 != 0 {
            return Err(BtrfsError::Corrupt);
        }
    }
    Ok(())
}

fn check_leaf(node: &Node) -> Result<(), BtrfsError> {
    let capacity = node.leaf_data_size();
    // An empty leaf is legal, as an emptied tree has a root
    if node.nritems == 0 {
        return Ok(());
    }
    let max_items = capacity as usize / ITEM_SIZE;
    if node.nritems as usize > max_items {
        return Err(BtrfsError::Corrupt);
    }
    check_key_order(node)?;

    // Item data must tile the block's back exactly: no gap, overlap, or stray.
    // Comparing each with the last makes it one pass
    let mut expected_end = capacity;
    for slot in 0..node.nritems {
        let at = HEADER_SIZE + ITEM_SIZE * slot as usize;
        let off = u32_at(&node.data, at + item::OFFSET)?;
        let size = u32_at(&node.data, at + item::SIZE)?;

        let end = off.checked_add(size).ok_or(BtrfsError::Corrupt)?;
        if end != expected_end {
            return Err(BtrfsError::Corrupt);
        }
        if end > capacity {
            return Err(BtrfsError::Corrupt);
        }
        // The two arrays grow toward each other; data starting before the item
        // array ends would read the array
        let item_array_end = ITEM_SIZE * (slot as usize + 1);
        if (off as usize) < item_array_end {
            return Err(BtrfsError::Corrupt);
        }
        expected_end = off;
    }
    Ok(())
}
