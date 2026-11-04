// Asking GSP-RM what the board has: display enumeration and the bring-up it
// drives. `modeset.rs` does the head/SOR/DP work it calls into.



use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use crate::mm::dma::DmaBuffer;

use super::chanmem::DISP_ROOT as CHAN_DISP_ROOT;
use super::gspboot::{DisplayMemory};

/// What this file's lines are signed with when the engine loader hands it the
/// boot's `say` (`nvidia::speaking`, `dev/klog.md`)
pub(super) static SITE: crate::kcore::klog::Site = crate::klog_site!();
use super::modeset::{DIAG, drive_output, settle_and_report, underflow_prop};
use super::window::{attach_window, live_scanout};
use crate::drivers::gpu::nvidia::gsp_common::output;

/// Every lit head dark, before the display objects are freed
/// (`dev/display-nvkms.md`, decision 7). NVKMS never frees the core channel
/// under a lit head: `nvShutDownApiHeads` first, a modeset with no displays,
/// whose `ApplyProposedModeSetHwStateOneHeadShutDown` detaches the connector,
/// gives the tiles and physical windows back and flips the layers to nothing;
/// then DPLib takes the link down. Freed lit, GSP-RM asserts, and the next
/// GSP-RM inherits a display engine mid-scan: a four-second core channel
/// allocation, Xid 56, a robust-channel report.
///
/// Window first, then the head, as two plain updates: on the way down "nothing
/// to scan, then no head" is order enough (`window.rs`)
pub fn shut_down_heads(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gsp_common::{disp, modeset, pushbuf::{self, PushBuf}};

    let lit: alloc::vec::Vec<(u32, u32, u32, bool, Option<disp::DpcdCaps>, bool)> =
        output::with_all(|outs| {
            outs.iter()
                .filter_map(|o| Some((o.display_id, o.head?, o.sor?, o.or.is_dp(), o.dpcd, o.connected)))
                .collect()
        });
    if lit.is_empty() {
        return;
    }
    let mut mem = super::gspboot::DISP_MEM.lock();
    let Some(mem) = mem.as_mut() else {
        said(Level::Warn, &say, "DISP: no display pushbuffers - the heads are left as they are");
        return;
    };

    // The windows: no surface, nothing held back by an acquire semaphore
    for &(_, head, _, _, _, _) in &lit {
        let window = head * 2;
        let user = pushbuf::wndw_chan_user(window);
        let Some(Some(buf)) = mem.wndw_push.get_mut(head as usize) else { continue };
        let cur = pushbuf::channel_get(bar0, user);
        let mut push = PushBuf::resume(buf.as_mut_slice(), user, cur);
        let staged = modeset::window_clear_acq_semaphore(&mut push).is_ok()
            && modeset::window_disable(&mut push).is_ok()
            && modeset::window_update(&mut push, false, 0).is_ok();
        if !staged {
            said(Level::Warn, &say, &alloc::format!("DISP: window {} would not take its shutdown", window));
            continue;
        }
        push.kick(bar0);
        if push.wait_idle(bar0, 1_000_000_000).is_err() {
            said(Level::Warn, &say, &alloc::format!("DISP: window {} did not consume its shutdown", window));
        }
    }

    // The heads: cursor off, SOR detached, display id 0, tiles and physical
    // windows back, one UPDATE for all
    let Some(pb) = mem.core_push.as_mut() else {
        said(Level::Warn, &say, "DISP: no core pushbuffer - the heads are left lit");
        return;
    };
    let cur = pushbuf::channel_get(bar0, pushbuf::CORE_CHAN_USER);
    let mut push = PushBuf::resume(pb.as_mut_slice(), pushbuf::CORE_CHAN_USER, cur);
    let mut staged = true;
    for &(_, head, sor, _, _, _) in &lit {
        staged = staged
            && modeset::head_set_cursor(&mut push, head, None).is_ok()
            && modeset::sor_detach(&mut push, sor).is_ok()
            && push.mthd1(modeset::M_HEAD_SET_DISPLAY_ID + head * modeset::HEAD_STRIDE, 0).is_ok()
            && disp::head_release_tiles(&mut push, head, &[head * 2, head * 2 + 1]).is_ok();
    }
    if staged && disp::core_update(&mut push, 0, 0).is_ok() {
        push.kick(bar0);
        if push.wait_idle(bar0, 2_000_000_000).is_err() {
            said(Level::Warn, &say, "DISP: the core channel did not consume the head shutdown");
        }
    } else {
        said(Level::Warn, &say, "DISP: the core channel would not take the head shutdown");
    }
    drop(push);

    // The links down, as DPLib does after the shutdown modeset: main link off,
    // the sink to D3
    for &(id, _, _, is_dp, dpcd, connected) in &lit {
        if !is_dp {
            continue;
        }
        let _ = modeset::dp_main_link_power(rpc, client, id, false);
        if let (true, Some(d)) = (connected, dpcd) {
            let _ = modeset::dp_set_sink_power(rpc, client, id, d.rev, false);
        }
    }
    say(&alloc::format!(
        "DISP: {} head(s) shut down before the display goes - windows empty, SORs detached, tiles back, links down",
        lit.len()
    ));
}

/// Ask GSP-RM what outputs the board has, which have a monitor attached, and
/// what those monitors are, then bring up the core channel and drive the best
/// connected output, DisplayPort or TMDS, at its native mode.
///
/// Order is forced by dependency: outputs before EDIDs, EDIDs before a mode,
/// instance memory and a pushbuffer before any method stream, and all of it
/// before `drive_output`, which needs the display id, output resource, DPCD and
/// mode together.
///
/// GSP-RM's display init takes scanout away from the GOP framebuffer, so the
/// log is printed as produced.
///
/// Every failure is reported and skipped: the boot has already succeeded
pub fn enumerate_displays(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    client: &crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient,
    mem: &mut DisplayMemory,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    use crate::drivers::gpu::nvidia::gsp_common::modeset;
    use crate::drivers::gpu::nvidia::gsp_common::vram::{self, Placement};
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{status_str, RmError};

    let fail = |what: &str, e: RmError| -> alloc::string::String {
        match e {
            RmError::Status(s) => alloc::format!("DISP: {} failed: {}", what, status_str(s)),
            other => alloc::format!("DISP: {} failed: {:?}", what, other),
        }
    };

    // nouveau's r535_disp_oneinit order: instance memory first, then the
    // display object.
    //
    // The instance memory goes through GSP-RM's internal client, whose handles
    // RM published as it came up (`bringup::rm::static_info`). The channel
    // setup further down needs them too
    let internal_client = super::chanmem::internal_client();
    // Whether the CPU can write video memory at all. Everything wanting some
    // asks the allocator; this says whether asking is worth anything
    let mut vram_ok = false;
    // Ask for a block instead of naming one.
    //
    // `Low` on all but the display's instance memory: the top of video memory
    // is GSP-RM's write-protected region and its heap, and a scanout surface
    // there reads back black without saying why
    let take = |size: u64, align: u64, place: Placement, owner: &'static str| -> u64 {
        match vram::alloc(size, align, place, owner) {
            Some(a) => a,
            None => {
                say(&alloc::format!(
                    "VRAM: no room for {} ({} KiB)", owner, size >> 10
                ));
                0
            }
        }
    };
    match internal_client {
        None => said(Level::Error, &say, "DISP: no internal handles - GSP_INIT did not answer"),
        Some(internal) => {
            // The one block from the top
            match vram::alloc(
                disp::INST_MEM_SIZE, disp::INST_MEM_ALIGN,
                Placement::High, "display instance memory",
            ) {
                Some(phys) => {
                    match disp::write_inst_mem(rpc, &internal, phys, disp::INST_MEM_SIZE) {
                        Ok(()) => say(&alloc::format!(
                            "DISP: inst mem {} KiB at vram {:#x} handed to GSP-RM",
                            disp::INST_MEM_SIZE >> 10, phys
                        )),
                        Err(e) => said(Level::Error, &say, &fail("WRITE_INST_MEM", e)),
                    }

                    // Can the CPU write video memory at all? Before GSP-RM
                    // boots the memory controller is not initialised and every
                    // read comes back zero, so this is asked. Everything below
                    // depends on the answer.
                    //
                    // One block, borrowed for the question and handed back. The
                    // surface is allocated in `attach_window`, at the size the
                    // mode needs. `gmmu` builds its page tables in system
                    // memory
                    let addr = take(0x10000, 0x10000, Placement::Low, "vram probe");
                    if addr != 0 {
                        let probe_ok = crate::drivers::gpu::nvidia::pramin::write_probe(bar0, addr);
                        // Which base register this chip honoured, reported
                        // regardless of probe outcome
                        say(&alloc::format!(
                            "R pramin win {:#x} probe {}",
                            crate::drivers::gpu::nvidia::pramin::window_reg_in_use(),
                            if probe_ok { "ok" } else { "FAILED" }
                        ));
                        if probe_ok {
                            say(&alloc::format!(
                                "DISP: PRAMIN reaches vram {:#x} - scanout can live there", addr
                            ));
                            vram_ok = true;
                        } else {
                            said(Level::Warn, &say, "DISP: PRAMIN still cannot write vram; scanout stays in sysmem");
                        }
                        vram::free(addr);
                    }
                }
                None => say("DISP: no usable vram region big enough for inst mem"),
            }
        }
    }

    if let Err(e) = disp::alloc_display(rpc, client) {
        said(Level::Error, &say, &fail("NV04_DISPLAY_COMMON alloc", e));
        return;
    }
    say("DISP: NV04_DISPLAY_COMMON allocated");

    // Subscribed before anything is enumerated: a monitor plugged in during the
    // bring-up should leave a notification in the queue
    super::control::subscribe_display_events(rpc, client, say);

    // No manual control of the DisplayPort watermark. Our `isModePossibleSST`
    // port gets hBlankSym right (126 against the driver's 128) but computes
    // vBlankSym off by the byte-rate/bit-rate factor, roughly 10x too small. A
    // register dump from the shipping driver on this card at 240 Hz, pclk
    // 561.35 MHz, shows vblank_sym 18278, which scales to 69091 at our 148.5
    // MHz; RM's automatic value there is 69275, within 0.3%. `DP_CONFIG_STREAM`
    // with an override also returns ILLEGAL_ACTION, so the call below stays
    // unused
    let _ = modeset::dp_set_manual;

    match (disp::num_heads(rpc, client), disp::head_mask(rpc, client)) {
        (Ok(n), Ok(m)) => say(&alloc::format!("DISP: {} heads, mask {:#x}", n, m)),
        (Ok(n), Err(e)) => {
            say(&alloc::format!("DISP: {} heads", n));
            said(Level::Error, &say, &fail("GET_ALL_HEAD_MASK", e));
        }
        (Err(e), _) => said(Level::Error, &say, &fail("GET_NUM_HEADS", e)),
    }

    let supported = match disp::supported(rpc, client) {
        Ok((mask, ddc)) => {
            say(&alloc::format!("DISP: outputs {:#010x} (ddc {:#010x})", mask, ddc));
            mask
        }
        Err(e) => {
            said(Level::Error, &say, &fail("GET_SUPPORTED", e));
            return;
        }
    };
    if supported == 0 {
        say("DISP: board reports no outputs");
        return;
    }

    let connected = match disp::connect_state(rpc, client, supported) {
        Ok(mask) => {
            say(&alloc::format!("DISP: connected {:#010x}", mask));
            mask
        }
        Err(e) => {
            said(Level::Error, &say, &fail("GET_CONNECT_STATE", e));
            return;
        }
    };
    if connected == 0 {
        say("DISP: nothing plugged in (or hotplug detect said so)");
        return;
    }

    // Every output the board has goes in the table, connected or not, so a
    // hot-plug event has an entry to fill in. Only connected ones are asked
    // about themselves: DPCD and EDID go over a wire with a monitor on the far
    // end.
    //
    // Display ids are single bits, so the mask is walked: an EDID belongs to
    // one monitor
    output::reset();
    for bit in 0..32u32 {
        let id = 1u32 << bit;
        if supported & id == 0 {
            continue;
        }
        // What the output physically is, before its EDID. DP or TMDS decides
        // the shape of the modeset that follows
        let or = match disp::or_info(rpc, client, id) {
            Ok(or) => {
                // An unassigned SOR is a dash, as in `nvidia disp`: RM binds
                // one at modeset time
                let index = match or.or_assigned() {
                    Some(i) => alloc::format!("#{}", i),
                    None => alloc::string::String::from("-"),
                };
                say(&alloc::format!(
                    "DISP: {:#010x} -> {} {} {}{}",
                    id, or.type_name(), index, or.protocol_name(),
                    if or.is_dp() { " (needs link training)" } else { "" }
                ));
                or
            }
            Err(e) => {
                said(Level::Warn, &say, &fail(&alloc::format!("OR_GET_INFO {:#010x}", id), e));
                continue;
            }
        };
        let mut out = output::Output::new(id, or);
        // Which socket it is, for its name (`nvidia/kms.rs`). Every id, plugged
        // in or not: a connector's name does not change with what is on it
        out.connector = match disp::connector_data(rpc, client, id) {
            Ok(c) => Some(c),
            Err(e) => {
                said(Level::Warn, &say, &fail(&alloc::format!("GET_CONNECTOR_DATA {:#010x}", id), e));
                None
            }
        };
        out.connected = connected & id != 0;
        if !out.connected {
            output::add(out);
            continue;
        }

        // On a DP link, ask the monitor what it can do, over the AUX sideband.
        // A sane answer proves a second, independent path to the panel, one
        // link training will use
        if or.is_dp() {
            match disp::read_dpcd_caps(rpc, client, id) {
                Ok(d) => match d {
                    Some((c, extended)) => {
                        say(&alloc::format!(
                            "DISP:   dpcd rev {}.{}, {} lanes x {} = {} Mbps{}{}",
                            c.rev >> 4, c.rev & 0xf, c.max_lane_count,
                            c.rate_name(), c.link_mbps(),
                            if c.enhanced_frame_cap { ", enhanced framing" } else { "" },
                            if extended { " (extended caps at 0x2200)" } else { "" }
                        ));
                        if c.msa_timing_par_ignored {
                            say("DISP:   sink ignores MSA timing - Adaptive-Sync capable");
                        }
                        out.dpcd = Some(c);
                    }
                    None => say("DISP:   dpcd read too short to parse"),
                },
                Err(e) => said(Level::Warn, &say, &fail("DP AUXCH_CTRL", e)),
            }
        }

        match disp::read_edid(rpc, client, id) {
            // One parse: the name, the timings and what the sink says about
            // HDMI are three answers out of one block
            Ok(edid) => match super::modeset::parse_edid(&edid) {
                Some(e) => {
                    let native = e.preferred();
                    say(&alloc::format!(
                        "DISP: {:#010x} = {}{:04x} \"{}\", native {}x{} ({} B edid, {} ext)",
                        id, e.manufacturer_str(), e.product, e.name_str(),
                        native.map(|t| t.h_display).unwrap_or(0),
                        native.map(|t| t.v_display).unwrap_or(0),
                        edid.len(), e.n_cea
                    ));
                    // Raw header bytes too, so the PnP id can be re-derived by
                    // hand. A three-letter id absent from any public list is
                    // normal, and only the raw bytes tell that from a broken
                    // unpack
                    let mut hex = alloc::string::String::new();
                    for b in edid.iter().take(16) {
                        hex.push_str(&alloc::format!("{:02x} ", b));
                    }
                    say(&alloc::format!("DISP:   edid[0..16] {}", hex.trim_end()));
                    // What the monitor says it can do over HDMI, when it
                    // shipped a CEA extension. On a DisplayPort panel this is
                    // all zero
                    if e.hdmi.hdmi || e.hdmi.max_tmds_khz != 0 {
                        say(&alloc::format!(
                            "DISP:   hdmi sink, max TMDS {} kHz{}{}",
                            e.hdmi.max_tmds_khz,
                            if e.hdmi.scdc { ", SCDC" } else { "" },
                            if e.hdmi.scramble_below_340 { ", scrambles below 340 MHz" } else { "" },
                        ));
                    }
                    out.name = alloc::string::String::from(e.name_str());
                    out.hdmi = e.hdmi;
                    out.vrr_range = modeset::vrr::edid_range(e.v_min_hz, e.v_max_hz);
                    out.modes = modeset::Mode::list_from(&e);
                    out.edid = edid;
                }
                None => say(&alloc::format!(
                    "DISP: {:#010x} returned {} B but no valid EDID header", id, edid.len()
                )),
            },
            Err(e) => said(Level::Warn, &say, &fail(&alloc::format!("GET_EDID {:#010x}", id), e)),
        }

        // The modes in full, not just their sizes: the modeset needs porches
        // and polarities, and printing them surfaces a bad unpack before the
        // hardware. Best first, and the best is what the output will be driven
        // at.
        //
        // Only the first size's refresh ladder: a monitor also names every VESA
        // and CTA mode it takes, twenty lines on a serial port costing
        // milliseconds a line. `nvidia disp` lists them all, and `trace` puts
        // them back here
        let best_size = out.modes.first().map(|m| (m.h_display, m.v_display));
        let all = crate::arch::x86_64::serial::tracing();
        let others = out.modes.iter()
            .filter(|m| Some((m.h_display, m.v_display)) != best_size)
            .count();
        for (n, m) in out.modes.iter().enumerate() {
            let r = m.refresh_centihz();
            let line = alloc::format!(
                "DISP:   mode {} {}x{}{} @ {}.{:02} Hz, {} kHz, h {}/{}/{} v {}/{}/{} {}{}",
                n, m.h_display, m.v_display,
                if m.interlace { "i" } else { "" },
                r / 100, r % 100, m.clock_khz,
                m.h_sync_start - m.h_display,
                m.h_sync_end - m.h_sync_start,
                m.h_total - m.h_sync_end,
                m.v_sync_start - m.v_display,
                m.v_sync_end - m.v_sync_start,
                m.v_total - m.v_sync_end,
                if m.nhsync { "-hsync" } else { "+hsync" },
                if m.nvsync { " -vsync" } else { " +vsync" },
            );
            // The rest goes to the kernel log only: `nvidia log disp debug` has
            // every mode
            if all || Some((m.h_display, m.v_display)) == best_size {
                say(&line);
            } else {
                crate::kdebug!("{}", line);
            }
        }
        if !all && others > 0 {
            say(&alloc::format!("DISP:   and {} smaller mode(s) - 'display' lists them", others));
        }
        if out.modes.is_empty() {
            say("DISP:   no usable timing in the EDID");
        }
        output::add(out);
    }
    // The connectors by their Linux names, what a `monitor =` line says and
    // what `display` takes
    for line in crate::drivers::gpu::nvidia::kms::connector_lines() {
        say(&alloc::format!("DISP: connector {}", line));
    }

    // The best monitor, not the first to answer: with two plugged in, the one
    // that shows more comes up.
    //
    // DisplayPort or TMDS, the two the modeset knows. A DisplayPort output with
    // no DPCD is excluded here: there is nothing to train the link against
    let target = output::with_all(|outs| {
        outs.iter()
            .filter(|o| o.connected && !o.modes.is_empty())
            .filter(|o| if o.or.is_dp() { o.dpcd.is_some() } else { o.or.is_tmds() })
            .max_by_key(|o| o.rank())
            .map(|o| (o.display_id, o.or, o.dpcd, o.hdmi))
    })
    // And which of its modes. `modes[0]` is the fastest, the most likely not to
    // fit the link, so walk down until one does and log what was skipped
    .and_then(|(id, or, dpcd, caps)| {
        let modes = output::with(id, |o| o.modes.clone()).unwrap_or_default();
        for m in modes.iter() {
            if super::modeset::mode_possible(&or, dpcd.as_ref(), &caps, m, super::modeset::BPC) {
                let r = m.refresh_centihz();
                say(&alloc::format!(
                    "DISP: driving {:#010x} at {}x{} @ {}.{:02} Hz",
                    id, m.h_display, m.v_display, r / 100, r % 100
                ));
                return Some((id, or, dpcd, *m));
            }
            let r = m.refresh_centihz();
            say(&alloc::format!(
                "DISP: {}x{} @ {}.{:02} Hz ({} kHz) does not fit this link - trying the next",
                m.h_display, m.v_display, r / 100, r % 100, m.clock_khz
            ));
        }
        say(&alloc::format!("DISP: {:#010x} offers nothing this link can carry", id));
        None
    });

    // With the outputs known, bring up the machinery a modeset is pushed
    // through, then hand it all to the modeset
    let Some(internal) = internal_client else {
        say("DISP: no internal handles, skipping channel setup");
        return;
    };
    let h_root = match disp::alloc_disp_root(rpc, client) {
        Ok(h) => {
            say(&alloc::format!("DISP: disp root {:#x} allocated", h));
            // Kept for after the boot: a second monitor's channels are
            // allocated under this root
            *CHAN_DISP_ROOT.lock() = Some(h);
            h
        }
        Err(e) => {
            said(Level::Error, &say, &fail("GB202_DISP alloc", e));
            return;
        }
    };

    // What the engine has, from its capability registers
    // (`EvoGetCapabilitiesCA`). Read and kept; the modeset uses the built-in
    // sizes, and a disagreement is said here (`dev/display-nvkms.md`, decision
    // 1)
    {
        use crate::drivers::gpu::nvidia::gsp_common::dispcaps;
        match dispcaps::read(rpc, client, h_root, bar0) {
            Ok(c) => {
                say(&alloc::format!("DISP: caps {}", c.describe()));
                if !c.plausible() {
                    say("DISP: the caps block does not read as a display engine - built-in sizes stand");
                } else if c.num_heads() != disp::CORE_HEADS || c.num_windows() != disp::CORE_WINDOWS {
                    say(&alloc::format!(
                        "DISP: caps say {} heads / {} windows, the built-in sizes {} / {}",
                        c.num_heads(), c.num_windows(), disp::CORE_HEADS, disp::CORE_WINDOWS
                    ));
                }
                dispcaps::set(Some(c));
            }
            Err(e) => {
                said(Level::Warn, &say, &fail("DISP_CAPS alloc", e));
                dispcaps::set(None);
            }
        }
    }
    // The object IMP is asked through (`nvEvoAllocRmCtrlObjectC3`). Not fatal:
    // the modeset goes on with the link's answer alone
    if let Err(e) = modeset::imp::alloc_object(rpc, client) {
        said(Level::Warn, &say, &fail("NVC372_DISPLAY_SW alloc", e));
    }

    // The pushbuffer goes in coherent system memory, which the driver can
    // allocate and map; the core channel's traffic is a handful of methods per
    // modeset
    let pb = match DmaBuffer::alloc(16) {
        Ok(b) => b,
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("DISP: pushbuffer alloc failed: {:?}", e));
            return;
        }
    };
    let (pb_phys, pb_size) = (pb.phys(), pb.size() as u64);
    // Parked before the control call: GSP-RM keeps the address, so the buffer
    // outlives this function
    mem.core_push = Some(pb);

    if let Err(e) = disp::set_channel_pushbuf(
        rpc, &internal, disp::GB202_DISP_CORE_CHANNEL_DMA, 0,
        pb_phys, pb_size, disp::PushbufTarget::SysmemCoherent,
    ) {
        said(Level::Error, &say, &fail("CHANNEL_PUSHBUFFER", e));
        return;
    }
    say(&alloc::format!(
        "DISP: core pushbuffer {} KiB at sysmem {:#x} registered", pb_size >> 10, pb_phys
    ));

    match disp::alloc_dma_channel(
        rpc, client, h_root, disp::GB202_DISP_CORE_CHANNEL_DMA, 0, 0,
    ) {
        Ok(h) => say(&alloc::format!(
            "DISP: core channel (class {:#x}) allocated as {:#x}",
            disp::GB202_DISP_CORE_CHANNEL_DMA, h
        )),
        Err(e) => {
            said(Level::Error, &say, &fail("core channel alloc", e));
            return;
        }
    }

    // Push the smallest real method stream: the usage bounds every window and
    // head declares, then UPDATE. These are permissions, so nothing appears,
    // but GET advancing to meet PUT proves the channel executes methods
    let Some(pb) = mem.core_push.as_mut() else { return };
    let mut push = crate::drivers::gpu::nvidia::gsp_common::pushbuf::PushBuf::new(
        pb.as_mut_slice(),
        crate::drivers::gpu::nvidia::gsp_common::pushbuf::CORE_CHAN_USER,
    );
    if let Err(e) = disp::core_channel_init(&mut push) {
        said(Level::Error, &say, &alloc::format!("DISP: core init stream failed: {:?}", e));
        return;
    }
    if let Err(e) = disp::core_update(&mut push, 0, 0) {
        said(Level::Error, &say, &alloc::format!("DISP: core update stream failed: {:?}", e));
        return;
    }
    push.kick(bar0);
    match push.wait_idle(bar0, 1_000_000_000) {
        Ok(()) => say("DISP: core channel executed its init stream (GET caught PUT)"),
        Err(_) => {
            say(&alloc::format!(
                "DISP: core channel did not consume the stream (get {} put {})",
                push.get(bar0), push.put_index()
            ));
            return;
        }
    }

    // Everything above was preparation. This is the modeset
    let Some((display_id, or, dpcd, mode)) = target else {
        say("DISP: no connected DisplayPort or TMDS output to drive, stopping before modeset");
        return;
    };
    // The head's output LUTs, built and parked in video memory before the
    // modeset pointing the head at the first. Three, so a curve asked for later
    // goes into one the head is not reading (`modeset::OlutRing`)
    let ring = vram_ok
        .then(|| vram::alloc(
            modeset::OLUT_RING_BYTES, 0x1000, Placement::Low, "head output LUTs",
        ))
        .flatten()
        .map(modeset::OlutRing::new);
    let olut = ring
        .map(|r| r.live())
        .and_then(|addr| {
        let mut table = alloc::vec![0u8; modeset::OLUT_BYTES];
        modeset::build_identity_olut(&mut table);
        crate::drivers::gpu::nvidia::pramin::copy_to_vram(bar0, addr, &table);
        crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);
        // Read one entry back: nothing downstream complains about the LUT, and
        // a bad write looks like a black screen
        let mut got = [0u32; 1];
        crate::drivers::gpu::nvidia::pramin::sample_vram(bar0, addr + 0x20 + 512 * 8, &mut got);
        let want = u32::from_le_bytes([table[0x20 + 512 * 8], table[0x20 + 512 * 8 + 1],
                                       table[0x20 + 512 * 8 + 2], table[0x20 + 512 * 8 + 3]]);
        DIAG.lock().olut_state = Some(if got[0] == want {
            alloc::format!("{:#x} ok", addr)
        } else {
            alloc::format!("{:#x} BAD want {:#010x} got {:#010x}", addr, want, got[0])
        });
        Some(addr)
    });

    let lit = drive_output(
        bar0, rpc, client, &mut push, display_id, &or, dpcd.as_ref(), &mode, olut, say,
    );

    // A head with nothing bound scans out black. A window turns the raster into
    // a picture
    let Some((head, sor)) = lit else { return };

    // Recorded when the modeset committed: this head and SOR belong to this
    // output from here on, and a head nobody wrote down would be handed out as
    // free.
    //
    // The mode recorded is the one the head is rastering, read back: a head
    // kept as the firmware left it (see `coldhead`) runs the firmware's timing.
    // Same size either way, so the window below is sized by the chosen one
    let running = {
        let t = modeset::head_raster(bar0, head);
        let found = t.valid().then(|| {
            output::with(display_id, |o| {
                o.modes.iter().copied().find(|m| {
                    m.h_total == t.htotal as u32
                        && m.v_total == t.vtotal as u32
                        && (m.clock_khz as u64 * 1000).abs_diff(t.pixel_clock_hz) < 50_000
                })
            })
            .flatten()
        })
        .flatten();
        match found {
            Some(m) if m.clock_khz != mode.clock_khz || m.h_total != mode.h_total => {
                let (a, b) = (m.refresh_centihz(), mode.refresh_centihz());
                say(&alloc::format!(
                    "DISP: head {} keeps the firmware's {}.{:02} Hz timing, not the {}.{:02} Hz chosen ('coldhead' programs it)",
                    head, a / 100, a % 100, b / 100, b % 100
                ));
                m
            }
            Some(m) => m,
            None => mode,
        }
    };
    output::with_mut(display_id, |o| {
        o.head = Some(head);
        o.sor = Some(sor);
        o.olut = olut.and(ring);
    });
    // What the head was made to be, in the display layer's record
    // (`drivers::gpu::kms::heads`)
    crate::drivers::gpu::kms::heads::set(crate::drivers::gpu::kms::heads::Record {
        key: display_id,
        mode: running.to_timing(),
        colour: Some(modeset::HeadColour::DEFAULT),
        asleep: false,
        vrr: None,
    });
    super::modeset::imp_opinion(bar0, rpc, client, display_id, head, super::modeset::Asked::BestSize, say);

    // Diagnostic: bring up the head with no window at all.
    //
    // A lit, synced black screen looks the same whether the fault is in the
    // head, the output, or the window's memory fetch. With no window bound, the
    // head composites only its desktop colour, which `head_set_mode` sets from
    // `DESKTOP_DIAG_*`. A visible colour confirms the head, SOR, link and panel
    // end to end and leaves the window's video-memory read (which PRAMIN cannot
    // verify, being the same path); black narrows it the other way
    const DIAG_NO_WINDOW: bool = false;
    if DIAG_NO_WINDOW {
        say("DISP: DIAG - no window attached, head shows its desktop colour only");
        settle_and_report(bar0, head, sor, say);
        return;
    }
    // Ask the raster generator to report when it starves, and to paint a colour
    // when it does.
    //
    // Overscan is generated at the raster; the active region is fetched. An
    // underflowing head paints its active region with the underflow colour
    // while blanking and overscan continue normally.
    //
    // `underflow_prop` chooses what it paints: REPEAT. `R under` says whether
    // an underflow happened
    underflow_prop(rpc, client, h_root, head, true, say);
    attach_window(
        bar0, rpc, client, &internal, h_root, &mut push,
        &mut mem.wndw_push, &mut mem.wimm_push, &mut mem.framebuffer,
        display_id, head, &mode, vram_ok, say,
    );
    // Let the raster run before asking whether it underflowed. The latch sets
    // within a frame or two of the fault, so this is eighteen frames of margin
    // at 60Hz.
    //
    // The raster's rate is measured across the same wait: frames over a
    // measured interval. That line and the underflow answer are what a boot
    // prints; the rest, and the diff against the shipping driver's armed state,
    // is `nvidia gb206-diff` (or every boot, with `trace` on the GRUB line),
    // `dev/display-nvkms.md`, decision 5
    let (rg0, _) = modeset::head_frame_counters(bar0, head);
    let sw = crate::kcore::time::Stopwatch::start();
    crate::kcore::time::mdelay(300);
    underflow_prop(rpc, client, h_root, head, false, say);
    let (rg1, _) = modeset::head_frame_counters(bar0, head);
    let hz10 = (rg1.wrapping_sub(rg0) as u64 * 10_000) / sw.elapsed_ms().max(1);
    say(&alloc::format!(
        "DISP: head {} rastering at {}.{} Hz, underflow: {}",
        head, hz10 / 10, hz10 % 10,
        DIAG.lock().underflow.as_deref().unwrap_or("not asked")
    ));
    if crate::arch::x86_64::serial::tracing() {
        settle_and_report(bar0, head, sor, say);
    }
    // The display stays up from here. The caller parks the buffers the hardware
    // reads (both pushbuffers, the window-immediate pushbuffer and the staging
    // copy of the surface) in `gspboot::DISP_MEM`
    if let Some(l) = live_scanout() {
        say(&alloc::format!(
            "DISP: scanout live - {}x{} at vram {:#x}, head {} window {}",
            l.width, l.height, l.vram, l.head, l.window
        ));
    }
    // The outputs exist now, at boot or after `gsp load`: whoever listens to
    // the display layer (the compositor's output globals) reads them again
    crate::drivers::gpu::kms::events::changed();
}
