// The EVO window: attaching a surface to it, the update notifier that confirms
// the attach, and the live-scanout record of what is on screen.

use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use crate::mm::dma::DmaBuffer;

use super::modeset::DIAG;
use crate::drivers::gpu::nvidia::gsp_common::output;
use crate::drivers::gpu::nvidia::gsp_common::vram;

/// What a scanout surface is aligned to. The window wants its surface on a 64
/// KiB boundary, and both surfaces of a flip satisfy it
pub const SURFACE_ALIGN: u64 = 0x10000;

// The notifier and the lookup tables are each their own allocation out of
// `gsp_common::vram`. A table is `0x20 + 1025 * 8` = 0x2028 bytes, forty over a
// round 8 KiB, so tables at 0x2000 offsets overlap: the output LUT over the
// notifier, the input LUT over the output LUT's header including entry 0
// (`dev/gb206-display.md`, section 6.1).

/// The core channel's update-completion notifier.
///
/// In video memory whenever the scanout is: `coreca7d_update` addresses the
/// record as PHYSICAL_NVM on every chip, the record is a display surface like
/// any other, and this engine silently reverts a system-memory scanout surface
/// to disabled. A notifier the engine declines to write reads NOT_BEGUN
/// forever, like a stuck update. System memory is the fallback when VRAM is
/// unreachable
enum Notifier {
    Vram(u64),
    Sys(DmaBuffer),
}

impl Notifier {
    fn phys(&self) -> u64 {
        match self {
            Notifier::Vram(off) => *off,
            Notifier::Sys(b) => b.phys(),
        }
    }

    fn target(&self) -> u32 {
        use crate::drivers::gpu::nvidia::gsp_common::disp;
        match self {
            Notifier::Vram(_) => disp::NOTIFIER_TARGET_VRAM,
            Notifier::Sys(_) => disp::NOTIFIER_TARGET_PCI_COHERENT,
        }
    }

    fn reset(&mut self, bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion) {
        use crate::drivers::gpu::nvidia::pramin;
        match self {
            Notifier::Vram(off) => {
                pramin::copy_to_vram(bar0, *off, &[0u8; 16]);
                pramin::flush_fb(bar0);
            }
            Notifier::Sys(b) => {
                crate::drivers::gpu::nvidia::gsp_common::disp::notifier_reset(b.as_mut_slice())
            }
        }
    }

    fn status(&self, bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion) -> u32 {
        match self {
            Notifier::Vram(off) => {
                let mut w = [0u32; 1];
                crate::drivers::gpu::nvidia::pramin::sample_vram(bar0, *off, &mut w);
                w[0] >> 30
            }
            Notifier::Sys(b) => {
                crate::drivers::gpu::nvidia::gsp_common::disp::notifier_status(b.as_slice())
            }
        }
    }

    /// Poll until the record reads FINISHED, or give up after `ms`
    fn wait(&self, bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion, ms: u64) -> u32 {
        use crate::drivers::gpu::nvidia::gsp_common::disp;
        let khz = crate::kcore::time::tsc_khz().max(1);
        let t0 = crate::kcore::time::rdtsc();
        loop {
            let status = self.status(bar0);
            if status == disp::NOTIFIER_STATUS_FINISHED {
                return status;
            }
            if crate::kcore::time::rdtsc().wrapping_sub(t0) / khz >= ms {
                return status;
            }
            core::hint::spin_loop();
        }
    }
}

fn notifier_name(status: u32) -> &'static str {
    match status {
        0 => "NOT_BEGUN",
        1 => "BEGUN",
        2 => "FINISHED",
        _ => "?",
    }
}

/// Bind a window to `head` and point it at a framebuffer, turning the raster
/// from black into a picture.
///
/// With `vram_ok` false the surface lives in system memory: the window's
/// surface target field accepts PHYSICAL_PCI_COHERENT. Every window method is
/// staged and committed by the window channel's own UPDATE, independent of the
/// core channel's
#[allow(clippy::too_many_arguments)]
pub fn attach_window(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    internal: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    h_root: u32,
    core_push: &mut crate::drivers::gpu::nvidia::gsp_common::pushbuf::PushBuf,
    wndw_push_mem: &mut [Option<DmaBuffer>],
    wimm_mem: &mut [Option<DmaBuffer>],
    fb_mem: &mut [Option<DmaBuffer>],
    display_id: u32,
    head: u32,
    mode: &crate::drivers::gpu::nvidia::gsp_common::modeset::Mode,
    vram_ok: bool,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    use crate::drivers::gpu::nvidia::gsp_common::modeset;
    use crate::drivers::gpu::nvidia::gsp_common::pushbuf::PushBuf;
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{status_str, RmError};

    let fail = |what: &str, e: RmError| -> alloc::string::String {
        match e {
            RmError::Status(s) => alloc::format!("DISP: {} failed: {}", what, status_str(s)),
            other => alloc::format!("DISP: {} failed: {:?}", what, other),
        }
    };

    // Windows go two per head, fixed: head N owns windows 2N and 2N+1. One is
    // enough for a full-screen image. Ownership was staged and committed with
    // the modeset.
    //
    // The head also indexes `DisplayMemory`: one pushbuffer, one immediate
    // pushbuffer and one surface per head, since the hardware reads the first
    // monitor's while the second is set up
    let window = head * 2;
    let slot = head as usize;
    if slot >= fb_mem.len() {
        say(&alloc::format!(
            "DISP: head {} is past the {} this driver keeps buffers for",
            head,
            fb_mem.len()
        ));
        return;
    }

    // Pitch is a whole number of 64-byte units, which 32-bit pixels at any sane
    // width are. The window feeds the head's viewport, so it cannot be larger:
    // an oversized window state makes the channel arm nothing (`R warm` shows
    // size 0, surface disabled), with no error
    let (w, h) = (
        mode.h_display.min(modeset::viewport_width(mode)),
        mode.v_display.min(modeset::viewport_height(mode)),
    );
    let pitch = (w * 4).next_multiple_of(modeset::PITCH_GRANULARITY);
    let bytes = pitch as usize * h as usize;
    let pages = bytes.div_ceil(4096);

    let mut fb = match DmaBuffer::alloc(pages) {
        Ok(b) => b,
        Err(e) => {
            say(&alloc::format!(
                "DISP: framebuffer alloc of {} KiB failed: {:?}",
                bytes >> 10,
                e
            ));
            return;
        }
    };
    // What the monitor shows between the modeset and the console arriving. The
    // checks below read this buffer back out of video memory, so whichever is
    // painted carries values they recognise (see `paint_blank`)
    if modeset::DIAG_TEST_PATTERN {
        modeset::paint_test_pattern(fb.as_mut_slice(), pitch, w, h);
    } else {
        modeset::paint_blank(fb.as_mut_slice(), pitch, w, h);
    }

    // Video memory when the CPU can reach it, otherwise system memory. Display
    // scanout is isochronous with hard fetch deadlines, and the class header
    // allowing PCI-coherent does not guarantee sysmem scanout. Asked for at the
    // size the mode needs
    let surface_vram = if vram_ok {
        vram::alloc(
            bytes as u64,
            SURFACE_ALIGN,
            vram::Placement::Low,
            "scanout surface",
        )
    } else {
        None
    };
    let (surface, target, where_) = match surface_vram {
        Some(vram) => {
            crate::drivers::gpu::nvidia::pramin::copy_to_vram(bar0, vram, fb.as_slice());
            let flushed = crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);
            // What is in the scanout surface, sampled at three points, so a
            // copy that stopped at the first aperture differs from one that
            // never started. Compared against the source
            let src = fb.as_slice();
            let mut got = [0u32; 1];
            let mut ok = 0u32;
            let mut first_bad = None;
            // 32 points spread across the surface, enough to tell a copy subtly
            // wrong from one mostly wrong
            const POINTS: u64 = 32;
            for i in 0..POINTS {
                let off = (bytes as u64 / POINTS * i) & !3;
                let o = off as usize;
                let want = u32::from_le_bytes([src[o], src[o + 1], src[o + 2], src[o + 3]]);
                crate::drivers::gpu::nvidia::pramin::sample_vram(bar0, vram + off, &mut got);
                if got[0] == want {
                    ok += 1;
                } else if first_bad.is_none() {
                    first_bad = Some((off, want, got[0]));
                }
            }
            let line = match first_bad {
                None => alloc::format!(" {}/{} ok", ok, POINTS),
                Some((off, want, got)) => alloc::format!(
                    " {}/{} ok, first bad {:#x} want {:#010x} got {:#010x}",
                    ok,
                    POINTS,
                    off,
                    want,
                    got
                ),
            };
            let line = alloc::format!(
                "{} win {:#x}",
                line,
                crate::drivers::gpu::nvidia::pramin::window_reg_in_use()
            );
            DIAG.lock().fb_sample = Some(alloc::format!(
                "{} flush {}",
                line,
                if flushed { "ok" } else { "TIMEOUT" }
            ));
            DIAG.lock().fb_late = Some((vram, pitch));
            (vram, modeset::SURFACE_TARGET_VRAM, "vram")
        }
        _ => (fb.phys(), modeset::SURFACE_TARGET_PCI_COHERENT, "sysmem"),
    };

    // Diagnostic: scan out of system memory instead of video memory.
    //
    // The class supports it (`GetSurfaceAddressTarget` picks
    // `PHYSICAL_PCI_COHERENT` for sysmem on any non-SOC chip). The VRAM copy
    // and readback still happen; only the address given to the window changes.
    //
    // Result: black. The window arms `iso 0x4` (ENABLE clear, TARGET forced
    // back to 1) against the `0x0edb900d` pushed with TARGET 3: the engine
    // refuses a system-memory scanout surface and disables it. The isochronous
    // fetch on this chip reads video memory or nothing
    const DIAG_FB_IN_SYSMEM: bool = false;
    let (surface, target, where_) = if DIAG_FB_IN_SYSMEM {
        (fb.phys(), modeset::SURFACE_TARGET_PCI_COHERENT, "sysmem")
    } else {
        (surface, target, where_)
    };
    // Parked either way: when it is not the scanout surface it is the staging
    // buffer the VRAM copy came from
    fb_mem[slot] = Some(fb);
    say(&alloc::format!(
        "DISP: framebuffer {}x{} pitch {} = {} KiB at {} {:#x}",
        w,
        h,
        pitch,
        bytes >> 10,
        where_,
        surface
    ));

    // The window channel needs its own pushbuffer registered before allocation,
    // like the core channel
    let wpb = match DmaBuffer::alloc(16) {
        Ok(b) => b,
        Err(e) => {
            said(
                Level::Error,
                &say,
                &alloc::format!("DISP: window pushbuffer alloc failed: {:?}", e),
            );
            return;
        }
    };
    let (wpb_phys, wpb_size) = (wpb.phys(), wpb.size() as u64);
    wndw_push_mem[slot] = Some(wpb);
    if let Err(e) = disp::set_channel_pushbuf(
        rpc,
        internal,
        disp::GB202_DISP_WINDOW_CHANNEL_DMA,
        window,
        wpb_phys,
        wpb_size,
        disp::PushbufTarget::SysmemCoherent,
    ) {
        said(Level::Error, &say, &fail("window CHANNEL_PUSHBUFFER", e));
        return;
    }
    if let Err(e) = disp::alloc_dma_channel(
        rpc,
        client,
        h_root,
        disp::GB202_DISP_WINDOW_CHANNEL_DMA,
        window,
        0,
    ) {
        said(Level::Error, &say, &fail("window channel alloc", e));
        return;
    }
    say(&alloc::format!(
        "DISP: window {} (class {:#x}) allocated, pushbuffer at {:#x}",
        window,
        disp::GB202_DISP_WINDOW_CHANNEL_DMA,
        wpb_phys
    ));

    // Somewhere to put the completion record: the top page of the video memory
    // carve, clear of the 8 MiB framebuffer at its base
    let mut ntfy = match vram_ok
        .then(|| vram::alloc(0x1000, 0x1000, vram::Placement::Low, "core update notifier"))
        .flatten()
    {
        Some(addr) => Some(Notifier::Vram(addr)),
        None => DmaBuffer::alloc(1).ok().map(Notifier::Sys),
    };

    // Control measurement: a NOT_BEGUN on the interlocked pair further down
    // says something about the interlock only if the notifier works. This arms
    // the record on a plain, un-interlocked core update (the one
    // coreca7d_update issues at assign_windows time). It stages nothing and the
    // head is armed, so it must report FINISHED
    if let Some(n) = ntfy.as_mut() {
        n.reset(bar0);
        let armed = disp::core_update_notify(core_push, 0, 0, n.phys(), n.target()).is_ok();
        core_push.kick(bar0);
        let consumed = armed && core_push.wait_idle(bar0, 2_000_000_000).is_ok();
        DIAG.lock().update_ntfy_ctl = Some(if consumed {
            notifier_name(n.wait(bar0, 2000))
        } else {
            "not consumed"
        });
    }

    let Some(wpb) = wndw_push_mem[slot].as_mut() else {
        return;
    };
    let mut push = PushBuf::new(
        wpb.as_mut_slice(),
        crate::drivers::gpu::nvidia::gsp_common::pushbuf::wndw_chan_user(window),
    );
    // The window's input LUT, in a buffer of its own. Neutralise the stages not
    // in use before pointing the window at a surface: an inherited LUT or
    // colour matrix turns a correct image into black with no error.
    //
    // Diagnostic: fill this one to full scale while the head's stays identity.
    // The ILUT sits after the fetch and before composition, so a saturating
    // table turns anything that arrives into white, making the window's
    // rectangle a yes/no test of the window:
    //
    //   white rectangle - the window fetches and composites. Remaining faults
    //                     are in memory contents or layout; check `KIND_PITCH`
    //                     against the driver's `BLOCKLINEAR` first
    //   black rectangle - the window contributes nothing. The fault is ahead
    //                     of the ILUT
    //
    // `EvoSetupIdentityBaseLutC5` fills the ILUT with half-floats while
    // `EvoSetupIdentityOutputLutC5` fills the OLUT with 16-bit fixed point.
    // Read as half-floats, a fixed-point identity is denormals for the dark
    // half and NaNs for the bright half
    const DIAG_SATURATE_ILUT: bool = false;
    let ilut = vram_ok
        .then(|| {
            vram::alloc(
                modeset::OLUT_BYTES as u64,
                0x1000,
                vram::Placement::Low,
                "window input LUT",
            )
        })
        .flatten()
        .map(|addr| {
            let mut table = alloc::vec![0u8; modeset::OLUT_BYTES];
            if DIAG_SATURATE_ILUT {
                modeset::build_saturated_ilut(&mut table);
            } else {
                modeset::build_identity_ilut(&mut table);
            }
            crate::drivers::gpu::nvidia::pramin::copy_to_vram(bar0, addr, &table);
            crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);
            addr
        });
    if let Err(e) = modeset::window_reset_pipeline(&mut push, ilut) {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: window reset push failed: {:?}", e),
        );
        return;
    }
    // Diagnostic: a window covering only the top-left quarter of the viewport.
    //
    // A full-screen window covers the desktop colour with
    // `DST_COLOR_FACTOR = ZERO`, so shrinking it splits the screen into three
    // regions, each naming the last stage that worked:
    //
    //   outside the viewport         - overscan, inserted at the raster
    //   inside viewport, outside window - the desktop colour: composition
    //                                     with no memory access at all
    //   inside the window            - the test pattern: needs the ISO fetch
    //                                   and the input LUT on top of the rest
    //
    // A black middle region puts the fault in composition; a blue middle region
    // with a black window puts it in the window's memory path. The framebuffer
    // keeps its full size and pitch, so the window scans the top-left corner of
    // the same test pattern
    const DIAG_QUARTER_WINDOW: bool = false;
    let (ww, wh) = if DIAG_QUARTER_WINDOW {
        (w / 2, h / 2)
    } else {
        (w, h)
    };
    if let Err(e) = modeset::window_set_image(&mut push, surface, pitch, ww, wh, target) {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: window image push failed: {:?}", e),
        );
        return;
    }
    // Not interlocked with the core, unlike nouveau, whose final core update
    // carries real staged state (the cursor and output-LUT half of the head,
    // from `nv50_head_flush_set_wndw`). Here the core stages nothing: the
    // modeset fully committed the head. Requesting the interlock gives a pair
    // that never releases: the core update never begins, while the same
    // notifier on an un-interlocked update reports FINISHED
    if let Err(e) = modeset::window_update(&mut push, false, 0) {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: window update push failed: {:?}", e),
        );
        return;
    }
    push.kick(bar0);
    if push.wait_idle(bar0, 2_000_000_000).is_err() {
        say(&alloc::format!(
            "DISP: window stream not consumed (get {} put {})",
            push.get(bar0),
            push.put_index()
        ));
        return;
    }

    // A closing core update, on the same notifier, naming nobody: the cheapest
    // check that the core channel is healthy after the window went through
    if let Some(n) = ntfy.as_mut() {
        n.reset(bar0);
    }
    let pushed = match ntfy.as_ref() {
        Some(n) => disp::core_update_notify(core_push, 0, 0, n.phys(), n.target()),
        None => disp::core_update(core_push, 0, 0),
    };
    if let Err(e) = pushed {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: releasing core update push failed: {:?}", e),
        );
        return;
    }
    core_push.kick(bar0);
    if core_push.wait_idle(bar0, 2_000_000_000).is_err() {
        say("DISP: core channel did not consume the releasing update");
        return;
    }
    if let Some(n) = ntfy.as_ref() {
        DIAG.lock().update_ntfy = Some(notifier_name(n.wait(bar0, 2000)));
    }
    crate::kcore::time::mdelay(50);

    // Where the window sits on the head lives on the window-immediate channel
    // alone, so it needs a third channel even for a static full-screen image.
    // Left alone it keeps the firmware's value, which can put the surface off
    // the visible raster
    let wimm = match DmaBuffer::alloc(4) {
        Ok(b) => b,
        Err(e) => {
            said(
                Level::Error,
                &say,
                &alloc::format!("DISP: wimm pushbuffer alloc failed: {:?}", e),
            );
            return;
        }
    };
    let (wimm_phys, wimm_size) = (wimm.phys(), wimm.size() as u64);
    wimm_mem[slot] = Some(wimm);
    if let Err(e) = disp::set_channel_pushbuf(
        rpc,
        internal,
        disp::GB202_DISP_WINDOW_IMM_CHANNEL_DMA,
        window,
        wimm_phys,
        wimm_size,
        disp::PushbufTarget::SysmemCoherent,
    ) {
        said(Level::Error, &say, &fail("wimm CHANNEL_PUSHBUFFER", e));
        return;
    }
    if let Err(e) = disp::alloc_dma_channel(
        rpc,
        client,
        h_root,
        disp::GB202_DISP_WINDOW_IMM_CHANNEL_DMA,
        window,
        0,
    ) {
        said(Level::Error, &say, &fail("wimm channel alloc", e));
        return;
    }
    let Some(wimm) = wimm_mem[slot].as_mut() else {
        return;
    };
    let mut ipush = PushBuf::new(
        wimm.as_mut_slice(),
        crate::drivers::gpu::nvidia::gsp_common::pushbuf::wimm_chan_user(window),
    );
    if let Err(e) = modeset::wimm_set_point(&mut ipush, 0, 0) {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: wimm point push failed: {:?}", e),
        );
        return;
    }
    ipush.kick(bar0);
    match ipush.wait_idle(bar0, 2_000_000_000) {
        Ok(()) => {}
        Err(_) => say(&alloc::format!(
            "DISP: wimm stream not consumed (get {} put {})",
            ipush.get(bar0),
            ipush.put_index()
        )),
    }

    // Reports on `SURFACE_LO_ISO`. The closing block reports the core channel;
    // this reports the window
    crate::kcore::time::mdelay(50);
    let armed = spin::Mutex::new(false);
    modeset::read_window_state(bar0, window, &|name, v| {
        // Always, not just when it reads disabled: this word says whether the
        // window is scanning out, and where from
        if name == "SURFACE_LO_ISO" {
            *armed.lock() = v & 1 != 0;
            say(&alloc::format!(
                "R wndw iso {:#010x} {}",
                v,
                if v & 1 == 0 { "DISABLED" } else { "enabled" }
            ));
        }
    });

    // Publish the live surface, only on the hardware's answer: `SURFACE_LO_ISO`
    // with ENABLE set means the window scans this address. Whatever draws later
    // reads these numbers
    if armed.into_inner() && target == modeset::SURFACE_TARGET_VRAM {
        output::with_mut(display_id, |o| {
            o.scanout = Some(LiveScanout {
                vram: surface,
                pitch,
                width: w,
                height: h,
                head,
                window,
            });
        });
        // The first monitor to carry a picture is where the console goes. Said
        // here, since `attach_console_with` below reads `live_scanout()`, the
        // primary output's. Only when there is none yet: a second monitor must
        // not move the shell off the first
        let became_primary = output::primary_id().is_none();
        if became_primary {
            output::set_primary(display_id);
        }

        // The pointer gets its own plane on the head before the console
        // arrives: arming it is a core update on a live screen, and this is
        // where a core update is known to go through. It comes up with a
        // transparent image
        arm_cursor(bar0, rpc, client, h_root, core_push, display_id, head, say);

        // Reach this surface with ordinary stores before the console moves onto
        // it, so the console picks the fast path if there is one. PRAMIN's 1
        // MiB window costs 21 ns per pixel, 44 ms for a full screen.
        //
        // Attached to the shell at once: the five-second settle, the R block
        // and the command's report take several more seconds. Retargeting
        // repaints the boot log onto the panel from the console's cells.
        //
        // Only for the monitor the shell lives on: `attach_console_with` reads
        // `live_scanout()`, the primary's, so calling it for a second monitor
        // would repaint the first monitor's surface
        if became_primary {
            match crate::drivers::gpu::nvidia::scanout::attach_console_with(*bar0) {
                Ok((cw, ch, ms)) => say(&alloc::format!(
                    "DISP: console on the monitor, {}x{} (first paint {} ms)",
                    cw,
                    ch,
                    ms
                )),
                Err(e) => say(&alloc::format!(
                    "DISP: console stays where it was: {} - shell is on serial only",
                    e
                )),
            }
        } else {
            say("DISP: second monitor lit; the console stays on the first");
        }
    }
}

/// 64 by 64 A8R8G8B8. The plane takes 32, 64, 128 or 256; every cursor in this
/// tree is drawn at 64, and the protocol caps a client's `set_cursor` to it
pub const CURSOR_SIZE: u32 = 64;
pub const CURSOR_BYTES: u64 = (CURSOR_SIZE as u64) * (CURSOR_SIZE as u64) * 4;

/// Bring up the head's cursor plane: video memory for the image, the PIO
/// channel that moves it, and one core update to arm both.
///
/// Best-effort: a failure costs one log line and leaves the compositor drawing
/// its own
#[allow(clippy::too_many_arguments)]
fn arm_cursor(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    h_root: u32,
    core_push: &mut crate::drivers::gpu::nvidia::gsp_common::pushbuf::PushBuf,
    display_id: u32,
    head: u32,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gsp_common::{disp, modeset};

    // Opt-out.
    //
    // A display channel that stops retiring takes the window channel with it,
    // and `flip` then sits in `reserve` for a hundred seconds with the
    // compositor thread inside it. So `modeset::cursor::move_to` writes two
    // methods, each behind its own wait, as `MoveCursorC3` in nvidia-open's
    // `nvkms-cursor3.c` does; the interlocks are head state in `cursor_init`.
    //
    // The software path is wrong by construction: the copy engine cannot blend,
    // so the compositor rebuilds the rectangle under the pointer every frame
    // and has to reproduce wallpaper, shadow, rounded corner, ring and blur.
    // The plane has nothing to reproduce.
    //
    // `nohwcursor` on the GRUB line turns it off, for a head with no plane of
    // its own
    if crate::arch::x86_64::grub::cmdline_flag("nohwcursor") {
        return;
    }

    // The class asks 256-byte alignment of a cursor surface; the scanout
    // surface's larger alignment costs nothing to reuse
    let Some(vram_addr) = vram::alloc(CURSOR_BYTES, SURFACE_ALIGN, vram::Placement::Low, "cursor")
    else {
        say("DISP: no video memory for the cursor plane");
        return;
    };

    // Transparent: the plane is armed here and uploaded to later, and video
    // memory nobody wrote need not read as zero
    let blank = alloc::vec![0u8; CURSOR_BYTES as usize];
    crate::drivers::gpu::nvidia::pramin::copy_to_vram(bar0, vram_addr, &blank);
    let _ = crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);

    // A PIO channel, so no pushbuffer is set up first, unlike the wimm channel
    // above
    if let Err(e) = disp::alloc_pio_channel(
        rpc,
        client,
        h_root,
        modeset::GB202_DISP_CURSOR_IMM_CHANNEL_PIO,
        head,
    ) {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: cursor channel alloc failed: {:?}", e),
        );
        vram::free(vram_addr);
        return;
    }

    // Nothing this channel does is atomic with anything else, said before the
    // first update: the flags are inherited
    modeset::cursor_init(bar0, head);

    // The control region answering proves the channel exists; a region nobody
    // owns reads back zero forever
    let free = modeset::free_slots(bar0, head);
    if free == 0 {
        say("DISP: cursor PIO channel has no free slots - plane left off");
        vram::free(vram_addr);
        return;
    }

    if let Err(e) = modeset::head_set_cursor(
        core_push,
        head,
        Some(modeset::CursorArm {
            surface: vram_addr,
            target: modeset::SURFACE_TARGET_VRAM,
            size: CURSOR_SIZE,
            // Zero for the life of the plane. Arming the hot spot would make
            // every `set_cursor` with a different one a core update on a live
            // screen; the host subtracts it from the position it pushes. The
            // PIO point is signed, so a hot spot near the bottom-right still
            // reaches the top-left corner of the panel
            hot_x: 0,
            hot_y: 0,
        }),
    ) {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: cursor push failed: {:?}", e),
        );
        vram::free(vram_addr);
        return;
    }
    if let Err(e) = disp::core_update(core_push, 0, 0) {
        said(
            Level::Error,
            &say,
            &alloc::format!("DISP: cursor core update push failed: {:?}", e),
        );
        vram::free(vram_addr);
        return;
    }
    core_push.kick(bar0);
    if core_push.wait_idle(bar0, 2_000_000_000).is_err() {
        say("DISP: core channel did not consume the cursor update");
        vram::free(vram_addr);
        return;
    }

    // Park it at the origin, invisibly since the image is transparent. This
    // proves the PIO channel takes a method
    let moved = modeset::move_to(bar0, head, 0, 0);
    output::with_mut(display_id, |o| {
        o.cursor = Some(LiveCursor {
            vram: vram_addr,
            size: CURSOR_SIZE,
            head,
        });
    });
    say(&alloc::format!(
        "DISP: cursor {}x{} at vram {:#x} head {}, PIO free {}{}",
        CURSOR_SIZE,
        CURSOR_SIZE,
        vram_addr,
        head,
        free,
        if moved { "" } else { " (first move timed out)" }
    ));
}

// The live state of an output (what is scanned out on it, whether its head has
// a cursor plane armed) lives in `gsp_common::output`, one entry per monitor.
// Re-exported so `blackwell::boot::live_scanout` and friends resolve for
// `scanout.rs`, `mayland` and the shell.
pub use crate::drivers::gpu::nvidia::gsp_common::output::{
    clear_live_cursor, live_cursor, live_scanout, set_live_surface, LiveCursor, LiveScanout,
};
