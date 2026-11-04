// GSP-RM coming up: its status queue linked, GSP_INIT answered, and what the
// boot shows of the events on the way
//
// 615 (`dev/gsp-615.md`, section 5; `kgspBootstrap_GH100` after FALCON_OS):
// GSP-RM writes its status queue's header once it runs, and the host links it
// (`GspStatusQueueInit`); then GMCAPI GSP_INIT carries the system information
// in and brings the static information back (`kgspSendInitRpcs_IMPL`). While
// GSP_INIT waits, the bootup events arrive and go to the dispatcher
// (`kgsp::events`); what the boot shows is an observer of that poll
// (`dev/gsp-foundation.md`, decision 12). The poll runs in half-second slices
// so the heartbeat goes on between them: two minutes at most, ten seconds of
// silence at most.

use crate::serial_println;
use crate::drivers::gpu::nvidia::gsp_common::rpc::RpcMessage;

use super::super::gspboot::{phase, BootReport};

/// Everything the boot shows of GSP-RM's events, and remembers about them
struct Narration {
    events_total: u32,
    last_fn: u32,
    event_counts: alloc::vec::Vec<(u32, u32)>,
    nocat_count: u32,
    nocat_seq_first: u32,
    nocat_seq_last: u32,
    /// The note about what NOCAT means has been printed
    nocat_noted: bool,
    /// Distinct assert/diag texts already shown, keeping the screen readable
    /// while surfacing every unique complaint
    seen_texts: alloc::vec::Vec<alloc::string::String>,
    log_verbose: bool,
    xid: Option<u32>,
    first_msg: Option<(u32, u32)>,
}

impl Narration {
    fn new(log_verbose: bool) -> Self {
        Narration {
            events_total: 0,
            last_fn: 0,
            event_counts: alloc::vec::Vec::new(),
            nocat_count: 0,
            nocat_seq_first: 0,
            nocat_seq_last: 0,
            nocat_noted: false,
            seen_texts: alloc::vec::Vec::new(),
            log_verbose,
            xid: None,
            first_msg: None,
        }
    }

    /// One event, after the dispatcher has had it
    fn on_event(&mut self, m: &RpcMessage) {
        use crate::drivers::gpu::nvidia::gsp_common::rpc::{
            ascii_runs, event_name, libos_print_buf, nocat_record, os_error_log,
            EV_GSP_POST_NOCAT_RECORD, EV_MMU_FAULT_QUEUED, EV_OS_ERROR_LOG, EV_RC_TRIGGERED,
            EV_UCODE_LIBOS_PRINT,
        };
        let log_verbose = self.log_verbose;
        self.events_total += 1;
        self.last_fn = m.function;
        serial_println!(
            "[gb206/boot] GSP event {:#x} ({}) len={}",
            m.function, event_name(m.function), m.payload.len()
        );
        if m.function == EV_GSP_POST_NOCAT_RECORD {
            // Diagnostic record. Sequence bookkeeping tells a stuck read
            // pointer (one record re-read forever) from a genuine stream, and
            // the first record goes on screen
            self.nocat_count += 1;
            self.nocat_seq_last = m.sequence;
            if self.nocat_count == 1 {
                self.nocat_seq_first = m.sequence;
            }
            // GSP-RM's report on a display step that went well, posted on every
            // boot (`NocatRecord::display_step`). The serial line below still
            // has it
            let routine = nocat_record(&m.payload)
                .and_then(|r| r.display_step())
                .is_some_and(|(_, status)| status == 0);
            if !routine && !self.nocat_noted {
                self.nocat_noted = true;
                // NOCAT is NOn-CATastrophic: this card can post an ASSERT on a
                // boot and still come up about two seconds later. Said once,
                // before the first record on screen
                crate::println!(
                    "    (NOCAT = non-catastrophic; GSP-RM logs these and continues)"
                );
            }
            let seen_texts = &mut self.seen_texts;
            // Decode the record: the 1 KiB diagBuffer is full of unrelated byte
            // runs (engine-name and panel-part tables), which would read as a
            // dozen unrelated failures
            let texts: alloc::vec::Vec<alloc::string::String> = match nocat_record(&m.payload) {
                Some(r) => {
                    // Dedupe key for a non-assert record: its two strings
                    // plus the error code. Asserts dedupe on their site instead
                    let key = alloc::format!("{}/{}/{:x}", r.source, r.faulting_engine, r.error_code);
                    let fresh = !seen_texts.contains(&key) && seen_texts.len() < 24;
                    if fresh && !r.is_assert() {
                        seen_texts.push(key);
                    }
                    // One line carries the whole record: type, both strings and
                    // the error code. The display takeover can cut the output
                    // off at any line
                    if fresh && !r.is_assert() && !routine {
                        // The diagnostic buffer, not just the code, unless it
                        // decodes (a display step's status). For the other
                        // display records this holds the engine's own detail,
                        // and the code alone has no published meaning
                        let l = crate::drivers::gpu::nvidia::gsp_common::kgsp::events::nocat_line(&r);
                        let l = l.strip_prefix("nocat ").unwrap_or(&l);
                        crate::println!("    {}", l);
                        if log_verbose {
                            crate::pstore_println!("{}", l);
                        }
                    }
                    serial_println!(
                        "[gb206/boot] nocat[{}] type={} src=\"{}\" engine=\"{}\" bugcheck={:#x} subsys={:#x} err={:#x} diaglen={}",
                        self.nocat_count, r.rec_type_name(), r.source,
                        r.faulting_engine, r.bugcheck, r.subsystem,
                        r.error_code, r.diag_len
                    );
                    // An ASSERT record carries the failing NV_ASSERT's source
                    // line and its image offset packed into errorCode, plus a
                    // call stack of image offsets. The strings are just
                    // "ASSERT"
                    if let Some((off, line)) = r.assert_site() {
                        // Every distinct assert gets two lines: the site
                        // identifies the failing NV_ASSERT and the stack says
                        // how GSP-RM got there. Dedupe on the site
                        let key = alloc::format!("A{:x}:{}", off, line);
                        let new_site = !seen_texts.contains(&key);
                        if new_site && seen_texts.len() < 24 {
                            seen_texts.push(key);
                            crate::println!("    ASSERT gsp-rm+{:#x} line {}", off, line);
                            if log_verbose {
                                crate::pstore_println!("ASSERT gsp-rm+{:#x} line {}", off, line);
                            }
                            if let Some((frames, repeats)) = r.assert_call_stack() {
                                if !frames.is_empty() {
                                    let mut s = alloc::string::String::new();
                                    for f in frames.iter().take(6) {
                                        s.push_str(&alloc::format!("{:x} ", f));
                                    }
                                    crate::println!("      stack x{}: {}", repeats, s);
                                }
                            }
                        }
                        serial_println!("[gb206/boot] assert site: gsp-rm+{:#x} line {}", off, line);
                        if let Some((frames, repeats)) = r.assert_call_stack() {
                            serial_println!("[gb206/boot] assert stack x{}: {:x?}", repeats, frames);
                        }
                    }
                    // The strings are on the one-line summary; only extra text
                    // inside a non-binary diag buffer is new
                    if fresh && !r.diag_is_binary() {
                        ascii_runs(&r.diag, 6)
                    } else {
                        alloc::vec::Vec::new()
                    }
                }
                // Undecodable record: fall back to scraping
                None => ascii_runs(&m.payload, 6),
            };
            for t in texts.iter().take(3) {
                if !seen_texts.contains(t) && seen_texts.len() < 24 {
                    crate::println!("      \"{}\"", t);
                    seen_texts.push(t.clone());
                }
            }
        } else if m.function == EV_OS_ERROR_LOG {
            // The Xid: if GSP-RM's init dies, this says why in plain text
            let (xid, chid, text) = os_error_log(&m.payload);
            self.xid = Some(xid);
            crate::println!("      GSP Xid {} (chid {}): {}", xid, chid, text);
            serial_println!("[gb206/boot] Xid:{} chid:{} {}", xid, chid, text);
        } else if m.function == EV_RC_TRIGGERED || m.function == EV_MMU_FAULT_QUEUED {
            crate::println!("      gsp fault: {} ({:#x})", event_name(m.function), m.function);
            serial_println!("[gb206/boot] fault event {:#x} len={}", m.function, m.payload.len());
        } else {
            // Repeating events (LOCKDOWN_NOTICE, LIBOS_PRINT) flood during
            // init, and the display takeover might eat a NOCAT record instead,
            // so each kind shows once
            let n = match self.event_counts.iter_mut().find(|(f, _)| *f == m.function) {
                Some((_, c)) => { *c += 1; *c }
                None => { self.event_counts.push((m.function, 1)); 1 }
            };
            if n == 1 {
                crate::println!("    ev {} {:#x}", event_name(m.function), m.function);
            }
            // libos carries GSP-RM's own boot log; surface text not yet seen,
            // same budget as NOCAT
            if m.function == EV_UCODE_LIBOS_PRINT {
                for t in ascii_runs(libos_print_buf(&m.payload), 8).iter().take(2) {
                    if !self.seen_texts.contains(t) && self.seen_texts.len() < 24 {
                        crate::println!("      libos \"{}\"", t);
                        self.seen_texts.push(t.clone());
                    }
                }
            }
        }
        if self.first_msg.is_none() {
            self.first_msg = Some((m.function, m.sequence));
        }
    }
}

/// Link GSP-RM's status queue, then send GSP_INIT and wait for its answer.
/// Returns the static information it brings back, or `None` if GSP-RM never got
/// that far; the report says how far it got
pub(super) fn gsp_init(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    init_data: &[u8],
    vram_con: &mut Option<crate::drivers::gpu::nvidia::pramin::VramConsole>,
    log_verbose: bool,
    led_heartbeat: bool,
    report: &mut BootReport,
) -> Option<crate::drivers::gpu::nvidia::gsp_common::staticinfo::StaticInfo> {
    use crate::drivers::gpu::nvidia::gsp_common::{gmcapi, kgsp, staticinfo};
    use crate::drivers::gpu::nvidia::gsp_common::rpc::{Context, RpcError};

    phase(7, "linking GSP-RM's queue");
    // GSP-RM's GSP_INIT handling prints into its own log; on screen from inside
    // the drain, within a budget, so a chatty init cannot push the failure off
    // the screen
    kgsp::logs::narrate(if log_verbose { 400 } else { 120 }, log_verbose);
    if !link_status_queue(bar0, rpc, vram_con) {
        report.msgq_initialized = false;
        finish(bar0, rpc, report, None);
        return None;
    }
    report.msgq_initialized = true;

    phase(7, "GSP_INIT");
    let seq = match rpc.send_gmcapi(gmcapi::GSP_INIT, init_data, gmcapi::GSP_INIT_MAX_RESPONSE, 1_000_000_000) {
        Ok(s) => s,
        Err(e) => {
            crate::println!("      GSP_INIT could not be sent: {:?}", e);
            crate::kerror!("[gb206/boot] GSP_INIT send: {:?}", e);
            finish(bar0, rpc, report, None);
            return None;
        }
    };
    serial_println!("[gb206/boot] GSP_INIT sent ({} B, sequence {})", init_data.len(), seq);

    let mut boot_events = rpc.events();
    let mut n = Narration::new(log_verbose);
    let mut answer = None;
    // Budget: up to 240 half-second slices (~2 min of init progress), bail
    // after 20 consecutive slices with no event at all (~10 s of nothing)
    let mut quiet_slices = 0u32;
    crate::println!("      (heartbeat every ~1 s; up to 2 min before giving up)");
    for slice in 0..240 {
        // One compact line so the screen shows whether the wait is alive and
        // GSP-RM still talking. Every ~1 s: the console stops accepting writes
        // within a few seconds of display init, so the last line on screen is
        // the measurement
        if slice % 2 == 1 {
            let hb = alloc::format!(
                "+{}s: {} ev (last {:#x}), {} nocat, quiet {}",
                (slice + 1) / 2, n.events_total, n.last_fn, n.nocat_count, quiet_slices
            );
            crate::println!("      {}", hb);
            // The same line through BAR0: once the BAR1 console goes quiet this
            // copy still reaches the screen
            if let Some(vc) = vram_con.as_mut() {
                vc.line(bar0, &hb);
            }
            if log_verbose {
                crate::pstore_println!("{}", hb);
            }
            // A GSP-RM that has posted hundreds of diagnostics is not on its
            // way to answering
            if n.nocat_count >= 200 {
                crate::println!("      GSP-RM keeps repeating diagnostics - stopping early");
                break;
            }
            if led_heartbeat {
                crate::drivers::input::ps2::set_leds(if slice % 20 == 9 { 0x01 } else { 0x00 });
            }
        }
        let before = n.events_total;
        let got = {
            let mut observe = |m: &RpcMessage, c: Context| {
                // What the event means is the dispatcher's; the narration only
                // shows it
                boot_events(m, c);
                n.on_event(m);
            };
            rpc.recv_poll(gmcapi::GSP_INIT, Some(seq), Context::PollBootup, 500_000_000, &mut observe)
        };
        match got {
            Ok(m) => {
                answer = Some(m);
                break;
            }
            Err(RpcError::RecvTimeout) => {
                if n.events_total != before {
                    quiet_slices = 0;
                } else {
                    quiet_slices += 1;
                    if quiet_slices >= 20 {
                        serial_println!("[gb206/boot] msgq quiet for 10 s, giving up on GSP_INIT");
                        break;
                    }
                }
            }
            Err(e) => {
                // A frame or length error means the ring view desynced from
                // GSP-RM's; a dead GSP-RM (the health check) ends it too. Stop
                // and say so on screen
                crate::println!("      msgq stream broke: {:?}", e);
                crate::kerror!("[gb206/boot] handshake recv error: {:?}", e);
                break;
            }
        }
    }

    let info = answer.and_then(|m| {
        let data = match gmcapi::response_data(m, gmcapi::GSP_INIT_MAX_RESPONSE) {
            Ok(d) => d,
            Err(gmcapi::GmcError::Status(s)) => {
                let s = crate::drivers::gpu::nvidia::gsp_common::rmapi::status_str(s);
                crate::println!("      GSP_INIT refused: {}", s);
                crate::kerror!("[gb206/boot] GSP_INIT refused: {}", s);
                return None;
            }
            Err(gmcapi::GmcError::TooLong(n)) => {
                crate::println!("      GSP_INIT answered {} B, more than the {} it may", n, gmcapi::GSP_INIT_MAX_RESPONSE);
                crate::kerror!("[gb206/boot] GSP_INIT answer too long: {} B", n);
                return None;
            }
        };
        match staticinfo::decode(&data) {
            Ok(si) => {
                serial_println!(
                    "[gb206/boot] GSP_INIT answered: {} B, {} keys ({} not read here), \"{}\", RPC version {:#x}",
                    data.len(), si.keys, si.skipped, si.gpu_name, si.vgx_version
                );
                Some(si)
            }
            Err(e) => {
                crate::println!("      GSP_INIT answer unreadable: {:?}", e);
                crate::kerror!("[gb206/boot] GSP_INIT static info: {:?}", e);
                None
            }
        }
    });
    report.init_done = info.is_some();
    finish(bar0, rpc, report, Some(&n));
    info
}

/// Wait for GSP-RM's status-queue header and take the ring
/// (`GspStatusQueueInit`: 4 s there; ten here, with a line a second, since this
/// card can take several seconds)
fn link_status_queue(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    vram_con: &mut Option<crate::drivers::gpu::nvidia::pramin::VramConsole>,
) -> bool {
    use crate::drivers::gpu::nvidia::gsp_common::kgsp;
    use crate::drivers::gpu::nvidia::gsp_common::rpc::LinkError;
    let t0 = crate::kcore::time::rdtsc();
    let mut shown = 0;
    loop {
        match rpc.rx_link() {
            Ok(rx) => {
                serial_println!(
                    "[gb206/boot] status queue linked: {} slots of {} B at +{:#x}",
                    rx.msg_count, rx.msg_size, rx.entry_off
                );
                return true;
            }
            Err(LinkError::Mismatch { major, size, msg_size, msg_count, entry_off }) => {
                crate::println!(
                    "      GSP-RM's queue header disagrees: v{} size {:#x} slot {} x{} at +{:#x}",
                    major, size, msg_size, msg_count, entry_off
                );
                crate::kerror!("[gb206/boot] status queue header mismatch: v{} size {:#x} msg {} count {} entry {:#x}",
                    major, size, msg_size, msg_count, entry_off);
                return false;
            }
            Err(LinkError::NotYet) => {}
        }
        rpc.dump_logs();
        if !kgsp::health::check(Some(bar0)) {
            crate::println!("      GSP-RM died before it linked its queue");
            return false;
        }
        let ms = crate::drivers::gpu::nvidia::gsp_common::rpc::elapsed_ms(t0);
        if ms / 1000 > shown {
            shown = ms / 1000;
            let hb = alloc::format!("+{}s: waiting for GSP-RM's queue", shown);
            crate::println!("      {}", hb);
            if let Some(vc) = vram_con.as_mut() {
                vc.line(bar0, &hb);
            }
        }
        if ms >= 10_000 {
            crate::println!("      GSP-RM never wrote its queue header");
            crate::kerror!("[gb206/boot] status queue not linked after 10 s");
            return false;
        }
        core::hint::spin_loop();
    }
}

/// What the handshake leaves behind, however it ended
fn finish(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    report: &mut BootReport,
    n: Option<&Narration>,
) {
    use crate::drivers::gpu::nvidia::gsp_common::kgsp;
    // Last sweep: whatever GSP-RM wrote after the final record, with a few
    // lines of screen kept back for it
    kgsp::logs::narrate(40, false);
    rpc.dump_logs();
    kgsp::logs::narrate_off();
    if let Some(n) = n {
        if let Some(x) = n.xid {
            report.xid = Some(x);
        }
        if report.gsp_first_msg.is_none() {
            report.gsp_first_msg = n.first_msg;
        }
        if n.nocat_count > 0 {
            crate::println!(
                "      ({} NOCAT records, rpc seq {}..{}; try nvidia gb206-log for the GSP log)",
                n.nocat_count, n.nocat_seq_first, n.nocat_seq_last
            );
        }
    }
    {
        // The narration has shown every record the journal holds, so it is
        // emptied here, not reported again by the event thread
        let _ = kgsp::events::take_journal();
        let (unexpected, refused) = kgsp::events::oddities();
        let line = alloc::format!(
            "GSP: boot events {}; lockdown {}; {} unexpected, {} refused at boot, {} stale record(s)",
            kgsp::events::summary(),
            if kgsp::in_lockdown() { "STILL ENGAGED" } else { "released" },
            unexpected, refused,
            crate::drivers::gpu::nvidia::gsp_common::rpc::stale_records()
        );
        serial_println!("[gb206/boot] {}", line);
        crate::pstore_println!("{}", line);
    }
    // No answer: one last look at the crash queue ("a last attempt to report
    // boot issues that might have escaped prior checks", `kgspInitRm_IMPL`)
    if !report.init_done {
        let _ = kgsp::health::check(Some(bar0));
        let (reports, last) = kgsp::crashcat::last();
        if reports > 0 {
            for l in last.iter().take(4) {
                crate::println!("      {}", l);
            }
        }
    }
}
