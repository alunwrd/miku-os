// Changing a tree's shape: inserting and deleting items
//
// `trans.rs` changes an item's bytes in place. Here an item arrives or leaves:
// a leaf's item array and data shift, a full leaf is relieved into siblings or
// split, a tree out of room grows a level, and a root with one child loses one.
//
// A sibling module, as copy-on-write and item placement fail differently. All
// of it is `impl Trans`: one transaction, one dirty set, one freed set. This
// file allocates through `Trans::new_block`, copies through `Trans::cow_block`
// and `cow_child`, and cannot touch the private `dirty` or `freed`, so a
// writable block only comes from asking.
//
// Paths
//
// `trans::Path` is `(block, slot)` pairs, valid until something moves an item.
// So `Cursor` is built by one operation, mutated as it splits and merges, and
// dropped on return. `insert_item` and `delete_item` take a root and key and
// return nothing to hold, at the cost of a descent per operation.
//
// API
//
// Beside `update_item`:
//
//     insert_item(root, key, body)   the key must not be there
//     delete_item(root, key)         the key must be there
//
// A duplicate key is `InvalidArg`, as for a size `update_item` refuses, and a
// missing key `NotFound`. Both are decided before any copy, so a refused call
// leaves the transaction unchanged.
//
// Reference
//
// `fs/btrfs/ctree.c`: `setup_items_for_insert` and `btrfs_del_items` are the
// leaf arithmetic; `split_leaf`, `push_leaf_left`, and `push_leaf_right` the
// relief and split; `insert_new_root`, `insert_ptr`, `split_node`, and
// `btrfs_del_ptr` the level above; `fixup_low_keys` the rule that a node's key
// is the smallest beneath it.
//
// Splits happen on the way down: `btrfs_search_slot` with a positive `ins_len`
// calls `setup_nodes_for_search`, splitting a node within three pointers of
// full before descending, so a child can always split into its parent.
//
// Absent
//
// Item contents are not checked. `check_leaf_item` in `tree-checker.c` is the
// other half, for the owner of the key type. This file guarantees a
// structurally sound leaf.
//
// `balance_level`, merging internal nodes on a delete, is not implemented.
// Leaves merge (`try_merge_leaf`) and the root drops a level with one child, so
// the tree stays correct and bounded; a sparse internal node costs only a read

use crate::error::BtrfsError;
use crate::node::{header, item, key_ptr};
use crate::structs::*;
use crate::trans::{BlockAlloc, ReadBlock, Step, Trans};
use alloc::vec::Vec;

// Reading and writing a block's two arrays
//
// Byte operations on a block this transaction owns, bounds-checked. Not in
// `node.rs`, which judges blocks others may have written

fn nritems(b: &[u8]) -> Result<u32, BtrfsError> {
    u32_at(b, header::NRITEMS)
}

fn set_nritems(b: &mut [u8], n: u32) -> Result<(), BtrfsError> {
    b.get_mut(header::NRITEMS..header::NRITEMS + 4)
        .ok_or(BtrfsError::Corrupt)?
        .copy_from_slice(&n.to_le_bytes());
    Ok(())
}

fn level(b: &[u8]) -> Result<u8, BtrfsError> {
    u8_at(b, header::LEVEL)
}

fn set_level(b: &mut [u8], l: u8) -> Result<(), BtrfsError> {
    *b.get_mut(header::LEVEL).ok_or(BtrfsError::Corrupt)? = l;
    Ok(())
}

/// Everything after the header, where `item.offset` is measured from
fn cap(b: &[u8]) -> u32 {
    (b.len() - HEADER_SIZE) as u32
}

fn item_at(slot: u32) -> usize {
    HEADER_SIZE + ITEM_SIZE * slot as usize
}

fn ptr_at(slot: u32) -> usize {
    HEADER_SIZE + KEY_PTR_SIZE * slot as usize
}

fn item_off(b: &[u8], slot: u32) -> Result<u32, BtrfsError> {
    u32_at(b, item_at(slot) + item::OFFSET)
}

fn item_len(b: &[u8], slot: u32) -> Result<u32, BtrfsError> {
    u32_at(b, item_at(slot) + item::SIZE)
}

/// Where slot's data stops, from the header's end. Slot 0's is the whole
/// capacity, as data tiles backward from the block's end
fn item_end(b: &[u8], slot: u32) -> Result<u32, BtrfsError> {
    Ok(item_off(b, slot)? + item_len(b, slot)?)
}

fn set_item_off(b: &mut [u8], slot: u32, off: u32) -> Result<(), BtrfsError> {
    let at = item_at(slot) + item::OFFSET;
    b.get_mut(at..at + 4)
        .ok_or(BtrfsError::Corrupt)?
        .copy_from_slice(&off.to_le_bytes());
    Ok(())
}

fn set_item_len(b: &mut [u8], slot: u32, size: u32) -> Result<(), BtrfsError> {
    let at = item_at(slot) + item::SIZE;
    b.get_mut(at..at + 4)
        .ok_or(BtrfsError::Corrupt)?
        .copy_from_slice(&size.to_le_bytes());
    Ok(())
}

fn key_of(b: &[u8], slot: u32, is_leaf: bool) -> Result<Key, BtrfsError> {
    Key::read(b, if is_leaf { item_at(slot) } else { ptr_at(slot) })
}

fn set_key(b: &mut [u8], slot: u32, is_leaf: bool, k: &Key) -> Result<(), BtrfsError> {
    k.write(b, if is_leaf { item_at(slot) } else { ptr_at(slot) })
}

fn set_blockptr(b: &mut [u8], slot: u32, bytenr: u64, generation: u64) -> Result<(), BtrfsError> {
    let at = ptr_at(slot);
    b.get_mut(at + key_ptr::BLOCKPTR..at + key_ptr::BLOCKPTR + 8)
        .ok_or(BtrfsError::Corrupt)?
        .copy_from_slice(&bytenr.to_le_bytes());
    b.get_mut(at + key_ptr::GENERATION..at + key_ptr::GENERATION + 8)
        .ok_or(BtrfsError::Corrupt)?
        .copy_from_slice(&generation.to_le_bytes());
    Ok(())
}

/// Where a leaf's data region starts. An empty leaf's is the whole capacity
fn data_start(b: &[u8]) -> Result<u32, BtrfsError> {
    let n = nritems(b)?;
    if n == 0 {
        Ok(cap(b))
    } else {
        item_off(b, n - 1)
    }
}

/// Bytes the items `[from, to)` occupy in both arrays: `leaf_space_used` in
/// `ctree.c`
fn space_used(b: &[u8], from: u32, to: u32) -> Result<u32, BtrfsError> {
    if from >= to {
        return Ok(0);
    }
    let data = item_end(b, from)? - item_off(b, to - 1)?;
    Ok(data + ITEM_SIZE as u32 * (to - from))
}

/// What is left between the item array and the data: `btrfs_leaf_free_space`
fn free_space(b: &[u8]) -> Result<u32, BtrfsError> {
    let n = nritems(b)?;
    let used = space_used(b, 0, n)?;
    cap(b).checked_sub(used).ok_or(BtrfsError::Corrupt)
}

/// Pointers an internal node of this size holds: `BTRFS_NODEPTRS_PER_BLOCK`
fn max_ptrs(nodesize: u32) -> u32 {
    (nodesize - HEADER_SIZE as u32) / KEY_PTR_SIZE as u32
}

/// Where one operation is while changing the tree. Born in `descend`, mutated
/// by each split and merge, dropped on return; see the note at the top
struct Cursor {
    /// The tree's address on the medium, the stable name `Trans` keys moved
    /// roots by
    root: u64,
    /// Where the root block is now in this transaction
    top: u64,
    /// Internal nodes from the root to the leaf's parent. Empty when the tree
    /// is one leaf
    path: Vec<Step>,
    leaf: u64,
    /// Where the key goes, or is, in `leaf`
    slot: u32,
}

impl Cursor {
    fn parent(&self) -> Option<Step> {
        self.path.last().copied()
    }
}

fn push_step(path: &mut Vec<Step>, s: Step) -> Result<(), BtrfsError> {
    path.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
    path.push(s);
    Ok(())
}

impl<A: BlockAlloc> Trans<A> {
    // The two verbs

    /// Puts `body` in the tree under `key`. `InvalidArg` if the key exists, or
    /// the item is larger than a leaf holds. Both are decided before any copy.
    /// `btrfs_insert_item`, by way of `btrfs_insert_empty_items`
    pub fn insert_item(
        &mut self,
        root: u64,
        key: &Key,
        body: &[u8],
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        let need = ITEM_SIZE as u32 + body.len() as u32;
        // An item and its array entry must fit in one leaf
        if need > self.nodesize - HEADER_SIZE as u32 {
            return Err(BtrfsError::InvalidArg);
        }
        if self.seek(root, key, read)? {
            return Err(BtrfsError::InvalidArg);
        }

        let mut c = self.descend(root, key, true, read)?;
        self.make_room(&mut c, key, need, read)?;

        // The leaf has room. The rest is arithmetic in one block, plus the
        // parent's key if the first item changed
        let slot = c.slot;
        let leaf = self.block_mut(c.leaf)?;
        insert_into_leaf(leaf, slot, key, body)?;

        if slot == 0 {
            // A node's key is the smallest beneath it. An old first key sends a
            // later search into the wrong subtree: `fixup_low_keys`
            let depth = c.path.len();
            self.fixup_low_keys(&c.path, depth, key)?;
        }
        self.set_root(c.root, c.top);
        Ok(())
    }

    /// Takes `key` out of the tree. `NotFound` if absent, decided before any
    /// copy. `btrfs_del_items`
    pub fn delete_item(
        &mut self,
        root: u64,
        key: &Key,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        if !self.seek(root, key, read)? {
            return Err(BtrfsError::NotFound);
        }

        let mut c = self.descend(root, key, false, read)?;
        let slot = c.slot;
        {
            let leaf = self.block_mut(c.leaf)?;
            // `seek` said it was there; otherwise the descent and seek disagree
            // about one tree
            if slot >= nritems(leaf)? || key_of(leaf, slot, true)? != *key {
                return Err(BtrfsError::Corrupt);
            }
            delete_from_leaf(leaf, slot)?;
        }

        let left = nritems(self.block_mut(c.leaf)?)?;
        if left == 0 {
            // An empty leaf that is the whole tree is legal, so only a leaf
            // with a parent is removed
            if c.parent().is_some() {
                self.drop_leaf(&mut c)?;
            }
        } else {
            if slot == 0 {
                let k = key_of(self.block_mut(c.leaf)?, 0, true)?;
                let depth = c.path.len();
                self.fixup_low_keys(&c.path, depth, &k)?;
            }
            self.try_merge_leaf(&mut c, read)?;
        }

        self.collapse_root(&mut c, read)?;
        self.set_root(c.root, c.top);
        Ok(())
    }

    // Finding things

    /// Whether `key` is in the tree. Reads only, so `insert_item` and
    /// `delete_item` decide before changing anything
    fn seek(&self, root: u64, key: &Key, read: ReadBlock<'_>) -> Result<bool, BtrfsError> {
        let mut logical = self.new_root_of(root).unwrap_or(root);
        for _ in 0..=MAX_LEVEL as usize {
            let node = self.node_at(logical, read)?;
            if node.is_leaf() {
                return Ok(node.search(key)?.is_ok());
            }
            let slot = match node.search(key)? {
                Ok(exact) => exact,
                Err(0) => return Ok(false),
                Err(next) => next - 1,
            };
            logical = node.blockptr_at(slot)?;
        }
        Err(BtrfsError::Corrupt)
    }

    /// Copies the path to `key`'s leaf, splitting on the way down when
    /// `for_insert`. `setup_nodes_for_search`: a node within three pointers of
    /// full splits before descent, so its child can split into room. The margin
    /// is the kernel's
    fn descend(
        &mut self,
        root: u64,
        key: &Key,
        for_insert: bool,
        read: ReadBlock<'_>,
    ) -> Result<Cursor, BtrfsError> {
        let start = self.new_root_of(root).unwrap_or(root);
        let top = self.cow_block(start, read)?;
        let mut c = Cursor { root, top, path: Vec::new(), leaf: top, slot: 0 };

        // Two above the format's limit: one for the loop's bound, one as
        // growing a root adds a level
        for _ in 0..MAX_LEVEL as usize + 2 {
            let node = self.node_at(c.leaf, read)?;
            if node.is_leaf() {
                c.slot = match node.search(key)? {
                    Ok(exact) => exact,
                    Err(next) => next,
                };
                // Recorded here and at the end: a failed operation must still
                // name the root it copied
                self.set_root(c.root, c.top);
                return Ok(c);
            }

            if for_insert && node.nritems + 3 >= max_ptrs(self.nodesize) {
                if c.path.is_empty() {
                    self.grow_root(&mut c, key, read)?;
                }
                self.split_node(&mut c, key, read)?;
            }

            let node = self.node_at(c.leaf, read)?;
            // A node's key is the smallest under its pointer, so the subtree is
            // the one before the first greater key
            let slot = match node.search(key)? {
                Ok(exact) => exact,
                Err(0) => 0,
                Err(next) => next - 1,
            };
            let child = self.cow_child(c.leaf, slot, read)?;
            push_step(&mut c.path, Step { logical: c.leaf, slot })?;
            c.leaf = child;
        }
        Err(BtrfsError::Corrupt)
    }

    /// A change to slot 0 of a block goes to whoever points at it, and if that
    /// pointer is at slot 0, onward, stopping at the first non-slot-0 pointer.
    /// `fixup_low_keys`
    fn fixup_low_keys(
        &mut self,
        path: &[Step],
        mut depth: usize,
        key: &Key,
    ) -> Result<(), BtrfsError> {
        while depth > 0 {
            depth -= 1;
            let s = path[depth];
            let b = self.block_mut(s.logical)?;
            set_key(b, s.slot, false, key)?;
            if s.slot != 0 {
                break;
            }
        }
        Ok(())
    }

    // The level above

    /// Adds a level: a new root with one pointer at the old root.
    /// `insert_new_root`, when a split has nowhere above for its second half
    fn grow_root(
        &mut self,
        c: &mut Cursor,
        key: &Key,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        if !c.path.is_empty() {
            return Err(BtrfsError::Corrupt);
        }
        let lower = c.leaf;
        let (lower_level, lower_key) = {
            let b = self.block_bytes(lower, read)?;
            let l = level(&b)?;
            // An empty block has no first key. The only empty block is a root
            // leaf, and the incoming key will be the only one under it
            let k = if nritems(&b)? == 0 {
                *key
            } else {
                key_of(&b, 0, l == 0)?
            };
            (l, k)
        };
        if lower_level as usize + 1 >= MAX_LEVEL as usize {
            return Err(BtrfsError::Corrupt);
        }

        let new = self.new_block(lower_level + 1, lower, read)?;
        let generation = self.generation;
        let b = self.block_mut(new)?;
        set_nritems(b, 1)?;
        set_key(b, 0, false, &lower_key)?;
        set_blockptr(b, 0, lower, generation)?;

        c.top = new;
        push_step(&mut c.path, Step { logical: new, slot: 0 })?;
        Ok(())
    }

    /// Splits the cursor's node and tells its parent: `split_node`. The parent
    /// has room since the descent split it first. The cursor ends on the half
    /// holding `key`
    fn split_node(
        &mut self,
        c: &mut Cursor,
        key: &Key,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        let node = c.leaf;
        let parent = c.parent().ok_or(BtrfsError::Corrupt)?;

        let (lvl, n) = {
            let b = self.block_bytes(node, read)?;
            (level(&b)?, nritems(&b)?)
        };
        if lvl == 0 || n < 2 {
            return Err(BtrfsError::Corrupt);
        }
        let mid = (n + 1) / 2;

        let right = self.new_block(lvl, node, read)?;
        let src = self.block_bytes(node, read)?;
        {
            let dst = self.block_mut(right)?;
            let from = ptr_at(mid);
            let to = ptr_at(n);
            dst[HEADER_SIZE..HEADER_SIZE + (to - from)].copy_from_slice(&src[from..to]);
            set_nritems(dst, n - mid)?;
        }
        set_nritems(self.block_mut(node)?, mid)?;

        let split_key = key_of(&src, mid, false)?;
        self.insert_ptr(parent.logical, parent.slot + 1, &split_key, right)?;

        // The path's slot is the pointer to this node, which may now be the new
        // half
        if *key >= split_key {
            c.leaf = right;
            let last = c.path.len() - 1;
            c.path[last].slot = parent.slot + 1;
        }
        Ok(())
    }

    /// Puts a pointer into a node at `at`, shifting the rest up: `insert_ptr`.
    /// The descent guarantees room
    fn insert_ptr(
        &mut self,
        node: u64,
        at: u32,
        key: &Key,
        bytenr: u64,
    ) -> Result<(), BtrfsError> {
        let generation = self.generation;
        let cap_ptrs = max_ptrs(self.nodesize);
        let b = self.block_mut(node)?;
        let n = nritems(b)?;
        if at > n || n >= cap_ptrs {
            return Err(BtrfsError::Corrupt);
        }
        if at < n {
            b.copy_within(ptr_at(at)..ptr_at(n), ptr_at(at + 1));
        }
        set_key(b, at, false, key)?;
        set_blockptr(b, at, bytenr, generation)?;
        set_nritems(b, n + 1)?;
        Ok(())
    }

    /// Takes the pointer at `path[depth]` out of its node, continuing up while
    /// that empties a node. `btrfs_del_ptr`, plus the loop `balance_level`
    /// would drive. A node losing its last pointer is freed with its own
    /// pointer removed; an emptied root becomes an empty leaf
    fn drop_ptr(&mut self, c: &mut Cursor, mut depth: usize) -> Result<bool, BtrfsError> {
        // The block whose last pointer went, returned only after the pointer to
        // it goes, so the address is not reused while named
        let mut doomed: Option<u64> = None;
        loop {
            let s = c.path[depth];
            let n = {
                let b = self.block_mut(s.logical)?;
                let n = nritems(b)?;
                if s.slot >= n {
                    return Err(BtrfsError::Corrupt);
                }
                if s.slot + 1 < n {
                    b.copy_within(ptr_at(s.slot + 1)..ptr_at(n), ptr_at(s.slot));
                }
                set_nritems(b, n - 1)?;
                n - 1
            };
            if let Some(d) = doomed.take() {
                self.discard_block(d)?;
            }

            if n == 0 {
                if depth == 0 {
                    // Nothing left under the root. It becomes an empty leaf, so
                    // the root's address stays valid
                    let b = self.block_mut(s.logical)?;
                    set_level(b, 0)?;
                    c.path.clear();
                    c.leaf = s.logical;
                    c.slot = 0;
                    c.top = s.logical;
                    return Ok(true);
                }
                doomed = Some(s.logical);
                depth -= 1;
                continue;
            }

            if s.slot == 0 {
                let k = {
                    let b = self.block_mut(s.logical)?;
                    key_of(b, 0, false)?
                };
                self.fixup_low_keys(&c.path, depth, &k)?;
            }
            c.path.truncate(depth + 1);
            return Ok(false);
        }
    }

    /// While the root is a node with one child, makes the child root:
    /// `promote_child_to_root`, from `balance_level` with no parent and one
    /// pointer
    fn collapse_root(&mut self, c: &mut Cursor, read: ReadBlock<'_>) -> Result<(), BtrfsError> {
        for _ in 0..MAX_LEVEL as usize + 1 {
            let (lvl, n) = {
                let b = self.block_bytes(c.top, read)?;
                (level(&b)?, nritems(&b)?)
            };
            if lvl == 0 || n != 1 {
                return Ok(());
            }
            // The child may be on the medium, not the one descended through, so
            // it is copied
            let child = self.cow_child(c.top, 0, read)?;
            let old = c.top;
            c.top = child;
            c.path.clear();
            self.discard_block(old)?;
        }
        Err(BtrfsError::Corrupt)
    }

    // Making leaf room

    /// Leaves the cursor on a leaf with `need` bytes free, at `key`'s slot. In
    /// `split_leaf`'s order: relieve into the right sibling, then the left,
    /// then split
    fn make_room(
        &mut self,
        c: &mut Cursor,
        key: &Key,
        need: u32,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        if free_space(self.block_mut(c.leaf)?)? >= need {
            return Ok(());
        }
        // Everything below needs somewhere to put a second leaf
        if c.path.is_empty() {
            self.grow_root(c, key, read)?;
        }

        self.push_right(c, key, need, read)?;
        if free_space(self.block_mut(c.leaf)?)? >= need {
            return Ok(());
        }
        self.push_left(c, key, need, read)?;
        if free_space(self.block_mut(c.leaf)?)? >= need {
            return Ok(());
        }

        // At most two splits; the second is the kernel's `num_doubles`, for an
        // item too big for either half of an even split
        for _ in 0..2 {
            if self.split_leaf(c, key, need, read)? {
                return Ok(());
            }
        }
        Err(BtrfsError::Corrupt)
    }

    /// Moves items off the leaf's end into its right sibling:
    /// `push_leaf_right`. Only items at or after the insertion slot move, so
    /// the slot stays in this leaf. No fast path moving the insertion into the
    /// sibling
    fn push_right(
        &mut self,
        c: &mut Cursor,
        key: &Key,
        need: u32,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        let parent = c.parent().ok_or(BtrfsError::Corrupt)?;
        let pn = nritems(self.block_mut(parent.logical)?)?;
        if parent.slot + 1 >= pn {
            return Ok(());
        }
        // The sibling is copied first; writing a block still on the medium
        // would change what a crash falls back to (rule 1)
        let right = self.cow_child(parent.logical, parent.slot + 1, read)?;

        let count = {
            let leaf = self.block_bytes(c.leaf, read)?;
            let rb = self.block_bytes(right, read)?;
            let mut room = free_space(&rb)?;
            let mut have = free_space(&leaf)?;
            let n = nritems(&leaf)?;
            let mut count = 0u32;
            let mut i = n;
            while i > c.slot && have < need {
                let cost = ITEM_SIZE as u32 + item_len(&leaf, i - 1)?;
                if cost > room {
                    break;
                }
                room -= cost;
                have += cost;
                count += 1;
                i -= 1;
            }
            count
        };
        if count == 0 {
            return Ok(());
        }

        self.move_items(c.leaf, right, count, false)?;

        // Both siblings' first keys may have moved
        let rk = key_of(&self.block_bytes(right, read)?, 0, true)?;
        set_key(self.block_mut(parent.logical)?, parent.slot + 1, false, &rk)?;
        let lk = {
            let leaf = self.block_bytes(c.leaf, read)?;
            if nritems(&leaf)? == 0 {
                // Everything moved right. The leaf's only key will be the new
                // one, set now to keep the parent's keys in order
                *key
            } else {
                key_of(&leaf, 0, true)?
            }
        };
        let depth = c.path.len();
        self.fixup_low_keys(&c.path, depth, &lk)?;
        Ok(())
    }

    /// Moves items off the leaf's front into its left sibling:
    /// `push_leaf_left`. Only items before the insertion slot move, and the
    /// slot comes down by that many
    fn push_left(
        &mut self,
        c: &mut Cursor,
        key: &Key,
        need: u32,
        read: ReadBlock<'_>,
    ) -> Result<(), BtrfsError> {
        let parent = c.parent().ok_or(BtrfsError::Corrupt)?;
        if parent.slot == 0 {
            return Ok(());
        }
        let left = self.cow_child(parent.logical, parent.slot - 1, read)?;

        let count = {
            let leaf = self.block_bytes(c.leaf, read)?;
            let lb = self.block_bytes(left, read)?;
            let mut room = free_space(&lb)?;
            let mut have = free_space(&leaf)?;
            let mut count = 0u32;
            while count < c.slot && have < need {
                let cost = ITEM_SIZE as u32 + item_len(&leaf, count)?;
                if cost > room {
                    break;
                }
                room -= cost;
                have += cost;
                count += 1;
            }
            count
        };
        if count == 0 {
            return Ok(());
        }

        self.move_items(c.leaf, left, count, true)?;
        c.slot -= count;

        let lk = {
            let leaf = self.block_bytes(c.leaf, read)?;
            if nritems(&leaf)? == 0 {
                *key
            } else {
                key_of(&leaf, 0, true)?
            }
        };
        let depth = c.path.len();
        self.fixup_low_keys(&c.path, depth, &lk)?;
        Ok(())
    }

    /// Splits the leaf and tells the parent of the new half. Returns whether
    /// the cursor's leaf has `need` free. `false` means the item fits neither
    /// half, `split_leaf`'s double split: the caller comes round once more,
    /// landing on a boundary.
    ///
    /// `mid` is chosen as in `split_leaf`: half, moved toward the insertion
    /// slot until the receiving half has room. With no such `mid` the item gets
    /// its own leaf: the kernel's `split == 0`
    fn split_leaf(
        &mut self,
        c: &mut Cursor,
        key: &Key,
        need: u32,
        read: ReadBlock<'_>,
    ) -> Result<bool, BtrfsError> {
        let parent = c.parent().ok_or(BtrfsError::Corrupt)?;
        let leaf = self.block_bytes(c.leaf, read)?;
        let n = nritems(&leaf)?;
        let capacity = cap(&leaf);
        let slot = c.slot;

        let mut mid = (n + 1) / 2;
        let mut fresh_leaf = false;
        let mut double = false;

        if mid <= slot {
            // The new item lands in the right half
            if n == 1 || space_used(&leaf, mid, n)? + need > capacity {
                if slot >= n {
                    fresh_leaf = true;
                } else {
                    mid = slot;
                    if space_used(&leaf, mid, n)? + need > capacity {
                        double = true;
                    }
                }
            }
        } else {
            // The new item lands in the left half
            if space_used(&leaf, 0, mid)? + need > capacity {
                if slot == 0 {
                    fresh_leaf = true;
                } else {
                    mid = slot;
                    if space_used(&leaf, mid, n)? + need > capacity {
                        double = true;
                    }
                }
            }
        }

        if fresh_leaf {
            // A leaf of its own, before or after this one by which end the key
            // belongs at; the key is outside every key here
            let fresh = self.new_block(0, c.leaf, read)?;
            let last = c.path.len() - 1;
            if slot >= n {
                self.insert_ptr(parent.logical, parent.slot + 1, key, fresh)?;
                c.path[last].slot = parent.slot + 1;
            } else {
                self.insert_ptr(parent.logical, parent.slot, key, fresh)?;
                // The old leaf moved up a slot and the new took its place; at
                // slot 0 the node's key changed
                if parent.slot == 0 {
                    self.fixup_low_keys(&c.path, c.path.len(), key)?;
                }
            }
            c.leaf = fresh;
            c.slot = 0;
            return Ok(true);
        }

        let right = self.new_block(0, c.leaf, read)?;
        self.move_items(c.leaf, right, n - mid, false)?;
        let split_key = key_of(&self.block_bytes(right, read)?, 0, true)?;
        self.insert_ptr(parent.logical, parent.slot + 1, &split_key, right)?;

        if mid <= slot {
            c.leaf = right;
            c.slot = slot - mid;
            let last = c.path.len() - 1;
            c.path[last].slot = parent.slot + 1;
        }

        if double {
            // Neither half had room, so nothing was placed. The cursor is at an
            // end of one half, and the next pass finishes
            return Ok(false);
        }
        Ok(free_space(self.block_mut(c.leaf)?)? >= need)
    }

    // Giving leaf room back

    /// A mostly empty leaf is pushed into its siblings, and removed if empty.
    /// `btrfs_del_items`' tail: a third full is the kernel's threshold. Left
    /// first and partially, then right all-or-nothing, as the goal is removing
    /// the block
    fn try_merge_leaf(&mut self, c: &mut Cursor, read: ReadBlock<'_>) -> Result<(), BtrfsError> {
        if c.parent().is_none() {
            return Ok(());
        }
        let (used, capacity) = {
            let b = self.block_bytes(c.leaf, read)?;
            (space_used(&b, 0, nritems(&b)?)?, cap(&b))
        };
        if used >= capacity / 3 {
            return Ok(());
        }

        self.drain_left(c, read)?;
        if nritems(self.block_mut(c.leaf)?)? > 0 {
            self.drain_right(c, read)?;
        }
        if nritems(self.block_mut(c.leaf)?)? == 0 {
            self.drop_leaf(c)?;
        }
        Ok(())
    }

    /// Push as many items as fit into the left sibling, from the front
    fn drain_left(&mut self, c: &mut Cursor, read: ReadBlock<'_>) -> Result<(), BtrfsError> {
        let parent = c.parent().ok_or(BtrfsError::Corrupt)?;
        if parent.slot == 0 {
            return Ok(());
        }
        let left = self.cow_child(parent.logical, parent.slot - 1, read)?;

        let count = {
            let leaf = self.block_bytes(c.leaf, read)?;
            let lb = self.block_bytes(left, read)?;
            let mut room = free_space(&lb)?;
            let n = nritems(&leaf)?;
            let mut count = 0u32;
            while count < n {
                let cost = ITEM_SIZE as u32 + item_len(&leaf, count)?;
                if cost > room {
                    break;
                }
                room -= cost;
                count += 1;
            }
            count
        };
        if count == 0 {
            return Ok(());
        }

        self.move_items(c.leaf, left, count, true)?;
        if nritems(self.block_mut(c.leaf)?)? > 0 {
            let k = key_of(&self.block_bytes(c.leaf, read)?, 0, true)?;
            let depth = c.path.len();
            self.fixup_low_keys(&c.path, depth, &k)?;
        }
        Ok(())
    }

    /// Push every remaining item into the right sibling, or none of them
    fn drain_right(&mut self, c: &mut Cursor, read: ReadBlock<'_>) -> Result<(), BtrfsError> {
        let parent = c.parent().ok_or(BtrfsError::Corrupt)?;
        let pn = nritems(self.block_mut(parent.logical)?)?;
        if parent.slot + 1 >= pn {
            return Ok(());
        }
        let right = self.cow_child(parent.logical, parent.slot + 1, read)?;

        let n = {
            let leaf = self.block_bytes(c.leaf, read)?;
            let rb = self.block_bytes(right, read)?;
            let n = nritems(&leaf)?;
            if space_used(&leaf, 0, n)? > free_space(&rb)? {
                return Ok(());
            }
            n
        };
        if n == 0 {
            return Ok(());
        }

        self.move_items(c.leaf, right, n, false)?;
        let rk = key_of(&self.block_bytes(right, read)?, 0, true)?;
        set_key(self.block_mut(parent.logical)?, parent.slot + 1, false, &rk)?;
        Ok(())
    }

    /// Unhooks an empty leaf and returns its address: `btrfs_del_leaf`. The
    /// pointer goes first, per `discard_block`, so the address is not reused
    /// while named
    fn drop_leaf(&mut self, c: &mut Cursor) -> Result<(), BtrfsError> {
        let depth = c.path.len();
        if depth == 0 {
            return Err(BtrfsError::Corrupt);
        }
        let gone = c.leaf;
        let root_is_leaf = self.drop_ptr(c, depth - 1)?;
        // The pointer is gone, so the address can go back
        self.discard_block(gone)?;
        if !root_is_leaf {
            // The cursor's leaf is gone. `delete_item` goes straight to
            // `collapse_root`, and no dangling address stays in a live cursor
            c.leaf = match c.path.last() {
                Some(s) => s.logical,
                None => c.top,
            };
            c.slot = 0;
        }
        Ok(())
    }

    // Moving item runs

    /// Moves `count` items between two leaves of this transaction. `front`
    /// picks the end: `true` from the front of `src` to the end of `dst`,
    /// `false` from the end of `src` to the front of `dst`. One function, as
    /// both must rebase each offset in the destination: `item.offset` is
    /// measured from the block's end. The caller checked the fit; this refuses
    /// an overlap
    fn move_items(
        &mut self,
        src: u64,
        dst: u64,
        count: u32,
        front: bool,
    ) -> Result<(), BtrfsError> {
        if count == 0 {
            return Ok(());
        }
        let s = self.block_mut(src)?.to_vec();
        let sn = nritems(&s)?;
        if count > sn {
            return Err(BtrfsError::Corrupt);
        }
        let capacity = cap(&s);

        // The items and their data bytes, contiguous as a leaf's data tiles
        // exactly
        let (first, last) = if front { (0, count - 1) } else { (sn - count, sn - 1) };
        let data_from = item_off(&s, last)?;
        let data_to = item_end(&s, first)?;
        let data_len = data_to - data_from;
        let items_from = item_at(first);
        let items_to = item_at(last + 1);

        {
            let (dn, d_start) = {
                let d = self.block_mut(dst)?;
                (nritems(d)?, data_start(d)?)
            };
            // The item array and data region grow toward each other; this stops
            // them meeting
            let new_start = d_start.checked_sub(data_len).ok_or(BtrfsError::Corrupt)?;
            if ITEM_SIZE as u32 * (dn + count) > new_start {
                return Err(BtrfsError::Corrupt);
            }
            let d = self.block_mut(dst)?;

            if front {
                // Appending: new items after `dn`, their data below what is
                // there
                d[HEADER_SIZE + new_start as usize..HEADER_SIZE + d_start as usize]
                    .copy_from_slice(&s[HEADER_SIZE + data_from as usize..HEADER_SIZE + data_to as usize]);
                let at = item_at(dn);
                d[at..at + (items_to - items_from)].copy_from_slice(&s[items_from..items_to]);
                set_nritems(d, dn + count)?;
            } else {
                // Prepending: what is in `dst` moves down to free the top of
                // the data region, and the item array shifts up by `count`
                d.copy_within(
                    HEADER_SIZE + d_start as usize..HEADER_SIZE + capacity as usize,
                    HEADER_SIZE + new_start as usize,
                );
                d[HEADER_SIZE + (capacity - data_len) as usize..HEADER_SIZE + capacity as usize]
                    .copy_from_slice(&s[HEADER_SIZE + data_from as usize..HEADER_SIZE + data_to as usize]);
                d.copy_within(HEADER_SIZE..item_at(dn), item_at(count));
                d[HEADER_SIZE..HEADER_SIZE + (items_to - items_from)]
                    .copy_from_slice(&s[items_from..items_to]);
                set_nritems(d, dn + count)?;
            }

            // Offsets that came across were measured against another block's
            // end. Recomputed from the back in one pass
            let total = dn + count;
            let mut off = capacity;
            for i in 0..total {
                off -= item_len(d, i)?;
                set_item_off(d, i, off)?;
            }
        }

        {
            let d = self.block_mut(src)?;
            let left = sn - count;
            if left == 0 {
                set_nritems(d, 0)?;
            } else if front {
                // Survivors are at the item array's back and the data's top;
                // both slide to the front
                let keep_end = item_off(&s, count - 1)?;
                let keep_start = item_off(&s, sn - 1)?;
                let keep_len = keep_end - keep_start;
                d.copy_within(
                    HEADER_SIZE + keep_start as usize..HEADER_SIZE + keep_end as usize,
                    HEADER_SIZE + (capacity - keep_len) as usize,
                );
                d.copy_within(item_at(count)..item_at(sn), HEADER_SIZE);
                set_nritems(d, left)?;
                let mut off = capacity;
                for i in 0..left {
                    off -= item_len(d, i)?;
                    set_item_off(d, i, off)?;
                }
            } else {
                // Survivors keep slots and offsets: the departing run was at
                // the data's low end, so the rest still tiles to the block's
                // end
                set_nritems(d, left)?;
            }
        }
        Ok(())
    }
}

// Arithmetic inside one leaf

/// Puts an item at `slot`, moving the item array forward and data back:
/// `setup_items_for_insert` with one item. Order: offsets of shifting items are
/// corrected in place, then the array shifts, then the data
fn insert_into_leaf(
    leaf: &mut [u8],
    slot: u32,
    key: &Key,
    body: &[u8],
) -> Result<(), BtrfsError> {
    let n = nritems(leaf)?;
    if slot > n {
        return Err(BtrfsError::Corrupt);
    }
    let size = body.len() as u32;
    if free_space(leaf)? < ITEM_SIZE as u32 + size {
        return Err(BtrfsError::Corrupt);
    }

    let mut end = data_start(leaf)?;
    if slot != n {
        let old_data = item_end(leaf, slot)?;
        if old_data < end {
            return Err(BtrfsError::Corrupt);
        }
        for i in slot..n {
            let off = item_off(leaf, i)?;
            set_item_off(leaf, i, off - size)?;
        }
        leaf.copy_within(item_at(slot)..item_at(n), item_at(slot + 1));
        leaf.copy_within(
            HEADER_SIZE + end as usize..HEADER_SIZE + old_data as usize,
            HEADER_SIZE + (end - size) as usize,
        );
        end = old_data;
    }

    set_key(leaf, slot, true, key)?;
    end -= size;
    set_item_off(leaf, slot, end)?;
    set_item_len(leaf, slot, size)?;
    leaf[HEADER_SIZE + end as usize..HEADER_SIZE + end as usize + body.len()]
        .copy_from_slice(body);
    set_nritems(leaf, n + 1)?;
    Ok(())
}

/// Takes the item at `slot` out, closing the gap in both arrays:
/// `btrfs_del_items` with one item. A hole left in the data region fails
/// `check_leaf` and the kernel's tree checker
fn delete_from_leaf(leaf: &mut [u8], slot: u32) -> Result<(), BtrfsError> {
    let n = nritems(leaf)?;
    if slot >= n {
        return Err(BtrfsError::Corrupt);
    }
    if slot + 1 != n {
        let last_off = item_off(leaf, slot)?;
        let end = data_start(leaf)?;
        let dsize = item_len(leaf, slot)?;

        leaf.copy_within(
            HEADER_SIZE + end as usize..HEADER_SIZE + last_off as usize,
            HEADER_SIZE + (end + dsize) as usize,
        );
        for i in slot + 1..n {
            let off = item_off(leaf, i)?;
            set_item_off(leaf, i, off + dsize)?;
        }
        leaf.copy_within(item_at(slot + 1)..item_at(n), item_at(slot));
    }
    set_nritems(leaf, n - 1)?;
    Ok(())
}
