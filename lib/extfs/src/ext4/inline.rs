// Inline data
//
// A file small enough for its inode has no blocks: the first sixty bytes live
// in `i_block`, and anything past that in the inode's extended-attribute area
// under `system.data`. Its block pointers are the file.

use crate::structs::*;
use crate::{FsError, MikuFS};
use crate::host::ExtHost;

/// Bytes of an inline file that live in `i_block`
const INLINE_IN_INODE: usize = 60;
/// The magic that marks the inode's extended-attribute area
const XATTR_MAGIC: u32 = 0xEA02_0000;

/// The part of an inline file in the inode's xattr area, as `system.data`.
/// Always in the inode body; a tail that no longer fits is converted to
/// block-mapped
fn inline_tail(inode: &Inode, inode_size: usize) -> &[u8] {
    let extra = inode.extra_isize() as usize;
    let start = 128 + extra;
    if inode_size <= 128 || extra == 0 || start + 4 > inode_size {
        return &[];
    }
    let magic = u32::from_le_bytes([
        inode.data[start],
        inode.data[start + 1],
        inode.data[start + 2],
        inode.data[start + 3],
    ]);
    if magic != XATTR_MAGIC {
        return &[];
    }

    // Entry and value offsets are measured from just past the magic
    let base = start + 4;
    let mut at = base;
    while at + 16 <= inode_size {
        let name_len = inode.data[at] as usize;
        let name_index = inode.data[at + 1];
        if name_len == 0 && name_index == 0 {
            break;
        }
        let value_offs = u16::from_le_bytes([inode.data[at + 2], inode.data[at + 3]]) as usize;
        let value_size = u32::from_le_bytes([
            inode.data[at + 8],
            inode.data[at + 9],
            inode.data[at + 10],
            inode.data[at + 11],
        ]) as usize;
        let name_at = at + 16;
        if name_at + name_len > inode_size {
            break;
        }
        if name_index == XATTR_INDEX_SYSTEM
            && name_len == 4
            && &inode.data[name_at..name_at + 4] == b"data"
        {
            let from = base + value_offs;
            if from + value_size <= inode_size {
                return &inode.data[from..from + value_size];
            }
            return &[];
        }
        at = (name_at + name_len + 3) & !3;
    }
    &[]
}

/// The namespace index `system.data` lives in
const XATTR_INDEX_SYSTEM: u8 = 7;

impl<H: ExtHost> MikuFS<H> {
    /// Inline capacity of this inode: sixty bytes in `i_block`, plus what its
    /// extended-attribute area holds
    pub fn inline_capacity(&self, inode: &Inode) -> usize {
        let inode_size = self.inode_size() as usize;
        let extra = inode.extra_isize() as usize;
        if inode_size <= 128 || extra == 0 {
            return INLINE_IN_INODE;
        }
        // 4 bytes of magic, a 16-byte entry header, the four-byte name, and the
        // four-byte terminator
        let overhead = 4 + 16 + 4 + 4;
        let area = inode_size - 128 - extra;
        INLINE_IN_INODE + area.saturating_sub(overhead)
    }

    /// Copies a whole inline file out, returning bytes written into `out`
    pub fn ext4_inline_content(&self, inode: &Inode, out: &mut [u8]) -> usize {
        let size = (inode.size_lo() as usize).min(out.len());
        let head_len = size.min(INLINE_IN_INODE);
        out[..head_len].copy_from_slice(&inode.data[40..40 + head_len]);
        if size <= INLINE_IN_INODE {
            return head_len;
        }
        let tail = inline_tail(inode, self.inode_size() as usize);
        let want = size - INLINE_IN_INODE;
        let take = want.min(tail.len());
        out[INLINE_IN_INODE..INLINE_IN_INODE + take].copy_from_slice(&tail[..take]);
        INLINE_IN_INODE + take
    }

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
        if off >= size {
            return Ok(0);
        }
        let mut whole = [0u8; 256];
        let have = self.ext4_inline_content(inode, &mut whole);
        if off >= have {
            // The inode claims more than is stored: a corrupt inode
            return Err(FsError::CorruptedFs);
        }
        let to_copy = buf.len().min(have - off);
        buf[..to_copy].copy_from_slice(&whole[off..off + to_copy]);
        Ok(to_copy)
    }

    pub fn ext4_write_inline(
        &mut self,
        inode_num: u32,
        data: &[u8],
        offset: u64,
    ) -> Result<usize, FsError> {
        let inode = self.read_inode(inode_num)?;
        let end = offset as usize + data.len();
        // Only the sixty bytes in `i_block` are written in place; anything past
        // them converts to blocks
        if end > INLINE_IN_INODE {
            return self.ext4_convert_inline_to_extents(inode_num, data, offset);
        }
        let mut inode = inode;
        let off = offset as usize;
        let mut buf = [0u8; 60];
        let old_size = inode.size_lo() as usize;
        if old_size > 0 && old_size <= INLINE_IN_INODE {
            buf[..old_size].copy_from_slice(&inode.data[40..40 + old_size]);
        }
        buf[off..end].copy_from_slice(data);
        inode.write_inline_data(&buf[..end]);
        let new_size = end.max(old_size.min(INLINE_IN_INODE));
        inode.set_size(new_size as u32);
        inode.set_flags(inode.flags() | EXT4_INLINE_DATA_FL);
        inode.set_blocks(0);
        let now = self.get_timestamp();
        inode.set_mtime(now);
        self.write_inode(inode_num, &inode)?;
        Ok(data.len())
    }

    /// Moves a whole inline file to blocks, including the xattr part
    pub fn ext4_convert_inline_to_extents(
        &mut self,
        inode_num: u32,
        new_data: &[u8],
        offset: u64,
    ) -> Result<usize, FsError> {
        let mut inode = self.read_inode(inode_num)?;
        let mut old = [0u8; 256];
        let old_len = self.ext4_inline_content(&inode, &mut old);

        inode.clear_block_pointers();
        inode.set_flags(inode.flags() & !EXT4_INLINE_DATA_FL);
        inode.init_extent_header(4);
        inode.set_size(0);
        inode.set_blocks(0);
        self.write_inode(inode_num, &inode)?;

        if old_len > 0 {
            self.ext4_write_file(inode_num, &old[..old_len], 0)?;
        }
        self.ext4_write_file(inode_num, new_data, offset)
    }

    pub fn ext4_can_inline(&self, size: usize) -> bool {
        size <= INLINE_IN_INODE
    }
}
