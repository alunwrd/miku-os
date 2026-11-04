// Reading the log: the one filter, the one line format, the one viewer
//
// `dmesg` and `nvidia log` are two front ends over this. A reader copies what
// is new out of the rings under the lock (into scratch sized before taking it)
// and does everything else (decoding, filtering, merging the three rings into
// one order by sequence number, printing) with the lock released.
//
// Everything this module prints is kept out of the log and off the UART
// (`is_viewer`): on the kernel console the mirror would send a follow's output
// back into the log it follows, and a long dump down a 115200-baud wire.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::{ring, Level, RINGS};
use crate::{cprint, cprintln};

/// This module's path, as the print macros record it
pub const MODULE: &str = module_path!();

/// Is this line one the viewer printed?
pub fn is_viewer(module: &str) -> bool {
    module.len() == MODULE.len() && module == MODULE
}

/// One record, out of the ring
#[derive(Clone, Debug)]
pub struct Line {
    pub seq: u64,
    pub ts_us: u64,
    pub level: Level,
    pub cpu: u8,
    pub pid: u32,
    pub module: &'static str,
    pub line: u32,
    pub flags: u8,
    pub text: String,
}

/// A place in the log, and the scratch to copy into
pub struct Reader {
    cur: [ring::Cursor; RINGS],
    scratch: [Vec<u8>; RINGS],
}

impl Reader {
    fn new(at_end: bool) -> Reader {
        // Sized before the lock is taken; `copy_since` never grows them
        let scratch = core::array::from_fn(|i| Vec::with_capacity(super::RING_BYTES[i]));
        Reader { cur: super::cursors(at_end), scratch }
    }

    /// From the oldest record still kept
    pub fn from_start() -> Reader {
        Reader::new(false)
    }

    /// From now on
    pub fn from_end() -> Reader {
        Reader::new(true)
    }

    /// Everything since the last call, oldest first, and how many records were
    /// overwritten before this reader reached them
    pub fn next_batch(&mut self) -> (Vec<Line>, u64) {
        for s in self.scratch.iter_mut() {
            s.clear();
        }
        let lost = super::copy_since(&mut self.cur, &mut self.scratch);
        let mut out = Vec::new();
        for s in &self.scratch {
            ring::decode_all(s, &mut |m, t| {
                out.push(Line {
                    seq: m.seq,
                    ts_us: m.ts_us,
                    level: Level::from_u8(m.level),
                    cpu: m.cpu,
                    pid: m.pid,
                    module: m.module,
                    line: m.line,
                    flags: m.flags,
                    text: String::from_utf8_lossy(t).into_owned(),
                });
            });
        }
        // Each ring is in order already; three sorted runs into one
        out.sort_by_key(|l| l.seq);
        (out, lost.iter().sum())
    }
}

/// Which lines to show
#[derive(Clone, Debug)]
pub struct Filter {
    /// The least severe level shown: `Info` shows errors to info
    pub least: Level,
    /// Module path fragments; a line is shown if its module contains any. Empty
    /// shows every module
    pub modules: Vec<String>,
    /// Shown only if the text contains this, ignoring case (lower case here)
    pub text: Option<String>,
    /// Only lines from this sequence number on
    pub since: u64,
    /// Console output mirrored to the log (`flags::ECHO`)
    pub echo: bool,
}

impl Default for Filter {
    fn default() -> Filter {
        Filter { least: Level::Info, modules: Vec::new(), text: None, since: 0, echo: true }
    }
}

impl Filter {
    pub fn matches(&self, l: &Line) -> bool {
        l.level <= self.least
            && l.seq >= self.since
            && (self.echo || l.flags & super::flags::ECHO == 0)
            && (self.modules.is_empty() || self.modules.iter().any(|m| module_matches(l.module, m)))
            && self.text.as_ref().map_or(true, |t| contains_ci(&l.text, t))
    }
}

/// A module path fragment, or with a trailing `$` the path's end:
/// `gpu::nvidia$`
/// is the driver's root module and none of the ones under it
pub fn module_matches(module: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('$') {
        Some(end) => module.ends_with(end),
        None => module.contains(pattern),
    }
}

/// `hay` contains `needle` (already lower case), ASCII case ignored
fn contains_ci(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    n.is_empty() || h.windows(n.len()).any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

/// Everything kept that passes `f`, oldest first
pub fn collect(f: &Filter) -> Vec<Line> {
    let (lines, _) = Reader::from_start().next_batch();
    lines.into_iter().filter(|l| f.matches(l)).collect()
}

/// What to do, from a command line
#[derive(Clone, Debug)]
pub struct Opts {
    pub filter: Filter,
    /// Only the last N lines
    pub last: Option<usize>,
    /// Keep printing new lines until Ctrl+C
    pub follow: bool,
    /// pid, CPU, flags and `module:line` on every line
    pub verbose: bool,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    /// Remember where the log is now
    Mark,
    /// Counts and ring usage
    Stat,
    /// `selftest.rs`
    SelfTest,
    Help,
}

/// Where `mark` left the log; `new` shows from here
static MARK: AtomicU64 = AtomicU64::new(0);

/// Parse the words both front ends share. A word neither knows goes to `extra`
/// (`nvidia log` takes its section names that way), and is an error if `extra`
/// does not want it
pub fn parse(args: &str, extra: &mut dyn FnMut(&str) -> bool) -> Result<Opts, String> {
    let mut o = Opts { filter: Filter::default(), last: None, follow: false, verbose: false, action: Action::Show };
    // Quotes honoured, so `-g "boot events"` is one word (`kcore::words`)
    let list = crate::kcore::words::words(args);
    let mut words = list.iter().map(String::as_str);
    while let Some(w) = words.next() {
        let mut value = |what: &str| words.next().map(String::from).ok_or(alloc::format!("{} needs a value", what));
        match w {
            // What is usually asked for: everything that went wrong
            "err" | "errors" => o.filter.least = Level::Warn,
            "-n" | "--lines" => {
                let v = value("-n")?;
                o.last = Some(v.parse().map_err(|_| alloc::format!("-n wants a number, not '{}'", v))?);
            }
            "-f" | "--follow" | "follow" => o.follow = true,
            "-g" | "--grep" | "grep" => o.filter.text = Some(value("-g")?.to_ascii_lowercase()),
            "-m" | "--module" => o.filter.modules.push(value("-m")?),
            "-v" | "--verbose" => o.verbose = true,
            "-E" | "--no-echo" => o.filter.echo = false,
            "mark" => o.action = Action::Mark,
            "new" => o.filter.since = MARK.load(Relaxed),
            "stat" | "stats" => o.action = Action::Stat,
            "selftest" | "test" => o.action = Action::SelfTest,
            "help" | "-h" | "--help" | "?" => o.action = Action::Help,
            _ => {
                if let Some(l) = Level::parse(w) {
                    o.filter.least = l;
                } else if let Ok(n) = w.parse::<usize>() {
                    o.last = Some(n);
                } else if !extra(w) {
                    return Err(alloc::format!("'{}' is not something this knows - try 'help'", w));
                }
            }
        }
    }
    Ok(o)
}

/// The option lines of a help text; the front end prints its own first line
pub const HELP: &[&str] = &[
    "  error | err | warn | notice | info | debug | trace   least severe level shown",
    "                     (default info; 'err' = errors and warnings; 'debug' adds",
    "                     every RPC and RM call, 'trace' everything)",
    "  N | -n N           only the last N lines",
    "  -f                 keep printing new lines until Ctrl+C",
    "  -g TEXT            only lines containing TEXT (case ignored)",
    "  -m MODULE          only modules whose path contains MODULE (MODULE$: ends with)",
    "  -v                 pid, CPU and file:line on every line",
    "  -E                 leave out console output mirrored to the log",
    "  mark / new         remember where the log is / show only what came after",
    "  stat               lines per level since boot, ring usage and losses",
    "  selftest           check the log itself (run=dmesg_selftest on the boot line)",
];

/// Run `o`: show, follow, mark or count
pub fn run(o: &Opts, title: &str) {
    match o.action {
        Action::Help => {
            for l in HELP {
                cprintln!(128, 222, 217, "{}", l);
            }
        }
        Action::Mark => {
            let at = super::next_seq();
            MARK.store(at, Relaxed);
            cprintln!(100, 220, 150, "  marked at #{} - '{} new' shows what comes after", at, title);
        }
        Action::Stat => print_stat(&o.filter),
        Action::SelfTest => super::selftest::cmd(),
        Action::Show => show(o, title),
    }
}

fn show(o: &Opts, title: &str) {
    let mut reader = Reader::from_start();
    let (lines, _) = reader.next_batch();
    let picked: Vec<&Line> = lines.iter().filter(|l| o.filter.matches(l)).collect();
    let skip = o.last.map_or(0, |n| picked.len().saturating_sub(n));
    if picked.is_empty() {
        cprintln!(120, 140, 140, "  nothing in the {} log matches ({} lines kept) - 'help' for filters",
            title, lines.len());
    } else if skip > 0 {
        cprintln!(120, 140, 140, "  ... {} earlier lines not shown", skip);
    }
    for l in &picked[skip..] {
        print_line(l, o.verbose);
    }
    if o.follow {
        follow(reader, o);
    }
}

/// Print what arrives until Ctrl+C: the console's flag, as `top` uses, or a
/// signal for the process, which a Ctrl+C in a terminal becomes under `mikudo`
fn follow(mut reader: Reader, o: &Opts) {
    crate::net::CTRL_C.store(false, Relaxed);
    x86_64::instructions::interrupts::enable();
    cprintln!(120, 140, 140, "  -- following; Ctrl+C to stop --");
    let me = crate::sched::current_pid();
    // A tenth of a second, whatever the timer runs at
    let nap = (crate::fs::procfs::tick_hz() / 10).max(1);
    loop {
        if crate::net::CTRL_C.load(Relaxed) || crate::process::signal::pending(me) {
            break;
        }
        let (lines, lost) = reader.next_batch();
        if lost > 0 {
            cprintln!(255, 200, 80, "  -- {} lines were overwritten before they could be shown --", lost);
        }
        for l in lines.iter().filter(|l| o.filter.matches(l)) {
            print_line(l, o.verbose);
        }
        crate::sched::sleep(nap);
    }
    crate::net::CTRL_C.store(false, Relaxed);
}

/// The module path with the crate name taken off and, for anything this deep,
/// only its last two parts: `gsp_common::rpc::poll` reads as `rpc::poll`
pub fn short_module(m: &str) -> &str {
    let m = m.split_once("::").map_or(m, |(_, rest)| rest);
    match m.rmatch_indices("::").nth(1) {
        Some((i, _)) => &m[i + 2..],
        None => m,
    }
}

fn colour(l: Level) -> (u8, u8, u8) {
    match l {
        Level::Error => (255, 90, 90),
        Level::Warn => (255, 200, 80),
        Level::Notice => (128, 222, 217),
        Level::Info => (225, 235, 235),
        Level::Debug => (140, 155, 165),
        Level::Trace => (105, 115, 125),
    }
}

/// `[    12.345678] 2 W rpc::poll            text`
pub fn print_line(l: &Line, verbose: bool) {
    let (r, g, b) = colour(l.level);
    cprint!(110, 130, 130, "[{:>6}.{:06}] ", l.ts_us / 1_000_000, l.ts_us % 1_000_000);
    if verbose {
        let mut fl = String::new();
        if l.flags & super::flags::ECHO != 0 { fl.push_str(" echo"); }
        if l.flags & super::flags::TRUNC != 0 { fl.push_str(" cut"); }
        if l.flags & super::flags::USER != 0 { fl.push_str(" user"); }
        cprint!(110, 130, 130, "cpu{} pid {}{} ", l.cpu, l.pid, fl);
        cprint!(r, g, b, "{} ", l.level.letter());
        cprint!(150, 170, 170, "{}:{} ", short_module(l.module), l.line);
    } else {
        cprint!(110, 130, 130, "{} ", l.cpu);
        cprint!(r, g, b, "{} ", l.level.letter());
        cprint!(150, 170, 170, "{:<20} ", short_module(l.module));
    }
    cprintln!(r, g, b, "{}", l.text);
}

fn print_stat(f: &Filter) {
    let s = super::stat();
    let c = super::counts();
    cprintln!(57, 197, 187, "  written since boot, by level (kept or not):");
    for lv in Level::ALL {
        let (r, g, b) = colour(lv);
        cprintln!(r, g, b, "    {:<8} {}", lv.name(), c[lv as usize]);
    }
    cprintln!(57, 197, 187, "  rings:");
    for (i, rs) in s.rings.iter().enumerate() {
        cprintln!(128, 222, 217, "    {:<7} {:>6} lines kept, {:>7} overwritten, {:>4} of {} KiB",
            super::RING_NAMES[i], rs.records, rs.overwritten, rs.used >> 10, rs.size >> 10);
    }
    cprintln!(128, 222, 217, "    {} lines in all; {} dropped (lock never came free), {} cut to {} bytes",
        s.next_seq, s.dropped, s.truncated, ring::MAX_TEXT);
    // What the filter would show, by level: for `nvidia log stat`, the driver's
    // own share
    if !f.modules.is_empty() || f.text.is_some() {
        let mut wide = f.clone();
        wide.least = Level::Trace;
        wide.since = 0;
        let lines = collect(&wide);
        let mut by = [0usize; 6];
        for l in &lines {
            by[l.level as usize] += 1;
        }
        cprintln!(57, 197, 187, "  kept lines this filter picks: {} error, {} warning, {} notice, {} info, {} debug, {} trace",
            by[0], by[1], by[2], by[3], by[4], by[5]);
    }
}
