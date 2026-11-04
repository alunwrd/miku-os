// NVMe driver behind the 'BlockDriver' trait
//
// One admin queue pair plus NUM_IO_QUEUES I/O queue pairs, one outstanding
// command per queue, data through a 64 KiB bounce buffer per queue addressed
// with PRP1 + a PRP list page.
//
// Completions arrive by interrupt (`dev/fs-architecture.md`, "NVMe on
// interrupts"). Each I/O completion queue has its own MSI-X entry, and its
// handler only wakes whoever waits on that queue (`IO_WAIT`); the waiter reads
// the completion itself. The admin queue polls (it runs at bring-up), and so
// does an I/O command that cannot sleep: no scheduler yet, interrupts off, the
// idle task, a dead task (`dev/kernel-core.md`, "A dead task never comes
// back"). `nvmepoll` on the boot line polls every command

extern crate alloc;

use crate::mm::dma::DmaBox;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex as SpinMutex;

use crate::kcore::sched_lock::SchedMutex;
use crate::sched::WaitQueue;

use crate::io::block::driver::{BlkError, BlockDevInfo, BlockDriver, HealthInfo};
use crate::drivers::bus::pci::{pci_read32, pci_read8, PCI_ADDR, PCI_DATA, pci_addr};

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

/// Independent I/O queue pairs. Requests route to one by CPU, so this many CPUs
/// can have NVMe commands in flight at once
const NUM_IO_QUEUES: usize = 4;

/// Who waits for a completion on each I/O queue, woken by that queue's
/// interrupt. Static, and indexed by queue: the handler has nothing else to
/// find them by
static IO_WAIT: [WaitQueue; NUM_IO_QUEUES] = [const { WaitQueue::new() }; NUM_IO_QUEUES];

/// Completions are signalled. False until every queue's MSI-X entry is set and
/// the function's MSI-X is on, and always with `nvmepoll`
static IRQ_READY: AtomicBool = AtomicBool::new(false);

/// How long one sleep on a completion lasts with no interrupt: the backstop,
/// for an interrupt that was lost
const WAIT_BACKSTOP_NS: u64 = 10_000_000;

/// The longest a command may take, either way of waiting
const COMMAND_TIMEOUT_NS: u64 = 30_000_000_000;

/// How long a waiter polls before it sleeps. A read from a fast SSD finishes
/// inside this with no interrupt and no switch; a longer one frees the
/// processor. In QEMU, where a 64 KiB read takes ~0.6 ms, a sleep and its wake
/// cost about a quarter on top of polling (87 against 111 MB/s cold)
pub const SPIN_BEFORE_SLEEP_NS: u64 = 50_000;

/// A queue's MSI-X handler: in interrupt context, so it reads nothing and
/// only wakes
fn completion_irq(queue: usize) {
    if let Some(w) = IO_WAIT.get(queue) {
        w.wake_all();
    }
}

/// Whether the caller may sleep for a completion rather than poll for it
fn may_sleep() -> bool {
    IRQ_READY.load(Ordering::Acquire)
        && crate::sched::started()
        && x86_64::instructions::interrupts::are_enabled()
        && !crate::sched::current_is_idle()
        && !crate::sched::current_is_dead()
}

pub const MAX_XFER_SECTORS: u32 = 128; // 64 KiB per command
const BOUNCE_SIZE: usize = MAX_XFER_SECTORS as usize * 512;

/// Admin queue pair plus the IDENTIFY/log scratch page. One per controller
#[repr(C, align(4096))]
struct AdminMem {
    sq:    [u8; 4096],
    cq:    [u8; 4096],
    ident: [u8; 4096],
}

// SAFETY: bytes only
unsafe impl crate::mm::dma::Zeroed for AdminMem {}

/// One I/O queue pair's controller-visible memory, page-aligned. Each queue
/// has its own bounce buffer and PRP list, so the queues never touch each
/// other's DMA region and can run fully in parallel
#[repr(C, align(4096))]
struct IoQueueMem {
    sq:     [u8; 4096],
    cq:     [u8; 4096],
    prp:    [u8; 4096],
    bounce: [u8; BOUNCE_SIZE],
}

// SAFETY: bytes only
unsafe impl crate::mm::dma::Zeroed for IoQueueMem {}

/// Mutable ring cursors for one queue, guarded so concurrent submitters on
/// the same queue serialize while different queues proceed independently
struct QState {
    sq_tail: u16,
    cq_head: u16,
    phase:   u8,
    cid:     u16,
}

impl QState {
    const fn new() -> Self {
        Self { sq_tail: 0, cq_head: 0, phase: 1, cid: 0 }
    }
}

/// One I/O queue: its DMA memory plus a lock over its cursors. A sleeping
/// lock: its holder sleeps for the completion, and so does whoever is next
struct IoQueue {
    qid: u16,
    mem: DmaBox<IoQueueMem>,
    st:  SchedMutex<QState>,
}

pub struct Nvme {
    mmio:      u64,   // virtual base of BAR0
    /// Where it is on the bus, for its MSI-X capability
    pci:       Option<crate::drivers::bus::msi::PciFn>,
    db_stride: u64,
    admin_mem: DmaBox<AdminMem>,
    admin:     SpinMutex<QState>,
    admin_depth: u16,
    io_depth:  u16,
    io:        alloc::vec::Vec<IoQueue>,
    /// Round-robin fallback for queue selection before per-CPU is meaningful
    rr:        core::sync::atomic::AtomicUsize,
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
    // Class 01 subclass 08 is NVM Express. BAR0 must be memory, and may be
    // a 64-bit BAR whose upper half lives in BAR1
    let ctrl = crate::drivers::bus::pci::find(|d| {
        d.class == 0x01 && d.subclass == 0x08 && d.mem_bar(0).unwrap_or(0) != 0
    })?;
    let (bus, dev, func) = (ctrl.bus, ctrl.dev, ctrl.func);
    let bar = ctrl.mem_bar(0)?;

    // Memory space + bus master on
    let cmd = pci_read32(bus, dev, func, 0x04);
    unsafe {
        use x86_64::instructions::port::Port;
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, 0x04));
        Port::<u32>::new(PCI_DATA).write(cmd | 0x0006);
    }
    crate::serial_println!("[nvme] controller {:02x}:{:02x}.{} bar0=0x{:X}", bus, dev, func, bar);

    // Registers + doorbells (stride is per CAP.DSTRD; map enough for both)
    crate::mm::vmm::map_mmio_uc(bar, 0x4000);
    let mmio = crate::arch::x86_64::grub::phys_to_virt(bar);

    Nvme::init(mmio, Some(crate::drivers::bus::msi::PciFn { bus, dev, func }))
}

impl Nvme {
    fn init(mmio: u64, pci: Option<crate::drivers::bus::msi::PciFn>) -> Option<Self> {
        let cap = rd64(mmio + REG_CAP);
        let db_stride = 4u64 << ((cap >> 32) & 0xF);
        let mqes = (cap & 0xFFFF) as usize + 1;
        let timeout_500ms = ((cap >> 24) & 0xFF) as u64;

        let admin_depth = ADMIN_QD.min(mqes) as u16;
        let io_depth = IO_QD.min(mqes) as u16;

        let mut drv = Nvme {
            mmio,
            pci,
            db_stride,
            // NVMe addresses 64 bits: anywhere in memory
            admin_mem: DmaBox::new().ok()?,
            admin: SpinMutex::new(QState::new()),
            admin_depth,
            io_depth,
            io: alloc::vec::Vec::new(),
            rr: core::sync::atomic::AtomicUsize::new(0),
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

        let asq = drv.admin_mem.phys_of(drv.admin_mem.sq.as_ptr());
        let acq = drv.admin_mem.phys_of(drv.admin_mem.cq.as_ptr());
        let aqa = ((admin_depth as u32 - 1) << 16) | (admin_depth as u32 - 1);
        wr32(mmio + REG_AQA, aqa);
        wr64(mmio + REG_ASQ, asq);
        wr64(mmio + REG_ACQ, acq);

        // Program the config first, then flip EN in a second write: some
        // controllers reject config changes in the write that enables
        wr32(mmio + REG_CC, CC_CONFIG & !CC_ENABLE);
        wr32(mmio + REG_CC, CC_CONFIG);
        if !drv.wait_csts(CSTS_RDY, timeout_500ms) {
            crate::serial_println!("[nvme] controller did not become ready");
            return None;
        }

        // IDENTIFY controller (CNS=1): model string for blkstat
        let ident_phys = drv.admin_mem.phys_of(drv.admin_mem.ident.as_ptr());
        if drv.admin_cmd(ADM_IDENTIFY, 0, ident_phys, 1, 0).is_err() {
            crate::serial_println!("[nvme] IDENTIFY controller failed");
            return None;
        }
        let mut n = 0usize;
        for &b in &drv.admin_mem.ident[24..64] {
            if n < drv.model.len() { drv.model[n] = b; n += 1; }
        }
        while n > 0 && (drv.model[n - 1] == b' ' || drv.model[n - 1] == 0) { n -= 1; }
        drv.model_len = n as u8;

        // Optional NVM command support (ONCS, bytes 520-521):
        // bit 2 = DSM (discard), bit 3 = Write Zeroes
        let oncs = u16::from_le_bytes([drv.admin_mem.ident[520], drv.admin_mem.ident[521]]);
        drv.has_dsm = oncs & (1 << 2) != 0;
        drv.has_wz  = oncs & (1 << 3) != 0;

        // IDENTIFY namespace 1 (CNS=0): size + LBA format
        if drv.admin_cmd(ADM_IDENTIFY, drv.nsid, ident_phys, 0, 0).is_err() {
            crate::serial_println!("[nvme] IDENTIFY namespace failed");
            return None;
        }
        let nsze = u64::from_le_bytes(drv.admin_mem.ident[0..8].try_into().ok()?);
        let flbas = drv.admin_mem.ident[26] & 0x0F;
        let lbaf_off = 128 + flbas as usize * 4;
        let lbads = drv.admin_mem.ident[lbaf_off + 2];
        if lbads != 9 {
            crate::serial_println!(
                "[nvme] namespace LBA size 2^{} unsupported (need 512 B)", lbads
            );
            return None;
        }
        drv.capacity = nsze;

        // Interrupts, if the function has MSI-X with an entry for every I/O
        // queue (entry 0 is the admin queue's, which polls). Without, or with
        // `nvmepoll`, every queue is created without one and polls
        let msix = drv.pci.and_then(|f| {
            let caps = crate::drivers::bus::msi::read_caps(f);
            crate::drivers::bus::msi::log_capabilities("nvme", &caps);
            caps.msix
        }).filter(|c| c.table_size as usize > NUM_IO_QUEUES)
          .filter(|_| !crate::arch::x86_64::grub::cmdline_flag("nvmepoll"));
        // Before the queues: a controller refuses a completion queue with a
        // vector while MSI-X is off ("invalid interrupt vector", 01h/08h)
        let irq = match (drv.pci, msix) {
            (Some(f), Some(cap)) => drv.enable_interrupts(f, &cap),
            _ => false,
        };

        // Create NUM_IO_QUEUES independent I/O queue pairs (qid 1..=N). Each
        // gets its own CQ then SQ; a per-queue PRP list is filled once
        let qsize = (io_depth as u32 - 1) << 16;
        for i in 0..NUM_IO_QUEUES {
            let qid = (i + 1) as u32;
            let mut mem = DmaBox::<IoQueueMem>::new().ok()?;

            let cq_phys = mem.phys_of(mem.cq.as_ptr());
            let sq_phys = mem.phys_of(mem.sq.as_ptr());
            // CREATE_IO_CQ: cdw10 = qsize|qid, cdw11 = IV (MSI-X entry qid)
            // in 31:16, IEN in bit 1, PC in bit 0
            let cq_flags = if irq { (qid << 16) | 2 | 1 } else { 1 };
            if drv.admin_cmd(ADM_CREATE_IO_CQ, 0, cq_phys, qsize | qid, cq_flags).is_err() {
                crate::serial_println!("[nvme] create IO CQ {} failed", qid);
                return None;
            }
            // CREATE_IO_SQ: cdw10 = qsize|qid, cdw11 = (cqid<<16)|PC
            if drv.admin_cmd(ADM_CREATE_IO_SQ, 0, sq_phys, qsize | qid, (qid << 16) | 1).is_err() {
                crate::serial_println!("[nvme] create IO SQ {} failed", qid);
                return None;
            }

            // PRP list for bounce pages 1.. (page 0 goes in PRP1)
            let bounce_phys = mem.phys_of(mem.bounce.as_ptr());
            for p in 1..(BOUNCE_SIZE / 4096) {
                let entry = bounce_phys + p as u64 * 4096;
                let off = (p - 1) * 8;
                mem.prp[off..off + 8].copy_from_slice(&entry.to_le_bytes());
            }

            drv.io.push(IoQueue { qid: qid as u16, mem, st: SchedMutex::new(QState::new()) });
        }
        // Waits sleep from here: every queue that will raise one exists
        IRQ_READY.store(irq, Ordering::Release);

        crate::serial_println!(
            "[nvme] '{}' ns{} {} sectors ({} MB) {} io-queues x qd={} dsm={} wz={} completions={}",
            core::str::from_utf8(&drv.model[..drv.model_len as usize]).unwrap_or("?"),
            drv.nsid, drv.capacity, drv.capacity * 512 / (1024 * 1024),
            NUM_IO_QUEUES, io_depth, drv.has_dsm, drv.has_wz,
            if IRQ_READY.load(Ordering::Acquire) { "msi-x" } else { "polled" }
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
            crate::io::block::io_relax(spins);
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

    /// One MSI-X entry per I/O queue, all to the boot CPU, the only one up when
    /// the block layer probes, and the function's MSI-X on. Every queue polls
    /// if any part of it cannot be had. Entry 0 is the admin queue's and stays
    /// masked
    fn enable_interrupts(&self, f: crate::drivers::bus::msi::PciFn, cap: &crate::drivers::bus::msi::MsixCapability) -> bool {
        use crate::arch::x86_64::apic;
        let Some(table) = crate::drivers::bus::msi::map_msix(f, cap) else {
            crate::serial_println!("[nvme] MSI-X table not in a memory BAR - polling");
            return false;
        };
        let lapic = apic::lapic_id();
        let mut taken = alloc::vec::Vec::new();
        for i in 0..NUM_IO_QUEUES {
            let Some(t) = apic::alloc_msi_vector(lapic, completion_irq, i) else {
                crate::serial_println!("[nvme] no MSI vector free for queue {} - polling", i + 1);
                for (k, t) in taken.into_iter().enumerate() {
                    table.mask((k + 1) as u16);
                    apic::free_msi_vector(t);
                }
                return false;
            };
            table.set((i + 1) as u16, t.address, t.data);
            taken.push(t);
        }
        crate::drivers::bus::msi::enable_msix(f, cap);
        true
    }

    /// One command on I/O queue `qi`, waited for (`submit_locked`). The queue's
    /// lock is held throughout, so callers on one queue take turns while
    /// different queues run in parallel. Takes '&self': the per-queue lock
    /// provides the mutability, and each queue owns disjoint SQ/CQ memory
    fn io_cmd(&self, qi: usize, entry: &[u8; 64]) -> Result<(), BlkError> {
        let q = &self.io[qi];
        let sq = q.mem.sq.as_ptr() as *mut u8;
        let cq = q.mem.cq.as_ptr();
        let mut g = q.st.lock();
        self.submit_locked(sq, cq, &mut g, q.qid as u64, self.io_depth, entry, Some(qi))
    }

    /// Core submit-and-wait with the queue's cursor lock already held by the
    /// caller (read/write hold it across the bounce-buffer copy).
    ///
    /// `wait` is the I/O queue to sleep on for the completion; `None`, or a
    /// caller that may not sleep, polls the completion's phase bit
    #[allow(clippy::too_many_arguments)]
    fn submit_locked(
        &self,
        sq: *mut u8,
        cq: *const u8,
        q: &mut QState,
        qid: u64,
        depth: u16,
        entry: &[u8; 64],
        wait: Option<usize>,
    ) -> Result<(), BlkError> {
        q.cid = q.cid.wrapping_add(1);
        let cid = q.cid;
        unsafe {
            let slot = sq.add(q.sq_tail as usize * 64);
            core::ptr::copy_nonoverlapping(entry.as_ptr(), slot, 64);
            // Patch the command id into dw0 bits 31:16
            core::ptr::write_volatile((slot as *mut u16).add(1), cid);
        }

        q.sq_tail = (q.sq_tail + 1) % depth;
        let tail = q.sq_tail;
        let head = q.cq_head;
        let phase = q.phase;

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        wr32(self.sq_doorbell(qid), tail as u32);

        // This queue's completion entry: done when its phase bit is the
        // queue's current phase
        let cqe = unsafe { cq.add(head as usize * 16) };
        let dw3 = || unsafe { core::ptr::read_volatile((cqe as *const u32).add(3)) };
        let done = || ((dw3() >> 16) & 1) as u8 == phase;
        let status: u16 = match wait {
            Some(qi) if may_sleep() => {
                let start = crate::kcore::time::now_ns();
                let give_up = start.saturating_add(COMMAND_TIMEOUT_NS);
                loop {
                    if done() {
                        break (dw3() >> 17) as u16;
                    }
                    let now = crate::kcore::time::now_ns();
                    if now < start + SPIN_BEFORE_SLEEP_NS {
                        core::hint::spin_loop();
                        continue;
                    }
                    if now > give_up {
                        return Err(BlkError::Timeout);
                    }
                    // On the queue and asleep-on-request before the bit is
                    // looked at, so the interrupt cannot fall in between
                    IO_WAIT[qi].wait_unless(now + WAIT_BACKSTOP_NS, done);
                }
            }
            _ => {
                let mut spins = 0u64;
                loop {
                    if done() {
                        break (dw3() >> 17) as u16;
                    }
                    crate::io::block::io_relax(spins);
                    spins += 1;
                    if spins > 200_000_000 {
                        return Err(BlkError::Timeout);
                    }
                }
            }
        };

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

    /// Admin command on queue 0 (serialized; admin traffic is rare)
    fn admin_cmd(&self, opcode: u8, nsid: u32, prp1: u64, cdw10: u32, cdw11: u32)
        -> Result<(), BlkError>
    {
        let mut e = [0u8; 64];
        e[0] = opcode;
        e[4..8].copy_from_slice(&nsid.to_le_bytes());
        e[24..32].copy_from_slice(&prp1.to_le_bytes());
        e[40..44].copy_from_slice(&cdw10.to_le_bytes());
        e[44..48].copy_from_slice(&cdw11.to_le_bytes());
        let sq = self.admin_mem.sq.as_ptr() as *mut u8;
        let cq = self.admin_mem.cq.as_ptr();
        let mut g = self.admin.lock();
        self.submit_locked(sq, cq, &mut g, 0, self.admin_depth, &e, None)
    }

    /// Pick an I/O queue for the current CPU (falls back to round-robin)
    fn pick_queue(&self) -> usize {
        let cpu = crate::arch::x86_64::percpu::current_index();
        if cpu < self.io.len() {
            cpu
        } else {
            self.rr.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % self.io.len()
        }
    }


    /// One read chunk into 'out' through queue qi's bounce buffer, the
    /// queue lock held across the whole transfer so the bounce is exclusive
    fn read_chunk(&self, qi: usize, lba: u64, sectors: u32, out: &mut [u8]) -> Result<(), BlkError> {
        let q = &self.io[qi];
        let bytes = sectors as usize * 512;
        let bounce_phys = q.mem.phys_of(q.mem.bounce.as_ptr());
        let prp_phys = q.mem.phys_of(q.mem.prp.as_ptr());
        let e = build_rw(NVM_READ, self.nsid, bounce_phys, prp_phys, lba, sectors, false);
        let sq = q.mem.sq.as_ptr() as *mut u8;
        let cq = q.mem.cq.as_ptr();
        let mut g = q.st.lock();
        self.submit_locked(sq, cq, &mut g, q.qid as u64, self.io_depth, &e, Some(qi))?;
        out[..bytes].copy_from_slice(&q.mem.bounce[..bytes]);
        Ok(())
    }

    /// One write chunk from 'data' through queue qi's bounce buffer
    fn write_chunk(&self, qi: usize, lba: u64, sectors: u32, data: &[u8], fua: bool) -> Result<(), BlkError> {
        let q = &self.io[qi];
        let bytes = sectors as usize * 512;
        let bounce_phys = q.mem.phys_of(q.mem.bounce.as_ptr());
        let prp_phys = q.mem.phys_of(q.mem.prp.as_ptr());
        let e = build_rw(NVM_WRITE, self.nsid, bounce_phys, prp_phys, lba, sectors, fua);
        let sq = q.mem.sq.as_ptr() as *mut u8;
        let cq = q.mem.cq.as_ptr();
        let mut g = q.st.lock();
        // Stage the data into the bounce under the queue lock
        unsafe {
            let dst = q.mem.bounce.as_ptr() as *mut u8;
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, bytes);
        }
        self.submit_locked(sq, cq, &mut g, q.qid as u64, self.io_depth, &e, Some(qi))
    }
}

/// Build an NVM READ/WRITE command entry. The bounce buffer holds the data;
/// PRP2 is the second page directly (<= 8 KiB) or the queue's PRP list
fn build_rw(opcode: u8, nsid: u32, bounce_phys: u64, prp_phys: u64,
            lba: u64, sectors: u32, fua: bool) -> [u8; 64] {
    let bytes = sectors as usize * 512;
    let mut e = [0u8; 64];
    e[0] = opcode;
    e[4..8].copy_from_slice(&nsid.to_le_bytes());
    e[24..32].copy_from_slice(&bounce_phys.to_le_bytes());
    if bytes > 4096 {
        let prp2 = if bytes <= 8192 { bounce_phys + 4096 } else { prp_phys };
        e[32..40].copy_from_slice(&prp2.to_le_bytes());
    }
    e[40..44].copy_from_slice(&(lba as u32).to_le_bytes());
    e[44..48].copy_from_slice(&((lba >> 32) as u32).to_le_bytes());
    // CDW12: NLB in bits 15:0 (0-based), FUA in bit 30
    let cdw12 = (sectors - 1) | if fua { 1 << 30 } else { 0 };
    e[48..52].copy_from_slice(&cdw12.to_le_bytes());
    e
}

impl BlockDriver for Nvme {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let qi = self.pick_queue();
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            let off = done as usize * 512;
            self.read_chunk(qi, lba + done as u64, chunk, &mut buf[off..off + bytes])?;
            done += chunk;
        }
        Ok(())
    }

    fn write_blocks(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let qi = self.pick_queue();
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            let off = done as usize * 512;
            self.write_chunk(qi, lba + done as u64, chunk, &buf[off..off + bytes], false)?;
            done += chunk;
        }
        Ok(())
    }

    fn write_blocks_fua(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let qi = self.pick_queue();
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            let off = done as usize * 512;
            self.write_chunk(qi, lba + done as u64, chunk, &buf[off..off + bytes], true)?;
            done += chunk;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlkError> {
        let mut e = [0u8; 64];
        e[0] = NVM_FLUSH;
        e[4..8].copy_from_slice(&self.nsid.to_le_bytes());
        self.io_cmd(self.pick_queue(), &e)
    }

    fn info(&self) -> BlockDevInfo {
        let mut out = BlockDevInfo::unknown();
        out.total_sectors = self.capacity;
        out.lba48 = true;
        out.model = self.model;
        out.model_len = self.model_len;
        out.discard = self.has_dsm;
        out
    }

    /// Dataset Management with the deallocate attribute - NVMe's discard.
    /// The 16-byte range descriptor is staged in queue 0's bounce buffer
    /// (exclusive under its lock; the bounce is rewritten by the next I/O)
    fn discard(&self, lba: u64, count: u32) -> Result<(), BlkError> {
        if !self.has_dsm {
            return Err(BlkError::Unsupported);
        }
        let q = &self.io[0];
        let bounce_phys = q.mem.phys_of(q.mem.bounce.as_ptr());
        let sq = q.mem.sq.as_ptr() as *mut u8;
        let cq = q.mem.cq.as_ptr();
        let mut g = q.st.lock();
        unsafe {
            let b = q.mem.bounce.as_ptr() as *mut u8;
            core::ptr::write_bytes(b, 0, 16);
            // ctx attrs (4) = 0; length in LBAs (4); starting LBA (8)
            core::ptr::copy_nonoverlapping(count.to_le_bytes().as_ptr(), b.add(4), 4);
            core::ptr::copy_nonoverlapping(lba.to_le_bytes().as_ptr(), b.add(8), 8);
        }
        let mut e = [0u8; 64];
        e[0] = NVM_DSM;
        e[4..8].copy_from_slice(&self.nsid.to_le_bytes());
        e[24..32].copy_from_slice(&bounce_phys.to_le_bytes());
        // cdw10 = number of ranges - 1 = 0; cdw11 bit 2 (AD) = deallocate
        e[44..48].copy_from_slice(&(1u32 << 2).to_le_bytes());
        self.submit_locked(sq, cq, &mut g, q.qid as u64, self.io_depth, &e, Some(0))
    }

    /// Write Zeroes: zeroes the range on the device without moving data.
    /// NLB is a 16-bit 0-based count, so up to 65536 sectors per command
    fn write_zeroes(&self, lba: u64, count: u32) -> Result<(), BlkError> {
        if !self.has_wz {
            return Err(BlkError::Unsupported);
        }
        let qi = self.pick_queue();
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
            self.io_cmd(qi, &e)?;
            done += n;
        }
        Ok(())
    }

    /// Smart / Health Information log page (LID 0x02, 512 bytes). Uses the
    /// admin queue's scratch page
    fn health(&self) -> Option<HealthInfo> {
        let prp1 = self.admin_mem.phys_of(self.admin_mem.ident.as_ptr());
        let numd = (512 / 4 - 1) as u32;
        if self.admin_cmd(ADM_GET_LOG_PAGE, 0xFFFF_FFFF, prp1, (numd << 16) | 0x02, 0).is_err() {
            return None;
        }
        let log = &self.admin_mem.ident;
        let crit_warning = log[0];
        let temp_k = u16::from_le_bytes([log[1], log[2]]);
        let pct_used = log[5];
        let u128le = |off: usize| -> u64 {
            u64::from_le_bytes(log[off..off + 8].try_into().unwrap_or([0; 8]))
        };
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
