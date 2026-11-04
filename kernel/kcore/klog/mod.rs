// klog: the kernel's log, in memory (`dev/klog.md`)
//
// Every line the kernel prints is a record here as well as on the UART: a level
// set where it was written, the module and line it was written from, when, on
// which CPU, for which task. Three rings by level, so an error is pushed out
// only by later errors; one lock over all three, so a reader sees one moment.
// Static, and a writer never allocates: the log exists from the first line of
// boot, and an interrupt handler may write to it.
//
//   ring.rs  one ring of records over a byte slice (host-tested)
//   line.rs  print fragments into lines, per CPU
//   view.rs  filters, the one line format, the one viewer
//   selftest.rs  what only the kernel can check (`dmesg selftest`)
//
// The caller decides what reaches the UART: `serial_println!` goes down the
// wire; `kdebug!` and `quiet` do not, unless the boot line says `trace`.

pub mod line;
pub mod ring;
pub mod selftest;
pub mod view;

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

use x86_64::instructions::interrupts;

/// How much a line matters. Lower is worse
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Notice = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    pub const ALL: [Level; 6] = [Level::Error, Level::Warn, Level::Notice, Level::Info, Level::Debug, Level::Trace];

    pub fn from_u8(v: u8) -> Level {
        Level::ALL[(v as usize).min(5)]
    }

    pub fn letter(self) -> char {
        ['E', 'W', 'N', 'I', 'D', 'T'][self as usize]
    }

    pub fn name(self) -> &'static str {
        ["error", "warning", "notice", "info", "debug", "trace"][self as usize]
    }

    /// The words a person types for a level
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "err" | "error" | "errors" => Level::Error,
            "warn" | "warning" | "warnings" => Level::Warn,
            "notice" => Level::Notice,
            "info" => Level::Info,
            "debug" => Level::Debug,
            "trace" | "all" => Level::Trace,
            _ => return None,
        })
    }

    /// Which ring keeps it: errors and warnings, notices and info, the rest
    fn ring(self) -> usize {
        match self {
            Level::Error | Level::Warn => 0,
            Level::Notice | Level::Info => 1,
            Level::Debug | Level::Trace => 2,
        }
    }
}

/// Where a line was written: `module_path!()` and `line!()` of the call site
#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub module: &'static str,
    pub line: u32,
}

impl Site {
    /// For the few callers that reach the print functions directly
    pub const UNKNOWN: Site = Site { module: "?", line: 0 };
    /// A line a ring-3 program wrote with `klog` (75)
    pub const USER: Site = Site { module: "user", line: 0 };
    /// What `unrecorded` speaks as: a line neither stored nor mirrored to the
    /// UART
    pub const UNRECORDED: Site = Site { module: "(unrecorded)", line: 0 };
}

/// The call site, for the macros
#[macro_export]
macro_rules! klog_site {
    () => {
        $crate::kcore::klog::Site { module: module_path!(), line: line!() }
    };
}

pub mod flags {
    /// A console line mirrored to the log: output, not a message
    pub const ECHO: u8 = 1 << 0;
    /// The text was longer than a record holds
    pub const TRUNC: u8 = 1 << 1;
    /// Written by a ring-3 program through `klog` (75)
    pub const USER: u8 = 1 << 2;
    /// `line.rs`'s own: a line is being assembled. Never stored
    pub(super) const STARTED: u8 = 1 << 7;
}

/// Ring sizes. Errors are few and each one matters; the other two are what a
/// boot and a few minutes of running say
const ERR_BYTES: usize = 256 << 10;
const MAIN_BYTES: usize = 1 << 20;
const DEBUG_BYTES: usize = 1 << 20;
pub const RINGS: usize = 3;
pub const RING_NAMES: [&str; RINGS] = ["errors", "main", "debug"];

struct Buf<const N: usize> {
    st: ring::State,
    b: [u8; N],
}

struct Store {
    next_seq: u64,
    err: Buf<ERR_BYTES>,
    main: Buf<MAIN_BYTES>,
    debug: Buf<DEBUG_BYTES>,
}

impl Store {
    fn ring(&mut self, i: usize) -> (&mut ring::State, &mut [u8]) {
        match i {
            0 => (&mut self.err.st, &mut self.err.b[..]),
            1 => (&mut self.main.st, &mut self.main.b[..]),
            _ => (&mut self.debug.st, &mut self.debug.b[..]),
        }
    }

    fn ring_ref(&self, i: usize) -> (&ring::State, &[u8]) {
        match i {
            0 => (&self.err.st, &self.err.b[..]),
            1 => (&self.main.st, &self.main.b[..]),
            _ => (&self.debug.st, &self.debug.b[..]),
        }
    }
}

// All zero, so all of it is .bss
static STORE: spin::Mutex<Store> = spin::Mutex::new(Store {
    next_seq: 0,
    err: Buf { st: ring::State::EMPTY, b: [0; ERR_BYTES] },
    main: Buf { st: ring::State::EMPTY, b: [0; MAIN_BYTES] },
    debug: Buf { st: ring::State::EMPTY, b: [0; DEBUG_BYTES] },
});

/// Lines written at each level since boot, kept or not
static COUNTS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
/// Lines that could not be stored because the lock never came free
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Lines longer than a record, stored cut
static TRUNCATED: AtomicU64 = AtomicU64::new(0);

/// How long a writer waits for the lock before giving the line up. A reader
/// holds it while copying a ring, well under a millisecond; this is tens of
/// milliseconds. For the panic path: a CPU halted by the panic holding the lock
/// never releases it, and the panic message must still get out
const LOCK_SPINS: u32 = 2_000_000;

/// Run `f` on the store with interrupts off, or give up and count it
fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> Option<R> {
    interrupts::without_interrupts(|| {
        let mut spins = 0u32;
        loop {
            if let Some(mut g) = STORE.try_lock() {
                return Some(f(&mut g));
            }
            spins += 1;
            if spins >= LOCK_SPINS {
                DROPPED.fetch_add(1, Relaxed);
                return None;
            }
            core::hint::spin_loop();
        }
    })
}

/// This CPU's index, once it has its per-CPU block. Read from the MSR: before
/// `install_gs_base` a `gs:` load reads address zero
pub(crate) fn this_cpu() -> Option<usize> {
    let gs = x86_64::registers::model_specific::GsBase::read().as_u64();
    (gs != 0).then(crate::arch::x86_64::percpu::current_index)
}

/// Per CPU, the least severe level a line may be stored at, while
/// `at_least` runs; `NO_FLOOR` otherwise
const NO_FLOOR: u8 = u8::MAX;
static FLOOR: [core::sync::atomic::AtomicU8; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU8::new(NO_FLOOR) }; crate::arch::x86_64::percpu::MAX_CPUS];

/// Run `f`, and record every line this CPU writes meanwhile at `level` or
/// worse.
///
/// For code reporting through a sink that cannot carry a level: the GPU driver
/// hands `&dyn Fn(&str)` down through most of its bring-up, and a failure told
/// through one arrives as the console's INFO. Wrapping the one call gives it
/// its level without a second copy of the line. Kernel code on a CPU is not
/// preempted, so the scope is this call; an interrupt printing in the middle of
/// it is raised too
pub fn at_least<R>(level: Level, f: impl FnOnce() -> R) -> R {
    let Some(cpu) = this_cpu() else { return f() };
    let slot = &FLOOR[cpu.min(FLOOR.len() - 1)];
    let prev = slot.swap((level as u8).min(slot.load(Relaxed)), Relaxed);
    let r = f();
    slot.store(prev, Relaxed);
    r
}

/// Per CPU, who a line is signed with while `speaking_for` runs; null
/// otherwise
static SPEAKER: [core::sync::atomic::AtomicPtr<Site>; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()) }; crate::arch::x86_64::percpu::MAX_CPUS];

/// Run `f`, and sign every line this CPU writes meanwhile with `site`, not the
/// site of the print.
///
/// `at_least`'s twin, for the source instead of the level. A sink handed down
/// as `&dyn Fn(&str)` prints with the site of its own file: the GPU boot's
/// `say` would make every line of the bring-up `bringup::rm`. Whoever hands a
/// subsystem a sink wraps the sink in this (`nvidia::speaking`), so each call
/// through it is signed, and only those: the subsystem's own RPC and RM traces
/// keep their sites.
///
/// Set only when nobody has: with sinks inside sinks, the wrapper nearest the
/// text runs first and wins. As with `at_least`, an interrupt printing in the
/// middle is signed too
pub fn speaking_for<R>(site: &'static Site, f: impl FnOnce() -> R) -> R {
    let Some(cpu) = this_cpu() else { return f() };
    let slot = &SPEAKER[cpu.min(SPEAKER.len() - 1)];
    let mine = site as *const Site as *mut Site;
    if slot.compare_exchange(core::ptr::null_mut(), mine, Relaxed, Relaxed).is_err() {
        return f();
    }
    let r = f();
    slot.store(core::ptr::null_mut(), Relaxed);
    r
}

/// Run `f` with nothing it prints stored in the log or mirrored to the UART:
/// the viewer's rule, for other code. For a line repeating on screen what the
/// log has
pub fn unrecorded<R>(f: impl FnOnce() -> R) -> R {
    static S: Site = Site::UNRECORDED;
    speaking_for(&S, f)
}

/// The site a line from `site` is signed with on this CPU right now
pub fn effective(site: Site) -> Site {
    let Some(cpu) = this_cpu() else { return site };
    let p = SPEAKER[cpu.min(SPEAKER.len() - 1)].load(Relaxed);
    if p.is_null() { site } else { unsafe { *p } }
}

/// A line from here goes nowhere but the screen: the viewer's own output, and
/// what `unrecorded` wraps
pub fn silent(module: &str) -> bool {
    view::is_viewer(module) || module == Site::UNRECORDED.module
}

/// Store one finished line
pub(crate) fn commit(mut level: Level, site: Site, mut fl: u8, text: &[u8]) {
    let site = effective(site);
    if silent(site.module) {
        return;
    }
    let (cpu, pid) = match this_cpu() {
        Some(c) => {
            let floor = FLOOR[c.min(FLOOR.len() - 1)].load(Relaxed);
            if floor < level as u8 {
                level = Level::from_u8(floor);
            }
            (c as u8, crate::sched::current_pid() as u32)
        }
        None => (0, 0),
    };
    if text.len() > ring::MAX_TEXT {
        fl |= flags::TRUNC;
    }
    if fl & flags::TRUNC != 0 {
        TRUNCATED.fetch_add(1, Relaxed);
    }
    COUNTS[level as usize].fetch_add(1, Relaxed);
    let ts_us = crate::kcore::time::since_boot_us();
    with_store(|s| {
        let meta = ring::Meta {
            seq: s.next_seq,
            ts_us,
            module: site.module,
            line: site.line,
            pid,
            level: level as u8,
            cpu,
            flags: fl,
        };
        s.next_seq += 1;
        let (st, b) = s.ring(level.ring());
        ring::push(st, b, &meta, text);
    });
    if PSTORE_MIRROR.load(Relaxed) {
        mirror(level, site, cpu, ts_us, text);
    }
}

/// Every line also goes to the persistent RAM store, with `klogpstore` on the
/// boot line: for a test that takes the screen down (GSP-RM unloaded and loaded
/// again), whose log is read after a warm reset. INFO and worse from
/// everywhere; DEBUG too from the NVIDIA driver, every RPC and RM call, where
/// the last one shows the hang
static PSTORE_MIRROR: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn mirror_to_pstore(on: bool) {
    PSTORE_MIRROR.store(on, Relaxed);
}

pub fn mirroring_to_pstore() -> bool {
    PSTORE_MIRROR.load(Relaxed)
}

/// The viewer's line, `[    12.345678] 2 W rpc::poll            text`, built on the stack: this runs wherever a line is committed, interrupt handlers included, and must not allocate
fn mirror(level: Level, site: Site, cpu: u8, ts_us: u64, text: &[u8]) {
    let wanted = level as u8 <= Level::Info as u8
        || (level == Level::Debug
            && (site.module.contains("gpu::nvidia") || site.module.contains("nvidia_cmds")));
    if !wanted || !crate::kcore::pstore::available() {
        return;
    }
    struct Buf { b: [u8; 1100], n: usize }
    impl fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let take = s.len().min(self.b.len() - self.n);
            self.b[self.n..self.n + take].copy_from_slice(&s.as_bytes()[..take]);
            self.n += take;
            Ok(())
        }
    }
    let mut out = Buf { b: [0; 1100], n: 0 };
    let body = core::str::from_utf8(text).unwrap_or("<not utf-8>");
    let body = body.trim_end_matches('\n');
    let _ = fmt::Write::write_fmt(&mut out, format_args!(
        "[{:>6}.{:06}] {} {} {:<20} {}",
        ts_us / 1_000_000, ts_us % 1_000_000, cpu, level.letter(),
        view::short_module(site.module), body,
    ));
    // Cut on a character boundary if the line ran out of room
    let mut n = out.n;
    while n > 0 && core::str::from_utf8(&out.b[..n]).is_err() {
        n -= 1;
    }
    if let Ok(line) = core::str::from_utf8(&out.b[..n]) {
        crate::kcore::pstore::write_line(line);
    }
}

/// A line for the UART and the log
pub fn emit(level: Level, site: Site, args: fmt::Arguments) {
    crate::arch::x86_64::serial::_print_at(level, site, 0, format_args!("{}\n", args));
}

/// A line for the log only, the UART too under `trace`. For what is too much
/// for a 115200-baud wire: every RPC, every RM call, GSP-RM's INFO records
pub fn quiet(level: Level, site: Site, args: fmt::Arguments) {
    quiet_as(level, site, 0, args)
}

/// `quiet`, with the record's flags: a program's own line (`flags::USER`)
/// stays one in the log
pub fn quiet_as(level: Level, site: Site, flags: u8, args: fmt::Arguments) {
    if crate::arch::x86_64::serial::tracing() {
        crate::arch::x86_64::serial::_print_at(level, site, flags, format_args!("{}\n", args));
        return;
    }
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        let mut w = line::Writer { level, site, flags };
        let _ = w.write_fmt(args);
        let _ = w.write_str("\n");
    });
}

/// Lines at each level since boot, `Level::ALL` order
pub fn counts() -> [u64; 6] {
    core::array::from_fn(|i| COUNTS[i].load(Relaxed))
}

/// One ring's state, for `stat`
#[derive(Clone, Copy, Debug)]
pub struct RingStat {
    pub records: u64,
    pub overwritten: u64,
    pub used: usize,
    pub size: usize,
}

pub struct Stat {
    pub rings: [RingStat; RINGS],
    pub next_seq: u64,
    pub dropped: u64,
    pub truncated: u64,
}

pub fn stat() -> Stat {
    let (rings, next_seq) = with_store(|s| {
        let r = core::array::from_fn(|i| {
            let (st, b) = s.ring_ref(i);
            RingStat { records: st.count, overwritten: st.first_idx, used: ring::used(st, b.len()), size: b.len() }
        });
        (r, s.next_seq)
    })
    .unwrap_or(([RingStat { records: 0, overwritten: 0, used: 0, size: 0 }; RINGS], 0));
    Stat { rings, next_seq, dropped: DROPPED.load(Relaxed), truncated: TRUNCATED.load(Relaxed) }
}

/// Each ring's size, for a reader sizing its scratch before it takes the lock
pub const RING_BYTES: [usize; RINGS] = [ERR_BYTES, MAIN_BYTES, DEBUG_BYTES];

/// Copy every record from `cur` on, ring by ring, into `scratch` laid end to
/// end (`ring::encode`). The caller has made each scratch's capacity its
/// ring's size, so nothing here allocates under the lock. Returns how many
/// records each ring overwrote before the reader got to them
pub(crate) fn copy_since(
    cur: &mut [ring::Cursor; RINGS],
    scratch: &mut [alloc::vec::Vec<u8>; RINGS],
) -> [u64; RINGS] {
    with_store(|s| {
        core::array::from_fn(|i| {
            let (st, b) = s.ring_ref(i);
            let out = &mut scratch[i];
            ring::read(st, b, &mut cur[i], &mut |m, t| {
                let at = out.len();
                let size = ring::record_size(t.len());
                if at + size <= out.capacity() {
                    out.resize(at + size, 0);
                    ring::encode(&mut out[at..], m, t);
                }
            })
        })
    })
    .unwrap_or([0; RINGS])
}

/// Cursors at the start and at the end of every ring
pub(crate) fn cursors(at_end: bool) -> [ring::Cursor; RINGS] {
    with_store(|s| {
        core::array::from_fn(|i| {
            let (st, _) = s.ring_ref(i);
            if at_end { ring::cursor_end(st) } else { ring::cursor_start(st) }
        })
    })
    .unwrap_or([ring::Cursor::default(); RINGS])
}

/// The next sequence number, which is where `mark` puts its mark
pub fn next_seq() -> u64 {
    with_store(|s| s.next_seq).unwrap_or(0)
}

#[macro_export]
macro_rules! kerror {
    ($($arg:tt)*) => {
        $crate::kcore::klog::emit($crate::kcore::klog::Level::Error, $crate::klog_site!(), format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kwarn {
    ($($arg:tt)*) => {
        $crate::kcore::klog::emit($crate::kcore::klog::Level::Warn, $crate::klog_site!(), format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! knotice {
    ($($arg:tt)*) => {
        $crate::kcore::klog::emit($crate::kcore::klog::Level::Notice, $crate::klog_site!(), format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kinfo {
    ($($arg:tt)*) => {
        $crate::kcore::klog::emit($crate::kcore::klog::Level::Info, $crate::klog_site!(), format_args!($($arg)*))
    };
}

/// Into the log and not down the UART (unless `trace`)
#[macro_export]
macro_rules! kdebug {
    ($($arg:tt)*) => {
        $crate::kcore::klog::quiet($crate::kcore::klog::Level::Debug, $crate::klog_site!(), format_args!($($arg)*))
    };
}
