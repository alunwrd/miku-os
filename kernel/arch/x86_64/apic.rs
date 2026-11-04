// Local APIC together with IO-APIC drivers replacing 8259 PIC

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::PageTableFlags;

use crate::arch::x86_64::grub;

// LAPIC register offsets
pub const LAPIC_ID:        u32 = 0x020;
pub const LAPIC_VERSION:   u32 = 0x030;
pub const LAPIC_TPR:       u32 = 0x080;
pub const LAPIC_EOI:       u32 = 0x0B0;
pub const LAPIC_LDR:       u32 = 0x0D0;
pub const LAPIC_DFR:       u32 = 0x0E0;
pub const LAPIC_SVR:       u32 = 0x0F0;
pub const LAPIC_ESR:       u32 = 0x280;
pub const LAPIC_ICR_LO:    u32 = 0x300;
pub const LAPIC_ICR_HI:    u32 = 0x310;
pub const LAPIC_LVT_CMCI:    u32 = 0x2F0;
pub const LAPIC_LVT_TIMER:   u32 = 0x320;
pub const LAPIC_LVT_THERMAL: u32 = 0x330;
pub const LAPIC_LVT_PERF:    u32 = 0x340;
pub const LAPIC_LVT_LINT0:   u32 = 0x350;
pub const LAPIC_LVT_LINT1:   u32 = 0x360;
pub const LAPIC_LVT_ERROR:   u32 = 0x370;
pub const LAPIC_IRR_BASE:    u32 = 0x200;
pub const LAPIC_ISR_BASE:    u32 = 0x100;
pub const LAPIC_INIT_CNT:    u32 = 0x380;
pub const LAPIC_CUR_CNT:   u32 = 0x390;
pub const LAPIC_DIV_CONF:  u32 = 0x3E0;

// ICR bits
const ICR_DELIVERY_INIT:    u32 = 0b101 << 8;
const ICR_DELIVERY_STARTUP: u32 = 0b110 << 8;
const ICR_DELIVERY_FIXED:   u32 = 0b000 << 8;
const ICR_LEVEL_ASSERT:     u32 = 1 << 14;
const ICR_TRIGGER_LEVEL:    u32 = 1 << 15;
const ICR_DEST_SELF:        u32 = 0b01 << 18;
const ICR_DEST_ALL:         u32 = 0b10 << 18;
const ICR_DEST_ALL_BUT_SELF: u32 = 0b11 << 18;
const ICR_PENDING:          u32 = 1 << 12;

// SVR bits
const SVR_ENABLE: u32 = 0x100;

// Vectors its our IDT layout:
// 32-47: legacy ISA irqs (via IOAPIC), same offsets as old PIC
// 0xE0-0xEF: IPIs
// 0xFE: LAPIC error
// 0xFF: spurious
pub const VEC_TIMER:     u8 = 0x20;  // 32
pub const VEC_KEYBOARD:  u8 = 0x21;
pub const VEC_MOUSE:     u8 = 0x2C;
pub const VEC_COM1:      u8 = 0x24;
pub const VEC_ATA_PRI:   u8 = 0x2E;
pub const VEC_ATA_SEC:   u8 = 0x2F;
pub const VEC_IPI_RESCHED: u8 = 0xE0;
pub const VEC_IPI_TLB:   u8 = 0xE1;
pub const VEC_IPI_HALT:  u8 = 0xE2;
pub const VEC_APIC_ERR:  u8 = 0xFE;
pub const VEC_SPURIOUS:  u8 = 0xFF;

static LAPIC_BASE_VIRT:   AtomicU64 = AtomicU64::new(0);
static LAPIC_TIMER_HZ:    AtomicU32 = AtomicU32::new(0);
static LAPIC_TICKS_PER_HZ: AtomicU32 = AtomicU32::new(0);
static INITIALIZED:       AtomicBool = AtomicBool::new(false);

pub const TIMER_HZ_DEFAULT: u32 = 250;

pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

#[inline(always)]
unsafe fn lapic_reg(off: u32) -> *mut u32 {
    (LAPIC_BASE_VIRT.load(Ordering::Relaxed) + off as u64) as *mut u32
}

#[inline(always)]
pub unsafe fn lapic_read(off: u32) -> u32 {
    core::ptr::read_volatile(lapic_reg(off))
}

#[inline(always)]
pub unsafe fn lapic_write(off: u32, val: u32) {
    core::ptr::write_volatile(lapic_reg(off), val);
}

#[inline(always)]
pub fn eoi() {
    unsafe { lapic_write(LAPIC_EOI, 0); }
}

pub fn lapic_id() -> u32 {
    unsafe { lapic_read(LAPIC_ID) >> 24 }
}

// Mask all 8259 PIC IRQs and leave it in a dummy initialized state
pub fn disable_8259() {
    unsafe {
        let mut a1: Port<u8> = Port::new(0xA1);
        let mut b1: Port<u8> = Port::new(0x21);
        let mut cmd_a: Port<u8> = Port::new(0xA0);
        let mut cmd_b: Port<u8> = Port::new(0x20);

        // ICW1 - cascade mode, ICW4 needed
        cmd_b.write(0x11);
        cmd_a.write(0x11);
        // ICW2 - vector offsets (remap to 0x20/0x28 even though masked)
        b1.write(0x20);
        a1.write(0x28);
        // ICW3 - wiring
        b1.write(0x04);
        a1.write(0x02);
        // ICW4 - 8086 mode
        b1.write(0x01);
        a1.write(0x01);
        // Mask everything
        b1.write(0xFF);
        a1.write(0xFF);
    }
    crate::serial_println!("[apic] 8259 PIC disabled (all irqs masked)");
}

// Map LAPIC MMIO into the kernel's HHDM region with uncacheable flags. The boot
// HHDM uses 1-GiB pages with default WB caching, which works in QEMU but hangs
// on real hardware when the MTRR does not cover 0xFEE00000 as UC: writes to
// LAPIC_EOI sink into the cache and never clear the in-service bit, so the
// first sti; hlt never wakes. Split the huge page and remap the 4 KiB LAPIC
// frame with PCD set
unsafe fn map_lapic(phys: u64) -> u64 {
    crate::mm::vmm::map_mmio_uc(phys, 0x1000);
    phys + grub::hhdm()
}

/// Initialize BSP LAPIC. Must be called after acpi::init()
pub fn init_bsp() -> Result<(), &'static str> {
    let topo_lock = crate::arch::x86_64::acpi::topology();
    let topo = topo_lock.as_ref().ok_or("acpi topology not ready")?;
    let lapic_phys = topo.lapic_phys;
    drop(topo_lock);

    disable_8259();

    let lapic_virt = unsafe { map_lapic(lapic_phys) };
    LAPIC_BASE_VIRT.store(lapic_virt, Ordering::Release);
    crate::serial_println!("[apic] lapic mmio phys={:#x} virt={:#x}", lapic_phys, lapic_virt);

// Put LAPIC in xAPIC/MMIO mode. Many modern BIOSes leave the CPU in x2APIC mode
// (MSR bit 10 set), which disables MMIO access: every later lapic_write is a
// silent no-op and no hardware interrupts arrive. From Intel SDM 10.12.5:
// x2APIC->xAPIC cannot flip directly; first globally disable (clear bit 11),
// then re-enable with bit 11=1, bit 10=0
    unsafe {
        use x86_64::registers::model_specific::Msr;
        let mut apic_base_msr = Msr::new(0x1B);
        let base = apic_base_msr.read();
        let was_x2apic = (base & (1 << 10)) != 0;
        let was_enabled = (base & (1 << 11)) != 0;
        crate::serial_println!(
            "[apic] MSR(0x1B)={:#x} x2apic={} enabled={}",
            base, was_x2apic, was_enabled
        );
        if was_x2apic {
            // Global disable, then re-enable in xAPIC mode.
            apic_base_msr.write(base & !((1 << 10) | (1 << 11)));
            let cleared = apic_base_msr.read();
            apic_base_msr.write((cleared & !(1 << 10)) | (1 << 11));
            crate::serial_println!(
                "[apic] forced x2APIC->xAPIC: MSR(0x1B)={:#x}",
                apic_base_msr.read()
            );
        } else {
            // Just make sure bit 11 is set; leave bit 10 clear.
            apic_base_msr.write((base & !(1 << 10)) | (1 << 11));
        }
    }

    unsafe {
        // Software-disable the LAPIC. BIOS or GRUB may have left an interrupt
        // latched in IRR (e.g. an error IRQ from "Send Illegal Vector"). IRR
        // cannot be cleared directly, but SVR bit 8 = 0 masks all delivery, so
        // the LVT entries can be programmed from a clean slate
        lapic_write(LAPIC_SVR, VEC_SPURIOUS as u32);

        // Drain pending ISR bits by sending EOIs. Up to 8 levels can nest;
        // extra EOIs are harmless (no-op on xAPIC with no ISR set)
        for _ in 0..16 {
            lapic_write(LAPIC_EOI, 0);
        }

        // Mask every LVT entry with a valid vector. Writing vectors 0-15 into
        // an LVT sets "Send Illegal Vector" in ESR, even masked (bit 16). Mask
        // LVT_ERROR first so errors from later writes can only go to our
        // APIC_ERR vector (masked)
        lapic_write(LAPIC_LVT_ERROR, (1 << 16) | VEC_APIC_ERR as u32);
        let mask_bits = (1 << 16) | VEC_SPURIOUS as u32;
        lapic_write(LAPIC_LVT_CMCI,    mask_bits);
        lapic_write(LAPIC_LVT_TIMER,   mask_bits);
        lapic_write(LAPIC_LVT_THERMAL, mask_bits);
        lapic_write(LAPIC_LVT_PERF,    mask_bits);
        lapic_write(LAPIC_LVT_LINT0,   mask_bits);
        lapic_write(LAPIC_LVT_LINT1,   mask_bits);

        // clear ESR (write-to-latch, second write clears)
        lapic_write(LAPIC_ESR, 0);
        lapic_write(LAPIC_ESR, 0);

        // Clear TPR, program logical destination, and re-enable the LAPIC via
        // SVR bit 8. Sources just masked stay quiet
        lapic_write(LAPIC_TPR, 0);
        lapic_write(LAPIC_DFR, 0xFFFFFFFF);
        lapic_write(LAPIC_LDR, 1 << 24);
        lapic_write(LAPIC_SVR, SVR_ENABLE | VEC_SPURIOUS as u32);
    }

    INITIALIZED.store(true, Ordering::Release);
    crate::serial_println!("[apic] bsp lapic_id={}", lapic_id());
    Ok(())
}

/// Mask every LVT entry the CPU exposes, before the first sti. If the BIOS left
/// THERMAL/PERF/CMCI unmasked with a stale vector, enabling IF would take it on
/// a vector our IDT does not handle
pub fn mask_all_lvt() {
    unsafe {
        // LVT_ERROR first, with a valid vector, so the other mask writes
        // cannot trip Send Illegal Vector and latch an error IRQ
        lapic_write(LAPIC_LVT_ERROR, (1 << 16) | VEC_APIC_ERR as u32);
        let m = (1 << 16) | VEC_SPURIOUS as u32;
        lapic_write(LAPIC_LVT_CMCI,    m);
        lapic_write(LAPIC_LVT_TIMER,   m);
        lapic_write(LAPIC_LVT_THERMAL, m);
        lapic_write(LAPIC_LVT_PERF,    m);
        lapic_write(LAPIC_LVT_LINT0,   m);
        lapic_write(LAPIC_LVT_LINT1,   m);
    }
}

/// Read the 256-bit IRR and return it as 8 32-bit words [irr0..irr7]
/// irr[i] has bits for vectors [i*32 .. i*32+31]. Used for diagnostics of
/// what interrupt is actually pending in the local APIC
pub fn read_irr() -> [u32; 8] {
    let mut out = [0u32; 8];
    for i in 0..8 {
        unsafe {
            out[i] = lapic_read(LAPIC_IRR_BASE + (i as u32) * 0x10);
        }
    }
    out
}

/// AP-side LAPIC init. Called by each AP after jumping into long mode
pub fn init_ap() {
    unsafe {
        use x86_64::registers::model_specific::Msr;
        let mut apic_base_msr = Msr::new(0x1B);
        let mut base = apic_base_msr.read();
        base |= 1 << 11;
        apic_base_msr.write(base);

        lapic_write(LAPIC_TPR, 0);
        lapic_write(LAPIC_DFR, 0xFFFFFFFF);
        let id = lapic_id();
        lapic_write(LAPIC_LDR, (1u32 << (id & 7)) << 24);
        lapic_write(LAPIC_SVR, SVR_ENABLE | VEC_SPURIOUS as u32);
        // Mask LVT_ERROR FIRST with a valid vector so subsequent masked LVT
        // writes cannot trigger Send Illegal Vector in ESR
        lapic_write(LAPIC_LVT_ERROR, (1 << 16) | VEC_APIC_ERR as u32);
        let m = (1 << 16) | VEC_SPURIOUS as u32;
        lapic_write(LAPIC_LVT_CMCI,    m);
        lapic_write(LAPIC_LVT_TIMER,   m);
        lapic_write(LAPIC_LVT_THERMAL, m);
        lapic_write(LAPIC_LVT_PERF,    m);
        lapic_write(LAPIC_LVT_LINT0,   m);
        lapic_write(LAPIC_LVT_LINT1,   m);
        lapic_write(LAPIC_ESR, 0);
        lapic_write(LAPIC_ESR, 0);
        // Now unmask ERROR so AP can report its own APIC errors
        lapic_write(LAPIC_LVT_ERROR, VEC_APIC_ERR as u32);
    }
}

// IPI helpers //

fn wait_ipi() {
    // Poll ICR_LO delivery status bit until 0
    for _ in 0..10_000 {
        unsafe {
            if lapic_read(LAPIC_ICR_LO) & ICR_PENDING == 0 { return; }
        }
        core::hint::spin_loop();
    }
}

pub fn send_ipi(dest_lapic_id: u32, vector: u8) {
    unsafe {
        lapic_write(LAPIC_ICR_HI, dest_lapic_id << 24);
        lapic_write(LAPIC_ICR_LO, ICR_DELIVERY_FIXED | ICR_LEVEL_ASSERT | vector as u32);
    }
    wait_ipi();
}

pub fn send_ipi_all_but_self(vector: u8) {
    unsafe {
        lapic_write(LAPIC_ICR_HI, 0);
        lapic_write(LAPIC_ICR_LO,
            ICR_DELIVERY_FIXED | ICR_LEVEL_ASSERT | ICR_DEST_ALL_BUT_SELF | vector as u32);
    }
    wait_ipi();
}

/// Request reschedule on all other CPUs
pub fn broadcast_reschedule() {
    if is_initialized() {
        send_ipi_all_but_self(VEC_IPI_RESCHED);
    }
}

/// Halt all other CPUs (used on panic)
pub fn broadcast_halt() {
    if is_initialized() {
        send_ipi_all_but_self(VEC_IPI_HALT);
    }
}

/// Milliseconds to wait between INIT and the first SIPI.
///
/// The MP spec's figure, a property of the INIT pulse: one wait covers every AP
/// sent INIT inside it. `start_aps` INITs every AP, waits once, then does the
/// SIPI handshake one at a time, since all APs read their stack and entry point
/// from one parameter block at the bottom of the trampoline
pub const INIT_TO_SIPI_MS: u64 = 10;

/// Put one AP into wait-for-SIPI. Does not wait afterwards: the caller owes
/// every AP it INITs a single `INIT_TO_SIPI_MS` delay before any SIPI
pub fn send_init(lapic_id: u32) {
    unsafe {
        // Clear error register
        lapic_write(LAPIC_ESR, 0);

        // Send INIT (assert)
        lapic_write(LAPIC_ICR_HI, lapic_id << 24);
        lapic_write(LAPIC_ICR_LO, ICR_DELIVERY_INIT | ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL);
        wait_ipi();

        // Send INIT de-assert
        lapic_write(LAPIC_ICR_HI, lapic_id << 24);
        lapic_write(LAPIC_ICR_LO, ICR_DELIVERY_INIT | ICR_TRIGGER_LEVEL);
        wait_ipi();
    }
}

/// Start an AP already past INIT and the delay. Two SIPIs, as the spec's
/// algorithm asks: the first is lost on some parts
pub fn send_sipi(lapic_id: u32, start_page_phys: u32) {
    let sipi_vec = ((start_page_phys >> 12) & 0xFF) as u32;
    for _ in 0..2 {
        unsafe {
            lapic_write(LAPIC_ESR, 0);
            lapic_write(LAPIC_ICR_HI, lapic_id << 24);
            lapic_write(LAPIC_ICR_LO, ICR_DELIVERY_STARTUP | ICR_LEVEL_ASSERT | sipi_vec);
            wait_ipi();
        }
        crate::kcore::time::udelay(200);
    }
}

// LAPIC timer //

// Calibrate LAPIC timer against PIT or TSC. We use a fixed divide-by-16 and
// measure how many ticks elapse in a known window
pub fn init_timer(hz: u32) {
    // Realistic LAPIC bus frequencies are 100-400 MHz; with divide-by-16 that
    // is 6k-25k ticks per ms. Accept anything from 1k to 50M ticks/ms. A value
    // outside means calibration misfired (PIT stuck, SMI storm):
    // LAPIC_INIT_CNT=1 causes an IRQ storm as soon as 'sti' runs and hangs the
    // CPU
    const MIN_TICKS_PER_MS: u32 = 1_000;
    const MAX_TICKS_PER_MS: u32 = 50_000_000;
    // Fallback assumes a 320 MHz LAPIC bus (shared with FSB / crystal on most
    // desktops); at /16 that is 20k ticks/ms. Wrong by up to a factor of 4
    // either way, never catastrophic
    const FALLBACK_TICKS_PER_MS: u32 = 20_000;

    // Two 10ms windows. An SMI landing inside one stops our detection of OUT
    // going high but not the LAPIC counter, so an interrupted window only
    // over-counts, and the smaller of two is the cleaner
    const CAL_MS: u32 = 10;
    let best = match (calibrate_with_pit(CAL_MS), calibrate_with_pit(CAL_MS)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    let ticks_per_ms = match best {
        Some(v) if v >= MIN_TICKS_PER_MS && v <= MAX_TICKS_PER_MS => v,
        Some(v) => {
            crate::serial_println!(
                "[apic] warn: PIT calibration returned {} ticks/ms (out of range [{}..{}]), using fallback {}",
                v, MIN_TICKS_PER_MS, MAX_TICKS_PER_MS, FALLBACK_TICKS_PER_MS
            );
            FALLBACK_TICKS_PER_MS
        }
        None => {
            crate::serial_println!(
                "[apic] warn: PIT calibration failed, using fallback {} ticks/ms",
                FALLBACK_TICKS_PER_MS
            );
            FALLBACK_TICKS_PER_MS
        }
    };

    let mut ticks_per_hz = ((ticks_per_ms as u64 * 1000) / hz as u64) as u32;
    // Final floor against pathological hz arguments. 10_000 at 100MHz/16 bus is
    // 1.6ms, 160x slower than the tightest safe interrupt rate
    if ticks_per_hz < 10_000 {
        ticks_per_hz = 10_000;
    }

    crate::serial_println!(
        "[apic] timer calibrated: {} lapic ticks/ms -> {} per hz @ {}Hz",
        ticks_per_ms, ticks_per_hz, hz
    );
    // Also on the framebuffer: on a machine without serial this is the only way
    // to see the calibration landed in a safe range before sti
    crate::cprintln!(180, 180, 180,
        "[apic] calibrated: {} ticks/ms, init_cnt = {}",
        ticks_per_ms, ticks_per_hz);

    LAPIC_TIMER_HZ.store(hz, Ordering::Relaxed);
    LAPIC_TICKS_PER_HZ.store(ticks_per_hz, Ordering::Release);
    LAPIC_TICKS_PER_MS.store(ticks_per_ms, Ordering::Release);
    TICK_PERIOD_NS.store(1_000_000_000 / hz.max(1) as u64, Ordering::Release);

    // How the timer is driven from here on, for every CPU. A machine with no
    // calibrated TSC has no clock for a deadline and keeps the 250 Hz
    // interrupt; so does one booted with `tickonly`
    let mode = if !crate::kcore::time::precise() || crate::arch::x86_64::grub::cmdline_flag("tickonly") {
        TimerMode::Periodic
    } else if has_tsc_deadline() {
        TimerMode::Deadline
    } else {
        TimerMode::OneShot
    };
    TIMER_MODE.store(mode as u8, Ordering::Release);
    crate::serial_println!("[apic] timer: {}", match mode {
        TimerMode::Periodic => "periodic, 250 Hz (no precise deadlines)",
        TimerMode::OneShot => "one-shot, armed for the next tick or the soonest sleeper",
        TimerMode::Deadline => "TSC-deadline, armed for the next tick or the soonest sleeper",
    });
    start_this_cpu(ticks_per_hz);
}

/// Start LAPIC timer on AP using the same calibration
pub fn start_ap_timer(ticks_per_hz: u32) {
    start_this_cpu(ticks_per_hz);
}

/// The calling CPU's timer again, as `init_timer` set it up, for a boot that
/// masked every LVT entry in between
pub fn restart_timer() {
    start_this_cpu(LAPIC_TICKS_PER_HZ.load(Ordering::Acquire).max(10_000));
}

/// The calling CPU's timer, in the mode `init_timer` chose
fn start_this_cpu(ticks_per_hz: u32) {
    let mode = timer_mode();
    unsafe {
        lapic_write(LAPIC_DIV_CONF, 0x3);  // divide by 16
        match mode {
            TimerMode::Periodic => {
                lapic_write(LAPIC_LVT_TIMER, (1 << 17) | VEC_TIMER as u32);
                lapic_write(LAPIC_INIT_CNT, ticks_per_hz.max(1));
            }
            TimerMode::OneShot => {
                lapic_write(LAPIC_LVT_TIMER, VEC_TIMER as u32);
            }
            TimerMode::Deadline => {
                lapic_write(LAPIC_LVT_TIMER, (2 << 17) | VEC_TIMER as u32);
                // The LVT write has to land before the deadline MSR is
                // written, or the MSR write is ignored (SDM 10.5.4.1)
                core::arch::asm!("mfence", options(nostack, preserves_flags));
            }
        }
    }
    if mode != TimerMode::Periodic {
        let cpu = crate::arch::x86_64::percpu::current_index();
        let now = crate::kcore::time::now_ns();
        NEXT_TICK_NS[cpu].store(now + TICK_PERIOD_NS.load(Ordering::Relaxed), Ordering::Relaxed);
        arm_timer(cpu);
    }
}

/// How a CPU's timer interrupt is made
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TimerMode {
    /// Every 4 ms whatever happens: the only mode without a calibrated TSC
    Periodic = 0,
    /// Counted down from a number the LAPIC's own clock divides
    OneShot = 1,
    /// When the TSC reaches a value: no counting, no rounding
    Deadline = 2,
}

static TIMER_MODE: AtomicU8 = AtomicU8::new(TimerMode::Periodic as u8);
static LAPIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(0);
static TICK_PERIOD_NS: AtomicU64 = AtomicU64::new(4_000_000);
/// When each CPU's next tick is due, on `now_ns`'s clock
static NEXT_TICK_NS: [AtomicU64; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::percpu::MAX_CPUS];

/// Timer interrupts each CPU has taken that were its tick, and that were only a
/// sleeper's deadline. `ipc test` reads them to tell a deadline that fired from
/// one that waited for the tick
static TICK_IRQS: [AtomicU64; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::percpu::MAX_CPUS];
static DEADLINE_IRQS: [AtomicU64; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::percpu::MAX_CPUS];

/// Times a CPU's timer was armed for a sleeper sooner than its next tick
static EARLY_ARMS: [AtomicU64; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::percpu::MAX_CPUS];

/// `(ticks, deadlines, early arms)` of every CPU together
pub fn timer_counts() -> (u64, u64, u64) {
    let sum = |a: &[AtomicU64]| a.iter().map(|c| c.load(Ordering::Relaxed)).sum();
    (sum(&TICK_IRQS), sum(&DEADLINE_IRQS), sum(&EARLY_ARMS))
}

/// A timer interrupt this close to the tick is the tick: one-shot counts on a
/// calibration, and a slightly short count should not cause a second interrupt
/// microseconds later
const TICK_SLACK_NS: u64 = 50_000;

const IA32_TSC_DEADLINE: u32 = 0x6E0;

pub fn timer_mode() -> TimerMode {
    match TIMER_MODE.load(Ordering::Acquire) {
        1 => TimerMode::OneShot,
        2 => TimerMode::Deadline,
        _ => TimerMode::Periodic,
    }
}

fn has_tsc_deadline() -> bool {
    // CPUID.01H:ECX[24]
    let r = core::arch::x86_64::__cpuid(1);
    r.ecx & (1 << 24) != 0
}

/// Whether the timer interrupt just arrived on `cpu` at `now` is its tick,
/// moving the next tick on when it is. In periodic mode every interrupt is a
/// tick; otherwise one can be a sleeper's deadline, which charges nobody and
/// does not move the clock
pub fn timer_due(cpu: usize, now: u64) -> bool {
    if timer_mode() == TimerMode::Periodic {
        TICK_IRQS[cpu].fetch_add(1, Ordering::Relaxed);
        return true;
    }
    let next = NEXT_TICK_NS[cpu].load(Ordering::Relaxed);
    if now + TICK_SLACK_NS < next {
        DEADLINE_IRQS[cpu].fetch_add(1, Ordering::Relaxed);
        return false;
    }
    TICK_IRQS[cpu].fetch_add(1, Ordering::Relaxed);
    let period = TICK_PERIOD_NS.load(Ordering::Relaxed);
    // A tick missed while interrupts were off is gone: the next one is a period
    // from now, not a burst
    let mut n = next + period;
    if n <= now { n = now + period; }
    NEXT_TICK_NS[cpu].store(n, Ordering::Relaxed);
    true
}

/// When `cpu`'s next tick is due, on `now_ns`'s clock. A tick-based sleep is
/// measured from this (`sched::sleep`)
pub fn next_tick_ns(cpu: usize) -> u64 {
    NEXT_TICK_NS[cpu].load(Ordering::Relaxed)
}

/// Arm the calling CPU's timer for its next tick or its soonest sleeper,
/// whichever comes first. Runs after every timer interrupt and whenever an
/// earlier sleeper goes on this CPU (`sched::timer::link`). Nothing to do in
/// periodic mode
pub fn arm_timer(cpu: usize) {
    let mode = timer_mode();
    if mode == TimerMode::Periodic {
        return;
    }
    let mut at = NEXT_TICK_NS[cpu].load(Ordering::Relaxed);
    if let Some(s) = crate::sched::timer::soonest(cpu) {
        if s < at {
            EARLY_ARMS[cpu].fetch_add(1, Ordering::Relaxed);
        }
        at = at.min(s);
    }
    match mode {
        TimerMode::Deadline => unsafe {
            x86_64::registers::model_specific::Msr::new(IA32_TSC_DEADLINE)
                .write(crate::kcore::time::ns_to_tsc(at).max(1));
        },
        TimerMode::OneShot => {
            let now = crate::kcore::time::now_ns();
            let delta = at.saturating_sub(now).max(1_000);
            let per_ms = LAPIC_TICKS_PER_MS.load(Ordering::Relaxed).max(1) as u64;
            let count = (delta.saturating_mul(per_ms) / 1_000_000).clamp(1, u32::MAX as u64) as u32;
            unsafe { lapic_write(LAPIC_INIT_CNT, count) };
        }
        TimerMode::Periodic => {}
    }
}

pub fn bsp_ticks_per_hz() -> u32 {
    LAPIC_TICKS_PER_HZ.load(Ordering::Acquire)
}

pub fn timer_hz() -> u32 {
    LAPIC_TIMER_HZ.load(Ordering::Relaxed)
}

// Calibrate the LAPIC timer against PIT channel 2. Returns
// ticks-per-millisecond, or None when the result cannot be trusted. The PIT
// one-shot lives in `kcore::time`
fn calibrate_with_pit(ms: u32) -> Option<u32> {
    let elapsed = crate::kcore::time::pit_one_shot(
        ms,
        || unsafe {
            lapic_write(LAPIC_DIV_CONF, 0x3);
            // Masked, with a valid vector: vector 0 here latches a Send Illegal
            // Vector error in ESR on real hardware
            lapic_write(LAPIC_LVT_TIMER, (1 << 16) | VEC_SPURIOUS as u32);
            lapic_write(LAPIC_INIT_CNT, u32::MAX);
        },
        || unsafe { u32::MAX - lapic_read(LAPIC_CUR_CNT) },
    );
    unsafe { lapic_write(LAPIC_INIT_CNT, 0); }

    let elapsed = elapsed?;
    if elapsed == 0 || elapsed == u32::MAX {
        return None;
    }
    Some(elapsed / ms)
}

// IO-APIC //

pub struct IoApic {
    base_virt: u64,
    gsi_base:  u32,
    max_gsi:   u32,
}

static IOAPICS: Mutex<alloc::vec::Vec<IoApic>> = Mutex::new(alloc::vec::Vec::new());

impl IoApic {
    unsafe fn read(&self, reg: u32) -> u32 {
        core::ptr::write_volatile(self.base_virt as *mut u32, reg);
        core::ptr::read_volatile((self.base_virt + 0x10) as *const u32)
    }
    unsafe fn write(&self, reg: u32, val: u32) {
        core::ptr::write_volatile(self.base_virt as *mut u32, reg);
        core::ptr::write_volatile((self.base_virt + 0x10) as *mut u32, val);
    }

    fn mask_all(&self) {
        for i in 0..=self.max_gsi {
            let reg = 0x10 + 2 * i;
            unsafe {
                let lo = self.read(reg);
                self.write(reg, lo | (1 << 16));
            }
        }
    }
}

pub fn ioapic_init() -> Result<(), &'static str> {
    let topo_lock = crate::arch::x86_64::acpi::topology();
    let topo = topo_lock.as_ref().ok_or("acpi not ready")?;
    let hhdm = grub::hhdm();
    let mut v = IOAPICS.lock();
    v.clear();
    for info in &topo.ioapics {
        // As with the LAPIC: IOAPIC MMIO must be UC on real hardware or
        // IOREDTBL writes never commit, leaving legacy IRQs unrouted
        crate::mm::vmm::map_mmio_uc(info.addr, 0x1000);
        let base_virt = info.addr + hhdm;
        let ioa = IoApic { base_virt, gsi_base: info.gsi_base, max_gsi: 0 };
        unsafe {
            let ver = ioa.read(1);
            let max = (ver >> 16) & 0xFF;
            let ioa = IoApic { base_virt, gsi_base: info.gsi_base, max_gsi: info.gsi_base + max };
            ioa.mask_all();
            crate::serial_println!("[apic] ioapic id={} ver={:#x} entries={} gsi=[{}..{}]",
                info.id, ver & 0xFF, max + 1, info.gsi_base, info.gsi_base + max);
            v.push(ioa);
        }
    }
    Ok(())
}

fn find_ioapic_locked<'a>(list: &'a [IoApic], gsi: u32) -> Option<&'a IoApic> {
    list.iter().find(|ia| gsi >= ia.gsi_base && gsi <= ia.max_gsi)
}

/// Route legacy ISA 'irq' to 'vector', delivered to LAPIC 'dest_id'
pub fn set_irq(irq: u8, vector: u8, dest_lapic_id: u32) -> Result<(), &'static str> {
    let (gsi, flags) = {
        let topo_lock = crate::arch::x86_64::acpi::topology();
        let topo = topo_lock.as_ref().ok_or("acpi not ready")?;
        topo.irq_to_gsi(irq)
    };

    let v = IOAPICS.lock();
    let ioa = find_ioapic_locked(&v, gsi).ok_or("no ioapic owns gsi")?;
    let entry = 0x10 + 2 * (gsi - ioa.gsi_base);

    // Polarity: bit 13 (1 = low active); Trigger: bit 15 (1 = level)
    let polarity = (flags & 0b11) == 0b11;
    let trigger  = ((flags >> 2) & 0b11) == 0b11;

    let mut lo: u32 = vector as u32;
    if polarity { lo |= 1 << 13; }
    if trigger  { lo |= 1 << 15; }
    let hi: u32 = dest_lapic_id << 24;

    unsafe {
        ioa.write(entry + 1, hi);
        ioa.write(entry, lo);  // unmask by NOT setting bit 16
    }
    crate::trace_println!("[apic] irq{} -> gsi{} vec={:#x} dest=lapic{}", irq, gsi, vector, dest_lapic_id);
    Ok(())
}

// MSI vector pool
//
// Reserves a fixed range of IDT vectors for Message Signaled Interrupts.
// Drivers call alloc_msi_vector for an (address, data) pair to write into a PCI
// MSI capability. The matching IDT stubs live in interrupts.rs and forward to
// msi_dispatch(slot)
pub const VEC_MSI_BASE:  u8    = 0x70;
pub const VEC_MSI_COUNT: usize = 16;

#[derive(Copy, Clone, Debug)]
pub struct MsiTarget {
    pub address: u64, // MSI message address (write into MSI_ADDR_LO/HI)
    pub data:    u32, // MSI message data    (write into MSI_DATA)
    pub vector:  u8,
    pub slot:    u8,  // index into the MSI handler table
}

static MSI_USED: [AtomicBool; VEC_MSI_COUNT] = [const { AtomicBool::new(false) }; VEC_MSI_COUNT];
// Installed from thread context (alloc_msi_vector) and read from the MSI IDT
// stubs in interrupt context: a plain spin::Mutex here deadlocks the CPU if the
// vector fires while its own handler slot is being written
static MSI_HANDLERS: [crate::kcore::irq_lock::IrqMutex<Option<(fn(usize), usize)>>; VEC_MSI_COUNT] =
    [const { crate::kcore::irq_lock::IrqMutex::new(None) }; VEC_MSI_COUNT];

/// Allocate an MSI vector and install a handler. Returns the programming triple
/// for the device's MSI capability. The handler runs in interrupt context with
/// `arg`; keep it short and non-blocking. `arg` tells apart a device's sources,
/// such as an NVMe controller's completion queues
pub fn alloc_msi_vector(lapic_id: u32, handler: fn(usize), arg: usize) -> Option<MsiTarget> {
    for slot in 0..VEC_MSI_COUNT {
        if MSI_USED[slot].compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            *MSI_HANDLERS[slot].lock() = Some((handler, arg));
            let vector = VEC_MSI_BASE + slot as u8;
            // x86 MSI message format (fixed delivery, edge-triggered,
            // physical):
            //   address = 0xFEE0_0000 | (dest << 12)
            //   data    = vector
            let address = 0xFEE0_0000u64 | ((lapic_id as u64) << 12);
            let data = vector as u32;
            return Some(MsiTarget { address, data, vector, slot: slot as u8 });
        }
    }
    None
}

pub fn free_msi_vector(target: MsiTarget) {
    let slot = target.slot as usize;
    if slot >= VEC_MSI_COUNT { return; }
    *MSI_HANDLERS[slot].lock() = None;
    MSI_USED[slot].store(false, Ordering::Release);
}

/// Called from the MSI IDT stubs. Looks up the installed handler
pub fn msi_dispatch(slot: usize) {
    if slot >= VEC_MSI_COUNT { return; }
    let h = *MSI_HANDLERS[slot].lock();
    if let Some((f, arg)) = h { f(arg); }
}

pub fn mask_irq(irq: u8) {
    let gsi = {
        let topo_lock = crate::arch::x86_64::acpi::topology();
        match topo_lock.as_ref() {
            Some(t) => t.irq_to_gsi(irq).0,
            None => return,
        }
    };
    let v = IOAPICS.lock();
    if let Some(ioa) = find_ioapic_locked(&v, gsi) {
        let entry = 0x10 + 2 * (gsi - ioa.gsi_base);
        unsafe {
            let lo = ioa.read(entry);
            ioa.write(entry, lo | (1 << 16));
        }
    }
}
