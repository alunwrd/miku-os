// RAM-backed block device
//
// Backs a block device with a physically-contiguous span of RAM, addressed
// through the HHDM. In practice a GRUB multiboot2 module: the bootloader loads
// the firmware image into memory, and this exposes it as a mountable ext block
// device, so one boot medium (the ISO/USB) carries both the kernel and
// /lib/firmware (see fwload::init).
//
// Writes land in RAM and are not persisted: ext metadata the filesystem dirties
// lives for the session

use crate::io::block::driver::{BlkError, BlockDevInfo, BlockDriver};

const SECTOR: u64 = 512;

pub struct RamBlockDev {
    /// HHDM virtual base of the backing RAM
    virt_base: u64,
    /// total length in bytes (sector-aligned)
    len: u64,
}

impl RamBlockDev {
    /// 'phys_base .. phys_base + len' must lie in RAM the HHDM maps. A GRUB
    /// module sits in reclaimable boot memory, which the HHDM covers
    pub fn new(phys_base: u64, len: u64) -> Self {
        Self {
            virt_base: crate::arch::x86_64::grub::phys_to_virt(phys_base),
            len: len & !(SECTOR - 1),
        }
    }

    /// The same device over memory already mapped, a heap allocation say. The
    /// driver uses only the virtual address, and this skips the HHDM
    /// translation
    pub fn from_virt(virt_base: u64, len: u64) -> Self {
        Self {
            virt_base,
            len: len & !(SECTOR - 1),
        }
    }

    #[inline]
    fn range_ok(&self, lba: u64, count: u32) -> Option<(u64, usize)> {
        let off = lba.checked_mul(SECTOR)?;
        let nbytes = (count as u64).checked_mul(SECTOR)?;
        if off.checked_add(nbytes)? > self.len {
            return None;
        }
        Some((off, nbytes as usize))
    }
}

impl BlockDriver for RamBlockDev {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        let (off, nbytes) = self.range_ok(lba, count).ok_or(BlkError::DeviceFault)?;
        if buf.len() < nbytes {
            return Err(BlkError::BufferTooSmall);
        }
        let src = (self.virt_base + off) as *const u8;
        unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), nbytes); }
        Ok(())
    }

    fn write_blocks(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        let (off, nbytes) = self.range_ok(lba, count).ok_or(BlkError::DeviceFault)?;
        if buf.len() < nbytes {
            return Err(BlkError::BufferTooSmall);
        }
        let dst = (self.virt_base + off) as *mut u8;
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, nbytes); }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlkError> {
        Ok(())
    }

    fn info(&self) -> BlockDevInfo {
        let mut info = BlockDevInfo::unknown();
        info.sector_size = SECTOR as u32;
        info.total_sectors = self.len / SECTOR;
        let name = b"ramdisk(fw)";
        info.model[..name.len()].copy_from_slice(name);
        info.model_len = name.len() as u8;
        info
    }
}
