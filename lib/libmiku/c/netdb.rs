// netdb.h: `getaddrinfo`, `freeaddrinfo`, `gai_strerror`
//
// The resolver for programs not linking `mikunet`: `std`'s `lookup_host` and C
// libraries. In order:
//
//   1. an address literal, `a.b.c.d`, needs no resolver
//   2. `localhost` and `*.localhost` are the loopback address (RFC 6761)
//   3. `/etc/hosts`, when present
//   4. DNS: one A query over UDP to the resolver the DHCP lease named,
//      published
//      as the `dns:` line of `/proc/net`, where `mikunet::dns::system_resolver` reads too
//
// Query and answer bytes are `lib/net/src/dns_wire.rs`, included by path,
// tested with `cd lib/net && cargo test`.
//
// IPv4 only: `AF_INET6` is `EAI_FAMILY`; `AF_UNSPEC` returns IPv4 addresses.
// Services are port numbers plus the names below; there is no `/etc/services`.

#[path = "../../net/src/dns_wire.rs"]
mod dns_wire;

use crate::libc::{close, free, malloc, open, poll, pollfd, read, recvfrom, sendto, socket};

// Linux's values, as the `libc` crate passes them
pub const EAI_BADFLAGS: i32 = -1;
pub const EAI_NONAME: i32 = -2;
pub const EAI_AGAIN: i32 = -3;
pub const EAI_FAIL: i32 = -4;
pub const EAI_FAMILY: i32 = -6;
pub const EAI_SOCKTYPE: i32 = -7;
pub const EAI_SERVICE: i32 = -8;
pub const EAI_MEMORY: i32 = -10;
pub const EAI_SYSTEM: i32 = -11;

pub const AI_PASSIVE: i32 = 0x0001;
pub const AI_CANONNAME: i32 = 0x0002;
pub const AI_NUMERICHOST: i32 = 0x0004;
pub const AI_V4MAPPED: i32 = 0x0008;
pub const AI_ALL: i32 = 0x0010;
pub const AI_ADDRCONFIG: i32 = 0x0020;
pub const AI_NUMERICSERV: i32 = 0x0400;
const AI_KNOWN: i32 = AI_PASSIVE | AI_CANONNAME | AI_NUMERICHOST | AI_V4MAPPED | AI_ALL
    | AI_ADDRCONFIG | AI_NUMERICSERV;

const AF_UNSPEC: i32 = 0;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_UDP: i32 = 17;
const MSG_DONTWAIT: i32 = 0x40;
const POLLIN: i16 = 0x001;
const CLOCK_MONOTONIC: i32 = 1;
const O_RDONLY: i32 = 0x0001;

/// Wait per DNS attempt and the attempt count, shorter than `mikunet`'s, since
/// a program blocked in `connect("name:port")` has a user waiting
const ATTEMPT_MS: i32 = 2_000;
const ATTEMPTS: u32 = 3;

/// `struct addrinfo`, Linux's layout: `ai_addr` before `ai_canonname`
#[repr(C)]
pub struct addrinfo {
    pub ai_flags: i32,
    pub ai_family: i32,
    pub ai_socktype: i32,
    pub ai_protocol: i32,
    pub ai_addrlen: u32,
    pub ai_addr: *mut u8,
    pub ai_canonname: *mut u8,
    pub ai_next: *mut addrinfo,
}

/// Each result is one allocation: the `addrinfo`, its `sockaddr_in`, and on the
/// first the canonical name. `freeaddrinfo` frees one block per node
const NODE: usize = core::mem::size_of::<addrinfo>();
const SIN: usize = 16;

unsafe fn cstr<'a>(p: *const u8) -> Option<&'a [u8]> {
    if p.is_null() {
        return None;
    }
    let n = crate::libc::strlen(p);
    Some(unsafe { core::slice::from_raw_parts(p, n) })
}

/// `a.b.c.d` only. `inet_aton`'s `127.1`, octal, and single-number forms could
/// read as different hosts
fn dotted(s: &[u8]) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = s.split(|&b| b == b'.');
    for o in out.iter_mut() {
        let p = parts.next()?;
        if p.is_empty() || p.len() > 3 || !p.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let v = p.iter().fold(0u32, |a, &d| a * 10 + (d - b'0') as u32);
        if v > 255 {
            return None;
        }
        *o = v as u8;
    }
    if parts.next().is_some() { None } else { Some(out) }
}

fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn is_localhost(name: &[u8]) -> bool {
    let name = name.strip_suffix(b".").unwrap_or(name);
    eq_ignore_case(name, b"localhost")
        || (name.len() > 10 && eq_ignore_case(&name[name.len() - 10..], b".localhost"))
}

/// Reads a small file into `buf`, returning its length
fn slurp(path: &[u8], buf: &mut [u8]) -> Option<usize> {
    let fd = open(path.as_ptr(), O_RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let mut at = 0;
    while at < buf.len() {
        let n = read(fd, unsafe { buf.as_mut_ptr().add(at) }, buf.len() - at);
        if n <= 0 {
            break;
        }
        at += n as usize;
    }
    close(fd);
    Some(at)
}

/// `name` in `/etc/hosts`: `address name alias...`, `#` to end of line
fn from_hosts(name: &[u8]) -> Option<[u8; 4]> {
    let mut buf = [0u8; 4096];
    let n = slurp(b"/etc/hosts\0", &mut buf)?;
    for line in buf[..n].split(|&b| b == b'\n') {
        let line = line.split(|&b| b == b'#').next().unwrap_or(&[]);
        let mut words = line.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty());
        let Some(addr) = words.next().and_then(dotted) else { continue };
        if words.any(|w| eq_ignore_case(w, name)) {
            return Some(addr);
        }
    }
    None
}

/// The lease's resolver, from `/proc/net`'s `dns:` line. `None` with no address
/// of our own (`ip: 0.0.0.0`): before the lease the line holds a fallback, and
/// asking it could only time out
fn system_resolver() -> Option<[u8; 4]> {
    let mut buf = [0u8; 2048];
    let n = slurp(b"/proc/net\0", &mut buf)?;
    let field = |key: &[u8]| {
        buf[..n]
            .split(|&b| b == b'\n')
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| dotted(v.trim_ascii()))
    };
    let ours = field(b"ip: ")?;
    let dns = field(b"dns: ")?;
    if ours == [0, 0, 0, 0] || dns == [0, 0, 0, 0] { None } else { Some(dns) }
}

fn now_ms() -> u64 {
    let mut ts = crate::libc::timespec { tv_sec: 0, tv_nsec: 0 };
    crate::libc::clock_gettime(CLOCK_MONOTONIC, &mut ts);
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

fn sockaddr_in(ip: [u8; 4], port: u16) -> [u8; SIN] {
    let mut sa = [0u8; SIN];
    sa[..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&ip);
    sa
}

/// One A query. `Err` is the `EAI_*` to return
fn dns_query(name: &[u8], server: [u8; 4]) -> Result<[u8; 4], i32> {
    let Ok(host) = core::str::from_utf8(name) else { return Err(EAI_NONAME) };
    let mut idb = [0u8; 2];
    // Unpredictable, or an answer can be forged by guessing it
    let r = unsafe { crate::sys::sc3(crate::sys::SYS_GETRANDOM, idb.as_mut_ptr() as u64, 2, 0) };
    if r != 2 {
        return Err(EAI_SYSTEM);
    }
    let id = u16::from_ne_bytes(idb);
    let mut query = [0u8; 512];
    let qlen = dns_wire::build_query(host, id, &mut query).ok_or(EAI_NONAME)?;

    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        return Err(EAI_SYSTEM);
    }
    let to = sockaddr_in(server, 53);
    let mut result = Err(EAI_AGAIN);
    'attempts: for _ in 0..ATTEMPTS {
        if sendto(fd, query.as_ptr(), qlen, 0, to.as_ptr(), SIN as u32) < 0 {
            continue;
        }
        let mut left = ATTEMPT_MS;
        while left > 0 {
            let mut p = pollfd { fd, events: POLLIN, revents: 0 };
            let t0 = now_ms();
            if poll(&mut p, 1, left) <= 0 {
                break;
            }
            // What arrived may not be the answer; the next wait is what is left
            // of this attempt
            left -= (now_ms().saturating_sub(t0) as i32).max(1);
            let mut buf = [0u8; 1500];
            let mut from = [0u8; SIN];
            let mut flen = SIN as u32;
            let n = recvfrom(fd, buf.as_mut_ptr(), buf.len(), MSG_DONTWAIT, from.as_mut_ptr(), &mut flen);
            if n < 12 {
                continue;
            }
            // Only the server asked, from port 53: the socket is unconnected
            // and anyone may send to it
            if from[2..4] != 53u16.to_be_bytes() || from[4..8] != server {
                continue;
            }
            let reply = &buf[..n as usize];
            if u16::from_be_bytes([reply[0], reply[1]]) != id || reply[2] & 0x80 == 0 {
                continue;
            }
            result = match dns_wire::parse_response(reply, id) {
                Some((ip, _ttl)) => Ok(ip),
                // NXDOMAIN or no A record: no such host, as opposed to no reply
                None if reply[3] & 0x0F == 3 || reply[3] & 0x0F == 0 => Err(EAI_NONAME),
                None => Err(EAI_FAIL),
            };
            break 'attempts;
        }
    }
    close(fd);
    result
}

fn service_port(service: &[u8], flags: i32) -> Result<u16, i32> {
    if !service.is_empty() && service.iter().all(u8::is_ascii_digit) {
        let v = service.iter().fold(0u32, |a, &d| a.saturating_mul(10).saturating_add((d - b'0') as u32));
        return u16::try_from(v).map_err(|_| EAI_SERVICE);
    }
    if flags & AI_NUMERICSERV != 0 {
        return Err(EAI_NONAME);
    }
    Ok(match service {
        b"http" => 80,
        b"https" => 443,
        b"ssh" => 22,
        b"domain" => 53,
        b"ntp" => 123,
        _ => return Err(EAI_SERVICE),
    })
}

/// POSIX `getaddrinfo`, IPv4. Name order is at the top of this file
#[no_mangle]
#[inline(never)]
pub extern "C" fn getaddrinfo(
    node: *const u8,
    service: *const u8,
    hints: *const addrinfo,
    res: *mut *mut addrinfo,
) -> i32 {
    if res.is_null() {
        return EAI_SYSTEM;
    }
    let (flags, family, socktype, protocol) = if hints.is_null() {
        (0, AF_UNSPEC, 0, 0)
    } else {
        let h = unsafe { &*hints };
        (h.ai_flags, h.ai_family, h.ai_socktype, h.ai_protocol)
    };
    if flags & !AI_KNOWN != 0 {
        return EAI_BADFLAGS;
    }
    if family != AF_UNSPEC && family != AF_INET {
        return EAI_FAMILY;
    }
    if !matches!(socktype, 0 | SOCK_STREAM | SOCK_DGRAM | SOCK_RAW) {
        return EAI_SOCKTYPE;
    }
    let node_s = unsafe { cstr(node) };
    let service_s = unsafe { cstr(service) };
    if node_s.is_none() && service_s.is_none() {
        return EAI_NONAME;
    }
    let port = match service_s {
        Some(s) => match service_port(s, flags) {
            Ok(p) => p,
            Err(e) => return e,
        },
        None => 0,
    };

    let ip = match node_s {
        None if flags & AI_PASSIVE != 0 => [0, 0, 0, 0],
        None => [127, 0, 0, 1],
        Some(n) => match dotted(n) {
            Some(ip) => ip,
            None if flags & AI_NUMERICHOST != 0 => return EAI_NONAME,
            None if is_localhost(n) => [127, 0, 0, 1],
            None => match from_hosts(n) {
                Some(ip) => ip,
                None => match system_resolver() {
                    // No lease yet: a temporary failure, as glibc reports
                    None => return EAI_AGAIN,
                    Some(server) => match dns_query(n, server) {
                        Ok(ip) => ip,
                        Err(e) => return e,
                    },
                },
            },
        },
    };

    // One result per socket type asked for, or the two a caller can use
    let types: &[(i32, i32)] = match socktype {
        SOCK_STREAM => &[(SOCK_STREAM, IPPROTO_TCP)],
        SOCK_DGRAM => &[(SOCK_DGRAM, IPPROTO_UDP)],
        SOCK_RAW => &[(SOCK_RAW, 0)],
        _ => &[(SOCK_STREAM, IPPROTO_TCP), (SOCK_DGRAM, IPPROTO_UDP)],
    };
    let canon = if flags & AI_CANONNAME != 0 { node_s } else { None };
    let sa = sockaddr_in(ip, port);

    let mut head: *mut addrinfo = core::ptr::null_mut();
    let mut tail: *mut *mut addrinfo = &mut head;
    for (i, &(ty, proto)) in types.iter().enumerate() {
        let name = if i == 0 { canon } else { None };
        let extra = name.map_or(0, |n| n.len() + 1);
        let block = malloc(NODE + SIN + extra);
        if block.is_null() {
            freeaddrinfo(head);
            return EAI_MEMORY;
        }
        unsafe {
            let addr = block.add(NODE);
            core::ptr::copy_nonoverlapping(sa.as_ptr(), addr, SIN);
            let canonname = match name {
                Some(n) => {
                    let c = addr.add(SIN);
                    core::ptr::copy_nonoverlapping(n.as_ptr(), c, n.len());
                    *c.add(n.len()) = 0;
                    c
                }
                None => core::ptr::null_mut(),
            };
            let node = block as *mut addrinfo;
            node.write(addrinfo {
                ai_flags: flags,
                ai_family: AF_INET,
                ai_socktype: ty,
                ai_protocol: if protocol != 0 { protocol } else { proto },
                ai_addrlen: SIN as u32,
                ai_addr: addr,
                ai_canonname: canonname,
                ai_next: core::ptr::null_mut(),
            });
            *tail = node;
            tail = &mut (*node).ai_next;
        }
    }
    unsafe { *res = head };
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn freeaddrinfo(mut ai: *mut addrinfo) {
    while !ai.is_null() {
        let next = unsafe { (*ai).ai_next };
        free(ai as *mut u8);
        ai = next;
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn gai_strerror(code: i32) -> *const u8 {
    let s: &'static [u8] = match code {
        0 => b"Success\0",
        EAI_BADFLAGS => b"Bad value for ai_flags\0",
        EAI_NONAME => b"Name or service not known\0",
        EAI_AGAIN => b"Temporary failure in name resolution\0",
        EAI_FAIL => b"Non-recoverable failure in name resolution\0",
        EAI_FAMILY => b"ai_family not supported\0",
        EAI_SOCKTYPE => b"ai_socktype not supported\0",
        EAI_SERVICE => b"Servname not supported for ai_socktype\0",
        EAI_MEMORY => b"Memory allocation failure\0",
        EAI_SYSTEM => b"System error\0",
        _ => b"Unknown error\0",
    };
    s.as_ptr()
}
