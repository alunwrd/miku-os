// Local APIC together with IO-APIC drivers replacing 8259 PIC

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::PageTableFlags;

use crate::grub;

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

/////////////////////////////////////////////////////////////////////////////////////////////
// Map LAPIC MMIO into kernel's HHDM region with uncacheable flags. The boot               //
// HHDM mapping uses 1-GiB pages with default WB caching, which works in QEMU              //
// (ignores cache attrs for MMIO) but hangs on real hardware when the MTRR                 //
// does not cover 0xFEE00000 as UC - writes to LAPIC_EOI sink into the cache               //
// and never clear the in-service bit, so the first sti; hlt never wakes                   //
// Split the huge page and remap the 4 KiB LAPIC frame with PCD set                        //
/////////////////////////////////////////////////////////////////////////////////////////////
unsafe fn map_lapic(phys: u64) -> u64 {
    crate::vmm::map_mmio_uc(phys, 0x1000);
    phys + grub::hhdm()
}

/// Initialize BSP LAPIC. Must be called after acpi::init()
pub fn init_bsp() -> Result<(), &'static str> {
    let topo_lock = crate::acpi::topology();
    let topo = topo_lock.as_ref().ok_or("acpi topology not ready")?;
    let lapic_phys = topo.lapic_phys;
    drop(topo_lock);

    disable_8259();

    let lapic_virt = unsafe { map_lapic(lapic_phys) };
    LAPIC_BASE_VIRT.store(lapic_virt, Ordering::Release);
    crate::serial_println!("[apic] lapic mmio phys={:#x} virt={:#x}", lapic_phys, lapic_virt);

////////////////////////////////////////////////////////////////////////////////
// Put LAPIC in xAPIC/MMIO mode. Many modern BIOSes leave the CPU in          //
// x2APIC mode (MSR bit 10 set), which disables MMIO access entirely -        //
// every subsequent lapic_write silently no-ops and the CPU never             //
// receives hardware-delivered interrupts. Transition rules from Intel        //
// SDM 10.12.5: cannot flip x2APIC->xAPIC directly; must first globally       //
// disable (clear bit 11), then re-enable with bit 11=1, bit 10=0             //
////////////////////////////////////////////////////////////////////////////////
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
        // software-disable the LAPIC. On real hardware the BIOS or
        // GRUB may have left an interrupt latched in IRR (e.g. an error IRQ
        // triggered by a "Send Illegal Vector" from an earlier write). We
        // cannot clear IRR directly, but software-disabling the LAPIC via
        // SVR bit 8 = 0 masks all interrupt delivery and lets us program
        // the LVT entries from a clean slate before we re-enable
        lapic_write(LAPIC_SVR, VEC_SPURIOUS as u32);

        // drain any pending ISR bits by sending EOIs. Up to 8 levels
        // can be nested; sending more than that is harmless (EOI with no ISR
        // set is a no-op on xAPIC)
        for _ in 0..16 {
            lapic_write(LAPIC_EOI, 0);
        }

        // mask every LVT entry with a VALID vector. Writing vectors
        // 0-15 into an LVT sets "Send Illegal Vector" in ESR; masking alone
        // (bit 16) with vector=0 falls into that trap. Mask LVT_ERROR FIRST
        // so any error generated by the subsequent writes can only be
        // delivered to our allocated APIC_ERR vector (masked)
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

        // clear TPR, program logical destination, and re-enable the
        // LAPIC via SVR bit 8. From here the LAPIC accepts new interrupts
        // but all sources we just masked will stay quiet
        lapic_write(LAPIC_TPR, 0);
        lapic_write(LAPIC_DFR, 0xFFFFFFFF);
        lapic_write(LAPIC_LDR, 1 << 24);
        lapic_write(LAPIC_SVR, SVR_ENABLE | VEC_SPURIOUS as u32);
    }

    INITIALIZED.store(true, Ordering::Release);
    crate::serial_println!("[apic] bsp lapic_id={}", lapic_id());
    Ok(())
}

/// Mask every LVT entry the CPU exposes. Used as a defensive step before the
/// first sti - if the BIOS left THERMAL/PERF/CMCI unmasked with a stale
/// vector, the moment we enable IF the CPU will take that interrupt and jump
/// somewhere our IDT does not handle. Writing bit 16 to each LVT slot forces
/// a clean slate
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

/// TLB shootdown for all other CPUs
pub fn broadcast_tlb_flush() {
    if is_initialized() {
        send_ipi_all_but_self(VEC_IPI_TLB);
    }
}

/// Halt all other CPUs (used on panic)
pub fn broadcast_halt() {
    if is_initialized() {
        send_ipi_all_but_self(VEC_IPI_HALT);
    }
}

/// Send INIT, wait, then two SIPIs to an AP's LAPIC
/// 'start_page' is the physical page (4K aligned) for the AP entry vector in real mode
pub fn send_init_sipi(lapic_id: u32, start_page_phys: u32) {
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

    crate::timing::udelay(10_000);

    let sipi_vec = ((start_page_phys >> 12) & 0xFF) as u32;
    for _ in 0..2 {
        unsafe {
            lapic_write(LAPIC_ESR, 0);
            lapic_write(LAPIC_ICR_HI, lapic_id << 24);
            lapic_write(LAPIC_ICR_LO, ICR_DELIVERY_STARTUP | ICR_LEVEL_ASSERT | sipi_vec);
            wait_ipi();
        }
        crate::timing::udelay(200);
    }
}

// LAPIC timer //

// Calibrate LAPIC timer against PIT or TSC. We use a fixed divide-by-16 and measure how many ticks elapse in a known window
pub fn init_timer(hz: u32) {
    // Realistic LAPIC bus frequencies are 100-400 MHz; with divide-by-16 that
    // is 6k-25k ticks per ms. Accept anything from 1k to 50M ticks/ms. A value
    // outside that range means calibration misfired (PIT stuck, SMI storm) and
    // we must not believe it: programming LAPIC_INIT_CNT=1 triggers an IRQ
    // storm the moment 'sti' runs and hangs the CPU before the next
    // boot_step can ever print
    const MIN_TICKS_PER_MS: u32 = 1_000;
    const MAX_TICKS_PER_MS: u32 = 50_000_000;
    // Fallback assumes a 320 MHz LAPIC bus (shared with FSB / crystal on most
    // desktops); at /16 that is 20k ticks/ms. Wrong by up to a factor of 4
    // in either direction, but never catastrophic
    const FALLBACK_TICKS_PER_MS: u32 = 20_000;

    let calibrate_ms = 50u32;
    let ticks_per_ms = match calibrate_with_pit(calibrate_ms) {
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
    // Final safety floor against pathological hz arguments. 10_000 at 100MHz/16
    // bus is 1.6ms, 160x slower than the tightest safe interrupt rate
    if ticks_per_hz < 10_000 {
        ticks_per_hz = 10_000;
    }

    crate::serial_println!(
        "[apic] timer calibrated: {} lapic ticks/ms -> {} per hz @ {}Hz",
        ticks_per_ms, ticks_per_hz, hz
    );
    // Also surface the calibrated values on the framebuffer - on machines
    // without working serial this is the only window we have to confirm the
    // calibration landed in a safe range before sti
    crate::cprintln!(180, 180, 180,
        "[apic] calibrated: {} ticks/ms, init_cnt = {}",
        ticks_per_ms, ticks_per_hz);

    LAPIC_TIMER_HZ.store(hz, Ordering::Relaxed);
    LAPIC_TICKS_PER_HZ.store(ticks_per_hz, Ordering::Release);

    unsafe {
        lapic_write(LAPIC_DIV_CONF, 0x3);  // divide by 16
        // Periodic timer, vector = VEC_TIMER
        lapic_write(LAPIC_LVT_TIMER, (1 << 17) | VEC_TIMER as u32);
        lapic_write(LAPIC_INIT_CNT, ticks_per_hz);
    }
}

/// Start LAPIC timer on AP using the same calibration
pub fn start_ap_timer(ticks_per_hz: u32) {
    unsafe {
        lapic_write(LAPIC_DIV_CONF, 0x3);
        lapic_write(LAPIC_LVT_TIMER, (1 << 17) | VEC_TIMER as u32);
        lapic_write(LAPIC_INIT_CNT, ticks_per_hz.max(1));
    }
}

pub fn bsp_ticks_per_hz() -> u32 {
    LAPIC_TICKS_PER_HZ.load(Ordering::Acquire)
}

pub fn timer_hz() -> u32 {
    LAPIC_TIMER_HZ.load(Ordering::Relaxed)
}

/////////////////////////////////////////////////////////////////////////////////////////////
// Calibrate LAPIC timer against PIT channel 2. Returns ticks-per-millisecond              //
// on success. Returns None if the PIT is unresponsive or produces a result                //
// the caller cannot trust (zero elapsed, full u32::MAX elapsed, or timeout).              //
//                                                                                         //
// Mode-0 PIT programming: writing the control word forces OUT low, counter                //
// loads N on the first CLK after the LSB+MSB are written, then decrements                 //
// while GATE is high. OUT goes high after N+1 clocks. We close the GATE                   //
// before programming so we start from a clean state regardless of whatever                //
// the BIOS left the chip in                                                               //
/////////////////////////////////////////////////////////////////////////////////////////////
fn calibrate_with_pit(ms: u32) -> Option<u32> {
    const PIT_FREQ: u32 = 1_193_182;
    let count = ((PIT_FREQ as u64 * ms as u64) / 1000) as u16;
    if count == 0 { return None; }

    unsafe {
        let mut gate: Port<u8> = Port::new(0x61);
        let gate_v = gate.read();
        // Close gate bit 0 and disable speaker bit 1 so the programming
        // sequence below starts from a known state
        gate.write(gate_v & !0x03);

        let mut cmd: Port<u8> = Port::new(0x43);
        cmd.write(0xB0);  // channel 2, access LSB+MSB, mode 0, binary

        let mut ch2: Port<u8> = Port::new(0x42);
        ch2.write(count as u8);
        ch2.write((count >> 8) as u8);

        // After mode-0 programming, OUT must be LOW. If bit 5 of port 0x61
        // is still high here, the PIT is either missing, emulated badly, or
        // being kept high by an SMM hook - none of which we can calibrate
        // against
        if gate.read() & 0x20 != 0 {
            gate.write(gate_v);
            return None;
        }

        lapic_write(LAPIC_DIV_CONF, 0x3);
        // Masked, but write a valid vector - writing vector 0 here would
        // latch a Send Illegal Vector error in ESR on real hardware
        lapic_write(LAPIC_LVT_TIMER, (1 << 16) | VEC_SPURIOUS as u32);
        lapic_write(LAPIC_INIT_CNT, u32::MAX);

        // Open the gate - PIT begins counting down
        gate.write((gate_v & !0x02) | 0x01);

        // Wait for OUT to go high with a hard ceiling. At 1.19 MHz and
        // ms=50 the expected wait is 60k PIT ticks (50ms). Cap the spin
        // at 500ms so broken hardware cannot hang boot here
        let mut timeout: u32 = 50_000_000;
        loop {
            if gate.read() & 0x20 != 0 { break; }
            timeout -= 1;
            if timeout == 0 {
                lapic_write(LAPIC_INIT_CNT, 0);
                gate.write(gate_v);
                return None;
            }
            core::hint::spin_loop();
        }

        let end = lapic_read(LAPIC_CUR_CNT);
        lapic_write(LAPIC_INIT_CNT, 0);
        gate.write(gate_v);

        let elapsed = u32::MAX - end;
        if elapsed == 0 || elapsed == u32::MAX {
            return None;
        }
        Some(elapsed / ms)
    }
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
    let topo_lock = crate::acpi::topology();
    let topo = topo_lock.as_ref().ok_or("acpi not ready")?;
    let hhdm = grub::hhdm();
    let mut v = IOAPICS.lock();
    v.clear();
    for info in &topo.ioapics {
        // Same reason as LAPIC: IOAPIC MMIO must be UC on real hardware or
        // IOREDTBL writes never commit, leaving legacy IRQs unrouted
        crate::vmm::map_mmio_uc(info.addr, 0x1000);
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
        let topo_lock = crate::acpi::topology();
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
    crate::serial_println!("[apic] irq{} -> gsi{} vec={:#x} dest=lapic{}", irq, gsi, vector, dest_lapic_id);
    Ok(())
}

/////////////////////////////////////////////////////////////////////////////////////
//                           MSI vector pool                                       //
//                                                                                 //
// Reserves a fixed range of IDT vectors for Message Signaled Interrupts.          //
// Drivers call alloc_msi_vector to obtain an (address, data) pair suitable        //
// for writing into a PCI MSI capability. The matching IDT stubs live in           //
// interrupts.rs and forward to msi_dispatch(slot)                                 //
/////////////////////////////////////////////////////////////////////////////////////
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
static MSI_HANDLERS: [Mutex<Option<fn()>>; VEC_MSI_COUNT] =
    [const { Mutex::new(None) }; VEC_MSI_COUNT];

/// Allocate an MSI vector and install a handler. Returns the programming
/// triple for the device's MSI capability. The handler is invoked in
/// interrupt context; keep it short and avoid blocking
pub fn alloc_msi_vector(lapic_id: u32, handler: fn()) -> Option<MsiTarget> {
    for slot in 0..VEC_MSI_COUNT {
        if MSI_USED[slot].compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            *MSI_HANDLERS[slot].lock() = Some(handler);
            let vector = VEC_MSI_BASE + slot as u8;
            // x86 MSI message format (fixed delivery, edge-triggered, physical):
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
    if let Some(f) = h { f(); }
}

pub fn mask_irq(irq: u8) {
    let gsi = {
        let topo_lock = crate::acpi::topology();
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
