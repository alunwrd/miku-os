#![no_std]
#![no_main]

//! mikuctl: controls services
//!
//! Usage: mikuctl <verb> [service]
//!
//!   start / stop / restart / reload <service>
//! enable / disable <service>       start at boot or not
//! status [service]
//! list
//! notify --ready|--watchdog        from inside a service
//!
//! mikuD lives in the kernel, so verbs go to it over `kcmd` (104). The
//! vocabulary is checked here, before reaching ring 0. Other kernel commands
//! use `mikudo` directly: `mikudo nvidia list`.

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::String;

#[path = "common.rs"]
mod common;
use common::*;

/// The verbs, and whether each one needs a service to act on
const VERBS: &[(&str, bool)] = &[
    ("start", true), ("stop", true), ("restart", true), ("reload", true),
    ("enable", true), ("disable", true),
    ("status", false), ("list", false),
];

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    if args.len() < 2 || args[1] == "-h" || args[1] == "--help" {
        usage();
        miku::exit(if args.len() < 2 { 2 } else { 0 });
    }

    let verb = args[1];
    if verb == "notify" {
        notify(&args[2..]);
    }
    let wants_name = match VERBS.iter().find(|(v, _)| *v == verb) {
        Some((_, needs)) => *needs,
        None => {
            errln(&format!("mikuctl: {verb}: not one of its verbs"));
            usage();
            miku::exit(2);
        }
    };

    let name = args.get(2).copied().unwrap_or("");
    if wants_name && name.is_empty() {
        errln(&format!("mikuctl: {verb}: which service?"));
        miku::exit(2);
    }
    // Validates the service name; the kernel screens the line as well
    if !name.is_empty() && !valid_name(name) {
        errln(&format!("mikuctl: {name}: a service name is letters, digits, - . _ and @"));
        miku::exit(2);
    }
    if args.len() > 3 {
        errln("mikuctl: one service at a time");
        miku::exit(2);
    }

    // `list` is the kernel's `sv list`; the rest are top-level commands
    let line = match verb {
        "list" => String::from("sv list"),
        "status" if name.is_empty() => String::from("sv status"),
        _ if name.is_empty() => String::from(verb),
        _ => format!("{verb} {name}"),
    };

    let rc = unsafe { miku::miku_kcmd(line.as_ptr(), line.len()) };
    if rc < 0 {
        match -rc {
            1 => errln("mikuctl: not permitted - services are the machine's, so: mikudo mikuctl ..."),
            22 => errln("mikuctl: the kernel would not take that"),
            _ => complain("mikuctl", &line, &reason(rc)),
        }
        miku::exit(1);
    }
    miku::exit(0);
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_alphanumeric()
            || b == b'-' || b == b'.' || b == b'_' || b == b'@')
}

/// `notify --ready` / `--watchdog`: READY=1 and WATCHDOG=1, like sd_notify.
/// Applies to the calling process's service, so it needs no mikudo. A program
/// can send `sv notify READY=1` through `miku_kcmd`
fn notify(flags: &[&str]) -> ! {
    let mut ready = false;
    let mut watchdog = false;
    for f in flags {
        match *f {
            "--ready" => ready = true,
            "--watchdog" => watchdog = true,
            other => {
                errln(&format!("mikuctl: notify: {other}: not --ready or --watchdog"));
                miku::exit(2);
            }
        }
    }
    if !ready && !watchdog {
        errln("mikuctl: notify: say --ready, --watchdog or both");
        miku::exit(2);
    }
    let mut line = String::from("sv notify");
    if ready { line.push_str(" READY=1"); }
    if watchdog { line.push_str(" WATCHDOG=1"); }
    let rc = unsafe { miku::miku_kcmd(line.as_ptr(), line.len()) };
    if rc < 0 {
        complain("mikuctl", &line, &reason(rc));
        miku::exit(1);
    }
    miku::exit(0);
}

fn usage() {
    outln("usage: mikuctl <verb> [service]");
    outln("");
    outln("  start|stop|restart|reload <service>");
    outln("  enable|disable <service>    whether it starts at boot");
    outln("  status [service]            what it is doing");
    outln("  list                        everything registered");
    outln("  notify --ready|--watchdog   from inside a service: it started / is alive");
    outln("");
    outln("  Services belong to the machine, so: mikudo mikuctl restart netd");
    outln("  Anything else that lives in the kernel goes through mikudo directly:");
    outln("      mikudo nvidia list");
    outln("      mikudo mount /dev/blk0 /mnt");
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
