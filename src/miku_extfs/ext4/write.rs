use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

impl MikuFS {
    pub fn ext4_ensure_block(
        &mut self,
        inode: &mut Inode,
        inode_num: u32,
        logical_block: u32,
    ) -> Result<u32, FsError> {
        if !inode.uses_extents() {
            return self.ensure_block(inode, inode_num, logical_block);
        }
        let header = inode.extent_header();
        if !header.valid() {
            inode.init_extent_header(4);
        }
        let existing = self.get_file_block_extent(inode, logical_block)?;
        if existing != 0 {
            return Ok(existing);
        }
        let group = ((inode_num - 1) / self.inodes_per_group) as usize;
        let new_block = self.alloc_block(group)?;
        self.zero_block(new_block)?;
        match self.ext4_insert_extent(inode, inode_num, logical_block, new_block) {
            Ok(()) => {
                let blks = inode.blocks() + (self.block_size / 512);
                inode.set_blocks(blks);
                Ok(new_block)
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
        let mut inode = self.read_inode(inode_num)?;
        if !inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }
        if inode.has_flag(EXT4_IMMUTABLE_FL) {
            return Err(FsError::ReadOnlyFs);
        }
        if inode.has_flag(EXT4_APPEND_FL) && offset < inode.size() {
            return Err(FsError::ReadOnlyFs);
        }
        let bs = self.block_size as usize;
        let mut done = 0usize;
        while done < data.len() {
            let file_off = offset as usize + done;
            let logical_block = (file_off / bs) as u32;
            let block_off = file_off % bs;
            let chunk = (bs - block_off).min(data.len() - done);
            let phys_block = self.ext4_ensure_block(&mut inode, inode_num, logical_block)?;
            if chunk == bs && block_off == 0 {
                self.write_block_data(phys_block, &data[done..done + chunk])?;
            } else {
                let mut block_buf = [0u8; 4096];
                self.read_block_into(phys_block, &mut block_buf[..bs])?;
                block_buf[block_off..block_off + chunk].copy_from_slice(&data[done..done + chunk]);
                self.write_block_data(phys_block, &block_buf[..bs])?;
            }
            done += chunk;
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
        let mut inode = Inode::zeroed();
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
        let mut inode = Inode::zeroed();
        inode.init_dir_ext4(mode, 0, 0, now);
        inode.set_extent_at_raw(0, 0, 1, 0, dir_block);
        inode.set_extent_entries(1);
        inode.set_size(self.block_size);
        inode.set_blocks(self.block_size / 512);
        self.write_inode(new_ino, &inode)?;

        let bs = self.block_size as usize;
        let mut block_data = [0u8; 4096];
        let dot_rec_len = 12u16;
        let dotdot_rec_len = (bs as u16) - dot_rec_len;
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
        self.write_block_data(dir_block, &block_data[..bs])?;
        self.add_dir_entry(parent_ino, name, new_ino, FT_DIR)?;

        let mut parent_inode = self.read_inode(parent_ino)?;
        let links = parent_inode.links_count() + 1;
        parent_inode.set_links_count(links);
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        let gidx = ((new_ino - 1) / self.inodes_per_group) as usize;
        if gidx < 32 {
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
        inode.set_size(0);
        inode.set_blocks(0);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
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
            // last link - free blocks and inode
            if inode.uses_extents() {
                self.ext4_free_extent_blocks(&inode)?;
            } else if !inode.is_symlink() || !inode.is_fast_symlink() {
                self.free_all_blocks(&inode)?;
            }
            let now = self.get_timestamp();
            inode.set_dtime(now);
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
        if gidx < 32 {
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

        for logical in start_block..end_block {
            self.ext4_ensure_block(&mut inode, inode_num, logical)?;
            inode = self.read_inode(inode_num)?;
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
            let mut buf = [0u8; 512];
            while offset < size {
                let to_read = ((size - offset) as usize).min(512);
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
