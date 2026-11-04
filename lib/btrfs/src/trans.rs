// A transaction: changing a tree without changing the medium
//
// btrfs does not write over a live block. To change one item, the leaf is
// copied to a new address, each node up to the root is copied to point at the
// copy, and the new root is recorded. Until a superblock with the new
// generation reaches the medium, the old tree stands. This file builds the
// state in memory for `commit`.
//
//   1. A block freed by copy-on-write may not be reused in the same
//      transaction: its contents are
//      the fallback tree. The kernel calls this pinning (`pin_down_extent`, `extent-tree.c`).
//   2. A block is copied at most once per transaction; a second change goes in
//      the copy. Otherwise
//      the first copy leaks and the parent points at it.
//   3. A parent's key for a child follows the child: a node's key is the
//      smallest beneath its pointer.
//
// Allocation reads the free space tree and writes the extent tree, themselves
// changed here, so it arrives as a trait; see the fixpoint loop

use crate::error::BtrfsError;
use crate::node::{header, item, key_ptr, Node};
use crate::structs::*;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

/// Where new blocks come from and old ones go. A trait, since the real
/// allocator reads and writes trees through this machinery, and a test
/// allocator with a scratch range proves the rules without a free space tree
pub trait BlockAlloc {
    /// A logical address for one metadata block of `size` bytes, aligned to the
    /// sector size and not freed by this transaction
    fn alloc(&mut self, size: u64) -> Result<u64, BtrfsError>;

    /// The new tree no longer references this block. It is not free until
    /// commit: the old tree, the crash fallback, still points at it (rule 1)
    fn free(&mut self, logical: u64, size: u64);
}

/// One step of a descent: the block and the slot taken
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub logical: u64,
    pub slot: u32,
}

/// The path from a tree's root to a leaf, after copy-on-write. Every block is
/// new, belongs to this transaction, and may change in place. The root is
/// `steps[0].logical`, reported to the root tree at commit
#[derive(Clone, Debug)]
pub struct Path {
    pub steps: Vec<Step>,
}

impl Path {
    pub fn leaf(&self) -> Option<Step> {
        self.steps.last().copied()
    }
    pub fn depth(&self) -> usize {
        self.steps.len()
    }
}

/// A transaction in progress, holding every block it has written by logical
/// address. A read checks here first, so a second change lands in the copy
pub struct Trans<A: BlockAlloc> {
    /// New or modified blocks, none of which are on the medium yet
    dirty: BTreeMap<u64, Vec<u8>>,
    /// Blocks copied away from, not reusable before commit: rule 1
    freed: BTreeSet<u64>,
    /// Roots that moved: the tree's objectid and its new block
    pub moved_roots: BTreeMap<u64, u64>,
    pub alloc: A,
    pub nodesize: u32,
    pub sectorsize: u32,
    fsid: [u8; FSID_SIZE],
    /// Generation of every block this writes: one past the volume's, as the
    /// superblock will get
    pub generation: u64,
}

/// How a block is read in an open transaction: from it if touched, otherwise
/// from the medium. A closure, as the volume owns the host and the transaction
/// cannot borrow it for life
pub type ReadBlock<'a> = &'a mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>;

impl<A: BlockAlloc> Trans<A> {
    pub fn new(
        alloc: A,
        nodesize: u32,
        sectorsize: u32,
        fsid: [u8; FSID_SIZE],
        generation: u64,
    ) -> Self {
        Self {
            dirty: BTreeMap::new(),
            freed: BTreeSet::new(),
            moved_roots: BTreeMap::new(),
            alloc,
            nodesize,
            sectorsize,
            fsid,
            generation,
        }
    }

    pub fn is_dirty(&self, logical: u64) -> bool {
        self.dirty.contains_key(&logical)
    }

    pub fn was_freed(&self, logical: u64) -> bool {
        self.freed.contains(&logical)
    }

    /// Every block this transaction will write, in address order
    pub fn dirty_blocks(&self) -> impl Iterator<Item = (u64, &Vec<u8>)> {
        self.dirty.iter().map(|(k, v)| (*k, v))
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    /// Bytes of a block, from this transaction or the medium. The transaction
    /// first, which enforces rule 2
    fn block(&self, logical: u64, read: ReadBlock<'_>) -> Result<Vec<u8>, BtrfsError> {
        if let Some(b) = self.dirty.get(&logical) {
            return Ok(b.clone());
        }
        read(logical)
    }

    /// Parses a block that may be in this transaction, with its checksum
    /// uncomputed and `WRITTEN` clear, which `parse_node` would refuse
    pub fn node_at(&self, logical: u64, read: ReadBlock<'_>) -> Result<Node, BtrfsError> {
        let data = self.block(logical, read)?;
        if self.dirty.contains_key(&logical) {
            crate::node::parse_node_in_flight(data, logical, self.sectorsize)
        } else {
            crate::node::parse_node(data, logical, &self.fsid, self.sectorsize)
        }
    }

    /// Copies a block to a new address, or says it is ours. As
    /// `btrfs_force_cow_block` in `ctree.c`: allocate, copy, restamp. Fixing
    /// whoever pointed at the old one is the caller's job, as it may be a
    /// parent or a root
    pub fn cow_block(&mut self, logical: u64, read: ReadBlock<'_>) -> Result<u64, BtrfsError> {
        // Rule 2: already ours, changed in place
        if self.dirty.contains_key(&logical) {
            return Ok(logical);
        }

        let mut data = read(logical)?;
        if data.len() != self.nodesize as usize {
            return Err(BtrfsError::Corrupt);
        }
        let new = self.alloc.alloc(self.nodesize as u64)?;
        if new % self.sectorsize as u64 != 0 {
            return Err(BtrfsError::Corrupt);
        }
        self.check_fresh(new)?;

        // The copy is this transaction's: its address, this generation, not yet
        // written. The checksum is computed at commit, as later changes would
        // invalidate it
        data[header::BYTENR..header::BYTENR + 8].copy_from_slice(&new.to_le_bytes());
        data[header::GENERATION..header::GENERATION + 8]
            .copy_from_slice(&self.generation.to_le_bytes());
        let flags = u64_at(&data, header::FLAGS)?;
        let cleared = flags & !crate::node::HEADER_FLAG_WRITTEN;
        data[header::FLAGS..header::FLAGS + 8].copy_from_slice(&cleared.to_le_bytes());
        data[header::FSID..header::FSID + FSID_SIZE].copy_from_slice(&self.fsid);

        self.dirty.insert(new, data);
        // Rule 1: unreferenced by the new tree, still referenced by the old,
        // the crash fallback
        self.freed.insert(logical);
        self.alloc.free(logical, self.nodesize as u64);
        Ok(new)
    }

    /// Copies the path from a tree's root to the leaf for `key` and returns it.
    /// Descends like a search, copying each block and pointing the copied
    /// parent at the copied child. Every block returned is this transaction's
    pub fn cow_path(
        &mut self,
        root: u64,
        key: &Key,
        read: ReadBlock<'_>,
    ) -> Result<Path, BtrfsError> {
        let mut steps: Vec<Step> = Vec::new();

        // Starts from where this tree's root is now, not where it was. `root`
        // is the medium's address; after the first copy the root lives
        // elsewhere, and starting at the old address would copy it again and
        // leak the first copy (rule 2)
        let start = self.moved_roots.get(&root).copied().unwrap_or(root);
        let mut logical = self.cow_block(start, read)?;
        let new_root = logical;

        // One above the format's limit, so a tree pointing into itself is an
        // error
        for _ in 0..=MAX_LEVEL as usize {
            let node = self.node_at(logical, read)?;

            if node.is_leaf() {
                let slot = match node.search(key)? {
                    Ok(exact) => exact,
                    Err(next) => next,
                };
                steps.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
                steps.push(Step { logical, slot });
                self.set_root(root, new_root);
                return Ok(Path { steps });
            }

            // A node's key is the smallest under its pointer, so the subtree is
            // the one before the first greater key
            let slot = match node.search(key)? {
                Ok(exact) => exact,
                Err(0) => 0,
                Err(next) => next - 1,
            };
            let new_child = self.cow_child(logical, slot, read)?;

            steps.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
            steps.push(Step { logical, slot });
            logical = new_child;
        }
        Err(BtrfsError::Corrupt)
    }

    /// A newly allocated address must be held by nothing else. A block this
    /// transaction holds would give one buffer two names. A pinned block is
    /// rule 1. `BlockAlloc`'s doc states both; this checks them
    fn check_fresh(&self, new: u64) -> Result<(), BtrfsError> {
        if self.dirty.contains_key(&new) || self.freed.contains(&new) {
            return Err(BtrfsError::Corrupt);
        }
        Ok(())
    }

    /// Copies the child under `parent[slot]` and points the copied parent at
    /// it. Rule 3 lives here: every descent, `cow_path`'s and `shape.rs`'s,
    /// passes through. `parent` must be this transaction's. Returns the child's
    /// new address, or its old one if already ours
    pub fn cow_child(
        &mut self,
        parent: u64,
        slot: u32,
        read: ReadBlock<'_>,
    ) -> Result<u64, BtrfsError> {
        if !self.dirty.contains_key(&parent) {
            // A parent on the medium cannot be repointed
            return Err(BtrfsError::Corrupt);
        }
        let node = self.node_at(parent, read)?;
        let child = node.blockptr_at(slot)?;
        let new_child = self.cow_block(child, read)?;
        if new_child != child {
            let at = HEADER_SIZE + KEY_PTR_SIZE * slot as usize;
            let p = self.dirty.get_mut(&parent).ok_or(BtrfsError::Corrupt)?;
            p[at + key_ptr::BLOCKPTR..at + key_ptr::BLOCKPTR + 8]
                .copy_from_slice(&new_child.to_le_bytes());
            p[at + key_ptr::GENERATION..at + key_ptr::GENERATION + 8]
                .copy_from_slice(&self.generation.to_le_bytes());
        }
        Ok(new_child)
    }

    /// A fresh, empty tree block of this transaction. `like` is a sibling or
    /// parent. fsid, chunk tree uuid, and owner are copied from it, as they
    /// identify the tree joined. `btrfs check` sees a wrong owner. `WRITTEN` is
    /// clear with no checksum; `seal` sets both last
    pub fn new_block(
        &mut self,
        level: u8,
        like: u64,
        read: ReadBlock<'_>,
    ) -> Result<u64, BtrfsError> {
        if level >= MAX_LEVEL {
            return Err(BtrfsError::Corrupt);
        }
        let template = self.block(like, read)?;
        if template.len() != self.nodesize as usize {
            return Err(BtrfsError::Corrupt);
        }
        let new = self.alloc.alloc(self.nodesize as u64)?;
        if new % self.sectorsize as u64 != 0 {
            return Err(BtrfsError::Corrupt);
        }
        self.check_fresh(new)?;

        let mut data = alloc::vec![0u8; self.nodesize as usize];
        data[header::FSID..header::FSID + FSID_SIZE].copy_from_slice(&self.fsid);
        data[header::CHUNK_TREE_UUID..header::CHUNK_TREE_UUID + UUID_SIZE].copy_from_slice(
            &template[header::CHUNK_TREE_UUID..header::CHUNK_TREE_UUID + UUID_SIZE],
        );
        data[header::OWNER..header::OWNER + 8]
            .copy_from_slice(&template[header::OWNER..header::OWNER + 8]);
        data[header::BYTENR..header::BYTENR + 8].copy_from_slice(&new.to_le_bytes());
        data[header::GENERATION..header::GENERATION + 8]
            .copy_from_slice(&self.generation.to_le_bytes());
        data[header::LEVEL] = level;
        // nritems and flags stay zero
        self.dirty.insert(new, data);
        Ok(new)
    }

    /// A block this transaction allocated and made unreachable. Unlike rule 1's
    /// pin for a block the old tree references, this one was never on the
    /// medium: this generation, `WRITTEN` clear. So it leaves the dirty set and
    /// returns to the allocator. `btrfs_free_tree_block` in `extent-tree.c`
    /// splits on `btrfs_header_flag(buf, WRITTEN)` likewise. The caller removes
    /// the pointer first
    pub fn discard_block(&mut self, logical: u64) -> Result<(), BtrfsError> {
        if self.dirty.remove(&logical).is_none() {
            // Not ours; freeing it would break rule 1
            return Err(BtrfsError::Corrupt);
        }
        self.alloc.free(logical, self.nodesize as u64);
        Ok(())
    }

    /// Bytes of a block this transaction owns, to change in place.
    /// `Err(Corrupt)` for one that is not ours
    pub fn block_mut(&mut self, logical: u64) -> Result<&mut [u8], BtrfsError> {
        self.dirty
            .get_mut(&logical)
            .map(|v| v.as_mut_slice())
            .ok_or(BtrfsError::Corrupt)
    }

    /// A copy of a block's bytes, from this transaction or the medium
    pub fn block_bytes(&self, logical: u64, read: ReadBlock<'_>) -> Result<Vec<u8>, BtrfsError> {
        self.block(logical, read)
    }

    /// Records where a tree's root is now, keyed by its address on the medium,
    /// the name `cow_path` starts from. Adding or dropping a level calls this
    /// again with the same key
    pub fn set_root(&mut self, old_root: u64, new_root: u64) {
        if old_root != new_root {
            self.moved_roots.insert(old_root, new_root);
        }
    }

    /// Where this tree's root moved to, if it did
    pub fn new_root_of(&self, old_root: u64) -> Option<u64> {
        self.moved_roots.get(&old_root).copied()
    }

    // Changing a leaf

    /// Replaces an item's bytes, keeping its size. No moves or key changes;
    /// only the path copy
    pub fn update_item(
        &mut self,
        root: u64,
        key: &Key,
        data: &[u8],
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        let path = self.cow_path(root, key, read)?;
        let leaf_step = path.leaf().ok_or(BtrfsError::Corrupt)?;

        let node = self.node_at(leaf_step.logical, read)?;
        if !node.is_leaf() {
            return Err(BtrfsError::Corrupt);
        }
        if leaf_step.slot >= node.nritems {
            return Err(BtrfsError::NotFound);
        }
        if node.key_at(leaf_step.slot)? != *key {
            return Err(BtrfsError::NotFound);
        }
        let size = node.item_size(leaf_step.slot)? as usize;
        if size != data.len() {
            // A size change moves items and may not fit: `insert`/`delete`'s
            // job
            return Err(BtrfsError::InvalidArg);
        }

        let at = HEADER_SIZE + ITEM_SIZE * leaf_step.slot as usize;
        let leaf = self
            .dirty
            .get_mut(&leaf_step.logical)
            .ok_or(BtrfsError::Corrupt)?;
        let off = u32_at(leaf, at + item::OFFSET)? as usize;
        let start = HEADER_SIZE + off;
        leaf.get_mut(start..start + size)
            .ok_or(BtrfsError::Corrupt)?
            .copy_from_slice(data);
        Ok(())
    }

    /// Finish: the blocks to write, in address order, each with its checksum
    /// and `WRITTEN` set. The checksum last, as it covers all after it;
    /// `WRITTEN` likewise, so a reader does not accept a block mid-edit
    pub fn seal(&mut self) -> Result<Vec<(u64, Vec<u8>)>, BtrfsError> {
        let mut out: Vec<(u64, Vec<u8>)> = Vec::new();
        out.try_reserve_exact(self.dirty.len())
            .map_err(|_| BtrfsError::OutOfMemory)?;

        for (logical, data) in self.dirty.iter_mut() {
            // The block must still name where it lives, or the reader refuses
            // it
            if u64_at(data, header::BYTENR)? != *logical {
                return Err(BtrfsError::Corrupt);
            }
            let flags = u64_at(data, header::FLAGS)?;
            let set = flags | crate::node::HEADER_FLAG_WRITTEN;
            data[header::FLAGS..header::FLAGS + 8].copy_from_slice(&set.to_le_bytes());

            let csum = fshost::crc32c::crc32c(0, &data[CSUM_SIZE..]);
            data[0..4].copy_from_slice(&csum.to_le_bytes());
            data[4..CSUM_SIZE].fill(0);

            out.push((*logical, data.clone()));
        }
        Ok(out)
    }

    /// Addresses no longer referenced. Free only after the commit is written
    /// (rule 1)
    pub fn freed_blocks(&self) -> impl Iterator<Item = u64> + '_ {
        self.freed.iter().copied()
    }
}
