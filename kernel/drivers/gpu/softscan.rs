// The screen when no card drives it
//
// Firmware left a framebuffer and the processor writes the rows into it. No
// raster to synchronise with, no engine fence to retire, no second surface to
// flip to; this keeps time against the monitor's refresh rate.

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

use super::scanout::{OutputInfo, Scanout, ScreenBlit};

/// The pacer's state. Two numbers carried between frames: the microseconds the
/// tick grid could not express, and when the next frame is due
static DEBT_US:  AtomicU64 = AtomicU64::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);

/// The framebuffer this backend writes into
///
/// Taken once when compositing starts, before the console is offered a window:
/// with no card the console is on the panel until then, and after it the
/// console's target is a shadow in memory. The presenter holds the screen
/// itself
static PANEL: spin::Mutex<Option<crate::io::console::Surface>> = spin::Mutex::new(None);

pub fn set_panel(s: Option<crate::io::console::Surface>) {
    *PANEL.lock() = s;
}

pub fn panel() -> Option<crate::io::console::Surface> {
    *PANEL.lock()
}

pub struct SoftScanout;

/// The frame period this display asks for, in microseconds
///
/// From the monitor, through the mode programmed: the Bochs device in a virtual
/// machine, whatever firmware chose on a real machine with no driver for its
/// card. QEMU's emulated monitor asks for 74.99 Hz
fn period_us() -> u64 {
    let centihz = crate::drivers::gpu::display::chosen()
        .and_then(|c| c.timing)
        .map(|t| t.refresh_centihz())
        .unwrap_or(0);
    // Outside what a panel can do means the descriptor was not a mode
    let centihz = if (2000..=36000).contains(&centihz) { centihz } else { 6000 };
    1_000_000u64 * 100 / centihz as u64
}

impl Scanout for SoftScanout {
    fn name(&self) -> &'static str { "processor" }

    fn output(&self, out: usize) -> Option<OutputInfo> {
        if out != 0 { return None; }
        let (w, h) = crate::io::console::fb_size()?;
        let centihz = crate::drivers::gpu::display::chosen()
            .and_then(|c| c.timing)
            .map(|t| t.refresh_centihz())
            .unwrap_or(6000);
        Some(OutputInfo {
            width: w as u32,
            height: h as u32,
            refresh_mhz: centihz as u32 * 10,
        })
    }

    /// Sleep until the next frame is due, without drifting
    ///
    /// `sched::sleep` counts whole ticks of 4 ms, so a 16.667 ms frame is
    /// 4.1667 of them, and sleeping 4 runs at 62.5 Hz: one frame swallowed or
    /// shown twice every couple of seconds.
    ///
    /// So the remainder is carried. The debt accumulates the microseconds the
    /// tick grid cannot express and spends them as a fifth tick when they add
    /// up to one; the frame times go 16, 16, 16, 16, 20 and average the period.
    /// What is left is trimmed by a short busy wait, bounded so a mispredicted
    /// deadline costs microseconds
    fn wait_next_frame(&self, _out: usize, _timeout_us: u64) -> bool {
        const TICK_US: u64 = 1_000_000 / crate::arch::x86_64::apic::TIMER_HZ_DEFAULT as u64;
        /// One tick of jitter is what the grid costs; removing the rest is
        /// worth a millisecond, under a tenth of a frame
        const TRIM_MAX_US: u64 = 1_000;

        let period = period_us();
        let khz = crate::kcore::time::tsc_khz();
        if khz == 0 {
            crate::sched::sleep(period / TICK_US);
            return true;
        }
        let now = crate::kcore::time::rdtsc();
        let mut deadline = DEADLINE.load(Ordering::Relaxed);
        let mut debt = DEBT_US.load(Ordering::Relaxed);

        // A deadline that is unset or a whole frame behind (a long compose, a
        // descheduled machine) is re-anchored. Chasing it would give a burst of
        // frames with no sleep between
        let late_us = now.saturating_sub(deadline) * 1000 / khz;
        if deadline == 0 || late_us > period {
            deadline = now;
            debt = 0;
        }

        let want = period + debt;
        let ticks = want / TICK_US;
        debt = want % TICK_US;
        deadline = deadline.wrapping_add(period * khz / 1000);

        DEBT_US.store(debt, Ordering::Relaxed);
        DEADLINE.store(deadline, Ordering::Relaxed);

        if ticks > 0 {
            crate::sched::sleep(ticks);
        }
        // Waking from `sleep` lands inside the tick that expired, so without
        // this the frame is up to a tick early
        let now = crate::kcore::time::rdtsc();
        if now < deadline {
            let left = (deadline - now) * 1000 / khz;
            if left <= TRIM_MAX_US {
                crate::kcore::time::udelay(left);
            }
        }
        true
    }

    /// The same batch of rectangles, written by the processor.
    ///
    /// The blits carry physical addresses for an engine; the processor reaches
    /// the same memory through the direct map
    fn blit_batch(&self, _out: usize, blits: &[ScreenBlit]) -> (u64, u64) {
        let Some(panel) = panel() else {
            return (0, blits.len() as u64);
        };
        let (mut ok, mut refused) = (0u64, 0u64);
        for b in blits {
            if b.src_phys == 0 || b.w == 0 || b.h == 0 {
                refused += 1;
                continue;
            }
            let src = crate::arch::x86_64::grub::phys_to_virt(b.src_phys) as *const u8;
            unsafe {
                crate::io::console::blit_xrgb_to(
                    &panel,
                    src, b.src_pitch as usize,
                    0, 0,
                    b.x as usize, b.y as usize,
                    b.w as usize, b.h as usize,
                );
            }
            ok += 1;
        }
        // The panel is mapped write-combining, and a store sits in a fill
        // buffer until something flushes it. Left alone, the tail of a frame
        // appears when the next one pushes it out. One fence a frame, not one a
        // row
        unsafe { core::arch::x86_64::_mm_sfence() };
        (ok, refused)
    }

    fn refusals(&self) -> String { String::new() }
}
