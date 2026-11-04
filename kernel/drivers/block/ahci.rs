// AHCI (SATA) driver behind the 'BlockDriver' trait
//
// Modern chipsets expose SATA disks only through an AHCI HBA (PCI class 01.06);
// the legacy IDE ports the ATA driver uses do not exist there.
//
// One command slot, one PRD entry into a 64 KiB bounce buffer. The HBA
// registers are MMIO behind BAR5, mapped uncached through the HHDM window.
//
// Completions arrive by interrupt, as the NVMe driver's do: one MSI for the
// HBA. Its handler takes each interrupting port's PxIS bits, errors included,
// into `PORT_EVENTS`, clears them for the next message, and wakes the port's
// queue. The waiter looks at PxCI itself. It polls where it may not sleep
// (`nvme::may_sleep`'s rules), and everywhere with `nvmepoll`

extern crate alloc;

use crate::mm::dma::DmaBox;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::kcore::sched_lock::SchedMutex;
use crate::sched::WaitQueue;

use crate::io::block::driver::{BlkError, BlockDevInfo, BlockDriver};
use crate::drivers::bus::pci::{pci_read32, pci_read8, PCI_ADDR, PCI_DATA, pci_addr};

// HBA global registers (offsets from ABAR)
const HBA_CAP: u64 = 0x00;
const HBA_GHC: u64 = 0x04;
const HBA_IS:  u64 = 0x08;
const HBA_PI:  u64 = 0x0C;

const GHC_AE: u32 = 1 << 31;
const GHC_IE: u32 = 1 << 1;

// Per-port registers (offsets from ABAR + 0x100 + port * 0x80)
const PX_CLB:  u64 = 0x00;
const PX_CLBU: u64 = 0x04;
const PX_FB:   u64 = 0x08;
const PX_FBU:  u64 = 0x0C;
const PX_IS:   u64 = 0x10;
const PX_IE:   u64 = 0x14;
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

/// What a port interrupts for: the FISes a command ends with (D2H register,
/// PIO setup, DMA setup, set device bits, descriptor done) and every error
/// Linux's `DEF_PORT_IRQ` names
const PX_IE_EVENTS: u32 = 0x7DC0_007F;

/// The HBA's registers, for the interrupt handler; 0 until interrupts are on
static HBA: AtomicU64 = AtomicU64::new(0);

/// PxIS bits the handler took off each port since its command was issued
static PORT_EVENTS: [AtomicU32; 32] = [const { AtomicU32::new(0) }; 32];

/// Who waits for a command on each port
static PORT_WAIT: [WaitQueue; 32] = [const { WaitQueue::new() }; 32];

static IRQ_READY: AtomicBool = AtomicBool::new(false);

/// How long one sleep on a command lasts with no interrupt
const WAIT_BACKSTOP_NS: u64 = 10_000_000;

/// The longest a command may take, either way of waiting
const COMMAND_TIMEOUT_NS: u64 = 30_000_000_000;

/// The HBA's MSI: every port that raised one has its PxIS taken and cleared,
/// then the HBA's own summary, and the port's waiter is woken
fn hba_irq(_: usize) {
    let hba = HBA.load(Ordering::Acquire);
    if hba == 0 {
        return;
    }
    let is = rd32(hba + HBA_IS);
    for port in 0..32u64 {
        if is & (1 << port) == 0 {
            continue;
        }
        let pm = hba + 0x100 + port * 0x80;
        let pis = rd32(pm + PX_IS);
        PORT_EVENTS[port as usize].fetch_or(pis, Ordering::AcqRel);
        wr32(pm + PX_IS, pis);
        PORT_WAIT[port as usize].wake_all();
    }
    wr32(hba + HBA_IS, is);
}

/// Whether the caller may sleep for a command, by the rules the NVMe driver
/// keeps
fn may_sleep() -> bool {
    IRQ_READY.load(Ordering::Acquire)
        && crate::sched::started()
        && x86_64::instructions::interrupts::are_enabled()
        && !crate::sched::current_is_idle()
        && !crate::sched::current_is_dead()
}

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

// SAFETY: bytes only
unsafe impl crate::mm::dma::Zeroed for PortMem {}

pub struct AhciPort {
    port_mmio: u64,             // virtual base of this port's register block
    port_no:   u32,
    mem:       DmaBox<PortMem>,
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
        crate::io::block::io_relax(spins);
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
    // Class 01 subclass 06 is AHCI. BAR5 is the HBA register block and has to
    // be memory-mapped; one left in I/O space or unassigned cannot be driven
    let hba_dev = crate::drivers::bus::pci::find(|d| {
        d.class == 0x01 && d.subclass == 0x06 && d.bars[5] & 1 == 0 && d.bars[5] != 0
    });
    let hba_fn = hba_dev.map(|d| crate::drivers::bus::msi::PciFn { bus: d.bus, dev: d.dev, func: d.func });
    let abar_phys = hba_dev.map(|d| {
        // Memory space + bus master on
        let cmd = pci_read32(d.bus, d.dev, d.func, 0x04);
        unsafe {
            use x86_64::instructions::port::Port;
            Port::<u32>::new(PCI_ADDR).write(pci_addr(d.bus, d.dev, d.func, 0x04));
            Port::<u32>::new(PCI_DATA).write(cmd | 0x0006);
        }
        let abar = (d.bars[5] & 0xFFFF_FFF0) as u64;
        crate::serial_println!(
            "[ahci] HBA {:02x}:{:02x}.{} abar=0x{:08X}",
            d.bus, d.dev, d.func, abar
        );
        abar
    });

    let Some(abar) = abar_phys else { return 0 };

    // HBA register block: 0x100 of globals + 32 ports x 0x80
    crate::mm::vmm::map_mmio_uc(abar, 0x1100);
    let hba = crate::arch::x86_64::grub::phys_to_virt(abar);

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
    if count > 0 {
        if let Some(f) = hba_fn {
            enable_interrupts(f, hba, out);
        }
    }
    count
}

/// One MSI for the HBA, to the boot CPU, and every driven port told what to
/// raise it for. Leaves the ports polling if the HBA has no MSI or the
/// boot line says `nvmepoll`
fn enable_interrupts(f: crate::drivers::bus::msi::PciFn, hba: u64, ports: &[Option<AhciPort>; 4]) {
    use crate::arch::x86_64::apic;
    if crate::arch::x86_64::grub::cmdline_flag("nvmepoll") {
        return;
    }
    let caps = crate::drivers::bus::msi::read_caps(f);
    let Some(cap) = caps.msi else {
        crate::serial_println!("[ahci] no MSI capability - polling");
        return;
    };
    let Some(target) = apic::alloc_msi_vector(apic::lapic_id(), hba_irq, 0) else {
        crate::serial_println!("[ahci] no MSI vector free - polling");
        return;
    };
    HBA.store(hba, Ordering::Release);
    if crate::drivers::bus::msi::program_msi(f, &cap, target.address, target.data).is_err() {
        HBA.store(0, Ordering::Release);
        apic::free_msi_vector(target);
        return;
    }
    for p in ports.iter().flatten() {
        wr32(p.port_mmio + PX_IS, 0xFFFF_FFFF);
        wr32(p.port_mmio + PX_IE, PX_IE_EVENTS);
    }
    wr32(hba + HBA_IS, 0xFFFF_FFFF);
    wr32(hba + HBA_GHC, rd32(hba + HBA_GHC) | GHC_IE);
    IRQ_READY.store(true, Ordering::Release);
    crate::serial_println!("[ahci] completions by MSI on vector {:#x}", target.vector);
}

impl AhciPort {
    fn init(pm: u64, port_no: u32) -> Option<Self> {
        // Stop command engine and FIS receive before touching the pointers
        wr32(pm + PX_CMD, rd32(pm + PX_CMD) & !CMD_ST);
        if !wait_clear(pm + PX_CMD, CMD_CR, 5_000_000) { return None; }
        wr32(pm + PX_CMD, rd32(pm + PX_CMD) & !CMD_FRE);
        if !wait_clear(pm + PX_CMD, CMD_FR, 5_000_000) { return None; }

        // Below 4 GiB: the upper dwords are written as zero (no S64A use)
        let mem = DmaBox::<PortMem>::new_below(crate::mm::dma::DMA32).ok()?;

        let clb_phys = mem.phys_of(mem.cmd_list.as_ptr());
        let fb_phys  = mem.phys_of(mem.fis.as_ptr());
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
            port_no,
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
            crate::io::block::io_relax(spins);
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
            let dba = self.mem.phys_of(self.mem.bounce.as_ptr());
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
        let ctba = self.mem.phys_of(self.mem.cmd_tbl.as_ptr());
        let prdtl: u32 = if with_data { 1 } else { 0 };
        let dw0: u32 = 5 | ((write as u32) << 6) | (prdtl << 16);
        let hdr = &mut self.mem.cmd_list[..32];
        hdr.fill(0);
        hdr[0..4].copy_from_slice(&dw0.to_le_bytes());
        hdr[8..12].copy_from_slice(&(ctba as u32).to_le_bytes());

        let events = &PORT_EVENTS[self.port_no as usize & 31];
        events.store(0, Ordering::Release);
        wr32(pm + PX_IS, 0xFFFF_FFFF);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        wr32(pm + PX_CI, 1);

        // What PxIS says now and what the handler has already taken off it
        let is = || rd32(pm + PX_IS) | events.load(Ordering::Acquire);
        let finished = || rd32(pm + PX_CI) & 1 == 0 || is() & IS_ERR_MASK != 0;
        if may_sleep() {
            let start = crate::kcore::time::now_ns();
            let give_up = start.saturating_add(COMMAND_TIMEOUT_NS);
            loop {
                if finished() {
                    break;
                }
                let now = crate::kcore::time::now_ns();
                // Polled first, as the NVMe driver does, for a command done
                // before a sleep would pay for itself
                if now < start + crate::drivers::block::nvme::SPIN_BEFORE_SLEEP_NS {
                    core::hint::spin_loop();
                    continue;
                }
                if now > give_up {
                    return Err(BlkError::Timeout);
                }
                PORT_WAIT[self.port_no as usize & 31].wait_unless(now + WAIT_BACKSTOP_NS, finished);
            }
        } else {
            let mut spins = 0u64;
            while !finished() {
                crate::io::block::io_relax(spins);
                spins += 1;
                if spins > 200_000_000 {
                    return Err(BlkError::Timeout);
                }
            }
        }
        let status = is();
        if status & IS_ERR_MASK != 0 {
            crate::serial_println!("[ahci] error IS=0x{:08X} TFD=0x{:08X}",
                status, rd32(pm + PX_TFD));
            wr32(pm + PX_SERR, 0xFFFF_FFFF);
            return Err(BlkError::DeviceFault);
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
pub struct AhciBlockDev(SchedMutex<AhciPort>);

impl AhciBlockDev {
    pub fn new(port: AhciPort) -> Self {
        Self(SchedMutex::new(port))
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
    /// range entries, 48-bit LBA in the low bits and a 16-bit sector count in
    /// the top, so one block describes up to 64 ranges; long discards loop
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
