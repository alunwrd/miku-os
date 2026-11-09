use core::sync::atomic::{AtomicU64, Ordering};

static TSC_KHZ: AtomicU64 = AtomicU64::new(0);

/// TSC at the moment the clock became readable, so anything later can ask
/// how long the machine has been up without threading a start value through
static BOOT_TSC: AtomicU64 = AtomicU64::new(0);

/// PIT input clock. 1.193182 MHz on every PC
const PIT_FREQ: u32 = 1_193_182;

/// Time a one-shot of `ms` milliseconds on PIT channel 2.
///
/// Channel 2 is the one PIT channel with no interrupt attached: its gate is
/// software-controlled through port 0x61 bit 0 and its output readable in bit
/// 5, so an interval of known length can be timed with two reads of a counter.
/// It needs no interrupts, no allocator and no LAPIC.
///
/// `start` runs with the gate closed, immediately before it opens; `end` runs
/// when OUT goes high. Returns `end`'s value, or `None` if the PIT did not
/// respond: missing, badly emulated, or held by an SMM hook.
///
/// Callers: the TSC calibration below and `apic::init_timer`, which counts
/// LAPIC ticks over the same window
pub fn pit_one_shot<T>(ms: u32, start: impl FnOnce(), end: impl FnOnce() -> T) -> Option<T> {
    use x86_64::instructions::port::Port;

    let count = ((PIT_FREQ as u64 * ms as u64) / 1000) as u16;
    if count == 0 { return None; }

    unsafe {
        let mut gate: Port<u8> = Port::new(0x61);
        let gate_v = gate.read();
        // Gate closed, speaker off, from whatever the BIOS left
        gate.write(gate_v & !0x03);

        let mut cmd: Port<u8> = Port::new(0x43);
        cmd.write(0xB0); // channel 2, LSB+MSB, mode 0, binary

        let mut ch2: Port<u8> = Port::new(0x42);
        ch2.write(count as u8);
        ch2.write((count >> 8) as u8);

        // Mode-0 programming forces OUT low. High here means no PIT worth
        // timing against
        if gate.read() & 0x20 != 0 {
            gate.write(gate_v);
            return None;
        }

        start();
        gate.write((gate_v & !0x02) | 0x01); // open the gate, counter runs

        // Hard ceiling on the spin, so a chip never raising OUT cannot hang the
        // boot
        let mut budget: u32 = 20_000_000;
        loop {
            if gate.read() & 0x20 != 0 { break; }
            budget -= 1;
            if budget == 0 {
                gate.write(gate_v);
                return None;
            }
            core::hint::spin_loop();
        }
        let v = end();
        gate.write(gate_v);
        Some(v)
    }
}

/// Calibrate the TSC against PIT channel 2 before interrupts exist.
///
/// The tick-counting calibration below cannot run until the LAPIC timer is
/// armed and `sti` has happened, nearly the last thing boot does. Until this,
/// `tsc_khz()` is zero and `udelay` counts reads of port 0x80, assuming one
/// microsecond per read; the real figure depends on how the chipset decodes LPC
/// and is off by a factor of two or more on modern boards.
///
/// Returns false if the PIT did not respond; `calibrate()` then falls back to
/// counting timer ticks
pub fn calibrate_early() -> bool {
    // 10 ms is 11932 PIT counts and roughly 30 million TSC cycles: four
    // significant figures, for a fortieth of the tick-counting path's cost
    const CAL_MS: u32 = 10;

    let mut t0 = 0u64;
    let Some(t1) = pit_one_shot(CAL_MS, || t0 = rdtsc(), rdtsc) else {
        crate::serial_println!("[timing] PIT channel 2 did not respond; TSC uncalibrated");
        return false;
    };
    let khz = t1.saturating_sub(t0) / CAL_MS as u64;

    // A modern part is 800 MHz at its lowest P-state and no shipping x86 is
    // past 6 GHz. Outside that the measurement is not a clock
    if !(800_000..=6_000_000).contains(&khz) {
        crate::serial_println!("[timing] PIT calibration gave {} kHz - out of range, ignored", khz);
        return false;
    }

    TSC_KHZ.store(khz, Ordering::Relaxed);
    BOOT_TSC.store(rdtsc(), Ordering::Relaxed);
    crate::serial_println!("[timing] TSC {} MHz (PIT ch2, {} ms window)", khz / 1000, CAL_MS);
    true
}

pub fn calibrate() {
    // Whether the timer fires still has to be established: `calibrate_early`
    // proves the PIT counts, not that the LAPIC delivers an interrupt. With the
    // TSC known this is a liveness check, not a measurement
    let have_tsc = TSC_KHZ.load(Ordering::Relaxed) != 0;
    if have_tsc {
        const FIRST_TICK_TIMEOUT_MS: u64 = 1000;
        let t0 = crate::arch::x86_64::interrupts::get_tick();
        let mut waited_ms = 0u64;
        while crate::arch::x86_64::interrupts::get_tick() == t0 {
            if waited_ms >= FIRST_TICK_TIMEOUT_MS {
                crate::serial_println!(
                    "[timing] error: no timer ticks received after {} ms!", waited_ms);
                crate::serial_println!("[timing] Timer interrupt may not be firing - check LAPIC configuration");
                return;
            }
            udelay(1000);
            waited_ms += 1;
        }
        crate::serial_println!(
            "[timing] timer live (first tick after {} ms), TSC {} MHz",
            waited_ms, tsc_khz() / 1000);
        return;
    }

    crate::serial_println!("[timing] no PIT calibration; falling back to counting timer ticks...");

    // Wait for the first tick, bounded by wall-clock time, not a spin count: at
    // 250 Hz ticks are 4 ms apart, and a small spin budget would declare a
    // healthy timer dead. udelay() works before TSC calibration (port 0x80
    // fallback)
    const FIRST_TICK_TIMEOUT_MS: u64 = 1000;
    let t0 = crate::arch::x86_64::interrupts::get_tick();
    let mut waited_ms = 0u64;
    while crate::arch::x86_64::interrupts::get_tick() == t0 {
        if waited_ms >= FIRST_TICK_TIMEOUT_MS {
            crate::serial_println!(
                "[timing] error: no timer ticks received after {} ms!", waited_ms);
            crate::serial_println!("[timing] Timer interrupt may not be firing - check LAPIC configuration");
            // No hang: return with TSC_KHZ = 0
            TSC_KHZ.store(0, Ordering::Relaxed);
            return;
        }
        udelay(1000);
        waited_ms += 1;
    }
    crate::serial_println!("[timing] first tick received after {} ms", waited_ms);

    let (tsc_start, tick_start) = x86_64::instructions::interrupts::without_interrupts(|| {
        (rdtsc(), crate::arch::x86_64::interrupts::get_tick())
    });

    crate::serial_println!("[timing] waiting for 100 ticks (started at tick {})...", tick_start);
    let mut wait_count = 0;
    while crate::arch::x86_64::interrupts::get_tick() < tick_start + 100 {
        wait_count += 1;
        if wait_count > 100_000_000 {
            crate::serial_println!("[timing] error: stuck waiting for ticks!");
            TSC_KHZ.store(0, Ordering::Relaxed);
            return;
        }
        core::hint::spin_loop();
    }
    crate::serial_println!("[timing] 100 ticks elapsed ({} spins)", wait_count);

    let tsc_end = rdtsc();
    let cycles = tsc_end.saturating_sub(tsc_start);

    let elapsed_ms = 100u64 * 1000 / crate::arch::x86_64::interrupts::PIT_HZ as u64;
    let khz = cycles / elapsed_ms;

    TSC_KHZ.store(khz, Ordering::Relaxed);
    crate::serial_println!("[timing] TSC ~{} MHz", khz / 1000);
}

/// The TSC as of calibration. The vDSO turns a raw TSC reading into time since
/// boot with it, without entering the kernel
pub fn boot_tsc() -> u64 {
    BOOT_TSC.load(Ordering::Relaxed)
}

pub fn tsc_khz() -> u64 {
    TSC_KHZ.load(Ordering::Relaxed)
}

/// Milliseconds since the TSC was calibrated, the third thing `kernel_main`
/// does. Zero if calibration never landed
pub fn since_boot_ms() -> u64 {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    let t0 = BOOT_TSC.load(Ordering::Relaxed);
    if khz == 0 || t0 == 0 { return 0; }
    rdtsc().wrapping_sub(t0) / khz
}

/// Whether the clock below is the TSC. False before calibration, and on a
/// machine where it failed: then it is the scheduler tick, 4 ms a step
pub fn precise() -> bool {
    TSC_KHZ.load(Ordering::Relaxed) != 0 && BOOT_TSC.load(Ordering::Relaxed) != 0
}

/// Nanoseconds since the TSC was calibrated: the clock of every deadline in the
/// kernel (`sched::timer`) and of the vDSO's `CLOCK_MONOTONIC`, by the same
/// arithmetic, so the syscall and the vDSO agree (`lib/runtime/vdso`, `cycles_to_time`)
pub fn now_ns() -> u64 {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    let t0 = BOOT_TSC.load(Ordering::Relaxed);
    if khz == 0 || t0 == 0 {
        let hz = (crate::arch::x86_64::apic::timer_hz() as u64).max(1);
        return crate::arch::x86_64::interrupts::get_tick() * (1_000_000_000 / hz);
    }
    let c = rdtsc().wrapping_sub(t0);
    let per_sec = khz * 1000;
    // rem < khz*1000, so rem * 1_000_000 < khz * 1e9: far inside a u64
    (c / per_sec) * 1_000_000_000 + (c % per_sec) * 1_000_000 / khz
}

/// The TSC reading at which `now_ns` will say `ns`. Only meaningful when
/// `precise`
pub fn ns_to_tsc(ns: u64) -> u64 {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    let t0 = BOOT_TSC.load(Ordering::Relaxed);
    let per_sec = khz * 1000;
    t0.wrapping_add((ns / 1_000_000_000) * per_sec + (ns % 1_000_000_000) * khz / 1_000_000)
}

/// Microseconds since the TSC was calibrated, for log timestamps. Zero before
/// then. Split so the multiply cannot overflow in any uptime
pub fn since_boot_us() -> u64 {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    let t0 = BOOT_TSC.load(Ordering::Relaxed);
    if khz == 0 || t0 == 0 { return 0; }
    let c = rdtsc().wrapping_sub(t0);
    c / khz * 1000 + (c % khz) * 1000 / khz
}

pub fn rdtsc() -> u64 {
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem, preserves_flags)
        );
        ((hi as u64) << 32) | lo as u64
    }
}

pub struct Stopwatch {
    start_tsc: u64,
}

impl Stopwatch {
    pub fn start() -> Self {
        Self { start_tsc: rdtsc() }
    }

    pub fn elapsed_us(&self) -> u64 {
        let khz = tsc_khz().max(1);
        let cycles = rdtsc().wrapping_sub(self.start_tsc);
        cycles * 1000 / khz
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_us() / 1000
    }
}

/// Busy-wait for approximately 'us' microseconds. Safe before the scheduler
/// exists. Falls back to I/O port reads if the TSC is not calibrated yet (each
/// 0x80 read is ~1us on real hardware)
pub fn udelay(us: u64) {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    if khz > 0 {
        let target_cycles = khz.saturating_mul(us) / 1000;
        let start = rdtsc();
        while rdtsc().wrapping_sub(start) < target_cycles {
            core::hint::spin_loop();
        }
        return;
    }
    // Fallback: port 0x80 ~1us per access on real hw
    use x86_64::instructions::port::Port;
    let mut p: Port<u8> = Port::new(0x80);
    for _ in 0..us {
        unsafe { let _ = p.read(); }
    }
}

pub fn mdelay(ms: u64) {
    udelay(ms.saturating_mul(1000));
}
