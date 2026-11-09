//! The directory index (htree)
//!
//! An indexed directory keeps a small B-tree of name hashes in its first
//! blocks. Block 0 is the root: two fake entries (`.` and `..`) covering the
//! whole block, so a reader unaware of the index sees an ordinary empty
//! directory block, followed by an array of `{hash, block}` pairs. Interior
//! nodes have the same array behind one eight-byte fake entry. Leaves are
//! ordinary directory blocks.
//!
//! An insert must follow the index: Linux's `ext4_find_entry` asks for exactly
//! one leaf and returns "no such file" on a miss, and `e2fsck` checks hash
//! ranges.
//!
//! The code has the shape of the extent tree in `ext4/extents.rs`: descend
//! recording the path, try the leaf, push a split up through the path when
//! full, then retry from the top.

use alloc::vec;
use alloc::vec::Vec;

pub use crate::ext4::checksum::{DX_NODE_ENTRIES, DX_ROOT_ENTRIES};
use crate::host::ExtHost;
use crate::structs::*;
use crate::{FsError, MikuFS};

use super::dir::{
    ext2_dx_hash_full, insert_into_dir_block, DirHash, DX_HASH_TEA_UNSIGNED,
};

/// A `dx_entry`'s block number is 28 bits; the top four are reserved
const DX_BLOCK_MASK: u32 = 0x0fff_ffff;

/// Levels of index walked, root included. ext4 allows two without `large_dir`,
/// which at 4 KiB blocks is 507 * 511 leaves
const DX_MAX_LEVELS: usize = 2;

/// `dx_root_info` lives between the fake `..` entry and the entry array
const DX_INFO_OFF: usize = 24;
const DX_HASH_VERSION_OFF: usize = DX_INFO_OFF + 4;
const DX_INFO_LENGTH_OFF: usize = DX_INFO_OFF + 5;
const DX_INDIRECT_LEVELS_OFF: usize = DX_INFO_OFF + 6;

/// One node on the path from root to leaf. `at` is the entry followed, and the
/// two hashes beside it tell a split whether it would cut a chain of same-hash
/// leaves
#[derive(Clone, Copy)]
struct DxFrame {
    phys: u32,
    /// Where this node's `dx_entry` array starts
    entries_off: usize,
    count: u16,
    limit: u16,
    at: u16,
    /// Hash of entry `at`, or 0 for the first, whose hash field is the node's
    /// `{limit, count}` header
    at_hash: u32,
    /// The hash of entry `at + 1`, or `u32::MAX` when `at` is the last
    next_hash: u32,
}

impl DxFrame {
    const fn empty() -> Self {
        Self {
            phys: 0, entries_off: 0, count: 0, limit: 0,
            at: 0, at_hash: 0, next_hash: u32::MAX,
        }
    }
}

/// Where a descent ended up
struct Descent {
    /// Index of the deepest frame; 0 when the root points at leaves
    depth: usize,
    /// Directory-logical block of the leaf that owns the hash
    leaf: u32,
    hash: DirHash,
    hash_version: u8,
    seed: [u32; 4],
}

/// One entry lifted from a leaf for the rebuild. Eight bytes: the source block
/// stays in hand during the split
#[derive(Clone, Copy)]
struct LeafRec {
    hash: u32,
    minor: u32,
    pos: u16,
    size: u16,
}

pub(crate) fn dx_entry_hash(buf: &[u8], entries_off: usize, i: u16) -> u32 {
    let off = entries_off + i as usize * 8;
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

pub(crate) fn dx_entry_block(buf: &[u8], entries_off: usize, i: u16) -> u32 {
    let off = entries_off + i as usize * 8 + 4;
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) & DX_BLOCK_MASK
}

fn dx_set_entry(buf: &mut [u8], entries_off: usize, i: u16, hash: u32, block: u32) {
    let off = entries_off + i as usize * 8;
    // Entry zero's hash field is the node's `{limit, count}` header, so the
    // hash is dropped; the first child's lower bound lives one level up
    if i > 0 {
        buf[off..off + 4].copy_from_slice(&hash.to_le_bytes());
    }
    buf[off + 4..off + 8].copy_from_slice(&block.to_le_bytes());
}

fn dx_set_count(buf: &mut [u8], entries_off: usize, count: u16) {
    buf[entries_off + 2..entries_off + 4].copy_from_slice(&count.to_le_bytes());
}

fn dx_set_limit(buf: &mut [u8], entries_off: usize, limit: u16) {
    buf[entries_off..entries_off + 2].copy_from_slice(&limit.to_le_bytes());
}

/// The last entry whose hash is not above `hash`. Entry zero has no hash of its
/// own and stands for everything below the second's
fn dx_probe(buf: &[u8], entries_off: usize, count: u16, hash: u32) -> u16 {
    let mut at = 0u16;
    for i in 1..count {
        if hash < dx_entry_hash(buf, entries_off, i) {
            break;
        }
        at = i;
    }
    at
}

impl<H: ExtHost> MikuFS<H> {
    /// Whether this directory carries an index we are expected to maintain
    pub fn dir_is_indexed(&self, inode: &Inode) -> bool {
        inode.has_flag(EXT4_INDEX_FL) && self.superblock.has_dir_index()
    }

    /// Indirect-level count of an htree directory's dx_root (0 = the root
    /// points at leaves)
    pub fn htree_depth(&mut self, dir_inode: &Inode) -> Result<u8, FsError> {
        // Byte 30 is `dx_root_info.indirect_levels` only with a dx_root; on a
        // linear directory it is part of a filename
        if !self.dir_is_indexed(dir_inode) {
            return Ok(0);
        }
        let bs = self.block_size as usize;
        let root_phys = self.get_file_block(dir_inode, 0)?;
        if root_phys == 0 {
            return Ok(0);
        }
        let mut root_buf = [0u8; 4096];
        self.read_block_into(root_phys, &mut root_buf[..bs])?;
        Ok(root_buf[DX_INDIRECT_LEVELS_OFF])
    }

    /// Writes block 0 of a directory. On an indexed directory it is the root,
    /// with a checksum in `dx_tail` covering the two fake entries, so changing
    /// `..` stamps it as an index block
    pub fn write_dir_head(
        &mut self,
        dir_ino: u32,
        dir_inode: &Inode,
        phys: u32,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        if self.dir_is_indexed(dir_inode) {
            self.write_dx_block(dir_ino, phys, DX_ROOT_ENTRIES, buf)
        } else {
            self.write_dir_block(dir_ino, phys, buf)
        }
    }

    /// Read one index node into `buf` and describe it
    fn dx_read_node(
        &mut self,
        dir_ino: u32,
        dir_inode: &Inode,
        logical: u32,
        entries_off: usize,
        buf: &mut [u8],
    ) -> Result<DxFrame, FsError> {
        let bs = self.block_size as usize;
        let phys = self.get_file_block(dir_inode, logical)?;
        if phys == 0 {
            return Err(FsError::CorruptedFs);
        }
        self.read_block_into(phys, &mut buf[..bs])?;
        if !self.verify_dx_block(dir_ino, dir_inode.generation(), entries_off, &buf[..bs]) {
            crate::extlog!(
                "[extfs] directory {} index block {} fails its checksum",
                dir_ino, logical
            );
            return Err(FsError::ChecksumError);
        }
        let limit = u16::from_le_bytes([buf[entries_off], buf[entries_off + 1]]);
        let count = u16::from_le_bytes([buf[entries_off + 2], buf[entries_off + 3]]);
        // A count past the limit, or a limit past the block, cannot be walked
        // without reading past the buffer
        if count == 0 || count > limit || entries_off + limit as usize * 8 > bs {
            return Err(FsError::CorruptedFs);
        }
        Ok(DxFrame {
            phys, entries_off, count, limit,
            at: 0, at_hash: 0, next_hash: u32::MAX,
        })
    }

    /// Fills in which entry of `frame` covers `hash` and returns its block
    fn dx_follow(&self, frame: &mut DxFrame, buf: &[u8], hash: u32) -> u32 {
        frame.at = dx_probe(buf, frame.entries_off, frame.count, hash);
        frame.at_hash = if frame.at == 0 {
            0
        } else {
            dx_entry_hash(buf, frame.entries_off, frame.at)
        };
        frame.next_hash = if frame.at + 1 < frame.count {
            dx_entry_hash(buf, frame.entries_off, frame.at + 1)
        } else {
            u32::MAX
        };
        dx_entry_block(buf, frame.entries_off, frame.at)
    }

    /// Walks the index from root to the leaf owning `name`, recording the path.
    /// The hash version is the directory's, written to the root when built, so
    /// a retuned volume default does not affect old directories
    fn dx_descend(
        &mut self,
        dir_ino: u32,
        dir_inode: &Inode,
        name: &str,
        path: &mut [DxFrame; DX_MAX_LEVELS],
    ) -> Result<Descent, FsError> {
        let bs = self.block_size as usize;
        let mut buf = vec![0u8; bs];
        let mut frame = self.dx_read_node(dir_ino, dir_inode, 0, DX_ROOT_ENTRIES, &mut buf)?;

        let levels = buf[DX_INDIRECT_LEVELS_OFF] as usize;
        if levels + 1 > DX_MAX_LEVELS {
            // Three levels is `large_dir`, absent from `EXT_INCOMPAT_SUPP`, so
            // this volume passed the mount check with a superblock not
            // describing it. A second defense; the policy is in `mount.rs`
            crate::extlog!(
                "[extfs] directory {} has a {}-level index (large_dir)",
                dir_ino, levels + 1
            );
            return Err(FsError::UnsupportedFeature);
        }
        let hash_version = buf[DX_HASH_VERSION_OFF];
        let seed = [
            self.superblock.hash_seed(0),
            self.superblock.hash_seed(1),
            self.superblock.hash_seed(2),
            self.superblock.hash_seed(3),
        ];
        let hash = ext2_dx_hash_full(name.as_bytes(), hash_version, &seed);

        let mut child = self.dx_follow(&mut frame, &buf, hash.hash);
        path[0] = frame;

        let mut depth = 0usize;
        while depth < levels {
            let mut f = self.dx_read_node(dir_ino, dir_inode, child, DX_NODE_ENTRIES, &mut buf)?;
            child = self.dx_follow(&mut f, &buf, hash.hash);
            depth += 1;
            path[depth] = f;
        }
        Ok(Descent { depth, leaf: child, hash, hash_version, seed })
    }

    /// Finds a name through the index.
    ///
    /// `Ok(None)` is final: the descent walked to the one leaf for this hash,
    /// followed any continuation, and the name was absent, as in Linux's
    /// `ext4_find_entry`. A scan here would make each create read the whole
    /// directory.
    ///
    /// `Err` means a bad checksum, an unwalkable header, or an unsupported
    /// depth, when the caller reads every block
    pub fn htree_lookup(
        &mut self,
        dir_ino: u32,
        dir_inode: &Inode,
        name: &str,
    ) -> Result<Option<u32>, FsError> {
        Ok(self.htree_find(dir_ino, dir_inode, name)?.map(|(_, ino)| ino))
    }

    /// `htree_lookup`, also saying which block holds the name, as an unlink
    /// needs. Same `Ok(None)` and `Err`
    pub fn htree_find(
        &mut self,
        dir_ino: u32,
        dir_inode: &Inode,
        name: &str,
    ) -> Result<Option<(u32, u32)>, FsError> {
        if !self.dir_is_indexed(dir_inode) {
            return Err(FsError::UnsupportedFeature);
        }
        let bs = self.block_size as usize;
        let mut path = [DxFrame::empty(); DX_MAX_LEVELS];
        let d = self.dx_descend(dir_ino, dir_inode, name, &mut path)?;

        let mut leaf = d.leaf;
        let mut buf = vec![0u8; bs];
        loop {
            let phys = self.get_file_block(dir_inode, leaf)?;
            if phys == 0 {
                return Err(FsError::CorruptedFs);
            }
            self.read_block_into(phys, &mut buf[..bs])?;
            if let Some(ino) = find_in_dir_block(&buf[..bs], name.as_bytes()) {
                return Ok(Some((leaf, ino)));
            }

            // Names sharing one hash can outgrow a leaf; ext4 spills into the
            // next, marking it with bit 0 of that leaf's hash in the parent. A
            // matching search must follow. This code does not build such
            // chains, but reads Linux's
            match self.dx_next_leaf(dir_ino, dir_inode, &mut path, d.depth)? {
                Some((next, hash)) if (hash & !1) == d.hash.hash => leaf = next,
                _ => return Ok(None),
            }
        }
    }

    /// The leaf following the path's, and the hash the index records for it.
    /// Walks up when the deepest node is exhausted and down the first child of
    /// everything below
    fn dx_next_leaf(
        &mut self,
        dir_ino: u32,
        dir_inode: &Inode,
        path: &mut [DxFrame; DX_MAX_LEVELS],
        depth: usize,
    ) -> Result<Option<(u32, u32)>, FsError> {
        let bs = self.block_size as usize;
        let mut buf = vec![0u8; bs];
        let mut level = depth as isize;
        while level >= 0 {
            let l = level as usize;
            if path[l].at + 1 < path[l].count {
                let (phys, entries_off) = (path[l].phys, path[l].entries_off);
                self.read_block_into(phys, &mut buf[..bs])?;
                path[l].at += 1;
                let hash = dx_entry_hash(&buf, entries_off, path[l].at);
                let mut child = dx_entry_block(&buf, entries_off, path[l].at);
                for below in l + 1..=depth {
                    let mut f =
                        self.dx_read_node(dir_ino, dir_inode, child, DX_NODE_ENTRIES, &mut buf)?;
                    f.at = 0;
                    child = dx_entry_block(&buf, DX_NODE_ENTRIES, 0);
                    path[below] = f;
                }
                return Ok(Some((child, hash)));
            }
            level -= 1;
        }
        Ok(None)
    }

    /// Adds one entry to an indexed directory. Three attempts at most: insert;
    /// reshape and insert; deepen and insert. A tree that keeps asking for
    /// reshaping is corrupt
    pub fn htree_insert(
        &mut self,
        dir_ino: u32,
        name: &str,
        child_ino: u32,
        file_type: u8,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(dir_ino)?;
        if !self.dir_is_indexed(&inode) {
            return Err(FsError::UnsupportedFeature);
        }
        for _ in 0..3 {
            if self.dx_try_insert(dir_ino, name, child_ino, file_type)? {
                return Ok(());
            }
        }
        crate::extlog!("[extfs] directory {} index will not take another name", dir_ino);
        Err(FsError::NoSpace)
    }

    /// One attempt. `false` means the tree was reshaped and the caller
    /// redescends
    fn dx_try_insert(
        &mut self,
        dir_ino: u32,
        name: &str,
        child_ino: u32,
        file_type: u8,
    ) -> Result<bool, FsError> {
        let bs = self.block_size as usize;
        let usable = self.dir_block_usable();
        let dir_inode = self.read_inode(dir_ino)?;
        let mut path = [DxFrame::empty(); DX_MAX_LEVELS];
        let d = self.dx_descend(dir_ino, &dir_inode, name, &mut path)?;

        let leaf_phys = self.get_file_block(&dir_inode, d.leaf)?;
        if leaf_phys == 0 {
            return Err(FsError::CorruptedFs);
        }
        let mut leaf = vec![0u8; bs];
        self.read_block_into(leaf_phys, &mut leaf[..bs])?;

        if insert_into_dir_block(&mut leaf, usable, name.as_bytes(), child_ino, file_type) {
            self.write_dir_block(dir_ino, leaf_phys, &mut leaf)?;
            return Ok(true);
        }

        // The leaf is full. Splitting gives its parent one more child, so the
        // parent needs room first
        let parent = path[d.depth];
        if parent.count >= parent.limit {
            self.dx_make_room(dir_ino, d.depth, &path)?;
            return Ok(false);
        }
        self.dx_split_leaf(dir_ino, &d, &path, d.leaf, leaf_phys, &leaf, name, child_ino, file_type)
    }

    /// Makes room in a full index node, one level at a time. The caller retries
    /// the whole insert, as the path no longer describes the tree
    fn dx_make_room(
        &mut self,
        dir_ino: u32,
        depth: usize,
        path: &[DxFrame; DX_MAX_LEVELS],
    ) -> Result<(), FsError> {
        if depth == 0 {
            // The root is full: its entries move to a node of their own,
            // leaving the root with one entry and the tree one level deeper
            return self.htree_grow(dir_ino);
        }
        let root = path[0];
        if root.count >= root.limit {
            crate::extlog!("[extfs] directory {} index is full", dir_ino);
            return Err(FsError::NoSpace);
        }
        self.dx_split_node(dir_ino, &path[depth], &root)
    }

    /// Deepen a tree whose root has filled up
    fn htree_grow(&mut self, dir_ino: u32) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let mut inode = self.read_inode(dir_ino)?;
        let root_phys = self.get_file_block(&inode, 0)?;
        if root_phys == 0 {
            return Err(FsError::CorruptedFs);
        }
        let mut root = vec![0u8; bs];
        self.read_block_into(root_phys, &mut root[..bs])?;
        if root[DX_INDIRECT_LEVELS_OFF] != 0 {
            crate::extlog!("[extfs] directory {} index is full at two levels", dir_ino);
            return Err(FsError::NoSpace);
        }
        let count = u16::from_le_bytes([root[DX_ROOT_ENTRIES + 2], root[DX_ROOT_ENTRIES + 3]]);

        let logical = (inode.size() / self.block_size as u64) as u32;
        if logical > DX_BLOCK_MASK {
            return Err(FsError::NoSpace);
        }
        let node_phys = self.dir_grow_block(dir_ino, &mut inode, logical)?;

        let mut node = vec![0u8; bs];
        init_dx_node(&mut node, bs);
        // The entry array moves whole, including the four header bytes in entry
        // zero's hash field, then overwritten with this node's limit and count
        let src = DX_ROOT_ENTRIES;
        let len = count as usize * 8;
        node[DX_NODE_ENTRIES..DX_NODE_ENTRIES + len].copy_from_slice(&root[src..src + len]);
        let node_limit = self.dx_limit(DX_NODE_ENTRIES);
        dx_set_limit(&mut node, DX_NODE_ENTRIES, node_limit);
        dx_set_count(&mut node, DX_NODE_ENTRIES, count);
        self.write_dx_block(dir_ino, node_phys, DX_NODE_ENTRIES, &mut node)?;

        root[DX_INDIRECT_LEVELS_OFF] = 1;
        dx_set_count(&mut root, DX_ROOT_ENTRIES, 1);
        dx_set_entry(&mut root, DX_ROOT_ENTRIES, 0, 0, logical);
        self.write_dx_block(dir_ino, root_phys, DX_ROOT_ENTRIES, &mut root)?;

        self.touch_dir(dir_ino, &mut inode)
    }

    /// Split a full interior node in two, giving the root one more child
    fn dx_split_node(
        &mut self,
        dir_ino: u32,
        frame: &DxFrame,
        root: &DxFrame,
    ) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let mut inode = self.read_inode(dir_ino)?;
        let mut node = vec![0u8; bs];
        self.read_block_into(frame.phys, &mut node[..bs])?;

        let split = frame.count / 2;
        if split == 0 || split >= frame.count {
            return Err(FsError::NoSpace);
        }
        let hash2 = dx_entry_hash(&node, DX_NODE_ENTRIES, split);
        let moved = frame.count - split;

        let logical = (inode.size() / self.block_size as u64) as u32;
        let new_phys = self.dir_grow_block(dir_ino, &mut inode, logical)?;

        let mut half = vec![0u8; bs];
        init_dx_node(&mut half, bs);
        let src = DX_NODE_ENTRIES + split as usize * 8;
        let len = moved as usize * 8;
        half[DX_NODE_ENTRIES..DX_NODE_ENTRIES + len].copy_from_slice(&node[src..src + len]);
        let limit = self.dx_limit(DX_NODE_ENTRIES);
        dx_set_limit(&mut half, DX_NODE_ENTRIES, limit);
        dx_set_count(&mut half, DX_NODE_ENTRIES, moved);
        self.write_dx_block(dir_ino, new_phys, DX_NODE_ENTRIES, &mut half)?;

        dx_set_count(&mut node, DX_NODE_ENTRIES, split);
        self.write_dx_block(dir_ino, frame.phys, DX_NODE_ENTRIES, &mut node)?;

        self.dx_add_child(dir_ino, root, hash2, logical)?;
        self.touch_dir(dir_ino, &mut inode)
    }

    /// Split a full leaf, and put the new name into whichever half claims it
    fn dx_split_leaf(
        &mut self,
        dir_ino: u32,
        d: &Descent,
        path: &[DxFrame; DX_MAX_LEVELS],
        leaf_logical: u32,
        leaf_phys: u32,
        leaf: &[u8],
        name: &str,
        child_ino: u32,
        file_type: u8,
    ) -> Result<bool, FsError> {
        let bs = self.block_size as usize;
        let usable = self.dir_block_usable();
        let parent = path[d.depth];

        // A leaf in a same-hash chain cannot be split without rewriting the
        // chain. This code builds none, and leaves a found one as it is
        if (parent.at_hash & 1) != 0 {
            crate::extlog!(
                "[extfs] directory {} block {} continues a hash chain - not splitting",
                dir_ino, leaf_logical
            );
            return Err(FsError::NoSpace);
        }

        let mut recs = collect_leaf(leaf, usable, d.hash_version, &d.seed)?;
        recs.sort_unstable_by_key(|r| (r.hash, r.minor));

        // Cuts where the hash changes, as close to half the bytes as possible,
        // never inside a group of equal hashes
        let total: usize = recs.iter().map(|r| r.size as usize).sum();
        let mut split = 0usize;
        let mut best = usize::MAX;
        let mut acc = 0usize;
        for i in 1..recs.len() {
            acc += recs[i - 1].size as usize;
            if recs[i].hash == recs[i - 1].hash {
                continue;
            }
            let d = acc.abs_diff(total / 2);
            if d < best {
                best = d;
                split = i;
            }
        }
        if split == 0 {
            crate::extlog!(
                "[extfs] directory {} block {}: every name hashes alike, cannot split",
                dir_ino, leaf_logical
            );
            return Err(FsError::NoSpace);
        }
        let hash2 = recs[split].hash;

        // Also no split when the next leaf continues our last hash: the
        // boundary would land inside that chain
        if parent.next_hash != u32::MAX
            && (parent.next_hash & !1) == recs[recs.len() - 1].hash
        {
            crate::extlog!(
                "[extfs] directory {} block {} is followed by a hash chain - not splitting",
                dir_ino, leaf_logical
            );
            return Err(FsError::NoSpace);
        }

        let mut lo = vec![0u8; bs];
        let mut hi = vec![0u8; bs];
        self.init_dirent_tail(&mut lo[..bs]);
        self.init_dirent_tail(&mut hi[..bs]);
        fill_dir_block(&mut lo, usable, leaf, &recs[..split]);
        fill_dir_block(&mut hi, usable, leaf, &recs[split..]);

        // The half the new name belongs in takes it. If it does not fit, the
        // split stands and the caller redescends to a half-full leaf
        let landed = if d.hash.hash >= hash2 {
            insert_into_dir_block(&mut hi, usable, name.as_bytes(), child_ino, file_type)
        } else {
            insert_into_dir_block(&mut lo, usable, name.as_bytes(), child_ino, file_type)
        };

        let mut inode = self.read_inode(dir_ino)?;
        let logical = (inode.size() / self.block_size as u64) as u32;
        let new_phys = self.dir_grow_block(dir_ino, &mut inode, logical)?;

        // Order for a power cut: the new half is written, then the old block
        // loses the moved names, then the index learns of the new block. A
        // crash between the last two leaves names in an unindexed block, found
        // by a linear scan and repaired by fsck
        self.write_dir_block(dir_ino, new_phys, &mut hi)?;
        self.write_dir_block(dir_ino, leaf_phys, &mut lo)?;
        self.dx_add_child(dir_ino, &parent, hash2, logical)?;
        self.touch_dir(dir_ino, &mut inode)?;
        Ok(landed)
    }

    /// Insert one `{hash, block}` pair into an index node that has room
    fn dx_add_child(
        &mut self,
        dir_ino: u32,
        frame: &DxFrame,
        hash: u32,
        block: u32,
    ) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        if frame.count >= frame.limit {
            return Err(FsError::NoSpace);
        }
        // A `dx_entry` has twenty-eight bits for a block number; the top four
        // are reserved. Reached only at a terabyte
        if block > DX_BLOCK_MASK {
            return Err(FsError::NoSpace);
        }
        let mut buf = vec![0u8; bs];
        self.read_block_into(frame.phys, &mut buf[..bs])?;

        let at = frame.at as usize + 1;
        let from = frame.entries_off + at * 8;
        let end = frame.entries_off + frame.count as usize * 8;
        buf.copy_within(from..end, from + 8);
        dx_set_entry(&mut buf, frame.entries_off, at as u16, hash, block);
        dx_set_count(&mut buf, frame.entries_off, frame.count + 1);
        self.write_dx_block(dir_ino, frame.phys, frame.entries_off, &mut buf)
    }

    /// Turns a one-block directory into an indexed one, as Linux does
    /// (`make_indexed_dir`): it is about to need a second block, and everything
    /// still fits in one leaf. Longer directories stay linear, left to
    /// `e2fsck -D`
    pub fn htree_build(&mut self, dir_ino: u32) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let usable = self.dir_block_usable();
        let mut inode = self.read_inode(dir_ino)?;
        if !inode.is_directory() {
            return Err(FsError::NotDirectory);
        }
        if inode.has_flag(EXT4_INDEX_FL) || !self.superblock.has_dir_index() {
            return Err(FsError::UnsupportedFeature);
        }
        if inode.size() != self.block_size as u64 {
            return Err(FsError::UnsupportedFeature);
        }
        let hash_version = self.superblock.def_hash_version();
        // Past the three real hashes is a userspace-only variant or siphash,
        // needing a key not given here
        if hash_version > DX_HASH_TEA_UNSIGNED {
            crate::extlog!(
                "[extfs] cannot index directory {}: hash version {}",
                dir_ino, hash_version
            );
            return Err(FsError::UnsupportedFeature);
        }

        let root_phys = self.get_file_block(&inode, 0)?;
        if root_phys == 0 {
            return Err(FsError::CorruptedFs);
        }
        let mut old = vec![0u8; bs];
        self.read_block_into(root_phys, &mut old[..bs])?;

        // Everything except the two structural entries moves to the leaf
        let seed = [
            self.superblock.hash_seed(0),
            self.superblock.hash_seed(1),
            self.superblock.hash_seed(2),
            self.superblock.hash_seed(3),
        ];
        let mut recs = collect_leaf(&old, usable, hash_version, &seed)?;
        recs.retain(|r| {
            let nl = old[r.pos as usize + 6] as usize;
            let name = &old[r.pos as usize + 8..r.pos as usize + 8 + nl];
            name != b"." && name != b".."
        });
        recs.sort_unstable_by_key(|r| (r.hash, r.minor));

        let leaf_phys = self.dir_grow_block(dir_ino, &mut inode, 1)?;
        let mut leaf = vec![0u8; bs];
        self.init_dirent_tail(&mut leaf[..bs]);
        fill_dir_block(&mut leaf, usable, &old, &recs);
        self.write_dir_block(dir_ino, leaf_phys, &mut leaf)?;

        // The root keeps `.` and `..`, with `..` stretched over the rest of the
        // block, index and checksum included, so an index-unaware reader sees
        // one empty record
        let mut root = vec![0u8; bs];
        let dot = u32::from_le_bytes([old[0], old[1], old[2], old[3]]);
        let dotdot = u32::from_le_bytes([old[12], old[13], old[14], old[15]]);
        root[0..4].copy_from_slice(&dot.to_le_bytes());
        root[4..6].copy_from_slice(&12u16.to_le_bytes());
        root[6] = 1;
        root[7] = FT_DIR;
        root[8] = b'.';
        root[12..16].copy_from_slice(&dotdot.to_le_bytes());
        root[16..18].copy_from_slice(&((bs - 12) as u16).to_le_bytes());
        root[18] = 2;
        root[19] = FT_DIR;
        root[20] = b'.';
        root[21] = b'.';
        root[DX_HASH_VERSION_OFF] = hash_version;
        root[DX_INFO_LENGTH_OFF] = 8;
        root[DX_INDIRECT_LEVELS_OFF] = 0;
        let limit = self.dx_limit(DX_ROOT_ENTRIES);
        if limit < 2 {
            return Err(FsError::UnsupportedFeature);
        }
        dx_set_limit(&mut root, DX_ROOT_ENTRIES, limit);
        dx_set_count(&mut root, DX_ROOT_ENTRIES, 1);
        dx_set_entry(&mut root, DX_ROOT_ENTRIES, 0, 0, 1);
        self.write_dx_block(dir_ino, root_phys, DX_ROOT_ENTRIES, &mut root)?;

        inode.set_flags(inode.flags() | EXT4_INDEX_FL);
        self.touch_dir(dir_ino, &mut inode)
    }

    /// Stamps a directory's times and writes the inode. Each reshaping step
    /// ends here, since `dir_grow_block` left the block map in the caller's
    /// copy
    fn touch_dir(&mut self, dir_ino: u32, inode: &mut Inode) -> Result<(), FsError> {
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(dir_ino, inode)
    }
}

/// Lays down an interior node's fake entry: inode zero over the whole block
fn init_dx_node(buf: &mut [u8], bs: usize) {
    buf[0..4].copy_from_slice(&0u32.to_le_bytes());
    buf[4..6].copy_from_slice(&(bs as u16).to_le_bytes());
    buf[6] = 0;
    buf[7] = 0;
}

/// Every real entry in a directory block, with its hash
fn collect_leaf(
    buf: &[u8],
    usable: usize,
    hash_version: u8,
    seed: &[u32; 4],
) -> Result<Vec<LeafRec>, FsError> {
    let mut recs = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= usable {
        let ino = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        let rec_len = u16::from_le_bytes([buf[pos + 4], buf[pos + 5]]) as usize;
        let name_len = buf[pos + 6] as usize;
        if rec_len < 8 || rec_len > usable - pos || rec_len % 4 != 0 {
            return Err(FsError::CorruptedFs);
        }
        if ino != 0 && name_len > 0 {
            if 8 + name_len > rec_len {
                return Err(FsError::CorruptedFs);
            }
            let h = ext2_dx_hash_full(&buf[pos + 8..pos + 8 + name_len], hash_version, seed);
            recs.push(LeafRec {
                hash: h.hash,
                minor: h.minor,
                pos: pos as u16,
                size: ((8 + name_len + 3) / 4 * 4) as u16,
            });
        }
        pos += rec_len;
    }
    Ok(recs)
}

/// Writes records back to back, with the last stretched to the end of the
/// usable area
fn fill_dir_block(out: &mut [u8], usable: usize, src: &[u8], recs: &[LeafRec]) {
    let mut pos = 0usize;
    for (i, r) in recs.iter().enumerate() {
        let size = r.size as usize;
        out[pos..pos + size].copy_from_slice(&src[r.pos as usize..r.pos as usize + size]);
        let rec_len = if i + 1 == recs.len() { usable - pos } else { size };
        out[pos + 4..pos + 6].copy_from_slice(&(rec_len as u16).to_le_bytes());
        pos += size;
    }
    if recs.is_empty() {
        // An empty block is one record: inode zero covering it
        out[0..4].copy_from_slice(&0u32.to_le_bytes());
        out[4..6].copy_from_slice(&(usable as u16).to_le_bytes());
        out[6] = 0;
        out[7] = 0;
    }
}

/// Inode of a name in this block, if present
pub(crate) fn find_in_dir_block(buf: &[u8], name: &[u8]) -> Option<u32> {
    let mut pos = 0usize;
    while pos + 8 <= buf.len() {
        let ino = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        let rec_len = u16::from_le_bytes([buf[pos + 4], buf[pos + 5]]) as usize;
        let name_len = buf[pos + 6] as usize;
        if rec_len < 8 || pos + rec_len > buf.len() {
            return None;
        }
        if ino != 0
            && name_len == name.len()
            && pos + 8 + name_len <= buf.len()
            && &buf[pos + 8..pos + 8 + name_len] == name
        {
            return Some(ino);
        }
        pos += rec_len;
    }
    None
}
