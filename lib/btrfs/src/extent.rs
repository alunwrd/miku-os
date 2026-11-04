// The extent tree: what each tree block is, and which root owns it
//
// `btrfs check`'s `[3/8] checking extents` cross-checks this against reachable
// tree blocks, and a block with no extent item is one the next kernel considers
// free and allocates over.
//
// Scope
//
// Metadata blocks only: describing a tree block this transaction allocated, and
// removing it when dropped. Data extents (`EXTENT_DATA_REF`, `SHARED_DATA_REF`,
// snapshot refcounts) are absent; `parse_tree_block` says so by name.
//
// Which form of the item
//
// Volumes from `mkfs.btrfs` here have incompat `0x361`:
// `MIXED_BACKREF | BIG_METADATA | EXTENDED_IREF | SKINNY_METADATA | NO_HOLES`.
//
// `SKINNY_METADATA` decides the key. A skinny metadata extent is keyed
// `(bytenr, METADATA_ITEM_KEY, level)`, a `btrfs_extent_item` followed by its
// inline backref. The older form is keyed `(bytenr, EXTENT_ITEM_KEY, length)`
// with an 18-byte `btrfs_tree_block_info` between. `alloc_reserved_tree_block`
// in `extent-tree.c` branches; `check_extent_item` in `tree-checker.c` rejects
// the wrong one.
//
// Only the skinny form is written: `ExtentTree::new` returns `Unsupported` on a
// volume without the bit, as does `parse_tree_block` for an `EXTENT_ITEM_KEY`.
// The other form's offsets are in `tbi` below.
//
// `MIXED_BACKREF` decides the backref. In `update_ref_for_cow` (`ctree.c`), a
// block with `BTRFS_MIXED_BACKREF_REV` whose owner is not a relocation tree
// gets no `FULL_BACKREF` and a zero `parent_start`, so the inline ref is a
// `TREE_BLOCK_REF` with the owning root's objectid. No snapshots or relocation
// trees here, so `SHARED_BLOCK_REF` is read and not written.
//
// Removing does not free space
//
// `btrfs_free_tree_block` has two acts. Dropping the refcount to zero deletes
// the item. Reuse must wait: `pin_down_extent` holds the address until commit,
// since the old block is what a crash falls back to. `trans.rs` rule 1 is that
// pin, enforced by `check_fresh`. The free space tree and a block group's
// `used` are in `space.rs`.
//
// Ordering
//
// Writing the extent tree allocates blocks needing extent tree items. The
// kernel uses delayed refs; a transaction built whole in memory uses a fixpoint
// loop: apply pending changes, then again over what that produced, until a pass
// changes nothing.
//
// `extent_pass` is one pass, `settle_extent_tree` the loop. `ExtentSettle`
// survives between passes, and is public since `space::settle` has the same
// shape for the free space tree, block groups, and root items, and each
// allocates blocks the other accounts for. The caller runs `extent_pass` and
// `space::settle` in one round until a round changes nothing.
//
// It terminates: metadata comes from allocated chunks, and rule 2 copies a
// block at most once, so the second pass allocates only for a leaf split and
// the third usually nothing. Not converging inside `MAX_SETTLE_PASSES` is an
// error.
//
// Reference
//
// `fs/btrfs/extent-tree.c`: `alloc_reserved_tree_block` writes the item,
// `run_delayed_tree_ref` picks add or drop, `__btrfs_free_extent` drops the
// refcount and deletes at zero, `btrfs_free_tree_block` pins.
// `fs/btrfs/ctree.c`'s `update_ref_for_cow` picks the backref.
// `fs/btrfs/tree-checker.c`'s `check_extent_item` runs on the next mount.
//
// Offsets came from the compiler over `include/uapi/linux/btrfs_tree.h`:
// `offsetof` per field of `btrfs_extent_item`, `btrfs_tree_block_info`, and
// `btrfs_extent_inline_ref`, as in `structs.rs`.

use crate::error::BtrfsError;
use crate::node::header;
use crate::structs::*;
use crate::trans::{BlockAlloc, ReadBlock, Trans};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

// Key types

/// `(bytenr, EXTENT_ITEM_KEY, length)`: a data extent, or a metadata block on a
/// volume without `SKINNY_METADATA`. Read, never written
pub const EXTENT_ITEM_KEY: u8 = 168;
/// `(bytenr, METADATA_ITEM_KEY, level)`: the skinny form, the only one written
pub const METADATA_ITEM_KEY: u8 = 169;
/// An inline backref naming the objectid of the root that owns the block
pub const TREE_BLOCK_REF_KEY: u8 = 176;
/// An inline backref naming the parent block, written by relocation and shared
/// snapshot subtrees, neither made here
pub const SHARED_BLOCK_REF_KEY: u8 = 182;
pub const EXTENT_DATA_REF_KEY: u8 = 178;
pub const SHARED_DATA_REF_KEY: u8 = 184;

// The flags word

pub const EXTENT_FLAG_DATA: u64 = 1 << 0;
pub const EXTENT_FLAG_TREE_BLOCK: u64 = 1 << 1;
/// Set on a tree block whose backrefs name parent blocks. `check_extent_item`
/// refuses it on a data extent, and `update_ref_for_cow` sets it only for a
/// relocation tree or a pre-`MIXED_BACKREF` volume
pub const BLOCK_FLAG_FULL_BACKREF: u64 = 1 << 8;

// The items

/// Field offsets in `struct btrfs_extent_item`
pub mod ei {
    pub const REFS: usize = 0;
    pub const GENERATION: usize = 8;
    pub const FLAGS: usize = 16;
    /// The whole struct, the minimum `check_extent_item` allows for either key
    /// type
    pub const SIZE_OF: usize = 24;
}

/// Field offsets in `struct btrfs_tree_block_info`, the 18 bytes between the
/// extent item and its first backref in the non-skinny form. Not written;
/// stated for the form refused below
pub mod tbi {
    /// A whole `btrfs_disk_key`: the first key of the block it describes
    pub const KEY: usize = 0;
    pub const LEVEL: usize = 17;
    pub const SIZE_OF: usize = 18;
}

/// Field offsets in `struct btrfs_extent_inline_ref`: one byte of type, then an
/// unaligned `__le64`, so 9 bytes
pub mod iref {
    pub const TYPE: usize = 0;
    pub const OFFSET: usize = 1;
    pub const SIZE_OF: usize = 9;
}

/// A skinny metadata extent with one inline backref: 24 + 9. The size of each
/// item here, and what `mkfs.btrfs` writes per tree block
pub const SKINNY_TREE_BLOCK_SIZE: usize = ei::SIZE_OF + iref::SIZE_OF;

/// Passes `settle_extent_tree` allows. Three is usual and a leaf split can cost
/// one more; past this it is an error
pub const MAX_SETTLE_PASSES: usize = 16;

// The two forms

/// Who references a tree block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backref {
    /// `TREE_BLOCK_REF`: objectid of the owning root. What a `MIXED_BACKREF`
    /// volume writes for an ordinary copy-on-write, the only form produced here
    Root(u64),
    /// `SHARED_BLOCK_REF`: address of the block pointing at this one. Read for
    /// relocated or snapshotted volumes; never written
    Shared(u64),
}

/// A metadata extent item, decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeBlockExtent {
    pub bytenr: u64,
    pub level: u8,
    pub refs: u64,
    pub generation: u64,
    pub flags: u64,
    pub backref: Backref,
}

/// The extent tree of one volume. The format is decided once, so one tree
/// cannot hold two shapes
#[derive(Clone, Copy, Debug)]
pub struct ExtentTree {
    root: u64,
}

impl ExtentTree {
    /// `root` is the extent tree's address on the medium, the name
    /// `Trans::cow_path` and `Trans::insert_item` start from, so a moved tree
    /// is followed and not re-copied.
    ///
    /// Refused by name:
    ///
    /// * no `SKINNY_METADATA`: the item would need the `EXTENT_ITEM` key and a
    ///   `btrfs_tree_block_info`, and
    ///   `check_extent_item` refuses a `METADATA_ITEM` on such a volume
    /// * no `MIXED_BACKREF`: `update_ref_for_cow` would set `FULL_BACKREF`,
    ///   with backrefs naming parent blocks
    /// * `EXTENT_TREE_V2`: a different tree
    pub fn new(root: u64, incompat: u64) -> Result<Self, BtrfsError> {
        if incompat & FEATURE_INCOMPAT_SKINNY_METADATA == 0 {
            return Err(BtrfsError::Unsupported);
        }
        if incompat & FEATURE_INCOMPAT_MIXED_BACKREF == 0 {
            return Err(BtrfsError::Unsupported);
        }
        if incompat & FEATURE_INCOMPAT_EXTENT_TREE_V2 != 0 {
            return Err(BtrfsError::Unsupported);
        }
        Ok(ExtentTree { root })
    }

    pub fn root(&self) -> u64 {
        self.root
    }
}

/// The key of a skinny metadata extent. The offset is the level, not the
/// length: `__btrfs_free_extent` looks up `(bytenr, METADATA_ITEM, level)`, and
/// `check_extent_item` refuses an offset not below `BTRFS_MAX_LEVEL`
pub fn skinny_key(bytenr: u64, level: u8) -> Key {
    Key { objectid: bytenr, kind: METADATA_ITEM_KEY, offset: level as u64 }
}

/// The 33 bytes of a new tree block: one reference, this transaction's
/// generation, and an inline `TREE_BLOCK_REF` naming the owning root.
/// `alloc_reserved_tree_block` without the `tree_block_info` branch. The
/// refcount is 1: a newly allocated block is in exactly one tree
pub fn encode_tree_block(generation: u64, owner: u64) -> [u8; SKINNY_TREE_BLOCK_SIZE] {
    let mut out = [0u8; SKINNY_TREE_BLOCK_SIZE];
    out[ei::REFS..ei::REFS + 8].copy_from_slice(&1u64.to_le_bytes());
    out[ei::GENERATION..ei::GENERATION + 8].copy_from_slice(&generation.to_le_bytes());
    // Exactly one of DATA and TREE_BLOCK, per `check_extent_item`, and no
    // FULL_BACKREF, since the ref names a root
    out[ei::FLAGS..ei::FLAGS + 8].copy_from_slice(&EXTENT_FLAG_TREE_BLOCK.to_le_bytes());
    let at = ei::SIZE_OF;
    out[at + iref::TYPE] = TREE_BLOCK_REF_KEY;
    out[at + iref::OFFSET..at + iref::OFFSET + 8].copy_from_slice(&owner.to_le_bytes());
    out
}

/// Reads a metadata extent item.
///
/// Refused by name:
///
/// * `EXTENT_ITEM_KEY`, a data extent or the non-skinny form: `Unsupported`
/// * an item that is not exactly one extent item and one inline backref: two
///   references means a snapshot's shared subtree
pub fn parse_tree_block(key: &Key, data: &[u8]) -> Result<TreeBlockExtent, BtrfsError> {
    if key.kind != METADATA_ITEM_KEY {
        return Err(BtrfsError::Unsupported);
    }
    if key.offset >= MAX_LEVEL as u64 {
        // `check_extent_item`: a METADATA_ITEM key's offset is a level
        return Err(BtrfsError::Corrupt);
    }
    if data.len() != SKINNY_TREE_BLOCK_SIZE {
        return Err(BtrfsError::Unsupported);
    }

    let flags = u64_at(data, ei::FLAGS)?;
    if flags & EXTENT_FLAG_TREE_BLOCK == 0 || flags & EXTENT_FLAG_DATA != 0 {
        return Err(BtrfsError::Corrupt);
    }

    let at = ei::SIZE_OF;
    let ty = u8_at(data, at + iref::TYPE)?;
    let off = u64_at(data, at + iref::OFFSET)?;
    let backref = match ty {
        TREE_BLOCK_REF_KEY => Backref::Root(off),
        SHARED_BLOCK_REF_KEY => Backref::Shared(off),
        // A data backref in an item flagged tree block contradicts itself
        _ => return Err(BtrfsError::Corrupt),
    };

    Ok(TreeBlockExtent {
        bytenr: key.objectid,
        level: key.offset as u8,
        refs: u64_at(data, ei::REFS)?,
        generation: u64_at(data, ei::GENERATION)?,
        flags,
        backref,
    })
}

// What a pass remembers

/// What the fixpoint loop has done, carried between passes. Public, since the
/// free space tree and block groups settle together with the extent tree, and
/// their driver uses `extent_pass` with one of these
#[derive(Default, Debug)]
pub struct ExtentSettle {
    /// Blocks with an item and the level of its key. A block can be discarded
    /// and its address reused at another level, and the item to delete is the
    /// one written
    described: BTreeMap<u64, u8>,
    /// Blocks whose item has been deleted
    removed: BTreeSet<u64>,
}

impl ExtentSettle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Blocks this settle has given an extent item
    pub fn described(&self) -> usize {
        self.described.len()
    }

    /// Blocks whose extent item this settle has deleted
    pub fn removed(&self) -> usize {
        self.removed.len()
    }

    pub fn is_described(&self, bytenr: u64) -> bool {
        self.described.contains_key(&bytenr)
    }
}

// The operations

impl<A: BlockAlloc> Trans<A> {
    /// Describes a tree block this transaction allocated. `owner` is the root's
    /// objectid, as in the block's header; `btrfs check`'s backref walk checks
    /// it.
    ///
    /// `alloc_reserved_tree_block`. Placement is `shape.rs`'s `insert_item`,
    /// refusing an existing key, so describing a block twice is an error
    pub fn add_tree_block(
        &mut self,
        ext: &ExtentTree,
        bytenr: u64,
        level: u8,
        owner: u64,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        // `check_extent_item` checks both on the next mount
        if bytenr % self.sectorsize as u64 != 0 {
            return Err(BtrfsError::InvalidArg);
        }
        if level >= MAX_LEVEL {
            return Err(BtrfsError::InvalidArg);
        }
        let body = encode_tree_block(self.generation, owner);
        self.insert_item(ext.root(), &skinny_key(bytenr, level), &body, read)
    }

    /// The new tree no longer references the block, so its item goes: the
    /// refcount reaching zero in `__btrfs_free_extent`. This does not make the
    /// address reusable, as the old block is what a crash falls back to (rule 1
    /// in `trans.rs`, enforced by `check_fresh`)
    pub fn drop_tree_block(
        &mut self,
        ext: &ExtentTree,
        bytenr: u64,
        level: u8,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        self.delete_item(ext.root(), &skinny_key(bytenr, level), read)
    }

    /// One pass of the fixpoint loop; returns items changed.
    ///
    /// 1. Items for blocks that stopped existing. `shape.rs` can discard a
    ///    merged leaf, and `discard_block` returns the address to the
    ///    allocator, even for another level, so the written level is
    ///    remembered.
    /// 2. Items for blocks no longer referenced: `trans.rs`'s pinned set, on
    ///    the medium as the old tree.
    /// 3. Items for new blocks, from their headers; see `held_blocks`.
    ///
    /// Deleting first keeps the leaves from growing across a net removal
    pub fn extent_pass(
        &mut self,
        ext: &ExtentTree,
        state: &mut ExtentSettle,
        read: ReadBlock<'_>,
    ) -> Result<usize, BtrfsError> {
        let mut changed = 0usize;

        // 1. Described blocks no longer held, or another block at the same
        //    address
        let held = self.held_blocks()?;
        let stale: Vec<(u64, u8)> = state
            .described
            .iter()
            .map(|(b, l)| (*b, *l))
            .filter(|(b, l)| held.get(b).map(|(now, _)| now) != Some(l))
            .collect();
        for (bytenr, level) in stale {
            self.drop_tree_block(ext, bytenr, level, read)?;
            state.described.remove(&bytenr);
            changed += 1;
        }

        // 2. Pinned blocks; their level is read from the medium
        let gone: Vec<u64> = self
            .freed_blocks()
            .filter(|b| !state.removed.contains(b))
            .collect();
        for bytenr in gone {
            let data = self.block_bytes(bytenr, read)?;
            let level = u8_at(&data, header::LEVEL)?;
            self.drop_tree_block(ext, bytenr, level, read)?;
            state.removed.insert(bytenr);
            changed += 1;
        }

        // 3. New blocks. The set is retaken, since deletions above copied more
        //    of the extent tree and may have dropped a merged leaf
        let fresh: Vec<(u64, u8, u64)> = self
            .held_blocks()?
            .into_iter()
            .filter(|(b, _)| !state.described.contains_key(b))
            .map(|(b, (level, owner))| (b, level, owner))
            .collect();
        for (bytenr, level, owner) in fresh {
            self.add_tree_block(ext, bytenr, level, owner, read)?;
            state.described.insert(bytenr, level);
            changed += 1;
        }

        Ok(changed)
    }

    /// Every block held by this transaction, with the level and owner from its
    /// header, as `check_extent_item` and `btrfs check`'s backref walk read
    /// them
    fn held_blocks(&self) -> Result<BTreeMap<u64, (u8, u64)>, BtrfsError> {
        self.dirty_blocks()
            .map(|(b, d)| Ok((b, (u8_at(d, header::LEVEL)?, u64_at(d, header::OWNER)?))))
            .collect()
    }

    /// Runs passes until one changes nothing, returning how many. The extent
    /// tree's half; a caller also owning the free space tree drives
    /// `extent_pass` from its own loop with a shared `ExtentSettle`
    pub fn settle_extent_tree(
        &mut self,
        ext: &ExtentTree,
        read: ReadBlock<'_>,
    ) -> Result<usize, BtrfsError> {
        self.settle_extent_tree_within(ext, MAX_SETTLE_PASSES, read)
    }

    /// `settle_extent_tree` with a named ceiling, so a test can trigger the
    /// error. `Corrupt` on running out of passes
    pub fn settle_extent_tree_within(
        &mut self,
        ext: &ExtentTree,
        max_passes: usize,
        read: ReadBlock<'_>,
    ) -> Result<usize, BtrfsError> {
        let mut state = ExtentSettle::new();
        for pass in 0..max_passes {
            if self.extent_pass(ext, &mut state, read)? == 0 {
                return Ok(pass);
            }
        }
        Err(BtrfsError::Corrupt)
    }
}
