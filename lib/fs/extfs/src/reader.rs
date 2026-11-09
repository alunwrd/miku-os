use super::structs::*;
use crate::error::FsError;
use crate::host::ExtHost;

/// Filesystem view of a block device: what moves sectors, plus the partition
/// start every block number is relative to. The one place knowing a filesystem
/// has an offset and turning a block into a sector
pub struct DiskReader<H: ExtHost> {
    pub host:      H,
    pub start_lba: u32,
    pub io_count:  u32,
    /// Refuses every write through this view, the one chokepoint for ext
    /// writes, so a read-only mount cannot dirty the medium through a file
    /// write, atime stamp, or mount counter
    pub read_only: bool,
}

impl<H: ExtHost> DiskReader<H> {
    pub const fn new(host: H) -> Self {
        Self { host, start_lba: 0, io_count: 0, read_only: false }
    }

    pub const fn new_partitioned(host: H, start_lba: u32) -> Self {
        Self { host, start_lba, io_count: 0, read_only: false }
    }

    /// Same, but every write through the returned view fails with ReadOnlyFs
    pub const fn new_read_only(host: H, start_lba: u32) -> Self {
        Self { host, start_lba, io_count: 0, read_only: true }
    }

    pub fn reset_io(&mut self) { self.io_count = 0; }

    #[inline]
    fn abs(&self, lba: u32) -> u64 {
        self.start_lba as u64 + lba as u64
    }

    pub fn read_sector(&mut self, lba: u32, buf: &mut [u8; 512]) -> Result<(), FsError> {
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.read_sectors(at, 1, buf)?)
    }

    pub fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.write_sectors(at, 1, buf)?)
    }

    /// Ordered write-through sector write, for journal records and other data
    /// whose on-disk order matters
    pub fn write_sector_sync(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.write_sectors_sync(at, 1, buf)?)
    }

    /// Ordered write-through block write (see 'write_sector_sync')
    pub fn write_block_sync(
        &mut self, lba: u32, buf: &[u8], sectors: u8,
    ) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.write_sectors_sync(at, sectors as u32, buf)?)
    }

    /// `write_block` for a run of blocks, any number of sectors: into the block
    /// layer's write-back cache
    pub fn write_blocks(
        &mut self, lba: u32, buf: &[u8], sectors: u32,
    ) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.write_sectors(at, sectors, buf)?)
    }

    /// `write_block_sync` for a run of blocks: file data past the block cache
    /// (`MikuFS::write_data_blocks`), any number of sectors
    pub fn write_blocks_sync(
        &mut self, lba: u32, buf: &[u8], sectors: u32,
    ) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.write_sectors_sync(at, sectors, buf)?)
    }

    /// Barrier write: on stable media on return (FUA on capable devices, write
    /// plus flush otherwise). For the journal commit block
    pub fn write_block_barrier(
        &mut self, lba: u32, buf: &[u8], sectors: u8,
    ) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.write_sectors_barrier(at, sectors as u32, buf)?)
    }

    pub fn read_block(
        &mut self, lba: u32, buf: &mut [u8], sectors: u8,
    ) -> Result<(), FsError> {
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.read_sectors(at, sectors as u32, buf)?)
    }

    pub fn write_block(
        &mut self, lba: u32, buf: &[u8], sectors: u8,
    ) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.abs(lba);
        Ok(self.host.write_sectors(at, sectors as u32, buf)?)
    }

    /// Tells the device a run of sectors holds nothing useful. Relative to the
    /// partition
    pub fn discard(&mut self, lba: u64, count: u32) -> Result<(), FsError> {
        if self.read_only { return Err(FsError::ReadOnlyFs); }
        self.io_count += 1;
        let at = self.start_lba as u64 + lba;
        Ok(self.host.discard(at, count)?)
    }

    pub fn flush_drive(&mut self) {
        if self.read_only { return; }
        self.io_count += 1;
        let _ = self.host.flush();
    }

    pub fn read_superblock(&mut self) -> Result<Superblock, FsError> {
        let mut sb = Superblock::zeroed();
        let mut buf = [0u8; 1024];
        self.read_block(2, &mut buf, 2)?;
        sb.data.copy_from_slice(&buf);
        Ok(sb)
    }

    pub fn read_group_descriptors(
        &mut self,
        gdt_block: u32,
        block_size: u32,
        count: usize,
        gd_size: usize,
        groups: &mut [GroupDesc],
    ) -> Result<(), FsError> {
        let total_bytes = count * gd_size;
        let blocks_needed = (total_bytes as u32 + block_size - 1) / block_size;
        let sectors_per_block = block_size / 512;
        let start_lba = gdt_block * sectors_per_block;
        let total_sectors = blocks_needed * sectors_per_block;

        let mut sector_buf = [0u8; 512];
        let mut gd_idx = 0usize;
        let mut carry_buf = [0u8; 64];
        let mut carry_len = 0usize;

        for s in 0..total_sectors {
            self.read_sector(start_lba + s, &mut sector_buf)?;
            let mut pos = 0usize;

            if carry_len > 0 {
                let need = gd_size - carry_len;
                carry_buf[carry_len..gd_size].copy_from_slice(&sector_buf[..need]);
                if gd_idx < count {
                    groups[gd_idx].data[..gd_size].copy_from_slice(&carry_buf[..gd_size]);
                    gd_idx += 1;
                }
                pos = need;
                carry_len = 0;
            }

            while pos + gd_size <= 512 && gd_idx < count {
                groups[gd_idx].data[..gd_size].copy_from_slice(&sector_buf[pos..pos + gd_size]);
                gd_idx += 1;
                pos += gd_size;
            }

            if pos < 512 && gd_idx < count {
                let remaining = 512 - pos;
                carry_buf[..remaining].copy_from_slice(&sector_buf[pos..]);
                carry_len = remaining;
            }
        }

        Ok(())
    }

    pub fn read_inode(
        &mut self,
        inode_num: u32,
        sb: &Superblock,
        groups: &[GroupDesc],
    ) -> Result<Inode, FsError> {
        if inode_num == 0 {
            return Err(FsError::InvalidInode);
        }

        let inodes_per_group = sb.inodes_per_group();
        let inode_size = sb.inode_size_val();
        let block_size = sb.block_size();

        let idx = inode_num - 1;
        let group = (idx / inodes_per_group) as usize;
        let local_idx = idx % inodes_per_group;

        if group >= groups.len() {
            return Err(FsError::InvalidInode);
        }

        let inode_table_block = groups[group].inode_table();
        let byte_offset = local_idx as u64 * inode_size as u64;
        let abs_byte = inode_table_block as u64 * block_size as u64 + byte_offset;
        let sector = (abs_byte / 512) as u32;
        let offset_in_sector = (abs_byte % 512) as usize;

        let mut inode = Inode::zeroed();
        let read_size = (inode_size as usize).min(256);
        inode.on_disk_size = read_size as u16;

        let mut buf = [0u8; 512];
        self.read_sector(sector, &mut buf)?;

        if offset_in_sector + read_size <= 512 {
            inode.data[..read_size]
                .copy_from_slice(&buf[offset_in_sector..offset_in_sector + read_size]);
        } else {
            let first_part = 512 - offset_in_sector;
            inode.data[..first_part].copy_from_slice(&buf[offset_in_sector..512]);

            let mut remaining = read_size - first_part;
            let mut data_pos = first_part;
            let mut next_sector = sector + 1;

            while remaining > 0 {
                self.read_sector(next_sector, &mut buf)?;
                let chunk = remaining.min(512);
                inode.data[data_pos..data_pos + chunk].copy_from_slice(&buf[..chunk]);
                data_pos += chunk;
                remaining -= chunk;
                next_sector += 1;
            }
        }

        Ok(inode)
    }
}
