// Metadata checksums: computing and checking
//
// Each chains from one seed: the volume UUID folded into `~0`, or with
// INCOMPAT_CSUM_SEED one stored by the filesystem, so `tune2fs -U` can change
// the UUID.
//
// The checksum field is fed in as zeros, not skipped: two zero bytes go through
// the accumulator where it sits, since CRC depends on length too. Checked
// against a real mke2fs image.
//
// The accumulation does not invert; see `crc32c_raw`, where the pieces are
// chained.
//
// A round trip cannot catch these mistakes, as compute and verify share them.
// `e2fsck` does.

use super::crc32c;
use crate::host::ExtHost;
use crate::structs::*;
use crate::{FsError, MikuFS};

/// Offset of `bg_checksum` within a group descriptor
const GD_CSUM_OFF: usize = 30;
/// Offset of `i_checksum_lo` within an inode
const INODE_CSUM_LO_OFF: usize = 124;
/// Offset of `i_checksum_hi`, in the extra area past the classic 128-byte inode
const INODE_CSUM_HI_OFF: usize = 130;
/// Inode size before `s_inode_size` existed
const GOOD_OLD_INODE_SIZE: usize = 128;
/// Where a directory index block keeps its `dx_entry` array. The root, block 0
/// of the directory, has two fake dirents (`.` and `..`) and `dx_root_info`
/// ahead of the array; an interior node has only an eight-byte fake dirent,
/// which an index-unaware reader sees as an empty block
pub const DX_ROOT_ENTRIES: usize = 32;
pub const DX_NODE_ENTRIES: usize = 8;

const XATTR_CSUM_OFF: usize = 16;

impl<H: ExtHost> MikuFS<H> {
    /// The value every metadata checksum on this volume chains from
    pub fn csum_seed(&self) -> u32 {
        let stored = if self.superblock.has_csum_seed() {
            Some(self.superblock.checksum_seed())
        } else {
            None
        };
        crc32c::ext4_csum_seed(self.superblock.uuid(), stored)
    }

    /// Whether this volume's checksums are computable. `s_checksum_type` has
    /// one defined value; computing crc32c for another would corrupt each
    /// metadata block written
    pub fn csum_algorithm_known(&self) -> bool {
        !self.superblock.has_metadata_csum() || self.superblock.checksum_type() == 1
    }

    // super

    pub fn compute_superblock_csum(&self) -> u32 {
        crc32c::ext4_superblock_csum(&self.superblock.data)
    }

    pub fn verify_superblock_csum(&self) -> bool {
        if !self.superblock.has_metadata_csum() {
            return true;
        }
        self.compute_superblock_csum() == self.superblock.checksum()
    }

    pub fn update_superblock_csum(&mut self) {
        if !self.superblock.has_metadata_csum() {
            return;
        }
        let csum = self.compute_superblock_csum();
        self.superblock.set_checksum(csum);
    }

    // group desc

    /// Sums a group descriptor, skipping its checksum field. `metadata_csum`
    /// uses crc32c seeded from the volume seed; older `GDT_CSUM` (uninit_bg)
    /// alone uses crc16 over the raw UUID
    pub fn compute_group_desc_csum(&self, group: usize) -> u16 {
        let gd_size = (self.superblock.group_desc_size() as usize).min(64);
        let data = &self.groups[group].data;
        let before = &data[..GD_CSUM_OFF];
        let after = &data[GD_CSUM_OFF + 2..gd_size];
        // Where the checksum lives, two zero bytes go through instead
        const ZERO: [u8; 2] = [0, 0];

        if self.superblock.has_metadata_csum() {
            let seed = self.csum_seed();
            let crc = crc32c::crc32c_raw(seed, &(group as u32).to_le_bytes());
            let crc = crc32c::crc32c_raw(crc, before);
            let crc = crc32c::crc32c_raw(crc, &ZERO);
            let crc = crc32c::crc32c_raw(crc, after);
            (crc & 0xFFFF) as u16
        } else {
            let crc = crc32c::crc16(0xFFFF, self.superblock.uuid());
            let crc = crc32c::crc16(crc, &(group as u32).to_le_bytes());
            let crc = crc32c::crc16(crc, before);
            let crc = crc32c::crc16(crc, &ZERO);
            crc32c::crc16(crc, after)
        }
    }

    pub fn verify_group_desc_csum(&self, group: usize) -> bool {
        if group >= self.groups.len() {
            return false;
        }
        if !self.has_gdt_csum() {
            return true;
        }
        self.compute_group_desc_csum(group) == self.groups[group].checksum()
    }

    pub fn update_group_desc_csum(&mut self, group: usize) {
        if group >= self.groups.len() || !self.has_gdt_csum() {
            return;
        }
        let computed = self.compute_group_desc_csum(group);
        self.groups[group].write_u16(GD_CSUM_OFF, computed);
    }

    pub fn has_gdt_csum(&self) -> bool {
        self.superblock.has_metadata_csum() || self.superblock.has_gdt_csum()
    }

    // inode

    /// Whether this inode is large enough for `i_checksum_hi`: Linux's
    /// EXT4_FITS_IN_INODE. The field ends at 132, and the usable extent is 128
    /// plus `i_extra_isize`
    fn inode_has_csum_hi(&self, inode: &Inode) -> bool {
        let size = inode.on_disk_size as usize;
        size > GOOD_OLD_INODE_SIZE
            && GOOD_OLD_INODE_SIZE + inode.extra_isize() as usize >= INODE_CSUM_HI_OFF + 2
    }

    /// Sums an inode, with zeros for its own checksum. Order: the seed, inode
    /// number, generation, then the inode with `i_checksum_lo` at 124 replaced
    /// by two zero bytes and, if large enough, `i_checksum_hi` at 130 the same
    /// way. Checked against a real mke2fs image
    pub fn compute_inode_csum_value(&self, inode_num: u32, inode: &Inode) -> u32 {
        const ZERO: [u8; 2] = [0, 0];
        let size = (inode.on_disk_size as usize).min(256);
        let seed = self.csum_seed();
        let gen = inode.generation();

        let crc = crc32c::crc32c_raw(seed, &inode_num.to_le_bytes());
        let crc = crc32c::crc32c_raw(crc, &gen.to_le_bytes());

        let crc = crc32c::crc32c_raw(crc, &inode.data[..INODE_CSUM_LO_OFF]);
        let crc = crc32c::crc32c_raw(crc, &ZERO);
        let tail = size.min(GOOD_OLD_INODE_SIZE);
        let crc = crc32c::crc32c_raw(crc, &inode.data[INODE_CSUM_LO_OFF + 2..tail]);

        if size <= GOOD_OLD_INODE_SIZE {
            return crc;
        }

        // The extra area, up to i_checksum_hi
        let crc = crc32c::crc32c_raw(crc, &inode.data[GOOD_OLD_INODE_SIZE..INODE_CSUM_HI_OFF]);
        if self.inode_has_csum_hi(inode) {
            let crc = crc32c::crc32c_raw(crc, &ZERO);
            crc32c::crc32c_raw(crc, &inode.data[INODE_CSUM_HI_OFF + 2..size])
        } else {
            crc32c::crc32c_raw(crc, &inode.data[INODE_CSUM_HI_OFF..size])
        }
    }

    /// Compares every bit written
    pub fn verify_inode_csum(&self, inode_num: u32, inode: &Inode) -> bool {
        if !self.superblock.has_metadata_csum() {
            return true;
        }
        let computed = self.compute_inode_csum_value(inode_num, inode);
        let stored_lo = inode.checksum_lo();
        if !self.inode_has_csum_hi(inode) {
            return (computed & 0xFFFF) as u16 == stored_lo;
        }
        let stored = (stored_lo as u32) | ((inode.checksum_hi() as u32) << 16);
        computed == stored
    }

    pub fn stamp_inode_csum(&self, inode_num: u32, inode: &mut Inode) {
        if !self.superblock.has_metadata_csum() {
            return;
        }
        let has_hi = self.inode_has_csum_hi(inode);
        // Zeroes both halves before summing
        inode.write_u16(INODE_CSUM_LO_OFF, 0);
        if has_hi {
            inode.write_u16(INODE_CSUM_HI_OFF, 0);
        }
        let csum = self.compute_inode_csum_value(inode_num, inode);
        inode.write_u16(INODE_CSUM_LO_OFF, (csum & 0xFFFF) as u16);
        if has_hi {
            inode.write_u16(INODE_CSUM_HI_OFF, ((csum >> 16) & 0xFFFF) as u16);
        }
    }

    // directory

    /// Bytes a directory block reserves for its checksum record. On
    /// `metadata_csum` the last 12 bytes of each directory block are a fake
    /// entry (inode 0, rec_len 12, file_type 0xDE) whose last four bytes
    /// checksum everything before. Readers ignoring inode 0 skip it
    pub fn dirent_tail_len(&self) -> usize {
        if self.superblock.has_metadata_csum() { 12 } else { 0 }
    }

    /// How much of a directory block real entries may occupy
    pub fn dir_block_usable(&self) -> usize {
        self.block_size as usize - self.dirent_tail_len()
    }

    /// Lay down the tail record in a freshly created directory block
    pub fn init_dirent_tail(&self, buf: &mut [u8]) {
        if self.dirent_tail_len() == 0 {
            return;
        }
        let bs = self.block_size as usize;
        let at = bs - 12;
        buf[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        buf[at + 4..at + 6].copy_from_slice(&12u16.to_le_bytes());
        buf[at + 6] = 0;
        buf[at + 7] = 0xDE;
        buf[at + 8..at + 12].copy_from_slice(&0u32.to_le_bytes());
    }

    /// Recomputes a directory block's checksum in place, right before it goes
    /// to disk
    pub fn stamp_dir_block(&self, dir_ino: u32, gen: u32, buf: &mut [u8]) {
        if self.dirent_tail_len() == 0 {
            return;
        }
        let bs = self.block_size as usize;
        let at = bs - 12;
        // Only stamps a real tail; a checksum written over a real entry would
        // destroy it
        if buf[at + 7] != 0xDE || u16::from_le_bytes([buf[at + 4], buf[at + 5]]) != 12 {
            return;
        }
        let seed = self.csum_seed();
        let csum = crc32c::ext4_dirent_csum(seed, dir_ino, gen, &buf[..at]);
        buf[at + 8..at + 12].copy_from_slice(&csum.to_le_bytes());
    }

    // extents

    /// Where the four-byte `ext4_extent_tail` sits in an extent tree block, or
    /// `None` when this volume has none.
    ///
    /// Linux's `EXT4_EXTENT_TAIL_OFFSET`: right past the last entry the header
    /// says fits, not the block's end. For most block sizes these match, since
    /// `(bs - 12) % 12` is four; for 2 KiB it is eight, with four unused bytes
    /// after the tail. Read from a real image.
    ///
    /// `eh_max` is not reduced for the tail
    pub fn extent_tail_offset(&self, eh_max: u16) -> Option<usize> {
        if !self.superblock.has_metadata_csum() {
            return None;
        }
        let at = 12 + eh_max as usize * 12;
        if at + 4 > self.block_size as usize {
            return None;
        }
        Some(at)
    }

    /// Recomputes an extent tree block's checksum in place, right before it
    /// goes to disk. Keyed by the owning inode and generation, so a block
    /// reused by another file does not validate
    pub fn stamp_extent_block(&self, inode_num: u32, gen: u32, buf: &mut [u8]) {
        let eh_max = u16::from_le_bytes([buf[4], buf[5]]);
        let Some(at) = self.extent_tail_offset(eh_max) else {
            return;
        };
        let seed = self.csum_seed();
        let csum = crc32c::ext4_extent_csum(seed, inode_num, gen, &buf[..at]);
        buf[at..at + 4].copy_from_slice(&csum.to_le_bytes());
    }

    /// Verifies an extent tree block against its checksum. `true` when there is
    /// none
    pub fn verify_extent_block(&self, inode_num: u32, gen: u32, buf: &[u8]) -> bool {
        let eh_max = u16::from_le_bytes([buf[4], buf[5]]);
        let Some(at) = self.extent_tail_offset(eh_max) else {
            return true;
        };
        let seed = self.csum_seed();
        let stored = u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
        crc32c::ext4_extent_csum(seed, inode_num, gen, &buf[..at]) == stored
    }

    /// Verifies a directory block against its checksum. `true` when there is
    /// none
    pub fn verify_dir_block(&self, dir_ino: u32, gen: u32, buf: &[u8]) -> bool {
        if self.dirent_tail_len() == 0 {
            return true;
        }
        let bs = self.block_size as usize;
        let at = bs - 12;
        if buf[at + 7] != 0xDE || u16::from_le_bytes([buf[at + 4], buf[at + 5]]) != 12 {
            // No tail where expected: cannot be judged
            return true;
        }
        let seed = self.csum_seed();
        let stored = u32::from_le_bytes([buf[at + 8], buf[at + 9], buf[at + 10], buf[at + 11]]);
        crc32c::ext4_dirent_csum(seed, dir_ino, gen, &buf[..at]) == stored
    }

    // directory index (htree)

    /// `dx_entry` slots in a directory index block: e2fsprogs'
    /// `ext2fs_htree_intnode_maxrecs`. The checksum record comes out of the
    /// array's space, so `metadata_csum` holds one entry fewer per node. The
    /// extent tree differs: `eh_max` stays and the tail follows the last entry.
    /// Both read from real `mke2fs` images
    pub fn dx_limit(&self, entries_off: usize) -> u16 {
        let csum = if self.superblock.has_metadata_csum() { 8 } else { 0 };
        let bs = self.block_size as usize;
        if bs <= entries_off + csum {
            return 0;
        }
        ((bs - entries_off - csum) / 8) as u16
    }

    /// Where the eight-byte `dx_tail` sits: right past the last entry the node
    /// holds, which with the reduced limit is the block's end
    pub fn dx_tail_offset(&self, entries_off: usize, limit: u16) -> Option<usize> {
        if !self.superblock.has_metadata_csum() {
            return None;
        }
        let at = entries_off + limit as usize * 8;
        if at + 8 > self.block_size as usize {
            return None;
        }
        Some(at)
    }

    /// Recomputes a directory index block's checksum in place, right before
    /// disk. `entries_off` names the kind: `DX_ROOT_ENTRIES` for block 0,
    /// `DX_NODE_ENTRIES` for an interior node
    pub fn stamp_dx_block(&self, dir_ino: u32, gen: u32, entries_off: usize, buf: &mut [u8]) {
        let Some((at, count)) = self.dx_csum_site(entries_off, buf) else {
            return;
        };
        let csum = self.dx_csum(dir_ino, gen, entries_off, buf, at, count);
        buf[at + 4..at + 8].copy_from_slice(&csum.to_le_bytes());
    }

    /// Verifies a directory index block against its checksum. `true` when there
    /// is none
    pub fn verify_dx_block(&self, dir_ino: u32, gen: u32, entries_off: usize, buf: &[u8]) -> bool {
        let Some((at, count)) = self.dx_csum_site(entries_off, buf) else {
            return true;
        };
        let stored = u32::from_le_bytes([buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7]]);
        self.dx_csum(dir_ino, gen, entries_off, buf, at, count) == stored
    }

    /// The tail's offset and the entry count covered. `None` when the volume
    /// keeps no checksums, or the header does not describe a node of this size
    fn dx_csum_site(&self, entries_off: usize, buf: &[u8]) -> Option<(usize, u16)> {
        let bs = self.block_size as usize;
        if buf.len() < bs || entries_off + 4 > bs {
            return None;
        }
        let limit = u16::from_le_bytes([buf[entries_off], buf[entries_off + 1]]);
        let count = u16::from_le_bytes([buf[entries_off + 2], buf[entries_off + 3]]);
        if count > limit {
            return None;
        }
        let at = self.dx_tail_offset(entries_off, limit)?;
        if entries_off + count as usize * 8 > at {
            return None;
        }
        Some((at, count))
    }

    /// Covers only entries in use, so it changes with a node's count. Keyed by
    /// the owning inode like a leaf's tail, so `ext4_dirent_csum` is reused and
    /// the tail bytes folded in after
    fn dx_csum(
        &self,
        dir_ino: u32,
        gen: u32,
        entries_off: usize,
        buf: &[u8],
        at: usize,
        count: u16,
    ) -> u32 {
        let seed = self.csum_seed();
        let size = entries_off + count as usize * 8;
        crc32c::ext4_dx_csum(seed, dir_ino, gen, &buf[..size], &buf[at..at + 8])
    }

    // xattr

    /// Recomputes an extended-attribute block's checksum. Not keyed by an
    /// inode, as several may share the block: keyed by its block number, as a
    /// 64-bit little-endian value
    pub fn stamp_xattr_block(&self, block: u64, buf: &mut [u8]) {
        if !self.superblock.has_metadata_csum() {
            return;
        }
        let csum = self.xattr_block_csum(block, buf);
        buf[XATTR_CSUM_OFF..XATTR_CSUM_OFF + 4].copy_from_slice(&csum.to_le_bytes());
    }

    pub fn verify_xattr_block(&self, block: u64, buf: &[u8]) -> bool {
        if !self.superblock.has_metadata_csum() {
            return true;
        }
        let stored = u32::from_le_bytes([
            buf[XATTR_CSUM_OFF],
            buf[XATTR_CSUM_OFF + 1],
            buf[XATTR_CSUM_OFF + 2],
            buf[XATTR_CSUM_OFF + 3],
        ]);
        self.xattr_block_csum(block, buf) == stored
    }

    fn xattr_block_csum(&self, block: u64, buf: &[u8]) -> u32 {
        const ZERO: [u8; 4] = [0; 4];
        let bs = (self.block_size as usize).min(buf.len());
        let seed = self.csum_seed();
        let crc = crc32c::crc32c_raw(seed, &block.to_le_bytes());
        let crc = crc32c::crc32c_raw(crc, &buf[..XATTR_CSUM_OFF]);
        let crc = crc32c::crc32c_raw(crc, &ZERO);
        crc32c::crc32c_raw(crc, &buf[XATTR_CSUM_OFF + 4..bs])
    }

    // flush

    pub fn flush_superblock_with_csum(&mut self) -> Result<(), FsError> {
        self.update_superblock_csum();
        self.flush_superblock()
    }

    pub fn flush_group_desc_with_csum(&mut self, group: usize) -> Result<(), FsError> {
        self.update_group_desc_csum(group);
        self.flush_group_desc(group)
    }

    // bitmaps

    fn read_bitmap_block(&mut self, bitmap_block: u32) -> Result<[u8; 4096], FsError> {
        let mut buf = [0u8; 4096];
        let bs = self.block_size as usize;
        self.read_block_into(bitmap_block, &mut buf[..bs])?;
        Ok(buf)
    }

    /// Sets a group's block-bitmap checksum from the caller's copy of the
    /// bitmap, avoiding a re-read per allocation
    pub fn set_block_bitmap_csum(&mut self, group: usize, bitmap: &[u8]) {
        if !self.superblock.has_metadata_csum() || group >= self.groups.len() {
            return;
        }
        let bytes = (self.blocks_per_group as usize)
            .div_ceil(8)
            .min(bitmap.len());
        let seed = self.csum_seed();
        let csum = crc32c::ext4_bitmap_csum(seed, &bitmap[..bytes]);
        self.groups[group].write_u16(24, (csum & 0xFFFF) as u16);
        if self.superblock.group_desc_size() >= 64 {
            self.groups[group].write_u16(56, (csum >> 16) as u16);
        }
        let _ = self.flush_group_desc(group);
    }

    /// The same for the inode bitmap
    pub fn set_inode_bitmap_csum(&mut self, group: usize, bitmap: &[u8]) {
        if !self.superblock.has_metadata_csum() || group >= self.groups.len() {
            return;
        }
        let bytes = (self.inodes_per_group as usize)
            .div_ceil(8)
            .min(bitmap.len());
        let seed = self.csum_seed();
        let csum = crc32c::ext4_bitmap_csum(seed, &bitmap[..bytes]);
        self.groups[group].write_u16(26, (csum & 0xFFFF) as u16);
        if self.superblock.group_desc_size() >= 64 {
            self.groups[group].write_u16(58, (csum >> 16) as u16);
        }
        let _ = self.flush_group_desc(group);
    }

    pub fn compute_block_bitmap_csum(&mut self, group: usize) -> Result<u32, FsError> {
        if group >= self.groups.len() {
            return Ok(0);
        }
        let bitmap_block = self.groups[group].block_bitmap();
        let buf = self.read_bitmap_block(bitmap_block)?;
        let bytes = (self.blocks_per_group as usize).div_ceil(8).min(buf.len());
        let seed = self.csum_seed();
        Ok(crc32c::ext4_bitmap_csum(seed, &buf[..bytes]))
    }

    pub fn compute_inode_bitmap_csum(&mut self, group: usize) -> Result<u32, FsError> {
        if group >= self.groups.len() {
            return Ok(0);
        }
        let bitmap_block = self.groups[group].inode_bitmap();
        let buf = self.read_bitmap_block(bitmap_block)?;
        let bytes = (self.inodes_per_group as usize).div_ceil(8).min(buf.len());
        let seed = self.csum_seed();
        Ok(crc32c::ext4_bitmap_csum(seed, &buf[..bytes]))
    }

    pub fn update_block_bitmap_csum(&mut self, group: usize) -> Result<(), FsError> {
        if !self.superblock.has_metadata_csum() || group >= self.groups.len() {
            return Ok(());
        }
        let csum = self.compute_block_bitmap_csum(group)?;
        let gd_size = self.superblock.group_desc_size() as usize;
        self.groups[group].write_u16(24, (csum & 0xFFFF) as u16);
        if gd_size >= 64 {
            self.groups[group].write_u16(56, ((csum >> 16) & 0xFFFF) as u16);
        }
        Ok(())
    }

    pub fn update_inode_bitmap_csum(&mut self, group: usize) -> Result<(), FsError> {
        if !self.superblock.has_metadata_csum() || group >= self.groups.len() {
            return Ok(());
        }
        let csum = self.compute_inode_bitmap_csum(group)?;
        let gd_size = self.superblock.group_desc_size() as usize;
        self.groups[group].write_u16(26, (csum & 0xFFFF) as u16);
        if gd_size >= 64 {
            self.groups[group].write_u16(58, ((csum >> 16) & 0xFFFF) as u16);
        }
        Ok(())
    }

    pub fn verify_block_bitmap_csum(&mut self, group: usize) -> bool {
        if !self.superblock.has_metadata_csum() {
            return true;
        }
        if group >= self.groups.len() {
            return false;
        }
        let computed = match self.compute_block_bitmap_csum(group) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let stored_lo = self.groups[group].block_bitmap_csum_lo();
        if self.superblock.group_desc_size() as usize >= 64 {
            let stored_hi = self.groups[group].block_bitmap_csum_hi();
            computed == (stored_lo as u32) | ((stored_hi as u32) << 16)
        } else {
            (computed & 0xFFFF) as u16 == stored_lo
        }
    }

    pub fn verify_inode_bitmap_csum(&mut self, group: usize) -> bool {
        if !self.superblock.has_metadata_csum() {
            return true;
        }
        if group >= self.groups.len() {
            return false;
        }
        let computed = match self.compute_inode_bitmap_csum(group) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let stored_lo = self.groups[group].inode_bitmap_csum_lo();
        if self.superblock.group_desc_size() as usize >= 64 {
            let stored_hi = self.groups[group].inode_bitmap_csum_hi();
            computed == (stored_lo as u32) | ((stored_hi as u32) << 16)
        } else {
            (computed & 0xFFFF) as u16 == stored_lo
        }
    }
}
