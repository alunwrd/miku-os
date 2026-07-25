use crate::fs::ext::{FsError, MikuFS};

impl MikuFS {
    pub fn alloc_block(&mut self, preferred_group: usize) -> Result<u32, FsError> {
        let gc = self.group_count as usize;
        let bs = self.block_size as usize;

        for offset in 0..gc {
            let group = (preferred_group + offset) % gc;
            if group >= 32 { continue; }

            if self.groups[group].free_blocks() == 0 {
                continue;
            }

            let bitmap_block = self.groups[group].block_bitmap();
            let blocks_in_group = self.blocks_per_group;
            let bytes_to_scan = (((blocks_in_group + 7) / 8) as usize).min(bs);

            let mut buf = [0u8; 4096];
            self.read_block_into(bitmap_block, &mut buf[..bs])?;

            let hint = self.alloc_hint[group].min(blocks_in_group);
            let start_byte = (hint / 8) as usize;

            if let Some(result) = self.bitmap_find_free(
                &mut buf, start_byte, bytes_to_scan,
                blocks_in_group, bitmap_block, group, bs,
            )? {
                return Ok(result);
            }

            if start_byte > 0 {
                if let Some(result) = self.bitmap_find_free(
                    &mut buf, 0, start_byte.min(bytes_to_scan),
                    blocks_in_group, bitmap_block, group, bs,
                )? {
                    return Ok(result);
                }
            }
        }

        Err(FsError::NoSpace)
    }

    #[inline]
    fn bitmap_find_free(
        &mut self,
        buf: &mut [u8],
        from_byte: usize,
        to_byte: usize,
        blocks_in_group: u32,
        bitmap_block: u32,
        group: usize,
        bs: usize,
    ) -> Result<Option<u32>, FsError> {
        let mut byte_idx = from_byte;

        let aligned_start = (byte_idx + 7) & !7;
        while byte_idx < aligned_start && byte_idx < to_byte {
            if buf[byte_idx] != 0xFF {
                if let Some(r) = self.try_alloc_in_byte(
                    buf, byte_idx, blocks_in_group, bitmap_block, group, bs,
                )? {
                    return Ok(Some(r));
                }
            }
            byte_idx += 1;
        }

        while byte_idx + 8 <= to_byte {
            let word = u64::from_le_bytes([
                buf[byte_idx], buf[byte_idx+1], buf[byte_idx+2], buf[byte_idx+3],
                buf[byte_idx+4], buf[byte_idx+5], buf[byte_idx+6], buf[byte_idx+7],
            ]);
            if word != u64::MAX {
                for b in 0..8 {
                    if buf[byte_idx + b] != 0xFF {
                        if let Some(r) = self.try_alloc_in_byte(
                            buf, byte_idx + b, blocks_in_group, bitmap_block, group, bs,
                        )? {
                            return Ok(Some(r));
                        }
                    }
                }
            }
            byte_idx += 8;
        }

        while byte_idx < to_byte {
            if buf[byte_idx] != 0xFF {
                if let Some(r) = self.try_alloc_in_byte(
                    buf, byte_idx, blocks_in_group, bitmap_block, group, bs,
                )? {
                    return Ok(Some(r));
                }
            }
            byte_idx += 1;
        }

        Ok(None)
    }

    #[inline]
    fn try_alloc_in_byte(
        &mut self,
        buf: &mut [u8],
        byte_idx: usize,
        blocks_in_group: u32,
        bitmap_block: u32,
        group: usize,
        bs: usize,
    ) -> Result<Option<u32>, FsError> {
        let b = buf[byte_idx];
        let bit = b.trailing_ones();
        if bit >= 8 { return Ok(None); }

        let bit_index = byte_idx as u32 * 8 + bit;
        if bit_index >= blocks_in_group { return Ok(None); }

        buf[byte_idx] |= 1 << bit;
        self.write_block_data(bitmap_block, &buf[..bs])?;

        self.alloc_hint[group] = bit_index + 1;

        self.update_block_bitmap_csum(group)?;
        self.update_group_free_blocks(group, -1)?;
        self.update_superblock_free_blocks(-1)?;

        let absolute_block = group as u32 * self.blocks_per_group
            + bit_index + self.superblock.first_data_block();

        Ok(Some(absolute_block))
    }

    pub fn free_block(&mut self, block_num: u32) -> Result<(), FsError> {
        let first = self.superblock.first_data_block();
        if block_num < first { return Err(FsError::InvalidBlock); }

        let adjusted = block_num - first;
        let group = (adjusted / self.blocks_per_group) as usize;
        let bit = adjusted % self.blocks_per_group;

        if group >= self.group_count as usize || group >= 32 {
            return Err(FsError::InvalidBlock);
        }

        let bitmap_block = self.groups[group].block_bitmap();
        self.set_bitmap_bit(bitmap_block, bit, false)?;

        if group < 32 && bit < self.alloc_hint[group] {
            self.alloc_hint[group] = bit;
        }

        self.update_block_bitmap_csum(group)?;
        self.update_group_free_blocks(group, 1)?;
        self.update_superblock_free_blocks(1)?;
        Ok(())
    }

    pub fn alloc_inode(&mut self, preferred_group: usize) -> Result<u32, FsError> {
        let gc = self.group_count as usize;
        let bs = self.block_size as usize;

        for offset in 0..gc {
            let group = (preferred_group + offset) % gc;
            if self.groups[group].free_inodes() == 0 { continue; }

            let bitmap_block = self.groups[group].inode_bitmap();
            let inodes_in_group = self.inodes_per_group;
            let bytes_to_scan = (((inodes_in_group + 7) / 8) as usize).min(bs);

            let mut buf = [0u8; 4096];
            self.read_block_into(bitmap_block, &mut buf[..bs])?;

            for byte_idx in 0..bytes_to_scan {
                let b = buf[byte_idx];
                if b == 0xFF { continue; }
                let bit = b.trailing_ones();
                if bit >= 8 { continue; }
                let bit_index = byte_idx as u32 * 8 + bit;
                if bit_index >= inodes_in_group { break; }

                buf[byte_idx] |= 1 << bit;
                self.write_block_data(bitmap_block, &buf[..bs])?;
                self.update_inode_bitmap_csum(group)?;
                self.update_group_free_inodes(group, -1)?;
                self.update_superblock_free_inodes(-1)?;
                let inode_num = group as u32 * self.inodes_per_group + bit_index + 1;
                return Ok(inode_num);
            }
        }

        Err(FsError::NoSpace)
    }

    pub fn free_inode(&mut self, inode_num: u32) -> Result<(), FsError> {
        if inode_num == 0 { return Err(FsError::InvalidInode); }
        let idx = inode_num - 1;
        let group = (idx / self.inodes_per_group) as usize;
        let bit = idx % self.inodes_per_group;
        if group >= self.group_count as usize || group >= 32 {
            return Err(FsError::InvalidInode);
        }
        let bitmap_block = self.groups[group].inode_bitmap();
        self.set_bitmap_bit(bitmap_block, bit, false)?;
        self.update_inode_bitmap_csum(group)?;
        self.update_group_free_inodes(group, 1)?;
        self.update_superblock_free_inodes(1)?;
        Ok(())
    }

    pub fn set_bitmap_bit(
        &mut self, bitmap_block: u32, bit_index: u32, value: bool,
    ) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let byte_index = (bit_index / 8) as usize;
        let bit_offset = bit_index % 8;
        let mut buf = [0u8; 4096];
        self.read_block_into(bitmap_block, &mut buf[..bs])?;
        if value {
            buf[byte_index] |= 1 << bit_offset;
        } else {
            buf[byte_index] &= !(1 << bit_offset);
        }
        self.write_block_data(bitmap_block, &buf[..bs])?;
        Ok(())
    }

    pub fn update_group_free_blocks(&mut self, group: usize, delta: i16) -> Result<(), FsError> {
        if group >= 32 { return Err(FsError::InvalidBlock); }
        let current = self.groups[group].free_blocks();
        let new_val = (current as i32 + delta as i32).max(0) as u16;
        self.groups[group].write_u16(12, new_val);
        self.flush_group_desc(group)
    }

    pub fn update_group_free_inodes(&mut self, group: usize, delta: i16) -> Result<(), FsError> {
        if group >= 32 { return Err(FsError::InvalidInode); }
        let current = self.groups[group].free_inodes();
        let new_val = (current as i32 + delta as i32).max(0) as u16;
        self.groups[group].write_u16(14, new_val);
        self.flush_group_desc(group)
    }

    pub fn update_superblock_free_blocks(&mut self, delta: i32) -> Result<(), FsError> {
        let current = self.superblock.free_blocks_count();
        let new_val = (current as i64 + delta as i64).max(0) as u32;
        self.superblock.write_u32(12, new_val);
        self.flush_superblock()
    }

    pub fn update_superblock_free_inodes(&mut self, delta: i32) -> Result<(), FsError> {
        let current = self.superblock.free_inodes_count();
        let new_val = (current as i64 + delta as i64).max(0) as u32;
        self.superblock.write_u32(16, new_val);
        self.flush_superblock()
    }
}
