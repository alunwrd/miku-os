// The GSP falcon's interrupt: the half running in the ISR
//
// `kgspService_TU102` (`kernel_gsp_tu102.c:1036`) with
// `kgspServiceFatalHwError_GB100` (`kernel_gsp_gb100.c:155`) folded in, in
// OpenRM's order: HALT, SWGEN0, fatal, retrigger. OpenRM handles the events in
// its DPC, under the GPU lock; here this clears, counts and raises flags, and
// everything needing a lock, a print or an RPC happens in thread context:
// `health::check` for HALT and fatal, the RPC waiter and the event thread for
// SWGEN0. The event thread is woken for all three (`worker::kick`).
//
// Atomics and MMIO only: no lock, no allocation, no printing.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use crate::drivers::gpu::nvidia::falcon::{
    FALCON_INTR_RETRIGGER, FALCON_IRQMCLR, FALCON_IRQSCLR, FALCON_IRQSTAT, FALCON_IRQ_SWGEN0,
};
use crate::drivers::gpu::nvidia::gsp_common::rpc;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

use super::GspRegs;

/// `NV_PFALCON_FALCON_IRQSTAT_HALT` (4:4): the CrashCat signal, GSP-RM wanting
/// the host to look at something
const IRQ_HALT: u32 = 1 << 4;
/// `NV_PRISCV_RISCV_IRQMASK` and `_IRQDEST` in the GA102 layout every GB20x
/// keeps (`ga102/dev_riscv_pri.h:28-29`)
const RISCV_IRQMASK: u32 = 0x528;
const RISCV_IRQDEST: u32 = 0x52c;

static SERVICED: AtomicU64 = AtomicU64::new(0);
/// Called with nothing pending. OpenRM asserts
static SPURIOUS: AtomicU64 = AtomicU64::new(0);
static HALTS: AtomicU32 = AtomicU32::new(0);
/// A HALT not yet looked at in thread context; `health::check` takes it
pub(super) static HALT_PENDING: AtomicBool = AtomicBool::new(false);
/// Sources nothing here handles: masked and cleared as they arrive, and
/// remembered for `nvidia intr`
static OTHER: AtomicU64 = AtomicU64::new(0);
static OTHER_BITS: AtomicU32 = AtomicU32::new(0);
/// `RISCV_FAULT_CONTAINMENT_SRCSTAT` when the fatal error arrived
pub(super) static FATAL_CODE: AtomicU32 = AtomicU32::new(0);

/// What the interrupt has seen, for `nvidia intr`
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub serviced: u64,
    pub spurious: u64,
    pub halts: u32,
    pub other: u64,
    pub other_bits: u32,
}

pub fn stats() -> Stats {
    Stats {
        serviced: SERVICED.load(Relaxed),
        spurious: SPURIOUS.load(Relaxed),
        halts: HALTS.load(Relaxed),
        other: OTHER.load(Relaxed),
        other_bits: OTHER_BITS.load(Relaxed),
    }
}

/// The falcon's own view, for `nvidia intr`: pending (IRQSTAT), enabled by the
/// core (RISCV_IRQMASK), and routed to the host (RISCV_IRQDEST). What reaches
/// the tree is the three ANDed
pub fn registers(bar0: &MmioRegion, regs: &GspRegs) -> (u32, u32, u32) {
    (
        bar0.read32(regs.falcon + FALCON_IRQSTAT),
        bar0.read32(regs.riscv + RISCV_IRQMASK),
        bar0.read32(regs.riscv + RISCV_IRQDEST),
    )
}

/// Have the falcon send its pending interrupt again
/// (`kflcnIntrRetrigger_GA100`).
///
/// The falcon raises its interrupt once, as a message the tree latches in a
/// leaf. What stands in IRQSTAT afterwards stays silent until this is written.
/// `service` ends with it, and opening the GSP's vector needs it too:
/// everything raised during boot was latched during set-up, which clears the
/// leaves
pub fn retrigger(bar0: &MmioRegion, regs: &GspRegs) {
    bar0.write32(regs.falcon + FALCON_INTR_RETRIGGER, 1);
}

/// The interrupt. `regs` says where this chip keeps the falcon
pub fn service(bar0: &MmioRegion, regs: &GspRegs) {
    let irqstat = regs.falcon + FALCON_IRQSTAT;
    let irqsclr = regs.falcon + FALCON_IRQSCLR;

    // `kflcnRiscvReadIntrStatus_GA102`: pending, enabled by the core, and
    // routed to the host
    let pending = bar0.read32(irqstat)
        & bar0.read32(regs.riscv + RISCV_IRQMASK)
        & bar0.read32(regs.riscv + RISCV_IRQDEST);
    if pending == 0 {
        SPURIOUS.fetch_add(1, Relaxed);
        return;
    }
    SERVICED.fetch_add(1, Relaxed);
    let mut handled = 0;

    // Cleared before it is looked at, so a second HALT for the next step of the
    // protocol is not lost
    if pending & IRQ_HALT != 0 {
        bar0.write32(irqsclr, IRQ_HALT);
        HALTS.fetch_add(1, Relaxed);
        HALT_PENDING.store(true, Relaxed);
        handled |= IRQ_HALT;
        super::worker::kick();
    }

    // Edge-triggered: cleared before, never after, or a message in between is
    // slept through
    if pending & FALCON_IRQ_SWGEN0 != 0 {
        bar0.write32(irqsclr, FALCON_IRQ_SWGEN0);
        rpc::note_msgq_interrupt();
        super::worker::kick();
        handled |= FALCON_IRQ_SWGEN0;
        // In lockdown the GSP's registers are off limits, including those
        // below: OpenRM returns here, with no retrigger
        if super::in_lockdown() {
            return;
        }
    }

    // `kgspServiceFatalHwError_GB100`. Not cleared: it cannot be without an
    // engine reset
    if regs.fatal_error != 0 && pending & regs.fatal_error != 0 {
        FATAL_CODE.store(bar0.read32(regs.fault_srcstat), Relaxed);
        super::FATAL.store(true, core::sync::atomic::Ordering::Release);
        handled |= regs.fatal_error;
        super::worker::kick();
    }

    // Anything else is disabled before it is acknowledged, so an unknown source
    // cannot come straight back
    let other = pending & !handled;
    if other != 0 {
        bar0.write32(regs.falcon + FALCON_IRQMCLR, other);
        bar0.write32(irqsclr, other);
        OTHER.fetch_add(1, Relaxed);
        OTHER_BITS.fetch_or(other, Relaxed);
    }

    // The tree above the falcon latches, and a handler stopping after the
    // acknowledge gets one interrupt ever. Not after a fatal error: it cannot
    // be cleared, and retriggering it is an interrupt storm
    if !super::fatal() {
        retrigger(bar0, regs);
    }
}
