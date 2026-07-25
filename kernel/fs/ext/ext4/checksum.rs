use super::crc32c;
use crate::fs::ext::structs::*;
use crate::fs::ext::{FsError, MikuFS};

impl MikuFS {
    pub fn verify_superblock_csum(&self) -> bool {
        if !self.superblock.has_metadata_csum() {
            return true;
        }
        let uuid = self.superblock.uuid();
        let mut sb_copy = [0u8; 1024];
        sb_copy.copy_from_slice(&self.superblock.data);
        sb_copy[0xFE] = 0;
        sb_copy[0xFF] = 0;
        sb_copy[0x100] = 0;
        sb_copy[0x101] = 0;
        let computed = crc32c::ext4_superblock_csum(uuid, &sb_copy);
        let stored = self.superblock.read_u32(0xFE);
        computed == stored
    }

    pub fn compute_superblock_csum(&self) -> u32 {
        let uuid = self.superblock.uuid();
        let mut sb_copy = [0u8; 1024];
        sb_copy.copy_from_slice(&self.superblock.data);
        sb_copy[0xFE] = 0;
        sb_copy[0xFF] = 0;
        sb_copy[0x100] = 0;
        sb_copy[0x101] = 0;
        crc32c::ext4_superblock_csum(uuid, &sb_copy)
    }

    pub fn update_superblock_csum(&mut self) {
        if !self.superblock.has_metadata_csum() {
            return;
        }
        let csum = self.compute_superblock_csum();
        self.superblock.write_u32(0xFE, csum);
    }

    pub fn verify_group_desc_csum(&self, group: usize) -> bool {
        if group >= self.groups.len() {
            return false;
        }
        if !self.superblock.has_metadata_csum() && !self.superblock.has_gdt_csum() {
            return true;
        }
        let uuid = self.superblock.uuid();
        let gd_size = self.superblock.group_desc_size() as usize;
        let mut gd_copy = [0u8; 64];
        gd_copy[..gd_size].copy_from_slice(&self.groups[group].data[..gd_size]);
        gd_copy[30] = 0;
        gd_copy[31] = 0;
        let computed = crc32c::ext4_group_desc_csum(uuid, group as u32, &gd_copy[..gd_size]);
        let stored = self.groups[group].checksum();
        computed == stored
    }

    pub fn update_group_desc_csum(&mut self, group: usize) {
        if group >= self.groups.len() {
            return;
        }
        if !self.superblock.has_metadata_csum() && !self.superblock.has_gdt_csum() {
            return;
        }
        let uuid_copy = {
            let mut u = [0u8; 16];
            u.copy_from_slice(self.superblock.uuid());
            u
        };
        let gd_size = self.superblock.group_desc_size() as usize;
        let mut gd_copy = [0u8; 64];
        gd_copy[..gd_size].copy_from_slice(&self.groups[group].data[..gd_size]);
        gd_copy[30] = 0;
        gd_copy[31] = 0;
        let computed = crc32c::ext4_group_desc_csum(&uuid_copy, group as u32, &gd_copy[..gd_size]);
        self.groups[group].write_u16(30, computed);
    }

    pub fn verify_inode_csum(&self, inode_num: u32, inode: &Inode) -> bool {
        if !self.superblock.has_metadata_csum() {
            return true;
        }
        let uuid = self.superblock.uuid();
        let gen = inode.generation();
        let size = inode.on_disk_size as usize;
        let mut data = [0u8; 256];
        data[..size].copy_from_slice(&inode.data[..size]);
        let lo_off = 124;
        data[lo_off] = 0;
        data[lo_off + 1] = 0;
        if size >= 132 {
            data[130] = 0;
            data[131] = 0;
        }
        let computed = crc32c::ext4_inode_csum(uuid, inode_num, gen, &data[..size]);
        let stored_lo = inode.checksum_lo();
        (computed & 0xFFFF) as u16 == stored_lo
    }

    pub fn flush_superblock_with_csum(&mut self) -> Result<(), FsError> {
        self.update_superblock_csum();
        self.flush_superblock()
    }

    pub fn flush_group_desc_with_csum(&mut self, group: usize) -> Result<(), FsError> {
        self.update_group_desc_csum(group);
        self.flush_group_desc(group)
    }

    pub fn has_gdt_csum(&self) -> bool {
        self.superblock.has_metadata_csum() || self.superblock.has_gdt_csum()
    }

    pub fn compute_inode_csum_value(&self, inode_num: u32, inode: &Inode) -> u32 {
        let uuid = self.superblock.uuid();
        let gen = inode.generation();
        let size = inode.on_disk_size as usize;
        let mut data = [0u8; 256];
        data[..size].copy_from_slice(&inode.data[..size]);
        data[124] = 0;
        data[125] = 0;
        if size >= 132 {
            data[130] = 0;
            data[131] = 0;
        }
        crc32c::ext4_inode_csum(uuid, inode_num, gen, &data[..size])
    }

    pub fn stamp_inode_csum(&self, inode_num: u32, inode: &mut Inode) {
        if !self.superblock.has_metadata_csum() {
            return;
        }
        let csum = self.compute_inode_csum_value(inode_num, inode);
        inode.write_u16(124, (csum & 0xFFFF) as u16);
        if inode.on_disk_size >= 132 {
            inode.write_u16(130, ((csum >> 16) & 0xFFFF) as u16);
        }
    }

    fn read_bitmap_block(&mut self, bitmap_block: u32) -> Result<[u8; 4096], FsError> {
        let mut buf = [0u8; 4096];
        let bs = self.block_size as usize;
        self.read_block_into(bitmap_block, &mut buf[..bs])?;
        Ok(buf)
    }

    pub fn compute_block_bitmap_csum(&mut self, group: usize) -> Result<u32, FsError> {
        if group >= self.groups.len() {
            return Ok(0);
        }
        let bitmap_block = self.groups[group].block_bitmap();
        let buf = self.read_bitmap_block(bitmap_block)?;
        let bytes = ((self.blocks_per_group + 7) / 8) as usize;
        let uuid_copy = {
            let mut u = [0u8; 16];
            u.copy_from_slice(self.superblock.uuid());
            u
        };
        Ok(crc32c::ext4_bitmap_csum(&uuid_copy, &buf[..bytes]))
    }

    pub fn compute_inode_bitmap_csum(&mut self, group: usize) -> Result<u32, FsError> {
        if group >= self.groups.len() {
            return Ok(0);
        }
        let bitmap_block = self.groups[group].inode_bitmap();
        let buf = self.read_bitmap_block(bitmap_block)?;
        let bytes = ((self.inodes_per_group + 7) / 8) as usize;
        let uuid_copy = {
            let mut u = [0u8; 16];
            u.copy_from_slice(self.superblock.uuid());
            u
        };
        Ok(crc32c::ext4_bitmap_csum(&uuid_copy, &buf[..bytes]))
    }

    pub fn update_block_bitmap_csum(&mut self, group: usize) -> Result<(), FsError> {
        if !self.superblock.has_metadata_csum() {
            return Ok(());
        }
        if group >= self.groups.len() {
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
        if !self.superblock.has_metadata_csum() {
            return Ok(());
        }
        if group >= self.groups.len() {
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
        let gd_size = self.superblock.group_desc_size() as usize;
        if gd_size >= 64 {
            let stored_hi = self.groups[group].block_bitmap_csum_hi();
            let stored = (stored_lo as u32) | ((stored_hi as u32) << 16);
            computed == stored
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
        let gd_size = self.superblock.group_desc_size() as usize;
        if gd_size >= 64 {
            let stored_hi = self.groups[group].inode_bitmap_csum_hi();
            let stored = (stored_lo as u32) | ((stored_hi as u32) << 16);
            computed == stored
        } else {
            (computed & 0xFFFF) as u16 == stored_lo
        }
    }
}
