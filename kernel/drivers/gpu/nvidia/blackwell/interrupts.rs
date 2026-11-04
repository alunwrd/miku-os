// What this card's interrupt means
//
// `nvidia::intr` owns the tree and the MSI; this knows which engines are in it
// and what to do when one speaks:
//
//   GSP  - the RISC-V core's falcon raising SWGEN0, its way of saying "there
//          is something in the message queue".
//   DISP - the front end's per-head timing event, once per frame, which the
//          compositor's frame loop waits on.
//
// Which bit either lands on is not fixed: GSP-RM assigns the vectors at boot,
// `gsp_common::intr` asks for the assignment, and `gsp_load` and `disp_load`
// below register the handlers, the GSP's as the first engine comes up, the
// display's (DISP, and DISP_LOW beside it) as the display does.
//
// The order in `open`: a vector is registered before it is allowed, and nothing
// without a handler is allowed. `nointr` on the GRUB command line stops at
// `intr::enable` and leaves every polling path.

use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::drivers::gpu::nvidia::gsp_common::{intr as gspintr, kgsp as gspkgsp, modeset, rpc};
use crate::drivers::gpu::nvidia::intr;

/// What this file's lines are signed with when the engine loader hands it the
/// boot's `say` (`nvidia::speaking`, `dev/klog.md`)
pub(super) static SITE: crate::kcore::klog::Site = crate::klog_site!();
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::gpu::nvidia::pci::GpuDevice;
use crate::drivers::gpu::nvidia::scanout;

use super::regs::{PGSP_FALCON2_BASE, PGSP_FALCON_BASE};

/// The head whose vblank paces the compositor. Read from interrupt context, so
/// an atomic: `LiveScanout` is behind a `spin::Mutex`
static VBLANK_HEAD: AtomicU32 = AtomicU32::new(NO_HEAD);
const NO_HEAD: u32 = u32::MAX;

/// Vblank events acknowledged for a head nothing is pacing on. Not an error: a
/// second head could be scanning
static VBLANK_OTHER: AtomicU64 = AtomicU64::new(0);

/// `(vblanks on heads nothing is pacing on, DISP_LOW interrupts)`. The GSP
/// falcon's own counts are `kgsp::service::stats`
pub fn stats() -> (u64, u64) {
    (VBLANK_OTHER.load(Ordering::Relaxed), DISP_LOW_SERVED.load(Ordering::Relaxed))
}

/// Where a GB20x keeps the GSP's registers. The fatal-error bit and the
/// fault-containment status are GB100's (`gb100/dev_gsp.h:68-73`), what every
/// GB20x resolves `kgspServiceFatalHwError` to (`g_kernel_gsp_nvoc.c:916-919`)
pub const GSP_REGS: gspkgsp::GspRegs = gspkgsp::GspRegs {
    falcon: PGSP_FALCON_BASE,
    riscv: PGSP_FALCON2_BASE,
    fatal_error: 1 << 24,
    fault_srcstat: 0x0011_1700,
};

/// The GSP falcon spoke: `kgsp::service`, over this chip's registers
fn gsp_service(bar0: &MmioRegion) {
    gspkgsp::service::service(bar0, &GSP_REGS);
}

/// A head finished a frame.
///
/// Every event on every pending head is acknowledged, whether or not anything
/// waits (see `head_timing_take`). Only a vblank on the head scanning the
/// compositor's surface advances the frame counter
fn disp_service(bar0: &MmioRegion) {
    let pending = modeset::vblank_pending(bar0);
    if pending == 0 {
        return;
    }
    let ours = VBLANK_HEAD.load(Ordering::Relaxed);
    for head in 0..8u32 {
        if pending & (1 << head) == 0 {
            continue;
        }
        let stat = modeset::head_timing_take(bar0, head);
        if stat & modeset::FE_EVT_HEAD_TIMING_LAST_DATA == 0 {
            continue;
        }
        if head == ours {
            scanout::note_vblank();
        } else {
            VBLANK_OTHER.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The shape of every GB20x's tree. OpenRM gives everything past Ada the GH100
/// interrupt functions (`g_intr_nvoc.c:394-412`)
pub const GEOMETRY: intr::Geometry = intr::Geometry::GH100;

/// GSP-RM's interrupt table, kept between the two halves below: the GSP's
/// vector is opened when RM comes up, the display's when the display does
static TABLE: spin::Mutex<Option<gspintr::KernelTable>> = spin::Mutex::new(None);

/// Interrupts that arrived on DISP_LOW, the display's low-latency line. On a
/// GB20x OpenRM services it like DISP (`kdispServiceInterrupt`,
/// `kern_disp.c:1121`); this shows whether anything uses it
static DISP_LOW_SERVED: AtomicU64 = AtomicU64::new(0);

/// The interrupt tree and the GSP's own vector: the first engine up once RM
/// answers, so every RPC after it can sleep while it waits.
///
/// Not fatal in any failure, and `nointr` is not a failure: everything works by
/// polling
pub fn gsp_load(
    bar0: &MmioRegion,
    gpu: &GpuDevice,
    rpc: &mut rpc::GspRpc,
    say: &dyn Fn(&str),
) -> Result<(), &'static str> {
    let internal = super::chanmem::internal_client()
        .ok_or("no internal RM handles - GSP_INIT did not answer")?;
    let table = match gspintr::kernel_table(rpc, &internal) {
        Ok(t) => t,
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("INTR: INTR_GET_KERNEL_TABLE failed: {:?}", e));
            return Err("GSP-RM's interrupt table did not come back");
        }
    };
    let gsp_vec = table.stall_vector(gspintr::MC_ENGINE_IDX_GSP, GEOMETRY);
    let disp_vec = table.stall_vector(gspintr::MC_ENGINE_IDX_DISP, GEOMETRY);
    let disp_low_vec = table.stall_vector(gspintr::MC_ENGINE_IDX_DISP_LOW, GEOMETRY);
    say(&alloc::format!(
        "INTR: {} vectors from GSP-RM; gsp {:?} disp {:?} disp_low {:?}",
        table.entries.len(), gsp_vec, disp_vec, disp_low_vec
    ));
    let nonstall = table.nonstall_subtrees();
    *TABLE.lock() = Some(table);

    let vector = match intr::enable(gpu, *bar0, GEOMETRY, nonstall) {
        Ok(v) => v,
        Err(e) => {
            say(&alloc::format!("INTR: not enabled ({}) - polling stays", e));
            return Ok(());
        }
    };
    say(&alloc::format!(
        "INTR: MSI vector {:#x}, {} leaves, subtrees {:#x} armed ({:#x} non-stall left off), every leaf blocked",
        vector, GEOMETRY.leaves, GEOMETRY.subtree_mask() & !nonstall, nonstall
    ));
    if open(gsp_vec, gsp_service, "GSP messages", say) {
        // Everything GSP-RM raised while the boot polled was latched by the
        // tree before `intr::enable` and `allow` cleared the leaves, and it
        // stays pending in the falcon, which sends nothing new until told. One
        // retrigger hands what is pending to the open vector
        let (stat, mask, dest) = gspkgsp::service::registers(bar0, &GSP_REGS);
        gspkgsp::service::retrigger(bar0, &GSP_REGS);
        say(&alloc::format!(
            "INTR: GSP falcon irqstat {:#x} mask {:#x} dest {:#x} - retriggered", stat, mask, dest
        ));
    }
    Ok(())
}

/// The display's vectors, DISP and DISP_LOW beside it, and the vblank of the
/// head pacing the compositor. The last thing the display engine does as it
/// comes up
pub fn disp_load(bar0: &MmioRegion, say: &dyn Fn(&str)) -> Result<(), &'static str> {
    if !intr::active() {
        say("INTR: the tree is off - the display stays polling");
        return Ok(());
    }
    let (disp_vec, disp_low_vec) = {
        let t = TABLE.lock();
        let t = t.as_ref().ok_or("no interrupt table - the GSP's interrupt did not come up")?;
        (
            t.stall_vector(gspintr::MC_ENGINE_IDX_DISP, GEOMETRY),
            t.stall_vector(gspintr::MC_ENGINE_IDX_DISP_LOW, GEOMETRY),
        )
    };

    // Which head the frame counter is about, before the display vector opens:
    // an event in between would be filed under "some other head"
    let head = super::window::live_scanout().map(|l| l.head);
    if let Some(h) = head {
        VBLANK_HEAD.store(h, Ordering::Release);
    }
    open(disp_vec, disp_service, "display", say);
    open(disp_low_vec, disp_low_service, "display, low latency", say);

    // The head's own event, a separate switch from the tree bit: the DISP
    // vector can be allowed and never fire until a head is told to raise it
    if disp_vec.is_some() {
        match head {
            Some(h) => {
                modeset::vblank_enable(bar0, h);
                say(&alloc::format!("INTR: vblank armed on head {}", h));
            }
            None => say("INTR: nothing is scanning out, so no head to take vblank from"),
        }
    }
    Ok(())
}

/// Register a handler, then let its vector through, in that order, one vector
/// at a time
fn open(vec: Option<u32>, handler: intr::Handler, what: &str, say: &dyn Fn(&str)) -> bool {
    let Some(v) = vec else {
        say(&alloc::format!("INTR: {} has no routable vector - left polling", what));
        return false;
    };
    if let Err(e) = intr::register(v, handler) {
        say(&alloc::format!("INTR: {} handler not registered: {}", what, e));
        return false;
    }
    match intr::allow(v) {
        Ok(()) => {
            say(&alloc::format!("INTR: {} on vector {}", what, v));
            true
        }
        Err(e) => {
            say(&alloc::format!("INTR: {} not allowed: {}", what, e));
            false
        }
    }
}

/// The GSP falcon's IRQSTAT, RISCV_IRQMASK and RISCV_IRQDEST, for `nvidia intr`
pub fn gsp_falcon_irq(bar0: &MmioRegion) -> (u32, u32, u32) {
    gspkgsp::service::registers(bar0, &GSP_REGS)
}

/// The compute or the copy engine released a fence and asked for the processor
/// (`NON_STALL_INTERRUPT` / `LAUNCH_DMA_INTERRUPT_TYPE_NON_BLOCKING`). Every
/// waiter looks at its own semaphore again (`gsp_common::chan`)
fn fence_service(_bar0: &MmioRegion) {
    crate::drivers::gpu::nvidia::gsp_common::chan::note_fence_interrupt();
}

/// Fences that wake their waiters (`dev/gpu-memory-compute.md`, 3.5a): the
/// non-stall vectors of GR0, the compute channel's engine, and of the copy
/// channel's engine, found by their interrupt-table index
/// (`ENGINE_INFO_TYPE_MC`), then the non-stall subtree armed. Both or neither:
/// a fence whose engine cannot wake anyone makes every wait on it a one-tick
/// sleep. `nononstall` on the boot line keeps it all off, and the pushbuffers
/// ask for no interrupt
pub fn fence_load(copy_engine_type: u32, say: &dyn Fn(&str)) {
    use crate::drivers::gpu::nvidia::gsp_common::chan;
    if crate::arch::x86_64::grub::cmdline_flag("nononstall") {
        say("INTR: 'nononstall' - fence waits spin, as before");
        return;
    }
    if !intr::active() {
        return;
    }
    let (gr, ce) = (chan::mc_index(chan::RM_ENGINE_TYPE_GR0), chan::mc_index(copy_engine_type));
    let (gr_vec, ce_vec) = {
        let t = TABLE.lock();
        let Some(t) = t.as_ref() else { return };
        (
            gr.and_then(|i| t.nonstall_vector(i, GEOMETRY)),
            ce.and_then(|i| t.nonstall_vector(i, GEOMETRY)),
        )
    };
    let (Some(gv), Some(cv)) = (gr_vec, ce_vec) else {
        say(&alloc::format!(
            "INTR: no non-stall vector for GR0 (engine {:?}: {:?}) or the copy engine (engine {:?}: {:?}) - fence waits spin",
            gr, gr_vec, ce, ce_vec
        ));
        return;
    };
    // The subtrees these two vectors are in, two leaves to a subtree, not the
    // category's mask, which OpenRM builds from the first subtree of a category
    // only (`intr.c:1723`)
    let nonstall = (1u32 << (gv / 64)) | (1u32 << (cv / 64));
    if !(open(Some(gv), fence_service, "compute fences", say) && open(Some(cv), fence_service, "copy fences", say)) {
        return;
    }
    intr::arm_also(nonstall);
    chan::set_fence_interrupts(true);
    say(&alloc::format!(
        "INTR: fences wake their waiters - GR0 (engine {}) on vector {}, copy engine {} on vector {}, subtree {:#x} armed",
        gr.unwrap_or(0), gv, ce.unwrap_or(0), cv, nonstall
    ));
}

/// DISP_LOW: counted, then handled as DISP is
fn disp_low_service(bar0: &MmioRegion) {
    DISP_LOW_SERVED.fetch_add(1, Ordering::Relaxed);
    disp_service(bar0);
}

/// The display's half of an unload: the head's vblank off. Its vectors close
/// with the rest of the tree (`gsp_unload`).
///
/// The tree bit stays until then, with the handler installed and acknowledging.
/// The front end stops raising events for a head that no longer means anything
pub fn disp_unload(bar0: &MmioRegion) {
    let head = VBLANK_HEAD.swap(NO_HEAD, Ordering::AcqRel);
    if head != NO_HEAD {
        modeset::vblank_disable(bar0, head);
    }
}

/// The GSP's half of an unload, and the tree's: every vector closed, the MSI
/// off and handed back (`intr::disable`), GSP-RM's interrupt table forgotten.
/// From here every wait polls
pub fn gsp_unload(gpu: &GpuDevice, say: &dyn Fn(&str)) {
    TABLE.lock().take();
    crate::drivers::gpu::nvidia::gsp_common::chan::set_fence_interrupts(false);
    if intr::disable(gpu) {
        say("INTR: tree unarmed, every leaf blocked, MSI off and the vector given back");
    }
}
