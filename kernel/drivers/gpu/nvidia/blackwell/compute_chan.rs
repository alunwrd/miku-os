// The compute channel: program upload, the blur/effects entry points, and the
// standalone `compute-test` command.



use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use crate::mm::dma::DmaBuffer;


/// The compute channel and the context buffers RM initialised for it, held
/// while the machine is up: the hardware walks those buffers whenever a program
/// runs
pub static COMPUTE: spin::Mutex<Option<(
    crate::drivers::gpu::nvidia::gsp_common::chan::Channel,
    crate::drivers::gpu::nvidia::gsp_common::compute::ComputeCtx,
)>> = spin::Mutex::new(None);

// The channel's ring, pushbuffer and semaphore live in the window its channel
// id picks (`chanmem::gpfifo_va` and friends). The context buffers run from
// `CTX_BASE_VA` to `CTX_LIMIT_VA`; how large they get is RM's answer.
use crate::drivers::gpu::nvidia::gsp_common::compute;
use crate::drivers::gpu::nvidia::gmmu;
use crate::drivers::gpu::nvidia::vaspace;
use super::chanmem::{CHAN_INST_COMPUTE, GPFIFO_BYTES, PUSHBUF_BYTES, SEMA_BYTES, gpfifo_va, pushbuf_va, sema_va};


/// Blur and blend every effect window, after the backdrop is down.
///
/// Returns the compute channel's semaphore and the value it will reach, or
/// `None` if there was nothing to do or it could not be done; the frame is then
/// what the copy engine put down, unblurred opaque windows
pub fn effects_submit(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    surface: (u32, u32),
    windows: &[crate::drivers::gpu::nvidia::blur::Window],
    after: Option<(u64, u32)>,
) -> Option<(u64, u32)> {
    use crate::drivers::gpu::nvidia::blur;

    if windows.is_empty() {
        return None;
    }
    let pdb = *CHAN_PDB_COMPUTE.lock();
    let guard = PROGRAMS.lock();
    let programs = guard.as_ref()?;
    let mut live = COMPUTE.lock();
    let (chan, ctx) = live.as_mut()?;
    if !chan.usable() {
        return None;
    }

    let fence = blur::submit(
        chan, bar0, pdb, programs, ctx.sass_version, surface, windows, after,
    ).ok()?;
    Some((chan.sema_va(), fence))
}

/// Bring the shell below the windows up to date, and blur it when `blur` names
/// a radius (`blur::submit_base`), after `after`.
///
/// The reason when it could not be done, for the compositor to say once:
/// nothing is folded that frame and the next starts from the whole screen
pub fn effects_base(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    layers: &[crate::drivers::gpu::nvidia::blur::Window],
    ops: &[crate::drivers::gpu::nvidia::blur::BaseOp],
    blur: Option<f32>,
    after: Option<(u64, u32)>,
) -> Result<(u64, u32), &'static str> {
    use crate::drivers::gpu::nvidia::blur;

    let pdb = *CHAN_PDB_COMPUTE.lock();
    let guard = PROGRAMS.lock();
    let programs = guard.as_ref().ok_or("no programs are loaded")?;
    let mut live = COMPUTE.lock();
    let (chan, ctx) = live.as_mut().ok_or("there is no compute channel")?;
    if !chan.usable() {
        return Err("the compute channel is not usable");
    }
    let fence = blur::submit_base(
        chan, bar0, pdb, programs, ctx.sass_version, layers, ops, blur, after,
    )?;
    Ok((chan.sema_va(), fence))
}

/// Map what the effects pass will read this frame, before the frame starts.
///
/// Apart from `effects_submit` because of where it happens: mapping is
/// page-table writes and a TLB invalidate that waits on the MMU, idle here and
/// busy once the backdrop is queued
pub fn effects_map(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    windows: &[crate::drivers::gpu::nvidia::blur::Window],
) -> bool {
    if windows.is_empty() {
        return true;
    }
    let pdb = *CHAN_PDB_COMPUTE.lock();
    crate::drivers::gpu::nvidia::blur::map_windows(bar0, pdb, windows).is_ok()
}

/// Say why a dispatch was refused, on the console for the first few and on the
/// serial port for ever after.
///
/// A client looping on a refused dispatch would otherwise push everything else
/// off the screen
fn refused(why: &str) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static SEEN: AtomicU32 = AtomicU32::new(0);
    const ON_SCREEN: u32 = 4;

    let n = SEEN.fetch_add(1, Ordering::Relaxed);
    crate::kwarn!("[nvidia] dispatch refused: {}", why);
    if n < ON_SCREEN {
        crate::println!("  nvidia: dispatch refused {}", why);
    } else if n == ON_SCREEN {
        crate::println!("  nvidia: further dispatch refusals go to the serial port only");
    }
}

/// Take a physical range out of the channel's address space: memory shown to
/// the effects pass is reachable by the GPU until this runs, and it should run
/// when the memory stops being the client's
pub fn effects_unbind(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    phys: u64,
    bytes: u64,
    aper: gmmu::Aperture,
) -> usize {
    let pdb = *CHAN_PDB_COMPUTE.lock();
    if pdb == 0 {
        return 0;
    }
    // A client's pool is system memory; the compositor's copy of a window is
    // video memory (`mayland::render::texture`). The caller says which, and
    // only mappings of that memory come out
    crate::drivers::gpu::nvidia::vaspace::unmap_phys(bar0, phys, bytes, aper)
}

/// Point the effects pass at the surface being drawn into. Apart from
/// submitting: the surface changes only on a flip, and the mapping is the
/// expensive half
pub fn effects_retarget(vram: u64, stride_bytes: u32) -> bool {
    crate::drivers::gpu::nvidia::blur::retarget(vram, stride_bytes).is_ok()
}

/// Wait out whatever the compute engine was last given
pub fn compute_wait_last(timeout_us: u64) -> bool {
    // Waited for outside the lock, as the copy engine's is (`ce_wait_last`)
    let Some(h) = COMPUTE.lock().as_ref().and_then(|(c, _)| c.last_handle()) else { return true };
    h.wait(timeout_us).is_ok()
}

/// Build a compute channel in an address space, and hand back the pieces.
///
/// There is more than one of these: a process loading its own programs gets a
/// channel in an address space of its own. The builds differ only in which page
/// directory, which instance block, and whether the graphics context is the
/// parked one
fn build_compute(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    internal: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    vas: &crate::drivers::gpu::nvidia::gsp_common::chan::ChanVaSpace,
    mthdbuf_size: u64,
    inst: u64,
    group: Option<u32>,
    own_context: bool,
    say: &dyn Fn(&str),
) -> Option<Built> {
    use crate::drivers::gpu::nvidia::gmmu;
    use crate::drivers::gpu::nvidia::vaspace;
    use crate::drivers::gpu::nvidia::gsp_common::compute::{self, ChanMem};

    if inst == 0 {
        say("compute: no video memory carved for the instance block");
        return None;
    }

    // Owned here until the channel exists to own them; see
    // `Channel::own_buffers`
    let mut bufs: alloc::vec::Vec<DmaBuffer> = alloc::vec::Vec::new();
    let mut take = |pages: usize| -> Option<u64> {
        let mut b = DmaBuffer::alloc(pages).ok()?;
        b.zero();
        let phys = b.phys();
        bufs.push(b);
        Some(phys)
    };
    let (Some(gpfifo), Some(pushbuf), Some(sema), Some(mthdbuf)) = (
        take((GPFIFO_BYTES / 4096) as usize),
        take((PUSHBUF_BYTES / 4096) as usize),
        take((SEMA_BYTES / 4096) as usize),
        take(mthdbuf_size.next_multiple_of(4096) as usize / 4096),
    ) else {
        say("compute: not enough system memory for the channel's buffers");
        return None;
    };
    DmaBuffer::write_barrier();

    // Its own window, picked by its own channel id: the lowest free one. A
    // rebuild with a different id and handle fails context promotion the same
    // way (`NV_ERR_OBJECT_NOT_FOUND`). The kernel's until a process's build
    // says otherwise (`client_own_gpu`)
    let Some(chid) = crate::drivers::gpu::nvidia::gsp_common::chan::alloc_chid(
        crate::drivers::gpu::nvidia::gsp_common::chan::Owner::Compute,
    ) else {
        say("compute: no channel id is free");
        return None;
    };
    let (c_gpfifo_va, c_pushbuf_va, c_sema_va) =
        (gpfifo_va(chid), pushbuf_va(chid), sema_va(chid));

    for (what, va, phys, bytes) in [
        ("gpfifo", c_gpfifo_va, gpfifo, GPFIFO_BYTES),
        ("pushbuf", c_pushbuf_va, pushbuf, PUSHBUF_BYTES),
        ("semaphore", c_sema_va, sema, SEMA_BYTES),
    ] {
        if let Err(e) = vaspace::map(bar0, vas.pdb, va, phys, bytes, gmmu::Aperture::Sysmem) {
            said(Level::Error, &say, &alloc::format!("compute: mapping the {} failed: {}", what, e));
            crate::drivers::gpu::nvidia::gsp_common::chan::free_chid(chid);
            return None;
        }
    }

    let mem = ChanMem {
        // The group this build is to join, if one is being kept alive
        group,
        own_context,
        chid,
        inst,
        mthdbuf,
        mthdbuf_size,
        gpfifo: (gpfifo, c_gpfifo_va, GPFIFO_BYTES),
        pushbuf: (pushbuf, c_pushbuf_va, PUSHBUF_BYTES),
        sema: (sema, c_sema_va, SEMA_BYTES),
    };
    // A compute channel that will not come back costs only the effects pass
    // until the next boot: the copy engine, the console, the display and every
    // client blit are on the other channel. See `dev/gb206-display.md` section
    // 12: a second promotion of the graphics context is refused
    let Some(mut pair) = compute::bring_up(rpc, client, internal, bar0, vas, &mem, say) else {
        // The id and the mappings were taken before the bring-up and go back
        // when it fails, or the card loses a channel id for the boot
        for va in [c_gpfifo_va, c_pushbuf_va, c_sema_va] {
            vaspace::unmap(bar0, vas.pdb, va);
        }
        crate::drivers::gpu::nvidia::gsp_common::chan::free_chid(chid);

        // The inherited group goes too: RM refuses another channel in a group
        // whose channel could not be built (`NV_ERR_INVALID_STATE`). Letting it
        // go makes the next attempt a first-time build, so one bad round costs
        // one context
        if let Some(g) = COMPUTE_GROUP.lock().take() {
            match client.free(rpc, g) {
                Ok(()) => say(&alloc::format!(
                    "compute: gave group {:#x} back, the next build starts fresh", g)),
                Err(e) => say(&alloc::format!(
                    "compute: RM would not take group {:#x} back: {:?}", g, e)),
            }
        }
        say("compute: the effects pass is unavailable until the next boot, \
             everything else is on the copy engine and unaffected");
        return None;
    };
    say("compute: the channel is up, a program can be launched on it");
    pair.0.own_buffers(core::mem::take(&mut bufs));

    // Everything that will be launched, uploaded and mapped now. The addresses
    // are the same numbers in every space, since the spaces are independent
    let programs = match compute::Programs::load(bar0, vas.pdb, say) {
        Ok(p) => p,
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("compute: loading the programs failed: {}", e));
            return None;
        }
    };
    Some(Built { chan: pair.0, ctx: pair.1, programs })
}

/// What a finished compute channel consists of
struct Built {
    chan: crate::drivers::gpu::nvidia::gsp_common::chan::Channel,
    ctx: crate::drivers::gpu::nvidia::gsp_common::compute::ComputeCtx,
    programs: crate::drivers::gpu::nvidia::gsp_common::compute::Programs,
}

/// Build the kernel's own compute channel, the one the desktop's effects run on
pub fn bring_up_compute(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    internal: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    vas: &crate::drivers::gpu::nvidia::gsp_common::chan::ChanVaSpace,
    mthdbuf_size: u64,
    say: &dyn Fn(&str),
) {
    let inst = *CHAN_INST_COMPUTE.lock();
    let group = *COMPUTE_GROUP.lock();
    // The parked context, not a fresh one: its buffers are mapped in the
    // kernel's page directory, the space this channel runs in
    let Some(built) = build_compute(
        bar0, rpc, client, internal, vas, mthdbuf_size, inst, group, false, say,
    ) else {
        return;
    };
    let mut chan = built.chan;
    super::channel::attach_userd(&mut chan);
    *COMPUTE.lock() = Some((chan, built.ctx));
    *PROGRAMS.lock() = Some(built.programs);
    *CHAN_PDB_COMPUTE.lock() = vas.pdb;
}

/// Which build of the kernel's compute channel is live. Changes with every
/// rebuild, which tells the compositor one it gave up on has come back
pub fn compute_epoch() -> Option<u32> {
    COMPUTE.lock().as_ref().map(|(c, _)| c.epoch())
}

/// A process's own channel died: mark it, so its dispatches are refused and not
/// queued behind a ring nothing runs, and say whose it was. OpenRM tells the
/// owner through its error notifier (`krcErrorSetNotifier`); here the owner
/// learns from the refusal
pub fn note_client_died(chid: u32) -> Option<u64> {
    let mut clients = CLIENTS.lock();
    let c = clients.iter_mut().find(|c| c.chan.chid == chid)?;
    c.chan.mark_failed(crate::drivers::gpu::nvidia::gsp_common::chan::Dead::ClientKilled);
    Some(c.pid)
}

/// Rebuilds of the kernel's compute channel this boot, and when the last one
/// ran
struct Recovery {
    count: u32,
    last_ms: u64,
    stopped: bool,
}

static RECOVERY: spin::Mutex<Recovery> = spin::Mutex::new(Recovery { count: 0, last_ms: 0, stopped: false });

/// At most this many rebuilds a boot: a fault coming back after every rebuild
/// is not fixed by one, and each costs the desktop its effects for a moment
const MAX_RECOVERIES: u32 = 4;
/// And not sooner than this after the last one
const RECOVERY_GAP_MS: u64 = 2_000;

/// RC recovery for the kernel's compute channel (`dev/gpu-memory-compute.md`,
/// 3.6).
///
/// A channel GSP-RM has killed does not come back by being submitted to: OpenRM
/// leaves it dead, and its client frees it and makes another
/// (`krcErrorSetNotifier`, then `NV_ERR_RESET_REQUIRED` on the next use). This
/// channel's client is the kernel: the teardown and bring-up of `chan-cycle`,
/// run from the event thread, with the compositor already off the engine
/// (`give_up_on_compute`, taken back when it sees the channel's epoch change).
///
/// Only the compute channel: the copy engine's puts every frame on the panel,
/// and a process reaches it only through a checked rectangle. Called on every
/// turn of `poll_events`; cheap when the channel is alive
pub fn recover_if_dead() {
    let dead = COMPUTE.lock().as_ref().filter(|(c, _)| c.ring_attached() && !c.usable()).map(|(c, _)| c.dead_why());
    let Some(why) = dead else { return };
    if crate::drivers::gpu::nvidia::gsp_common::kgsp::fatal() {
        return;
    }
    let why = why.map_or("unusable", |w| w.describe());
    let now = crate::kcore::time::since_boot_ms();
    {
        let mut r = RECOVERY.lock();
        if r.stopped {
            return;
        }
        if r.count >= MAX_RECOVERIES {
            r.stopped = true;
            crate::kerror!("[nvidia] the compute channel died again after {} rebuilds - leaving it dead; \
                the desktop keeps working without GPU effects until the next boot", r.count);
            return;
        }
        if r.count > 0 && now.saturating_sub(r.last_ms) < RECOVERY_GAP_MS {
            return;
        }
        r.count += 1;
        r.last_ms = now;
    }
    let who = LAST_CLIENT.swap(0, core::sync::atomic::Ordering::Relaxed);
    crate::kwarn!("[nvidia] the compute channel is dead ({}){} - rebuilding it", why,
        if who != 0 { alloc::format!(" - the last dispatch on it was pid {}'s", who) } else { alloc::string::String::new() });

    let log = |l: &str| crate::kdebug!("[nvidia] recovery: {}", l);
    let rebuilt = super::control::with_rm(|bar0, rpc, client| {
        // Whatever waits on its fence (the copy engine's next batch, a window's
        // semaphore) is let go first: nothing will release it now
        if let Some((c, _)) = COMPUTE.lock().as_mut() {
            c.force_fence();
        }
        compute_destroy(bar0, rpc, client, &log);
        let pdb = *super::chanmem::CHAN_PDB.lock();
        let Some(internal) = super::chanmem::internal_client() else {
            return Err("GSP-RM's own client is not known");
        };
        let mthdbuf = crate::drivers::gpu::nvidia::gsp_common::chan::method_buffer_size(rpc, &internal)
            .map_err(|_| "RM would not say how large a method buffer is")?;
        let vas = ChanVaSpace { handle: crate::drivers::gpu::nvidia::gsp_common::chan::H_VASPACE, pdb };
        bring_up_compute(bar0, rpc, client, &internal, &vas, mthdbuf, &log);
        if compute_ready() { Ok(()) } else { Err("the channel did not come back") }
    });
    match rebuilt {
        Ok(Ok(())) => crate::knotice!("[nvidia] compute channel rebuilt (epoch {}) - GPU effects can come back",
            compute_epoch().unwrap_or(0)),
        Ok(Err(e)) | Err(e) => crate::kerror!("[nvidia] rebuilding the compute channel failed: {} - \
            'nvidia log chan debug' has every step", e),
    }
}

// A process with its own GPU
//
// A client running only the kernel's programs needs nothing of its own: it
// names memory by pool handle, the kernel resolves every address, and it shares
// the kernel's channel.
//
// A client bringing its own machine code is different: in a shared address
// space its program can read the console's shadow buffer, another client's pool
// and the compositor's surfaces, and a shader cannot be checked as a rectangle
// can. So at `gpu_program` a process gets an address space of its own, a
// channel in it, a graphics context of its own, and its own copy of every
// program.
//
// A graphics context is megabytes, plus a channel id, an instance block and a
// page directory in video memory, so this is not the default. A CUDA context
// per process is why MPS exists on Linux.

use crate::drivers::gpu::nvidia::gsp_common::chan::ChanVaSpace;

/// Everything one process owns on the GPU
pub struct ClientGpu {
    pid: u64,
    /// Video memory, both freed when the process exits
    pdb: u64,
    inst: u64,
    vas: ChanVaSpace,
    chan: crate::drivers::gpu::nvidia::gsp_common::chan::Channel,
    ctx: crate::drivers::gpu::nvidia::gsp_common::compute::ComputeCtx,
    programs: crate::drivers::gpu::nvidia::gsp_common::compute::Programs,
}

/// BAR1 has just been bound physical: the compute channel and every process's
/// own channel get their USERD windows (`channel::attach_userd`)
pub(super) fn attach_userd_compute() {
    if let Some((c, _)) = COMPUTE.lock().as_mut() {
        super::channel::attach_userd(c);
    }
    for g in CLIENTS.lock().iter_mut() {
        super::channel::attach_userd(&mut g.chan);
    }
}

/// How many processes may have their own GPU at once. Four: each is a graphics
/// context of several megabytes and a channel id, and a refusal naming the
/// limit beats running out of video memory
pub const MAX_CLIENT_GPUS: usize = 4;

pub(super) static CLIENTS: spin::Mutex<alloc::vec::Vec<ClientGpu>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// A different address-space handle for every private GPU ever built
static VASPACE_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Every process the GPU is doing something for: one with a GPU of its own, or
/// one that loaded a program into the kernel's. An unload refuses while there
/// are any
pub(super) fn users() -> alloc::vec::Vec<u64> {
    let mut out: alloc::vec::Vec<u64> = CLIENTS.lock().iter().map(|c| c.pid).collect();
    if let Some(p) = PROGRAMS.lock().as_ref() {
        out.extend(p.owners());
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// GSP-RM was unloaded, after `compute_destroy` took the channel down: the
/// group carrying the graphics context, the address space the programs were
/// mapped into, and the context itself belonged to it
pub(super) fn forget() {
    COMPUTE.lock().take();
    PROGRAMS.lock().take();
    COMPUTE_GROUP.lock().take();
    *CHAN_PDB_COMPUTE.lock() = 0;
    crate::drivers::gpu::nvidia::gsp_common::compute::forget();
}

/// Does this process have a GPU of its own?
pub fn client_has_own(pid: u64) -> bool {
    CLIENTS.lock().iter().any(|c| c.pid == pid)
}

/// Build a process its own address space, channel, context and programs.
///
/// Idempotent: a process that has one gets it back. Called only from
/// `gpu_program`, since loading machine code alone needs it
pub fn client_own_gpu(pid: u64, say: &dyn Fn(&str)) -> Result<(), &'static str> {
    use crate::drivers::gpu::nvidia::gsp_common::{chan, vram};

    if client_has_own(pid) {
        return Ok(());
    }
    // Off unless asked. The global context buffers belong to the engine, their
    // `bInitialized` flags live on `KernelGraphics`, and the kernel's boot has
    // set them. A private context's promotion is refused, or it re-initialises
    // them and the kernel's live context stops working, silencing the desktop's
    // effects and every client dispatch until the next boot.
    //
    // 3.3 of `dev/gpu-memory-compute.md` follows the reference, and not with
    // `localCtxBuffer`, which is for VPR channels only
    // (`kgrctxAllocCtxBuffers_IMPL`). A second context gets its own `MAIN` and
    // `PATCH`, initialised, and borrows the engine's global buffers: the
    // kernel's pages, mapped into this space, promoted by address with no
    // initialise half (`compute::allocate_process_context`). Not run on the
    // card yet, so it stays behind `gpuown`
    if !crate::arch::x86_64::grub::cmdline_flag("gpuown") {
        return Err("a private GPU address space is off by default, see `gpuown`");
    }
    if CLIENTS.lock().len() >= MAX_CLIENT_GPUS {
        return Err("as many processes have their own GPU as this machine allows");
    }

    // A page directory and an instance block, both in video memory, this
    // process's until it exits
    let pdb = vram::alloc(4 << 10, 4 << 10, vram::Placement::Low, "client page directory")
        .ok_or("no video memory for a page directory")?;
    let inst = vram::alloc(chan::CHAN_MEM_SIZE, 4 << 10, vram::Placement::Low, "client channel")
        .ok_or_else(|| {
            vram::free(pdb);
            "no video memory for an instance block"
        })?;

    let out = super::control::with_rm(|bar0, rpc, client| {
        // A page directory starts empty, and PRAMIN is the processor's only
        // path to video memory
        {
            let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
            for off in (0..4096u64).step_by(4) {
                w.write32(pdb + off, 0);
            }
        }
        // A fresh number each time: two pids sixteen bits apart would fold onto
        // one handle, and RM answers that with a duplicate
        let handle = chan::H_VASPACE_CLIENT
            | (VASPACE_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed) & 0xffff);
        let vas = match ChanVaSpace::create(rpc, client, pdb, handle) {
            Ok(v) => v,
            Err(e) => {
                said(Level::Warn, &say, &alloc::format!("gpu: a private address space was refused: {:?}", e));
                return Err("RM refused a private address space");
            }
        };
        crate::drivers::gpu::nvidia::vaspace::create(pdb, "process", &super::chanmem::layout());

        let internal = match super::chanmem::internal_client() {
            Some(i) => i,
            None => return Err("GSP-RM's own client is not known yet"),
        };
        // The question the boot asks, asked again: a property of the chip's
        // copy engines, a control away
        let mthdbuf = match chan::method_buffer_size(rpc, &internal) {
            Ok(n) => n,
            Err(_) => {
                vas.destroy(rpc, client);
                crate::drivers::gpu::nvidia::vaspace::destroy(bar0, pdb);
                return Err("RM would not say how large a method buffer is");
            }
        };
        // A group and a context of its own, shared with nothing of the kernel's
        let Some(built) = build_compute(
            bar0, rpc, client, &internal, &vas, mthdbuf, inst, None, true, say,
        ) else {
            vas.destroy(rpc, client);
            crate::drivers::gpu::nvidia::vaspace::destroy(bar0, pdb);
            return Err("the private compute channel did not come up");
        };
        Ok((vas, built))
    });

    let (vas, built) = match out {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            vram::free(inst);
            vram::free(pdb);
            return Err(e);
        }
        Err(e) => {
            vram::free(inst);
            vram::free(pdb);
            return Err(e);
        }
    };

    chan::set_owner(built.chan.chid, chan::Owner::Process(pid));
    let mut chan = built.chan;
    super::channel::attach_userd(&mut chan);
    CLIENTS.lock().push(ClientGpu {
        pid,
        pdb,
        inst,
        vas,
        chan,
        ctx: built.ctx,
        programs: built.programs,
    });
    say(&alloc::format!(
        "gpu: pid {} has its own address space, channel and context", pid));
    Ok(())
}

/// Give a process's GPU back when it exits.
///
/// The order of `dev/gb206-display.md` and `Channel::destroy`: nothing new, let
/// what is queued finish, off the runlist, RM frees the engine object and the
/// channel, and only then is the address space ours to change. The page
/// directory and the instance block are video memory and go last, after RM has
/// agreed the channel is gone.
///
/// Its group goes with it, unlike the kernel's: nothing builds a second channel
/// for an exited process
pub fn client_drop(pid: u64, say: &dyn Fn(&str)) {
    use crate::drivers::gpu::nvidia::vaspace;
    use crate::drivers::gpu::nvidia::gsp_common::{chan, compute, vram};

    let Some(c) = ({
        let mut t = CLIENTS.lock();
        t.iter().position(|c| c.pid == pid).map(|i| t.remove(i))
    }) else {
        return;
    };
    let ClientGpu { pid, pdb, inst, vas, chan: channel, ctx, programs } = c;

    let _ = super::control::with_rm(|bar0, rpc, client| {
        // The programs come out of the space before the channel goes:
        // afterwards the page directory is being taken apart
        for (va, bytes) in programs.mapped_ranges() {
            vaspace::unmap(bar0, pdb, va);
        }
        chan::retire_dma(programs.into_buffers());

        let engine = compute::H_COMPUTE | channel.chid;
        channel.destroy(rpc, client, bar0, pdb, engine, false, say);

        // The context buffers are this process's own, unlike the kernel's,
        // parked for the next channel
        for (va, _) in ctx.mapped.iter() {
            vaspace::unmap(bar0, pdb, *va);
        }
        chan::retire_dma(ctx.bufs);

        vas.destroy(rpc, client);
        // Whatever is still mapped (the context the loop above never saw, on
        // the legacy path) comes out, so its page tables come back before the
        // directory they hang off is freed
        let left = crate::drivers::gpu::nvidia::vaspace::destroy(bar0, pdb);
        if left > 0 {
            crate::kdebug!("[gpu/va] pid {}: {} mapping(s) were still in its space", pid, left);
        }
    });

    // Video memory last: RM has agreed the channel is gone, so nothing walks
    // this page directory or reads this instance block
    vram::free(inst);
    vram::free(pdb);
    say(&alloc::format!("gpu: pid {} gave its address space, channel and context back", pid));
}

/// The timeslice group the compute channel lives in, kept across a teardown.
///
/// RM's `KernelGraphicsContext` is this object's child, so it is what carries
/// the graphics context from one channel to the next
pub static COMPUTE_GROUP: spin::Mutex<Option<u32>> = spin::Mutex::new(None);

/// The programs, and the page directory their surfaces get mapped into
static PROGRAMS: spin::Mutex<Option<crate::drivers::gpu::nvidia::gsp_common::compute::Programs>> =
    spin::Mutex::new(None);
static CHAN_PDB_COMPUTE: spin::Mutex<u64> = spin::Mutex::new(0);

/// Set the blur pass up against the surface being scanned. Apart from the
/// channel bring-up since it needs the display running: the levels are sized
/// from the surface
pub fn blur_init(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    width: u32,
    height: u32,
    stride_bytes: u32,
    say: &dyn Fn(&str),
) -> bool {
    use crate::drivers::gpu::nvidia::blur;

    let pdb = *CHAN_PDB_COMPUTE.lock();
    let guard = PROGRAMS.lock();
    let Some(programs) = guard.as_ref() else {
        say("blur: no programs are loaded, the compute channel did not come up");
        return false;
    };
    // PROGRAMS then COMPUTE, the order `blur_surface` takes them in
    let idle = || COMPUTE.lock().as_ref().is_none_or(|(chan, _)| chan.wait_last(500_000).is_ok());
    match blur::init(bar0, pdb, programs, width, height, stride_bytes, &idle, say) {
        Ok(()) => true,
        Err(e) => {
            say(&alloc::format!("blur: {}", e));
            false
        }
    }
}

/// Blur the surface being drawn into, and wait for it. A command runs this; a
/// frame hands the fence on
pub fn blur_surface(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    vram: u64,
    stride_bytes: u32,
    width: u32,
    height: u32,
    radius: f32,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::blur;

    let pdb = *CHAN_PDB_COMPUTE.lock();
    let guard = PROGRAMS.lock();
    let Some(programs) = guard.as_ref() else {
        say("blur: no programs are loaded");
        return;
    };
    let mut live = COMPUTE.lock();
    let Some((chan, ctx)) = live.as_mut() else {
        say("blur: the compute channel is not up");
        return;
    };

    if let Err(e) = blur::retarget(vram, stride_bytes) {
        say(&alloc::format!("blur: {}", e));
        return;
    }

    let t0 = crate::kcore::time::rdtsc();
    let fence = match blur::submit_whole(
        chan, bar0, programs, ctx.sass_version, width, height, radius,
    ) {
        Ok(f) => f,
        Err(e) => {
            say(&alloc::format!("blur: {}", e));
            return;
        }
    };
    let submitted = crate::kcore::time::rdtsc();
    let done = chan.wait_fence(fence, 500_000).is_ok();
    let end = crate::kcore::time::rdtsc();

    let khz = crate::kcore::time::tsc_khz().max(1);
    let us = |a: u64, b: u64| b.wrapping_sub(a) * 1000 / khz;
    if done {
        say(&alloc::format!(
            "blur: {}x{}, {} passes, radius {} - {} us to submit, {} us on the GPU",
            width, height, blur::PASSES, radius as u32,
            us(t0, submitted), us(submitted, end)));
    } else {
        say("blur: the chain never finished");
        crate::drivers::gpu::nvidia::gmmu::dump_mmu_faults(bar0, say);
    }
}

/// Whether the compute engine came up and is still running work, for the shell
/// to report and for the frame to decide whether the effects pass is available
pub fn compute_ready() -> bool {
    COMPUTE.lock().as_ref().is_some_and(|(c, _)| c.usable())
}

/// Launch a minimal compute program and check its output.
///
/// `fill` writes `value + i` into a buffer. Correct values mean the compute
/// channel, the graphics context, the launch descriptor, the program upload and
/// the fence all worked; a failure in any shows as wrong or missing output
pub fn compute_test(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion, say: &dyn Fn(&str)) {
    use crate::drivers::gpu::nvidia::gmmu;
    use crate::drivers::gpu::nvidia::vaspace;
    use crate::drivers::gpu::nvidia::gsp_common::compute;

    let pdb = *CHAN_PDB_COMPUTE.lock();
    let guard = PROGRAMS.lock();
    let Some(programs) = guard.as_ref() else {
        say("compute: no programs are loaded, the channel did not come up");
        return;
    };
    let Some(fill) = programs.index("fill") else {
        say("compute: no 'fill' program was built (no CUDA toolkit at build time)");
        return;
    };
    let mut live = COMPUTE.lock();
    let Some((chan, ctx)) = live.as_mut() else {
        say("compute: the channel is not up");
        return;
    };

    // One page for the answer. The program, its constant bank and its
    // descriptor were mapped when the channel came up
    const ELEMENTS: u32 = 64;
    let Ok(mut scratch) = DmaBuffer::alloc(1) else {
        say("compute: not enough memory for the output buffer");
        return;
    };
    scratch.zero();
    let phys = scratch.phys();
    let out = crate::arch::x86_64::grub::phys_to_virt(phys) as *mut u8;
    // The channel keeps it: the engine reads this page, so it must outlive the
    // test
    chan.own_buffers(alloc::vec![scratch]);
    if let Err(e) = vaspace::map(bar0, pdb, compute::SCRATCH_VA, phys, 4096,
                               gmmu::Aperture::Sysmem) {
        said(Level::Error, &say, &alloc::format!("compute: mapping the output failed: {}", e));
        return;
    }

    // `fill(out, bytes, value, count)`: the page, its size, the base
    let mut params = alloc::vec::Vec::new();
    params.extend_from_slice(&compute::SCRATCH_VA.to_le_bytes());
    params.extend_from_slice(&4096u64.to_le_bytes());
    params.extend_from_slice(&0x1000u32.to_le_bytes());
    params.extend_from_slice(&ELEMENTS.to_le_bytes());

    let qmd = match programs.prepare(
        0, fill, &params, (ELEMENTS, 1, 1), (1, 1, 1), ctx.sass_version,
    ) {
        Ok(q) => q,
        Err(e) => {
            say(&alloc::format!("compute: {}", e));
            return;
        }
    };
    DmaBuffer::write_barrier();
    say(&alloc::format!("compute: launching 'fill', {} threads", ELEMENTS));

    match chan.submit_compute(bar0, qmd) {
        Ok(fence) => match chan.wait_fence(fence, 1_000_000) {
            Ok(()) => {
                let mut wrong = None;
                for i in 0..ELEMENTS {
                    let got = unsafe {
                        core::ptr::read_volatile(out.add(i as usize * 4) as *const u32)
                    };
                    if got != 0x1000 + i {
                        wrong = Some((i, got));
                        break;
                    }
                }
                match wrong {
                    None => say(&alloc::format!(
                        "compute: the GPU ran our program, {} values correct", ELEMENTS)),
                    Some((i, got)) => say(&alloc::format!(
                        "compute: element {} is {:#x}, expected {:#x}, the launch happened \
                         and the program did not do what it was asked",
                        i, got, 0x1000 + i)),
                }
            }
            Err(_) => {
                // A fence that never passes has three shapes: the host never
                // read the pushbuffer, the host read it and the engine faulted,
                // or the grid ran and the release did not. GP_GET separates the
                // first from the other two, and the fault buffer the second
                // from the third
                let (get, put) = chan.userd_gp(bar0);
                say(&alloc::format!(
                    "compute: the fence never passed, gpget {} gpput {}", get, put));
                gmmu::dump_mmu_faults(bar0, say);
                let head = unsafe { core::ptr::read_volatile(out as *const u32) };
                say(&alloc::format!(
                    "compute: the output buffer starts {:#x} (would be {:#x} if the grid ran)",
                    head, 0x1000));
            }
        },
        Err(_) => said(Level::Warn, &say, "compute: the dispatch was refused by the ring"),
    }
}

/// Bind a client's pool into the channel's address space, at the slot's
/// address.
///
/// For memory a ring-3 program owns. Returns the GPU address, the only thing
/// the client learns, to pass back as a parameter
pub fn client_bind(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    pid: u64,
    slot: usize,
    phys: u64,
    bytes: u64,
    // System memory for a pool of it, video memory for a `gpu_alloc` pool
    aperture: gmmu::Aperture,
) -> Option<u64> {
    let pdb = client_pdb(pid);
    if pdb == 0 || slot >= compute::MAX_CLIENT_BINDS || bytes > compute::CLIENT_VA_STRIDE {
        return None;
    }
    let va = compute::client_va(slot);
    vaspace::map(bar0, pdb, va, phys, bytes, aperture).ok()?;
    Some(va)
}

/// Which page directory a process's bindings go into: its own if it has one,
/// otherwise the kernel's. The slot numbers and addresses are the same either
/// way, since the spaces are independent
fn client_pdb(pid: u64) -> u64 {
    if let Some(c) = CLIENTS.lock().iter().find(|c| c.pid == pid) {
        return c.vas.pdb;
    }
    *CHAN_PDB_COMPUTE.lock()
}

/// Take a client's pool back out of the channel's address space
pub fn client_unbind(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    pid: u64,
    slot: usize,
) {
    let pdb = client_pdb(pid);
    if pdb == 0 || slot >= compute::MAX_CLIENT_BINDS {
        return;
    }
    vaspace::unmap(bar0, pdb, compute::client_va(slot));
}

/// One launch of a client's submission: a program, its parameters laid out as
/// the program's compiler expects (every GPU address resolved by the kernel
/// from a bind slot), and its grid
pub struct ClientLaunch<'a> {
    pub program: usize,
    pub params: &'a [u8],
    pub grid: (u32, u32, u32),
    pub block: (u32, u32, u32),
}

/// Run a chain of programs on memory a client has bound: in order, each
/// finished before the next starts, one fence for all. Returns
/// `(epoch, fence)`: the channel's build and the value its semaphore will reach
pub fn client_dispatch(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    pid: u64,
    launches: &[ClientLaunch<'_>],
) -> Result<(u32, u32), DispatchError> {
    // A process with its own machine code runs on its own channel, in its own
    // address space. One running only the kernel's programs shares the kernel's
    // channel, cheaper and as safe: the kernel resolved every address in that
    // dispatch
    {
        let mut clients = CLIENTS.lock();
        if let Some(c) = clients.iter_mut().find(|c| c.pid == pid) {
            return dispatch_on(bar0, &c.programs, &mut c.chan, c.ctx.sass_version, launches);
        }
    }
    // Every refusal says which one it was, where the person running the program
    // can see it. The first few go to the console and the rest to serial, so a
    // client in a loop cannot push everything else off it
    let guard = PROGRAMS.lock();
    let Some(programs) = guard.as_ref() else {
        refused("no programs are loaded");
        return Err(DispatchError::NoPrograms);
    };
    let mut live = COMPUTE.lock();
    let Some((chan, ctx)) = live.as_mut() else {
        refused("the compute channel is not up");
        return Err(DispatchError::NoChannel);
    };
    let sass = ctx.sass_version;
    LAST_CLIENT.store(pid, core::sync::atomic::Ordering::Relaxed);
    dispatch_on(bar0, programs, chan, sass, launches)
}

/// The last process that dispatched onto the kernel's channel, named when that
/// channel dies: a robust-channel report says which channel, not whose work
static LAST_CLIENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// One submission onto one channel, whichever channel that is
fn dispatch_on(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    programs: &compute::Programs,
    chan: &mut crate::drivers::gpu::nvidia::gsp_common::chan::Channel,
    sass_version: u32,
    launches: &[ClientLaunch<'_>],
) -> Result<(u32, u32), DispatchError> {
    if launches.is_empty() || launches.len() > compute::CLIENT_SLOTS {
        refused("a chain is one to sixteen launches");
        return Err(DispatchError::Prepare);
    }
    if !chan.usable() {
        let dead = chan.ring_attached();
        refused(if dead {
            "the compute channel is marked dead"
        } else {
            "the compute channel has no ring attached"
        });
        return Err(if dead { DispatchError::ChannelDead } else { DispatchError::NoRing });
    }
    // Every client shares the `CLIENT_SLOTS`, and `prepare` rewrites their
    // descriptors and constant banks in place. `submit_compute_chain` waits for
    // the channel's previous work only after `prepare` has run, so without this
    // a second process overwrites the parameters of a running grid. The
    // in-flight limit is per process. The same budget as the submit's own wait
    if chan.wait_last(500_000).is_err() {
        refused("the previous dispatch on the channel has not finished");
        return Err(DispatchError::Submit);
    }
    let mut qmds = alloc::vec::Vec::with_capacity(launches.len());
    for (i, l) in launches.iter().enumerate() {
        match programs.prepare(compute::CLIENT_SLOT + i, l.program, l.params, l.block, l.grid, sass_version) {
            Ok(q) => qmds.push(q),
            Err(e) => {
                refused(e);
                return Err(DispatchError::Prepare);
            }
        }
    }
    // Nothing to wait for: a client's dispatch is not part of the frame, so it
    // is not ordered against the copy engine
    match chan.submit_compute_chain(bar0, &qmds, None) {
        Ok(fence) => Ok((chan.epoch(), fence)),
        Err(e) => {
            refused(&alloc::format!("the submit failed ({:#x})", e));
            Err(DispatchError::Submit)
        }
    }
}

/// Why a dispatch was refused, surviving the trip to ring 3. Each becomes its
/// own errno, so the program can say which check refused it
#[derive(Clone, Copy)]
pub enum DispatchError {
    NoPrograms,
    NoChannel,
    ChannelDead,
    NoRing,
    Prepare,
    Submit,
}

/// Take the compute channel apart: the channel, its graphics context, and the
/// programs mapped alongside it.
///
/// `build_channel` brings the compute channel up too, so a cycle without this
/// leaks a channel id, three ring mappings, a graphics context of several
/// megabytes and a copy of every program, and RM refuses to promote a context
/// while the first holds it (`Status(31)`).
///
/// The programs go with it: `Programs::load` maps every kernel at a fixed
/// address and holds their pages, so a second set would map over the first and
/// drop its buffers while the GPU reaches them
pub fn compute_destroy(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    say: &dyn Fn(&str),
) -> bool {
    use crate::drivers::gpu::nvidia::gsp_common::chan;

    let pdb = *CHAN_PDB_COMPUTE.lock();
    let Some((chan_obj, ctx)) = COMPUTE.lock().take() else { return false };

    // The programs first: mapped in this address space, which the channel walks
    if let Some(progs) = PROGRAMS.lock().take() {
        for (va, bytes) in progs.mapped_ranges() {
            vaspace::unmap(bar0, pdb, va);
        }
        chan::retire_dma(progs.into_buffers());
    }

    // Then the channel. RM frees the engine object and the channel, preempts,
    // and clears the instance block; only then is the address space ours to
    // change
    let engine = compute::H_COMPUTE | chan_obj.chid;
    let chan_obj_group = chan_obj.group;
    // The group stays. It owns RM's `KernelGraphicsContext`
    // (`kernel_channel_group_api.c:408`), holding the main context buffer and
    // the flags saying it is initialised, so a rebuild into the same group has
    // nothing to promote.
    //
    // RM discards the engine context descriptor with the channel
    // (`kgrctxUnmapMainCtxBuffer`, `kernel_graphics_context.c:2695`): on the
    // hardware side, the pointer at 0x210 in the channel's instance block. The
    // new channel's instance block gets it from `Channel::bind_gr_context`,
    // which is `gv100_ectx_bind`
    chan_obj.destroy(rpc, client, bar0, pdb, engine, true, say);
    *COMPUTE_GROUP.lock() = Some(chan_obj_group).filter(|g| *g != 0);

    // The context buffers are not torn down here, for two reasons.
    //
    // On the UVM path they are RM's; the mappings are ours and go, since the
    // next channel maps at the same addresses and a stale translation is a
    // fault.
    //
    // On the legacy path the buffers are this driver's, and most belong to the
    // graphics engine (`GR_GLOBALCTX_BUFFER_*`), so the mappings and the pages
    // stay and the next channel promotes the same addresses
    let mut ctx = ctx;
    for (va, _) in core::mem::take(&mut ctx.mapped) {
        vaspace::unmap(bar0, pdb, va);
    }

    // RM's own graphics context is a child of the channel group
    // (`kernel_channel_group_api.c:408`), just freed, so the next promotion is
    // a first promotion from its point of view.
    //
    // The engine's flags survive: `pCtxBuffers->bInitialized[]` belongs to the
    // graphics engine. Clearing `initialized` and asking RM to initialise the
    // global buffers answers `NV_ERR_INVALID_ARGUMENT`.
    //
    // So the flag stays set, and a rebuild owes RM the middle case: every
    // buffer present so the new context learns its address, `bInitialize` only
    // on the two belonging to the context. See `promote_params`
    compute::park_context(ctx);
    true
}

/// Where the compute fence `(epoch, seq)` stands (`FenceState`).
///
/// Every channel built has its own epoch (`chan::next_epoch`), so the epoch
/// finds the channel: the kernel's, or a process's private one from
/// `client_own_gpu`.
///
/// A channel that is gone (rebuilt after a robust-channel report,
/// `recover_if_dead`) or marked dead has lost its work: "not coming", not
/// finished. OpenRM tells a client through its error notifier
/// (`krcErrorSetNotifier`); here it is the answer to a wait
pub fn compute_fence_state(epoch_is: impl Fn(u32) -> bool, seq: u32) -> FenceState {
    let judge = |c: &crate::drivers::gpu::nvidia::gsp_common::chan::Channel| {
        if !c.usable() {
            FenceState::Lost
        } else if c.fence_passed(seq) {
            FenceState::Passed
        } else {
            FenceState::Pending
        }
    };
    if let Some((c, _)) = COMPUTE.lock().as_ref() {
        if epoch_is(c.epoch()) {
            return judge(c);
        }
    }
    CLIENTS
        .lock()
        .iter()
        .find(|c| epoch_is(c.chan.epoch()))
        .map_or(FenceState::Lost, |c| judge(&c.chan))
}

/// Where a fence stands: done, still coming, or never coming because the
/// channel writing it died
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceState {
    Passed,
    Pending,
    Lost,
}

/// Load a program a process compiled, into the compute channel's program area.
///
/// `nvidia::cubin` has read the cubin; here arrive one kernel's machine code
/// and the size of the constant bank it reads. The register and shared-memory
/// counts are the caller's declaration (the build gets them from
/// `cuobjdump -res-usage`) and are clamped.
///
/// This is where ring 3 gets its own instructions onto this GPU, and without
/// its own address space they run in the shared one and can read any buffer
/// mapped in it. So `gpu_program` is behind `CAP_GPU`
pub fn client_load_program(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    pid: u64,
    name: &str,
    code: &[u8],
    const0: u32,
    regs: u32,
    shared: u32,
) -> Result<usize, &'static str> {
    {
        let mut clients = CLIENTS.lock();
        if let Some(c) = clients.iter_mut().find(|c| c.pid == pid) {
            let pdb = c.vas.pdb;
            return c.programs.add(bar0, pdb, pid, name, code, const0, regs, shared);
        }
    }
    // No private space: the program joins the kernel's, in the shared address
    // space. `CAP_GPU` is the whole of the guard, as `docs/MikuOS_ABI.md` says
    let pdb = *CHAN_PDB_COMPUTE.lock();
    if pdb == 0 {
        return Err("the compute channel is not up");
    }
    let mut guard = PROGRAMS.lock();
    let Some(programs) = guard.as_mut() else {
        return Err("no programs are loaded");
    };
    programs.add(bar0, pdb, pid, name, code, const0, regs, shared)
}

/// Take the programs an exited process loaded out of the kernel's set.
///
/// Only the shared set: `client_drop` takes the rest. The lock order of a
/// dispatch, programs then channel, so no process can launch one between the
/// wait and the unmap. If the channel will not go idle the programs stay: an
/// unmap under a running grid is a fault
pub fn client_forget_programs(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    pid: u64,
    say: &dyn Fn(&str),
) {
    let pdb = *CHAN_PDB_COMPUTE.lock();
    if pdb == 0 {
        return;
    }
    let mut guard = PROGRAMS.lock();
    let Some(programs) = guard.as_mut() else { return };
    if !programs.owned_by(pid) {
        return;
    }
    if let Some((chan, _)) = COMPUTE.lock().as_ref() {
        if chan.wait_last(500_000).is_err() {
            say(&alloc::format!(
                "compute channel still busy; pid {}'s programs stay loaded", pid));
            return;
        }
    }
    let bufs = programs.remove_owned(bar0, pdb, pid);
    say(&alloc::format!("pid {}: {} loaded program(s) unloaded", pid, bufs.len()));
    crate::drivers::gpu::nvidia::gsp_common::chan::retire_dma(bufs);
}

/// Every program by name, in index order, for `gpu_info`.
///
/// Owned strings: a program from `gpu_program` has no name in the binary, and
/// the list is read from under a lock not held while printing
pub fn program_names(pid: u64) -> alloc::vec::Vec<alloc::string::String> {
    if let Some(c) = CLIENTS.lock().iter().find(|c| c.pid == pid) {
        return c.programs.names().into_iter().map(alloc::string::String::from).collect();
    }
    match PROGRAMS.lock().as_ref() {
        Some(p) => p.names().into_iter().map(alloc::string::String::from).collect(),
        None => alloc::vec::Vec::new(),
    }
}
