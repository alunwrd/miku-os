#![no_std]
#![no_main]

//! host: resolves a name to an address through a UDP query

extern crate alloc;

pub use miku;

use alloc::string::String;

fn num(v: u64) -> String {
    let mut buf = [0u8; 24];
    let n = miku::fmt_dec(&mut buf, v);
    String::from(core::str::from_utf8(&buf[..n]).unwrap_or("?"))
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
    miku::println("usage: host <name> [server]");
    miku::println("  server   the resolver to ask; /proc/net's by default");
    miku::exit(2);
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut name: Option<&str> = None;
    let mut server: Option<&str> = None;
    for a in args.iter().skip(1) {
        match *a {
            "-v" => mikunet::plat::set_verbose(true),
            "-h" | "--help" => usage(),
            other if other.starts_with('-') => usage(),
            other if name.is_none() => name = Some(other),
            other if server.is_none() => server = Some(other),
            _ => usage(),
        }
    }
    let name = match name { Some(n) => n, None => usage() };

    // An address needs no resolver
    if let Some(ip) = mikunet::parse_ipv4(name) {
        miku::print(name);
        miku::print(" is ");
        miku::println(&dotted(ip));
        miku::exit(0);
    }

    let dns = match server {
        Some(s) => match mikunet::parse_ipv4(s) {
            Some(ip) => ip,
            None => {
                miku::eprint("host: not an address: ");
                miku::eprintln(s);
                miku::exit(2);
            }
        },
        None => match mikunet::dns::system_resolver() {
            Ok(ip) => ip,
            Err(e) => {
                miku::eprint("host: ");
                miku::eprintln(e.message());
                miku::exit(1);
            }
        },
    };

    match mikunet::dns::resolve(name, &dns) {
        Some(ip) => {
            miku::print(name);
            miku::print(" has address ");
            miku::println(&dotted(ip));
            miku::exit(0);
        }
        None => {
            miku::eprint("host: ");
            miku::eprint(name);
            miku::eprint(" not found (asked ");
            miku::eprint(&dotted(dns));
            miku::eprintln(")");
            miku::exit(1);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::eprintln("host: panic");
    miku::exit(1);
}
