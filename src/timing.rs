use core::sync::atomic::{AtomicU64, Ordering};

static TSC_KHZ: AtomicU64 = AtomicU64::new(0);

pub fn calibrate() {
    let t0 = crate::interrupts::get_tick();
    while crate::interrupts::get_tick() == t0 {}

    let (tsc_start, tick_start) = x86_64::instructions::interrupts::without_interrupts(|| {
        (rdtsc(), crate::interrupts::get_tick())
    });

    while crate::interrupts::get_tick() < tick_start + 100 {}

    let tsc_end = rdtsc();
    let cycles = tsc_end.saturating_sub(tsc_start);

    let elapsed_ms = 100u64 * 1000 / crate::interrupts::PIT_HZ as u64;
    let khz = cycles / elapsed_ms;

    TSC_KHZ.store(khz, Ordering::Relaxed);
    crate::serial_println!("[timing] TSC ~{} MHz", khz / 1000);
}

pub fn tsc_khz() -> u64 {
    TSC_KHZ.load(Ordering::Relaxed)
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

/// Busy-wait for approximately `us` microseconds.
/// Safe to call before the scheduler exists. Falls back to I/O port reads
/// if TSC isn't calibrated yet (each 0x80 read is ~1us on real hardware)
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
