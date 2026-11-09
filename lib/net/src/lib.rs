// mikunet: the protocols a program speaks
//
// DNS, NTP, TLS 1.2 and 1.3 with X.509 verification, HTTP/1.1, and HTTP/2.
//
// `sock` is the seam: protocols above it, a byte stream below it.
//
// Without the `userspace` feature it is pure computation and compiles for the
// build host, so `cargo test` covers the crypto, certificates, and parsers.
// With it, `sock` and the platform hooks appear.

#![cfg_attr(not(test), no_std)]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

#[macro_use]
pub mod plat;

// Encoding and certificates, with no I/O. Behind `crypto`, on by default: the
// kernel links this crate for `stack` and does not speak TLS
#[cfg(feature = "crypto")]
pub use mikucrypto::der;
pub mod icmp;
pub mod ipv4;

/// Ethernet, ARP, IPv4 routing, UDP, TCP, and DHCP. On by default so
/// `cargo test` reaches it; a program wanting only TLS uses
/// `default-features = false`
#[cfg(feature = "stack")]
pub mod stack;
#[cfg(feature = "crypto")]
pub mod x509;

// The arithmetic and the primitives the handshake is built out of
#[cfg(feature = "crypto")]
pub use mikucrypto::{bignum, ec, gcm, primitives as crypto, rsa};

// Trust: chain, anchors, dates, and name
#[cfg(feature = "crypto")]
pub mod verify;

#[cfg(feature = "userspace")]
pub mod sock;

#[cfg(feature = "userspace")]
pub mod tls;
/// HTTP. `http::wire` holds the bytes (urls, requests, responses, chunked
/// framing, the streaming decoder) and compiles with `crypto`, so `cargo test`
/// covers it. The transport half is `userspace`-gated
#[cfg(feature = "crypto")]
pub mod http;
#[cfg(feature = "userspace")]
pub mod http2;
/// DNS as bytes, shared with `getaddrinfo` in `libc_miku.so`
pub mod dns_wire;
#[cfg(feature = "userspace")]
pub mod dns;
#[cfg(feature = "userspace")]
pub mod ntp;

use core::sync::atomic::{AtomicBool, Ordering};

/// Four octets of a dotted quad, or None for a name. Used by the certificate
/// matcher, the HTTP client, and the time client, which must agree.
///
/// Leading zeros are refused: `010.0.2.2` is ten here and eight to C's
/// `inet_aton`
pub fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = 0usize;
    for field in s.split('.') {
        if parts >= 4 || field.is_empty() || field.len() > 3 {
            return None;
        }
        if field.len() > 1 && field.as_bytes()[0] == b'0' {
            return None;
        }
        let mut v = 0u16;
        for b in field.bytes() {
            if !b.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (b - b'0') as u16;
        }
        if v > 255 {
            return None;
        }
        out[parts] = v as u8;
        parts += 1;
    }
    if parts == 4 { Some(out) } else { None }
}

/// Stop request, set by `interrupt()` from a SIGINT handler. Long-running loops
/// read it and give up cleanly, not in the middle of a record
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupt() {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub fn clear_interrupt() {
    INTERRUPTED.store(false, Ordering::SeqCst);
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}
