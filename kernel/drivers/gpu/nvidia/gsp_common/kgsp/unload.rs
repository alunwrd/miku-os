// Telling GSP-RM to go, and waiting until it has
//
// `kgspUnloadRm_IMPL` (615): GMCAPI `GSP_SUSPEND`, fire and forget, then the
// wait for the processor to suspend: MAILBOX0 with the suspended bit
// (`_kgspIsProcessorSuspended`), handling what GSP-RM says meanwhile
// (`kgspWaitForProcessorSuspend_TU102` with bProcessRpcs), then the teardown,
// on Hopper and later only the wait for the RISC-V core to halt so the ACR and
// GSP-FMC can finish their shutdown (`kgspTeardown_GH100`, through
// `kflcnWaitForHaltRiscv_GA102`). Without the halt WPR2 stays up for the next
// boot.
//
// Chip-neutral: the chip hands in the rings and where the GSP's registers are
// (`GspRegs`).
//
// Every wait is bounded, and each step's outcome is reported separately: this
// also runs on the way to a reboot, where "we asked" and "it stopped" are
// different facts

use crate::drivers::gpu::nvidia::gsp_common::gmcapi;
use crate::drivers::gpu::nvidia::gsp_common::rpc::{self, GspRpc, RpcError};
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

/// `NV_PFALCON_FALCON_MAILBOX0`, in the falcon window
const FALCON_MAILBOX0: u32 = 0x040;
/// The bit MAILBOX0 carries once libos has suspended the processor
/// (`INTERRUPT_PROCESSOR_SUSPENDED_VALUE`; tested as a bit since 615)
const PROCESSOR_SUSPENDED: u32 = 0x8000_0000;
/// `NV_PRISCV_RISCV_CPUCTL` and `_HALTED`, in the RISC-V window
const RISCV_CPUCTL: u32 = 0x388;
const CPUCTL_HALTED: u32 = 1 << 4;

/// How long each wait may take
#[derive(Clone, Copy)]
pub struct Budget {
    /// Getting GSP_SUSPEND onto the ring
    pub rpc_ns: u64,
    /// MAILBOX0 reaching the suspended value
    pub suspend_us: u64,
    /// The RISC-V core halting after that
    pub halt_us: u64,
}

impl Budget {
    /// OpenRM's: `GPU_TIMEOUT_DEFAULT`, four seconds, for each wait
    /// (`gpuTimeoutCondWait` with no timeout of its own; `os.c:1952`)
    pub const OPENRM: Budget = Budget { rpc_ns: 4_000_000_000, suspend_us: 4_000_000, halt_us: 4_000_000 };
    /// On the way to a reboot: a second in all, and the machine turns off
    /// regardless
    pub const SHUTDOWN: Budget = Budget { rpc_ns: 500_000_000, suspend_us: 500_000, halt_us: 500_000 };
}

/// What the unload managed. Every field is false when that step was not reached
#[derive(Clone, Copy, Debug, Default)]
pub struct UnloadReport {
    /// GSP_SUSPEND went out
    pub rpc_ok: bool,
    /// MAILBOX0 carries the suspended bit
    pub suspended: bool,
    /// The RISC-V core reports halted
    pub halted: bool,
    /// What MAILBOX0 last read
    pub mbox0: u32,
    /// What RISCV_CPUCTL last read
    pub cpuctl: u32,
}

/// `kgspUnloadRm_IMPL`, normal mode: not a power transition, no flags
pub fn unload_rm(
    rpc: &mut GspRpc,
    bar0: &MmioRegion,
    budget: Budget,
) -> UnloadReport {
    let mut report = UnloadReport::default();

    // `GmcApiGspSuspendParams`: one u64 of flags
    let flags = 0u64;
    match rpc.gmcapi_post(gmcapi::GSP_SUSPEND, &flags.to_le_bytes(), budget.rpc_ns) {
        Ok(()) => report.rpc_ok = true,
        // A GSP-RM known dead gets no command (`health::sanity`), and the waits
        // below still run: a dead core may be halted already
        Err(e @ (RpcError::ResetRequired | RpcError::GpuLost)) => {
            crate::kwarn!("[gsp] GSP_SUSPEND not sent: {:?}", e);
        }
        Err(e) => crate::kerror!("[gsp] GSP_SUSPEND failed: {:?}", e),
    }

    let Some(regs) = super::regs() else {
        crate::kwarn!("[gsp] unload: no GSP register window was set - nothing to wait on");
        return report;
    };

    // Suspend first, then halt: OpenRM looks for the halt once the processor
    // says it is done. While waiting for the suspend it reads the ring, where
    // GSP-RM's last events come, and the logs
    let mut dispatch = rpc.events();
    let mut handled = 0;
    let (ok, mbox0) = wait_for(budget.suspend_us, || {
        if report.rpc_ok {
            let _ = rpc.drain(rpc::NOTHING, None, rpc::Context::Poll, &mut dispatch, &mut handled);
        }
        let v = bar0.read32(regs.falcon + FALCON_MAILBOX0);
        (v & PROCESSOR_SUSPENDED != 0, v)
    });
    report.suspended = ok;
    report.mbox0 = mbox0;
    // What GSP-RM said on its way down, before the teardown
    rpc.dump_logs();

    let (ok, cpuctl) = wait_for(budget.halt_us, || {
        let v = bar0.read32(regs.riscv + RISCV_CPUCTL);
        (v & CPUCTL_HALTED != 0, v)
    });
    report.halted = ok;
    report.cpuctl = cpuctl;

    let clean = report.rpc_ok && report.suspended && report.halted;
    crate::kcore::klog::emit(
        if clean { crate::kcore::klog::Level::Info } else { crate::kcore::klog::Level::Warn },
        crate::klog_site!(),
        format_args!("[gsp] unload: rpc {} suspended {} (mbox0 {:#x}) halted {} (cpuctl {:#010x})",
            report.rpc_ok, report.suspended, report.mbox0, report.halted, report.cpuctl),
    );
    report
}

/// Poll `probe` until it says done or `budget_us` has passed. Returns whether
/// it finished and what it last read
fn wait_for(budget_us: u64, mut probe: impl FnMut() -> (bool, u32)) -> (bool, u32) {
    let sw = crate::kcore::time::Stopwatch::start();
    loop {
        let (done, v) = probe();
        if done {
            return (true, v);
        }
        if sw.elapsed_us() >= budget_us {
            return (false, v);
        }
        crate::kcore::time::udelay(50);
    }
}

/// GSP-RM is gone: forget what this driver knew about it. Its fatal state and
/// lockdown were facts about that processor, and OpenRM builds a new
/// `KernelGsp` on a driver load. The counters stay: they describe the machine's
/// whole run
pub fn forget() {
    use core::sync::atomic::Ordering;
    super::FATAL.store(false, Ordering::Release);
    super::set_lockdown(false);
    super::service::HALT_PENDING.store(false, Ordering::Release);
    super::service::FATAL_CODE.store(0, Ordering::Release);
    super::health::rearm();
    super::logs::forget();
}
