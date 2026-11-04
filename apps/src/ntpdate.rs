#![no_std]
#![no_main]

//! ntpdate: asks a server for the time and sets the clock. Only setting it is
//! privileged

extern crate alloc;

pub use miku;

use alloc::string::String;
use mikunet::dns::LookupError;

/// time.google.com, as an address so it works before the resolver is up
const DEFAULT_SERVER: [u8; 4] = [216, 239, 35, 0];

fn num(v: u64) -> String {
    let mut buf = [0u8; 24];
    let n = miku::fmt_dec(&mut buf, v);
    String::from(core::str::from_utf8(&buf[..n]).unwrap_or("?"))
}

fn pad2(v: u64) -> String {
    let s = num(v);
    if s.len() < 2 { let mut p = String::from("0"); p.push_str(&s); p } else { s }
}

fn dotted(ip: [u8; 4]) -> String {
    let mut s = String::new();
    for (i, o) in ip.iter().enumerate() {
        if i > 0 { s.push('.'); }
        s.push_str(&num(*o as u64));
    }
    s
}

fn usage() -> ! {
    miku::println("usage: ntpdate [server] [-q]");
    miku::println("  -q   query only; do not set the clock");
    miku::exit(2);
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut server = DEFAULT_SERVER;
    let mut query_only = false;
    for a in args.iter().skip(1) {
        match *a {
            "-q" => query_only = true,
            "-v" => mikunet::plat::set_verbose(true),
            "-h" | "--help" => usage(),
            other if other.starts_with('-') => usage(),
            other => match mikunet::dns::lookup(other) {
                Ok(ip) => server = ip,
                Err(e) => {
                    miku::eprint("ntpdate: ");
                    match e {
                        LookupError::NoResolver(r) => miku::eprintln(r.message()),
                        LookupError::NotFound => {
                            miku::eprint("cannot resolve ");
                            miku::eprintln(other);
                        }
                    }
                    miku::exit(1);
                }
            },
        }
    }

    miku::print("ntpdate: asking ");
    miku::print(&dotted(server));
    miku::println("...");

    let r = match mikunet::ntp::sync(&server) {
        Some(r) => r,
        None => {
            miku::eprintln("ntpdate: no answer");
            miku::exit(1);
        }
    };

    let f = r.format();
    let when = alloc::format!(
        "{}-{}-{} {}:{}:{}.{} UTC",
        num(f.year as u64), pad2(f.month as u64), pad2(f.day as u64),
        pad2(f.hour as u64), pad2(f.min as u64), pad2(f.sec as u64),
        num(f.ms as u64),
    );
    miku::print("ntpdate: ");
    miku::print(&when);
    miku::print("  stratum ");
    miku::println(&num(r.stratum as u64));

    if query_only {
        miku::exit(0);
    }

    match unsafe { miku::miku_set_realtime(r.unix_secs as i64) } {
        0 => {
            miku::println("ntpdate: clock set");
            miku::exit(0);
        }
        // -EPERM
        -1 => {
            miku::eprintln("ntpdate: only root may set the clock (-q reports without setting it)");
            miku::exit(1);
        }
        _ => {
            miku::eprintln("ntpdate: the kernel refused the new time");
            miku::exit(1);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::eprintln("ntpdate: panic");
    miku::exit(1);
}
