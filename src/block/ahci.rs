// AHCI (SATA) driver behind the 'BlockDriver' trait
//
// Modern chipsets expose SATA disks only through an AHCI HBA (PCI class
// 01.06) - the legacy IDE ports the ATA driver uses simply don't exist
// there, so this driver is what makes MikuOS's storage stack work on real  hardware
//
// Scope mirrors the other drivers: one command slot, one PRD entry into a
// 64 KiB bounce buffer, polled completion (PxCI). The HBA registers are
// MMIO behind BAR5, mapped uncached through the HHDM window

extern crate alloc;

use alloc::boxed::Box;
use spin::Mutex as SpinMutex;

use super::driver::{BlkError, BlockDevInfo, BlockDriver};
use crate::net::pci::{pci_read32, pci_read8, PCI_ADDR, PCI_DATA, pci_addr};

// HBA global registers (offsets from ABAR)
const HBA_CAP: u64 = 0x00;
const HBA_GHC: u64 = 0x04;
const HBA_PI:  u64 = 0x0C;

const GHC_AE: u32 = 1 << 31;

// Per-port registers (offsets from ABAR + 0x100 + port * 0x80)
const PX_CLB:  u64 = 0x00;
const PX_CLBU: u64 = 0x04;
const PX_FB:   u64 = 0x08;
const PX_FBU:  u64 = 0x0C;
const PX_IS:   u64 = 0x10;
const PX_CMD:  u64 = 0x18;
const PX_TFD:  u64 = 0x20;
const PX_SIG:  u64 = 0x24;
const PX_SSTS: u64 = 0x28;
const PX_SERR: u64 = 0x30;
const PX_CI:   u64 = 0x38;

const CMD_ST:  u32 = 1 << 0;
const CMD_SUD: u32 = 1 << 1;
const CMD_POD: u32 = 1 << 2;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR:  u32 = 1 << 14;
const CMD_CR:  u32 = 1 << 15;

const TFD_ERR: u32 = 1 << 0;
const TFD_DRQ: u32 = 1 << 3;
const TFD_BSY: u32 = 1 << 7;

// Fatal error bits in PxIS
const IS_ERR_MASK: u32 = (1 << 30) | (1 << 29) | (1 << 28) | (1 << 27) | (1 << 26);

const SIG_SATA_DISK: u32 = 0x0000_0101;

const FIS_TYPE_H2D: u8 = 0x27;

const ATA_READ_DMA_EXT:     u8 = 0x25;
const ATA_WRITE_DMA_EXT:     u8 = 0x35;
const ATA_WRITE_DMA_FUA_EXT: u8 = 0x3D;
const ATA_FLUSH_CACHE_EXT:  u8 = 0xEA;
const ATA_IDENTIFY:         u8 = 0xEC;
const ATA_DSM:             u8 = 0x06; // DATA SET MANAGEMENT (TRIM)

/// DSM FEATURES bit 0: the range payload is a TRIM request
const DSM_FEATURE_TRIM: u16 = 0x0001;

pub const MAX_XFER_SECTORS: u32 = 128;
const BOUNCE_SIZE: usize = MAX_XFER_SECTORS as usize * 512;

/// Per-port DMA structures in one physically-contiguous allocation.
/// Offsets satisfy the AHCI alignment rules: command list 1 KiB, received
/// FIS 256 B, command table 128 B
#[repr(C, align(1024))]
struct PortMem {
    cmd_list: [u8; 1024],       // 32 headers; only slot 0 is used
    fis:      [u8; 256],        // received FIS area
    cmd_tbl:  [u8; 256],        // CFIS(64) + ACMD(16) + rsv(48) + PRDT(16)
    bounce:   [u8; BOUNCE_SIZE],
}

pub struct AhciPort {
    port_mmio: u64,             // virtual base of this port's register block
    mem:       Box<PortMem>,
    capacity:  u64,
    model:     [u8; 40],
    model_len: u8,
    /// Drive supports DATA SET MANAGEMENT / TRIM (IDENTIFY word 169 bit 0)
    has_trim:  bool,
}

#[inline]
fn rd32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
fn wr32(addr: u64, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

fn wait_clear(addr: u64, mask: u32, max_spins: u64) -> bool {
    let mut spins = 0u64;
    while rd32(addr) & mask != 0 {
        super::io_relax(spins);
        spins += 1;
        if spins > max_spins {
            return false;
        }
    }
    true
}

/// Find the first AHCI HBA on the PCI bus and bring up every implemented
/// port with a SATA disk behind it. Returns initialized ports via 'out'
pub fn find_ports(out: &mut [Option<AhciPort>; 4]) -> usize {
    let mut abar_phys: Option<u64> = None;

    'scan: for bus in 0..=255u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = pci_read32(bus, dev, func, 0x00);
                if (id & 0xFFFF) as u16 == 0xFFFF {
                    if func == 0 { break; }
                    continue;
                }
                let class_rev = pci_read32(bus, dev, func, 0x08);
                if (class_rev >> 24) as u8 == 0x01 && ((class_rev >> 16) & 0xFF) as u8 == 0x06 {
                    let bar5 = pci_read32(bus, dev, func, 0x10 + 5 * 4);
                    if bar5 & 1 == 0 && bar5 != 0 {
                        // Memory space + bus master on
                        let cmd = pci_read32(bus, dev, func, 0x04);
                        unsafe {
                            use x86_64::instructions::port::Port;
                            Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, 0x04));
                            Port::<u32>::new(PCI_DATA).write(cmd | 0x0006);
                        }
                        abar_phys = Some((bar5 & 0xFFFF_FFF0) as u64);
                        crate::serial_println!(
                            "[ahci] HBA {:02x}:{:02x}.{} abar=0x{:08X}",
                            bus, dev, func, abar_phys.unwrap()
                        );
                        break 'scan;
                    }
                }
                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 {
                    break;
                }
            }
        }
    }

    let Some(abar) = abar_phys else { return 0 };

    // HBA register block: 0x100 of globals + 32 ports x 0x80
    crate::vmm::map_mmio_uc(abar, 0x1100);
    let hba = crate::grub::phys_to_virt(abar);

    // AHCI mode on
    wr32(hba + HBA_GHC, rd32(hba + HBA_GHC) | GHC_AE);

    let pi  = rd32(hba + HBA_PI);
    let cap = rd32(hba + HBA_CAP);
    crate::serial_println!("[ahci] CAP=0x{:08X} PI=0x{:08X}", cap, pi);

    let mut count = 0usize;
    for port in 0..32u32 {
        if pi & (1 << port) == 0 || count >= out.len() {
            continue;
        }
        let pm = hba + 0x100 + port as u64 * 0x80;

        let ssts = rd32(pm + PX_SSTS);
        let det = ssts & 0xF;
        let ipm = (ssts >> 8) & 0xF;
        if det != 3 || ipm != 1 {
            continue; // no active device on this port
        }
        if rd32(pm + PX_SIG) != SIG_SATA_DISK {
            continue; // ATAPI / port multiplier - out of scope
        }

        match AhciPort::init(pm, port) {
            Some(p) => {
                out[count] = Some(p);
                count += 1;
            }
            None => {
                crate::serial_println!("[ahci] port {} init failed", port);
            }
        }
    }
    count
}

impl AhciPort {
    fn init(pm: u64, port_no: u32) -> Option<Self> {
        // Stop command engine and FIS receive before touching the pointers
        wr32(pm + PX_CMD, rd32(pm + PX_CMD) & !CMD_ST);
        if !wait_clear(pm + PX_CMD, CMD_CR, 5_000_000) { return None; }
        wr32(pm + PX_CMD, rd32(pm + PX_CMD) & !CMD_FRE);
        if !wait_clear(pm + PX_CMD, CMD_FR, 5_000_000) { return None; }

        let mem = Box::new(PortMem {
            cmd_list: [0u8; 1024],
            fis:      [0u8; 256],
            cmd_tbl:  [0u8; 256],
            bounce:   [0u8; BOUNCE_SIZE],
        });

        let clb_phys = crate::net::virt_to_phys(mem.cmd_list.as_ptr() as u64);
        let fb_phys  = crate::net::virt_to_phys(mem.fis.as_ptr() as u64);
        if clb_phys > u32::MAX as u64 || fb_phys > u32::MAX as u64 {
            crate::serial_println!("[ahci] port {}: DMA memory above 4 GiB", port_no);
            return None;
        }

        wr32(pm + PX_CLB,  clb_phys as u32);
        wr32(pm + PX_CLBU, 0);
        wr32(pm + PX_FB,   fb_phys as u32);
        wr32(pm + PX_FBU,  0);

        wr32(pm + PX_SERR, 0xFFFF_FFFF);
        wr32(pm + PX_IS,   0xFFFF_FFFF);

        // FIS receive on, then start the command engine (with power-on /
        // spin-up for HBAs that implement staggered spin-up)
        wr32(pm + PX_CMD, rd32(pm + PX_CMD) | CMD_FRE);
        wr32(pm + PX_CMD, rd32(pm + PX_CMD) | CMD_POD | CMD_SUD | CMD_ST);

        let mut p = AhciPort {
            port_mmio: pm,
            mem,
            capacity:  0,
            model:     [0u8; 40],
            model_len: 0,
            has_trim:  false,
        };

        // IDENTIFY DEVICE fills capacity + model
        if p.issue(ATA_IDENTIFY, 0, 0, 512, false, 0).is_err() {
            crate::serial_println!("[ahci] port {}: IDENTIFY failed", port_no);
            return None;
        }
        let w = |i: usize| -> u64 {
            u16::from_le_bytes([p.mem.bounce[i * 2], p.mem.bounce[i * 2 + 1]]) as u64
        };
        p.capacity = w(100) | (w(101) << 16) | (w(102) << 32) | (w(103) << 48);
        if p.capacity == 0 {
            p.capacity = w(60) | (w(61) << 16); // LBA28 fallback
        }
        let mut n = 0usize;
        for i in 27..=46 {
            let word = w(i) as u16;
            for b in [(word >> 8) as u8, (word & 0xFF) as u8] {
                if n < p.model.len() { p.model[n] = b; n += 1; }
            }
        }
        while n > 0 && (p.model[n - 1] == b' ' || p.model[n - 1] == 0) { n -= 1; }
        p.model_len = n as u8;
        p.has_trim = w(169) & 1 != 0;

        crate::serial_println!(
            "[ahci] port {}: '{}' {} sectors ({} MB) trim={}",
            port_no,
            core::str::from_utf8(&p.model[..n]).unwrap_or("?"),
            p.capacity,
            p.capacity * 512 / (1024 * 1024),
            p.has_trim
        );
        Some(p)
    }

    /// Build and issue one command in slot 0, polling PxCI to completion
    fn issue(&mut self, cmd: u8, lba: u64, count: u16, bytes: usize, write: bool, features: u16)
        -> Result<(), BlkError>
    {
        let pm = self.port_mmio;

        // Wait for the device to be idle (BSY/DRQ clear in the task file)
        let mut spins = 0u64;
        while rd32(pm + PX_TFD) & (TFD_BSY | TFD_DRQ) != 0 {
            super::io_relax(spins);
            spins += 1;
            if spins > 50_000_000 { return Err(BlkError::Timeout); }
        }

        // Command FIS (host -> device)
        let cfis = &mut self.mem.cmd_tbl[..64];
        cfis.fill(0);
        cfis[0] = FIS_TYPE_H2D;
        cfis[1] = 0x80; // C: command register update
        cfis[2] = cmd;
        cfis[3] = features as u8;
        cfis[11] = (features >> 8) as u8;
        cfis[4] = lba as u8;
        cfis[5] = (lba >> 8) as u8;
        cfis[6] = (lba >> 16) as u8;
        cfis[7] = 0x40; // LBA mode
        cfis[8] = (lba >> 24) as u8;
        cfis[9] = (lba >> 32) as u8;
        cfis[10] = (lba >> 40) as u8;
        cfis[12] = (count & 0xFF) as u8;
        cfis[13] = (count >> 8) as u8;

        // One PRD entry at command-table offset 0x80
        let with_data = bytes > 0;
        if with_data {
            let dba = crate::net::virt_to_phys(self.mem.bounce.as_ptr() as u64);
            if dba + bytes as u64 > u32::MAX as u64 {
                return Err(BlkError::DeviceFault);
            }
            let prd = &mut self.mem.cmd_tbl[0x80..0x90];
            prd[0..4].copy_from_slice(&(dba as u32).to_le_bytes());
            prd[4..8].copy_from_slice(&0u32.to_le_bytes());
            prd[8..12].copy_from_slice(&0u32.to_le_bytes());
            prd[12..16].copy_from_slice(&((bytes as u32 - 1) & 0x3F_FFFF).to_le_bytes());
        }

        // Command header, slot 0: CFIS length 5 dwords, write flag, PRDTL
        let ctba = crate::net::virt_to_phys(self.mem.cmd_tbl.as_ptr() as u64);
        let prdtl: u32 = if with_data { 1 } else { 0 };
        let dw0: u32 = 5 | ((write as u32) << 6) | (prdtl << 16);
        let hdr = &mut self.mem.cmd_list[..32];
        hdr.fill(0);
        hdr[0..4].copy_from_slice(&dw0.to_le_bytes());
        hdr[8..12].copy_from_slice(&(ctba as u32).to_le_bytes());

        wr32(pm + PX_IS, 0xFFFF_FFFF);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        wr32(pm + PX_CI, 1);

        let mut spins = 0u64;
        loop {
            let is = rd32(pm + PX_IS);
            if is & IS_ERR_MASK != 0 {
                crate::serial_println!("[ahci] error IS=0x{:08X} TFD=0x{:08X}",
                    is, rd32(pm + PX_TFD));
                wr32(pm + PX_SERR, 0xFFFF_FFFF);
                return Err(BlkError::DeviceFault);
            }
            if rd32(pm + PX_CI) & 1 == 0 {
                break;
            }
            super::io_relax(spins);
            spins += 1;
            if spins > 200_000_000 {
                return Err(BlkError::Timeout);
            }
        }

        if rd32(pm + PX_TFD) & TFD_ERR != 0 {
            return Err(BlkError::DeviceFault);
        }
        Ok(())
    }

    /// Shared write loop; 'fua' selects WRITE DMA FUA EXT so the data is on
    /// stable media when the command completes
    fn write_loop(&mut self, lba: u64, count: u32, buf: &[u8], fua: bool) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let cmd = if fua { ATA_WRITE_DMA_FUA_EXT } else { ATA_WRITE_DMA_EXT };
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            let off = done as usize * 512;
            self.mem.bounce[..bytes].copy_from_slice(&buf[off..off + bytes]);
            self.issue(cmd, lba + done as u64, chunk as u16, bytes, true, 0)?;
            done += chunk;
        }
        Ok(())
    }
}

/// Block-layer wrapper: one AHCI port has a single command slot and bounce
/// buffer, so concurrent '&self' dispatch serializes on this internal mutex
/// (no parallelism within one port, same as before - the win is that the
/// block layer no longer needs a device-wide lock around every backend)
pub struct AhciBlockDev(SpinMutex<AhciPort>);

impl AhciBlockDev {
    pub fn new(port: AhciPort) -> Self {
        Self(SpinMutex::new(port))
    }
}

impl BlockDriver for AhciBlockDev {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        self.0.lock().bd_read_blocks(lba, count, buf)
    }
    fn write_blocks(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        self.0.lock().write_loop(lba, count, buf, false)
    }
    fn write_blocks_fua(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        self.0.lock().write_loop(lba, count, buf, true)
    }
    fn flush(&self) -> Result<(), BlkError> {
        self.0.lock().issue(ATA_FLUSH_CACHE_EXT, 0, 0, 0, false, 0)
    }
    fn discard(&self, lba: u64, count: u32) -> Result<(), BlkError> {
        self.0.lock().bd_discard(lba, count)
    }
    fn info(&self) -> BlockDevInfo {
        self.0.lock().bd_info()
    }
}

impl AhciPort {
    fn bd_read_blocks(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            self.issue(ATA_READ_DMA_EXT, lba + done as u64, chunk as u16, bytes, false, 0)?;
            let off = done as usize * 512;
            buf[off..off + bytes].copy_from_slice(&self.mem.bounce[..bytes]);
            done += chunk;
        }
        Ok(())
    }

    /// DATA SET MANAGEMENT / TRIM. The payload is 512-byte blocks of 8-byte
    /// range entries - 48-bit LBA in the low bits, a 16-bit sector count in
    /// the top - so one block describes up to 64 ranges; long discards loop
    fn bd_discard(&mut self, lba: u64, count: u32) -> Result<(), BlkError> {
        if !self.has_trim {
            return Err(BlkError::Unsupported);
        }
        let mut done = 0u32;
        while done < count {
            self.mem.bounce[..512].fill(0);
            let mut used = 0usize;
            while used < 64 && done < count {
                let n = (count - done).min(0xFFFF);
                let entry = ((lba + done as u64) & 0xFFFF_FFFF_FFFF) | ((n as u64) << 48);
                self.mem.bounce[used * 8..used * 8 + 8].copy_from_slice(&entry.to_le_bytes());
                used += 1;
                done += n;
            }
            // LBA is reserved for DSM; count = number of 512-byte range blocks
            self.issue(ATA_DSM, 0, 1, 512, true, DSM_FEATURE_TRIM)?;
        }
        Ok(())
    }

    fn bd_info(&mut self) -> BlockDevInfo {
        let mut out = BlockDevInfo::unknown();
        out.total_sectors = self.capacity;
        out.lba48 = true;
        out.model = self.model;
        out.model_len = self.model_len;
        out.discard = self.has_trim;
        out
    }
}
