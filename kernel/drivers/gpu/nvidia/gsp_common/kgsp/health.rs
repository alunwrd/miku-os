// Is GSP-RM alive, and may an RPC go out?
//
// Both OpenRM's. `_kgspRpcSanityCheck` (`kernel_gsp.c:261`) runs before an RPC
// is sent (`:377`) and on every turn of a poll (`:2174`): a GSP known dead, or
// a GPU gone, gets no more RPCs, each of which would wait out its timeout.
// `kgspHealthCheck` (`kernel_gsp_tu102.c:976`) runs after every drain
// (`_kgspRpcDrainEvents`) and decides whether GSP-RM has died.
//
// The health check reads CrashCat reports, written into a queue the WPR
// metadata points at (`kernel_gsp_gh100.c:363-370`); `crashcat` reads them, and
// a report not contained below GSP-RM makes it dead (`dev/gsp-foundation.md`,
// section 15). Without a queue (`gspbase`, or no page for it) this judges by
// the fatal interrupt and a HALT that left the RISC-V core halted (decision 4).

use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::gpu::nvidia::gsp_common::rpc::RpcError;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

/// `NV_PRISCV_RISCV_CPUCTL` and its HALTED bit (`gh100/dev_riscv_pri.h:58-62`)
const RISCV_CPUCTL: u32 = 0x388;
const CPUCTL_HALTED: u32 = 1 << 4;

/// What a read of a GPU that has fallen off the bus returns
const GPU_GONE: u32 = 0xffff_ffff;

/// The first fatal is announced; the ones after it are the same fact
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// A new GSP-RM gets its own announcement (`unload::forget`)
pub(super) fn rearm() {
    ANNOUNCED.store(false, Ordering::Release);
}

/// May an RPC go out? (`_kgspRpcSanityCheck`)
///
/// `bar0` is the card the transport is on, if the caller can afford a register
/// read: PMC_BOOT_0 reading all ones is a GPU gone (`PDB_PROP_GPU_IS_LOST`).
/// `None` checks the flag alone
pub fn sanity(bar0: Option<&MmioRegion>) -> Result<(), RpcError> {
    if super::fatal() {
        return Err(RpcError::ResetRequired);
    }
    if bar0.is_some_and(|b| b.read32(0) == GPU_GONE) {
        return Err(RpcError::GpuLost);
    }
    Ok(())
}

/// Every report in the queue, logged; any impacting GSP-RM makes it dead
/// (`kgspHealthCheck_TU102`, the CrashCat half). A libos watchdog report is
/// logged and kept but is not an error: "timeouts are not considered as errors"
fn read_crash_reports(bar0: &MmioRegion, falcon: u32) {
    let mut healthy = true;
    while let Some(report) = super::crashcat::next_report(bar0, falcon) {
        let watchdog = report.is_watchdog();
        if report.is_fatal() {
            healthy = false;
        }
        let lines = report.lines();
        if watchdog {
            crate::kwarn!("[gsp] *************************** LibOS Task Watchdog Report ***************************");
            crate::pstore_println!("GSP libos watchdog report:");
            for l in &lines {
                crate::kwarn!("[gsp] {}", l);
                crate::pstore_println!("  {}", l);
            }
        } else {
            crate::kerror!("[gsp] ****************************** GSP-CrashCat Report *******************************");
            crate::pstore_println!("GSP-CrashCat report:");
            for l in &lines {
                crate::kerror!("[gsp] {}", l);
                crate::pstore_println!("  {}", l);
            }
        }
        super::crashcat::remember(lines);
    }
    if !healthy {
        super::FATAL.store(true, Ordering::Release);
        crate::kerror!("[gsp] **********************************************************************************");
    }
}

/// Is GSP-RM alive? (`kgspHealthCheck`). False once, and then for good
pub fn check(bar0: Option<&MmioRegion>) -> bool {
    if !super::fatal() {
        if let (Some(bar0), Some(regs)) = (bar0, super::regs()) {
            if super::crashcat::configured() {
                // In lockdown the reports wait for the next drain: no GSP
                // register is read then
                if !super::in_lockdown() {
                    super::service::HALT_PENDING.store(false, Ordering::Relaxed);
                    read_crash_reports(bar0, regs.falcon);
                }
            } else if super::service::HALT_PENDING.swap(false, Ordering::Relaxed) {
                let cpuctl = bar0.read32(regs.riscv + RISCV_CPUCTL);
                if cpuctl & CPUCTL_HALTED != 0 {
                    crate::kerror!(
                        "[gsp] HALT, and the RISC-V core is halted (cpuctl {:#010x}) - GSP-RM has stopped",
                        cpuctl
                    );
                    super::FATAL.store(true, Ordering::Release);
                } else {
                    crate::kwarn!(
                        "[gsp] HALT with the core still running (cpuctl {:#010x}) - a CrashCat report, and this boot has no queue to read it from",
                        cpuctl
                    );
                }
            }
        }
    }
    if !super::fatal() {
        return true;
    }
    if !ANNOUNCED.swap(true, Ordering::AcqRel) {
        let code = super::service::FATAL_CODE.load(Ordering::Relaxed);
        // Serial only: this runs inside the transport, possibly under the
        // display's locks, and the console may need them. The event thread says
        // it on the screen (`control::report_trouble`)
        crate::kerror!(
            "[gsp] GSP-RM is dead{} - no further RPCs, every channel stops; only a GPU reset brings it back",
            if code != 0 { alloc::format!(" (fault containment {:#x})", code) } else { alloc::string::String::new() }
        );
    }
    false
}
