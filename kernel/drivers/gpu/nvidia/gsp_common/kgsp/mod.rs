// KernelGsp: what belongs to GSP-RM itself, not to one transport or engine
//
// `dev/gsp-foundation.md` is the design: the dispatcher (`events.rs`, step
// 1.5), the interrupt half (`service.rs`) and the health check (`health.rs`,
// both 1.8), the worker's wakeup (`worker.rs`, 1.9), the engine order
// (`engine.rs`, 1.11), the unload (`unload.rs`, 1.14), GSP-RM's own log
// (`logs.rs`) and its crash reports (`crashcat.rs`, section 15), and two facts
// the rest of the driver respects: GSP-RM has fenced its registers off, or
// GSP-RM is dead.

use core::sync::atomic::{AtomicBool, Ordering};

pub mod crashcat;
pub mod engine;
pub mod events;
pub mod health;
pub mod logs;
pub mod service;
pub mod unload;
pub mod worker;

/// GSP-RM is dead (`bFatalError`).
///
/// Set by a fatal interrupt or a failed health check, cleared only when GSP-RM
/// is unloaded (`unload::forget`): a GSP that died stays dead until a new one
/// boots. While set no RPC goes out (`health::sanity`) and no channel takes
/// work (`chan::Channel::usable`)
static FATAL: AtomicBool = AtomicBool::new(false);

pub fn fatal() -> bool {
    FATAL.load(Ordering::Acquire)
}

/// Where the GSP's registers are. The chip's to say (`blackwell::interrupts`),
/// read by the interrupt half and the health check
#[derive(Clone, Copy, Debug)]
pub struct GspRegs {
    /// The falcon window (`NV_PGSP_FALCON_*`, 0x110000)
    pub falcon: u32,
    /// The RISC-V window (`NV_PRISCV_RISCV_*`)
    pub riscv: u32,
    /// `NV_PGSP_FALCON_IRQSTAT_FATAL_ERROR`, or 0 on a chip without one
    pub fatal_error: u32,
    /// `NV_PGSP_RISCV_FAULT_CONTAINMENT_SRCSTAT`, what a fatal error says it
    /// was
    pub fault_srcstat: u32,
}

static REGS: spin::Mutex<Option<GspRegs>> = spin::Mutex::new(None);

/// Tell KernelGsp where this chip keeps the GSP's registers. Once, at boot
pub fn set_regs(regs: GspRegs) {
    *REGS.lock() = Some(regs);
}

fn regs() -> Option<GspRegs> {
    *REGS.lock()
}

/// GSP-RM said its core is locked down (`bInLockdown`).
///
/// While it is, some of the GSP's registers, its interrupt status and control
/// among them, must not be touched (`_kgspRpcGspLockdownNotice`,
/// `kernel_gsp.c:1355`). Set and cleared by `GSP_LOCKDOWN_NOTICE`, flooding
/// during init and ending disengaged
static IN_LOCKDOWN: AtomicBool = AtomicBool::new(false);

pub fn in_lockdown() -> bool {
    IN_LOCKDOWN.load(Ordering::Acquire)
}

fn set_lockdown(engaged: bool) {
    IN_LOCKDOWN.store(engaged, Ordering::Release);
}
