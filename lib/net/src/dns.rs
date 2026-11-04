// Name to address over a UDP datagram socket. Wire format in `dns_wire`

extern crate alloc;
use alloc::string::{String, ToString};
use spin::Mutex;

use crate::sock::Dgram;
// Wire format from `dns_wire`, shared with `getaddrinfo` in `libc_miku.so`
pub use crate::dns_wire::{build_query, parse_response};

const CACHE_SIZE: usize = 16;

struct CacheEntry {
    name: String,
    ip: [u8; 4],
    expires: u64,
}

/// Answers already received, to avoid a resolver round trip per connection
static CACHE: Mutex<alloc::vec::Vec<CacheEntry>> = Mutex::new(alloc::vec::Vec::new());

fn cache_get(name: &str) -> Option<[u8; 4]> {
    let now = crate::plat::uptime_ms();
    let mut c = CACHE.lock();
    c.retain(|e| e.expires > now);
    c.iter().find(|e| e.name == name).map(|e| e.ip)
}

fn cache_put(name: &str, ip: [u8; 4], ttl_secs: u32) {
    // A zero TTL means do not cache
    if ttl_secs == 0 {
        return;
    }
    // Long TTLs are common and the table is small
    let ttl = (ttl_secs as u64).min(3600);
    let expires = crate::plat::uptime_ms() + ttl * 1000;
    let mut c = CACHE.lock();
    if let Some(e) = c.iter_mut().find(|e| e.name == name) {
        e.ip = ip;
        e.expires = expires;
        return;
    }
    if c.len() >= CACHE_SIZE {
        c.remove(0);
    }
    c.push(CacheEntry { name: name.to_string(), ip, expires });
}

/// Drops all cached answers. A new lease can mean a new resolver
pub fn cache_flush() {
    CACHE.lock().clear();
}

pub fn resolve(hostname: &str, dns_server: &[u8; 4]) -> Option<[u8; 4]> {
    if hostname.is_empty() {
        return None;
    }

    if let Some(ip) = cache_get(hostname) {
        return Some(ip);
    }

    crate::clear_interrupt();

    // A fresh, unpredictable id per query, checked on reply. A guessable id
    // allows a forged answer
    let mut idb = [0u8; 2];
    if !crate::plat::random(&mut idb) {
        netlog_err!("dns: no entropy for a query id");
        return None;
    }
    let id = u16::from_ne_bytes(idb);

    let mut sock = Dgram::new()?;
    let r = resolve_inner(&mut sock, hostname, dns_server, id);
    if let Some((ip, ttl)) = r {
        cache_put(hostname, ip, ttl);
        return Some(ip);
    }
    None
}

/// Wait per attempt, and the number of attempts
const ATTEMPT_MS: u64 = 3_000;
const ATTEMPTS: u32 = 4;

fn resolve_inner(
    sock: &mut Dgram,
    hostname: &str,
    dns_server: &[u8; 4],
    id: u16,
) -> Option<([u8; 4], u32)> {
    let mut query = [0u8; 512];
    let qlen = build_query(hostname, id, &mut query)?;

    for attempt in 0..ATTEMPTS {
        if !sock.send_to(dns_server, 53, &query[..qlen]) {
            netlog!("dns: query {} not sent", attempt + 1);
        }
        netlog!("dns: query attempt {}/{} for {}", attempt + 1, ATTEMPTS, hostname);

        let start = crate::plat::uptime_ms();
        loop {
            if crate::interrupted() {
                return None;
            }
            let spent = crate::plat::uptime_ms().wrapping_sub(start);
            if spent >= ATTEMPT_MS {
                netlog!("dns: attempt {}/{} timeout", attempt + 1, ATTEMPTS);
                break;
            }

            let mut buf = [0u8; 1500];
            let (n, src_ip, src_port) = match sock.recv_from(&mut buf, ATTEMPT_MS - spent) {
                Some(v) => v,
                None => continue,
            };
            // The socket is unconnected, so anyone on the network can send
            // here; the id alone is not enough
            if src_port != 53 || src_ip != *dns_server {
                continue;
            }
            if let Some(answer) = parse_response(&buf[..n], id) {
                return Some(answer);
            }
        }
    }
    None
}

/// Why no resolver could be named. `Unconfigured` is fixed by `dhcp`;
/// `Unreadable` is not
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoResolver {
    /// `/proc/net` says `dns: 0.0.0.0`: no lease has set one
    Unconfigured,
    /// `/proc/net` could not be read or parsed
    Unreadable,
}

impl NoResolver {
    /// The sentence to print, without a program name in front of it
    pub fn message(self) -> &'static str {
        match self {
            NoResolver::Unconfigured =>
                "this machine has no resolver (is the network up? try 'dhcp')",
            NoResolver::Unreadable =>
                "cannot read /proc/net (kernel and userspace disagree; 'dhcp' will not help)",
        }
    }
}

/// The resolver from the DHCP lease, as published in `/proc/net`. There is no
/// `/etc/resolv.conf`.
///
/// An error means no resolver, a different failure from a lookup that got no
/// answer
pub fn system_resolver() -> Result<[u8; 4], NoResolver> {
    let mut size: usize = 0;
    let buf = unsafe { miku::miku_read_file(b"/proc/net\0".as_ptr(), &mut size as *mut usize) };
    if buf.is_null() {
        return Err(NoResolver::Unreadable);
    }
    let text = unsafe { core::slice::from_raw_parts(buf, size) };
    let out = core::str::from_utf8(text)
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("dns: ")))
        .and_then(parse_dotted);
    unsafe { miku::miku_free(buf) };

    // 0.0.0.0 is the field's value before a lease; asking it would only time
    // out
    match out {
        Some([0, 0, 0, 0]) => Err(NoResolver::Unconfigured),
        Some(ip) => Ok(ip),
        // Readable, but no parsable `dns:` line: the kernel and this parser
        // disagree on the format
        None => Err(NoResolver::Unreadable),
    }
}

/// The three ways turning a target into an address can end
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LookupError {
    /// No resolver to ask
    NoResolver(NoResolver),
    /// A resolver was asked and gave no answer
    NotFound,
}

/// Resolves an address literal or a name, keeping the two failures apart.
/// Printing stays in the caller, which owns the prefix and exit code
pub fn lookup(target: &str) -> Result<[u8; 4], LookupError> {
    if let Some(ip) = crate::parse_ipv4(target) {
        return Ok(ip);
    }
    let dns = system_resolver().map_err(LookupError::NoResolver)?;
    resolve(target, &dns).ok_or(LookupError::NotFound)
}

fn parse_dotted(s: &str) -> Option<[u8; 4]> {
    let mut p = s.trim().split('.');
    let out = [
        p.next()?.trim().parse().ok()?,
        p.next()?.trim().parse().ok()?,
        p.next()?.trim().parse().ok()?,
        p.next()?.trim().parse().ok()?,
    ];
    if p.next().is_some() { return None; }
    Some(out)
}
