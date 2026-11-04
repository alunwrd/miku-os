// The kernel log, tested where it runs
//
// The ring itself (eviction, wrap, cursors, losses) is tested on the build host
// over a `Vec` (`ring.rs` has no lock and no static for that reason). Here is
// the rest: the one store behind three rings, the per-CPU line assembly, the
// levels a caller's macro chose, the viewer staying out of its own log. A
// handful of lines are written into the live log under a marker nobody else
// prints, and read back.
//
// `dmesg selftest` from the shell, `run=dmesg_selftest` on the boot line.

use alloc::format;
use alloc::string::String;

use super::view::{self, Filter, Reader};
use super::{flags, line, ring, Level, Site};
use crate::kcore::selftest::{Report, Runner};

const SITE: Site = Site { module: module_path!(), line: 0 };

/// A string no other line in the log contains
fn marker(what: &str) -> String {
    format!("klogtest-{}-{}", super::next_seq(), what)
}

fn only(text: &str) -> Filter {
    Filter { least: Level::Trace, text: Some(text.to_ascii_lowercase()), ..Default::default() }
}

pub fn run() -> Report {
    let mut r = Runner::new();

    // Each level lands, in the order written, whichever ring keeps it
    let m = marker("levels");
    for lv in Level::ALL {
        super::quiet(lv, SITE, format_args!("{} {}", m, lv.name()));
    }
    let got = view::collect(&only(&m));
    let levels: alloc::vec::Vec<Level> = got.iter().map(|l| l.level).collect();
    r.check("levels_route", levels == Level::ALL,
        format!("wrote one line at each of 6 levels, read back {:?}", levels));
    let ordered = got.windows(2).all(|w| w[0].seq < w[1].seq);
    r.check("merge_order", ordered && got.len() == 6,
        format!("three rings merged by sequence: {:?}", got.iter().map(|l| l.seq).collect::<alloc::vec::Vec<_>>()));
    r.check("site_kept", got.first().is_some_and(|l| l.module == SITE.module),
        format!("module {:?}", got.first().map(|l| l.module)));

    // Fragments on one CPU become lines at the newline, not before
    let m = marker("frag");
    x86_64::instructions::interrupts::without_interrupts(|| {
        let feed = |s: &str| line::feed(Level::Info, SITE, 0, s.as_bytes());
        feed(&m);
        feed(" ab");
        feed("c\n");
        feed(&m);
        feed(" de");
        feed("f\n");
    });
    let texts: alloc::vec::Vec<String> = view::collect(&only(&m)).into_iter().map(|l| l.text).collect();
    let want = [format!("{} abc", m), format!("{} def", m)];
    r.check("fragments", texts == want, format!("got {:?}", texts));

    // A line longer than a record is kept cut, and says so
    let m = marker("long");
    let long = format!("{} {}", m, "x".repeat(ring::MAX_TEXT + 200));
    super::quiet(Level::Info, SITE, format_args!("{}", long));
    let got = view::collect(&only(&m));
    let ok = got.len() == 1 && got[0].text.len() == ring::MAX_TEXT && got[0].flags & flags::TRUNC != 0;
    r.check("truncation", ok, format!("{} line(s), {} bytes, flags {:#x}",
        got.len(), got.first().map_or(0, |l| l.text.len()), got.first().map_or(0, |l| l.flags)));

    // `at_least` raises what is written inside it, and only inside it
    let m = marker("floor");
    super::at_least(Level::Warn, || super::quiet(Level::Info, SITE, format_args!("{} inside", m)));
    super::quiet(Level::Info, SITE, format_args!("{} after", m));
    let got = view::collect(&only(&m));
    let lv: alloc::vec::Vec<Level> = got.iter().map(|l| l.level).collect();
    r.check("floor", lv == [Level::Warn, Level::Info], format!("levels {:?}", lv));

    // What the viewer prints does not come back into the log
    let m = marker("viewer");
    crate::arch::x86_64::serial::echo(Site { module: view::MODULE, line: 0 }, format_args!("{}\n", m));
    let n = view::collect(&only(&m)).len();
    r.check("viewer_silent", n == 0, format!("{} line(s) from the viewer's module recorded", n));

    // `speaking_for` signs what is written inside it and nothing after; of two
    // wrappers the one entered first (nearest the text, when sinks wrap sinks)
    // wins; `unrecorded` keeps a line out, mirror included
    static OUTER: Site = Site { module: "klogtest::outer", line: 1 };
    static INNER: Site = Site { module: "klogtest::inner", line: 2 };
    let m = marker("speaker");
    super::speaking_for(&OUTER, || super::quiet(Level::Info, SITE, format_args!("{} one", m)));
    super::speaking_for(&INNER, || {
        super::speaking_for(&OUTER, || super::quiet(Level::Info, SITE, format_args!("{} two", m)))
    });
    super::quiet(Level::Info, SITE, format_args!("{} three", m));
    super::unrecorded(|| {
        super::quiet(Level::Warn, SITE, format_args!("{} four", m));
        crate::arch::x86_64::serial::echo(SITE, format_args!("{} five\n", m));
    });
    let mods: alloc::vec::Vec<&str> = view::collect(&only(&m)).iter().map(|l| l.module).collect();
    r.check("speaker", mods == [OUTER.module, INNER.module, SITE.module],
        format!("signed {:?}", mods));

    // A reader from the end sees exactly what came after it, once
    let m = marker("follow");
    let mut reader = Reader::from_end();
    for i in 0..3 {
        super::quiet(Level::Debug, SITE, format_args!("{} {}", m, i));
    }
    let f = only(&m);
    let (first, lost1) = reader.next_batch();
    let first = first.into_iter().filter(|l| f.matches(l)).count();
    let (second, lost2) = reader.next_batch();
    let second = second.into_iter().filter(|l| f.matches(l)).count();
    r.check("reader_from_end", first == 3 && second == 0 && lost1 + lost2 == 0,
        format!("first batch {}, second {}, lost {}", first, second, lost1 + lost2));

    // Filters: level, module, text with its case ignored
    let m = marker("filter");
    super::quiet(Level::Debug, SITE, format_args!("{} MiXeD", m));
    let all = view::collect(&only(&m)).len();
    let info_only = view::collect(&Filter { least: Level::Info, ..only(&m) }).len();
    let mixed = view::collect(&only(&format!("{} mixed", m))).len();
    let other_module = view::collect(&Filter { modules: alloc::vec![String::from("no::such")], ..only(&m) }).len();
    r.check("filters", (all, info_only, mixed, other_module) == (1, 0, 1, 0),
        format!("any level {}, info and worse {}, case-folded {}, other module {}", all, info_only, mixed, other_module));

    // The counters move with what is written
    let before = super::counts()[Level::Debug as usize];
    super::quiet(Level::Debug, SITE, format_args!("{}", marker("count")));
    let after = super::counts()[Level::Debug as usize];
    r.check("counts", after > before, format!("debug lines {} -> {}", before, after));

    // A phrase with a space in it is one word to `-g`, quoted as msh and
    // `mikudod` quote it (`kcore::words`)
    let w = crate::kcore::words::words(r#"-g "boot events" 'a b' c\ d "q\"x" err"#);
    let opts = view::parse(r#"err -g "Boot Events" 5"#, &mut |_| false);
    let phrase = opts.as_ref().ok().and_then(|o| o.filter.text.clone());
    r.check("quoted_words",
        w == ["-g", "boot events", "a b", "c d", "q\"x", "err"] && phrase.as_deref() == Some("boot events"),
        format!("words {:?}, -g took {:?}", w, phrase));

    r.report()
}

/// `dmesg selftest`
pub fn cmd() {
    crate::cprintln!(57, 197, 187, "klog: kernel log selftest");
    let report = run();
    for c in &report.cases {
        if c.ok {
            crate::cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            crate::print_error!("  FAIL  {}", c.name);
        }
        crate::println!("          {}", c.detail);
        crate::serial_println!("[klog-test] {} {} - {}", if c.ok { "ok  " } else { "FAIL" }, c.name, c.detail);
    }
    let (pass, fail) = (report.passed(), report.failed());
    crate::serial_println!("[klog-test] {} passed, {} failed", pass, fail);
    if fail == 0 {
        crate::print_success!("klog: {} passed", pass);
    } else {
        crate::print_error!("klog: {} passed, {} FAILED", pass, fail);
    }
}
