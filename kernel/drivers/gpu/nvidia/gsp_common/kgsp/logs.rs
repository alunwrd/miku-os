// GSP-RM's own log, read while it runs
//
// GSP-RM writes token records into three libos buffers (INIT, INTR, RM) in the
// boot args; `logdecode` turns them into text with the logging ELF.
//
// OpenRM reads them continuously (`kgspDumpGspLogs_IMPL`, `kernel_gsp.c:3958`
// -> `libosExtractLogs`, `liblogdecode.c:1940`): after every event it drains
// (`_kgspRpcDrainEvents`, `:1778`), from a 1 Hz callback, and after the
// unload's suspend wait (`:3857`). Here the drain is the one hook
// (`rpc::poll`), and the event thread drains at least every 200 ms. The unload
// calls `dump` itself.
//
// Kept: the put pointer per buffer, so each record prints once, and a count per
// level.
//
// Every record goes to the kernel log at its own level (INFO as DEBUG, as
// OpenRM does not print INFO; NOTICE, WARNING, and ERROR for the three kinds of
// error), so `nvidia log fw` has all and `nvidia log err` its warnings. The
// UART gets WARNING and up: a serial line costs ~9 ms with interrupts off
// (115200 baud), inside an RPC. The rest go out under `trace`, and the boot
// handshake asks for everything.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::kcore::firmware::load::Firmware;
use crate::kcore::klog::{self, Level};

use super::super::logdecode::{decode_buffer, level_name, LogElf, LogRecord};

/// The buffers in the order `QueueMem::log_buffers` gives them
pub const TAGS: [&str; 3] = ["INIT", "INTR", "RM"];

/// `LEVEL_WARNING` (`nvprintf_level.h:36`): the first level worth a serial line
/// without a trace
const WARNING: u8 = 3;
/// `NV_LEVEL_MAX` = `LEVEL_FATAL`. A record claiming more is clamped, as OpenRM
/// does (`liblogdecode.c:321`)
const LEVEL_MAX: u8 = 6;

/// Records one pass decodes per buffer. OpenRM's scratch buffer has the same
/// kind of bound and says when it overflows (`liblogdecode.c:1317`)
const PER_PASS: usize = 256;
/// Warnings and errors `summary` hands `gb206-log`, from the kernel log
const KEEP: usize = 16;

/// GSP-RM's level as the kernel log's
fn klog_level(level: u8) -> Level {
    match level {
        0 | 1 => Level::Debug,
        2 => Level::Notice,
        3 => Level::Warn,
        _ => Level::Error,
    }
}

struct Installed {
    fw: Firmware,
    /// Per buffer, the put pointer already printed up to
    put: [u64; 3],
}

/// The logging ELF and where each buffer was left. `None` without a logging ELF
/// (`pLogElf == NULL`), and nothing here works then.
///
/// Taken only with `try_lock`: OpenRM's `logDumpLock` is a try-lock giving up
/// when contended at raised IRQL, and a dump giving up loses nothing, the next
/// starting from the same put pointers
static STATE: spin::Mutex<Option<Installed>> = spin::Mutex::new(None);

/// Records seen since the logging ELF was installed, by level 0..=6
static LEVELS: [AtomicU32; LEVEL_MAX as usize + 1] = [const { AtomicU32::new(0) }; 7];
/// Passes that hit `PER_PASS`, and so may have skipped records
static OVERFLOWS: AtomicU32 = AtomicU32::new(0);

/// Hand over the logging ELF for a GSP-RM about to boot. Its buffers are new,
/// so every one is read from the start
pub fn install(fw: Firmware) {
    *STATE.lock() = Some(Installed { fw, put: [0; 3] });
    for l in &LEVELS {
        l.store(0, Ordering::Relaxed);
    }
    OVERFLOWS.store(0, Ordering::Relaxed);
}

/// GSP-RM is gone and its buffers with it. The counts stay, and its lines are
/// in the kernel log, so `gb206-log` can say what the last one complained about
pub fn forget() {
    *STATE.lock() = None;
}

fn installed() -> bool {
    STATE.lock().is_some()
}

/// The boot handshake's screen: while GSP-RM boots, the records the drain reads
/// also go on the console (WARNING and worse, everything with `log=1`) within a
/// line budget, and INFO/NOTICE to serial as well. Set around the wait for
/// GSP_INIT, so the narration rides the one drain (`recv_poll`)
struct Narration {
    budget: u32,
    verbose: bool,
}

static NARRATION: spin::Mutex<Option<Narration>> = spin::Mutex::new(None);

/// Start narrating with `budget` screen lines, or top an ongoing narration up
/// to at least `budget`
pub fn narrate(budget: u32, verbose: bool) {
    let mut n = NARRATION.lock();
    match n.as_mut() {
        Some(x) => x.budget = x.budget.max(budget),
        None => *n = Some(Narration { budget, verbose }),
    }
}

/// Stop narrating
pub fn narrate_off() {
    NARRATION.lock().take();
}

/// Print what is new in the buffers (`kgspDumpGspLogs`). Cheap when nothing is:
/// three put pointers compared
pub fn dump(buffers: [&[u8]; 3]) {
    let Some(mut guard) = STATE.try_lock() else { return };
    let Some(st) = guard.as_mut() else { return };
    let Some(elf) = LogElf::parse(st.fw.bytes()) else { return };

    let mut records: Vec<(usize, LogRecord)> = Vec::new();
    for (idx, buf) in buffers.iter().enumerate() {
        let (got, _status, put) = decode_buffer(&elf, buf, st.put[idx], PER_PASS);
        st.put[idx] = put;
        if got.len() == PER_PASS {
            OVERFLOWS.fetch_add(1, Ordering::Relaxed);
            crate::kwarn!("[gsp-rm/{}] **** more than {} records since the last look; older ones lost ****",
                TAGS[idx], PER_PASS);
        }
        records.extend(got.into_iter().map(|r| (idx, r)));
    }
    drop(guard);
    if records.is_empty() {
        return;
    }
    // One timeline across the three buffers, as `libosExtractLogs` merges them.
    // Each buffer is in order, and the sort is stable
    records.sort_by_key(|(_, r)| r.timestamp);

    let mut narration = NARRATION.lock();
    let serial_all = crate::arch::x86_64::serial::tracing() || narration.is_some();
    for (idx, r) in &records {
        let level = r.level.min(LEVEL_MAX);
        LEVELS[level as usize].fetch_add(1, Ordering::Relaxed);
        let lvl = klog_level(level);
        let site = crate::klog_site!();
        if serial_all || level >= WARNING {
            klog::emit(lvl, site, format_args!("[gsp-rm/{}] {}({}) [{}] {}",
                TAGS[*idx], r.file, r.line, level_name(level), r.text));
        } else {
            klog::quiet(lvl, site, format_args!("[gsp-rm/{}] {}({}) [{}] {}",
                TAGS[*idx], r.file, r.line, level_name(level), r.text));
        }
        // Screen budget: an init logging thousands of INFO lines would push the
        // one ERROR line off the top
        if let Some(n) = narration.as_mut() {
            if (n.verbose || level >= WARNING) && n.budget > 0 {
                n.budget -= 1;
                crate::println!("    gsp {}({}) {}", r.file, r.line, r.text);
            }
        }
    }
}

/// What the log has said since the logging ELF was installed
pub struct Summary {
    pub installed: bool,
    /// By level: INFO, NOTICE, WARNING, ERROR and worse
    pub info: u32,
    pub notice: u32,
    pub warning: u32,
    pub error: u32,
    pub overflows: u32,
    /// The last warnings and errors, oldest first
    pub kept: Vec<String>,
}

pub fn summary() -> Summary {
    let n = |l: usize| LEVELS[l].load(Ordering::Relaxed);
    Summary {
        installed: installed(),
        info: n(0) + n(1),
        notice: n(2),
        warning: n(3),
        error: n(4) + n(5) + n(6),
        overflows: OVERFLOWS.load(Ordering::Relaxed),
        kept: kept(),
    }
}

/// The last warnings and errors GSP-RM logged, from the kernel log's errors
/// ring, however much INFO came after
fn kept() -> Vec<String> {
    let f = klog::view::Filter {
        least: Level::Warn,
        modules: alloc::vec![String::from(module_path!())],
        ..Default::default()
    };
    let lines = klog::view::collect(&f);
    let skip = lines.len().saturating_sub(KEEP);
    lines.into_iter().skip(skip).map(|l| l.text).collect()
}
