use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

impl MikuFS {
    pub fn ext4_read_inline(
        &mut self,
        inode: &Inode,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if !inode.has_inline_data() {
            return Err(FsError::UnsupportedFeature);
        }
        let size = inode.size_lo() as usize;
        let off = offset as usize;
        if off >= size || off >= 60 {
            return Ok(0);
        }
        let avail = size - off;
        let to_read = buf.len().min(avail).min(60 - off);
        let data = inode.read_inline_data(size);
        if off < data.len() {
            let copy_len = to_read.min(data.len() - off);
            buf[..copy_len].copy_from_slice(&data[off..off + copy_len]);
            Ok(copy_len)
        } else {
            Ok(0)
        }
    }

    pub fn ext4_write_inline(
        &mut self,
        inode_num: u32,
        data: &[u8],
        offset: u64,
    ) -> Result<usize, FsError> {
        if data.len() + offset as usize > 60 {
            return self.ext4_convert_inline_to_extents(inode_num, data, offset);
        }
        let mut inode = self.read_inode(inode_num)?;
        let off = offset as usize;
        let mut buf = [0u8; 60];
        let old_size = inode.size_lo() as usize;
        if old_size > 0 && old_size <= 60 {
            buf[..old_size].copy_from_slice(&inode.data[40..40 + old_size]);
        }
        let end = off + data.len();
        buf[off..end].copy_from_slice(data);
        inode.write_inline_data(&buf[..end]);
        let new_size = end.max(old_size);
        inode.set_size(new_size as u32);
        inode.set_flags(inode.flags() | EXT4_INLINE_DATA_FL);
        inode.set_blocks(0);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(data.len())
    }

    fn ext4_convert_inline_to_extents(
        &mut self,
        inode_num: u32,
        new_data: &[u8],
        offset: u64,
    ) -> Result<usize, FsError> {
        let mut inode = self.read_inode(inode_num)?;
        let old_size = inode.size_lo() as usize;
        let mut old_data = [0u8; 60];
        if old_size > 0 && old_size <= 60 {
            old_data[..old_size].copy_from_slice(&inode.data[40..40 + old_size]);
        }

        inode.clear_block_pointers();
        inode.set_flags(inode.flags() & !EXT4_INLINE_DATA_FL);
        inode.init_extent_header(4);
        inode.set_size(0);
        inode.set_blocks(0);
        self.write_inode(inode_num, &inode)?;

        if old_size > 0 {
            self.ext4_write_file(inode_num, &old_data[..old_size], 0)?;
        }
        self.ext4_write_file(inode_num, new_data, offset)
    }

    pub fn ext4_can_inline(&self, size: usize) -> bool {
        size <= 60
    }
}
