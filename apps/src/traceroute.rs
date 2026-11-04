#![no_std]
#![no_main]

//! traceroute: echo requests with rising TTL (`setsockopt(IP_TTL)`). Each
//! router reports through the source address of its time-exceeded message

extern crate alloc;

pub use miku;

use alloc::string::String;
use mikunet::icmp;
use mikunet::sock::Icmp;
use mikunet::dns::LookupError;

const MAX_HOPS: u8 = 30;
const PROBES_PER_HOP: u16 = 3;
const PROBE_WAIT_MS: u64 = 1_000;
const PAYLOAD: &[u8] = b"MikuOS traceroute probe";

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

fn ms(us: u64) -> String {
    let mut s = num(us / 1000);
    s.push('.');
    s.push_str(&num((us % 1000) / 100));
    s
}

fn usage() -> ! {
    miku::println("usage: traceroute <host|address> [max-hops]");
    miku::exit(2);
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut target: Option<&str> = None;
    let mut max_hops = MAX_HOPS;
    for a in args.iter().skip(1) {
        match *a {
            "-h" | "--help" => usage(),
            other if other.starts_with('-') => usage(),
            other if target.is_none() => target = Some(other),
            other => max_hops = other.parse::<u8>().unwrap_or(MAX_HOPS).clamp(1, 64),
        }
    }
    let target = match target { Some(t) => t, None => usage() };

    let ip = match mikunet::dns::lookup(target) {
        Ok(ip) => ip,
        Err(e) => {
            miku::eprint("traceroute: ");
            match e {
                LookupError::NoResolver(r) => miku::eprintln(r.message()),
                LookupError::NotFound => {
                    miku::eprint("cannot resolve ");
                    miku::eprintln(target);
                }
            }
            miku::exit(1);
        }
    };

    let mut sock = match Icmp::new() {
        Some(s) => s,
        None => {
            miku::eprintln("traceroute: cannot open an ICMP socket");
            miku::exit(1);
        }
    };

    let mut idb = [0u8; 2];
    if !miku::getrandom(&mut idb) {
        miku::eprintln("traceroute: no entropy for an identifier");
        miku::exit(1);
    }
    let id = u16::from_ne_bytes(idb);

    miku::print("traceroute to ");
    miku::print(target);
    miku::print(" (");
    miku::print(&dotted(ip));
    miku::print("), ");
    miku::print(&num(max_hops as u64));
    miku::println(" hops max");

    let mut seq: u16 = 0;
    let mut arrived = false;

    for ttl in 1..=max_hops {
        if !sock.set_ttl(ttl) {
            miku::eprintln("traceroute: the kernel refused the hop limit");
            miku::exit(1);
        }

        miku::print(&num(ttl as u64));
        miku::print("  ");

        // Address answering this hop, printed once
        let mut shown: Option<[u8; 4]> = None;
        let mut any = false;

        for _ in 0..PROBES_PER_HOP {
            seq = seq.wrapping_add(1);
            let msg = icmp::echo_request(id, seq, PAYLOAD);
            let start = unsafe { miku::miku_uptime_ms() };
            if !sock.send_to(&ip, &msg) {
                miku::print("!send ");
                continue;
            }

            let mut buf = [0u8; 1500];
            let mut got = false;
            loop {
                let waited = unsafe { miku::miku_uptime_ms() }.wrapping_sub(start);
                if waited >= PROBE_WAIT_MS { break; }
                let n = match sock.recv(&mut buf, PROBE_WAIT_MS - waited) {
                    Some(n) => n,
                    None => continue,
                };
                let m = match icmp::parse(&buf[..n]) { Some(m) => m, None => continue };
                // Only this probe: a time-exceeded message quotes our echo's
                // identifier and sequence
                if m.id != id || m.seq != seq { continue; }

                let rtt_us = unsafe { miku::miku_uptime_ms() }.wrapping_sub(start) * 1000;
                if shown != Some(m.src) {
                    miku::print(&dotted(m.src));
                    miku::print("  ");
                    shown = Some(m.src);
                }
                miku::print(&ms(rtt_us));
                miku::print(" ms  ");
                any = true;
                got = true;

                if m.icmp_type == icmp::ECHO_REPLY {
                    arrived = true;
                } else if m.icmp_type == icmp::DEST_UNREACHABLE {
                    miku::print("(");
                    miku::print(icmp::describe(m.icmp_type, m.code));
                    miku::print(")  ");
                }
                break;
            }
            if !got {
                miku::print("*  ");
            }
        }

        // A silent hop prints stars and is not an error
        let _ = any;
        miku::println("");

        if arrived {
            break;
        }
    }

    if !arrived {
        miku::println("traceroute: the target did not answer within the hop limit");
    }
    miku::exit(if arrived { 0 } else { 1 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::eprintln("traceroute: panic");
    miku::exit(1);
}
