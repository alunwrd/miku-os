// A dead GSP-RM, brought back without a reboot
//
// `dev/gsp-foundation.md`, section 16. OpenRM stops at "the GPU needs a reset"
// (`kgspRcAndNotifyAllChannels`). This orders the forced unload, the
// function-level reset, the BAR firewall and the boot. The display comes back
// from cold: the reset took the head the firmware lit.
//
// `nvidia gsp reset` runs it by hand, on a live GSP-RM for testing.
// `gsprecover` on the boot line runs it when the event thread first sees FATAL,
// on a kernel worker, at most twice a boot.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;

/// Automatic recoveries this boot, and the ceiling: a GSP-RM dying again right
/// after coming back is not fixed by a reset
static AUTOMATIC: AtomicU32 = AtomicU32::new(0);
const MAX_AUTOMATIC: u32 = 2;

/// Unload, reset, boot, and put the desktop back if it was up. The card is
/// found the way every command finds it; `say` is told every step
pub fn reset_and_reload(say: &dyn Fn(&str)) -> Result<(), &'static str> {
    // Under the display layer's lock: no 'display' request lands while the
    // display goes down and comes back from cold
    crate::drivers::gpu::kms::exclusive(|| reset_and_reload_held(say))
}

fn reset_and_reload_held(say: &dyn Fn(&str)) -> Result<(), &'static str> {
    let desktop = crate::mayland::render::running();
    if desktop {
        say("recovery: stopping the compositor");
        crate::mayland::render::stop();
    }
    let result = crate::drivers::gpu::nvidia::with_gb20x(|d| reset_card(&d.bar0, &d.pci, d.gb, say))
        .unwrap_or(Err("no GB20x card registered"));
    if desktop && result.is_ok() {
        match crate::mayland::render::start() {
            Ok(()) => say("recovery: compositor started again"),
            Err(e) => said(Level::Warn, &say, &alloc::format!("recovery: the compositor did not start again: {}", e)),
        }
    }
    result
}

fn reset_card(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    gpu: &crate::drivers::gpu::nvidia::pci::GpuDevice,
    chip: &'static super::super::chips::Chip,
    say: &dyn Fn(&str),
) -> Result<(), &'static str> {
    // 1. Down, without waiting for anyone: with GSP-RM dead every RPC refuses
    //    at once, and the engines come down on their bookkeeping
    if super::super::gspboot::is_booted() {
        match super::unload::unload_forced(bar0, gpu, say) {
            Ok(u) => say(&alloc::format!(
                "recovery: unloaded (halted {}, WPR2 hi {:#010x}, rings {})",
                u.gsp.halted, u.wpr2_hi, if u.freed { "freed" } else { "kept pinned" }
            )),
            Err(e) => {
                said(Level::Error, &say, &alloc::format!("recovery: unload refused: {}", e));
                return Err("the unload was refused");
            }
        }
    }

    // 2. The reset, and the FSP's firewall over the BARs after it
    //    (`kbifPollBarFirewallDisengage_GB202`): nothing may be asked of the
    //    card before it drops. The boot waits for GFW itself
    if let Err(e) = crate::drivers::gpu::nvidia::reset::function_level_reset(gpu) {
        said(Level::Error, &say, &alloc::format!("recovery: FLR failed: {:?} - the card needs a power cycle", e));
        return Err("the function-level reset failed");
    }
    match super::super::hal_gb20x::poll_bar_firewall(bar0) {
        Ok(ms) => say(&alloc::format!("recovery: FLR done, BAR firewall down after {} ms", ms)),
        Err(v) => {
            said(Level::Error, &say, &alloc::format!(
                "recovery: the BAR firewall is still up after the FLR (VSEC_DEBUG_SEC_2 {:#010x})", v));
            return Err("the BAR firewall stayed up");
        }
    }

    // 3. The machine's own boot. From cold: the head the firmware lit went with
    //    the reset
    match super::boot(bar0, gpu, chip) {
        Ok(r) if r.init_done => {
            match super::super::window::live_scanout() {
                Some(l) => say(&alloc::format!(
                    "recovery: GSP-RM is up again, picture {}x{} on head {}", l.width, l.height, l.head)),
                None => said(Level::Warn, &say, "recovery: GSP-RM is up again, but no head is scanning out"),
            }
            Ok(())
        }
        Ok(_) => {
            said(Level::Error, &say, "recovery: the boot ended without an answer to GSP_INIT");
            Err("GSP-RM did not come back")
        }
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("recovery: the boot failed: {:?}", e));
            Err("the boot failed")
        }
    }
}

/// The event thread saw GSP-RM die. With `gsprecover` on the boot line, hand
/// the recovery to a kernel worker: the unload takes the RM lock, and this runs
/// on the thread draining what RM says
pub fn on_fatal() {
    if !crate::arch::x86_64::grub::cmdline_flag("gsprecover") {
        return;
    }
    let n = AUTOMATIC.fetch_add(1, Ordering::AcqRel);
    if n >= MAX_AUTOMATIC {
        crate::kerror!("[nvidia] GSP-RM died again after {} recoveries - leaving it; reboot to get the GPU back", n);
        return;
    }
    crate::kwarn!("[nvidia] GSP-RM is dead - 'gsprecover': resetting the GPU and booting it again ({} of {})",
        n + 1, MAX_AUTOMATIC);
    crate::sched::submit_task(|| {
        let say = |l: &str| crate::kinfo!("[nvidia] {}", l);
        match reset_and_reload(&say) {
            Ok(()) => crate::knotice!("[nvidia] recovery done - the GPU is back"),
            Err(e) => crate::kerror!("[nvidia] recovery failed: {} - reboot to get the GPU back", e),
        }
    });
}
