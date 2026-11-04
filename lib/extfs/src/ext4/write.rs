use crate::structs::*;
use crate::{FsError, MikuFS};
use crate::host::ExtHost;

/// Whole blocks `ext4_write_parts` gathers for one write: 1 MiB of 4 KiB blocks
const WHOLE_BATCH: usize = 256;

/// An in-place overwrite at least this long goes to the device at once, in one
/// or more commands; a shorter one to the block layer's cache. A new block's
/// data always goes at once
const SYNC_OVERWRITE_BYTES: usize = 64 * 1024;

impl<H: ExtHost> MikuFS<H> {
    pub fn ext4_ensure_block(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
    ) -> Result<u32, FsError> {
        self.ext4_ensure_block_with(inode, inode_num, logical_block, None).map(|(b, _)| b)
    }

    /// `ext4_ensure_block` with the whole block's contents when known: a block
    /// allocated here gets them instead of zeros, and `true` says so
    pub(crate) fn ext4_ensure_block_with(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
        fill: Option<&[u8]>,
    ) -> Result<(u32, bool), FsError> {
        if !inode.uses_extents() {
            return self.ensure_block(inode, inode_num, logical_block).map(|b| (b, false));
        }
        let header = inode.extent_header();
        if !header.valid() {
            inode.init_extent_header(4);
        }
        let owner = Some((inode_num, inode.generation()));
        let (existing, unwritten) = self.ext4_map_block(inode, owner, logical_block)?;
        if existing != 0 {
            // Reserved by fallocate and unwritten: already ours, only the zeros
            // claim ends
            if unwritten {
                return self.ext4_convert_to_written(inode, inode_num, logical_block).map(|b| (b, false));
            }
            return Ok((existing, false));
        }
        let group = ((inode_num - 1) / self.inodes_per_group) as usize;
        let new_block = self.alloc_block(group)?;
        // Filled before the extent names it
        let filled = match fill {
            Some(data) => self.write_block_data(new_block, data),
            None => self.zero_block(new_block),
        };
        if let Err(e) = filled {
            let _ = self.free_block(new_block);
            return Err(e);
        }
        match self.ext4_insert_extent(inode, inode_num, logical_block, new_block) {
            Ok(()) => {
                let blks = inode.blocks_full() + (self.block_size / 512) as u64;
                inode.set_blocks_full(blks);
                Ok((new_block, fill.is_some()))
            }
            Err(e) => {
                let _ = self.free_block(new_block);
                Err(e)
            }
        }
    }

    pub fn ext4_write_file(
        &mut self,
        inode_num: u32,
        data: &[u8],
        offset: u64,
    ) -> Result<usize, FsError> {
        self.ext4_write_parts(inode_num, &[data], offset)
    }

    /// `parts` back to back from `offset`, reading and writing the inode once:
    /// the page cache's write-out of a run of pages
    pub fn ext4_write_parts(
        &mut self,
        inode_num: u32,
        parts: &[&[u8]],
        offset: u64,
    ) -> Result<usize, FsError> {
        let mut inode = self.read_inode(inode_num)?;
        if !inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }
        if inode.has_flag(EXT4_IMMUTABLE_FL) {
            return Err(FsError::ReadOnlyFs);
        }
        // An inline file's `i_block` is its contents, not block numbers
        if inode.has_inline_data() {
            return match parts {
                [data] => self.ext4_write_inline(inode_num, data, offset),
                _ => self.ext4_write_inline(inode_num, &parts.concat(), offset),
            };
        }
        if inode.has_flag(EXT4_APPEND_FL) && offset < inode.size() {
            return Err(FsError::ReadOnlyFs);
        }
        let bs = self.block_size as usize;
        // Whole blocks go past the block cache a batch at a time
        // (`ext4_write_whole`) when the map is an extent tree
        let direct = inode.uses_extents();
        if direct && !inode.extent_header().valid() {
            inode.init_extent_header(4);
        }
        let mut batch: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::new();
        let mut batch_at = 0u32;
        let mut done = 0usize;
        for data in parts {
            let mut at = 0usize;
            while at < data.len() {
                let file_off = offset as usize + done;
                let logical_block = (file_off / bs) as u32;
                let block_off = file_off % bs;
                let chunk = (bs - block_off).min(data.len() - at);
                let piece = &data[at..at + chunk];
                if chunk == bs && direct {
                    if batch.is_empty() {
                        batch_at = logical_block;
                    }
                    batch.push(piece);
                    if batch.len() == WHOLE_BATCH {
                        self.ext4_write_whole(&mut inode, inode_num, batch_at, &batch)?;
                        batch.clear();
                    }
                    at += chunk;
                    done += chunk;
                    continue;
                }
                if !batch.is_empty() {
                    self.ext4_write_whole(&mut inode, inode_num, batch_at, &batch)?;
                    batch.clear();
                }
                if chunk == bs {
                    let (phys_block, filled) =
                        self.ext4_ensure_block_with(&mut inode, inode_num, logical_block, Some(piece))?;
                    if !filled {
                        self.write_block_data(phys_block, piece)?;
                    }
                } else {
                    let phys_block = self.ext4_ensure_block(&mut inode, inode_num, logical_block)?;
                    let mut block_buf = [0u8; 4096];
                    self.read_block_into(phys_block, &mut block_buf[..bs])?;
                    block_buf[block_off..block_off + chunk].copy_from_slice(piece);
                    self.write_block_data(phys_block, &block_buf[..bs])?;
                }
                at += chunk;
                done += chunk;
            }
        }
        if !batch.is_empty() {
            self.ext4_write_whole(&mut inode, inode_num, batch_at, &batch)?;
        }
        let new_end = offset as u64 + done as u64;
        if new_end > inode.size() {
            inode.set_size_full(new_end);
        }
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(done)
    }

    /// Whole blocks from `logical`, past the block cache. A hole is allocated a
    /// run at a time, with data on the device before the extent is in the tree;
    /// existing blocks are written in place. A reserved, unwritten block goes
    /// through `ext4_ensure_block_with`
    fn ext4_write_whole(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical: u32,
        blocks: &[&[u8]],
    ) -> Result<(), FsError> {
        let owner = Some((inode_num, inode.generation()));
        let group = ((inode_num - 1) / self.inodes_per_group) as usize;
        let spb = (self.block_size / 512) as u64;
        let mut i = 0usize;
        while i < blocks.len() {
            let lb = logical + i as u32;
            let (phys, unwritten) = self.ext4_map_block(inode, owner, lb)?;
            if phys != 0 && unwritten {
                let (b, filled) = self.ext4_ensure_block_with(inode, inode_num, lb, Some(blocks[i]))?;
                if !filled {
                    self.write_block_data(b, blocks[i])?;
                }
                i += 1;
                continue;
            }
            // Extent of the same kind: a hole, or written blocks consecutive on
            // disk too
            let mut n = 1usize;
            while i + n < blocks.len() {
                let (p, u) = self.ext4_map_block(inode, owner, lb + n as u32)?;
                let same = if phys == 0 { p == 0 } else { !u && p == phys + n as u32 };
                if !same {
                    break;
                }
                n += 1;
            }
            if phys != 0 {
                let sync = n * self.block_size as usize >= SYNC_OVERWRITE_BYTES;
                self.write_data_blocks(phys, &blocks[i..i + n], sync)?;
                i += n;
                continue;
            }
            let (start, got) = self.alloc_blocks(group, n as u32)?;
            let got = got as usize;
            let placed = self
                .write_data_blocks(start, &blocks[i..i + got], true)
                .and_then(|()| self.ext4_insert_extent_run(inode, inode_num, lb, got as u16, start));
            if let Err(e) = placed {
                for b in start..start + got as u32 {
                    let _ = self.free_block(b);
                }
                return Err(e);
            }
            inode.set_blocks_full(inode.blocks_full() + got as u64 * spb);
            i += got;
        }
        Ok(())
    }

    pub fn ext4_create_file(
        &mut self,
        parent_ino: u32,
        name: &str,
        mode: u16,
    ) -> Result<u32, FsError> {
        let parent = self.read_inode(parent_ino)?;
        if !parent.is_directory() {
            return Err(FsError::NotDirectory);
        }
        if self.ext2_lookup_in_dir(parent_ino, name)?.is_some() {
            return Err(FsError::AlreadyExists);
        }
        let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        let new_ino = self.alloc_inode(group)?;
        let now = self.get_timestamp();
        let mut inode = self.fresh_inode();
        inode.init_file_ext4(mode, 0, 0, now);
        self.write_inode(new_ino, &inode)?;
        self.add_dir_entry(parent_ino, name, new_ino, FT_REG_FILE)?;

        // update parent dir timestamps
        let mut parent_inode = self.read_inode(parent_ino)?;
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        Ok(new_ino)
    }

    pub fn ext4_create_dir(
        &mut self,
        parent_ino: u32,
        name: &str,
        mode: u16,
    ) -> Result<u32, FsError> {
        let parent = self.read_inode(parent_ino)?;
        if !parent.is_directory() {
            return Err(FsError::NotDirectory);
        }
        if self.ext2_lookup_in_dir(parent_ino, name)?.is_some() {
            return Err(FsError::AlreadyExists);
        }
        let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        let new_ino = self.alloc_inode(group)?;
        let dir_block = self.alloc_block(group)?;
        let now = self.get_timestamp();
        let mut inode = self.fresh_inode();
        inode.init_dir_ext4(mode, 0, 0, now);
        inode.set_extent_at_raw(0, 0, 1, 0, dir_block);
        inode.set_extent_entries(1);
        inode.set_size(self.block_size);
        inode.set_blocks(self.block_size / 512);
        self.write_inode(new_ino, &inode)?;

        let bs = self.block_size as usize;
        let mut block_data = [0u8; 4096];
        // As in ext2: ".." runs to the end of the usable area, stopping one
        // tail short on metadata_csum
        let usable = self.dir_block_usable();
        self.init_dirent_tail(&mut block_data[..bs]);
        let dot_rec_len = 12u16;
        let dotdot_rec_len = usable as u16 - dot_rec_len;
        block_data[0..4].copy_from_slice(&new_ino.to_le_bytes());
        block_data[4..6].copy_from_slice(&dot_rec_len.to_le_bytes());
        block_data[6] = 1;
        block_data[7] = FT_DIR;
        block_data[8] = b'.';
        let off2 = dot_rec_len as usize;
        block_data[off2..off2 + 4].copy_from_slice(&parent_ino.to_le_bytes());
        block_data[off2 + 4..off2 + 6].copy_from_slice(&dotdot_rec_len.to_le_bytes());
        block_data[off2 + 6] = 2;
        block_data[off2 + 7] = FT_DIR;
        block_data[off2 + 8] = b'.';
        block_data[off2 + 9] = b'.';
        self.write_dir_block(new_ino, dir_block, &mut block_data)?;
        self.add_dir_entry(parent_ino, name, new_ino, FT_DIR)?;

        let mut parent_inode = self.read_inode(parent_ino)?;
        // Each subdirectory's ".." adds a link to its parent, so creating
        // directories can reach the u16 ceiling
        if parent_inode.links_count() >= EXT2_LINK_MAX {
            return Err(FsError::TooManyLinks);
        }
        let links = parent_inode.links_count() + 1;
        parent_inode.set_links_count(links);
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        let gidx = ((new_ino - 1) / self.inodes_per_group) as usize;
        if gidx < self.groups.len() {
            self.groups[gidx].inc_used_dirs();
            self.flush_group_desc(gidx)?;
        }
        Ok(new_ino)
    }

    pub fn ext4_truncate(&mut self, inode_num: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        if inode.uses_extents() {
            self.ext4_free_extent_blocks(&inode)?;
            inode.clear_block_pointers();
            inode.init_extent_header(4);
        } else {
            self.free_all_blocks(&inode)?;
            for i in 0..15 {
                inode.set_block(i, 0);
            }
        }
        inode.set_size_full(0);
        inode.set_blocks_full(0);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
    }

    /// Truncates an extent-mapped file to any length. Growing is sparse.
    /// Shrinking walks the tree and drops blocks past the new end, with no
    /// rebalancing: a tree with spare room is valid
    pub fn ext4_truncate_to(&mut self, inode_num: u32, new_size: u64) -> Result<(), FsError> {
        if new_size == 0 {
            return self.ext4_truncate(inode_num);
        }

        let inode = self.read_inode(inode_num)?;
        if !inode.uses_extents() {
            return self.ext2_truncate_to(inode_num, new_size);
        }

        let old_size = inode.size();
        if new_size >= old_size {
            let mut inode = inode;
            inode.set_size_full(new_size);
            let now = self.get_timestamp();
            inode.set_mtime(now);
            inode.set_ctime(now);
            return self.write_inode(inode_num, &inode);
        }

        let bs = self.block_size as u64;
        let first_free = new_size.div_ceil(bs) as u32;
        let old_blocks = old_size.div_ceil(bs) as u32;

        // Frees blocks past the new end one logical block at a time, so the
        // tree never describes blocks back in the pool
        let mut freed = 0u64;
        for logical in first_free..old_blocks {
            let inode = self.read_inode(inode_num)?;
            let owner = Some((inode_num, inode.generation()));
            // The map, not the lookup: an unwritten block is still owned here,
            // and skipping them would leak `fallocate`'s reservation
            let (phys, _) = self.ext4_map_block(&inode, owner, logical)?;
            if phys == 0 {
                continue;
            }
            self.free_block(phys)?;
            freed += 1;
        }
        self.ext4_trim_extents_past(inode_num, first_free)?;

        let tail = (new_size % bs) as usize;
        if tail != 0 {
            let inode = self.read_inode(inode_num)?;
            let last = (new_size / bs) as u32;
            let owner = Some((inode_num, inode.generation()));
            let phys = self.get_file_block_extent_owned(&inode, owner, last)?;
            if phys != 0 {
                let bsz = self.block_size as usize;
                let mut buf = [0u8; 4096];
                self.read_block_into(phys, &mut buf[..bsz])?;
                for b in buf[tail..bsz].iter_mut() {
                    *b = 0;
                }
                self.write_block_data(phys, &buf[..bsz])?;
            }
        }

        let mut inode = self.read_inode(inode_num)?;
        inode.set_size_full(new_size);
        let per_block = (self.block_size / 512) as u64;
        let blocks = inode.blocks_full().saturating_sub(freed * per_block);
        inode.set_blocks_full(blocks);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
    }

    /// Drops or shortens each extent in a depth-0 root at or past `first_free`.
    /// Deeper trees keep their entries; blocks are freed, and reads are bounded
    /// by the size
    fn ext4_trim_extents_past(&mut self, inode_num: u32, first_free: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        let header = self.extent_root(&inode)?;
        if header.depth != 0 {
            return Ok(());
        }

        let mut kept = 0u16;
        for i in 0..header.entries as usize {
            let ext = inode.extent_at(i);
            let len = ext.actual_len();
            if ext.block >= first_free {
                continue;
            }
            let new_len = if ext.block + len > first_free {
                (first_free - ext.block) as u16
            } else {
                len as u16
            };
            inode.set_extent_at_raw(
                kept as usize,
                ext.block,
                new_len,
                ext.start_hi,
                ext.start_lo,
            );
            kept += 1;
        }
        inode.set_extent_entries(kept);
        self.write_inode(inode_num, &inode)
    }

    pub fn ext4_delete_file(&mut self, parent_ino: u32, name: &str) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };
        let mut inode = self.read_inode(target_ino)?;
        if inode.is_directory() {
            return Err(FsError::IsDirectory);
        }

        self.remove_dir_entry(parent_ino, name)?;

        let links = inode.links_count();
        if links <= 1 {
            // Last link: frees blocks and inode
            if inode.uses_extents() {
                self.ext4_free_extent_blocks(&inode)?;
            } else if !inode.is_symlink() || !inode.is_fast_symlink() {
                self.free_all_blocks(&inode)?;
            }
            let now = self.deletion_timestamp();
            inode.set_dtime(now);
            inode.set_links_count(0);
            self.write_inode(target_ino, &inode)?;
            self.free_inode(target_ino)?;
        } else {
            // decrement link count
            inode.set_links_count(links - 1);
            let now = self.get_timestamp();
            inode.set_ctime(now);
            self.write_inode(target_ino, &inode)?;
        }

        // update parent dir timestamps
        let now = self.get_timestamp();
        let mut parent_inode = self.read_inode(parent_ino)?;
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        Ok(())
    }

    pub fn ext4_delete_dir(&mut self, parent_ino: u32, name: &str) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };
        let inode = self.read_inode(target_ino)?;
        if !inode.is_directory() {
            return Err(FsError::NotDirectory);
        }
        if !self.is_ext2_dir_empty(target_ino)? {
            return Err(FsError::NotEmpty);
        }
        if inode.uses_extents() {
            self.ext4_free_extent_blocks(&inode)?;
        } else {
            self.free_all_blocks(&inode)?;
        }
        self.free_inode(target_ino)?;
        self.remove_dir_entry(parent_ino, name)?;
        let now = self.get_timestamp();
        let mut parent_inode = self.read_inode(parent_ino)?;
        let links = parent_inode.links_count();
        if links > 1 {
            parent_inode.set_links_count(links - 1);
        }
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;
        let gidx = ((target_ino - 1) / self.inodes_per_group) as usize;
        if gidx < self.groups.len() {
            self.groups[gidx].dec_used_dirs();
            self.flush_group_desc(gidx)?;
        }
        Ok(())
    }

    // preallocate blocks for a file (fallocate)
    pub fn ext4_fallocate(
        &mut self,
        inode_num: u32,
        offset: u64,
        len: u64,
    ) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        if !inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }
        let bs = self.block_size as u64;
        let start_block = (offset / bs) as u32;
        let end_block = ((offset + len + bs - 1) / bs) as u32;

        if !inode.uses_extents() {
            // Without extents nothing records a reserved, unwritten block, so
            // blocks must be real
            for logical in start_block..end_block {
                self.ext4_ensure_block(&mut inode, inode_num, logical)?;
                inode = self.read_inode(inode_num)?;
            }
        } else {
            // With extents, reserves blocks and marks them unwritten, writing
            // no data
            let group = ((inode_num - 1) / self.inodes_per_group) as usize;
            let owner = Some((inode_num, inode.generation()));
            let mut logical = start_block;
            while logical < end_block {
                let (existing, _) = self.ext4_map_block(&inode, owner, logical)?;
                if existing != 0 {
                    logical += 1;
                    continue;
                }
                let new_block = self.alloc_block(group)?;
                let record = crate::ext4::extents::unwritten_record(logical, 1, new_block);
                if let Err(e) = self.ext4_insert_extent_record(&mut inode, inode_num, &record, true)
                {
                    let _ = self.free_block(new_block);
                    self.write_inode(inode_num, &inode)?;
                    return Err(e);
                }
                let blks = inode.blocks_full() + (self.block_size / 512) as u64;
                inode.set_blocks_full(blks);
                logical += 1;
            }
        }

        let new_end = offset + len;
        if new_end > inode.size() {
            inode.set_size_full(new_end);
        }
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
    }

    pub fn ext4_append_file(&mut self, inode_num: u32, data: &[u8]) -> Result<usize, FsError> {
        let inode = self.read_inode(inode_num)?;
        let offset = inode.size();
        self.ext4_write_file(inode_num, data, offset)
    }

    pub fn ext4_copy_file(
        &mut self,
        src_ino: u32,
        dst_parent_ino: u32,
        dst_name: &str,
    ) -> Result<u32, FsError> {
        let src_inode = self.read_inode(src_ino)?;
        if !src_inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }
        let size = src_inode.size();
        let mode = src_inode.permissions();
        let new_ino = if self.superblock.has_extents() {
            self.ext4_create_file(dst_parent_ino, dst_name, mode)?
        } else {
            self.ext2_create_file(dst_parent_ino, dst_name, mode)?
        };
        if size > 0 {
            let mut offset = 0u64;
            // A block at a time, avoiding read-modify-write per 512-byte piece
            let bs = self.block_size as usize;
            let mut buf = [0u8; 4096];
            while offset < size {
                let to_read = ((size - offset) as usize).min(bs);
                let src_inode = self.read_inode(src_ino)?;
                let n = self.read_file(&src_inode, offset, &mut buf[..to_read])?;
                if n == 0 {
                    break;
                }
                if self.superblock.has_extents() {
                    self.ext4_write_file(new_ino, &buf[..n], offset)?;
                } else {
                    self.ext2_write_file(new_ino, &buf[..n], offset)?;
                }
                offset += n as u64;
            }
        }
        Ok(new_ino)
    }
}
