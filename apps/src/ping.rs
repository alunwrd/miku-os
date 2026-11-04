#![no_std]
#![no_main]

//! ping: ICMP echo with round-trip time
//!
//! ICMP has no ports, so the socket sees all ICMP messages. The identifier
//! comes from kernel entropy, so concurrent pings do not read each other's
//! replies.

extern crate alloc;

pub use miku;

use alloc::string::String;
use mikunet::icmp;
use mikunet::sock::Icmp;
use mikunet::dns::LookupError;

/// Standard 56-byte payload: 64 bytes with the ICMP header, 84 with the IP
/// header
const PAYLOAD: &[u8] = b"MikuOS ping data 56b padding xxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

const REPLY_WAIT_MS: u64 = 1_000;
const GAP_MS: u64 = 1_000;

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

/// Milliseconds with one decimal, from a microsecond count
fn ms(us: u64) -> String {
    let mut s = num(us / 1000);
    s.push('.');
    s.push_str(&num((us % 1000) / 100));
    s
}

fn say(parts: &[&str]) {
    for p in parts { miku::print(p); }
    miku::println("");
}

fn usage() -> ! {
    miku::println("usage: ping <host|address> [count]");
    miku::exit(2);
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut target: Option<&str> = None;
    let mut count: u64 = 4;
    for a in args.iter().skip(1) {
        match *a {
            "-h" | "--help" => usage(),
            other if other.starts_with('-') => usage(),
            other if target.is_none() => target = Some(other),
            other => count = other.parse::<u64>().unwrap_or(4),
        }
    }
    let target = match target { Some(t) => t, None => usage() };

    let ip = match mikunet::dns::lookup(target) {
        Ok(ip) => ip,
        Err(e) => {
            miku::eprint("ping: ");
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
            miku::eprintln("ping: cannot open an ICMP socket");
            miku::exit(1);
        }
    };

    // Random identifier separating our replies from other pings'
    let mut idb = [0u8; 2];
    if !miku::getrandom(&mut idb) {
        miku::eprintln("ping: no entropy for an identifier");
        miku::exit(1);
    }
    let id = u16::from_ne_bytes(idb);

    say(&["PING ", target, " (", &dotted(ip), "): 56 bytes of data."]);

    let mut sent = 0u64;
    let mut received = 0u64;
    let mut rtt_min = u64::MAX;
    let mut rtt_max = 0u64;
    let mut rtt_sum = 0u64;

    for seq in 1..=count as u16 {
        let msg = icmp::echo_request(id, seq, PAYLOAD);
        let start = unsafe { miku::miku_uptime_ms() };
        if !sock.send_to(&ip, &msg) {
            miku::eprintln("ping: send failed (no route, or ARP got no answer)");
            break;
        }
        sent += 1;

        // Waits for our reply, ignoring unrelated ICMP traffic
        let mut buf = [0u8; 1500];
        loop {
            let waited = unsafe { miku::miku_uptime_ms() }.wrapping_sub(start);
            if waited >= REPLY_WAIT_MS {
                say(&["Request timeout for icmp_seq ", &num(seq as u64)]);
                break;
            }
            let n = match sock.recv(&mut buf, REPLY_WAIT_MS - waited) {
                Some(n) => n,
                None => continue,
            };
            let m = match icmp::parse(&buf[..n]) { Some(m) => m, None => continue };

            if m.icmp_type == icmp::ECHO_REPLY {
                if m.id != id || m.seq != seq { continue; }
                let rtt_us = unsafe { miku::miku_uptime_ms() }.wrapping_sub(start) * 1000;
                received += 1;
                rtt_sum += rtt_us;
                if rtt_us < rtt_min { rtt_min = rtt_us; }
                if rtt_us > rtt_max { rtt_max = rtt_us; }
                say(&["64 bytes from ", &dotted(m.src),
                      ": icmp_seq=", &num(seq as u64),
                      " ttl=", &num(m.ttl as u64),
                      " time=", &ms(rtt_us), " ms"]);
                break;
            }

            // An error about our probe is also an answer
            if m.id == id && m.seq == seq {
                say(&["From ", &dotted(m.src), ": ", icmp::describe(m.icmp_type, m.code)]);
                break;
            }
        }

        if seq as u64 != count {
            unsafe { miku::miku_sleep_ms(GAP_MS) };
        }
    }

    miku::println("");
    say(&["--- ", target, " ping statistics ---"]);
    let loss = if sent > 0 { ((sent - received) * 100) / sent } else { 100 };
    say(&[&num(sent), " packets transmitted, ", &num(received),
          " received, ", &num(loss), "% packet loss"]);
    if received > 0 {
        say(&["rtt min/avg/max = ", &ms(rtt_min), "/", &ms(rtt_sum / received),
              "/", &ms(rtt_max), " ms"]);
    }

    miku::exit(if received > 0 { 0 } else { 1 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::eprintln("ping: panic");
    miku::exit(1);
}
