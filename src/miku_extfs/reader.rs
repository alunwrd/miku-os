use super::structs::*;
use super::FsError;
use crate::ata::AtaDrive;
use crate::vfs::types::BlockDevId;

/// Filesystem-side view of a block device: a stable 'BlockDevId' plus the
/// partition start offset. All I/O is routed through the block layer
/// ('crate::block') rather than touching a driver directly
pub struct DiskReader {
    pub dev_id:    BlockDevId,
    pub start_lba: u32,
    pub io_count:  u32,
}

impl DiskReader {
    pub fn new(drive: AtaDrive) -> Self {
        let dev_id = crate::block::register_ata(drive);
        Self { dev_id, start_lba: 0, io_count: 0 }
    }

    pub fn new_partitioned(drive: AtaDrive, start_lba: u32) -> Self {
        let dev_id = crate::block::register_ata(drive);
        Self { dev_id, start_lba, io_count: 0 }
    }

    /// Construct directly from an already-registered block device id
    /// (virtio-blk and other non-ATA backends)
    pub fn from_dev(dev_id: BlockDevId, start_lba: u32) -> Self {
        Self { dev_id, start_lba, io_count: 0 }
    }

    pub fn reset_io(&mut self) { self.io_count = 0; }

    pub fn read_sector(&mut self, lba: u32, buf: &mut [u8; 512]) -> Result<(), FsError> {
        self.io_count += 1;
        crate::block::read(self.dev_id, (self.start_lba + lba) as u64, 1, buf)
            .map_err(|_| FsError::IoError)
    }

    pub fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), FsError> {
        self.io_count += 1;
        crate::block::write(self.dev_id, (self.start_lba + lba) as u64, 1, buf)
            .map_err(|_| FsError::IoError)
    }

    /// Ordered write-through sector write (journal records and other data
    /// whose on-disk ordering matters)
    pub fn write_sector_sync(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), FsError> {
        self.io_count += 1;
        crate::block::write_sync(self.dev_id, (self.start_lba + lba) as u64, 1, buf)
            .map_err(|_| FsError::IoError)
    }

    /// Ordered write-through block write (see 'write_sector_sync')
    pub fn write_block_sync(
        &mut self, lba: u32, buf: &[u8], sectors: u8,
    ) -> Result<(), FsError> {
        self.io_count += 1;
        crate::block::write_sync(self.dev_id, (self.start_lba + lba) as u64, sectors as u32, buf)
            .map_err(|_| FsError::IoError)
    }

    /// Barrier write: the block is on stable media when this returns (FUA on
    /// capable devices, write-plus-flush otherwise). Used for the journal
    /// commit block, where crash durability is the whole point
    pub fn write_block_barrier(
        &mut self, lba: u32, buf: &[u8], sectors: u8,
    ) -> Result<(), FsError> {
        self.io_count += 1;
        crate::block::write_barrier(self.dev_id, (self.start_lba + lba) as u64, sectors as u32, buf)
            .map_err(|_| FsError::IoError)
    }

    pub fn read_block(
        &mut self, lba: u32, buf: &mut [u8], sectors: u8,
    ) -> Result<(), FsError> {
        self.io_count += 1;
        crate::block::read(self.dev_id, (self.start_lba + lba) as u64, sectors as u32, buf)
            .map_err(|_| FsError::IoError)
    }

    pub fn write_block(
        &mut self, lba: u32, buf: &[u8], sectors: u8,
    ) -> Result<(), FsError> {
        self.io_count += 1;
        crate::block::write(self.dev_id, (self.start_lba + lba) as u64, sectors as u32, buf)
            .map_err(|_| FsError::IoError)
    }

    pub fn flush_drive(&mut self) {
        self.io_count += 1;
        let _ = crate::block::flush(self.dev_id);
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
