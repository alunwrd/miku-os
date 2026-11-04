// Driving the head, the SOR and the DP link, and the closing `R` block
// reporting what landed. The diagnostic statics live here since
// `settle_and_report` reads them.

use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use crate::drivers::gpu::nvidia::le;




// One record for the diagnostics. Each is written once as the boot goes past
// what it describes, and read once by `settle_and_report` for the closing `R`
// block.
//
// Nothing behaves differently for what is in here, and the boot still produces
// its report when it fails before the boot state exists.
#[derive(Default)]
pub struct DiagReport {
    /// Surface address and pitch, sampled when the window armed, for the
    /// late-probe read at the end of `settle_and_report`
    pub fb_late: Option<(u64, u32)>,
    /// What the underflow logger answered, or why it could not be asked
    pub underflow: Option<alloc::string::String>,
    /// The four probe pixels as they read straight after the surface was
    /// written
    pub fb_sample: Option<alloc::string::String>,
    /// RM's own view of the head, from the control call
    pub rm_view: Option<alloc::string::String>,
    /// Status of the last window UPDATE notifier
    pub update_ntfy: Option<&'static str>,
    /// Status of the core UPDATE notifier, printed first: it gives the window's
    /// number its meaning
    pub update_ntfy_ctl: Option<&'static str>,
    /// Whether the output LUT read back the way it was written
    pub olut_state: Option<alloc::string::String>,
    /// Armed cursor-composition mode
    pub cursor_comp: Option<u32>,
    /// Armed OCSC0 diagonal
    pub ocsc_was: Option<(u32, u32, u32)>,
    /// Which physical tile the head claimed
    pub tile_claim: Option<u32>,
    /// Armed clamp / OCSC / stall gates
    pub gates_was: Option<(u32, u32, u32, u32)>,
}

pub static DIAG: spin::Mutex<DiagReport> = spin::Mutex::new(DiagReport {
    fb_late: None, underflow: None, fb_sample: None, rm_view: None,
    update_ntfy: None, update_ntfy_ctl: None, olut_state: None,
    cursor_comp: None, ocsc_was: None, tile_claim: None, gates_was: None,
});


/// `NV5070_CTRL_CMD_{SET,GET}_RG_UNDERFLOW_PROP` on the display object.
///
/// One parameter block serves both: `subdeviceIndex`, `head`, `enable`,
/// `underflow`, `epfifoUnderflow`, `mode`, six words, in and out. On the set
/// pass `enable` turns reporting on and `mode` picks what the hardware paints
/// when the raster runs dry: REPEAT, which repeats the last pixel, or RED. On
/// the get pass the same fields come back filled, and the read clears the
/// latched bits, so it is asked once
pub fn underflow_prop(
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    h_root: u32,
    head: u32,
    set: bool,
    say: &dyn Fn(&str),
) {
    const SET_RG_UNDERFLOW_PROP: u32 = 0x5070_0204;
    const GET_RG_UNDERFLOW_PROP: u32 = 0x5070_0203;
    /// What the raster generator paints when it runs dry. Repeating the last
    /// pixel is the shipping behaviour and the less disruptive. Reporting stays
    /// on, and `R under` says whether an underflow happened
    const MODE_REPEAT: u32 = 0;
    const ENABLE_YES: u32 = 1;
    const CLEAR_UNDERFLOW_YES: u32 = 1;

    let mut params = [0u32; 6];
    params[1] = head;
    if set {
        params[2] = ENABLE_YES;
        params[3] = CLEAR_UNDERFLOW_YES;
        params[5] = MODE_REPEAT;
    }
    let mut bytes = alloc::vec![0u8; params.len() * 4];
    for (i, w) in params.iter().enumerate() {
        le::put_u32(&mut bytes, i * 4, *w);
    }

    let cmd = if set { SET_RG_UNDERFLOW_PROP } else { GET_RG_UNDERFLOW_PROP };
    match client.control(rpc, h_root, cmd, &bytes) {
        Ok(reply) if !set => {
            let word = |i: usize| -> u32 {
                match reply.get(i * 4..i * 4 + 4) {
                    Some(b) => u32::from_le_bytes(b.try_into().unwrap()),
                    None => u32::MAX,
                }
            };
            DIAG.lock().underflow = Some(alloc::format!(
                "en {} under {} epfifo {} mode {}",
                word(2), word(3), word(4), word(5)
            ));
            // The latch is the one sign the display fetched too slowly: the
            // panel showed the underflow colour for part of a frame
            if word(3) != 0 && word(3) != u32::MAX {
                crate::kwarn!("[nvidia] head {} underflowed (latch {}, epfifo {}) - the fetch fell behind the raster",
                    head, word(3), word(4));
            }
        }
        Ok(_) => {}
        Err(e) => {
            let what = if set { "arm" } else { "read" };
            DIAG.lock().underflow = Some(alloc::format!("{} failed {:?}", what, e));
        }
    }
}

/// RM's own view of an output's link, for `R rm` in the report: the lanes and
/// rate it trained and the pattern the main link emits. Asked when the report
/// is, never on the boot path; GET_ACTIVE is left out (see `apply`)
pub fn rm_link_view(
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    display_id: u32,
) -> alloc::string::String {
    use crate::drivers::gpu::nvidia::gsp_common::modeset;
    let linkcfg = modeset::dp_get_link_config(rpc, client, display_id);
    let pat = modeset::dp_get_testpattern(rpc, client, display_id);
    alloc::format!(
        " link={} pat={}",
        match linkcfg {
            Ok((lanes, bw)) => alloc::format!("{}x{:#x}", lanes, bw),
            Err(e) => alloc::format!("err {:?}", e),
        },
        match pat {
            Ok(v) => alloc::format!("{}", modeset::testpattern_name(v)),
            Err(e) => alloc::format!("err {:?}", e),
        },
    )
}

/// Let the picture settle, then report on it. The boot goes on to park every
/// buffer the hardware reads in `gspboot::DISP_MEM`
pub fn settle_and_report(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    head: u32,
    sor: u32,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gsp_common::modeset as ms;

    // Sample the frame counters over a measured window: the refresh rate is
    // frames over elapsed time, so a short window will do. 600ms is 36 frames
    // at 60Hz, good to about 1.5 Hz
    const FRAME_SAMPLE_MS: u64 = 600;
    let (rg0, pc0) = ms::head_frame_counters(bar0, head);
    let sw = crate::kcore::time::Stopwatch::start();
    crate::kcore::time::mdelay(FRAME_SAMPLE_MS);
    let (rg1, pc1) = ms::head_frame_counters(bar0, head);
    let sample_ms = sw.elapsed_ms().max(1);

    // Everything the boot recorded on its way here, taken out in one lock.
    // Copied into locals: the report calls into the hardware between prints
    let (fb_sample, rm_view, ntfy_ctl, ntfy, olut, cursor_comp,
         gates_was, ocsc_was, tile_claim, underflow, fb_late) = {
        let d = DIAG.lock();
        (d.fb_sample.clone(), d.rm_view.clone(), d.update_ntfy_ctl, d.update_ntfy,
         d.olut_state.clone(), d.cursor_comp, d.gates_was, d.ocsc_was,
         d.tile_claim, d.underflow.clone(), d.fb_late)
    };

    // Compact dump, sized to copy off a screen by hand
    say("=== R ===");
    let d_rg = rg1.wrapping_sub(rg0);
    let d_pc = pc1.wrapping_sub(pc0);
    // Tenths of a hertz, in integers: a raster not running reads 0.0, and one
    // running reads the mode's refresh rate
    let hz10 = (d_rg as u64 * 10_000) / sample_ms;
    say(&alloc::format!(
        "R frames rg+{} pc+{} over {} ms ({}.{} Hz)",
        d_rg, d_pc, sample_ms, hz10 / 10, hz10 % 10
    ));
    let (wm, hs, vs) = ms::sf_stream_state(bar0, head);
    say(&alloc::format!("R sf {:#x} {:#x} {:#x}", wm, hs, vs));
    let s = ms::sor_dp_state(bar0, sor);
    say(&alloc::format!(
        "R sor{} ctl {:#x} pa {:#x} pb {:#x} ln {:#x} tg {:#x}",
        sor, s[0].2, s[1].2, s[2].2, s[3].2, s[4].2
    ));
    // The SOR pattern register is not decoded: the only table for it is
    // Maxwell-era, and two of its neighbours read back as PRI poison here.
    // `R rm pat=` answers from RM. Only the armed methods that failed to take;
    // when the hardware agrees with everything staged, this is one word
    let mut differs = alloc::string::String::new();
    for m in ms::read_core_state(bar0, head, sor, head * 2) {
        if m.assy != m.arm {
            differs.push_str(&alloc::format!(
                " {}={:#x}/{:#x}", m.name, m.assy, m.arm
            ));
        }
    }
    say(&alloc::format!(
        "R core{}", if differs.is_empty() { " all-armed".into() } else { differs }
    ));
    // Sampled when the surface was written, reported here so it lands in the
    // block that gets copied
    if let Some(fbs) = fb_sample.as_ref() {
        say(&alloc::format!("R fbmem{}", fbs));
    }
    if let Some(rv) = rm_view.as_ref() {
        say(&alloc::format!("R rm{}", rv));
    }
    // Control first: it gives the second number its meaning
    say(&alloc::format!(
        "R update ctl {} last {}",
        ntfy_ctl.unwrap_or("none"),
        ntfy.unwrap_or("none")
    ));
    // Front-end state of both channels. Idle is 0xb0000 for the core and
    // 0x40000 for the window; anything else is a channel holding something
    let (core_st, wndw_st) = ms::channel_state(bar0, head * 2);
    say(&alloc::format!(
        "R chn core {:#010x} wndw {:#010x} olut {} cc-was {:#x}",
        core_st, wndw_st,
        olut.as_deref().unwrap_or("none"),
        cursor_comp.unwrap_or(0)
    ));
    let (cg, crb, oc, sl) = gates_was.unwrap_or((0, 0, 0, 0));
    say(&alloc::format!(
        "R gates-was clamp {:#x}/{:#x} ocsc0 {:#x} stall {:#x}",
        cg, crb, oc, sl
    ));
    let (o0, o1, o2) = ocsc_was.unwrap_or((0, 0, 0));
    say(&alloc::format!(
        "R ocsc-was {:#x} {:#x} {:#x} tile {}",
        o0, o1, o2,
        match tile_claim {
            Some(t) => alloc::format!("{}", t),
            None => alloc::string::String::from("none"),
        }
    ));
    // The window's armed half, at its page base + 0x800. The staged half only
    // echoes what we wrote
    let mut warm = alloc::string::String::new();
    for (name, v) in ms::window_armed_state(bar0, head * 2) {
        warm.push_str(&alloc::format!(" {} {:#x}", name, v));
    }
    say(&alloc::format!("R warm{}", warm));
    // Same rule for exceptions: name them only when there are any
    let exc = spin::Mutex::new(alloc::string::String::new());
    ms::read_exceptions(bar0, head * 2, &|line| {
        // "none"/"NONE" are the per-channel all-clear; the pending header is
        // clear when both of its masks read zero
        let all_clear = line.contains("none")
            || line.contains("NONE")
            || line.contains("pending: other 0x00000000 winim 0x00000000");
        if !all_clear {
            exc.lock().push_str(&alloc::format!(" [{}]", line));
        }
    });
    let exc = exc.into_inner();
    say(&alloc::format!(
        "R exc{}", if exc.is_empty() { " clean".into() } else { exc }
    ));
    say(&alloc::format!(
        "R under {}",
        underflow.as_deref().unwrap_or("not asked")
    ));
    // The scanout surface read again, five seconds into scanning. Four pixels
    // whose values are known whether the surface came up blank or carrying the
    // test pattern (`modeset::late_probe_points`). Anything else means video
    // memory did not keep what was written
    if crate::drivers::gpu::nvidia::scanout::console_attached() {
        // The console owns the surface by now and has painted over the probe
        // pixels, so reading them would report a false failure. Text staying on
        // the panel answers the same question
        say("R fblate console owns the surface");
    } else if let Some((vram, pitch)) = fb_late {
        let mut got = [0u32; 1];
        let mut line = alloc::string::String::new();
        for (off, want) in
            crate::drivers::gpu::nvidia::gsp_common::modeset::late_probe_points(pitch)
        {
            crate::drivers::gpu::nvidia::pramin::sample_vram(bar0, vram + off, &mut got);
            line.push_str(&alloc::format!(
                " {}", if got[0] == want { alloc::format!("ok") } else { alloc::format!("{:x}", got[0]) }
            ));
        }
        say(&alloc::format!("R fblate{}", line));
    }
    // The whole armed state against the shipping driver's, in one pass
    crate::drivers::gpu::nvidia::gsp_common::refstate::diff_against_driver(
        bar0, head, sor, head * 2, 24, &say,
    );
    say("=== R end ===");
}


/// 8 bits per component: what every head is lit with. NVKMS starts at the most
/// the panel claims and steps down when the link or IMP refuses
/// (`nvChooseCurrentColorSpaceAndRangeEvo`, `nvDowngradeColorBpc`); here ten
/// bits is asked for afterwards (`display color depth=10`), and every check
/// uses the depth the head is sending (`dev/display-nvkms.md`, 2.6)
pub const BPC: u32 = 8;

/// The depth `display_id`'s head sends, in bits per component, from the display
/// layer's record of the head
pub fn output_bpc(display_id: u32) -> u32 {
    crate::drivers::gpu::kms::heads::get(display_id)
        .and_then(|r| r.colour)
        .map_or(BPC, |c| c.depth.bits())
}

/// Read a monitor's EDID as this driver was told to.
///
/// `edidbase` on the boot line stops at the base block. The extensions are
/// where a fast panel keeps its fast timings, so this is the one-word way back
/// to 60 Hz, next to `nogpu`, `nointr` and `hwcursor`
pub fn parse_edid(bytes: &[u8]) -> Option<crate::drivers::gpu::edid::Edid> {
    let extensions = !crate::arch::x86_64::grub::cmdline_flag("edidbase");
    crate::drivers::gpu::edid::parse_with(bytes, extensions)
}

/// Whether this output could carry this mode at all, asked before anything is
/// programmed. The fastest of a list of timings is the most likely not to fit,
/// and finding out after the SOR is assigned and the link trained leaves the
/// monitor dark
pub fn mode_possible(
    or: &crate::drivers::gpu::nvidia::gsp_common::disp::OrInfo,
    dpcd: Option<&crate::drivers::gpu::nvidia::gsp_common::disp::DpcdCaps>,
    caps: &crate::drivers::gpu::edid::HdmiCaps,
    mode: &crate::drivers::gpu::nvidia::gsp_common::modeset::Mode,
    bpc: u32,
) -> bool {
    use crate::drivers::gpu::nvidia::gsp_common::modeset;
    if or.is_dp() {
        let Some(d) = dpcd else { return false };
        modeset::dp_sst_stream_params(
            mode, d.max_lane_count as u32, d.max_link_rate as u32, bpc, d.enhanced_frame_cap,
        )
        .is_some()
    } else if or.is_tmds() {
        // Deep colour runs the TMDS clock faster by the ratio of the depths
        // (HDMI 1.3 deep colour: 10 bits is 1.25 times the pixel clock)
        modeset::mode_fits_tmds(mode.clock_khz * bpc.max(8) / 8, caps)
    } else {
        false
    }
}

/// The tiles and physical windows `head` gets (`dev/display-nvkms.md`, decision
/// 3), from what every head holds in the armed state and what the caps say
/// exists. `None` without believable caps: the caller then uses the inherited
/// or like-numbered tile and physical windows 0 and 1, which this also returns
/// for a single head
pub fn plan_tiles(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    head: u32,
) -> Option<crate::drivers::gpu::nvidia::gsp_common::modeset::assign::HeadTiles> {
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    use crate::drivers::gpu::nvidia::gsp_common::modeset::assign::{self, HeadTiles};
    let caps = crate::drivers::gpu::nvidia::gsp_common::dispcaps::get()?;
    let held = |h: u32| HeadTiles {
        tiles: disp::armed_tile_mask(bar0, h),
        phywins: [disp::armed_window_physical(bar0, 2 * h), disp::armed_window_physical(bar0, 2 * h + 1)],
    };
    let others: alloc::vec::Vec<HeadTiles> = crate::drivers::gpu::nvidia::gsp_common::output::lit_heads()
        .into_iter()
        .filter(|&(h, _)| h != head)
        .map(|(h, _)| held(h))
        .collect();
    // One tile: every mode the panels here offer fits in one, and IMP's count
    // is only an opinion so far (decision 2)
    assign::assign(&caps, &others, held(head), 1, assign::required_tile_type(false))
}

/// IMP's opinion of every mode `display_id` offers on `head`, beside the link's
/// (`dev/display-nvkms.md`, decision 2).
///
/// Logged and counted, and nothing is refused on it: a parameter block RM reads
/// differently from how we wrote it would refuse good modes. Every other lit
/// head goes into the question too, with the tiles it holds, as NVKMS asks
/// about the whole display at once.
///
/// `Asked::BestSize` is the boot's question: the modes of the size being
/// driven, the refresh ladder. All modes are `nvidia disp imp`'s job.
///
/// `Asked::One` is a modeset's question, and asks for the minimum v-pstate as
/// NVKMS's modeset does (`imp.rs`); the others validate. Returns the answer
/// about the last mode asked
pub enum Asked<'a> {
    One(&'a crate::drivers::gpu::nvidia::gsp_common::modeset::Mode),
    BestSize,
    All,
}

pub fn imp_opinion(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    display_id: u32,
    head: u32,
    asked: Asked,
    say: &dyn Fn(&str),
) -> Option<crate::drivers::gpu::nvidia::gsp_common::modeset::imp::Imp> {
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    use crate::drivers::gpu::nvidia::gsp_common::modeset::imp::{self, ImpHead};
    use crate::drivers::gpu::nvidia::gsp_common::output;

    let Some((or, dpcd, caps, modes)) =
        output::with(display_id, |o| (o.or, o.dpcd, o.hdmi, o.modes.clone()))
    else {
        return None;
    };
    let others: alloc::vec::Vec<ImpHead> = output::with_all(|outs| {
        outs.iter()
            .filter(|o| o.display_id != display_id)
            .filter_map(|o| Some((o.head?, o.mode()?)))
            .filter(|&(h, _)| h != head)
            .map(|(h, mode)| ImpHead { head: h, mode, tiles: disp::armed_tile_mask(bar0, h) })
            .collect()
    });
    let modeset = matches!(asked, Asked::One(_));
    let asked: alloc::vec::Vec<_> = match asked {
        Asked::One(m) => alloc::vec![*m],
        Asked::All => modes,
        Asked::BestSize => {
            let size = modes.first().map(|m| (m.h_display, m.v_display));
            modes.into_iter().filter(|m| Some((m.h_display, m.v_display)) == size).collect()
        }
    };
    let mut last = None;
    for m in &asked {
        let link = mode_possible(&or, dpcd.as_ref(), &caps, m, output_bpc(display_id));
        let mut heads = others.clone();
        heads.push(ImpHead { head, mode: *m, tiles: 0 });
        let r = m.refresh_centihz();
        match imp::is_mode_possible(rpc, client, &heads, modeset) {
            Ok(answer) => {
                imp::tally(link, answer.possible);
                said(if link == answer.possible { Level::Info } else { Level::Warn }, &say, &alloc::format!(
                    "DISP: {}x{} @ {}.{:02} Hz: link {}, IMP {}{}",
                    m.h_display, m.v_display, r / 100, r % 100,
                    if link { "yes" } else { "no" }, answer.describe(head),
                    if link == answer.possible { "" } else { " - DISAGREE" }
                ));
                last = Some(answer);
            }
            Err(e) => {
                // One failure says the question itself is wrong; the next mode
                // would only repeat it
                said(Level::Warn, &say, &alloc::format!("DISP: IMP not answered: {:?} - no second opinion this time", e));
                return None;
            }
        }
    }
    last
}

/// What a modeset is going to do, decided before anything is written: NVKMS's
/// proposed state (`AssignProposedModeSetHwState`, `nvkms-modeset.c:1026`;
/// `dev/display-nvkms.md`, decision 4)
#[derive(Clone, Copy, Debug)]
pub struct Proposal {
    pub head: u32,
    /// The firmware lit this head: its link is trained and carrying a picture
    pub inherited: bool,
    pub mode: crate::drivers::gpu::nvidia::gsp_common::modeset::Mode,
    /// Bits per component the link is sized for
    pub bpc: u32,
    /// Tiles and physical windows, when the caps were read (`plan_tiles`)
    pub tiles: Option<crate::drivers::gpu::nvidia::gsp_common::modeset::assign::HeadTiles>,
}

/// Choose the head and the resources it composites through. Asks RM which heads
/// are active and reads the armed state; writes nothing, trains nothing
pub fn propose(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    display_id: u32,
    mode: &crate::drivers::gpu::nvidia::gsp_common::modeset::Mode,
    say: &dyn Fn(&str),
) -> Option<Proposal> {
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    use crate::drivers::gpu::nvidia::gsp_common::modeset;
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{status_str, RmError};

    let fail = |what: &str, e: RmError| -> alloc::string::String {
        match e {
            RmError::Status(s) => alloc::format!("DISP: {} failed: {}", what, status_str(s)),
            other => alloc::format!("DISP: {} failed: {:?}", what, other),
        }
    };

    // A UEFI boot has a head scanning out, the firmware console's, inherited by
    // GSP-RM. A different head into the same connector would put two heads on
    // one output resource, so take over the one that has it. Head 0 when
    // nothing is active
    let mut head = 0u32;
    let mut inherited = None;
    for h in 0..disp::MAX_HEADS {
        match modeset::get_active(rpc, client, h) {
            Ok(active) => {
                if active != 0 {
                    say(&alloc::format!("DISP: head {} is already driving {:#010x}", h, active));
                }
                if active == display_id && inherited.is_none() {
                    inherited = Some(h);
                    head = h;
                }
            }
            Err(e) => {
                said(Level::Warn, &say, &fail(&alloc::format!("GET_ACTIVE head {}", h), e));
                break;
            }
        }
    }
    // RM reports nothing active; ask the hardware, which is still scanning out
    // the console.
    //
    // Only while nothing of ours is lit: once this driver runs a monitor, the
    // head the hardware reports is ours, and handing it to a second output
    // takes the first one's picture away
    let ours = crate::drivers::gpu::nvidia::gsp_common::output::lit_heads();
    if inherited.is_none() && ours.is_empty() {
        if let Some((h, raster, clock)) = modeset::live_head_from_hw(bar0, disp::MAX_HEADS) {
            said(Level::Notice, &say, &alloc::format!(
                "DISP: RM reports no active head, but head {} is scanning {}x{} at {} kHz",
                h, raster & 0x7fff, raster >> 16, clock / 1000
            ));
            inherited = Some(h);
            head = h;
        }
    }
    // Nothing to inherit: take a head nobody has. An unconditional head 0 would
    // hand a second monitor the first one's head
    if inherited.is_none() {
        // The board's own head mask, and every head this class has if RM will
        // not say
        let mask = disp::head_mask(rpc, client).unwrap_or((1 << disp::MAX_HEADS) - 1);
        match crate::drivers::gpu::nvidia::gsp_common::output::free_head(mask) {
            Some(h) => head = h,
            None => {
                said(Level::Warn, &say, &alloc::format!(
                    "DISP: every head in mask {:#x} is taken - {:#010x} stays dark",
                    mask, display_id
                ));
                return None;
            }
        }
    }
    say(&alloc::format!(
        "DISP: using head {} ({})", head,
        if inherited.is_some() { "inherited from firmware" } else { "none was active" }
    ));

    let plan = plan_tiles(bar0, head);
    say(&match plan {
        Some(p) => alloc::format!(
            "DISP: head {} gets tiles {:#x}, physical windows {:#x}/{:#x}",
            head, p.tiles, p.phywins[0], p.phywins[1]
        ),
        None => alloc::format!("DISP: head {} - no caps, built-in tile and physical windows 0/1", head),
    });
    Some(Proposal { head, inherited: inherited.is_some(), mode: *mode, bpc: BPC, tiles: plan })
}

/// Whether the proposal can be carried: the link's answer, which decides, and
/// IMP's, logged beside it (`ValidateProposedModeSetHwState`,
/// `nvkms-modeset.c:2201`; decision 2)
pub fn check(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    display_id: u32,
    or: &crate::drivers::gpu::nvidia::gsp_common::disp::OrInfo,
    dpcd: Option<&crate::drivers::gpu::nvidia::gsp_common::disp::DpcdCaps>,
    p: &Proposal,
    say: &dyn Fn(&str),
) -> bool {
    let caps = crate::drivers::gpu::nvidia::gsp_common::output::with(display_id, |o| o.hdmi)
        .unwrap_or_default();
    let imp = imp_opinion(bar0, rpc, client, display_id, p.head, Asked::One(&p.mode), say);
    let link = mode_possible(or, dpcd, &caps, &p.mode, p.bpc);
    // The display's floor for `nvidia perf`, as this modeset's IMP saw it
    if link {
        crate::drivers::gpu::nvidia::gsp_common::modeset::imp::note_floor(
            p.head, imp.filter(|a| a.possible).and_then(|a| a.min_pstate));
    }
    if !link {
        said(Level::Warn, &say, &alloc::format!(
            "DISP: {}x{} at {} kHz does not fit this output - not programming it",
            p.mode.h_display, p.mode.v_display, p.mode.clock_khz
        ));
    }
    link
}

/// Program the proposal: the SOR, the link, the head, the windows' capacity,
/// the tiles, and the commit (`ApplyProposedModeSetHwState*` and
/// `KickoffProposedModeSetHwState`, `nvkms-modeset.c:2585-3256`). Returns the
/// head and SOR driving the output, `None` if the modeset did not land
#[allow(clippy::too_many_arguments)]
pub fn apply(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    push: &mut crate::drivers::gpu::nvidia::gsp_common::pushbuf::PushBuf,
    display_id: u32,
    or: &crate::drivers::gpu::nvidia::gsp_common::disp::OrInfo,
    dpcd: Option<&crate::drivers::gpu::nvidia::gsp_common::disp::DpcdCaps>,
    p: &Proposal,
    olut: Option<u64>,
    say: &dyn Fn(&str),
) -> Option<(u32, u32)> {
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    use crate::drivers::gpu::nvidia::gsp_common::modeset;
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{status_str, RmError};

    let fail = |what: &str, e: RmError| -> alloc::string::String {
        match e {
            RmError::Status(s) => alloc::format!("DISP: {} failed: {}", what, status_str(s)),
            other => alloc::format!("DISP: {} failed: {:?}", what, other),
        }
    };

    let head = p.head;
    let inherited = p.inherited.then_some(p.head);
    let mode = &p.mode;

    // Which SOR drives this connector is RM's decision and need not match
    // OR_GET_INFO, which describes the current wiring; this establishes it. Not
    // a SOR another lit output holds (`AssignSor`), or RM could hand this
    // connector the one driving a picture
    let exclude = {
        let lit_sors: alloc::vec::Vec<u32> = crate::drivers::gpu::nvidia::gsp_common::output::with_all(|outs| {
            outs.iter().filter(|o| o.display_id != display_id).filter_map(|o| o.sor).collect()
        });
        let own = crate::drivers::gpu::nvidia::gsp_common::output::with(display_id, |o| o.sor).flatten();
        modeset::assign::sor_exclude_mask(&lit_sors, own)
    };
    let sor = match modeset::assign_sor(rpc, client, display_id, exclude) {
        Ok(s) => {
            say(&alloc::format!(
                "DISP: SOR #{} assigned to {:#010x}{}",
                s, display_id,
                if s == or.index { "" } else { " (OR_GET_INFO had said otherwise)" }
            ));
            s
        }
        Err(e) => {
            said(Level::Error, &say, &fail("DFP_ASSIGN_SOR", e));
            return None;
        }
    };

    // The far side of the connector. Both branches answer two questions: which
    // SOR protocol the head will speak, and whether the link can carry the mode
    let protocol = if or.is_dp() {
        let Some(dpcd) = dpcd else {
            said(Level::Error, &say, "DISP: DisplayPort output with no DPCD - nothing to train against");
            return None;
        };

        // Train at everything the sink claims. RM falls back on its own if the
        // link will not hold, and reports what it settled on through the stream
        // configuration.
        //
        // Skipped when the head was inherited: a firmware-lit head has a
        // trained, locked link, retraining returns NV_ERR_GENERIC and would
        // take the picture away
        let mut lanes = dpcd.max_lane_count as u32;
        let mut bw = dpcd.max_link_rate as u32;
        if inherited.is_some() {
            say(&alloc::format!(
                "DISP: link inherited from firmware, not retraining ({} lanes x {})",
                lanes, dpcd.rate_name()
            ));
        } else {
            match modeset::dp_train_target(rpc, client, display_id, 0, lanes, bw, dpcd) {
                // What held may be less than the sink claims: the training
                // falls back (`EvoMainLink::train`), and the stream below is
                // sized for the link that is up
                Ok((got_lanes, got_bw)) => {
                    let full = (got_lanes, got_bw) == (lanes, bw);
                    said(if full { Level::Info } else { Level::Notice }, &say, &alloc::format!(
                        "DISP: link trained: {} lanes x {:#x}{}",
                        got_lanes, got_bw,
                        if (got_lanes, got_bw) == (lanes, bw) {
                            alloc::format!(" ({} Mbps, all the sink offers)", dpcd.link_mbps())
                        } else {
                            alloc::format!(" - fell back from {} lanes x {}", lanes, dpcd.rate_name())
                        }
                    ));
                    lanes = got_lanes;
                    bw = got_bw;
                }
                Err(e) => {
                    said(Level::Error, &say, &fail("DP_CTRL link training", e));
                    return None;
                }
            }
        }

        // Training brings the link to where the sink can lock to it; it does
        // not turn on the main link carrying the picture
        if inherited.is_none() {
            match modeset::dp_main_link_power(rpc, client, display_id, true) {
                Ok(()) => say("DISP: DP main link powered up"),
                Err(e) => said(Level::Error, &say, &fail("DP_MAIN_LINK_CTRL powerup", e)),
            }
        }

        // How the head's pixels are packed into the link. "Impossible" from the
        // formula beats a link that comes up and never locks
        let Some(sp) = modeset::dp_sst_stream_params(mode, lanes, bw, p.bpc, dpcd.enhanced_frame_cap)
        else {
            said(Level::Warn, &say, "DISP: mode does not fit the trained link, refusing to program it");
            return None;
        };
        say(&alloc::format!(
            "DISP: stream watermark {}, hblank {} sym, vblank {} sym",
            sp.watermark, sp.hblank_sym, sp.vblank_sym
        ));
        // The DP stream framer is RM's to program: `dp_config_stream_sst` and
        // `sf_program_stream` both fill in a vBlankSym ten times too small, so
        // neither is taken. The reasoning is in `dev/gb206-display.md`
        let _ = (modeset::dp_config_stream_sst, modeset::sf_program_stream, &sp);

        // Protocol DP-B means the second sublink; DP-A is the first
        if or.protocol == 9 { modeset::SOR_PROTOCOL_DP_B } else { modeset::SOR_PROTOCOL_DP_A }
    } else if or.is_tmds() {
        // TMDS negotiates nothing: no training, no symbol budget, no stream
        // configuration. The link runs at the pixel clock and the sink locks to
        // it or does not. One question for RM and one thing to tell it.
        //
        // Asked: what this connector is. `OR_GET_INFO` describes the wiring;
        // `DFP_GET_INFO` describes what the board will do with it, and
        // HDMI-capable and HDMI-allowed are separate bits, since a board can be
        // wired for HDMI with it disabled in the VBIOS
        let info = match modeset::dfp_get_info(rpc, client, display_id) {
            Ok(i) => {
                say(&alloc::format!(
                    "DISP: dfp signal {} lanes {}{}{}{}",
                    i.signal_name(), i.lane(),
                    if i.hdmi_capable() { ", HDMI capable" } else { "" },
                    if i.hdmi_allowed() { ", HDMI allowed" } else { "" },
                    // Two answers to one question, so a disagreement gets a
                    // line: `OR_GET_INFO` describes the wiring, `DFP_GET_INFO`
                    // the panel, and this branch was chosen on the first
                    if i.is_tmds() { "" } else { " (but OR_GET_INFO said TMDS)" },
                ));
                i
            }
            Err(e) => {
                // Not fatal: a DVI modeset needs none of this, so the monitor
                // lights as plain TMDS
                said(Level::Warn, &say, &fail("DFP_GET_INFO", e));
                Default::default()
            }
        };

        // What the monitor said, from its CEA extension. Empty on a sink with
        // no extension, which is not "is DVI"; hence the board's answer below
        // too
        let caps = crate::drivers::gpu::nvidia::gsp_common::output::with(display_id, |o| o.hdmi)
            .unwrap_or_default();

        // Told: whether the sink is an HDMI receiver. RM does not read the
        // EDID, and the header asks for this before every modeset on an
        // HDMI-capable output.
        //
        // Both halves: the board's, whether this connector can speak HDMI, and
        // the monitor's, whether the far end is an HDMI receiver and not DVI
        // behind an adapter, which data islands would confuse
        let hdmi = info.hdmi_capable() && info.hdmi_allowed() && caps.hdmi;
        match modeset::set_hdmi_enable(rpc, client, display_id, hdmi) {
            Ok(()) => say(&alloc::format!(
                "DISP: HDMI components {}", if hdmi { "enabled" } else { "left off (DVI)" }
            )),
            Err(e) => said(Level::Error, &say, &fail("SET_HDMI_ENABLE", e)),
        }

        // Above 340 MHz the link is scrambled, and RM programs that only when
        // told the sink can descramble. Sent before the ceiling check, which
        // then asks about a link RM has been configured for
        if hdmi && (caps.scdc || caps.scramble_below_340) {
            match modeset::set_hdmi_sink_caps(rpc, client, display_id, &caps) {
                Ok(bits) => say(&alloc::format!("DISP: sink caps {:#x} sent to RM", bits)),
                Err(e) => said(Level::Warn, &say, &fail("SET_HDMI_SINK_CAPS", e)),
            }
        }

        // And the ceiling, the monitor's own when it named one
        if !modeset::mode_fits_tmds(mode.clock_khz, &caps) {
            said(Level::Warn, &say, &alloc::format!(
                "DISP: {} kHz is past what this sink takes (max {} kHz, SCDC {}) - refusing",
                mode.clock_khz,
                if caps.max_tmds_khz != 0 { caps.max_tmds_khz } else { modeset::TMDS_SCRAMBLING_THRESHOLD_KHZ },
                caps.scdc
            ));
            return None;
        }
        let p = modeset::tmds_protocol(or.protocol, mode.clock_khz);
        say(&alloc::format!(
            "DISP: TMDS at {} kHz, SOR protocol {}", mode.clock_khz,
            if p == modeset::SOR_PROTOCOL_DUAL_TMDS { "dual" }
            else if p == modeset::SOR_PROTOCOL_SINGLE_TMDS_B { "single B" }
            else { "single A" }
        ));
        p
    } else {
        said(Level::Warn, &say, &alloc::format!(
            "DISP: {} #{} speaks {} - this driver drives DisplayPort and TMDS",
            or.type_name(), or.index, or.protocol_name()
        ));
        return None;
    };

    // Diagnostic: leave a firmware-lit head as it is.
    //
    // Every method programmed here matches the shipping driver's armed state,
    // yet a head from cold composites nothing, while a firmware-lit head scans
    // out through the same SOR. So the difference is in what initialises the
    // display engine before any method is written.
    //
    // An inherited head is left unprogrammed: keep its raster, viewport, LUTs
    // and output, stage only the core-channel plumbing the window needs
    // (ownership, usage bounds, tiles), then attach the window. A cold head is
    // programmed as usual.
    //
    // `coldhead` on the GRUB line turns this off: the inherited head is
    // programmed from cold, at the mode chosen for it (`dev/display-nvkms.md`,
    // decision 5). The link stays the firmware's, as RM will not retrain a live
    // one, so the mode has to fit the link RM says is up; if not, the head is
    // kept and the log says why
    const DIAG_KEEP_FIRMWARE_HEAD: bool = true;
    let cold = inherited.is_some() && crate::arch::x86_64::grub::cmdline_flag("coldhead") && {
        if or.is_dp() {
            match (modeset::dp_get_link_config(rpc, client, display_id), dpcd) {
                (Ok((lanes, bw)), Some(d)) => {
                    let fits = modeset::dp_sst_stream_params(mode, lanes, bw, p.bpc, d.enhanced_frame_cap).is_some();
                    say(&alloc::format!(
                        "DISP: coldhead - the firmware's link is {} lanes x {:#x}; {}x{} at {} kHz {}",
                        lanes, bw, mode.h_display, mode.v_display, mode.clock_khz,
                        if fits { "fits, programming the head from cold" } else { "does not fit it, keeping the firmware's head" }
                    ));
                    fits
                }
                (Err(e), _) => {
                    said(Level::Warn, &say, &fail("coldhead: DP_GET_LINK_CONFIG", e));
                    false
                }
                (Ok(_), None) => false,
            }
        } else {
            say("DISP: coldhead - programming the inherited head from cold");
            true
        }
    };
    let keep = DIAG_KEEP_FIRMWARE_HEAD && inherited.is_some() && !cold;

    // Now the near side. The SOR binding and the head's raster land together on
    // the UPDATE, so the hardware never sees a head driving an unconfigured
    // output resource
    if !keep {
        if let Err(e) = modeset::sor_set_control(push, sor, head, protocol) {
            said(Level::Error, &say, &alloc::format!("DISP: SOR_SET_CONTROL push failed: {:?}", e));
            return None;
        }
    }
    // Sampled before the head stream overwrites it: the inherited value is the
    // evidence
    DIAG.lock().cursor_comp = Some(modeset::armed_cursor_composition(bar0, head));
    DIAG.lock().ocsc_was = Some(modeset::armed_ocsc0_diagonal(bar0, head));
    DIAG.lock().gates_was = Some(modeset::armed_output_gates(bar0, head));
    if keep {
        // Seed the assembly half from the armed half before staging anything.
        // The core channel comes up with junk in assembly (RASTER_SIZE 0x50008
        // staged against 0x4650898 armed), and an update arms assembly
        // wholesale, replacing a working firmware head.
        // `R update ctl NOT_BEGUN` says the hardware would not start it
        match crate::drivers::gpu::nvidia::gsp_common::refstate::adopt_armed_state(
            push, bar0, head,
        ) {
            Ok(()) => {
                // Kick this on its own and read the assembly half back before
                // staging anything else. No UPDATE goes with it, so nothing
                // arms: the methods move into assembly, and either land or do
                // not. This settles GET catching PUT while assembly reads stale
                push.kick(bar0);
                let waited = push.wait_idle(bar0, 2_000_000_000).is_ok();
                let a = |m: u32| bar0.read32(0x0068_0000 + m);
                say(&alloc::format!(
                    "DISP: DIAG - adopted head into staging (idle {}), assy raster {:#x} clk {:#x} sor {:#x}",
                    waited, a(0x2064), a(0x200c), a(0x0300 + sor * 0x20)
                ));
            }
            Err(e) => {
                said(Level::Error, &say, &alloc::format!("DISP: adopt push failed: {:?}", e));
                return None;
            }
        }
    } else {
        // The scaler coefficient RAM for this head, before its configuration.
        // Only this head: all four is 1536 dwords, past what the channel's PUT
        // can address
        if let Err(e) = modeset::init_output_scaler_coeffs_head(push, head) {
            said(Level::Error, &say, &alloc::format!("DISP: scaler coeff push failed: {:?}", e));
            return None;
        }
        if let Err(e) = modeset::head_set_mode(
            push, head, mode, display_id, olut,
            // The boot's head has never been asked for anything, so the
            // default: identity ramp, full range, no dither
            &modeset::HeadColour::DEFAULT,
        ) {
            said(Level::Error, &say, &alloc::format!("DISP: head method push failed: {:?}", e));
            return None;
        }
    }
    // Tile state is not touched here. `core_channel_init` set TILE_MASK and
    // TILE_SIZE as `coreca7d_init` does, and the real driver never revisits
    // them during a modeset.
    //
    // Window ownership goes in this batch and commits with the head, matching
    // `nv50_disp_atomic_commit_tail`: it runs `nv50_head_flush_set` first, then
    // `assign_windows` appends the owner methods and issues one core update for
    // both. Its warning that the assignment must be "in an update that's not
    // interlocked with any window channels" means the window interlock mask is
    // empty here, not that the head is excluded. Split, the ownership update
    // arms a head with no configuration
    if let Err(e) = modeset::core_window_owner(push, disp::CORE_WINDOWS) {
        said(Level::Error, &say, &alloc::format!("DISP: window owner push failed: {:?}", e));
        return None;
    }
    // Capacity, now that there is a head and a width to size it against.
    // `core_channel_init` left every window at flags-only
    let plan = p.tiles;
    if let Err(e) = disp::core_window_usage(
        push, head, mode.h_display, plan.map_or([1, 2], |p| p.phywins),
    ) {
        said(Level::Error, &say, &alloc::format!("DISP: window usage push failed: {:?}", e));
        return None;
    }
    // Take this head's tile and give up everyone else's before anything is
    // committed. NVKMS warns that leaving the power-on assignments ends in a
    // display engine hang.
    //
    // A monitor already scanning keeps its tile and windows. One core channel
    // and an UPDATE arming every head from assembly at once: this batch has to
    // be true about all of them
    let keep = crate::drivers::gpu::nvidia::gsp_common::output::lit_heads();
    match disp::core_claim_tiles(push, bar0, head, mode.h_display, &keep, plan.map(|p| p.tiles)) {
        Ok(tile) => DIAG.lock().tile_claim = Some(tile),
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("DISP: tile claim push failed: {:?}", e));
            return None;
        }
    }
    if let Err(e) = disp::core_update(push, 0, 0) {
        said(Level::Error, &say, &alloc::format!("DISP: modeset update push failed: {:?}", e));
        return None;
    }
    push.kick(bar0);
    match push.wait_idle(bar0, 2_000_000_000) {
        Ok(()) => {
            let r = mode.refresh_centihz();
            say(&alloc::format!(
                "DISP: head {} driving SOR #{} at {}x{} @ {}.{:02} Hz - modeset committed",
                head, sor, mode.h_display, mode.v_display, r / 100, r % 100
            ));
            // The DP stream framer is RM's to program. A read only, reported
            // once in the closing block, to show what RM put there
            let _ = modeset::sf_stream_state(bar0, head);
            // NVKMS asks GET_ACTIVE only once, before any modeset
            // (`GetVbiosHeadAssignment`, `nvkms-rm.c:1213`); the link and the
            // pattern are asked on demand (`rm_link_view`,
            // `nvidia gb206-diff`). The boot's warning
            // (`nocat ASSERT gsp-rm+0x12c2214 line 70`) comes from GSP-RM after
            // this UPDATE, in its own handling of the modeset, and is read
            // during whatever RPC comes next
            Some((head, sor))
        }
        Err(_) => {
            said(Level::Error, &say, &alloc::format!(
                "DISP: modeset stream not consumed (get {} put {})",
                push.get(bar0), push.put_index()
            ));
            None
        }
    }}

/// Light one output: propose a head and its resources, check the mode fits,
/// apply it. Returns the head and SOR driving the output (the window binds to
/// the head, and transmitter diagnostics index by the SOR), or `None` if the
/// modeset did not land
#[allow(clippy::too_many_arguments)]
pub fn drive_output(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    push: &mut crate::drivers::gpu::nvidia::gsp_common::pushbuf::PushBuf,
    display_id: u32,
    or: &crate::drivers::gpu::nvidia::gsp_common::disp::OrInfo,
    dpcd: Option<&crate::drivers::gpu::nvidia::gsp_common::disp::DpcdCaps>,
    mode: &crate::drivers::gpu::nvidia::gsp_common::modeset::Mode,
    olut: Option<u64>,
    say: &dyn Fn(&str),
) -> Option<(u32, u32)> {
    let p = propose(bar0, rpc, client, display_id, mode, say)?;
    if !check(bar0, rpc, client, display_id, or, dpcd, &p, say) {
        return None;
    }
    apply(bar0, rpc, client, push, display_id, or, dpcd, &p, olut, say)
}
