// NVMe driver behind the 'BlockDriver' trait
// 
// Completes the real-hardware storage trio (legacy ATA, AHCI SATA, NVMe):
// most modern machines boot from an NVMe SSD where neither of the other two
// controllers exists.
// 
// Scope matches the rest of the block layer: one admin queue pair plus one
// I/O queue pair, single outstanding command, polled completion via the
// CQ phase bit, data through a 64 KiB bounce buffer addressed with  PRP1 + a PRP list page

extern crate alloc;

use alloc::boxed::Box;

use super::driver::{BlkError, BlockDevInfo, BlockDriver, HealthInfo};
use crate::net::pci::{pci_read32, pci_read8, PCI_ADDR, PCI_DATA, pci_addr};

// Controller registers (offsets from BAR0)
const REG_CAP:  u64 = 0x00; // 8 bytes
const REG_CC:   u64 = 0x14;
const REG_CSTS: u64 = 0x1C;
const REG_AQA:  u64 = 0x24;
const REG_ASQ:  u64 = 0x28; // 8 bytes
const REG_ACQ:  u64 = 0x30; // 8 bytes
const DOORBELLS: u64 = 0x1000;

const CC_ENABLE: u32 = 1;
// IOSQES=6 (64 B submission entries), IOCQES=4 (16 B completion entries),
// MPS=0 (4 KiB pages), CSS=0 (NVM command set)
const CC_CONFIG: u32 = (6 << 16) | (4 << 20) | CC_ENABLE;

const CSTS_RDY: u32 = 1;

// Admin opcodes
const ADM_CREATE_IO_SQ: u8 = 0x01;
const ADM_CREATE_IO_CQ: u8 = 0x05;
const ADM_IDENTIFY:     u8 = 0x06;
const ADM_GET_LOG_PAGE: u8 = 0x02;

// NVM I/O opcodes
const NVM_WRITE: u8 = 0x01;
const NVM_READ:  u8 = 0x02;
const NVM_FLUSH: u8 = 0x00;
const NVM_DSM:   u8 = 0x09; // Dataset Management (deallocate = discard)
const NVM_WRITE_ZEROES: u8 = 0x08;

const ADMIN_QD: usize = 16; // admin queue depth
const IO_QD:    usize = 64; // I/O queue depth

pub const MAX_XFER_SECTORS: u32 = 128; // 64 KiB per command
const BOUNCE_SIZE: usize = MAX_XFER_SECTORS as usize * 512;

/// All controller-visible memory in one physically-contiguous, page-aligned
/// allocation. Queue bases must be page aligned, hence the padded fields
#[repr(C, align(4096))]
struct NvmeMem {
    admin_sq: [u8; 4096],        // 16 x 64 B used
    admin_cq: [u8; 4096],        // 16 x 16 B used
    io_sq:    [u8; 4096],        // 64 x 64 B
    io_cq:    [u8; 4096],        // 64 x 16 B
    prp_list: [u8; 4096],        // PRP entries for bounce pages 1..15
    ident:    [u8; 4096],        // IDENTIFY result buffer
    bounce:   [u8; BOUNCE_SIZE],
}

struct Queue {
    sq_tail: u16,
    cq_head: u16,
    phase:   u8,
    depth:   u16,
}

impl Queue {
    const fn new(depth: u16) -> Self {
        Self { sq_tail: 0, cq_head: 0, phase: 1, depth }
    }
}

pub struct Nvme {
    mmio:      u64,   // virtual base of BAR0
    db_stride: u64,
    mem:       Box<NvmeMem>,
    admin:     Queue,
    io:        Queue,
    cid:       u16,
    nsid:      u32,
    capacity:  u64,   // in 512-byte sectors
    model:     [u8; 40],
    model_len: u8,
    /// Controller supports Dataset Management (ONCS bit 2) - the discard path
    has_dsm:   bool,
    /// Controller supports Write Zeroes (ONCS bit 3)
    has_wz:    bool,
}

#[inline]
fn rd32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
fn rd64(addr: u64) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

#[inline]
fn wr32(addr: u64, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

#[inline]
fn wr64(addr: u64, val: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, val) }
}

/// Find the first NVMe controller (PCI class 01.08) and bring it up
pub fn find_controller() -> Option<Nvme> {
    let mut found: Option<(u8, u8, u8, u64)> = None;

    'scan: for bus in 0..=255u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = pci_read32(bus, dev, func, 0x00);
                if (id & 0xFFFF) as u16 == 0xFFFF {
                    if func == 0 { break; }
                    continue;
                }
                let class_rev = pci_read32(bus, dev, func, 0x08);
                if (class_rev >> 24) as u8 == 0x01
                    && ((class_rev >> 16) & 0xFF) as u8 == 0x08
                {
                    let bar_lo = pci_read32(bus, dev, func, 0x10);
                    if bar_lo & 1 != 0 { continue; } // must be MMIO
                    let mut bar = (bar_lo & 0xFFFF_FFF0) as u64;
                    if (bar_lo >> 1) & 3 == 2 {
                        bar |= (pci_read32(bus, dev, func, 0x14) as u64) << 32;
                    }
                    if bar == 0 { continue; }
                    found = Some((bus, dev, func, bar));
                    break 'scan;
                }
                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 {
                    break;
                }
            }
        }
    }

    let (bus, dev, func, bar) = found?;

    // Memory space + bus master on
    let cmd = pci_read32(bus, dev, func, 0x04);
    unsafe {
        use x86_64::instructions::port::Port;
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, 0x04));
        Port::<u32>::new(PCI_DATA).write(cmd | 0x0006);
    }
    crate::serial_println!("[nvme] controller {:02x}:{:02x}.{} bar0=0x{:X}", bus, dev, func, bar);

    // Registers + doorbells (stride is per CAP.DSTRD; map enough for both)
    crate::vmm::map_mmio_uc(bar, 0x4000);
    let mmio = crate::grub::phys_to_virt(bar);

    Nvme::init(mmio)
}

impl Nvme {
    fn init(mmio: u64) -> Option<Self> {
        let cap = rd64(mmio + REG_CAP);
        let db_stride = 4u64 << ((cap >> 32) & 0xF);
        let mqes = (cap & 0xFFFF) as usize + 1;
        let timeout_500ms = ((cap >> 24) & 0xFF) as u64;

        let mut drv = Nvme {
            mmio,
            db_stride,
            mem: Box::new(NvmeMem {
                admin_sq: [0u8; 4096],
                admin_cq: [0u8; 4096],
                io_sq:    [0u8; 4096],
                io_cq:    [0u8; 4096],
                prp_list: [0u8; 4096],
                ident:    [0u8; 4096],
                bounce:   [0u8; BOUNCE_SIZE],
            }),
            admin: Queue::new(ADMIN_QD.min(mqes) as u16),
            io:    Queue::new(IO_QD.min(mqes) as u16),
            cid:   0,
            nsid:  1,
            capacity:  0,
            model:     [0u8; 40],
            model_len: 0,
            has_dsm:   false,
            has_wz:    false,
        };

        // Reset: clear EN, wait for RDY to drop
        wr32(mmio + REG_CC, rd32(mmio + REG_CC) & !CC_ENABLE);
        if !drv.wait_csts(0, timeout_500ms) {
            crate::serial_println!("[nvme] controller stuck busy on reset");
            return None;
        }

        let asq = crate::net::virt_to_phys(drv.mem.admin_sq.as_ptr() as u64);
        let acq = crate::net::virt_to_phys(drv.mem.admin_cq.as_ptr() as u64);
        let aqa = ((drv.admin.depth as u32 - 1) << 16) | (drv.admin.depth as u32 - 1);
        wr32(mmio + REG_AQA, aqa);
        wr64(mmio + REG_ASQ, asq);
        wr64(mmio + REG_ACQ, acq);

        // Program the config first, then flip EN in a second write - some
        // controllers reject config changes in the same write that enables
        wr32(mmio + REG_CC, CC_CONFIG & !CC_ENABLE);
        wr32(mmio + REG_CC, CC_CONFIG);
        if !drv.wait_csts(CSTS_RDY, timeout_500ms) {
            crate::serial_println!("[nvme] controller did not become ready");
            return None;
        }

        // IDENTIFY controller (CNS=1): model string for blkstat
        let ident_phys = crate::net::virt_to_phys(drv.mem.ident.as_ptr() as u64);
        if drv.admin_cmd(ADM_IDENTIFY, 0, ident_phys, 1, 0).is_err() {
            crate::serial_println!("[nvme] IDENTIFY controller failed");
            return None;
        }
        let mut n = 0usize;
        for &b in &drv.mem.ident[24..64] {
            if n < drv.model.len() { drv.model[n] = b; n += 1; }
        }
        while n > 0 && (drv.model[n - 1] == b' ' || drv.model[n - 1] == 0) { n -= 1; }
        drv.model_len = n as u8;

        // Optional NVM command support (ONCS, bytes 520-521):
        // bit 2 = DSM (discard), bit 3 = Write Zeroes
        let oncs = u16::from_le_bytes([drv.mem.ident[520], drv.mem.ident[521]]);
        drv.has_dsm = oncs & (1 << 2) != 0;
        drv.has_wz  = oncs & (1 << 3) != 0;

        // IDENTIFY namespace 1 (CNS=0): size + LBA format
        if drv.admin_cmd(ADM_IDENTIFY, drv.nsid, ident_phys, 0, 0).is_err() {
            crate::serial_println!("[nvme] IDENTIFY namespace failed");
            return None;
        }
        let nsze = u64::from_le_bytes(drv.mem.ident[0..8].try_into().ok()?);
        let flbas = drv.mem.ident[26] & 0x0F;
        let lbaf_off = 128 + flbas as usize * 4;
        let lbads = drv.mem.ident[lbaf_off + 2];
        if lbads != 9 {
            crate::serial_println!(
                "[nvme] namespace LBA size 2^{} unsupported (need 512 B)", lbads
            );
            return None;
        }
        drv.capacity = nsze;

        // I/O completion queue first, then the submission queue tied to it
        let io_cq = crate::net::virt_to_phys(drv.mem.io_cq.as_ptr() as u64);
        let io_sq = crate::net::virt_to_phys(drv.mem.io_sq.as_ptr() as u64);
        let qsize = (drv.io.depth as u32 - 1) << 16;
        if drv.admin_cmd(ADM_CREATE_IO_CQ, 0, io_cq, qsize | 1, 1).is_err() {
            crate::serial_println!("[nvme] create IO CQ failed");
            return None;
        }
        if drv.admin_cmd(ADM_CREATE_IO_SQ, 0, io_sq, qsize | 1, (1 << 16) | 1).is_err() {
            crate::serial_println!("[nvme] create IO SQ failed");
            return None;
        }

        // PRP list for bounce pages 1.. (page 0 goes in PRP1); the bounce is
        // physically contiguous, so the list never changes
        let bounce_phys = crate::net::virt_to_phys(drv.mem.bounce.as_ptr() as u64);
        for i in 1..(BOUNCE_SIZE / 4096) {
            let entry = bounce_phys + i as u64 * 4096;
            let off = (i - 1) * 8;
            drv.mem.prp_list[off..off + 8].copy_from_slice(&entry.to_le_bytes());
        }

        crate::serial_println!(
            "[nvme] '{}' ns{} {} sectors ({} MB) qd={} dsm={} wz={}",
            core::str::from_utf8(&drv.model[..drv.model_len as usize]).unwrap_or("?"),
            drv.nsid, drv.capacity, drv.capacity * 512 / (1024 * 1024), drv.io.depth,
            drv.has_dsm, drv.has_wz
        );
        Some(drv)
    }

    fn wait_csts(&self, want: u32, timeout_500ms: u64) -> bool {
        // CAP.TO is in 500 ms units; convert to a generous spin budget
        let max_spins = (timeout_500ms + 1) * 100_000_000;
        let mut spins = 0u64;
        loop {
            if rd32(self.mmio + REG_CSTS) & CSTS_RDY == want {
                return true;
            }
            super::io_relax(spins);
            spins += 1;
            if spins > max_spins {
                return false;
            }
        }
    }

    fn sq_doorbell(&self, qid: u64) -> u64 {
        self.mmio + DOORBELLS + (2 * qid) * self.db_stride
    }

    fn cq_doorbell(&self, qid: u64) -> u64 {
        self.mmio + DOORBELLS + (2 * qid + 1) * self.db_stride
    }

    /// Submit one command on the given queue and poll its completion.
    /// Returns the completion status field (0 = success)
    fn submit(&mut self, qid: u64, entry: &[u8; 64]) -> Result<(), BlkError> {
        self.cid = self.cid.wrapping_add(1);
        let cid = self.cid;

        let (sq, cq, q) = if qid == 0 {
            (
                self.mem.admin_sq.as_mut_ptr(),
                self.mem.admin_cq.as_ptr(),
                &mut self.admin,
            )
        } else {
            (
                self.mem.io_sq.as_mut_ptr(),
                self.mem.io_cq.as_ptr(),
                &mut self.io,
            )
        };

        unsafe {
            let slot = sq.add(q.sq_tail as usize * 64);
            core::ptr::copy_nonoverlapping(entry.as_ptr(), slot, 64);
            // Patch the command id into dw0 bits 31:16
            core::ptr::write_volatile(
                (slot as *mut u16).add(1),
                cid,
            );
        }

        q.sq_tail = (q.sq_tail + 1) % q.depth;
        let tail = q.sq_tail;
        let head = q.cq_head;
        let phase = q.phase;
        let depth = q.depth;

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        wr32(self.sq_doorbell(qid), tail as u32);

        // Poll the completion entry's phase bit
        let cqe = unsafe { cq.add(head as usize * 16) };
        let mut spins = 0u64;
        let status: u16 = loop {
            let dw3 = unsafe {
                core::ptr::read_volatile((cqe as *const u32).add(3))
            };
            if ((dw3 >> 16) & 1) as u8 == phase {
                break (dw3 >> 17) as u16; // status field sans phase bit
            }
            super::io_relax(spins);
            spins += 1;
            if spins > 200_000_000 {
                return Err(BlkError::Timeout);
            }
        };

        // Advance CQ head (flip phase on wrap) and ring the head doorbell
        let q = if qid == 0 { &mut self.admin } else { &mut self.io };
        q.cq_head = (head + 1) % depth;
        if q.cq_head == 0 {
            q.phase ^= 1;
        }
        let new_head = q.cq_head;
        wr32(self.cq_doorbell(qid), new_head as u32);

        if status == 0 {
            Ok(())
        } else {
            crate::serial_println!("[nvme] command failed, status=0x{:04X}", status);
            Err(BlkError::DeviceFault)
        }
    }

    fn admin_cmd(&mut self, opcode: u8, nsid: u32, prp1: u64, cdw10: u32, cdw11: u32)
        -> Result<(), BlkError>
    {
        let mut e = [0u8; 64];
        e[0] = opcode;
        e[4..8].copy_from_slice(&nsid.to_le_bytes());
        e[24..32].copy_from_slice(&prp1.to_le_bytes());
        e[40..44].copy_from_slice(&cdw10.to_le_bytes());
        e[44..48].copy_from_slice(&cdw11.to_le_bytes());
        self.submit(0, &e)
    }

    /// One READ/WRITE of up to 128 sectors through the bounce buffer
    fn rw(&mut self, opcode: u8, lba: u64, sectors: u32) -> Result<(), BlkError> {
        let bytes = sectors as usize * 512;
        let bounce_phys = crate::net::virt_to_phys(self.mem.bounce.as_ptr() as u64);

        let mut e = [0u8; 64];
        e[0] = opcode;
        e[4..8].copy_from_slice(&self.nsid.to_le_bytes());
        e[24..32].copy_from_slice(&bounce_phys.to_le_bytes());
        // PRP2: second page directly, or the PRP list for longer transfers
        if bytes > 4096 {
            let prp2 = if bytes <= 8192 {
                bounce_phys + 4096
            } else {
                crate::net::virt_to_phys(self.mem.prp_list.as_ptr() as u64)
            };
            e[32..40].copy_from_slice(&prp2.to_le_bytes());
        }
        e[40..44].copy_from_slice(&(lba as u32).to_le_bytes());
        e[44..48].copy_from_slice(&((lba >> 32) as u32).to_le_bytes());
        e[48..52].copy_from_slice(&(sectors - 1).to_le_bytes()); // 0-based
        self.submit(1, &e)
    }
}

impl BlockDriver for Nvme {
    fn read_blocks(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            self.rw(NVM_READ, lba + done as u64, chunk)?;
            let off = done as usize * 512;
            buf[off..off + bytes].copy_from_slice(&self.mem.bounce[..bytes]);
            done += chunk;
        }
        Ok(())
    }

    fn write_blocks(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            let off = done as usize * 512;
            self.mem.bounce[..bytes].copy_from_slice(&buf[off..off + bytes]);
            self.rw(NVM_WRITE, lba + done as u64, chunk)?;
            done += chunk;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), BlkError> {
        let mut e = [0u8; 64];
        e[0] = NVM_FLUSH;
        e[4..8].copy_from_slice(&self.nsid.to_le_bytes());
        self.submit(1, &e)
    }

    fn info(&mut self) -> BlockDevInfo {
        let mut out = BlockDevInfo::unknown();
        out.total_sectors = self.capacity;
        out.lba48 = true;
        out.model = self.model;
        out.model_len = self.model_len;
        out.discard = self.has_dsm;
        out
    }

    /// Dataset Management with the deallocate attribute - NVMe's discard.
    /// A range descriptor is 16 bytes (context attributes, length in LBAs,
    /// starting LBA) and the whole request fits one descriptor, since
    /// 'count' is itself a u32 length in LBAs
    fn discard(&mut self, lba: u64, count: u32) -> Result<(), BlkError> {
        if !self.has_dsm {
            return Err(BlkError::Unsupported);
        }
        // The ident page doubles as scratch for admin-sized payloads
        // (health() does the same); a DSM range list reads only 16 bytes
        self.mem.ident[..4].fill(0);
        self.mem.ident[4..8].copy_from_slice(&count.to_le_bytes());
        self.mem.ident[8..16].copy_from_slice(&lba.to_le_bytes());
        let prp1 = crate::net::virt_to_phys(self.mem.ident.as_ptr() as u64);

        let mut e = [0u8; 64];
        e[0] = NVM_DSM;
        e[4..8].copy_from_slice(&self.nsid.to_le_bytes());
        e[24..32].copy_from_slice(&prp1.to_le_bytes());
        // cdw10 = number of ranges - 1 = 0; cdw11 bit 2 (AD) = deallocate
        e[44..48].copy_from_slice(&(1u32 << 2).to_le_bytes());
        self.submit(1, &e)
    }

    /// Write Zeroes: zeroes the range on the device without moving data.
    /// NLB is a 16-bit 0-based count, so up to 65536 sectors per command
    fn write_zeroes(&mut self, lba: u64, count: u32) -> Result<(), BlkError> {
        if !self.has_wz {
            return Err(BlkError::Unsupported);
        }
        let mut done = 0u32;
        while done < count {
            let n = (count - done).min(65536);
            let cur = lba + done as u64;
            let mut e = [0u8; 64];
            e[0] = NVM_WRITE_ZEROES;
            e[4..8].copy_from_slice(&self.nsid.to_le_bytes());
            e[40..44].copy_from_slice(&(cur as u32).to_le_bytes());
            e[44..48].copy_from_slice(&((cur >> 32) as u32).to_le_bytes());
            e[48..52].copy_from_slice(&(n - 1).to_le_bytes());
            self.submit(1, &e)?;
            done += n;
        }
        Ok(())
    }

    /// SMART / Health Information log page (LID 0x02, 512 bytes)
    fn health(&mut self) -> Option<HealthInfo> {
        let prp1 = crate::net::virt_to_phys(self.mem.ident.as_ptr() as u64);
        // cdw10: NUMD (dwords - 1) in bits 27:16, LID in bits 7:0
        let numd = (512 / 4 - 1) as u32;
        if self.admin_cmd(ADM_GET_LOG_PAGE, 0xFFFF_FFFF, prp1, (numd << 16) | 0x02, 0).is_err() {
            return None;
        }
        let log = &self.mem.ident;

        let crit_warning = log[0];
        // Composite temperature, Kelvin, LE u16
        let temp_k = u16::from_le_bytes([log[1], log[2]]);
        let pct_used = log[5];
        let u128le = |off: usize| -> u64 {
            // 16-byte LE counters; the low 8 bytes are plenty here
            u64::from_le_bytes(log[off..off + 8].try_into().unwrap_or([0; 8]))
        };
        // Data units are 1000 x 512 B
        let read_mb    = u128le(32).saturating_mul(512_000) / (1024 * 1024);
        let written_mb = u128le(48).saturating_mul(512_000) / (1024 * 1024);
        let poh        = u128le(128);

        Some(HealthInfo {
            healthy: crit_warning == 0,
            temp_c: if temp_k == 0 { i16::MIN } else { temp_k as i16 - 273 },
            percent_used: pct_used,
            power_on_hours: poh,
            data_read_mb: read_mb,
            data_written_mb: written_mb,
        })
    }
}
