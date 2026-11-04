use crate::structs::*;
use crate::{FsError, MikuFS};
use crate::host::ExtHost;

impl<H: ExtHost> MikuFS<H> {
    /// The validated extent tree root in an inode. No walker indexes `entries`
    /// before this agrees the count fits in the inode's 60 bytes
    pub fn extent_root(&self, inode: &Inode) -> Result<ExtentHeader, FsError> {
        let header = inode.extent_header();
        if !header.check(EXTENT_ROOT_SPACE, None) {
            return Err(FsError::CorruptedFs);
        }
        Ok(header)
    }

    /// Reads one extent tree node and validates its header before any indexing.
    ///
    /// `expected_depth` is what the parent said; a node pointing back up has
    /// the wrong depth, so a cycle stops the walk.
    ///
    /// `owner` is the file's inode number and generation, keying the tail
    /// checksum. Walkers with only an `&Inode` pass `None` and get the header
    /// checks
    pub fn read_extent_node(
        &mut self,
        block: u32,
        expected_depth: Option<u16>,
        owner: Option<(u32, u32)>,
        buf: &mut [u8],
    ) -> Result<ExtentHeader, FsError> {
        let bs = self.block_size as usize;
        if block == 0 || block >= self.superblock.blocks_count() {
            return Err(FsError::InvalidBlock);
        }
        self.read_block_into(block, &mut buf[..bs])?;
        let header = ExtentHeader {
            magic:      u16::from_le_bytes([buf[0], buf[1]]),
            entries:    u16::from_le_bytes([buf[2], buf[3]]),
            max:        u16::from_le_bytes([buf[4], buf[5]]),
            depth:      u16::from_le_bytes([buf[6], buf[7]]),
            generation: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        };
        if !header.check(bs, expected_depth) {
            return Err(FsError::CorruptedFs);
        }
        if let Some((ino, gen)) = owner {
            if !self.verify_extent_block(ino, gen, &buf[..bs]) {
                crate::extlog!(
                    "[extfs] extent block {} of inode {} fails its checksum",
                    block,
                    ino
                );
                return Err(FsError::ChecksumError);
            }
        }
        Ok(header)
    }

    pub fn get_file_block_extent(
        &mut self,
        inode: &Inode,
        logical_block: u32,
    ) -> Result<u32, FsError> {
        self.get_file_block_extent_owned(inode, None, logical_block)
    }

    /// The same lookup, knowing whose tree it walks, so each node's tail
    /// checksum is verified. An unwritten extent reads as a hole: `fallocate`
    /// reserves blocks without contents, the extent is flagged, and each reader
    /// honors it. The write path uses `ext4_map_block`
    pub fn get_file_block_extent_owned(
        &mut self,
        inode: &Inode,
        owner: Option<(u32, u32)>,
        logical_block: u32,
    ) -> Result<u32, FsError> {
        let (phys, unwritten) = self.ext4_map_block(inode, owner, logical_block)?;
        Ok(if unwritten { 0 } else { phys })
    }

    /// Where a logical block lives and whether it was written. `(0, false)` for
    /// a hole
    pub fn ext4_map_block(
        &mut self,
        inode: &Inode,
        owner: Option<(u32, u32)>,
        logical_block: u32,
    ) -> Result<(u32, bool), FsError> {
        let header = self.extent_root(inode)?;
        if header.depth == 0 {
            for i in 0..header.entries as usize {
                let ext = inode.extent_at(i);
                if logical_block >= ext.block && logical_block < ext.block + ext.actual_len() {
                    let offset = logical_block - ext.block;
                    return Ok((
                        (ext.start() + offset as u64) as u32,
                        ext.is_uninitialized(),
                    ));
                }
            }
            return Ok((0, false));
        }
        let mut target_block = 0u64;
        for i in 0..header.entries as usize {
            let idx = inode.extent_idx_at(i);
            if logical_block >= idx.block {
                target_block = idx.leaf();
            }
        }
        if target_block == 0 {
            return Ok((0, false));
        }
        self.search_extent_tree(target_block as u32, logical_block, header.depth - 1, owner)
    }

    fn search_extent_tree(
        &mut self,
        block_num: u32,
        logical_block: u32,
        depth: u16,
        owner: Option<(u32, u32)>,
    ) -> Result<(u32, bool), FsError> {
        let mut buf = [0u8; 4096];
        let node = self.read_extent_node(block_num, Some(depth), owner, &mut buf)?;
        let entries = node.entries;
        if depth == 0 {
            for i in 0..entries as usize {
                let base = 12 + i * 12;
                let ee_block =
                    u32::from_le_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]]);
                let ee_len = u16::from_le_bytes([buf[base + 4], buf[base + 5]]);
                let ee_start_hi = u16::from_le_bytes([buf[base + 6], buf[base + 7]]);
                let ee_start_lo = u32::from_le_bytes([
                    buf[base + 8],
                    buf[base + 9],
                    buf[base + 10],
                    buf[base + 11],
                ]);
                let actual_len = if ee_len > 32768 {
                    ee_len - 32768
                } else {
                    ee_len
                } as u32;
                if logical_block >= ee_block && logical_block < ee_block + actual_len {
                    let offset = logical_block - ee_block;
                    let start = (ee_start_lo as u64) | ((ee_start_hi as u64) << 32);
                    return Ok(((start + offset as u64) as u32, ee_len > 32768));
                }
            }
            return Ok((0, false));
        }
        let mut target_block = 0u64;
        for i in 0..entries as usize {
            let base = 12 + i * 12;
            let ei_block =
                u32::from_le_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]]);
            let ei_leaf_lo =
                u32::from_le_bytes([buf[base + 4], buf[base + 5], buf[base + 6], buf[base + 7]]);
            let ei_leaf_hi = u16::from_le_bytes([buf[base + 8], buf[base + 9]]);
            if logical_block >= ei_block {
                target_block = (ei_leaf_lo as u64) | ((ei_leaf_hi as u64) << 32);
            }
        }
        if target_block == 0 {
            return Ok((0, false));
        }
        self.search_extent_tree(target_block as u32, logical_block, depth - 1, owner)
    }

    /// Adds one block to a file's extent tree: a root of four entries in the
    /// inode, with index nodes below.
    ///
    /// Descend once recording the path, insert if the leaf has room, otherwise
    /// split upward to the deepest ancestor with room. With no room on the
    /// path, deepen the tree and retry. No node is split before its index
    /// entry's destination is known
    pub fn ext4_insert_extent(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
        phys_block: u32,
    ) -> Result<(), FsError> {
        self.ext4_insert_extent_run(inode, inode_num, logical_block, 1, phys_block)
    }

    /// `len` blocks in a row, from `phys_block` on disk and `logical_block` in
    /// the file, as one written extent, merged into the previous when
    /// continuous
    pub fn ext4_insert_extent_run(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
        len: u16,
        phys_block: u32,
    ) -> Result<(), FsError> {
        self.ext4_insert_extent_record(
            inode,
            inode_num,
            &extent_entry(logical_block, len, phys_block),
            true,
        )
    }

    /// Inserts a whole extent record. `merge` lets an adjacent extent absorb
    /// it, as a sequential write wants and an unwritten split must not
    pub fn ext4_insert_extent_record(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        record: &[u8; 12],
        merge: bool,
    ) -> Result<(), FsError> {
        // One deepening per level, within the maximum depth, so this terminates
        for _ in 0..=EXTENT_MAX_DEPTH {
            if self.ext4_try_insert_extent(inode, inode_num, record, merge)? {
                return Ok(());
            }
            self.ext4_grow_extent_tree(inode, inode_num)?;
        }
        Err(FsError::ExtentFull)
    }

    /// One attempt. `Ok(false)` means every node from leaf to root is full and
    /// the tree must grow
    fn ext4_try_insert_extent(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        record: &[u8; 12],
        merge: bool,
    ) -> Result<bool, FsError> {
        let logical_block = read_u32_at(record, 0);
        let root = self.extent_root(inode)?;
        let gen = inode.generation();
        let owner = Some((inode_num, gen));
        let new_extent = *record;

        if root.depth == 0 {
            return Ok(self.ext4_insert_extent_root(inode, record, merge));
        }

        // Descends keeping only what decides if a level has room; every node's
        // buffer would be 20 KiB of stack on a five-level tree
        let mut levels = [Level::EMPTY; MAX_LEVELS];
        levels[0] = Level {
            block:   0,
            index:   pick_slot_inode(inode, root.entries, logical_block),
            entries: root.entries,
            max:     root.max,
        };
        let mut n = 1usize;
        let mut child = inode.extent_idx_at(levels[0].index).leaf() as u32;
        let mut depth = root.depth - 1;
        let mut buf = [0u8; 4096];

        loop {
            if n >= MAX_LEVELS {
                return Err(FsError::CorruptedFs);
            }
            let node = self.read_extent_node(child, Some(depth), owner, &mut buf)?;
            let index = pick_slot(&buf, node.entries, logical_block);
            levels[n] = Level {
                block: child,
                index,
                entries: node.entries,
                max: node.max,
            };
            n += 1;
            if depth == 0 {
                break;
            }
            child = node_child(&buf, index);
            depth -= 1;
        }

        // `buf` still holds the leaf
        let leaf = levels[n - 1];

        // Extending an extent is free, as sequential writes do. Only one in the
        // same state may absorb this: mixing would say written blocks read as
        // zeros
        if merge {
            for i in 0..leaf.entries as usize {
                let base = 12 + i * 12;
                if let Some(grown) = merged_len(&buf[base..base + 12], record) {
                    buf[base + 4..base + 6].copy_from_slice(&grown.to_le_bytes());
                    self.write_extent_node(inode_num, gen, leaf.block, &mut buf)?;
                    return Ok(true);
                }
            }
        }

        if leaf.entries < leaf.max {
            node_insert_sorted(&mut buf, leaf.entries, &new_extent);
            self.write_extent_node(inode_num, gen, leaf.block, &mut buf)?;
            return Ok(true);
        }

        // The leaf is full. Finds the deepest ancestor with room, where
        // splitting stops
        let mut target = None;
        for i in (0..n - 1).rev() {
            if levels[i].entries < levels[i].max {
                target = Some(i);
                break;
            }
        }
        let Some(target) = target else {
            // Every level is full, including the root: the tree is too short
            return Ok(false);
        };

        let mut carry = self.ext4_split_node(inode, inode_num, gen, leaf.block, &mut buf, &new_extent)?;

        // Every level between the target and leaf is full too, so each splits
        // and gives its parent an index entry
        for i in ((target + 1)..n - 1).rev() {
            let block = levels[i].block;
            self.read_extent_node(block, None, owner, &mut buf)?;
            carry = self.ext4_split_node(inode, inode_num, gen, block, &mut buf, &carry)?;
        }

        if target == 0 {
            let entries = inode.extent_header().entries;
            let pos = insert_pos_inode(inode, entries, read_u32_at(&carry, 0));
            for j in (pos..entries as usize).rev() {
                let e = inode.extent_idx_at(j);
                inode.set_extent_idx_at_raw(j + 1, e.block, e.leaf() as u32, 0);
            }
            let leaf_block = read_u32_at(&carry, 4);
            inode.set_extent_idx_at_raw(pos, read_u32_at(&carry, 0), leaf_block, 0);
            inode.set_extent_entries(entries + 1);
        } else {
            let block = levels[target].block;
            let node = self.read_extent_node(block, None, owner, &mut buf)?;
            node_insert_sorted(&mut buf, node.entries, &carry);
            self.write_extent_node(inode_num, gen, block, &mut buf)?;
        }
        Ok(true)
    }

    /// Finds the record covering `logical` and where it lives: the holding
    /// node's block (zero for the root, in the inode) and its slot
    fn ext4_locate_extent(
        &mut self,
        inode: &Inode,
        owner: Option<(u32, u32)>,
        logical: u32,
    ) -> Result<Option<(u32, usize, Extent)>, FsError> {
        let root = self.extent_root(inode)?;
        if root.depth == 0 {
            for i in 0..root.entries as usize {
                let ext = inode.extent_at(i);
                if logical >= ext.block && logical < ext.block + ext.actual_len() {
                    return Ok(Some((0, i, ext)));
                }
            }
            return Ok(None);
        }

        let mut child = inode
            .extent_idx_at(pick_slot_inode(inode, root.entries, logical))
            .leaf() as u32;
        let mut depth = root.depth - 1;
        let mut buf = [0u8; 4096];
        loop {
            let node = self.read_extent_node(child, Some(depth), owner, &mut buf)?;
            if depth == 0 {
                for i in 0..node.entries as usize {
                    let base = 12 + i * 12;
                    let ext = Extent {
                        block:    read_u32_at(&buf, base),
                        len:      u16::from_le_bytes([buf[base + 4], buf[base + 5]]),
                        start_hi: u16::from_le_bytes([buf[base + 6], buf[base + 7]]),
                        start_lo: read_u32_at(&buf, base + 8),
                    };
                    if logical >= ext.block && logical < ext.block + ext.actual_len() {
                        return Ok(Some((child, i, ext)));
                    }
                }
                return Ok(None);
            }
            child = node_child(&buf, pick_slot(&buf, node.entries, logical));
            depth -= 1;
        }
    }

    /// Overwrite one record in place, wherever it lives
    fn ext4_replace_extent(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        node_block: u32,
        slot: usize,
        record: &[u8; 12],
    ) -> Result<(), FsError> {
        if node_block == 0 {
            let at = 52 + slot * 12;
            inode.data[at..at + 12].copy_from_slice(record);
            return Ok(());
        }
        let gen = inode.generation();
        let mut buf = [0u8; 4096];
        self.read_extent_node(node_block, None, Some((inode_num, gen)), &mut buf)?;
        let at = 12 + slot * 12;
        buf[at..at + 12].copy_from_slice(record);
        self.write_extent_node(inode_num, gen, node_block, &mut buf)
    }

    /// Turns one block of an unwritten extent into written. `fallocate` marks
    /// blocks unwritten so reads give zeros without disk writes. The first
    /// write splits the extent into up to three: unwritten before, this written
    /// block, unwritten after. Only this block is zeroed, so a large
    /// `fallocate` is paid one block at a time
    pub fn ext4_convert_to_written(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical: u32,
    ) -> Result<u32, FsError> {
        let owner = Some((inode_num, inode.generation()));
        let Some((node_block, slot, ext)) = self.ext4_locate_extent(inode, owner, logical)? else {
            return Err(FsError::InvalidBlock);
        };
        let start = ext.start() as u32;
        let phys = start + (logical - ext.block);
        if !ext.is_uninitialized() {
            return Ok(phys);
        }

        let len = ext.actual_len();
        let before = logical - ext.block;
        let after = ext.block + len - logical - 1;

        // About to read as written data, so it must hold the promised zeros
        self.zero_block(phys)?;

        let (keep, extra) = if before > 0 {
            (
                unwritten_entry(ext.block, before as u16, start),
                Some(extent_entry(logical, 1, phys)),
            )
        } else {
            (extent_entry(logical, 1, phys), None)
        };
        self.ext4_replace_extent(inode, inode_num, node_block, slot, &keep)?;

        if let Some(written) = extra {
            self.ext4_insert_extent_record(inode, inode_num, &written, false)?;
        }
        if after > 0 {
            let tail = unwritten_entry(logical + 1, after as u16, phys + 1);
            self.ext4_insert_extent_record(inode, inode_num, &tail, false)?;
        }
        Ok(phys)
    }

    /// Inserts into the root's entries while the tree is one level. `false`
    /// when the root is full
    fn ext4_insert_extent_root(
        &mut self,
        inode: &mut Inode,
        record: &[u8; 12],
        merge: bool,
    ) -> bool {
        let header = inode.extent_header();
        let entries = header.entries;
        let max = header.max;
        let logical_block = read_u32_at(record, 0);
        let record_len = u16::from_le_bytes([record[4], record[5]]);
        let phys_block = read_u32_at(record, 8);

        if merge {
            for i in 0..entries as usize {
                let ext = inode.extent_at(i);
                let existing = extent_entry_raw(&ext);
                if let Some(grown) = merged_len(&existing, record) {
                    inode.set_extent_len_at(i, grown);
                    return true;
                }
            }
        }

        if entries < max {
            let mut pos = entries as usize;
            for i in 0..entries as usize {
                if logical_block < inode.extent_at(i).block {
                    pos = i;
                    break;
                }
            }
            for i in (pos..entries as usize).rev() {
                let e = inode.extent_at(i);
                inode.set_extent_at_raw(i + 1, e.block, e.len, e.start_hi, e.start_lo);
            }
            let hi = u16::from_le_bytes([record[6], record[7]]);
            inode.set_extent_at_raw(pos, logical_block, record_len, hi, phys_block);
            inode.set_extent_entries(entries + 1);
            return true;
        }
        false
    }

    /// Splits one node around a new entry and returns the index entry its
    /// parent gains. Called only with a parent known to have room or about to
    /// split itself
    fn ext4_split_node(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        gen: u32,
        block: u32,
        buf: &mut [u8],
        entry: &[u8; 12],
    ) -> Result<[u8; 12], FsError> {
        let bs = self.block_size as usize;
        let entries = node_entries(buf) as usize;
        let max = node_max(buf);
        let depth = node_depth(buf);

        // Entries plus the new one, in order. `max + 1` entries is 4092 bytes
        // at 4 KiB, within one block
        let mut sorted = [0u8; 4096];
        let key = read_u32_at(entry, 0);
        let mut pos = entries;
        for i in 0..entries {
            if key < read_u32_at(buf, 12 + i * 12) {
                pos = i;
                break;
            }
        }
        sorted[..pos * 12].copy_from_slice(&buf[12..12 + pos * 12]);
        sorted[pos * 12..pos * 12 + 12].copy_from_slice(entry);
        sorted[(pos + 1) * 12..(entries + 1) * 12]
            .copy_from_slice(&buf[12 + pos * 12..12 + entries * 12]);

        let total = entries + 1;
        let keep = total / 2;
        let moved = total - keep;

        let group = ((inode_num - 1) / self.inodes_per_group) as usize;
        let new_block = self.alloc_block(group)?;

        let mut new_node = [0u8; 4096];
        new_node[0..2].copy_from_slice(&EXT4_EXT_MAGIC.to_le_bytes());
        new_node[2..4].copy_from_slice(&(moved as u16).to_le_bytes());
        new_node[4..6].copy_from_slice(&max.to_le_bytes());
        new_node[6..8].copy_from_slice(&depth.to_le_bytes());
        new_node[12..12 + moved * 12].copy_from_slice(&sorted[keep * 12..total * 12]);
        self.write_extent_node(inode_num, gen, new_block, &mut new_node)?;

        buf[12..12 + keep * 12].copy_from_slice(&sorted[..keep * 12]);
        for b in buf[12 + keep * 12..bs].iter_mut() {
            *b = 0;
        }
        buf[2..4].copy_from_slice(&(keep as u16).to_le_bytes());
        self.write_extent_node(inode_num, gen, block, buf)?;

        // The tree block itself counts against the file
        let blks = inode.blocks_full() + (self.block_size / 512) as u64;
        inode.set_blocks_full(blks);

        let first = read_u32_at(&new_node, 12);
        Ok(index_entry(first, new_block))
    }

    /// Pushes the whole tree one level down: the root's entries move to a new
    /// block, leaving one index to it
    pub fn ext4_grow_extent_tree(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
    ) -> Result<(), FsError> {
        let group = ((inode_num - 1) / self.inodes_per_group) as usize;
        let new_block = self.alloc_block(group)?;
        let header = inode.extent_header();
        let old_depth = header.depth;
        let old_entries = header.entries;
        let bs = self.block_size as usize;
        let max_entries = ExtentHeader::capacity(bs);

        let mut leaf_data = [0u8; 4096];
        leaf_data[0..2].copy_from_slice(&EXT4_EXT_MAGIC.to_le_bytes());
        leaf_data[2..4].copy_from_slice(&old_entries.to_le_bytes());
        leaf_data[4..6].copy_from_slice(&max_entries.to_le_bytes());
        leaf_data[6..8].copy_from_slice(&old_depth.to_le_bytes());
        leaf_data[8..12].copy_from_slice(&0u32.to_le_bytes());

        for i in 0..old_entries as usize {
            let src_base = 52 + i * 12;
            let dst_base = 12 + i * 12;
            leaf_data[dst_base..dst_base + 12]
                .copy_from_slice(&inode.data[src_base..src_base + 12]);
        }
        let gen = inode.generation();
        self.write_extent_node(inode_num, gen, new_block, &mut leaf_data)?;

        let first_logical = if old_entries > 0 {
            read_u32_at(&inode.data, 52)
        } else {
            0
        };

        for i in 0..4 {
            let base = 52 + i * 12;
            for j in 0..12 {
                inode.data[base + j] = 0;
            }
        }

        inode.set_extent_depth(old_depth + 1);
        inode.set_extent_entries(1);
        inode.set_extent_idx_at_raw(0, first_logical, new_block, 0);

        let blks = inode.blocks_full() + (self.block_size / 512) as u64;
        inode.set_blocks_full(blks);

        Ok(())
    }


    pub fn ext4_free_extent_blocks(&mut self, inode: &Inode) -> Result<u32, FsError> {
        // The extended-attribute block, outside the tree. A double free is
        // harmless: `free_block` moves counters only if the bit changed
        let acl = inode.file_acl_lo();
        if acl != 0 && self.is_valid_block(acl) {
            self.free_block(acl)?;
        }

        // An unusable root means nothing to free, not an error on a delete
        // path; the count is still validated for the loops below
        let Ok(header) = self.extent_root(inode) else {
            return Ok(0);
        };
        // A regular file's blocks are unlogged data, needing no revoke; others
        // (a directory's) may be logged
        let revoke = !inode.is_regular();
        if header.depth == 0 {
            let mut freed = 0u32;
            for i in 0..header.entries as usize {
                let ext = inode.extent_at(i);
                let start = ext.start() as u32;
                let len = ext.actual_len();
                let _ = self.free_blocks(start, len, revoke);
                freed += len;
            }
            return Ok(freed);
        }
        let mut freed = 0u32;
        for i in 0..header.entries as usize {
            let idx = inode.extent_idx_at(i);
            let child = idx.leaf() as u32;
            if child != 0 {
                freed += self.ext4_free_extent_tree_block(child, header.depth - 1, revoke)?;
                let _ = self.free_block(child);
                freed += 1;
            }
        }
        Ok(freed)
    }

    /// Walks one node of a tree being freed, through `read_extent_node`, which
    /// bounds the count. No `owner`: a crash may leave a tree mid-write, and
    /// refusing over a checksum would leak its blocks
    fn ext4_free_extent_tree_block(&mut self, block_num: u32, depth: u16, revoke: bool) -> Result<u32, FsError> {
        let mut buf = [0u8; 4096];
        let node = self.read_extent_node(block_num, Some(depth), None, &mut buf)?;
        let entries = node.entries;
        let mut freed = 0u32;

        if depth == 0 {
            for i in 0..entries as usize {
                let base = 12 + i * 12;
                let ee_len = u16::from_le_bytes([buf[base + 4], buf[base + 5]]);
                let ee_start_lo = u32::from_le_bytes([
                    buf[base + 8],
                    buf[base + 9],
                    buf[base + 10],
                    buf[base + 11],
                ]);
                let actual_len = if ee_len > 32768 {
                    ee_len - 32768
                } else {
                    ee_len
                } as u32;
                let _ = self.free_blocks(ee_start_lo, actual_len, revoke);
                freed += actual_len;
            }
        } else {
            for i in 0..entries as usize {
                let base = 12 + i * 12;
                let leaf_lo = u32::from_le_bytes([
                    buf[base + 4],
                    buf[base + 5],
                    buf[base + 6],
                    buf[base + 7],
                ]);
                if leaf_lo != 0 {
                    freed += self.ext4_free_extent_tree_block(leaf_lo, depth - 1, revoke)?;
                    let _ = self.free_block(leaf_lo);
                    freed += 1;
                }
            }
        }
        Ok(freed)
    }

    pub fn ext4_extent_count(&mut self, inode: &Inode) -> Result<u32, FsError> {
        let Ok(header) = self.extent_root(inode) else {
            return Ok(0);
        };
        if header.depth == 0 {
            return Ok(header.entries as u32);
        }
        let mut total = 0u32;
        for i in 0..header.entries as usize {
            let idx = inode.extent_idx_at(i);
            let child = idx.leaf() as u32;
            if child != 0 {
                total += self.ext4_count_tree_extents(child, header.depth - 1)?;
            }
        }
        Ok(total)
    }

    fn ext4_count_tree_extents(&mut self, block_num: u32, depth: u16) -> Result<u32, FsError> {
        let mut buf = [0u8; 4096];
        let node = self.read_extent_node(block_num, Some(depth), None, &mut buf)?;
        let entries = node.entries;
        if depth == 0 {
            return Ok(entries as u32);
        }
        let mut total = 0u32;
        for i in 0..entries as usize {
            let base = 12 + i * 12;
            let leaf_lo =
                u32::from_le_bytes([buf[base + 4], buf[base + 5], buf[base + 6], buf[base + 7]]);
            if leaf_lo != 0 {
                total += self.ext4_count_tree_extents(leaf_lo, depth - 1)?;
            }
        }
        Ok(total)
    }

    // fiemap: returns the extent map as (logical_block, physical_block, length)
    // tuples
    pub fn ext4_fiemap(
        &mut self,
        inode_num: u32,
        extents: &mut [(u32, u32, u32)],
    ) -> Result<usize, FsError> {
        let inode = self.read_inode(inode_num)?;
        if !inode.uses_extents() {
            // for non-extent files, build a simple block map
            let size = inode.size();
            let bs = self.block_size as u64;
            let total_blocks = ((size + bs - 1) / bs) as u32;
            let mut count = 0usize;
            let mut logical = 0u32;
            while logical < total_blocks && count < extents.len() {
                let phys = self.get_file_block(&inode, logical)?;
                if phys != 0 {
                    // try to merge with previous extent
                    if count > 0 {
                        let (pl, pp, plen) = extents[count - 1];
                        if logical == pl + plen && phys == pp + plen {
                            extents[count - 1].2 += 1;
                            logical += 1;
                            continue;
                        }
                    }
                    extents[count] = (logical, phys, 1);
                    count += 1;
                }
                logical += 1;
            }
            return Ok(count);
        }

        // Extent-based file: reads from the tree
        let header = self.extent_root(&inode)?;

        if header.depth == 0 {
            let mut count = 0usize;
            for i in 0..header.entries as usize {
                if count >= extents.len() { break; }
                let ext = inode.extent_at(i);
                extents[count] = (ext.block, ext.start() as u32, ext.actual_len());
                count += 1;
            }
            return Ok(count);
        }

        // Multi-level tree: collects leaf extents
        let mut count = 0usize;
        let owner = Some((inode_num, inode.generation()));
        self.ext4_fiemap_tree(&inode, owner, header.depth, &mut count, extents)?;
        Ok(count)
    }

    fn ext4_fiemap_tree(
        &mut self,
        inode: &Inode,
        owner: Option<(u32, u32)>,
        depth: u16,
        count: &mut usize,
        extents: &mut [(u32, u32, u32)],
    ) -> Result<(), FsError> {
        let header = inode.extent_header();
        for i in 0..header.entries as usize {
            if *count >= extents.len() { return Ok(()); }
            let idx = inode.extent_idx_at(i);
            let leaf = idx.leaf() as u32;
            if leaf == 0 { continue; }
            self.ext4_fiemap_node(leaf, owner, depth - 1, count, extents)?;
        }
        Ok(())
    }

    fn ext4_fiemap_node(
        &mut self,
        block_num: u32,
        owner: Option<(u32, u32)>,
        depth: u16,
        count: &mut usize,
        extents: &mut [(u32, u32, u32)],
    ) -> Result<(), FsError> {
        let mut buf = [0u8; 4096];
        let node = self.read_extent_node(block_num, Some(depth), owner, &mut buf)?;
        let entries = node.entries;

        if depth == 0 {
            for i in 0..entries as usize {
                if *count >= extents.len() { return Ok(()); }
                let base = 12 + i * 12;
                let ee_block = u32::from_le_bytes([buf[base], buf[base+1], buf[base+2], buf[base+3]]);
                let ee_len = u16::from_le_bytes([buf[base+4], buf[base+5]]);
                let ee_start_hi = u16::from_le_bytes([buf[base+6], buf[base+7]]);
                let ee_start_lo = u32::from_le_bytes([buf[base+8], buf[base+9], buf[base+10], buf[base+11]]);
                let actual_len = if ee_len > 32768 { ee_len - 32768 } else { ee_len } as u32;
                let phys = (ee_start_lo as u64 | ((ee_start_hi as u64) << 32)) as u32;
                extents[*count] = (ee_block, phys, actual_len);
                *count += 1;
            }
        } else {
            for i in 0..entries as usize {
                if *count >= extents.len() { return Ok(()); }
                let base = 12 + i * 12;
                let leaf_lo = u32::from_le_bytes([buf[base+4], buf[base+5], buf[base+6], buf[base+7]]);
                if leaf_lo != 0 {
                    self.ext4_fiemap_node(leaf_lo, owner, depth - 1, count, extents)?;
                }
            }
        }
        Ok(())
    }
}

// Node helpers
//
// An extent record and an index record are both twelve bytes starting with the
// logical block, the only field ordering and search use, so the helpers serve
// both, and leaf and index nodes split by the same code

/// One level of a descent: enough to decide later, without rereading, if it has
/// room
#[derive(Clone, Copy)]
struct Level {
    /// The node's block. Zero for the root, in the inode
    block:   u32,
    /// The entry the descent followed
    index:   usize,
    entries: u16,
    max:     u16,
}

impl Level {
    const EMPTY: Level = Level { block: 0, index: 0, entries: 0, max: 0 };
}

/// The root plus `EXTENT_MAX_DEPTH` levels below it
const MAX_LEVELS: usize = EXTENT_MAX_DEPTH as usize + 1;

#[inline]
fn read_u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[inline]
fn node_entries(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[2], b[3]])
}

#[inline]
fn node_max(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[4], b[5]])
}

#[inline]
fn node_depth(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[6], b[7]])
}

/// The child block of index entry `i` in an interior node
#[inline]
fn node_child(b: &[u8], i: usize) -> u32 {
    read_u32_at(b, 12 + i * 12 + 4)
}

fn extent_entry(logical: u32, len: u16, phys: u32) -> [u8; 12] {
    let mut e = [0u8; 12];
    e[0..4].copy_from_slice(&logical.to_le_bytes());
    e[4..6].copy_from_slice(&len.to_le_bytes());
    e[8..12].copy_from_slice(&phys.to_le_bytes());
    e
}

fn index_entry(logical: u32, child: u32) -> [u8; 12] {
    let mut e = [0u8; 12];
    e[0..4].copy_from_slice(&logical.to_le_bytes());
    e[4..8].copy_from_slice(&child.to_le_bytes());
    e
}

/// The entry a search for `logical` follows: the last starting at or before it,
/// or the first when all start after
fn pick_slot(b: &[u8], entries: u16, logical: u32) -> usize {
    let mut chosen = 0usize;
    for i in 0..entries as usize {
        if logical >= read_u32_at(b, 12 + i * 12) {
            chosen = i;
        }
    }
    chosen
}

fn pick_slot_inode(inode: &Inode, entries: u16, logical: u32) -> usize {
    let mut chosen = 0usize;
    for i in 0..entries as usize {
        if logical >= inode.extent_idx_at(i).block {
            chosen = i;
        }
    }
    chosen
}

fn insert_pos_inode(inode: &Inode, entries: u16, logical: u32) -> usize {
    for i in 0..entries as usize {
        if logical < inode.extent_idx_at(i).block {
            return i;
        }
    }
    entries as usize
}

/// Put `entry` in its place in a node that has room, shifting the rest up
fn node_insert_sorted(b: &mut [u8], entries: u16, entry: &[u8; 12]) {
    let key = read_u32_at(entry, 0);
    let n = entries as usize;
    let mut pos = n;
    for i in 0..n {
        if key < read_u32_at(b, 12 + i * 12) {
            pos = i;
            break;
        }
    }
    for i in (pos..n).rev() {
        let from = 12 + i * 12;
        let to = from + 12;
        let mut tmp = [0u8; 12];
        tmp.copy_from_slice(&b[from..from + 12]);
        b[to..to + 12].copy_from_slice(&tmp);
    }
    let at = 12 + pos * 12;
    b[at..at + 12].copy_from_slice(entry);
    b[2..4].copy_from_slice(&(entries + 1).to_le_bytes());
}

/// Whether `record` continues `existing`, and the merged length. Both must be
/// in one state, as an extent is written or unwritten as a whole. The ceiling
/// is 32767 for a written extent, since 32768 is the bit marking the other kind
fn merged_len(existing: &[u8], record: &[u8; 12]) -> Option<u16> {
    let raw = u16::from_le_bytes([existing[4], existing[5]]);
    let rec_raw = u16::from_le_bytes([record[4], record[5]]);
    let unwritten = raw > 32768;
    if unwritten != (rec_raw > 32768) {
        return None;
    }
    let len = if unwritten { raw - 32768 } else { raw } as u32;
    let rec_len = if unwritten { rec_raw - 32768 } else { rec_raw } as u32;
    if len == 0 || rec_len == 0 {
        return None;
    }
    let block = read_u32_at(existing, 0);
    let start = read_u32_at(existing, 8);
    if read_u32_at(record, 0) != block + len || read_u32_at(record, 8) != start + len {
        return None;
    }
    let total = len + rec_len;
    let ceiling = if unwritten { 32768 } else { 32767 };
    if total > ceiling {
        return None;
    }
    Some(if unwritten {
        (total + 32768) as u16
    } else {
        total as u16
    })
}

/// The twelve bytes an `Extent` would occupy on disk
fn extent_entry_raw(e: &Extent) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[0..4].copy_from_slice(&e.block.to_le_bytes());
    b[4..6].copy_from_slice(&e.len.to_le_bytes());
    b[6..8].copy_from_slice(&e.start_hi.to_le_bytes());
    b[8..12].copy_from_slice(&e.start_lo.to_le_bytes());
    b
}

/// A record for reserved blocks never written. The top bit of `ee_len` says
/// they hold nothing
pub fn unwritten_record(logical: u32, len: u16, phys: u32) -> [u8; 12] {
    unwritten_entry(logical, len, phys)
}

fn unwritten_entry(logical: u32, len: u16, phys: u32) -> [u8; 12] {
    let mut e = extent_entry(logical, len, phys);
    let marked = len as u32 + 32768;
    e[4..6].copy_from_slice(&(marked as u16).to_le_bytes());
    e
}
