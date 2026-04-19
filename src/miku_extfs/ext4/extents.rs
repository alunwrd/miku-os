use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

impl MikuFS {
    pub fn get_file_block_extent(
        &mut self,
        inode: &Inode,
        logical_block: u32,
    ) -> Result<u32, FsError> {
        let header = inode.extent_header();
        if !header.valid() {
            return Err(FsError::CorruptedFs);
        }
        if header.depth == 0 {
            for i in 0..header.entries as usize {
                let ext = inode.extent_at(i);
                if logical_block >= ext.block && logical_block < ext.block + ext.actual_len() {
                    let offset = logical_block - ext.block;
                    return Ok((ext.start() + offset as u64) as u32);
                }
            }
            return Ok(0);
        }
        let mut target_block = 0u64;
        for i in 0..header.entries as usize {
            let idx = inode.extent_idx_at(i);
            if logical_block >= idx.block {
                target_block = idx.leaf();
            }
        }
        if target_block == 0 {
            return Ok(0);
        }
        self.search_extent_tree(target_block as u32, logical_block, header.depth - 1)
    }

    fn search_extent_tree(
        &mut self,
        block_num: u32,
        logical_block: u32,
        depth: u16,
    ) -> Result<u32, FsError> {
        let mut buf = [0u8; 4096];
        let bs = self.block_size as usize;
        self.read_block_into(block_num, &mut buf[..bs])?;
        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        let entries = u16::from_le_bytes([buf[2], buf[3]]);
        if magic != EXT4_EXT_MAGIC {
            return Err(FsError::CorruptedFs);
        }
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
                    return Ok((start + offset as u64) as u32);
                }
            }
            return Ok(0);
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
            return Ok(0);
        }
        self.search_extent_tree(target_block as u32, logical_block, depth - 1)
    }

    pub fn ext4_insert_extent(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
        phys_block: u32,
    ) -> Result<(), FsError> {
        let header = inode.extent_header();
        if !header.valid() {
            return Err(FsError::CorruptedFs);
        }
        if header.depth == 0 {
            return self.ext4_insert_extent_leaf(inode, inode_num, logical_block, phys_block);
        }
        self.ext4_insert_extent_deep(inode, inode_num, logical_block, phys_block, header.depth)
    }

    fn ext4_insert_extent_leaf(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
        phys_block: u32,
    ) -> Result<(), FsError> {
        let header = inode.extent_header();
        let entries = header.entries;
        let max = header.max;

        if entries > 0 {
            let last = (entries - 1) as usize;
            let ext = inode.extent_at(last);
            let end_logical = ext.block + ext.actual_len();
            let end_physical = (ext.start() + ext.actual_len() as u64) as u32;
            if logical_block == end_logical
                && phys_block == end_physical
                && ext.actual_len() < 32767
            {
                inode.set_extent_len_at(last, (ext.actual_len() + 1) as u16);
                return Ok(());
            }
        }

        if entries < max {
            let mut insert_pos = entries as usize;
            for i in 0..entries as usize {
                if logical_block < inode.extent_at(i).block {
                    insert_pos = i;
                    break;
                }
            }
            if insert_pos < entries as usize {
                for i in (insert_pos..entries as usize).rev() {
                    let e = inode.extent_at(i);
                    inode.set_extent_at_raw(i + 1, e.block, e.len, e.start_hi, e.start_lo);
                }
            }
            inode.set_extent_at_raw(insert_pos, logical_block, 1, 0, phys_block);
            inode.set_extent_entries(entries + 1);
            return Ok(());
        }

        self.ext4_grow_extent_tree(inode, inode_num)?;
        self.ext4_insert_extent(inode, inode_num, logical_block, phys_block)
    }

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
        let max_entries = ((bs - 12) / 12) as u16;

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
        self.write_block_data(new_block, &leaf_data[..bs])?;

        let first_logical = if old_entries > 0 {
            inode.extent_at(0).block
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

        let blks = inode.blocks() + (self.block_size / 512);
        inode.set_blocks(blks);

        Ok(())
    }

    fn ext4_insert_extent_deep(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
        phys_block: u32,
        _depth: u16,
    ) -> Result<(), FsError> {
        let header = inode.extent_header();
        let mut target_idx = 0usize;
        for i in 0..header.entries as usize {
            let idx = inode.extent_idx_at(i);
            if logical_block >= idx.block {
                target_idx = i;
            }
        }
        let idx_entry = inode.extent_idx_at(target_idx);
        let child_block = idx_entry.leaf() as u32;
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(child_block, &mut buf[..bs])?;

        let child_magic = u16::from_le_bytes([buf[0], buf[1]]);
        let child_entries = u16::from_le_bytes([buf[2], buf[3]]);
        let child_max = u16::from_le_bytes([buf[4], buf[5]]);
        let child_depth = u16::from_le_bytes([buf[6], buf[7]]);

        if child_magic != EXT4_EXT_MAGIC {
            return Err(FsError::CorruptedFs);
        }

        if child_depth == 0 {
            if child_entries > 0 {
                let last_base = 12 + (child_entries as usize - 1) * 12;
                let ext_block = u32::from_le_bytes([
                    buf[last_base],
                    buf[last_base + 1],
                    buf[last_base + 2],
                    buf[last_base + 3],
                ]);
                let ext_len = u16::from_le_bytes([buf[last_base + 4], buf[last_base + 5]]);
                let ext_start_lo = u32::from_le_bytes([
                    buf[last_base + 8],
                    buf[last_base + 9],
                    buf[last_base + 10],
                    buf[last_base + 11],
                ]);
                let actual_len = if ext_len > 32768 {
                    ext_len - 32768
                } else {
                    ext_len
                } as u32;
                let end_logical = ext_block + actual_len;
                let end_physical = ext_start_lo + actual_len;
                if logical_block == end_logical && phys_block == end_physical && actual_len < 32767
                {
                    let new_len = (actual_len + 1) as u16;
                    buf[last_base + 4..last_base + 6].copy_from_slice(&new_len.to_le_bytes());
                    self.write_block_data(child_block, &buf[..bs])?;
                    return Ok(());
                }
            }

            if child_entries < child_max {
                let new_base = 12 + child_entries as usize * 12;
                buf[new_base..new_base + 4].copy_from_slice(&logical_block.to_le_bytes());
                buf[new_base + 4..new_base + 6].copy_from_slice(&1u16.to_le_bytes());
                buf[new_base + 6..new_base + 8].copy_from_slice(&0u16.to_le_bytes());
                buf[new_base + 8..new_base + 12].copy_from_slice(&phys_block.to_le_bytes());
                let new_entries = child_entries + 1;
                buf[2..4].copy_from_slice(&new_entries.to_le_bytes());
                self.write_block_data(child_block, &buf[..bs])?;
                return Ok(());
            }

            return self.ext4_split_leaf(
                inode,
                inode_num,
                child_block,
                &buf[..bs],
                logical_block,
                phys_block,
                target_idx,
            );
        }

        let mut sub_target = 0usize;
        for i in 0..child_entries as usize {
            let base = 12 + i * 12;
            let ei_block =
                u32::from_le_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]]);
            if logical_block >= ei_block {
                sub_target = i;
            }
        }
        let sub_base = 12 + sub_target * 12;
        let sub_leaf_lo = u32::from_le_bytes([
            buf[sub_base + 4],
            buf[sub_base + 5],
            buf[sub_base + 6],
            buf[sub_base + 7],
        ]);
        let sub_leaf_hi = u16::from_le_bytes([buf[sub_base + 8], buf[sub_base + 9]]);
        let sub_child = (sub_leaf_lo as u64) | ((sub_leaf_hi as u64) << 32);

        let mut sub_buf = [0u8; 4096];
        self.read_block_into(sub_child as u32, &mut sub_buf[..bs])?;
        let sub_entries = u16::from_le_bytes([sub_buf[2], sub_buf[3]]);
        let sub_max = u16::from_le_bytes([sub_buf[4], sub_buf[5]]);

        if sub_entries < sub_max {
            let new_base = 12 + sub_entries as usize * 12;
            sub_buf[new_base..new_base + 4].copy_from_slice(&logical_block.to_le_bytes());
            sub_buf[new_base + 4..new_base + 6].copy_from_slice(&1u16.to_le_bytes());
            sub_buf[new_base + 6..new_base + 8].copy_from_slice(&0u16.to_le_bytes());
            sub_buf[new_base + 8..new_base + 12].copy_from_slice(&phys_block.to_le_bytes());
            let ne = sub_entries + 1;
            sub_buf[2..4].copy_from_slice(&ne.to_le_bytes());
            self.write_block_data(sub_child as u32, &sub_buf[..bs])?;
            return Ok(());
        }

        Err(FsError::ExtentFull)
    }

    fn ext4_split_leaf(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        old_block: u32,
        old_data: &[u8],
        logical_block: u32,
        phys_block: u32,
        _parent_idx: usize,
    ) -> Result<(), FsError> {
        let group = ((inode_num - 1) / self.inodes_per_group) as usize;
        let new_block = self.alloc_block(group)?;
        let bs = self.block_size as usize;
        let old_entries = u16::from_le_bytes([old_data[2], old_data[3]]);
        let max_entries = ((bs - 12) / 12) as u16;
        let split_at = old_entries / 2;

        let mut new_data = [0u8; 4096];
        new_data[0..2].copy_from_slice(&EXT4_EXT_MAGIC.to_le_bytes());
        let new_count = old_entries - split_at;
        new_data[2..4].copy_from_slice(&new_count.to_le_bytes());
        new_data[4..6].copy_from_slice(&max_entries.to_le_bytes());
        new_data[6..8].copy_from_slice(&0u16.to_le_bytes());
        new_data[8..12].copy_from_slice(&0u32.to_le_bytes());
        for i in 0..new_count as usize {
            let src = 12 + (split_at as usize + i) * 12;
            let dst = 12 + i * 12;
            new_data[dst..dst + 12].copy_from_slice(&old_data[src..src + 12]);
        }
        self.write_block_data(new_block, &new_data[..bs])?;

        let mut updated_old = [0u8; 4096];
        updated_old[..bs].copy_from_slice(&old_data[..bs]);
        updated_old[2..4].copy_from_slice(&split_at.to_le_bytes());
        self.write_block_data(old_block, &updated_old[..bs])?;

        let new_first_block =
            u32::from_le_bytes([new_data[12], new_data[13], new_data[14], new_data[15]]);

        let header = inode.extent_header();
        if header.entries < header.max {
            let pos = header.entries as usize;
            inode.set_extent_idx_at_raw(pos, new_first_block, new_block, 0);
            inode.set_extent_entries(header.entries + 1);
        }

        let blks = inode.blocks() + (self.block_size / 512);
        inode.set_blocks(blks);

        self.ext4_insert_extent(inode, inode_num, logical_block, phys_block)
    }

    pub fn ext4_free_extent_blocks(&mut self, inode: &Inode) -> Result<u32, FsError> {
        let header = inode.extent_header();
        if !header.valid() {
            return Ok(0);
        }
        if header.depth == 0 {
            let mut freed = 0u32;
            for i in 0..header.entries as usize {
                let ext = inode.extent_at(i);
                let start = ext.start() as u32;
                let len = ext.actual_len();
                for b in 0..len {
                    let _ = self.free_block(start + b);
                    freed += 1;
                }
            }
            return Ok(freed);
        }
        let mut freed = 0u32;
        for i in 0..header.entries as usize {
            let idx = inode.extent_idx_at(i);
            let child = idx.leaf() as u32;
            if child != 0 {
                freed += self.ext4_free_extent_tree_block(child, header.depth - 1)?;
                let _ = self.free_block(child);
                freed += 1;
            }
        }
        Ok(freed)
    }

    fn ext4_free_extent_tree_block(&mut self, block_num: u32, depth: u16) -> Result<u32, FsError> {
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(block_num, &mut buf[..bs])?;
        let entries = u16::from_le_bytes([buf[2], buf[3]]);
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
                for b in 0..actual_len {
                    let _ = self.free_block(ee_start_lo + b);
                    freed += 1;
                }
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
                    freed += self.ext4_free_extent_tree_block(leaf_lo, depth - 1)?;
                    let _ = self.free_block(leaf_lo);
                    freed += 1;
                }
            }
        }
        Ok(freed)
    }

    pub fn ext4_extent_count(&mut self, inode: &Inode) -> Result<u32, FsError> {
        let header = inode.extent_header();
        if !header.valid() {
            return Ok(0);
        }
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
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(block_num, &mut buf[..bs])?;
        let entries = u16::from_le_bytes([buf[2], buf[3]]);
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

    // fiemap - return the extent map for a file
    // returns array of (logical_block, physical_block, length) tuples
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

        // extent-based file: read directly from extent tree
        let header = inode.extent_header();
        if !header.valid() {
            return Err(FsError::CorruptedFs);
        }

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

        // multi-level tree: collect leaf extents
        let mut count = 0usize;
        self.ext4_fiemap_tree(&inode, header.depth, &mut count, extents)?;
        Ok(count)
    }

    fn ext4_fiemap_tree(
        &mut self,
        inode: &Inode,
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
            self.ext4_fiemap_node(leaf, depth - 1, count, extents)?;
        }
        Ok(())
    }

    fn ext4_fiemap_node(
        &mut self,
        block_num: u32,
        depth: u16,
        count: &mut usize,
        extents: &mut [(u32, u32, u32)],
    ) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(block_num, &mut buf[..bs])?;

        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        let entries = u16::from_le_bytes([buf[2], buf[3]]);
        if magic != EXT4_EXT_MAGIC { return Err(FsError::CorruptedFs); }

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
                    self.ext4_fiemap_node(leaf_lo, depth - 1, count, extents)?;
                }
            }
        }
        Ok(())
    }
}
