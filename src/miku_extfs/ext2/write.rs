use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

pub struct TreeEntry {
    pub name: [u8; 60],
    pub name_len: u8,
    pub depth: u8,
    pub is_last: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
}

impl TreeEntry {
    pub const fn empty() -> Self {
        Self {
            name: [0; 60], name_len: 0, depth: 0,
            is_last: false, is_dir: false, is_symlink: false, size: 0,
        }
    }
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }
}

pub struct TreeResult {
    pub entries: [TreeEntry; 128],
    pub count: usize,
    pub max: usize,
}

impl TreeResult {
    pub const fn new() -> Self {
        Self { entries: [const { TreeEntry::empty() }; 128], count: 0, max: 128 }
    }
}

pub struct FsckResult {
    pub checked: bool,
    pub errors: u32,
    pub total_inodes: u32,
    pub total_blocks: u32,
    pub free_inodes: u32,
    pub free_blocks: u32,
    pub used_inodes: u32,
    pub block_size: u32,
    pub inode_size: u32,
    pub bad_magic: bool,
    pub root_ok: bool,
    pub root_not_dir: bool,
    pub bad_groups: u32,
    pub orphan_inodes: u32,
    pub group_free_blocks: [u16; 32],
    pub group_free_inodes: [u16; 32],
}

impl FsckResult {
    pub const fn new() -> Self {
        Self {
            checked: false, errors: 0, total_inodes: 0, total_blocks: 0,
            free_inodes: 0, free_blocks: 0, used_inodes: 0, block_size: 0,
            inode_size: 0, bad_magic: false, root_ok: false, root_not_dir: false,
            bad_groups: 0, orphan_inodes: 0,
            group_free_blocks: [0; 32], group_free_inodes: [0; 32],
        }
    }
}

impl MikuFS {
    pub fn ext2_write_file(
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
            return Err(FsError::ReadOnlyFs); // append-only: can't write before end
        }

        let bs = self.block_size as usize;
        let mut done = 0usize;

        while done < data.len() {
            let file_off = offset as usize + done;
            let logical_block = (file_off / bs) as u32;
            let block_off = file_off % bs;
            let chunk = (bs - block_off).min(data.len() - done);

            let phys_block = self.ensure_block(&mut inode, inode_num, logical_block)?;

            if chunk == bs && block_off == 0 {
                self.write_block_data(phys_block, &data[done..done + chunk])?;
            } else {
                let mut block_buf = [0u8; 4096];
                self.read_block_into(phys_block, &mut block_buf[..bs])?;
                block_buf[block_off..block_off + chunk]
                    .copy_from_slice(&data[done..done + chunk]);
                self.write_block_data(phys_block, &block_buf[..bs])?;
            }

            done += chunk;
        }

        let new_end = offset + done as u64;
        if new_end > inode.size() {
            inode.set_size(new_end as u32);
        }

        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(done)
    }

    pub fn ext2_create_file(
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
        inode.init_file(mode, 0, 0, now);

        self.write_inode(new_ino, &inode)?;
        self.add_dir_entry(parent_ino, name, new_ino, FT_REG_FILE)?;

        // update parent dir timestamps
        let mut parent_inode = self.read_inode(parent_ino)?;
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        Ok(new_ino)
    }

    pub fn ext2_create_dir(
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
        inode.init_dir(mode, 0, 0, now);
        inode.set_block(0, dir_block);
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

    pub fn ext2_create_symlink(
        &mut self,
        parent_ino: u32,
        name: &str,
        target: &str,
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
        inode.init_symlink(0o777, 0, 0, now);

        let target_bytes = target.as_bytes();
        let target_len = target_bytes.len();

        if target_len <= 60 {
            inode.data[40..40 + target_len].copy_from_slice(target_bytes);
            inode.set_size(target_len as u32);
        } else {
            let data_block = self.alloc_block(group)?;
            self.zero_block(data_block)?;
            let mut block_buf = [0u8; 4096];
            let bs = self.block_size as usize;
            let copy_len = target_len.min(bs);
            block_buf[..copy_len].copy_from_slice(&target_bytes[..copy_len]);
            self.write_block_data(data_block, &block_buf[..bs])?;
            inode.set_block(0, data_block);
            inode.set_size(target_len as u32);
            inode.set_blocks(self.block_size / 512);
        }

        self.write_inode(new_ino, &inode)?;
        self.add_dir_entry(parent_ino, name, new_ino, FT_SYMLINK)?;

        // update parent dir timestamps
        let mut parent_inode = self.read_inode(parent_ino)?;
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        Ok(new_ino)
    }

    pub fn ext2_delete_file(&mut self, parent_ino: u32, name: &str) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };

        let mut inode = self.read_inode(target_ino)?;
        if inode.is_directory() {
            return Err(FsError::IsDirectory);
        }
        if inode.has_flag(EXT4_IMMUTABLE_FL) || inode.has_flag(EXT4_APPEND_FL) {
            return Err(FsError::ReadOnlyFs);
        }

        self.remove_dir_entry(parent_ino, name)?;

        let links = inode.links_count();
        if links <= 1 {
            // last link - free blocks and inode
            if !inode.is_symlink() || !inode.is_fast_symlink() {
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

    pub fn ext2_delete_dir(&mut self, parent_ino: u32, name: &str) -> Result<(), FsError> {
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

        self.free_all_blocks(&inode)?;
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

    pub fn ext2_delete_recursive(&mut self, parent_ino: u32, name: &str) -> Result<u32, FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };

        let inode = self.read_inode(target_ino)?;

        if !inode.is_directory() {
            if !inode.is_symlink() || !inode.is_fast_symlink() {
                self.free_all_blocks(&inode)?;
            }
            self.free_inode(target_ino)?;
            self.remove_dir_entry(parent_ino, name)?;
            return Ok(1);
        }

        let mut children_names: [[u8; 255]; 32] = [[0u8; 255]; 32];
        let mut children_lens: [u8; 32] = [0; 32];
        let mut child_count = 0usize;

        {
            let dir_inode = self.read_inode(target_ino)?;
            let mut entries = [const { DirEntry::empty() }; 64];
            let count = self.read_dir(&dir_inode, &mut entries)?;
            for i in 0..count {
                let n = entries[i].name_str();
                if n == "." || n == ".." { continue; }
                if child_count < 32 {
                    let nb = n.as_bytes();
                    let l = nb.len().min(255);
                    children_names[child_count][..l].copy_from_slice(&nb[..l]);
                    children_lens[child_count] = l as u8;
                    child_count += 1;
                }
            }
        }

        let mut total = 0u32;
        for i in 0..child_count {
            let l = children_lens[i] as usize;
            let name_bytes = &children_names[i][..l];
            if let Ok(child_name) = core::str::from_utf8(name_bytes) {
                match self.ext2_delete_recursive(target_ino, child_name) {
                    Ok(n) => total += n,
                    Err(_) => {}
                }
            }
        }

        let target_inode = self.read_inode(target_ino)?;
        self.free_all_blocks(&target_inode)?;
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

        Ok(total + 1)
    }

    pub fn free_all_blocks(&mut self, inode: &Inode) -> Result<(), FsError> {
        if inode.is_symlink() && inode.is_fast_symlink() {
            return Ok(());
        }
        if inode.blocks() == 0 {
            return Ok(());
        }

        let total_blocks = self.superblock.blocks_count();
        let first_data = self.superblock.first_data_block();

        for i in 0..12 {
            let blk = inode.block(i);
            if blk != 0 && blk >= first_data && blk < total_blocks {
                self.free_block(blk)?;
            }
        }

        let ind = inode.block(12);
        if ind != 0 && ind >= first_data && ind < total_blocks {
            self.free_indirect_chain(ind, 1)?;
        }
        let dind = inode.block(13);
        if dind != 0 && dind >= first_data && dind < total_blocks {
            self.free_indirect_chain(dind, 2)?;
        }
        let tind = inode.block(14);
        if tind != 0 && tind >= first_data && tind < total_blocks {
            self.free_indirect_chain(tind, 3)?;
        }

        Ok(())
    }

    fn free_indirect_chain(&mut self, block: u32, depth: u32) -> Result<(), FsError> {
        if block == 0 { return Ok(()); }

        let ptrs_per_block = self.block_size / 4;
        let total_blocks = self.superblock.blocks_count();
        let first_data = self.superblock.first_data_block();

        if depth == 1 {
            for i in 0..ptrs_per_block {
                let ptr = self.read_indirect_entry(block, i)?;
                if ptr != 0 && ptr >= first_data && ptr < total_blocks {
                    self.free_block(ptr)?;
                }
            }
        } else {
            for i in 0..ptrs_per_block {
                let ptr = self.read_indirect_entry(block, i)?;
                if ptr != 0 && ptr >= first_data && ptr < total_blocks {
                    self.free_indirect_chain(ptr, depth - 1)?;
                }
            }
        }

        self.free_block(block)?;
        Ok(())
    }

    pub fn ext2_truncate(&mut self, inode_num: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        self.free_all_blocks(&inode)?;
        for i in 0..15 { inode.set_block(i, 0); }
        inode.set_size(0);
        inode.set_blocks(0);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
    }

    pub fn ext2_rename(
        &mut self, parent_ino: u32, old_name: &str, new_name: &str,
    ) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, old_name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };
        if self.ext2_lookup_in_dir(parent_ino, new_name)?.is_some() {
            return Err(FsError::AlreadyExists);
        }

        let inode = self.read_inode(target_ino)?;
        let ft = match inode.file_type() {
            InodeType::Directory => FT_DIR,
            InodeType::Symlink => FT_SYMLINK,
            _ => FT_REG_FILE,
        };

        self.remove_dir_entry(parent_ino, old_name)?;
        self.add_dir_entry(parent_ino, new_name, target_ino, ft)?;

        // update ctime on renamed inode
        let now = self.get_timestamp();
        let mut target_inode = self.read_inode(target_ino)?;
        target_inode.set_ctime(now);
        self.write_inode(target_ino, &target_inode)?;

        // update parent dir timestamps
        let mut parent_inode = self.read_inode(parent_ino)?;
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        Ok(())
    }

    // cross-directory move (rename between different parent directories)
    pub fn ext2_move(
        &mut self,
        src_parent: u32,
        src_name: &str,
        dst_parent: u32,
        dst_name: &str,
    ) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(src_parent, src_name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };
        if self.ext2_lookup_in_dir(dst_parent, dst_name)?.is_some() {
            return Err(FsError::AlreadyExists);
        }

        let inode = self.read_inode(target_ino)?;
        let ft = match inode.file_type() {
            InodeType::Directory => FT_DIR,
            InodeType::Symlink => FT_SYMLINK,
            _ => FT_REG_FILE,
        };

        // add entry in destination directory first
        self.add_dir_entry(dst_parent, dst_name, target_ino, ft)?;
        // then remove from source
        self.remove_dir_entry(src_parent, src_name)?;

        let now = self.get_timestamp();

        // update ctime on moved inode
        let mut target_inode = self.read_inode(target_ino)?;
        target_inode.set_ctime(now);

        // if moving a directory, update its .. entry to point to new parent
        if inode.is_directory() {
            let bs = self.block_size as usize;
            let first_block = self.get_file_block(&target_inode, 0)?;
            if first_block != 0 {
                let mut block_data = [0u8; 4096];
                self.read_block_into(first_block, &mut block_data[..bs])?;
                // .. entry is at offset 12 (after . entry)
                let dot_rec_len = u16::from_le_bytes([block_data[4], block_data[5]]) as usize;
                if dot_rec_len <= bs {
                    block_data[dot_rec_len..dot_rec_len + 4]
                        .copy_from_slice(&dst_parent.to_le_bytes());
                    self.write_block_data(first_block, &block_data[..bs])?;
                }
            }

            // update link counts
            let mut src_parent_inode = self.read_inode(src_parent)?;
            let links = src_parent_inode.links_count();
            if links > 1 {
                src_parent_inode.set_links_count(links - 1);
            }
            src_parent_inode.set_mtime(now);
            src_parent_inode.set_ctime(now);
            self.write_inode(src_parent, &src_parent_inode)?;

            let mut dst_parent_inode = self.read_inode(dst_parent)?;
            dst_parent_inode.set_links_count(dst_parent_inode.links_count() + 1);
            dst_parent_inode.set_mtime(now);
            dst_parent_inode.set_ctime(now);
            self.write_inode(dst_parent, &dst_parent_inode)?;
        } else {
            // update both parent dir timestamps
            let mut src_p = self.read_inode(src_parent)?;
            src_p.set_mtime(now);
            src_p.set_ctime(now);
            self.write_inode(src_parent, &src_p)?;

            if dst_parent != src_parent {
                let mut dst_p = self.read_inode(dst_parent)?;
                dst_p.set_mtime(now);
                dst_p.set_ctime(now);
                self.write_inode(dst_parent, &dst_p)?;
            }
        }

        self.write_inode(target_ino, &target_inode)?;
        Ok(())
    }

    pub fn ext2_chmod(&mut self, inode_num: u32, mode: u16) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        let type_bits = inode.mode() & 0xF000;
        inode.set_mode(type_bits | (mode & 0o7777));
        let now = self.get_timestamp();
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)
    }

    pub fn ext2_chown(&mut self, inode_num: u32, uid: u16, gid: u16) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        inode.set_uid(uid);
        inode.set_gid(gid);
        let now = self.get_timestamp();
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)
    }

    // set inode flags (chattr equivalent)
    pub fn ext2_chflags(&mut self, inode_num: u32, flags: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        // preserve system flags, only allow user-settable flags
        let user_mask: u32 = EXT4_IMMUTABLE_FL | EXT4_APPEND_FL | EXT4_NODUMP_FL | EXT4_NOATIME_FL;
        let sys_flags = inode.flags() & !user_mask;
        inode.set_flags(sys_flags | (flags & user_mask));
        let now = self.get_timestamp();
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)
    }

    // check if inode is immutable
    pub fn is_immutable(&mut self, inode_num: u32) -> Result<bool, FsError> {
        let inode = self.read_inode(inode_num)?;
        Ok(inode.has_flag(EXT4_IMMUTABLE_FL))
    }

    // check if inode is append-only
    pub fn is_append_only(&mut self, inode_num: u32) -> Result<bool, FsError> {
        let inode = self.read_inode(inode_num)?;
        Ok(inode.has_flag(EXT4_APPEND_FL))
    }

    pub fn ext2_copy_file(
        &mut self, src_ino: u32, dst_parent_ino: u32, dst_name: &str,
    ) -> Result<u32, FsError> {
        let src_inode = self.read_inode(src_ino)?;
        if !src_inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }

        let size = src_inode.size();
        let mode = src_inode.permissions();
        let new_ino = self.ext2_create_file(dst_parent_ino, dst_name, mode)?;

        if size > 0 {
            let bs = self.block_size as usize;
            let mut offset = 0u64;
            let mut buf = [0u8; 4096];

            while offset < size {
                let to_read = ((size - offset) as usize).min(bs);
                let src_inode = self.read_inode(src_ino)?;
                let n = self.read_file(&src_inode, offset, &mut buf[..to_read])?;
                if n == 0 { break; }
                self.ext2_write_file(new_ino, &buf[..n], offset)?;
                offset += n as u64;
            }
        }

        Ok(new_ino)
    }

    pub fn ext2_file_size(&mut self, inode_num: u32) -> Result<u64, FsError> {
        let inode = self.read_inode(inode_num)?;
        Ok(inode.size())
    }

    // punch hole - deallocate blocks in a range (sparse file support)
    pub fn ext2_punch_hole(
        &mut self,
        inode_num: u32,
        offset: u64,
        len: u64,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(inode_num)?;
        if !inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }
        let bs = self.block_size as u64;
        // only punch aligned full blocks
        let start_block = ((offset + bs - 1) / bs) as u32;
        let end_block = ((offset + len) / bs) as u32;

        let ptrs_per_block = self.block_size / 4;
        for logical in start_block..end_block {
            let phys = self.get_file_block(&inode, logical)?;
            if phys == 0 {
                continue;
            }

            let cleared = if logical < 12 {
                let mut im = self.read_inode(inode_num)?;
                im.set_block(logical as usize, 0);
                self.write_inode(inode_num, &im)?;
                true
            } else {
                let adjusted = logical - 12;
                if adjusted < ptrs_per_block {
                    let ind = inode.block(12);
                    if ind != 0 {
                        self.write_indirect_entry(ind, adjusted, 0)?;
                        true
                    } else {
                        false
                    }
                } else if adjusted < ptrs_per_block + ptrs_per_block * ptrs_per_block {
                    let adj2 = adjusted - ptrs_per_block;
                    let dind = inode.block(13);
                    if dind != 0 {
                        let idx1 = adj2 / ptrs_per_block;
                        let idx2 = adj2 % ptrs_per_block;
                        let l1 = self.read_indirect_entry(dind, idx1)?;
                        if l1 != 0 {
                            self.write_indirect_entry(l1, idx2, 0)?;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    let adj3 = adjusted - ptrs_per_block - ptrs_per_block * ptrs_per_block;
                    let tind = inode.block(14);
                    if tind != 0 {
                        let idx1 = adj3 / (ptrs_per_block * ptrs_per_block);
                        let rem = adj3 % (ptrs_per_block * ptrs_per_block);
                        let idx2 = rem / ptrs_per_block;
                        let idx3 = rem % ptrs_per_block;
                        let l1 = self.read_indirect_entry(tind, idx1)?;
                        if l1 != 0 {
                            let l2 = self.read_indirect_entry(l1, idx2)?;
                            if l2 != 0 {
                                self.write_indirect_entry(l2, idx3, 0)?;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
            };

            if cleared {
                self.free_block(phys)?;
                let mut im = self.read_inode(inode_num)?;
                let blks = im.blocks().saturating_sub(self.block_size / 512);
                im.set_blocks(blks);
                self.write_inode(inode_num, &im)?;
            }
        }

        let now = self.get_timestamp();
        let mut inode = self.read_inode(inode_num)?;
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
    }

    // preallocate blocks for a file without writing data (like fallocate)
    pub fn ext2_fallocate(
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
            let existing = self.get_file_block(&inode, logical)?;
            if existing == 0 {
                self.ensure_block(&mut inode, inode_num, logical)?;
                inode = self.read_inode(inode_num)?;
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

    pub fn ext2_append_file(&mut self, inode_num: u32, data: &[u8]) -> Result<usize, FsError> {
        let inode = self.read_inode(inode_num)?;
        let offset = inode.size();
        self.ext2_write_file(inode_num, data, offset)
    }

    pub fn ext2_dir_size(&mut self, dir_ino: u32) -> Result<(u32, u64), FsError> {
        self.ext2_dir_size_depth(dir_ino, 0)
    }

    fn ext2_dir_size_depth(
        &mut self, dir_ino: u32, depth: u32,
    ) -> Result<(u32, u64), FsError> {
        if depth >= 32 { return Ok((0, 0)); }

        let inode = self.read_inode(dir_ino)?;
        if !inode.is_directory() { return Err(FsError::NotDirectory); }

        let mut entries = [const { DirEntry::empty() }; 64];
        let count = self.read_dir(&inode, &mut entries)?;
        let mut total_files = 0u32;
        let mut total_bytes = 0u64;

        for i in 0..count {
            let e = &entries[i];
            let name = e.name_str();
            if name == "." || name == ".." { continue; }
            let child_inode = self.read_inode(e.inode)?;
            if child_inode.is_directory() {
                let (sf, sb) = self.ext2_dir_size_depth(e.inode, depth + 1)?;
                total_files += sf + 1;
                total_bytes += sb;
            } else {
                total_files += 1;
                total_bytes += child_inode.size();
            }
        }
        Ok((total_files, total_bytes))
    }

    pub fn ext2_tree(
        &mut self, dir_ino: u32, prefix: &str, result: &mut TreeResult,
    ) -> Result<(), FsError> {
        if result.count >= result.max { return Ok(()); }
        let inode = self.read_inode(dir_ino)?;
        if !inode.is_directory() { return Err(FsError::NotDirectory); }

        let mut entries = [const { DirEntry::empty() }; 64];
        let count = self.read_dir(&inode, &mut entries)?;

        let mut real_count = 0usize;
        for i in 0..count {
            let n = entries[i].name_str();
            if n != "." && n != ".." { real_count += 1; }
        }

        let mut idx = 0usize;
        for i in 0..count {
            let e = &entries[i];
            let name = e.name_str();
            if name == "." || name == ".." { continue; }
            idx += 1;
            let is_last = idx == real_count;
            if result.count >= result.max { break; }

            let entry = &mut result.entries[result.count];
            entry.depth = prefix.len() as u8 / 4;
            entry.is_last = is_last;
            entry.is_dir = e.file_type == FT_DIR;
            entry.is_symlink = e.file_type == FT_SYMLINK;
            let nb = name.as_bytes();
            let l = nb.len().min(59);
            entry.name[..l].copy_from_slice(&nb[..l]);
            entry.name_len = l as u8;
            if !entry.is_dir {
                let child_inode = self.read_inode(e.inode)?;
                entry.size = child_inode.size();
            }
            result.count += 1;

            if e.file_type == FT_DIR && entry.depth < 4 {
                let mut new_prefix = [0u8; 64];
                let plen = prefix.len();
                let pl = plen.min(56);
                new_prefix[..pl].copy_from_slice(&prefix.as_bytes()[..pl]);
                let suffix = if is_last { b"    " } else { b"|   " };
                new_prefix[pl..pl + 4].copy_from_slice(suffix);
                let np = unsafe { core::str::from_utf8_unchecked(&new_prefix[..pl + 4]) };
                let _ = self.ext2_tree(e.inode, np, result);
            }
        }
        Ok(())
    }

    pub fn ext2_fsck(&mut self) -> FsckResult {
        let mut result = FsckResult::new();
        result.total_inodes = self.superblock.inodes_count();
        result.total_blocks = self.superblock.blocks_count();
        result.free_inodes = self.superblock.free_inodes_count();
        result.free_blocks = self.superblock.free_blocks_count();
        result.block_size = self.block_size;
        result.inode_size = self.inode_size();

        if self.superblock.magic() != EXT2_MAGIC {
            result.errors += 1;
            result.bad_magic = true;
            return result;
        }

        for g in 0..self.group_count as usize {
            if g >= 32 { break; }
            let bb = self.groups[g].block_bitmap();
            let ib = self.groups[g].inode_bitmap();
            let it = self.groups[g].inode_table();
            if bb == 0 || ib == 0 || it == 0 {
                result.errors += 1;
                result.bad_groups += 1;
            }
            result.group_free_blocks[g] = self.groups[g].free_blocks();
            result.group_free_inodes[g] = self.groups[g].free_inodes();
        }

        if let Ok(root_inode) = self.read_inode(EXT2_ROOT_INO) {
            if !root_inode.is_directory() {
                result.errors += 1;
                result.root_not_dir = true;
            }
            result.root_ok = true;
        } else {
            result.errors += 1;
            result.root_ok = false;
        }

        let mut used_inodes = 0u32;
        let first_ino = if self.superblock.rev_level() >= 1 {
            self.superblock.first_ino()
        } else { EXT2_FIRST_INO_OLD };

        for ino in 1..=self.superblock.inodes_count().min(256) {
            if let Ok(inode) = self.read_inode(ino) {
                if inode.mode() != 0 || inode.links_count() != 0 {
                    used_inodes += 1;
                    if inode.links_count() == 0 && inode.dtime() == 0 && ino >= first_ino {
                        result.orphan_inodes += 1;
                    }
                }
            }
        }
        result.used_inodes = used_inodes;
        result.checked = true;
        result
    }

    pub fn ext2_lookup_in_dir(
        &mut self, dir_ino: u32, name: &str,
    ) -> Result<Option<u32>, FsError> {
        let inode = self.read_inode(dir_ino)?;
        let mut entries = [const { DirEntry::empty() }; 64];
        let count = self.read_dir(&inode, &mut entries)?;
        let name_bytes = name.as_bytes();
        for i in 0..count {
            let e = &entries[i];
            let elen = e.name_len as usize;
            if elen == name_bytes.len() && &e.name[..elen] == name_bytes {
                return Ok(Some(e.inode));
            }
        }
        Ok(None)
    }

    pub fn is_ext2_dir_empty(&mut self, dir_ino: u32) -> Result<bool, FsError> {
        let inode = self.read_inode(dir_ino)?;
        let mut entries = [const { DirEntry::empty() }; 64];
        let count = self.read_dir(&inode, &mut entries)?;
        for i in 0..count {
            let name = entries[i].name_str();
            if name != "." && name != ".." { return Ok(false); }
        }
        Ok(true)
    }

    pub fn add_dir_entry(
        &mut self, dir_ino: u32, name: &str, child_ino: u32, file_type: u8,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(dir_ino)?;
        let bs = self.block_size as usize;
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len();
        let needed = ((8 + name_len + 3) / 4) * 4;

        let num_blocks = if inode.size_lo() == 0 { 0 }
        else { (inode.size_lo() as usize + bs - 1) / bs };

        // htree directories (created by Linux's dir_index) keep an index in
        // block 0 (dx_root) and, for multi-level trees, in dx_node blocks.
        // A linear insert must never write those index blocks or it corrupts
        // the tree. Single-level trees (the common case) have real entry
        // leaves from block 1 on, so we insert there and let the linear
        // lookup/readdir fallback find them; deeper trees we refuse rather
        // than risk damage, so the on-disk directory stays consistent.
        let start_block = if inode.has_flag(EXT4_INDEX_FL) && self.superblock.has_dir_index() {
            if self.htree_depth(&inode)? > 0 {
                crate::serial_println!(
                    "[extfs] add_dir_entry: refusing write to multi-level htree dir {} (would corrupt index)",
                    dir_ino
                );
                return Err(FsError::UnsupportedFeature);
            }
            1
        } else {
            0
        };

        for b in start_block..num_blocks {
            let phys = self.get_file_block(&inode, b as u32)?;
            if phys == 0 { continue; }

            let mut block_data = [0u8; 4096];
            self.read_block_into(phys, &mut block_data[..bs])?;

            let mut pos = 0;
            while pos < bs {
                if pos + 8 > bs { break; }
                let rec_ino = u32::from_le_bytes([
                    block_data[pos], block_data[pos+1],
                    block_data[pos+2], block_data[pos+3],
                ]);
                let rec_len = u16::from_le_bytes([
                    block_data[pos+4], block_data[pos+5],
                ]) as usize;
                let rec_name_len = block_data[pos+6] as usize;
                if rec_len == 0 || rec_len > bs { break; }

                let actual_size = if rec_ino == 0 { 8 }
                else { ((8 + rec_name_len + 3) / 4) * 4 };

                if rec_len < actual_size { pos += rec_len; continue; }
                let free_space = rec_len - actual_size;

                if free_space >= needed {
                    if rec_ino != 0 {
                        let new_rec_len = actual_size as u16;
                        block_data[pos+4..pos+6].copy_from_slice(&new_rec_len.to_le_bytes());
                        pos += actual_size;
                    }
                    let remaining = if rec_ino != 0 { rec_len - actual_size } else { rec_len };

                    block_data[pos..pos+4].copy_from_slice(&child_ino.to_le_bytes());
                    block_data[pos+4..pos+6].copy_from_slice(&(remaining as u16).to_le_bytes());
                    block_data[pos+6] = name_len as u8;
                    block_data[pos+7] = file_type;
                    block_data[pos+8..pos+8+name_len].copy_from_slice(name_bytes);

                    self.write_block_data(phys, &block_data[..bs])?;
                    return Ok(());
                }
                pos += rec_len;
            }
        }

        let group = ((dir_ino - 1) / self.inodes_per_group) as usize;
        let new_block = self.alloc_block(group)?;
        self.zero_block(new_block)?;

        let mut block_data = [0u8; 4096];
        block_data[0..4].copy_from_slice(&child_ino.to_le_bytes());
        block_data[4..6].copy_from_slice(&(bs as u16).to_le_bytes());
        block_data[6] = name_len as u8;
        block_data[7] = file_type;
        block_data[8..8+name_len].copy_from_slice(name_bytes);
        self.write_block_data(new_block, &block_data[..bs])?;

        let logical_block = num_blocks as u32;
        let mut dir_inode = self.read_inode(dir_ino)?;

        if dir_inode.uses_extents() {
            self.ext4_insert_extent(&mut dir_inode, dir_ino, logical_block, new_block)?;
            let blks = dir_inode.blocks() + (self.block_size / 512);
            dir_inode.set_blocks(blks);
        } else if logical_block < 12 {
            dir_inode.set_block(logical_block as usize, new_block);
            let blks = dir_inode.blocks() + (self.block_size / 512);
            dir_inode.set_blocks(blks);
        } else {
            let _ = self.free_block(new_block);
            return Err(FsError::NoSpace);
        }

        let new_size = (logical_block + 1) * self.block_size;
        dir_inode.set_size(new_size);
        let now = self.get_timestamp();
        dir_inode.set_mtime(now);
        dir_inode.set_ctime(now);
        self.write_inode(dir_ino, &dir_inode)?;
        Ok(())
    }

    pub fn remove_dir_entry(&mut self, dir_ino: u32, name: &str) -> Result<(), FsError> {
        let inode = self.read_inode(dir_ino)?;
        let bs = self.block_size as usize;
        let name_bytes = name.as_bytes();
        let num_blocks = if inode.size_lo() == 0 { 0 }
        else { (inode.size_lo() as usize + bs - 1) / bs };

        for b in 0..num_blocks {
            let phys = self.get_file_block(&inode, b as u32)?;
            if phys == 0 { continue; }

            let mut block_data = [0u8; 4096];
            self.read_block_into(phys, &mut block_data[..bs])?;

            let mut pos = 0;
            let mut prev_pos: Option<usize> = None;
            while pos < bs {
                if pos + 8 > bs { break; }
                let rec_ino = u32::from_le_bytes([
                    block_data[pos], block_data[pos+1],
                    block_data[pos+2], block_data[pos+3],
                ]);
                let rec_len = u16::from_le_bytes([
                    block_data[pos+4], block_data[pos+5],
                ]) as usize;
                let rec_name_len = block_data[pos+6] as usize;
                if rec_len == 0 { break; }

                if rec_ino != 0
                    && rec_name_len == name_bytes.len()
                    && pos + 8 + rec_name_len <= bs
                    && &block_data[pos+8..pos+8+rec_name_len] == name_bytes
                {
                    if let Some(pp) = prev_pos {
                        let prev_rec_len = u16::from_le_bytes([
                            block_data[pp+4], block_data[pp+5],
                        ]) as usize;
                        let merged = prev_rec_len + rec_len;
                        block_data[pp+4..pp+6].copy_from_slice(&(merged as u16).to_le_bytes());
                    } else {
                        block_data[pos..pos+4].copy_from_slice(&0u32.to_le_bytes());
                    }
                    self.write_block_data(phys, &block_data[..bs])?;
                    return Ok(());
                }
                prev_pos = Some(pos);
                pos += rec_len;
            }
        }
        Err(FsError::NotFound)
    }
}
