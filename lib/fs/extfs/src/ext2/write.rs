use crate::structs::*;
use crate::{FsError, MikuFS};
use crate::host::ExtHost;
use super::dir::insert_into_dir_block;

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

impl<H: ExtHost> MikuFS<H> {
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
        // An inline file's `i_block` is its contents, not block numbers
        if inode.has_inline_data() {
            return self.ext4_write_inline(inode_num, data, offset);
        }
        if inode.has_flag(EXT4_APPEND_FL) && offset < inode.size() {
            return Err(FsError::ReadOnlyFs); // append-only: no writes before the end
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
            // The 64-bit setter, as `ext2_fallocate` uses
            inode.set_size_full(new_end);
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
        let mut inode = self.fresh_inode();
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
        let mut inode = self.fresh_inode();
        inode.init_dir(mode, 0, 0, now);
        inode.set_block(0, dir_block);
        inode.set_size(self.block_size);
        inode.set_blocks_full((self.block_size / 512) as u64);

        self.write_inode(new_ino, &inode)?;

        let bs = self.block_size as usize;
        let mut block_data = [0u8; 4096];

        // ".." runs to the end of the usable area, which on metadata_csum stops
        // one tail short of the block for the checksum record
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
        let mut inode = self.fresh_inode();
        inode.init_symlink(0o777, 0, 0, now);

        let target_bytes = target.as_bytes();
        let target_len = target_bytes.len();

        if target_len <= 60 {
            inode.data[40..40 + target_len].copy_from_slice(target_bytes);
            inode.set_size(target_len as u32);
        } else {
            // As many blocks as the target needs, through `ensure_block`, which
            // grows the indirect map and keeps `i_blocks`
            let bs = self.block_size as usize;
            let mut buf = alloc::vec![0u8; bs];
            let mut written = 0usize;
            let mut logical = 0u32;
            while written < target_len {
                let data_block = self.ensure_block(&mut inode, new_ino, logical)?;
                let n = (target_len - written).min(bs);
                buf[..n].copy_from_slice(&target_bytes[written..written + n]);
                if n < bs {
                    buf[n..].fill(0);
                }
                self.write_block_data(data_block, &buf)?;
                written += n;
                logical += 1;
            }
            inode.set_size(target_len as u32);
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

    /// A character device, block device, FIFO, or socket. `kind` is the `FT_*`
    /// of the directory entry, also deciding the mode's type bits
    pub fn ext2_mknod(
        &mut self,
        parent_ino: u32,
        name: &str,
        kind: u8,
        mode: u16,
        major: u32,
        minor: u32,
    ) -> Result<u32, FsError> {
        let type_bits = match kind {
            FT_CHRDEV => S_IFCHR,
            FT_BLKDEV => S_IFBLK,
            FT_FIFO => S_IFIFO,
            FT_SOCK => S_IFSOCK,
            _ => return Err(FsError::InvalidArg),
        };

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
        inode.init_special(type_bits, mode, 0, 0, now, major, minor);

        self.write_inode(new_ino, &inode)?;
        self.add_dir_entry(parent_ino, name, new_ino, kind)?;

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
            // Last link. Freeing is several independent writes (blocks, two
            // bitmaps, counters), so the orphan list brackets it for the next
            // mount to finish
            inode.set_links_count(0);
            self.write_inode(target_ino, &inode)?;
            self.orphan_add(target_ino)?;

            if !inode.is_symlink() || !inode.is_fast_symlink() {
                self.free_all_blocks(&inode)?;
            }

            self.orphan_del(target_ino)?;
            let now = self.deletion_timestamp();
            let mut inode = self.read_inode(target_ino)?;
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
        // As in the file path: a freed inode records when it died, or a check
        // reports zero dtime
        let mut dead = inode;
        dead.set_links_count(0);
        dead.set_dtime(self.deletion_timestamp());
        self.write_inode(target_ino, &dead)?;
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

    /// Deletes a name and, for a directory, everything under it. The directory
    /// is emptied in batches until nothing is left, and the parent is freed
    /// only then
    pub fn ext2_delete_recursive(&mut self, parent_ino: u32, name: &str) -> Result<u32, FsError> {
        self.delete_recursive_depth(parent_ino, name, 0)
    }

    /// A deeper tree is from no tool, or corrupt with a cycle; recursing
    /// further exhausts the stack. `ext2_dir_size_depth` has the same limit
    const MAX_DELETE_DEPTH: u32 = 32;

    fn delete_recursive_depth(
        &mut self, parent_ino: u32, name: &str, depth: u32,
    ) -> Result<u32, FsError> {
        if depth >= Self::MAX_DELETE_DEPTH {
            return Err(FsError::LinkLoop);
        }

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

        let mut total = 0u32;

        // Empties the directory in batches until a pass finds nothing
        loop {
            let mut names: [[u8; MAX_NAME]; 32] = [[0u8; MAX_NAME]; 32];
            let mut lens: [u8; 32] = [0; 32];
            let mut found = 0usize;

            {
                let dir_inode = self.read_inode(target_ino)?;
                self.read_dir_each(&dir_inode, |e| {
                    if found == names.len() {
                        return false;
                    }
                    let n = e.name_str();
                    if n == "." || n == ".." {
                        return true;
                    }
                    let nb = n.as_bytes();
                    let l = nb.len().min(MAX_NAME);
                    names[found][..l].copy_from_slice(&nb[..l]);
                    lens[found] = l as u8;
                    found += 1;
                    true
                })?;
            }

            if found == 0 {
                break;
            }

            let mut removed_any = false;
            for i in 0..found {
                let l = lens[i] as usize;
                let Ok(child) = core::str::from_utf8(&names[i][..l]) else {
                    // A non-UTF-8 name cannot be looked up here; removing the
                    // entry directly keeps the loop from spinning
                    continue;
                };
                match self.delete_recursive_depth(target_ino, child, depth + 1) {
                    Ok(n) => {
                        total += n;
                        removed_any = true;
                    }
                    Err(e) => return Err(e),
                }
            }

            // Nothing in this batch could be removed, so another pass would
            // repeat. Stops, leaving the directory in place
            if !removed_any {
                return Err(FsError::NotEmpty);
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
        if gidx < self.groups.len() {
            self.groups[gidx].dec_used_dirs();
            self.flush_group_desc(gidx)?;
        }

        Ok(total + 1)
    }

    pub fn free_all_blocks(&mut self, inode: &Inode) -> Result<(), FsError> {
        // The extended-attribute block is the file's too, and not in the block
        // map
        let acl = inode.file_acl_lo();
        if acl != 0 && self.is_valid_block(acl) {
            self.free_block(acl)?;
        }
        if inode.is_symlink() && inode.is_fast_symlink() {
            return Ok(());
        }
        // A device node's `i_block[0]` is its major and minor, not a block;
        // freeing 1:3 would free block 259
        if inode.is_special() {
            return Ok(());
        }
        if inode.blocks_full() == 0 {
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

    /// Truncate to zero: frees everything and resets the block pointers
    pub fn ext2_truncate(&mut self, inode_num: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        self.free_all_blocks(&inode)?;
        for i in 0..15 { inode.set_block(i, 0); }
        inode.set_size_full(0);
        inode.set_blocks_full(0);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
    }

    /// Truncates to any length. Growing is sparse: the size moves with no
    /// blocks allocated, so the gap reads as zeroes, as on other Unix
    /// filesystems
    pub fn ext2_truncate_to(&mut self, inode_num: u32, new_size: u64) -> Result<(), FsError> {
        if new_size == 0 {
            return self.ext2_truncate(inode_num);
        }

        let mut inode = self.read_inode(inode_num)?;
        let old_size = inode.size();
        let bs = self.block_size as u64;

        if new_size >= old_size {
            // Grow: sparse, allocating nothing
            inode.set_size_full(new_size);
            let now = self.get_timestamp();
            inode.set_mtime(now);
            inode.set_ctime(now);
            return self.write_inode(inode_num, &inode);
        }

        // Frees every block entirely past the new end, keeping the block it
        // falls in
        let first_free = new_size.div_ceil(bs) as u32;
        let old_blocks = old_size.div_ceil(bs) as u32;
        let mut freed = 0u32;

        for logical in first_free..old_blocks {
            let inode = self.read_inode(inode_num)?;
            let phys = self.get_file_block(&inode, logical)?;
            if phys == 0 {
                continue;
            }
            self.free_block(phys)?;
            self.clear_block_pointer(inode_num, logical)?;
            freed += 1;
        }

        // Zeroes the last kept block's tail, so a later grow reads zeroes
        let tail = (new_size % bs) as usize;
        if tail != 0 {
            let inode = self.read_inode(inode_num)?;
            let last = (new_size / bs) as u32;
            let phys = self.get_file_block(&inode, last)?;
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
        let blocks = inode.blocks_full().saturating_sub(freed as u64 * per_block);
        inode.set_blocks_full(blocks);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(())
    }

    /// Drops one logical block's pointer, freeing its indirect block once empty
    fn clear_block_pointer(&mut self, inode_num: u32, logical: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        if inode.uses_extents() {
            // Extent-mapped files drop whole extents; `ext4_truncate_to`
            // handles them
            return Ok(());
        }
        let ptrs = self.block_size / 4;
        let bsz = self.block_size as usize;

        if logical < 12 {
            inode.set_block(logical as usize, 0);
            return self.write_inode(inode_num, &inode);
        }

        let mut idx = logical - 12;
        if idx < ptrs {
            let ind = inode.block(12);
            if ind != 0 {
                self.clear_indirect_slot(ind, idx as usize, bsz)?;
            }
            return Ok(());
        }

        idx -= ptrs;
        if idx < ptrs * ptrs {
            let dind = inode.block(13);
            if dind == 0 { return Ok(()); }
            let mut buf = [0u8; 4096];
            self.read_block_into(dind, &mut buf[..bsz])?;
            let outer = (idx / ptrs) as usize;
            let child = u32::from_le_bytes([
                buf[outer * 4], buf[outer * 4 + 1], buf[outer * 4 + 2], buf[outer * 4 + 3],
            ]);
            if child != 0 {
                self.clear_indirect_slot(child, (idx % ptrs) as usize, bsz)?;
            }
            return Ok(());
        }

        idx -= ptrs * ptrs;
        let tind = inode.block(14);
        if tind == 0 { return Ok(()); }
        let mut buf = [0u8; 4096];
        self.read_block_into(tind, &mut buf[..bsz])?;
        let outer = (idx / (ptrs * ptrs)) as usize;
        let dind = u32::from_le_bytes([
            buf[outer * 4], buf[outer * 4 + 1], buf[outer * 4 + 2], buf[outer * 4 + 3],
        ]);
        if dind == 0 { return Ok(()); }
        let rem = idx % (ptrs * ptrs);
        self.read_block_into(dind, &mut buf[..bsz])?;
        let mid = (rem / ptrs) as usize;
        let child = u32::from_le_bytes([
            buf[mid * 4], buf[mid * 4 + 1], buf[mid * 4 + 2], buf[mid * 4 + 3],
        ]);
        if child != 0 {
            self.clear_indirect_slot(child, (rem % ptrs) as usize, bsz)?;
        }
        Ok(())
    }

    fn clear_indirect_slot(&mut self, block: u32, slot: usize, bsz: usize) -> Result<(), FsError> {
        if slot * 4 + 4 > bsz {
            return Ok(());
        }
        let mut buf = [0u8; 4096];
        self.read_block_into(block, &mut buf[..bsz])?;
        buf[slot * 4..slot * 4 + 4].copy_from_slice(&0u32.to_le_bytes());
        self.write_block_data(block, &buf[..bsz])
    }

    /// Removes what `name` already names in `parent_ino`, so a rename can take
    /// its place. POSIX renames replace their target, and installers rely on
    /// it.
    ///
    /// The dispatch is `ext3_delete_file`'s: an extent inode cannot be freed by
    /// walking block pointers. Emptiness is judged by `*_delete_dir`, the one
    /// source of `NotEmpty`
    fn unlink_replaced(
        &mut self, parent_ino: u32, name: &str, victim: u32,
    ) -> Result<(), FsError> {
        let inode = self.read_inode(victim)?;
        if inode.is_directory() {
            return if inode.uses_extents() {
                self.ext4_delete_dir(parent_ino, name)
            } else {
                self.ext2_delete_dir(parent_ino, name)
            };
        }
        if inode.uses_extents() {
            self.ext4_delete_file(parent_ino, name)
        } else {
            self.ext2_delete_file(parent_ino, name)
        }
    }

    /// Whether `src` may be renamed onto `victim`: a directory replaces only a
    /// directory, a non-directory only a non-directory. A victim directory's
    /// emptiness is decided elsewhere
    fn replaceable(&mut self, src: u32, victim: u32) -> Result<(), FsError> {
        let src_is_dir = self.read_inode(src)?.is_directory();
        let victim_is_dir = self.read_inode(victim)?.is_directory();
        match (src_is_dir, victim_is_dir) {
            (true, false) => Err(FsError::NotDirectory),
            (false, true) => Err(FsError::IsDirectory),
            _ => Ok(()),
        }
    }

    pub fn ext2_rename(
        &mut self, parent_ino: u32, old_name: &str, new_name: &str,
    ) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, old_name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };
        // A rename onto an existing name replaces it. Two names for one inode,
        // a hard link or a rename to itself, is caught first, or the unlink
        // would delete the renamed file. POSIX says that case succeeds and
        // changes nothing
        if let Some(victim) = self.ext2_lookup_in_dir(parent_ino, new_name)? {
            if victim == target_ino {
                return Ok(());
            }
            self.replaceable(target_ino, victim)?;
            self.unlink_replaced(parent_ino, new_name, victim)?;
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

    // rename across parent directories
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
        // Replaces its target, as `ext2_rename` does. Bookkeeping cancels when
        // a directory replaces one: `ext2_delete_dir` takes a link off
        // `dst_parent` for the victim's `..`, and the move adds one back
        if let Some(victim) = self.ext2_lookup_in_dir(dst_parent, dst_name)? {
            if victim == target_ino {
                return Ok(());
            }
            self.replaceable(target_ino, victim)?;
            self.unlink_replaced(dst_parent, dst_name, victim)?;
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
                if dot_rec_len + 4 <= bs {
                    block_data[dot_rec_len..dot_rec_len + 4]
                        .copy_from_slice(&dst_parent.to_le_bytes());
                    // Block 0 of an indexed directory is the root, whose
                    // checksum covers the two fake entries, so a move
                    // invalidates it. `write_dir_block` looks for a leaf's 0xDE
                    // tail, which a root lacks
                    self.write_dir_head(target_ino, &target_inode, first_block, &mut block_data)?;
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
            if dst_parent_inode.links_count() >= EXT2_LINK_MAX {
                return Err(FsError::TooManyLinks);
            }
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

    /// Sets the owner, full width, matching `uid_full` on the read side
    pub fn ext2_chown(&mut self, inode_num: u32, uid: u32, gid: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        inode.set_uid_full(uid);
        inode.set_gid_full(gid);
        let now = self.get_timestamp();
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)
    }

    // set inode flags (chattr equivalent)
    pub fn ext2_chflags(&mut self, inode_num: u32, flags: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        // Preserves system flags; only user-settable flags may change
        let user_mask: u32 = EXT4_IMMUTABLE_FL | EXT4_APPEND_FL | EXT4_NODUMP_FL | EXT4_NOATIME_FL;
        let sys_flags = inode.flags() & !user_mask;
        inode.set_flags(sys_flags | (flags & user_mask));
        let now = self.get_timestamp();
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)
    }

    pub fn is_immutable(&mut self, inode_num: u32) -> Result<bool, FsError> {
        let inode = self.read_inode(inode_num)?;
        Ok(inode.has_flag(EXT4_IMMUTABLE_FL))
    }

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

    // deallocate blocks in a range (sparse file support)
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
        // Only punches aligned full blocks
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
                let blks = im.blocks_full().saturating_sub((self.block_size / 512) as u64);
                im.set_blocks_full(blks);
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

        // Collected first, since walking into a subdirectory needs `self`,
        // which the callback holds
        let mut entries: alloc::vec::Vec<DirEntry> = alloc::vec::Vec::new();
        self.read_dir_each(&inode, |e| {
            entries.push(*e);
            true
        })?;
        let mut total_files = 0u32;
        let mut total_bytes = 0u64;

        for e in entries.iter() {
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

        let mut entries: alloc::vec::Vec<DirEntry> = alloc::vec::Vec::new();
        self.read_dir_each(&inode, |e| {
            entries.push(*e);
            true
        })?;

        let mut real_count = 0usize;
        for e in entries.iter() {
            let n = e.name_str();
            if n != "." && n != ".." { real_count += 1; }
        }

        let mut idx = 0usize;
        for e in entries.iter() {
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

    pub fn ext2_lookup_in_dir(
        &mut self, dir_ino: u32, name: &str,
    ) -> Result<Option<u32>, FsError> {
        let inode = self.read_inode(dir_ino)?;
        // Through `lookup`, so an indexed directory answers from its index
        match self.lookup(dir_ino, &inode, name) {
            Ok(ino) => Ok(Some(ino)),
            Err(FsError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn is_ext2_dir_empty(&mut self, dir_ino: u32) -> Result<bool, FsError> {
        let inode = self.read_inode(dir_ino)?;
        let mut empty = true;
        self.read_dir_each(&inode, |e| {
            let name = e.name_str();
            if name != "." && name != ".." {
                empty = false;
                return false;
            }
            true
        })?;
        if !empty {
            return Ok(false);
        }
        Ok(true)
    }

    pub fn add_dir_entry(
        &mut self, dir_ino: u32, name: &str, child_ino: u32, file_type: u8,
    ) -> Result<(), FsError> {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len();

        // A directory entry stores its name length in one byte, and a longer
        // name would overrun the block buffer
        if name_len == 0 || name_len > MAX_NAME {
            return Err(FsError::NameTooLong);
        }
        // "." and ".." come from `ext2_create_dir` and are structural
        if name == "." || name == ".." {
            return Err(FsError::AlreadyExists);
        }

        let mut inode = self.read_inode(dir_ino)?;
        let bs = self.block_size as usize;
        let usable = self.dir_block_usable();
        let needed = ((8 + name_len + 3) / 4) * 4;
        // An entry has to fit in one block, header included
        if needed > usable {
            return Err(FsError::NameTooLong);
        }

        // An indexed directory places an entry by its name's hash, since
        // Linux's lookup asks the index and does not scan
        if self.dir_is_indexed(&inode) {
            return self.htree_insert(dir_ino, name, child_ino, file_type);
        }

        let num_blocks = if inode.size() == 0 { 0 }
        else { ((inode.size() + bs as u64 - 1) / bs as u64) as usize };

        let mut block_data = [0u8; 4096];
        for b in 0..num_blocks {
            let phys = self.get_file_block(&inode, b as u32)?;
            if phys == 0 { continue; }

            self.read_block_into(phys, &mut block_data[..bs])?;
            if insert_into_dir_block(&mut block_data, usable, name_bytes, child_ino, file_type) {
                self.write_dir_block(dir_ino, phys, &mut block_data)?;
                return Ok(());
            }
        }

        // No room. On a filesystem indexing directories, Linux builds the index
        // now too: the directory needs a second block and all of it fits in one
        // leaf
        if num_blocks == 1 && self.superblock.has_dir_index() {
            match self.htree_build(dir_ino) {
                Ok(()) => return self.htree_insert(dir_ino, name, child_ino, file_type),
                // An index that cannot be built, as with an unknown hash
                // version, does not refuse the file. The directory stays linear
                Err(FsError::UnsupportedFeature) => {}
                Err(e) => return Err(e),
            }
            inode = self.read_inode(dir_ino)?;
        }

        let logical = num_blocks as u32;
        let new_block = self.dir_grow_block(dir_ino, &mut inode, logical)?;

        let mut block_data = [0u8; 4096];
        self.init_dirent_tail(&mut block_data[..bs]);
        // The record runs to the start of the tail, not the block's end, or the
        // first insert overwrites the checksum record
        block_data[0..4].copy_from_slice(&child_ino.to_le_bytes());
        block_data[4..6].copy_from_slice(&(usable as u16).to_le_bytes());
        block_data[6] = name_len as u8;
        block_data[7] = file_type;
        block_data[8..8+name_len].copy_from_slice(name_bytes);
        self.write_dir_block(dir_ino, new_block, &mut block_data)?;

        let now = self.get_timestamp();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(dir_ino, &inode)?;
        Ok(())
    }

    pub fn remove_dir_entry(&mut self, dir_ino: u32, name: &str) -> Result<(), FsError> {
        let inode = self.read_inode(dir_ino)?;
        let bs = self.block_size as usize;
        let name_bytes = name.as_bytes();
        let num_blocks = if inode.size_lo() == 0 { 0 }
        else { (inode.size_lo() as usize + bs - 1) / bs };

        // Block 0 of an indexed directory is the root, which a linear scan
        // skips by rule. Interior nodes are covered by their own fake entry
        let start_block = if self.dir_is_indexed(&inode) { 1 } else { 0 };

        // An indexed directory names the one leaf holding the name. An index
        // that cannot be walked falls back to the scan below, as a lookup does
        let blocks = if self.dir_is_indexed(&inode) {
            match self.htree_find(dir_ino, &inode, name) {
                Ok(Some((leaf, _))) => leaf..leaf + 1,
                Ok(None) => return Err(FsError::NotFound),
                Err(_) => start_block..num_blocks as u32,
            }
        } else {
            start_block..num_blocks as u32
        };

        for b in blocks {
            let b = b as usize;
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
                    self.write_dir_block(dir_ino, phys, &mut block_data)?;
                    return Ok(());
                }
                prev_pos = Some(pos);
                pos += rec_len;
            }
        }
        Err(FsError::NotFound)
    }
}
