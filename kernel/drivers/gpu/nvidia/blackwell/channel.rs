// The GPFIFO channel and its copy engine: creation, the submit API the
// compositor calls every frame, and the first-blit diagnostics.



use crate::mm::dma::DmaBuffer;
use crate::drivers::gpu::nvidia::gsp_common::chan;

use super::chanmem::{CHAN_INST, CHAN_INTERNAL, CHAN_SCRATCH, GPFIFO_BYTES, PUSHBUF_BYTES, SEMA_BYTES, gpfifo_va, pushbuf_va, sema_va};
use super::compute_chan::{COMPUTE, bring_up_compute};

/// What this file's lines are signed with when the engine loader hands it the
/// boot's `say` (`nvidia::speaking`, `dev/klog.md`)
pub(super) static SITE: crate::kcore::klog::Site = crate::klog_site!();

/// The channel the copy engine runs on, once it exists
static CHANNEL: spin::Mutex<Option<crate::drivers::gpu::nvidia::gsp_common::chan::Channel>> =
    spin::Mutex::new(None);

/// Is there a copy engine to push pixels with?
pub fn copy_engine_ready() -> bool {
    CHANNEL.lock().as_ref().is_some_and(|c| c.usable())
}

/// Move a rectangle from system memory into video memory with the copy engine.
///
/// What the console's flush calls instead of two million MMIO writes. Both
/// addresses are physical: the source is a physically contiguous DMA buffer, as
/// `scanout` allocates the console's shadow.
///
/// False if there is no channel or the engine did not signal in time; the
/// caller falls back to PRAMIN
pub fn ce_copy_to_vram(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    src_phys: u64,
    dst_vram: u64,
    pitch_in: u32,
    pitch_out: u32,
    line_len: u32,
    lines: u32,
) -> bool {
    let mut g = CHANNEL.lock();
    let Some(chan) = g.as_mut() else { return false };
    if !chan.usable() {
        return false;
    }
    chan.copy_2d(
        bar0, src_phys, true, dst_vram, false, pitch_in, pitch_out, line_len, lines,
        200_000,
    )
    .is_ok()
}

/// A whole list of copies, as one submission the engine executes without
/// letting the processor back in, waited for. Each copy says which memory its
/// source and destination are in (`Copy2d::src_sysmem`, `dst_sysmem`): into
/// video memory for the console, out of it for the processor's reads of a
/// window there (`Scanout::texture_download`, `dev/gpu-window-buffers.md`).
///
/// The timeout is the single-copy one scaled by the list: one wait for all of
/// them
pub fn ce_copy_batch(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    copies: &[crate::drivers::gpu::nvidia::gsp_common::chan::Copy2d],
) -> bool {
    let mut g = CHANNEL.lock();
    let Some(chan) = g.as_mut() else { return false };
    if !chan.usable() || copies.is_empty() {
        return false;
    }
    let budget = 200_000 + 20_000 * copies.len() as u64;
    chan.copy_2d_batch(bar0, copies, budget).is_ok()
}

/// The same batch, queued and left to run.
///
/// The processor goes on with the next frame. The engine reads the sources
/// after this returns, so nothing may rewrite one until `ce_wait_last` says the
/// batch is done
pub fn ce_submit_batch_to_vram(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    copies: &[crate::drivers::gpu::nvidia::gsp_common::chan::Copy2d],
) -> bool {
    let mut g = CHANNEL.lock();
    let Some(chan) = g.as_mut() else { return false };
    if !chan.usable() || copies.is_empty() {
        return false;
    }
    chan.submit_batch(bar0, copies).is_ok()
}

/// A batch that waits for another engine first, and says where its own fence
/// is.
///
/// Returns the semaphore address and the value it will reach, what the next
/// thing in the frame passes as its own `after`
pub fn ce_submit_batch_after(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    copies: &[crate::drivers::gpu::nvidia::gsp_common::chan::Copy2d],
    after: Option<(u64, u32)>,
) -> Option<(u64, u32)> {
    let mut g = CHANNEL.lock();
    let chan = g.as_mut()?;
    if !chan.usable() || copies.is_empty() {
        return None;
    }
    let fence = chan.submit_batch_after(bar0, copies, after).ok()?;
    Some((chan.sema_va(), fence))
}

/// Where the copy engine's fence lives and what the last batch will write
/// there, for making the hardware wait.
///
/// The address is physical, as a display window addresses a semaphore; the
/// value is the channel's monotonic counter, safe against the window's
/// strict-equality compare
pub fn ce_fence_target() -> Option<(u64, u32)> {
    CHANNEL.lock().as_ref()?.fence_target()
}

/// What another engine's chain waits on to come after everything the copy
/// engine was last given: the copy channel's semaphore in the address space
/// both channels share, and the value its last batch will write. The compute
/// chain folding the shell below the windows waits on this for the windows'
/// uploads (`blur::submit_base`)
pub fn ce_after() -> Option<(u64, u32)> {
    CHANNEL.lock().as_ref().map(|chan| (chan.sema_va(), chan.last_fence()))
}

/// Has the copy engine finished everything it was given?
///
/// A poll, so a flip can decide "arm now" or "wait a frame" without stopping.
/// This keeps the pipeline one flip deep
pub fn ce_idle() -> bool {
    match CHANNEL.lock().as_ref() {
        Some(chan) => chan.fence_passed(chan.last_fence()),
        None => true,
    }
}

/// Let both channels go, whatever they wait for.
///
/// For a frame that has waited past its budget: one engine is stuck on the
/// other's fence and neither will move on its own.
///
/// Both fences are released, since either channel may hold the other, and only
/// the compute channel is marked dead. The caller has given up on compute
/// (`give_up_on_compute`), so refusing its work costs only a blur; the copy
/// engine puts the console and every frame on the panel. The copy engine's
/// forced fence is counted, and `nvidia intr` prints the count.
///
/// First, before the fences are written over: one WARN with where each channel
/// stood: fence, semaphore, GP_GET/GP_PUT, and how long ago BAR1 was last bound
pub fn unstick_engines() {
    let bar0 = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0);
    let line = |c: &chan::Channel| match bar0.as_ref() {
        Some(b) => c.stall_line(b),
        None => alloc::format!("chid {}: no card to read", c.chid),
    };
    let copy = CHANNEL.lock().as_ref().map(|c| line(c));
    let compute = COMPUTE.lock().as_ref().map(|(c, _)| line(c));
    crate::kwarn!("[nvidia] a frame waited past its budget for the engines - copy {}; compute {}{}",
        copy.as_deref().unwrap_or("not up"), compute.as_deref().unwrap_or("not up"),
        match super::bar1::last_bind_ms_ago() {
            Some(ms) => alloc::format!("; BAR1 was last bound {} ms ago", ms),
            None => alloc::string::String::new(),
        });

    if let Some(chan) = CHANNEL.lock().as_mut() {
        chan.force_fence();
    }
    if let Some((chan, _)) = COMPUTE.lock().as_mut() {
        chan.force_fence();
        chan.mark_failed(chan::Dead::FrameStall);
    }
}

/// How long `with_submissions_held` waits for a channel lock before going on
/// without it
const HOLD_BUDGET_US: u64 = 100_000;

/// Run `f` with no submission to any channel half-written: the copy engine's
/// channel, the kernel's compute channel and every process's own, each lock
/// held, in that order, which no other path reverses.
///
/// For a BAR1 bind (`bar1::bind`). Bounded: at a reboot the holder of a lock
/// may be a halted processor, and an unguarded bind beats a machine that does
/// not restart
pub(super) fn with_submissions_held<R>(f: impl FnOnce() -> R) -> R {
    fn hold<T>(m: &'static spin::Mutex<T>, sw: &crate::kcore::time::Stopwatch) -> Option<spin::MutexGuard<'static, T>> {
        loop {
            if let Some(g) = m.try_lock() {
                return Some(g);
            }
            if sw.elapsed_us() >= HOLD_BUDGET_US {
                return None;
            }
            core::hint::spin_loop();
        }
    }
    let sw = crate::kcore::time::Stopwatch::start();
    let copy = hold(&CHANNEL, &sw);
    let compute = hold(&COMPUTE, &sw);
    let clients = hold(&super::compute_chan::CLIENTS, &sw);
    if copy.is_none() || compute.is_none() || clients.is_none() {
        crate::kwarn!("[nvidia] a channel stayed busy for {} ms - going on without holding its submissions",
            HOLD_BUDGET_US / 1000);
    }
    let r = f();
    drop(clients);
    drop(compute);
    drop(copy);
    r
}

/// Give a channel its USERD window through BAR1 (3.5b): one page, uncached,
/// when BAR1 is physical and reaches that far; otherwise GP_PUT goes through
/// PRAMIN.
///
/// The epoch is read before BAR1 is asked about. `bar1::off` takes the aperture
/// away before it raises the epoch, so a rebind between the two reads leaves a
/// window stale from the start, never one that looks current and points at what
/// BAR1 maps now
pub(super) fn attach_userd(c: &mut chan::Channel) {
    let epoch = chan::userd_epoch();
    let cpu = super::bar1::map(c.userd_vram(), chan::USERD_SIZE, false);
    c.set_userd_window(cpu, epoch);
}

/// BAR1 has just been bound physical: every live channel gets its window
pub(super) fn attach_userd_all() {
    if let Some(c) = CHANNEL.lock().as_mut() {
        attach_userd(c);
    }
    super::compute_chan::attach_userd_compute();
}

/// Mark whichever channel GSP-RM killed, by the chid its report names.
///
/// Called from the event handler, outside the transport. A chid this driver
/// does not own is not an error: GSP-RM reports robust-channel events for its
/// own channels too, such as the golden channel at chid 0
pub fn note_channel_died(chid: u32) -> bool {
    let mut hit = false;
    if let Some(chan) = CHANNEL.lock().as_mut() {
        if chan.chid == chid {
            chan.mark_failed(chan::Dead::RobustChannel);
            hit = true;
        }
    }
    if let Some((chan, _)) = COMPUTE.lock().as_mut() {
        if chan.chid == chid {
            chan.mark_failed(chan::Dead::RobustChannel);
            hit = true;
        }
    }
    // A process's own channel is ours too
    if let Some(pid) = super::compute_chan::note_client_died(chid) {
        crate::kerror!("[nvidia] pid {}'s own compute channel (chid {}) was killed by GSP-RM - \
            its dispatches are refused from here", pid, chid);
        hit = true;
    }
    hit
}

/// A submission from ring 3: a batch of copies, queued and left to run.
///
/// Returns `(epoch, fence)`, which the syscall layer encodes into the number it
/// hands the client. The fence alone means nothing once the channel has been
/// rebuilt
pub fn ce_submit_client_batch(
    copies: &[crate::drivers::gpu::nvidia::gsp_common::chan::Copy2d],
) -> Option<(u32, u32)> {
    let bar0 = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0)?;
    let mut g = CHANNEL.lock();
    let chan = g.as_mut()?;
    if !chan.usable() || copies.is_empty() {
        return None;
    }
    // A client's pools: system memory, or video memory from `gpu_alloc`, as
    // each copy says: one pair for the whole batch, checked by the caller
    let fence = chan.submit_batch(&bar0, copies).ok()?;
    Some((chan.epoch(), fence))
}

/// Where the copy-engine fence `(epoch, seq)` stands: the same three answers as
/// `compute_fence_state`. From a build that is gone, or a channel marked dead,
/// the work is lost
pub fn ce_fence_state(epoch_is: impl Fn(u32) -> bool, seq: u32) -> super::compute_chan::FenceState {
    use super::compute_chan::FenceState;
    match CHANNEL.lock().as_ref() {
        Some(c) if epoch_is(c.epoch()) => {
            if !c.usable() {
                FenceState::Lost
            } else if c.fence_passed(seq) {
                FenceState::Passed
            } else {
                FenceState::Pending
            }
        }
        _ => FenceState::Lost,
    }
}

/// Is the copy engine still taking work, and how many of its fences did the
/// processor have to write? For `nvidia intr` and the shell
pub fn ce_health() -> (bool, u32) {
    match CHANNEL.lock().as_ref() {
        Some(c) => (c.healthy(), c.forced_fences()),
        None => (true, 0),
    }
}

/// Wait out whatever the copy engine was last given.
///
/// False only if it was running when the budget ran out. Three quarters of a
/// millisecond of work in a sixteen millisecond frame should never do that
pub fn ce_wait_last(timeout_us: u64) -> bool {
    // The fence is taken under the lock and waited for without it: the wait may
    // sleep (3.5a)
    let Some(h) = CHANNEL.lock().as_ref().and_then(|c| c.last_handle()) else { return true };
    h.wait(timeout_us).is_ok()
}

/// Take both channels apart: the compute engine's and the copy engine's.
///
/// False when there was nothing to take apart. Each engine object's handle is
/// derived as it was allocated: `H_COPY | chid`, `H_COMPUTE | chid`.
///
/// Both, since `build_channel` builds both
pub fn ce_destroy(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    pdb: u64,
    say: &dyn Fn(&str),
) -> bool {
    // The compute channel first: `build_channel` ends by bringing the compute
    // channel up. A teardown of the copy engine's alone leaks a channel id,
    // three ring mappings, a graphics context and a copy of every program, and
    // RM refuses to promote a context while the first holds it
    super::compute_chan::compute_destroy(bar0, rpc, client, say);

    let Some(chan) = CHANNEL.lock().take() else { return false };
    let engine = chan::H_COPY | chan.chid;
    chan.destroy(rpc, client, bar0, pdb, engine, false, say);
    true
}

/// Build and tear the copy engine's channel down `rounds` times, printing what
/// the driver holds after each.
///
/// The test for the teardown: a leak is a count growing every round, and a
/// use-after-free is a machine that stops. QEMU has no GB206, so this is a
/// shell command.
///
/// The compositor must not be running: it submits to this channel every frame
pub fn chan_cycle(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    pdb: u64,
    rounds: u32,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gmmu;
    use crate::drivers::gpu::nvidia::vaspace;
    use crate::drivers::gpu::nvidia::gsp_common::vram;

    if crate::mayland::render::running() {
        say("chan-cycle: the compositor is running, 'mayland stop' first");
        return;
    }
    if pdb == 0 {
        say("chan-cycle: no page directory, the card was never booted");
        return;
    }
    let vas = chan::ChanVaSpace { handle: chan::H_VASPACE, pdb };

    let report = |say: &dyn Fn(&str), round: u32| {
        let (tables, spare) = gmmu::table_counts();
        let (free, used) = vram::totals();
        say(&alloc::format!(
            "chan-cycle {}: {} binding(s), {} table(s) ({} spare), chids {:#b}, \
             {} RM object(s), {} buffer(s) held, {} frame(s) retired, \
             vram {} used / {} free",
            round, vaspace::mapping_count(), tables, spare, chan::chids_in_use(),
            crate::drivers::gpu::nvidia::gsp_common::rmapi::object_count(),
            CHANNEL.lock().as_ref().map_or(0, |c| c.buffer_count()),
            chan::retired_buffers(), used, free
        ));
    };
    report(say, 0);

    for round in 1..=rounds {
        if !ce_destroy(bar0, rpc, client, pdb, say) {
            say("chan-cycle: there is no channel to tear down");
            return;
        }
        // Frames are released a frame after the engines are done with them, and
        // with no compositor nothing else drains the list
        let reaped = chan::reap_buffers();
        build_channel(bar0, rpc, client, &vas, say);
        if CHANNEL.lock().is_none() {
            say(&alloc::format!("chan-cycle {}: the channel did not come back", round));
            report(say, round);
            return;
        }
        say(&alloc::format!("chan-cycle {}: rebuilt, {} frame(s) freed", round, reaped));
        report(say, round);
    }
    say("chan-cycle: done, the counts above should be flat, not climbing");
}

/// Everything between "RM took our page directory" and "the copy engine is on
/// the channel", one round trip at a time.
///
/// Every step reports what it did or what RM called the failure, and stops on
/// error
pub fn build_channel(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    vas: &crate::drivers::gpu::nvidia::gsp_common::chan::ChanVaSpace,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gmmu;
    use crate::drivers::gpu::nvidia::vaspace;
    use crate::drivers::gpu::nvidia::gsp_common::chan::{self, Channel};
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{status_str, RmClient, RmError};

    let inst = *CHAN_INST.lock();
    if inst == 0 {
        say("RM: no video memory carved for the channel instance block");
        return;
    }

    // How big the copy engine's method buffer has to be. A property of this
    // chip's engines, and RM knows it
    let Some((h_client, h_device, h_subdevice)) = *CHAN_INTERNAL.lock() else {
        say("RM: GSP-RM's internal handles were never captured");
        return;
    };
    let internal = RmClient { h_client, h_device, h_subdevice };
    let mthdbuf_size = match chan::method_buffer_size(rpc, &internal) {
        Ok(n) => n,
        Err(RmError::ShortReply) => {
            say("RM: method buffer size reply too short");
            return;
        }
        Err(RmError::Status(s)) => {
            say(&alloc::format!("RM: method buffer size refused: {}", status_str(s)));
            return;
        }
        Err(e) => {
            say(&alloc::format!("RM: method buffer size failed: {:?}", e));
            return;
        }
    };

    // Ring, pushbuffer and method buffer, all in system memory, where the CPU
    // writes the first two with ordinary stores. Owned here until the channel
    // exists, so a failure on the way frees them
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
        say("RM: not enough system memory for the channel's buffers");
        return;
    };
    DmaBuffer::write_barrier();

    // The channel id comes first, since the addresses come from it: a channel's
    // window in the address space is its id
    let Some(chid) = chan::alloc_chid(chan::Owner::Copy) else {
        say("RM: no channel id is free");
        return;
    };
    let (gpfifo_va, pushbuf_va, sema_va) =
        (gpfifo_va(chid), pushbuf_va(chid), sema_va(chid));

    // The ring and the pushbuffer have to exist in the channel's address space
    // before RM is told where they are: `gpFifoOffset` is a virtual address the
    // host unit dereferences
    for (what, va, phys, bytes) in [
        ("gpfifo", gpfifo_va, gpfifo, GPFIFO_BYTES),
        ("pushbuf", pushbuf_va, pushbuf, PUSHBUF_BYTES),
        ("semaphore", sema_va, sema, SEMA_BYTES),
    ] {
        if let Err(e) = vaspace::map(bar0, vas.pdb, va, phys, bytes, gmmu::Aperture::Sysmem) {
            say(&alloc::format!("RM: mapping the {} at {:#x} failed: {}", what, va, e));
            chan::free_chid(chid);
            return;
        }
    }
    say(&alloc::format!(
        "RM: chid {} mapped gpfifo va {:#x} -> {:#x}, pushbuf va {:#x} -> {:#x}, mthdbuf {:#x} ({} B)",
        chid, gpfifo_va, gpfifo, pushbuf_va, pushbuf, mthdbuf, mthdbuf_size
    ));

    // Which copy engine, and on which runlist, asked before the channel is
    // created: the engine type goes into the allocation parameters. COPY0, a
    // GRCE sharing runlist 0 with graphics, gets the channel refused by GR
    let ce = match chan::pick_copy_engine(rpc, &internal, say) {
        Ok(Some(ce)) => {
            say(&alloc::format!(
                "RM: using copy engine type {:#x} on runlist {} (no graphics there)",
                ce.engine_type, ce.runlist
            ));
            ce
        }
        Ok(None) => {
            say("RM: the device info table lists no copy engine at all");
            return;
        }
        Err(RmError::Status(s)) => {
            say(&alloc::format!("RM: device info table refused: {}", status_str(s)));
            return;
        }
        Err(e) => {
            say(&alloc::format!("RM: device info table failed: {:?}", e));
            return;
        }
    };

    let chan = match Channel::create(
        rpc, client, bar0, vas, inst, mthdbuf, mthdbuf_size, gpfifo_va, GPFIFO_BYTES,
        ce.engine_type, chid,
        // The copy engine has no graphics context, so its group owns nothing
        // worth keeping and goes with it
        None,
        say,
    ) {
        Ok(c) => {
            say(&alloc::format!(
                "RM: channel {:#x} chid {} inst {:#x} allocated, bound to {:#x}, scheduled",
                c.handle, c.chid, c.inst, ce.engine_type
            ));
            c
        }
        Err(RmError::Status(s)) => {
            say(&alloc::format!("RM: channel refused: {}", status_str(s)));
            chan::free_chid(chid);
            return;
        }
        Err(e) => {
            say(&alloc::format!("RM: channel failed: {:?}", e));
            chan::free_chid(chid);
            return;
        }
    };

    let mut chan = chan;
    // From here the frames belong to the channel, and are freed when it is
    chan.own_buffers(core::mem::take(&mut bufs));
    match chan.alloc_copy_engine(rpc, client, ce.engine_type) {
        Ok(()) => say(&alloc::format!(
            "RM: copy engine (class {:#x}) is on the channel", chan::CLASS_DMA_COPY
        )),
        Err(RmError::Status(s)) => {
            say(&alloc::format!("RM: copy engine refused: {}", status_str(s)));
            return;
        }
        Err(e) => {
            say(&alloc::format!("RM: copy engine failed: {:?}", e));
            return;
        }
    }

    // What to write into the doorbell.
    //
    // Computed, as nouveau computes it. Under 570 RM's work submit token was
    // not the register value: a control answered by GSP-RM took the GSP branch
    // of `kfifoGenerateWorkSubmitTokenHal_GB202` and came back as the bare
    // channel id. 615 dropped that branch and builds the doorbell value on GSP
    // too; the line says whether the two agree
    let token = chan.work_submit_token(rpc, client).ok();
    chan.doorbell = chan.doorbell_handle(ce.runlist);
    say(&alloc::format!(
        "RM: doorbell {:#010x} (RM's token {:#010x}, {})",
        chan.doorbell, token.unwrap_or(0),
        if token == Some(chan.doorbell) { "the same" } else { "different" }
    ));

    if let Err(e) = chan.attach_ring(
        crate::arch::x86_64::grub::phys_to_virt(pushbuf) as *mut u32,
        pushbuf_va,
        (PUSHBUF_BYTES / 4) as usize,
        crate::arch::x86_64::grub::phys_to_virt(gpfifo) as *mut u32,
        crate::arch::x86_64::grub::phys_to_virt(sema) as *mut u32,
        sema_va,
        sema,
    ) {
        say(&alloc::format!("RM: channel ring refused: {}", e));
        return;
    }
    first_blit(bar0, rpc, &mut chan, gpfifo, pushbuf, sema, say);
    attach_userd(&mut chan);
    *CHANNEL.lock() = Some(chan);

    bring_up_compute(bar0, rpc, client, &internal, vas, mthdbuf_size, say);
    // Both channels exist: their engines' fences can wake a waiter now
    super::interrupts::fence_load(ce.engine_type, say);
}

/// One copy, video memory to video memory, and read the answer back.
///
/// Two pages nothing else touches: a pattern written into the first through
/// PRAMIN, zeroes in the second, and the copy engine asked to move 4096 bytes
/// between them. PRAMIN reads back the destination. This depends on no page
/// table, BAR1 or display, so it verifies the GPU can execute a command from
/// this driver
fn first_blit(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    chan: &mut crate::drivers::gpu::nvidia::gsp_common::chan::Channel,
    gpfifo: u64,
    pushbuf: u64,
    sema: u64,
    say: &dyn Fn(&str),
) {
    let scratch = *CHAN_SCRATCH.lock();
    if scratch == 0 {
        say("RM: no video memory carved for the blit test");
        return;
    }
    let (src, dst) = (scratch, scratch + 0x1000);
    const PATTERN: u32 = 0xca11_b1a7;

    {
        let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
        for off in (0..0x1000u64).step_by(4) {
            w.write32(src + off, PATTERN ^ (off as u32));
            w.write32(dst + off, 0);
        }
    }
    crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);

    // Both buffers are system memory, so the CPU reaches them through the
    // direct map at the physical address the GPU was given
    let push = crate::arch::x86_64::grub::phys_to_virt(pushbuf) as *mut u32;
    let ring = crate::arch::x86_64::grub::phys_to_virt(gpfifo) as *mut u32;
    // Goes through the `copy_2d` the console uses: two submit paths would each
    // update state the other owned, and a stale `self.put` makes GP_PUT equal
    // GET, so the doorbell does nothing
    let result = chan.copy_2d(bar0, src, false, dst, false, 0x1000, 0x1000, 0x1000, 1, 500_000);

    // What was put in front of the host unit, as raw words, to check against
    // `push906f.h` and `clca6f.h`
    let push = crate::arch::x86_64::grub::phys_to_virt(pushbuf) as *const u32;
    let ring = crate::arch::x86_64::grub::phys_to_virt(gpfifo) as *const u32;
    let sem = crate::arch::x86_64::grub::phys_to_virt(sema) as *const u32;
    {
        let gp = unsafe { [*ring, *ring.add(1)] };
        let mut mth = alloc::string::String::new();
        for i in 0..8 {
            mth.push_str(&alloc::format!(" {:08x}", unsafe { *push.add(i) }));
        }
        say(&alloc::format!("RM: gpfifo entry {:08x} {:08x}", gp[0], gp[1]));
        say(&alloc::format!("RM: pushbuf[0..8]{}", mth));
    }
    let (gp_get, gp_put) = chan.userd_gp(bar0);
    say(&alloc::format!(
        "RM: semaphore {:#x} after the ring; userd gpget {:#x} gpput {:#x}",
        unsafe { core::ptr::read_volatile(sem) }, gp_get, gp_put
    ));

    // Check the destination unconditionally: a stale GP_GET can hide a working
    // copy engine
    crate::drivers::gpu::nvidia::pramin::l2_flush_dirty(bar0);
    crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);
    let got = {
        let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
        [w.read32(dst), w.read32(dst + 4), w.read32(dst + 0xffc)]
    };
    let want = [PATTERN, PATTERN ^ 4, PATTERN ^ 0xffc];
    match (result.map(|_| ()), got == want) {
        (Ok(()), true) => {
            say(&alloc::format!(
                "RM: blit ok, 4 KiB {:#x} -> {:#x}, semaphore signalled", src, dst
            ));
            return;
        }
        (Ok(()), false) => say(&alloc::format!(
            "RM: semaphore signalled but the bytes are wrong: {:#x} {:#x} {:#x} (want {:#x} {:#x} {:#x})",
            got[0], got[1], got[2], want[0], want[1], want[2]
        )),
        (Err(v), true) => say(&alloc::format!(
            "RM: the copy landed but the semaphore stayed {:#x}, completion is still not readable",
            v
        )),
        (Err(v), false) => say(&alloc::format!(
            "RM: blit did not run, semaphore {:#x}, destination {:#x} {:#x} {:#x}",
            v, got[0], got[1], got[2]
        )),
    }

    // From here the question is whether the host unit ever looked. Three
    // sources:
    blit_postmortem(bar0, rpc, chan, say);
}

/// What the hardware and GSP-RM say after a submit that went nowhere.
///
/// A GPFIFO fetch that faults, a channel never scheduled and a doorbell nobody
/// heard look identical from GP_GET. But an MMU fault leaves an entry in the
/// fault buffer and makes GSP-RM post `RC_TRIGGERED` naming the address, and a
/// channel RM never armed leaves the instance block's RAMFC untouched
fn blit_postmortem(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    chan: &crate::drivers::gpu::nvidia::gsp_common::chan::Channel,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gsp_common::rpc as gsprpc;

    // RAMFC is the first 0x200 bytes of the instance block, written by GSP-RM
    // at channel setup. All zeroes means it never did
    {
        let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
        let mut nonzero = 0;
        // Does the channel state contain the ring's address at all? RAMFC
        // layout is undocumented in the references here, so the field cannot be
        // named, but the value can be searched for
        let mut has_gpfifo = false;
        let mut head = alloc::string::String::new();
        for off in (0..0x200u64).step_by(4) {
            let v = w.read32(chan.inst + off);
            if v != 0 {
                nonzero += 1;
            }
            if v == chan.gpfifo_va as u32 {
                has_gpfifo = true;
            }
            if off < 32 {
                head.push_str(&alloc::format!(" {:08x}", v));
            }
        }
        say(&alloc::format!("RM: ramfc[0..8]{}", head));
        say(&alloc::format!(
            "RM: ramfc {} the gpfifo address {:#x}",
            if has_gpfifo { "contains" } else { "does NOT contain" }, chan.gpfifo_va
        ));
        let pdb_lo = w.read32(chan.inst + 0x200);
        let pdb_hi = w.read32(chan.inst + 0x204);
        let lim_lo = w.read32(chan.inst + 0x208);
        let lim_hi = w.read32(chan.inst + 0x20c);
        let sc0_lo = w.read32(chan.inst + 0x2a0);
        let mask = w.read32(chan.inst + 0x298);
        drop(w);
        // Printed as separate words: we wrote `pdb | BIT(10) | BIT(11)` here,
        // and a merged value hides whether the flag bits survived
        say(&alloc::format!(
            "RM: ramfc {}/128 words; inst 0x200 {:08x}:{:08x} 0x208 {:08x}:{:08x}",
            nonzero, pdb_hi, pdb_lo, lim_hi, lim_lo
        ));
        say(&alloc::format!(
            "RM: inst subctx0 {:08x} mask {:08x} (we wrote the page directory | 0xc00)",
            sc0_lo, mask
        ));
    }

    // The MMU fault buffer and the XAL interrupt tree. Prefixed, so they land
    // in the closing block
    let relay = |s: &str| say(&alloc::format!("RM: {}", s));
    crate::drivers::gpu::nvidia::gmmu::dump_mmu_faults(bar0, &relay);
    say(&alloc::format!(
        "RM: xal intr {:#010x}",
        crate::drivers::gpu::nvidia::pramin::xal_intr(bar0)
    ));

    // GSP-RM's own account. `RC_TRIGGERED` carries the channel, the exception
    // type and the faulting address.
    //
    // Read through the dispatcher, which puts robust-channel reports, NOCAT
    // records and Xids in the journal. Anything else is said here as it passes.
    // A round bringing nothing ends it: 50 ms of quiet
    let mut seen = 0;
    let mut events = rpc.events();
    for _ in 0..40 {
        let before = seen;
        let mut dispatch = |m: &gsprpc::RpcMessage, ctx: gsprpc::Context| {
            seen += 1;
            if !matches!(
                m.function,
                gsprpc::EV_RC_TRIGGERED | gsprpc::EV_GSP_POST_NOCAT_RECORD
                    | gsprpc::EV_OS_ERROR_LOG | gsprpc::EV_MMU_FAULT_QUEUED
            ) {
                say(&alloc::format!(
                    "RM: gsp event {:#x} {} ({} B)",
                    m.function, gsprpc::event_name(m.function), m.payload.len()
                ));
            }
            events(m, ctx);
        };
        let _ = rpc.recv_poll(gsprpc::NOTHING, None, gsprpc::Context::Poll, 50_000_000, &mut dispatch);
        if seen == before {
            break;
        }
    }
    let j = crate::drivers::gpu::nvidia::gsp_common::kgsp::events::take_journal();
    for rc in &j.rc {
        say(&alloc::format!("RM: {}", rc.describe()));
    }
    if j.rc_dropped != 0 {
        say(&alloc::format!("RM: {} further robust-channel report(s)", j.rc_dropped));
    }
    for (xid, chid, text) in &j.xid {
        say(&alloc::format!("RM: Xid {} (chid {}): {}", xid, chid, text));
    }
    for line in &j.nocat {
        say(&alloc::format!("RM: {}", line));
    }
    if seen == 0 {
        say("RM: GSP-RM posted nothing, no robust-channel error, no nothing faulted");
    }
}
