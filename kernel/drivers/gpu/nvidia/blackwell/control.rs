// Changing a display after the boot has returned
//
// `display.rs` and `modeset.rs` run once, inside a bring-up owning the card: it
// has `bar0`, a fresh `RmClient`, a `GspRpc` over the boot arguments, and a
// core-channel pushbuffer nobody else writes. A command at the shell, or a
// monitor being unplugged, picks those up again here: the driver's RM handles
// (`chanmem::rm_client`), the rings and display pushbuffers (`gspboot::RM`,
// `gspboot::DISP_MEM`), and the output table saying which head and SOR each
// monitor was given (`gsp_common::output`).
//
// A change to a lit head (mode, colour, sleep) is one request to the display
// layer and one update (`apply.rs`, K2 of `dev/kms-architecture.md`). Here is
// what runs around that update or instead of it: variable refresh, lighting a
// new head, the hot-plug pass.
//
// The pushbuffer cursor
//
// A `PushBuf` is a position in a ring, recorded nowhere. `channel_get` reads
// GET from the channel's control region, and on an idle channel GET is where
// the next method goes; `scanout::enable_flip` seeds the window channel's
// cursor the same way.

use crate::kcore::klog::Level;
use crate::drivers::gpu::nvidia::gsp_common::modeset;
use crate::drivers::gpu::nvidia::gsp_common::output;
use crate::drivers::gpu::nvidia::gsp_common::pushbuf::{self, PushBuf};
use crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient;
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

use super::gspboot::{DisplayMemory, DISP_MEM, RM};

/// RM, then the display pushbuffers: the order for every caller
/// (`dev/gsp-foundation.md`, decision 8). Both are sleeping locks, since an RPC
/// in here may sleep waiting for GSP-RM
pub(super) fn with_rm_and_disp<R>(
    f: impl FnOnce(&MmioRegion, &mut GspRpc, &RmClient, &mut DisplayMemory) -> R,
) -> Result<R, &'static str> {
    let client = super::chanmem::rm_client().ok_or("no RM client - the card was never booted")?;
    let bar0 = crate::drivers::gpu::nvidia::device().ok_or("no GB20x in this machine")?.bar0;
    let mut rm = RM.lock();
    let rm = rm.as_mut().ok_or("GSP-RM is not booted")?;
    let mut disp = DISP_MEM.lock();
    let disp = disp.as_mut().ok_or("GSP-RM is not booted")?;
    let mut rpc = GspRpc::new(&bar0, &mut rm.bootargs);
    Ok(f(&bar0, &mut rpc, &client, disp))
}

/// An RPC, with no display in it: the rings and the driver's RM handles,
/// under the RM lock alone
pub fn with_rm<R>(
    f: impl FnOnce(&MmioRegion, &mut GspRpc, &RmClient) -> R,
) -> Result<R, &'static str> {
    let client = super::chanmem::rm_client().ok_or("no RM client - the card was never booted")?;
    let bar0 = crate::drivers::gpu::nvidia::device().ok_or("no GB20x in this machine")?.bar0;
    let mut rm = RM.lock();
    let rm = rm.as_mut().ok_or("GSP-RM is not booted")?;
    let mut rpc = GspRpc::new(&bar0, &mut rm.bootargs);
    Ok(f(&bar0, &mut rpc, &client))
}

/// The closing `R` block of a bring-up and the diff against the shipping
/// driver's armed state, on demand: `nvidia gb206-diff`. A boot keeps one line
/// of it (`dev/display-nvkms.md`, decision 5). Register reads only; takes 600
/// ms to count frames
pub fn settle_report(say: &dyn Fn(&str)) -> Result<(), &'static str> {
    let bar0 = crate::drivers::gpu::nvidia::device().ok_or("no GB20x in this machine")?.bar0;
    let (id, head, sor, is_dp) = output::with_primary(|o| Some((o.display_id, o.head?, o.sor?, o.or.is_dp())))
        .flatten()
        .ok_or("nothing is lit - the report is about a head that is scanning")?;
    // RM's view of the link, asked now rather than carried from the boot
    if is_dp {
        if let Ok(v) = with_rm(|_bar0, rpc, client| super::modeset::rm_link_view(rpc, client, id)) {
            super::modeset::DIAG.lock().rm_view = Some(v);
        }
    }
    super::modeset::settle_and_report(&bar0, head, sor, say);
    Ok(())
}

/// IMP's opinion of every mode a lit output offers, next to the link's:
/// `nvidia disp imp`. The lines, then how often the two have agreed since boot
pub fn imp_opinion(display_id: u32) -> Result<alloc::vec::Vec<alloc::string::String>, &'static str> {
    let head = output::with(display_id, |o| o.head)
        .ok_or("no such output")?
        .ok_or("that output is not lit - IMP is asked about the head driving it")?;
    let lines = spin::Mutex::new(alloc::vec::Vec::new());
    with_rm(|bar0, rpc, client| {
        super::modeset::imp_opinion(bar0, rpc, client, display_id, head, super::modeset::Asked::All,
            &|l: &str| lines.lock().push(alloc::string::String::from(l)));
    })?;
    let mut lines = lines.into_inner();
    let (agreed, disagreed) = crate::drivers::gpu::nvidia::gsp_common::modeset::imp::agreement();
    lines.push(alloc::format!("since boot: {} agreed, {} disagreed", agreed, disagreed));
    Ok(lines)
}

/// IMP's opinion of one mode on `display_id`'s head, logged beside the link's,
/// asked before a mode change is applied (`dev/display-nvkms.md`, decision 2).
/// Logged only: the link decides
pub fn imp_opinion_of(display_id: u32, mode: &modeset::Mode) {
    let Some(head) = output::with(display_id, |o| o.head).flatten() else { return };
    let _ = with_rm(|bar0, rpc, client| {
        super::modeset::imp_opinion(bar0, rpc, client, display_id, head,
            super::modeset::Asked::One(mode), &|l: &str| crate::kinfo!("[nvidia] {}", l));
    });
}

/// Everything a post-boot display change needs, assembled once: the rings, the
/// RM handles and the core channel's pushbuffer, resumed where the hardware
/// says it is.
///
/// A closure, so no caller holds the locks by hand in the wrong order
pub fn with_display<R>(
    f: impl FnOnce(&MmioRegion, &mut GspRpc, &RmClient, &mut PushBuf) -> R,
) -> Result<R, &'static str> {
    with_rm_and_disp(|bar0, rpc, client, mem| {
        let Some(pb) = mem.core_push.as_mut() else {
            return Err("no core pushbuffer - the display bring-up did not get that far");
        };
        let cur = pushbuf::channel_get(bar0, pushbuf::CORE_CHAN_USER);
        let mut push = PushBuf::resume(pb.as_mut_slice(), pushbuf::CORE_CHAN_USER, cur);
        Ok(f(bar0, rpc, client, &mut push))
    })?
}

/// Variable refresh on or off on one output (`dev/display-nvkms.md`, decision
/// 6): `nvEnableVrr` then `SetStallLockOneDisp` to turn it on, `nvDisableVrr`
/// (which deactivates on its way) to turn it off, for this output's head alone.
/// Returns the minimum-refresh timeout in microseconds, zero when off.
///
/// Allowed under a running compositor: `with_display` holds the display
/// pushbuffers, so no flip goes out between the supervisor being armed and
/// disarmed, and the update is interlocked with nothing.
///
/// The display layer calls this around its update (`nvDisableVrr` before,
/// `nvEnableVrr` after); its record of the head is written here, once the
/// hardware took it
pub fn set_vrr(display_id: u32, on: bool) -> Result<u32, &'static str> {
    use crate::drivers::gpu::kms::heads;
    use crate::drivers::gpu::nvidia::gsp_common::modeset::vrr;

    let Some(head) = output::with(display_id, |o| o.head).flatten() else {
        return Err("that output is not being driven");
    };
    let (is_dp, dpcd, range) = output::with(display_id, |o| (o.or.is_dp(), o.dpcd, o.vrr_range))
        .ok_or("no such output")?;
    let rec = heads::get(display_id).ok_or("the display layer has no record of that head")?;
    let mode = modeset::Mode::from_timing(&rec.mode);
    match (on, rec.vrr) {
        (true, Some(timeout_us)) => return Ok(timeout_us),
        (false, None) => return Ok(0),
        _ => {}
    }
    if on && rec.asleep {
        return Err("the output is asleep - wake it first");
    }
    let timeout = match rec.vrr {
        Some(timeout_us) => timeout_us,
        None => vrr::allowed(is_dp, dpcd.as_ref(), range, Some(&mode))?,
    };

    with_display(|bar0, rpc, client, push| {
        if on {
            vrr_enable(bar0, rpc, client, push, display_id, head, &mode, timeout)
        } else {
            vrr_disable(bar0, rpc, client, push, display_id, head, &mode, timeout);
            Ok(())
        }
    })??;

    heads::update(display_id, |r| r.vrr = on.then_some(timeout));
    vrr::set_active(head, on);
    crate::knotice!("[nvidia] {:#010x} head {}: variable refresh {}", display_id, head,
        if on { alloc::format!("on, one-shot, repeats after {} us", timeout) } else { "off".into() });
    Ok(if on { timeout } else { 0 })
}

/// `nvEnableVrr` and the activating half of `SetStallLockOneDisp`. A failure
/// stops at the two steps after which the panel would be told something it
/// cannot follow; what was changed is put back
#[allow(clippy::too_many_arguments)]
fn vrr_enable(
    bar0: &MmioRegion,
    rpc: &mut GspRpc,
    client: &RmClient,
    push: &mut PushBuf,
    display_id: u32,
    head: u32,
    mode: &modeset::Mode,
    timeout: u32,
) -> Result<(), &'static str> {
    use crate::drivers::gpu::nvidia::gsp_common::{disp, modeset::vrr};

    // The sink first (`nvDPLibSetAdaptiveSync`): stop locking to the timing the
    // stream carries. Not acknowledged is a refusal, unlike NVKMS
    if let Err(e) = set_msa_ignore(rpc, client, display_id, true) {
        crate::kwarn!("[nvidia] {:#010x}: DPCD 0x107 not written: {:?}", display_id, e);
        return Err("the sink did not take MSA_TIMING_PAR_IGNORE_EN (DPCD 0x107)");
    }
    // RM's half: which displays stretch vblank, and how far. A failure is a
    // memory clock that will not switch inside the blank; NVKMS only warns
    let warn = |what: &str, e: crate::drivers::gpu::nvidia::gsp_common::rmapi::RmError| {
        crate::kwarn!("[nvidia] {:#010x}: {} failed: {:?}", display_id, what, e)
    };
    if let Err(e) = vrr::control(rpc, client, vrr::CMD_VRR_DISPLAY_INFO, &vrr::display_info_params(display_id, true)) {
        warn("VRR_DISPLAY_INFO", e);
    }
    if let Err(e) = vrr::control(rpc, client, vrr::CMD_CONFIG_VRR_PSTATE_SWITCH,
                                 &vrr::pstate_params(display_id, true, false, true, mode, timeout)) {
        warn("CONFIG_VRR_PSTATE_SWITCH (enable)", e);
    }

    // Activate
    if let Err(e) = vrr::control(rpc, client, vrr::CMD_CONFIG_VRR_PSTATE_SWITCH,
                                 &vrr::pstate_params(display_id, true, true, false, mode, timeout)) {
        warn("CONFIG_VRR_PSTATE_SWITCH (state)", e);
    }
    // Whatever was pushed before has landed (`nvRMSyncEvoChannel`)
    let undo = |rpc: &mut GspRpc| {
        let _ = vrr::control(rpc, client, vrr::CMD_CONFIG_VRR_PSTATE_SWITCH,
                             &vrr::pstate_params(display_id, false, false, true, mode, timeout));
        let _ = vrr::control(rpc, client, vrr::CMD_VRR_DISPLAY_INFO, &vrr::display_info_params(display_id, false));
        let _ = set_msa_ignore(rpc, client, display_id, false);
    };
    if push.wait_idle(bar0, 2_000_000_000).is_err() {
        undo(rpc);
        return Err("the core channel was busy before the stall lock");
    }
    if let Err(e) = vrr::control(rpc, client, vrr::CMD_ARM_LIGHTWEIGHT_SUPERVISOR,
                                 &vrr::lwsv_params(display_id, true, true, mode)) {
        warn("ARM_LIGHTWEIGHT_SUPERVISOR", e);
        undo(rpc);
        return Err("RM would not arm the lightweight supervisor - nothing was changed");
    }
    let pushed = vrr::head_methods(push, head, true, timeout).is_ok()
        && disp::core_update(push, 0, 0).is_ok();
    if pushed {
        push.kick(bar0);
    }
    let idle = pushed && push.wait_idle(bar0, 2_000_000_000).is_ok();
    if let Err(e) = vrr::control(rpc, client, vrr::CMD_ARM_LIGHTWEIGHT_SUPERVISOR,
                                 &vrr::lwsv_params(display_id, false, true, mode)) {
        warn("ARM_LIGHTWEIGHT_SUPERVISOR (disarm)", e);
    }
    if !pushed {
        undo(rpc);
        return Err("the core channel would not take the stall-lock methods");
    }
    if !idle {
        return Err("the core channel did not consume the stall-lock update - 'display vrr off'");
    }
    Ok(())
}

/// `nvDisableVrr`: RM told first, then the head deactivated as
/// `SetStallLockOneDisp` does it, RM's pstate half off, the sink last. Every
/// step is tried whatever the one before said
#[allow(clippy::too_many_arguments)]
fn vrr_disable(
    bar0: &MmioRegion,
    rpc: &mut GspRpc,
    client: &RmClient,
    push: &mut PushBuf,
    display_id: u32,
    head: u32,
    mode: &modeset::Mode,
    timeout: u32,
) {
    use crate::drivers::gpu::nvidia::gsp_common::{disp, modeset::vrr};

    let warn = |what: &str| crate::kwarn!("[nvidia] {:#010x}: variable refresh off: {} failed", display_id, what);
    if vrr::control(rpc, client, vrr::CMD_VRR_DISPLAY_INFO, &vrr::display_info_params(display_id, false)).is_err() {
        warn("VRR_DISPLAY_INFO");
    }
    if push.wait_idle(bar0, 2_000_000_000).is_err() {
        warn("waiting for the core channel");
    }
    if vrr::control(rpc, client, vrr::CMD_ARM_LIGHTWEIGHT_SUPERVISOR,
                    &vrr::lwsv_params(display_id, true, false, mode)).is_err() {
        warn("ARM_LIGHTWEIGHT_SUPERVISOR");
    }
    if vrr::control(rpc, client, vrr::CMD_CLEAR_ELV_BLOCK, &vrr::clear_elv_params(display_id)).is_err() {
        warn("CLEAR_ELV_BLOCK");
    }
    if vrr::head_methods(push, head, false, 0).is_ok() && disp::core_update(push, 0, 0).is_ok() {
        push.kick(bar0);
        if push.wait_idle(bar0, 2_000_000_000).is_err() {
            warn("the core channel consuming the update");
        }
    } else {
        warn("pushing the stall-lock methods");
    }
    if vrr::control(rpc, client, vrr::CMD_ARM_LIGHTWEIGHT_SUPERVISOR,
                    &vrr::lwsv_params(display_id, false, false, mode)).is_err() {
        warn("ARM_LIGHTWEIGHT_SUPERVISOR (disarm)");
    }
    if vrr::control(rpc, client, vrr::CMD_CONFIG_VRR_PSTATE_SWITCH,
                    &vrr::pstate_params(display_id, true, false, false, mode, timeout)).is_err() {
        warn("CONFIG_VRR_PSTATE_SWITCH (state)");
    }
    if vrr::control(rpc, client, vrr::CMD_CONFIG_VRR_PSTATE_SWITCH,
                    &vrr::pstate_params(display_id, false, false, true, mode, timeout)).is_err() {
        warn("CONFIG_VRR_PSTATE_SWITCH (disable)");
    }
    if set_msa_ignore(rpc, client, display_id, false).is_err() {
        warn("DPCD 0x107");
    }
}

/// DPCD 0x107 bit 7, read-modify-write (`setIgnoreMSATimingParamters`).
/// Returns whether it was already as asked
fn set_msa_ignore(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    on: bool,
) -> Result<bool, crate::drivers::gpu::nvidia::gsp_common::rmapi::RmError> {
    use crate::drivers::gpu::nvidia::gsp_common::{disp, modeset::vrr};
    let b = disp::dpcd_read(rpc, client, display_id, vrr::DPCD_DOWNSPREAD_CTRL, 1)?
        .first()
        .copied()
        .ok_or(crate::drivers::gpu::nvidia::gsp_common::rmapi::RmError::ShortReply)?;
    let want = if on { b | vrr::DPCD_MSA_TIMING_PAR_IGNORE_EN } else { b & !vrr::DPCD_MSA_TIMING_PAR_IGNORE_EN };
    if want == b {
        return Ok(true);
    }
    disp::dpcd_write(rpc, client, display_id, vrr::DPCD_DOWNSPREAD_CTRL, &[want])?;
    Ok(false)
}

/// Every head with variable refresh, off, before GSP-RM is unloaded. The next
/// boot keeps the firmware's head and does not program it, so a head left
/// stall-locked stays so
pub fn vrr_all_off(say: &dyn Fn(&str)) {
    let ids: alloc::vec::Vec<u32> = crate::drivers::gpu::kms::heads::all()
        .iter()
        .filter(|r| r.vrr.is_some())
        .map(|r| r.key)
        .collect();
    for id in ids {
        match set_vrr(id, false) {
            Ok(_) => say(&alloc::format!("{:#010x}: variable refresh off", id)),
            Err(e) => crate::drivers::gpu::nvidia::said(Level::Warn, &say,
                &alloc::format!("{:#010x}: variable refresh not turned off: {}", id, e)),
        }
    }
}

/// Which SOR protocol this output speaks, for putting a head back on it. The
/// connector's reported protocol answers for both families: the
/// `NV0073_CTRL_SPECIFIC_OR_PROTOCOL_SOR_*` numbers and the class enum agree
pub(super) fn sor_protocol(or_protocol: u32, is_dp: bool) -> u32 {
    if is_dp {
        if or_protocol == 9 { modeset::SOR_PROTOCOL_DP_B } else { modeset::SOR_PROTOCOL_DP_A }
    } else {
        or_protocol
    }
}

// Cables arriving and leaving

/// Re-read everything about one output from the card.
///
/// The questions of `enumerate_displays`, about a single connector: what it is,
/// whether something is on it, and what that something can do. For a monitor
/// swapped on the same cable
pub fn refresh(display_id: u32) -> Result<bool, &'static str> {
    use crate::drivers::gpu::nvidia::gsp_common::disp;

    with_rm(|_bar0, rpc, client| {
        let or = disp::or_info(rpc, client, display_id).map_err(|_| "OR_GET_INFO failed")?;
        let connected = disp::connect_state(rpc, client, display_id)
            .map_err(|_| "GET_CONNECT_STATE failed")?
            & display_id
            != 0;

        let mut out = output::Output::new(display_id, or);
        out.connected = connected;
        if connected {
            if or.is_dp() {
                if let Ok(d) = disp::read_dpcd_caps(rpc, client, display_id) {
                    out.dpcd = d.map(|(c, _)| c);
                }
            }
            if let Ok(edid) = disp::read_edid(rpc, client, display_id) {
                if let Some(e) = super::modeset::parse_edid(&edid) {
                    out.name = alloc::string::String::from(e.name_str());
                    out.hdmi = e.hdmi;
                    out.vrr_range = modeset::vrr::edid_range(e.v_min_hz, e.v_max_hz);
                    out.modes = modeset::Mode::list_from(&e);
                }
                out.edid = edid;
            }
        }
        // Whatever the connector was driving survives the re-read: the head and
        // the SOR belong to the connector, and forgetting them leaks a head on
        // every unplug. What the head was asked to be is the display layer's
        // record
        output::with(display_id, |old| (old.head, old.sor, old.olut, old.scanout, old.cursor, old.connector))
            .map(|(head, sor, olut, scanout, cursor, connector)| {
                out.head = head;
                out.sor = sor;
                out.olut = olut;
                out.scanout = scanout;
                out.cursor = cursor;
                out.connector = connector;
            });
        output::add(out);
        Ok(connected)
    })?
}

/// Drain GSP-RM's event queue and act on what happened to a monitor.
///
/// Two halves: the first is inside the transport and only writes numbers down;
/// the second is outside it and may make RM calls. A hot-plug handler in the
/// first would issue a `GET_CONNECT_STATE` from inside the loop draining the
/// queue its answer arrives on.
///
/// Returns how many events went past, what `nvidia disp poll` prints. Nothing
/// else drains this queue after boot, and an undrained queue eventually stops
/// the card talking
pub fn poll_events() -> usize {
    use crate::drivers::gpu::nvidia::gsp_common::rpc;

    // Every event goes through the dispatcher, which writes down what needs
    // following up (the journal, the hot-plug masks) and returns
    let drained = with_rm(|_bar0, rpc_, _client| {
        let mut handled = 0;
        let mut dispatch = rpc_.events();
        if let Err(e) = rpc_.drain(rpc::NOTHING, None, rpc::Context::Interrupt, &mut dispatch, &mut handled) {
            crate::serial_println!("[nvidia] event drain stopped: {:?}", e);
        }
        handled
    })
    .unwrap_or(0);

    // Outside the transport now
    report_trouble();
    // GSP-RM polls its clocks and temperatures into the shared page only while
    // somebody reads it; the readers only say so, and the RPC is made here
    // (`perf::rusd`, `dev/gpu-perf.md`)
    if crate::drivers::gpu::nvidia::gsp_common::perf::rusd::pending().is_some() {
        if let Some(internal) = super::chanmem::internal_client() {
            let _ = with_rm(|_bar0, rpc_, _client| {
                crate::drivers::gpu::nvidia::gsp_common::perf::rusd::service(rpc_, &internal)
            });
        }
    }
    // A compute channel the report above (or a stuck frame) killed is rebuilt
    // here, on the event thread, where waiting on RM is allowed
    super::compute_chan::recover_if_dead();

    let short = output::take_dp_irq();
    for bit in 0..32u32 {
        if short & (1 << bit) != 0 {
            short_pulse(1 << bit);
        }
    }

    let (plugged, unplugged) = output::take_hotplug();
    for bit in 0..32u32 {
        let id = 1u32 << bit;
        if unplugged & id != 0 {
            crate::serial_println!("[nvidia] {:#010x} unplugged", id);
            // Asleep, not torn down: the head, the window and the surface
            // belong to the connector, and the next monitor on this cable wants
            // them. Marked unplugged first, so the sleep does not ask a gone
            // sink to go to D3, and through the display layer like any other
            // change to a head
            output::with_mut(id, |o| o.connected = false);
            asleep_through_layer(id, true);
            let _ = refresh(id);
        }
        if plugged & id != 0 {
            crate::serial_println!("[nvidia] {:#010x} plugged in", id);
            match refresh(id) {
                Ok(true) => {
                    // It already had a head, blanked on the way out, so waking
                    // it is all a re-plug needs. A connector that never had one
                    // needs a modeset, `enumerate_displays`' job, not to be run
                    // from an event handler on a live screen
                    let known = output::with(id, |o| (o.head.is_some(), o.modes.len()));
                    match known {
                        Some((true, _)) => asleep_through_layer(id, false),
                        Some((false, n)) => crate::serial_println!(
                            "[nvidia] {:#010x} has {} mode(s) and no head yet - \
                             'nvidia gb206-boot' would light it",
                            id, n
                        ),
                        None => {}
                    }
                }
                Ok(false) => crate::serial_println!(
                    "[nvidia] {:#010x} reported a plug but reads as disconnected", id
                ),
                Err(e) => crate::kwarn!("[nvidia] {:#010x} refresh: {}", id, e),
            }
        }
    }
    // A cable moved, so what is connected changed, even where nothing could be
    // woken or put to sleep
    if plugged | unplugged != 0 {
        crate::drivers::gpu::kms::events::changed();
    }
    drained
}

/// Sleep or wake the output a cable just left or reached, as one request to the
/// display layer: its lock, its check, its record
fn asleep_through_layer(display_id: u32, asleep: bool) {
    use crate::drivers::gpu::kms;
    let st = kms::state();
    let Some(o) = st.by_key(display_id) else { return };
    if o.head.is_none() {
        return;
    }
    let mut r = kms::HeadRequest::current(o);
    r.asleep = asleep;
    match kms::apply(&kms::Request::one(r, false)) {
        Ok(lines) => crate::serial_println!("[nvidia] {:#010x}: {}", display_id, lines.join("; ")),
        Err(e) => crate::kwarn!("[nvidia] {:#010x} not {}: {}", display_id,
            if asleep { "put to sleep" } else { "woken" }, e),
    }
}

/// A DisplayPort sink's short pulse, for a single-stream link
/// (`ConnectorImpl::notifyShortPulse`, the part applying without MST, HDCP or
/// compliance tests): if the sink is awake and the link has lost lock, retrain
/// it at the configuration RM has up.
///
/// A lost link is a black screen already; a link that is fine is left alone,
/// whatever else the vector says
fn short_pulse(display_id: u32) {
    let Some((is_dp, lit, dpcd)) = output::with(display_id, |o| (o.or.is_dp(), o.head.is_some(), o.dpcd)) else {
        return;
    };
    if !is_dp || !lit {
        return;
    }
    let r = with_rm(|_bar0, rpc, client| restore_link(rpc, client, display_id, dpcd.as_ref()));
    match r {
        Ok((level, what)) => crate::kcore::klog::emit(level, crate::klog_site!(),
            format_args!("[nvidia] {:#010x} short pulse: {}", display_id, what)),
        Err(e) => crate::kwarn!("[nvidia] {:#010x} short pulse: {}", display_id, e),
    }
}

/// If the sink is awake and its link has lost lock, retrain it at what RM has
/// up (`isLinkInD3`, `isLinkLost`, then the retrain `notifyShortPulse` and a
/// wake from D3 both end in). Returns what happened, in words, and how much it
/// matters
pub(super) fn restore_link(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    dpcd: Option<&crate::drivers::gpu::nvidia::gsp_common::disp::DpcdCaps>,
) -> (Level, alloc::string::String) {
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    let power = disp::dpcd_read(rpc, client, display_id, modeset::DPCD_SET_POWER, 1)
        .ok()
        .and_then(|b| b.first().copied());
    if modeset::dp_in_d3(power) {
        return (Level::Info, alloc::format!("asleep (SET_POWER {:?}) - nothing to restore", power));
    }
    let status = match disp::dpcd_read(rpc, client, display_id, modeset::DPCD_LINK_STATUS,
                                       modeset::DPCD_LINK_STATUS_LEN) {
        Ok(s) => s,
        Err(e) => return (Level::Warn, alloc::format!("link status unreadable: {:?}", e)),
    };
    let (lanes, bw) = modeset::dp_get_link_config(rpc, client, display_id).unwrap_or((0, 0));
    if !modeset::dp_link_lost(lanes, &status) {
        return (Level::Info, alloc::format!("link holds ({} lanes x {:#x}), status {:02x?}", lanes, bw, status));
    }
    // What RM has up, or everything the sink offers if RM has nothing
    let Some(d) = dpcd else {
        return (Level::Error, alloc::string::String::from("link lost, and no DPCD to retrain against"));
    };
    let (l, b) = if lanes != 0 && bw != 0 { (lanes, bw) } else { (d.max_lane_count as u32, d.max_link_rate as u32) };
    match modeset::dp_train_target(rpc, client, display_id, 0, l, b, d) {
        Ok((gl, gb)) => (Level::Notice,
            alloc::format!("link lost (status {:02x?}) - retrained at {} lanes x {:#x}{}", status, gl, gb,
                msa_after_retrain(rpc, client, display_id))),
        Err(e) => (Level::Error,
            alloc::format!("link lost (status {:02x?}) - retraining failed: {:?}", status, e)),
    }
}

/// RM trains the link through `DP_CTRL`, which writes DPCD 0x107 for the
/// downspread. Whether it keeps bit 7 is inside GSP-RM, so a retrain under
/// variable refresh reads the byte back and sets the bit again if it went
fn msa_after_retrain(rpc: &mut GspRpc, client: &RmClient, display_id: u32) -> &'static str {
    if crate::drivers::gpu::kms::heads::get(display_id).map_or(true, |r| r.vrr.is_none()) {
        return "";
    }
    match set_msa_ignore(rpc, client, display_id, true) {
        Ok(true) => ", MSA ignore kept",
        Ok(false) => ", MSA ignore was cleared - set again",
        Err(_) => ", MSA ignore could not be checked",
    }
}

/// Say it where it will be read.
///
/// This runs on the event thread, not inside a syscall, so the kernel console
/// paints from here. A robust-channel report on serial alone goes unseen on a
/// machine with a monitor and no cable. Bounded, so a channel faulting in a
/// loop cannot take the screen
fn loud(line: &str) {
    crate::kerror!("[nvidia] {}", line);
    on_screen(line);
}

/// The screen half of `loud`, for what the kernel log has: an Xid or a
/// robust-channel report is written down when it arrives (`kgsp::events`).
/// `println!` records what it prints, so the relay runs under
/// `klog::unrecorded`
fn on_screen(line: &str) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static SEEN: AtomicU32 = AtomicU32::new(0);
    const ON_SCREEN: u32 = 8;

    let n = SEEN.fetch_add(1, Ordering::Relaxed);
    crate::kcore::klog::unrecorded(|| {
        if n < ON_SCREEN {
            crate::println!("  nvidia: {}", line);
        } else if n == ON_SCREEN {
            crate::println!("  nvidia: further GPU error reports are in 'nvidia log err' only");
        }
    });
}

/// Act on what the drain wrote down: say what died, read the fault buffer, and
/// stop handing work to a channel GSP-RM has killed.
///
/// Everything here reads a register or touches driver state, so it is outside
/// the closure above
fn report_trouble() {
    // First and once: the health check found GSP-RM dead, and could say so only
    // on serial from where it runs
    {
        use core::sync::atomic::{AtomicBool, Ordering};
        static SHOWN: AtomicBool = AtomicBool::new(false);
        if crate::drivers::gpu::nvidia::gsp_common::kgsp::fatal() && !SHOWN.swap(true, Ordering::AcqRel) {
            on_screen("GSP-RM has died - no further RPCs, every channel has stopped; the GPU needs a reset");
            // `gsprecover`: the reset and a second boot, on a worker
            // (`dev/gsp-foundation.md`, 16). Without it, it stays dead
            super::bringup::on_fatal();
        }
    }
    let j = crate::drivers::gpu::nvidia::gsp_common::kgsp::events::take_journal();
    if j.is_empty() {
        return;
    }
    let (rc, rc_dropped, mmu_queued, nocat, xid) = (j.rc, j.rc_dropped, j.mmu_queued, j.nocat, j.xid);

    // First: the one line saying what happened in words.
    //
    // Reported, not acted on: an Xid is GSP-RM's account of something wrong,
    // and many are survivable. `RC_TRIGGERED` means RM killed a channel, and
    // only it marks ours dead: taking the copy engine down on a report blanks
    // the screen for something the card recovered from
    for (n, chid, text) in &xid {
        on_screen(&alloc::format!("GSP Xid {} (chid {}): {}", n, chid, text));
    }

    let mut want_fault_buffer = mmu_queued != 0;
    for r in &rc {
        on_screen(&r.describe());
        if r.is_mmu_fault() {
            want_fault_buffer = true;
        }
        // A chid this driver does not own belongs to GSP-RM, and saying so
        // tells "our channel died" from "the firmware's did"
        if !super::channel::note_channel_died(r.chid) {
            loud(&alloc::format!("chid {} is not one of ours - GSP-RM's own channel", r.chid));
        }
    }
    if rc_dropped != 0 {
        loud(&alloc::format!(
            "{} further robust-channel report(s) dropped - this is a storm", rc_dropped
        ));
    }
    for line in &nocat {
        on_screen(line);
    }

    // The addresses live in the fault buffer. The manual `nvidia gmmu-faults`
    // dump, run when it has something to say
    if want_fault_buffer {
        let relay = |line: &str| crate::kerror!("[nvidia] {}", line);
        crate::drivers::gpu::nvidia::with_gb20x(|d| {
            crate::drivers::gpu::nvidia::gmmu::dump_mmu_faults(&d.bar0, &relay);
        });
    }
}

/// The numbers GSP-RM last wrote into the shared page, aged against this card's
/// PTIMER, with GSP-RM kept polling for whoever reads next (`perf::snapshot`).
/// `None` with no card or no page linked. Never waits on GSP-RM, so safe on a
/// syscall path
pub fn perf_snapshot() -> Option<crate::drivers::gpu::nvidia::gsp_common::perf::Snapshot> {
    if !super::gspboot::is_booted() {
        return None;
    }
    let ptimer = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0.read_ptimer_ns());
    crate::drivers::gpu::nvidia::gsp_common::perf::snapshot(ptimer)
}

/// Subscribe to the two notifications a display driver cares about.
///
/// Called at the end of the bring-up, once there is a client and a display
/// object. Failure is a line and not an error: without hot-plug a new monitor
/// has to be asked for by hand
pub fn subscribe_display_events(
    rpc: &mut GspRpc,
    client: &RmClient,
    say: &dyn Fn(&str),
) {
    // Handles after nouveau's, so traces of the two drivers read alike
    for (handle, id, what) in [
        (0x007e_0000u32, output::NOTIFIER_HOTPLUG, "hotplug"),
        (0x007e_0001u32, output::NOTIFIER_DP_IRQ, "DP IRQ"),
    ] {
        match client.subscribe_event(rpc, handle, id) {
            Ok(()) => say(&alloc::format!("DISP: subscribed to {} events", what)),
            Err(e) => say(&alloc::format!("DISP: no {} events: {:?}", what, e)),
        }
    }
}

/// The longest the event thread sleeps. The GSP's interrupt wakes it
/// (`kgsp::worker`) whenever GSP-RM says anything; this is the backstop, for a
/// missed SWGEN0 or an interrupt that is off (`nointr`), and at a fifth of a
/// second a cable is still a human action
const BACKSTOP_MS: u64 = 200;

/// The thread keeping the card's event queue moving: OpenRM's DPC half
/// (`kgspRpcRecvEvents`).
///
/// Every other drain is a side effect of an RM call: `call` hands the events it
/// passes to the dispatcher on its way to the reply, but only the journal hears
/// them, and only this thread acts on the journal. A machine left alone at a
/// desktop makes no RM calls, and a full queue is a card that stops answering
fn event_thread() -> ! {
    // Ticks, not milliseconds: the scheduler counts in timer ticks, 250 a
    // second
    const TICKS: u64 = BACKSTOP_MS * 250 / 1000;
    loop {
        crate::drivers::gpu::nvidia::gsp_common::kgsp::worker::wait(TICKS);
        if !super::gspboot::is_booted() {
            continue;
        }
        poll_events();
    }
}

/// Start the event thread, once, at the end of a boot that brought RM up
pub fn start_event_thread() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::sched::spawn_named(event_thread, "gpu-events", 1);
}

/// Bring up a monitor the boot did not.
///
/// The same `drive_output` and `attach_window` the boot runs, on a connector
/// enumerated and left alone, while the first monitor keeps showing the
/// desktop. The display object, the disp root and the core channel are the
/// boot's; the head, the window, the surface and the LUTs are this monitor's.
///
/// `want` is a mode from the monitor's list; `None` takes the best the link can
/// carry, as the boot does. Called by the display layer's driver
/// (`nvidia/kms.rs`), which has refused it under a running compositor: the core
/// channel's UPDATE arms every head at once, and a second head's modeset under
/// a compositor flipping the first head's window is two writers on one ring.
/// The layer's record of the head is written here, once lit
pub fn light(display_id: u32, want: Option<usize>) -> Result<(), &'static str> {
    use crate::drivers::gpu::nvidia::gsp_common::vram;
    use crate::drivers::gpu::nvidia::pramin;

    let Some(internal) = super::chanmem::internal_client() else {
        return Err("no internal RM handles - the display bring-up did not reach them");
    };
    let Some(h_root) = *super::chanmem::DISP_ROOT.lock() else {
        return Err("no disp root - the display bring-up did not reach it");
    };

    let Some((or, dpcd, caps, modes, already)) = output::with(display_id, |o| {
        (o.or, o.dpcd, o.hdmi, o.modes.clone(), o.head.is_some())
    }) else {
        return Err("no such output");
    };
    if already {
        return Err("that output already has a head - use 'display on' or 'display mode'");
    }
    // The mode asked for, or the best the link can carry: the same walk as the
    // boot
    let possible = |m: &modeset::Mode| super::modeset::mode_possible(&or, dpcd.as_ref(), &caps, m, super::modeset::BPC);
    let mode = match want {
        Some(i) => *modes.get(i).filter(|m| possible(m)).ok_or("that mode is not one this link can carry")?,
        None => *modes.iter().find(|m| possible(m))
            .ok_or("nothing is plugged into it, or it offers nothing this link can carry")?,
    };

    // The head's output LUTs, built and parked in video memory before the
    // modeset pointing the head at the first. Its own: the LUT is head state
    let olut = vram::alloc(
        modeset::OLUT_RING_BYTES, 0x1000, vram::Placement::Low, "head output LUTs",
    ).map(modeset::OlutRing::new);

    let say = |s: &str| crate::serial_println!("{}", s);
    let lit = with_display(|bar0, rpc, client, push| {
        if let Some(ring) = olut {
            let mut table = alloc::vec![0u8; modeset::OLUT_BYTES];
            modeset::build_identity_olut(&mut table);
            pramin::copy_to_vram(bar0, ring.live(), &table);
            pramin::flush_fb(bar0);
        }
        super::modeset::drive_output(
            bar0, rpc, client, push, display_id, &or, dpcd.as_ref(), &mode,
            olut.map(|r| r.live()), &say,
        )
    })?;
    let Some((head, sor)) = lit else {
        if let Some(r) = olut {
            vram::free(r.base);
        }
        return Err("the modeset did not light it - see serial");
    };
    output::with_mut(display_id, |o| {
        o.head = Some(head);
        o.sor = Some(sor);
        o.olut = olut;
    });
    crate::drivers::gpu::kms::heads::set(crate::drivers::gpu::kms::heads::Record {
        key: display_id,
        mode: mode.to_timing(),
        colour: Some(modeset::HeadColour::DEFAULT),
        asleep: false,
        vrr: None,
    });

    // And the window, which turns the raster into a picture. Its own
    // pushbuffers and surface, in this head's slot. It needs the core
    // pushbuffer and this head's slots of the other three at once: one borrow
    // of the display memory, split by field
    let attached = with_rm_and_disp(|bar0, rpc, client, mem| {
        let DisplayMemory { core_push, wndw_push, wimm_push, framebuffer } = mem;
        let Some(pb) = core_push.as_mut() else { return false };
        let cur = pushbuf::channel_get(bar0, pushbuf::CORE_CHAN_USER);
        let mut push = PushBuf::resume(pb.as_mut_slice(), pushbuf::CORE_CHAN_USER, cur);
        super::window::attach_window(
            bar0, rpc, client, &internal, h_root, &mut push,
            wndw_push, wimm_push, framebuffer,
            display_id, head, &mode, true, &say,
        );
        true
    });
    match attached {
        Ok(true) => Ok(()),
        _ => Err("the window setup did not run - see serial"),
    }
}

/// Ask the raster generator whether it has run dry since last asked.
///
/// Underflow is invisible from this side unless asked: the head keeps scanning,
/// the link keeps carrying, and the last pixel repeats across whatever the
/// fetch did not deliver in time, as horizontal smears that come and go with
/// load.
///
/// Reading the register clears the latch. Re-armed after the read, so each call
/// reports the window since the previous one
pub fn underflow(display_id: u32) -> Result<alloc::string::String, &'static str> {
    let Some(head) = output::with(display_id, |o| o.head).flatten() else {
        return Err("that output is not being driven");
    };
    let Some(h_root) = *super::chanmem::DISP_ROOT.lock() else {
        return Err("no disp root - the display bring-up did not reach it");
    };
    let say = |_: &str| {};
    with_rm(|_bar0, rpc, client| {
        super::modeset::DIAG.lock().underflow = None;
        super::modeset::underflow_prop(rpc, client, h_root, head, false, &say);
        let got = super::modeset::DIAG.lock().underflow.clone();
        // Armed again, or the next call reports a window never watched
        super::modeset::underflow_prop(rpc, client, h_root, head, true, &say);
        got.ok_or("the control did not answer")
    })?
}
