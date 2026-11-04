// What an event from GSP-RM means: one table, whoever read it
//
// `_kgspProcessRpcEvent` (`kernel_gsp.c:1408`), cut down to what a GB20x with
// no vGPU, NVLink or MIG can send. Every event read comes here: `recv_poll` on
// the way to a reply, the worker when nothing is waited for, the boot handshake
// while GSP-RM comes up.
//
// A handler only writes things down: it runs inside the receive path, and an
// RPC from there would re-enter the transport mid-drain
// (`rpc::RpcError::NestedPoll`). What needs a register read or an RM call goes
// in the journal, read by whoever drained once outside the transport
// (`control::poll_events`).

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::gsp_common::output;
use crate::drivers::gpu::nvidia::gsp_common::rpc::{self, Context, RpcMessage};
use crate::drivers::gpu::nvidia::le;
use crate::kcore::klog::{self, Level, Site};
use crate::serial_println;

/// How much of one kind of trouble reaches the UART and the log before only a
/// sample does. A channel faulting in a loop posts reports faster than a
/// 115200-baud line carries them, from inside the receive path, and copies of
/// one report would push older errors out of the errors ring. `n` is this
/// report's number, from one
const TROUBLE_WIRE: u32 = 8;
const TROUBLE_LOG: u32 = 256;
const TROUBLE_SAMPLE: u32 = 100;

fn trouble(level: Level, site: Site, n: u32, args: core::fmt::Arguments) {
    if n <= TROUBLE_WIRE {
        klog::emit(level, site, args);
    } else if n <= TROUBLE_LOG || n % TROUBLE_SAMPLE == 0 {
        klog::quiet(level, site, args);
    }
}

/// The events OpenRM screens as safe while GSP-RM boots
/// (`_kgspProcessRpcEvent`, 615). OpenRM asserts on and skips anything else;
/// this counts and skips it
const BOOTUP_ALLOWED: [u32; 5] = [
    rpc::EV_UCODE_LIBOS_PRINT,
    rpc::EV_GSP_LOCKDOWN_NOTICE,
    rpc::EV_GSP_POST_NOCAT_RECORD,
    rpc::EV_OS_ERROR_LOG,
    rpc::EV_PFM_REQ_HNDLR_STATE_SYNC_CALLBACK,
];

const EVENTS: usize = (rpc::EV_NUM_EVENTS - rpc::EV_FIRST_EVENT) as usize;

/// How many of each event arrived since boot, `EV_FIRST_EVENT` onward
static SEEN: [AtomicU32; EVENTS] = [const { AtomicU32::new(0) }; EVENTS];
/// Messages that were not an event, or an event nothing here handles: OpenRM's
/// "Unexpected RPC event", also what a reply after its RPC timed out looks like
static UNEXPECTED: AtomicU32 = AtomicU32::new(0);
/// Events that arrived during boot and were not on `BOOTUP_ALLOWED`
static BOOT_REFUSED: AtomicU32 = AtomicU32::new(0);

/// Robust-channel reports and MMU fault notifications since boot, kept in full
/// even when the journal drops the records
static RC_TOTAL: AtomicU32 = AtomicU32::new(0);
static FAULT_TOTAL: AtomicU32 = AtomicU32::new(0);
/// Xids and NOCAT records since boot, for the log's rate limit
static XID_TOTAL: AtomicU32 = AtomicU32::new(0);
static NOCAT_TOTAL: AtomicU32 = AtomicU32::new(0);

/// What happened that needs a register read or an RM call to follow up on.
///
/// Bounded: a channel faulting in a loop posts robust-channel reports faster
/// than anything consumes them. The count is kept in full, the records are not
#[derive(Default)]
pub struct Journal {
    pub rc: Vec<rpc::RcRecord>,
    /// Reports past `RC_KEEP` that were counted and dropped
    pub rc_dropped: u32,
    /// `MMU_FAULT_QUEUED`: the fault buffer has entries
    pub mmu_queued: u32,
    /// GSP-RM's own journal records, decoded, one line each
    pub nocat: Vec<String>,
    /// `(exceptType, chid, text)` from `OS_ERROR_LOG`: the Xid
    pub xid: Vec<(u32, u32, String)>,
}

impl Journal {
    pub fn is_empty(&self) -> bool {
        self.rc.is_empty() && self.mmu_queued == 0 && self.nocat.is_empty() && self.xid.is_empty()
    }
}

/// How many of each kind to keep between two reads of the journal. More than a
/// couple is a storm, and the first says what happened
const RC_KEEP: usize = 4;
const NOCAT_KEEP: usize = 4;
const XID_KEEP: usize = 4;

static JOURNAL: spin::Mutex<Journal> = spin::Mutex::new(Journal {
    rc: Vec::new(),
    rc_dropped: 0,
    mmu_queued: 0,
    nocat: Vec::new(),
    xid: Vec::new(),
});

/// Everything written down since the last call, and a clean journal after it
pub fn take_journal() -> Journal {
    core::mem::take(&mut *JOURNAL.lock())
}

/// `(robust-channel reports, MMU fault notifications)` since boot
pub fn trouble_counts() -> (u32, u32) {
    (RC_TOTAL.load(Relaxed), FAULT_TOTAL.load(Relaxed))
}

/// How many times `function` has arrived since boot
pub fn seen(function: u32) -> u32 {
    function
        .checked_sub(rpc::EV_FIRST_EVENT)
        .and_then(|i| SEEN.get(i as usize))
        .map_or(0, |c| c.load(Relaxed))
}

/// `(unexpected, refused during boot)`
pub fn oddities() -> (u32, u32) {
    (UNEXPECTED.load(Relaxed), BOOT_REFUSED.load(Relaxed))
}

/// Every event that has arrived, as `NAME xN`, for a log line
pub fn summary() -> String {
    let mut out = String::new();
    for (i, c) in SEEN.iter().enumerate() {
        let n = c.load(Relaxed);
        if n != 0 {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&alloc::format!(
                "{} x{}", rpc::event_name(rpc::EV_FIRST_EVENT + i as u32), n
            ));
        }
    }
    out
}

/// Handle one event (`_kgspProcessRpcEvent`)
pub fn dispatch(msg: &RpcMessage, ctx: Context) {
    // A GMCAPI command from GSP-RM: Turing's and Ampere's bootloader requests
    // and the resume notice, none of which a GB20x running normally sends
    // (`_kgspRpcDrainOneEvent`, the GMCAPI half)
    if msg.nvdm == rpc::NVDM_GMCAPI {
        UNEXPECTED.fetch_add(1, Relaxed);
        crate::kwarn!("[gsp] GMCAPI {} ({:#x}, {} B){} - not handled here",
            super::super::gmcapi::name(msg.function), msg.function, msg.payload.len(),
            if msg.response { ", a response nobody waited for" } else { "" });
        return;
    }
    let first = count(msg.function);
    // What was in flight when it was read: GSP-RM posts an assert or a
    // robust-channel report about the request it handles
    let during = rpc::read_during(ctx);
    crate::kdebug!("[gsp] event {} ({} B), {}", rpc::event_name(msg.function), msg.payload.len(), during);

    if ctx == Context::PollBootup && !BOOTUP_ALLOWED.contains(&msg.function) {
        BOOT_REFUSED.fetch_add(1, Relaxed);
        crate::kwarn!(
            "[gsp] {:#x} ({}) during boot - not handled, as OpenRM does not",
            msg.function, rpc::event_name(msg.function)
        );
        return;
    }

    match msg.function {
        rpc::EV_POST_EVENT => post_event(&msg.payload),
        rpc::EV_RC_TRIGGERED => {
            let n = RC_TOTAL.fetch_add(1, Relaxed) + 1;
            match rpc::RcRecord::parse(&msg.payload) {
                Some(rc) => trouble(Level::Error, crate::klog_site!(), n,
                    format_args!("[gsp] robust-channel report #{}: {} ({}), {}", n, rc.describe(),
                        super::super::chan::owner_of(rc.chid).describe(), during)),
                None => trouble(Level::Error, crate::klog_site!(), n,
                    format_args!("[gsp] robust-channel report #{} ({} B, too short to read), {}",
                        n, msg.payload.len(), during)),
            }
            if let Some(rc) = rpc::RcRecord::parse(&msg.payload) {
                let mut j = JOURNAL.lock();
                if j.rc.len() < RC_KEEP {
                    j.rc.push(rc);
                } else {
                    j.rc_dropped += 1;
                }
            }
        }
        // The addresses are in hardware, so only "the fault buffer is worth
        // reading" is recorded
        rpc::EV_MMU_FAULT_QUEUED => {
            let n = FAULT_TOTAL.fetch_add(1, Relaxed) + 1;
            trouble(Level::Warn, crate::klog_site!(), n,
                format_args!("[gsp] MMU fault #{} queued - the fault buffer has entries, {}", n, during));
            JOURNAL.lock().mmu_queued += 1;
        }
        // The Xid: GSP-RM's plain text, naming the channel
        rpc::EV_OS_ERROR_LOG => {
            let (xid, chid, text) = rpc::os_error_log(&msg.payload);
            let n = XID_TOTAL.fetch_add(1, Relaxed) + 1;
            trouble(Level::Error, crate::klog_site!(), n,
                format_args!("[gsp] Xid {} on channel {} ({}): {}, {}", xid, chid,
                    super::super::chan::owner_of(chid).describe(), text, during));
            let mut j = JOURNAL.lock();
            if j.xid.len() < XID_KEEP {
                j.xid.push((xid, chid, text));
            }
        }
        // GSP-RM's own log line. At boot the handshake shows these itself
        rpc::EV_UCODE_LIBOS_PRINT => {
            if ctx != Context::PollBootup {
                for t in rpc::ascii_runs(rpc::libos_print_buf(&msg.payload), 8).iter().take(2) {
                    serial_println!("[gsp] libos \"{}\"", t);
                }
            }
        }
        rpc::EV_GSP_LOCKDOWN_NOTICE => {
            // `rpc_gsp_lockdown_notice_v17_00`: one NvBool, bLockdownEngaging
            super::set_lockdown(msg.payload.first().is_some_and(|b| *b != 0));
        }
        rpc::EV_GSP_POST_NOCAT_RECORD => {
            let Some(r) = rpc::nocat_record(&msg.payload) else { return };
            let line = nocat_line(&r);
            // The log's line also carries where the assert was reached from and
            // during what; the journal keeps the short one, for the screen and
            // for spotting a repeat
            let stack = nocat_stack(&r);
            // A display step that went well is GSP-RM keeping its journal, and
            // OpenRM prints no NOCAT record (`rcdbNocatInsertNocatError` only
            // stores it). Logged, not shown
            let routine = r.display_step().is_some_and(|(_, status)| status == 0);
            // At boot these are the handshake's to show
            if ctx == Context::PollBootup {
                crate::kdebug!("[gsp] {}{}, {}", line, stack, during);
            } else if routine {
                crate::kinfo!("[gsp] {}, {}", line, during);
            } else {
                let n = NOCAT_TOTAL.fetch_add(1, Relaxed) + 1;
                trouble(Level::Warn, crate::klog_site!(), n, format_args!("[gsp] {}{}, {}", line, stack, during));
            }
            if routine {
                return;
            }
            let mut j = JOURNAL.lock();
            // A burst repeats one record
            if j.nocat.len() < NOCAT_KEEP && !j.nocat.contains(&line) {
                j.nocat.push(line);
            }
        }
        // Counted and named. Each has a handler in OpenRM acting on something
        // this driver lacks: FECS logging, a recovery policy, GSP tracing,
        // semaphore callbacks, boost-sync limits, and the vGPU plugin's engine
        // samples (`_kgspRpcGpuacctPerfmonUtilSamples`; the load comes from the
        // shared page).
        //
        // GSP-RM giving up on itself or on the PMU: both need a reset, and the
        // health check finds the crash report saying why
        // (`_kgspRpcGspForcedDriverShutdown`, `_kgspRpcGspEventPmuHalted`)
        rpc::EV_FORCED_DRIVER_SHUTDOWN | rpc::EV_PMU_HALTED | rpc::EV_TRIGGER_BUGCHECK => {
            crate::kerror!("[gsp] {} ({} B), {}", rpc::event_name(msg.function), msg.payload.len(), during);
        }
        rpc::EV_PFM_REQ_HNDLR_STATE_SYNC_CALLBACK
        | rpc::EV_FECS_ERROR
        | rpc::EV_RECOVERY_ACTION
        | rpc::EV_UPDATE_GSP_TRACE
        | rpc::EV_SEMAPHORE_SCHEDULE_CALLBACK
        | rpc::EV_TIMED_SEMAPHORE_RELEASE
        | rpc::EV_PERF_GPU_BOOST_SYNC_LIMITS_CALLBACK
        | rpc::EV_PERF_BRIDGELESS_INFO_UPDATE
        | rpc::EV_GPUACCT_PERFMON_UTIL_SAMPLES => {
            // FECS errors and recovery actions are GSP-RM saying something went
            // wrong; the rest are news
            let bad = matches!(msg.function, rpc::EV_FECS_ERROR | rpc::EV_RECOVERY_ACTION);
            if first {
                let lv = if bad { Level::Warn } else { Level::Info };
                klog::emit(lv, crate::klog_site!(), format_args!("[gsp] {} ({} B) - counted, not acted on",
                    rpc::event_name(msg.function), msg.payload.len()));
            } else if bad {
                crate::kdebug!("[gsp] {} again ({} B)", rpc::event_name(msg.function), msg.payload.len());
            }
        }
        _ => {
            UNEXPECTED.fetch_add(1, Relaxed);
            crate::kwarn!("[gsp] unexpected message {:#x} ({}) result {:#x} len {}",
                msg.function, rpc::event_name(msg.function), msg.rpc_result, msg.payload.len());
        }
    }
}

/// Count one arrival; true for the first of its kind
fn count(function: u32) -> bool {
    match function.checked_sub(rpc::EV_FIRST_EVENT).and_then(|i| SEEN.get(i as usize)) {
        Some(c) => c.fetch_add(1, Relaxed) == 0,
        None => false,
    }
}

/// An RM notification (`_kgspRpcPostEvent`). Two mean something here: hot-plug,
/// written down for `control` to act on outside the transport, and the sink's
/// DP interrupt, only reported
fn post_event(payload: &[u8]) {
    let Some(e) = rpc::PostEvent::parse(payload) else { return };
    match e.notify_index {
        output::NOTIFIER_HOTPLUG if e.event_data.len() >= 8 => {
            // `Nv2080HotplugNotification {plugDisplayMask, unplugDisplayMask}`,
            // both: a monitor swapped arrives as one notification with an
            // unplug and a plug
            let (plug, unplug) = (le::u32_or0(e.event_data, 0), le::u32_or0(e.event_data, 4));
            crate::kdebug!("[nvidia] hot-plug notification: plugged {:#x}, unplugged {:#x}", plug, unplug);
            output::note_hotplug(plug, unplug);
        }
        output::NOTIFIER_DP_IRQ if e.event_data.len() >= 4 => {
            // `Nv2080DpIrqNotification {displayId}`. The sink raised its DPCD
            // interrupt vector, a short pulse. Queued: `control::poll_events`
            // reads what it means over AUX and acts, outside the transport
            // (`ConnectorImpl::notifyShortPulse`)
            let id = le::u32_or0(e.event_data, 0);
            crate::knotice!("[nvidia] DP IRQ from {:#010x} - sink asked for attention", id);
            output::note_dp_irq(id);
        }
        _ => crate::kdebug!("[gsp] POST_EVENT notifier {} ({} B of data) - nothing here listens",
            e.notify_index, e.event_data.len()),
    }
}

/// An ASSERT record's call stack, as `, stack gsp-rm+0x.. 0x.. (xN)`: image
/// offsets from the record's diagnostic buffer (`RM_NOCAT_ASSERT_DIAG_BUFFER`),
/// innermost first, and how many times the assert has fired. Empty for any
/// other record, or one with no stack
fn nocat_stack(r: &rpc::NocatRecord) -> String {
    let Some((frames, count)) = r.assert_call_stack() else {
        return String::new();
    };
    if frames.is_empty() {
        return String::new();
    }
    let mut s = String::from(", stack gsp-rm");
    for f in &frames {
        s.push_str(&alloc::format!(" +{:#x}", f));
    }
    if count > 1 {
        s.push_str(&alloc::format!(" (x{})", count));
    }
    s
}

/// One line for a NOCAT record: the assert site for an ASSERT; the step and its
/// status for GSP-RM's display step report (`NocatRecord::display_step`); for
/// anything else its type, both strings, the codes and the start of the
/// diagnostic buffer, where a display record keeps the engine's detail
pub fn nocat_line(r: &rpc::NocatRecord) -> String {
    if let Some((off, line)) = r.assert_site() {
        return alloc::format!("nocat ASSERT gsp-rm+{:#x} line {}", off, line);
    }
    match r.display_step() {
        Some((step, status)) => alloc::format!(
            "nocat display step {:#x}: {}", step, super::super::rmapi::status_str(status)),
        None => {
            let mut diag = String::new();
            for b in r.diag.iter().take(24) {
                diag.push_str(&alloc::format!("{:02x}", b));
            }
            alloc::format!(
                "nocat {} \"{}\"/\"{}\" err {:#x} sub {:#x} d{} {}",
                r.rec_type_name(), r.source, r.faulting_engine,
                r.error_code, r.subsystem, r.diag_len, diag
            )
        }
    }
}
