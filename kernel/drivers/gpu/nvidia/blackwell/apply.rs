// The display layer's update on this card: every head's change, one UPDATE
//
// K2 of `dev/kms-architecture.md`. One batch, as
// `KickoffProposedModeSetHwState` (`nvkms-modeset.c`) writes every changed
// head's PreUpdate and kicks one `KickoffModesetUpdateState`. Lighting a head
// is not here: it allocates the head's channels first and runs its own modeset
// (`control::light`), which the display layer's driver does before calling
// this.
//
// The shape
//
// `prepare` decides everything needing memory or a register read: a new surface
// for a new raster, the next output LUT filled with a new curve, the tiles and
// physical windows the head keeps. `emit` turns those decisions into methods
// and reads nothing, so `nvidia selftest` can build a batch in a plain buffer
// and count its updates. `commit` sends it.
//
// One session of the core channel
//
// A pushbuffer is resumed at GET, so an update written in a second
// `with_display`, with GET unmoved, would overwrite the head methods. The core
// channel is resumed once, the batch is measured and the ring wound to fit it,
// and the result is read back off the armed state.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::gpu::kms;
use crate::drivers::gpu::nvidia::gsp_common::disp::{self, DpcdCaps};
use crate::drivers::gpu::nvidia::gsp_common::modeset::{self, HeadColour, Mode, OlutRing};
use crate::drivers::gpu::nvidia::gsp_common::output;
use crate::drivers::gpu::nvidia::gsp_common::pushbuf::{self, PushBuf, PushError};
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::gpu::nvidia::{pramin, scanout};
use crate::kcore::klog::Level;
use crate::mm::dma::DmaBuffer;

use super::gspboot::DisplayMemory;

/// One head's part of a request, as the display layer passed it
pub struct HeadPlan {
    /// The output's name, for the lines that say what happened
    pub output: String,
    pub display_id: u32,
    /// A new timing, with its place in the monitor's list
    pub mode: Option<(usize, Mode)>,
    /// A new colour
    pub colour: Option<HeadColour>,
    /// `Some(true)` puts the monitor to sleep, `Some(false)` wakes it
    pub sleep: Option<bool>,
}

/// A new raster's decisions, made before the batch
pub struct Raster {
    /// The window's size: the mode's, clamped to the viewport as
    /// `attach_window` clamps it: the hardware declines a window larger than
    /// the viewport it feeds without reporting an error
    pub width: u32,
    pub height: u32,
    /// Physical windows for the head's two windows
    pub phywins: [u32; 2],
    /// Tiles `modeset::assign` handed out, when the caps were read
    pub tiles: Option<u32>,
    /// Every other lit head and the width it scans: they keep their tiles
    pub keep: Vec<(u32, u32)>,
    /// Every head's armed tile mask, read before the batch
    pub armed_tiles: [u32; disp::CORE_HEADS as usize],
}

/// Everything the batch writes for one head, decided
pub struct Emit {
    pub head: u32,
    pub display_id: u32,
    pub sor: u32,
    /// The SOR protocol to put the head on
    pub protocol: u32,
    /// The timing the head ends with
    pub mode: Mode,
    /// The colour it ends with
    pub colour: HeadColour,
    /// The output LUT it ends on
    pub olut: Option<u64>,
    /// The colour changes: its methods go in, with the switch to `olut`
    pub recolour: bool,
    /// Asleep once the update lands
    pub asleep: bool,
    /// Sleep or wake: who the head feeds changes
    pub feed: bool,
    pub raster: Option<Raster>,
    /// The head's cursor plane, image address and side, when it has one armed:
    /// `head_set_mode` switches the plane off, so a new raster puts it back in
    /// the same update
    pub cursor: Option<(u64, u32)>,
}

/// The batch: every head's methods, then one update, interlocked with the
/// windows `interlock` names, the ones getting a new surface
pub fn emit(push: &mut PushBuf, heads: &[Emit], interlock: u32) -> Result<(), PushError> {
    for e in heads {
        let h = e.head * modeset::HEAD_STRIDE;
        if e.raster.is_some() {
            // The whole head block: raster, viewport, clocks, the colour it
            // ends with and the LUT it ends on. Passing no LUT here disables
            // it, a black screen (`gb206-display.md`, FP16)
            modeset::head_set_mode(push, e.head, &e.mode, e.display_id, e.olut, &e.colour)?;
            // The plane as `attach_window` armed it: the same image, the hot
            // spot at zero (the host subtracts it from the position)
            if let Some((surface, size)) = e.cursor {
                modeset::head_set_cursor(push, e.head, Some(modeset::CursorArm {
                    surface, target: modeset::SURFACE_TARGET_VRAM, size, hot_x: 0, hot_y: 0,
                }))?;
            }
        } else if e.recolour {
            modeset::head_colour_methods(push, h, &e.colour, e.olut)?;
            // The depth is in the output resource, which a modeset also writes.
            // Re-emitting it with the head's own mode asks for ten bits without
            // a mode change
            push.mthd1(
                modeset::M_HEAD_SET_CONTROL_OUTPUT_RESOURCE + h,
                modeset::output_resource_value(&e.mode, &e.colour),
            )?;
        }
        // Who the head feeds. `head_set_mode` names the display but writes no
        // SOR_SET_CONTROL, so a new raster says it as well, asleep or awake: a
        // sleeping monitor given a new mode stays asleep
        if e.raster.is_some() || e.feed {
            if e.asleep {
                modeset::sor_detach(push, e.sor)?;
                push.mthd1(modeset::M_HEAD_SET_DISPLAY_ID + h, 0)?;
            } else {
                modeset::sor_set_control(push, e.sor, e.head, e.protocol)?;
                push.mthd1(modeset::M_HEAD_SET_DISPLAY_ID + h, e.display_id)?;
            }
        }
        // The capacity the new width is budgeted against, and the tile the head
        // composites into, sized in pixels, so it changes with the mode
        if let Some(r) = &e.raster {
            disp::core_window_usage(push, e.head, r.width, r.phywins)?;
            disp::claim_tiles(push, &r.armed_tiles, e.head, r.width, &r.keep, r.tiles)?;
        }
    }
    disp::core_update(push, 0, interlock)
}

/// `emit` into a buffer of dwords that is not a channel's. Returns how many it
/// wrote: what the ring makes room for, and what `nvidia selftest` reads back
pub fn emit_into(words: &mut [u32], heads: &[Emit], interlock: u32) -> Result<usize, PushError> {
    // A slice of dwords and not of bytes: `PushBuf` writes dwords, aligned
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, words.len() * 4)
    };
    let mut p = PushBuf::new(bytes, 0);
    emit(&mut p, heads, interlock)?;
    Ok(p.used())
}

/// What one head needs besides its methods: the facts for the RPCs around the
/// batch, a new surface and what it replaces, the LUT ring once the head has
/// moved on
struct Prepared {
    output: String,
    /// Into the monitor's list, for the line that says which mode
    mode_index: Option<usize>,
    is_dp: bool,
    connected: bool,
    dpcd: Option<DpcdCaps>,
    surface: Option<NewSurface>,
    olut_after: Option<OlutRing>,
}

struct NewSurface {
    vram: u64,
    pitch: u32,
    window: u32,
    /// The black frame copied into the surface, kept as the console's staging
    /// copy, as in `attach_window`
    staging: Option<DmaBuffer>,
    /// The surface being scanned now, freed once nothing reads it
    old: u64,
}

impl Prepared {
    /// Give back what was taken for a batch that never went out
    fn release(&mut self) {
        if let Some(s) = self.surface.take() {
            crate::drivers::gpu::nvidia::gsp_common::vram::free(s.vram);
        }
    }
}

/// Decide one head's part, and take the memory it needs
fn prepare(bar0: &MmioRegion, p: &HeadPlan) -> Result<(Emit, Prepared), String> {
    use crate::drivers::gpu::nvidia::gsp_common::vram;

    let Some((head, sor, or, dpcd, connected, olut, scanout, cursor)) = output::with(p.display_id, |o| {
        Some((o.head?, o.sor?, o.or, o.dpcd, o.connected, o.olut, o.scanout, o.cursor))
    })
    .flatten() else {
        return Err(format!("{}: nothing drives it", p.output));
    };
    let Some(rec) = kms::heads::get(p.display_id) else {
        return Err(format!("{}: the display layer has no record of its head", p.output));
    };

    let colour_now = rec.colour.unwrap_or(HeadColour::DEFAULT);
    let colour = p.colour.unwrap_or(colour_now);
    let recolour = p.colour.is_some_and(|c| c != colour_now);
    let mode = p.mode.map_or(Mode::from_timing(&rec.mode), |(_, m)| m);
    let asleep = p.sleep.unwrap_or(rec.asleep);

    // A new curve into the next table, which the head is not reading until the
    // update switches it. Nothing visible happens here
    let (olut_addr, olut_after) = match (olut, recolour) {
        (Some(ring), true) => {
            let next = ring.next();
            let mut table = alloc::vec![0u8; modeset::OLUT_BYTES];
            modeset::build_olut(&mut table, &colour.curve);
            pramin::copy_to_vram(bar0, next.live(), &table);
            pramin::flush_fb(bar0);
            (Some(next.live()), Some(next))
        }
        (Some(ring), false) => (Some(ring.live()), None),
        (None, _) => (None, None),
    };

    let mut surface = None;
    let raster = match p.mode {
        None => None,
        Some((_, m)) => {
            let Some(old) = scanout else {
                return Err(format!("{}: it has no surface - it was never brought up", p.output));
            };
            let (w, h) = (
                m.h_display.min(modeset::viewport_width(&m)),
                m.v_display.min(modeset::viewport_height(&m)),
            );
            let pitch = (w * 4).next_multiple_of(modeset::PITCH_GRANULARITY);
            let bytes = pitch as u64 * h as u64;
            // A new surface: the old one is being scanned, and the window keeps
            // fetching it until the update arms. Freed afterwards, once nothing
            // reads it
            let Some(vram) = vram::alloc(bytes, super::window::SURFACE_ALIGN, vram::Placement::Low,
                                         "scanout surface") else {
                return Err(format!("{}: no video memory for a surface at that size", p.output));
            };
            let mut staging = match DmaBuffer::alloc((bytes as usize).div_ceil(4096)) {
                Ok(b) => b,
                Err(_) => {
                    vram::free(vram);
                    return Err(format!("{}: no system memory for the staging copy", p.output));
                }
            };
            // Opaque black, so the first frame of the new mode is not the
            // previous tenant's pixels
            modeset::paint_blank(staging.as_mut_slice(), pitch, w, h);
            surface = Some(NewSurface { vram, pitch, window: head * 2, staging: Some(staging), old: old.vram });

            let plan = super::modeset::plan_tiles(bar0, head);
            Some(Raster {
                width: w,
                height: h,
                phywins: plan.map_or([1, 2], |t| t.phywins),
                tiles: plan.map(|t| t.tiles),
                keep: output::lit_heads().into_iter().filter(|&(x, _)| x != head).collect(),
                armed_tiles: disp::armed_tile_masks(bar0),
            })
        }
    };

    Ok((
        Emit {
            head,
            display_id: p.display_id,
            sor,
            protocol: super::control::sor_protocol(or.protocol, or.is_dp()),
            mode,
            colour,
            olut: olut_addr,
            recolour,
            asleep,
            feed: p.sleep.is_some(),
            raster,
            cursor: cursor.filter(|c| c.head == head).map(|c| (c.vram, c.size)),
        },
        Prepared {
            output: p.output.clone(),
            mode_index: p.mode.map(|(i, _)| i),
            is_dp: or.is_dp(),
            connected,
            dpcd,
            surface,
            olut_after,
        },
    ))
}

/// How a batch that did not go through went wrong
enum Failed {
    /// Nothing was armed: the screen is as it was
    Untouched(&'static str),
    /// The update was handed over and not seen to land: what is on the screen
    /// is unknown, so nothing is freed
    Unknown(&'static str),
}

/// Apply `plans` with one update. A line for each thing that happened
pub fn commit(plans: &[HeadPlan]) -> Result<Vec<String>, String> {
    // Two new rasters in one update need the tiles of both decided together,
    // the second head's work (2.11), with no monitor to check it on
    if plans.iter().filter(|p| p.mode.is_some()).count() > 1 {
        return Err(String::from(
            "one new mode per request - two at once come with a second monitor to check them on"));
    }
    let bar0 = crate::drivers::gpu::nvidia::device().ok_or("no GB20x in this machine")?.bar0;

    let mut emits = Vec::new();
    let mut prepared: Vec<Prepared> = Vec::new();
    for p in plans {
        match prepare(&bar0, p) {
            Ok((e, x)) => {
                emits.push(e);
                prepared.push(x);
            }
            Err(e) => {
                prepared.iter_mut().for_each(Prepared::release);
                return Err(e);
            }
        }
    }
    let interlock = prepared.iter().filter_map(|x| x.surface.as_ref()).fold(0, |m, s| m | 1 << s.window);

    // The console paints through a raw address, and a new raster on its head
    // is about to retire the surface that address describes
    let console = emits.iter().zip(&prepared).any(|(e, x)| {
        x.surface.is_some() && Some(e.display_id) == output::primary_id()
    }) && scanout::console_attached();
    if console {
        scanout::detach_console();
    }

    let sent = super::control::with_rm_and_disp(|bar0, rpc, client, mem| {
        send(bar0, rpc, client, mem, &emits, &mut prepared, interlock)
    })
    .unwrap_or(Err(Failed::Untouched("GSP-RM is not booted")));

    let mut lines = Vec::new();
    let result = match sent {
        Ok(()) => {
            for (e, x) in emits.iter().zip(prepared.iter_mut()) {
                lines.extend(record(&bar0, e, x));
            }
            Ok(lines)
        }
        Err(Failed::Untouched(why)) => {
            prepared.iter_mut().for_each(Prepared::release);
            Err(String::from(why))
        }
        Err(Failed::Unknown(why)) => Err(format!(
            "{} - the screen may be in either state; 'nvidia disp' and 'nvidia gb206-diff' read it", why)),
    };

    if console {
        let _ = scanout::attach_console_with(bar0);
    }
    result
}

/// The batch, in one session of the core channel: methods, the windows' new
/// surfaces, the monitors told, one update, and the monitors woken
#[allow(clippy::too_many_arguments)]
fn send(
    bar0: &MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    mem: &mut DisplayMemory,
    emits: &[Emit],
    prepared: &mut [Prepared],
    interlock: u32,
) -> Result<(), Failed> {
    let DisplayMemory { core_push, wndw_push, framebuffer, .. } = mem;
    let Some(pb) = core_push.as_mut() else {
        return Err(Failed::Untouched("no core pushbuffer - the display bring-up did not get that far"));
    };

    // The new surfaces painted black before any head is pointed at them
    for x in prepared.iter() {
        if let Some(s) = &x.surface {
            if let Some(st) = &s.staging {
                pramin::copy_to_vram(bar0, s.vram, st.as_slice());
            }
        }
    }
    pramin::flush_fb(bar0);

    // The batch into the core ring, not kicked yet. Measured first, so the ring
    // winds to make room: PUT cannot address past 4 KiB, and a stream that does
    // is silently truncated
    let n = {
        let mut scratch = alloc::vec![0u32; 1024];
        emit_into(&mut scratch, emits, interlock)
            .map_err(|_| Failed::Untouched("the update does not fit a core pushbuffer"))?
    };
    let cur = pushbuf::channel_get(bar0, pushbuf::CORE_CHAN_USER);
    let mut push = PushBuf::resume(pb.as_mut_slice(), pushbuf::CORE_CHAN_USER, cur);
    push.reserve(bar0, n + 1, 2_000_000_000)
        .map_err(|_| Failed::Untouched("the core channel has no room and did not drain"))?;
    emit(&mut push, emits, interlock)
        .map_err(|_| Failed::Untouched("the core channel would not take the methods"))?;

    // The windows: each new surface, in an update interlocked with the core one
    // above, so the head's new raster and the window's new surface arrive on
    // the same frame
    for (e, x) in emits.iter().zip(prepared.iter()) {
        let Some(s) = &x.surface else { continue };
        let Some(Some(wpb)) = wndw_push.get_mut(e.head as usize) else {
            return Err(Failed::Untouched("no window pushbuffer for that head"));
        };
        let user = pushbuf::wndw_chan_user(s.window);
        let cur = pushbuf::channel_get(bar0, user);
        let mut wp = PushBuf::resume(wpb.as_mut_slice(), user, cur);
        let r = e.raster.as_ref().map_or((0, 0), |r| (r.width, r.height));
        let pushed = wp.reserve(bar0, 32, 100_000_000).is_ok()
            && modeset::window_set_image(&mut wp, s.vram, s.pitch, r.0, r.1, modeset::SURFACE_TARGET_VRAM).is_ok()
            && modeset::window_update(&mut wp, true, 0).is_ok();
        if !pushed {
            return Err(Failed::Untouched("the window channel would not take the surface"));
        }
        wp.kick(bar0);
    }

    // From here the batch goes out whatever happens: a window kicked above
    // waits for its partner, this update.
    //
    // A monitor going to sleep is told before the head lets go of it, and its
    // main link goes down: the sink should never see a head driving a link that
    // is not powered (`nvRmSetDpmsEvo`: a DisplayPort sink to D3 first,
    // `DeviceImpl::setPanelPowerParams`; anything else `SET_MONITOR_POWER`).
    // Not on a connector with nothing on it: the hot-plug pass sleeps those
    for (e, x) in emits.iter().zip(prepared.iter()) {
        if !(e.feed && e.asleep) {
            continue;
        }
        if x.is_dp {
            if x.connected {
                if let Some(d) = x.dpcd.as_ref() {
                    if let Err(err) = modeset::dp_set_sink_power(rpc, client, e.display_id, d.rev, false) {
                        crate::kwarn!("[nvidia] {:#010x} did not take D3: {:?}", e.display_id, err);
                    }
                }
            }
            let _ = modeset::dp_main_link_power(rpc, client, e.display_id, false);
        } else if x.connected {
            let _ = modeset::set_monitor_power(rpc, client, e.display_id, e.head, false);
        }
    }

    push.kick(bar0);
    if push.wait_idle(bar0, 2_000_000_000).is_err() {
        return Err(Failed::Unknown("the core channel did not consume the update"));
    }

    // Woken: the link up once the head has taken it back, then the sink out of
    // D3 and the link checked; a sink that slept may have let it go
    for (e, x) in emits.iter().zip(prepared.iter()) {
        if !(e.feed && !e.asleep) {
            continue;
        }
        if x.is_dp {
            let _ = modeset::dp_main_link_power(rpc, client, e.display_id, true);
            if x.connected {
                if let Some(d) = x.dpcd.as_ref() {
                    if let Err(err) = modeset::dp_set_sink_power(rpc, client, e.display_id, d.rev, true) {
                        crate::kwarn!("[nvidia] {:#010x} did not wake: {:?}", e.display_id, err);
                    }
                }
                let (level, what) = super::control::restore_link(rpc, client, e.display_id, x.dpcd.as_ref());
                crate::kcore::klog::emit(level, crate::klog_site!(),
                    format_args!("[nvidia] {:#010x} woken: {}", e.display_id, what));
            }
        } else if x.connected {
            let _ = modeset::set_monitor_power(rpc, client, e.display_id, e.head, true);
        }
    }

    // The staging copies are kept for the console's damaged rectangles, as in
    // `attach_window`, in the head's slot
    for (e, x) in emits.iter().zip(prepared.iter_mut()) {
        if let Some(st) = x.surface.as_mut().and_then(|s| s.staging.take()) {
            if let Some(slot) = framebuffer.get_mut(e.head as usize) {
                *slot = Some(st);
            }
        }
    }
    Ok(())
}

/// The update landed: write down what the hardware now does, give back what
/// nothing reads any more, and say what happened
fn record(bar0: &MmioRegion, e: &Emit, x: &mut Prepared) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(s) = x.surface.take() {
        // Only now is the old surface not being read
        crate::drivers::gpu::nvidia::gsp_common::vram::free(s.old);
        let (w, h) = e.raster.as_ref().map_or((0, 0), |r| (r.width, r.height));
        output::with_mut(e.display_id, |o| {
            o.scanout = Some(super::window::LiveScanout {
                vram: s.vram, pitch: s.pitch, width: w, height: h, head: e.head, window: s.window,
            });
        });
        let (level, line) = took(bar0, e.head, &e.mode);
        let c = e.mode.refresh_centihz();
        let said = format!("{}: mode {}: {}x{} @ {}.{:02} Hz - {}",
            x.output, x.mode_index.unwrap_or(0), e.mode.h_display, e.mode.v_display,
            c / 100, c % 100, line);
        crate::kcore::klog::emit(level, crate::klog_site!(), format_args!("[nvidia] {}", said));
        lines.push(said);
    }
    if let Some(ring) = x.olut_after {
        output::with_mut(e.display_id, |o| o.olut = Some(ring));
    }
    kms::heads::update(e.display_id, |r| {
        if e.raster.is_some() {
            r.mode = e.mode.to_timing();
        }
        if e.recolour {
            r.colour = Some(e.colour);
        }
        if e.feed {
            r.asleep = e.asleep;
        }
    });
    if e.recolour {
        lines.push(format!("{}: colour set", x.output));
    }
    if e.feed {
        lines.push(format!("{}: {}", x.output,
            if e.asleep { "asleep (DPMS off) - the surface is untouched" } else { "woken" }));
    }
    lines
}

/// Whether the head took the raster: the armed state read back. A modeset arms
/// through the supervisor a frame or two after the channel has read it, so this
/// waits up to 200 ms
fn took(bar0: &MmioRegion, head: u32, mode: &Mode) -> (Level, String) {
    let want = mode.refresh_centihz() as u64 * 10;
    let mut got = 0u64;
    for _ in 0..50 {
        got = modeset::head_raster(bar0, head).refresh_mhz() as u64;
        if got.abs_diff(want) * 200 <= want {
            return (Level::Info, format!("the head is rastering at {}.{:02} Hz", got / 1000, got % 1000 / 10));
        }
        crate::sched::sleep(1);
    }
    (Level::Warn, format!("but the head reports {}.{:02} Hz - the update did not arm it", got / 1000, got % 1000 / 10))
}
