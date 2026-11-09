// Userspace socket API (TCP client, phase 1)
//
// Thin wrappers over the socket syscalls. The fd returned by miku_socket is
// a normal descriptor usable with miku_read/miku_write/miku_close as well as
// the dedicated miku_send/miku_recv here.
//
// Example (C / Rust-FFI):
//   long fd = miku_socket();
//   unsigned char ip[4] = {93, 184, 216, 34};   // example.com
//   if (miku_connect(fd, ip, 80) == 0) {
//       miku_send(fd, req, req_len);
//       long n = miku_recv(fd, buf, sizeof buf);  // 0 = peer closed
//   }
//   miku_close(fd);

use crate::sys::*;

pub const AF_INET: u16 = miku_abi::socket::AF_INET as u16;
pub const SOCK_STREAM: u64 = miku_abi::socket::SOCK_STREAM as u64;
pub const SOCK_DGRAM: u64 = miku_abi::socket::SOCK_DGRAM as u64;
/// Whole IP packets. Only ICMP is allowed; raw IP of a chosen protocol would
/// put anything on the wire
pub const SOCK_RAW: u64 = miku_abi::socket::SOCK_RAW as u64;
pub const IPPROTO_ICMP: u64 = 1;

/// setsockopt level and name, the Linux numbers
pub const SOL_IP: u64 = 0;
pub const IP_TTL: u64 = miku_abi::inet::IP_TTL as u64;

/// Build the fixed 16-byte sockaddr_in the kernel expects
fn sockaddr_in(ip: *const u8, port: u16) -> [u8; 16] {
    let mut sa = [0u8; 16];
    sa[0] = (AF_INET & 0xff) as u8;
    sa[1] = (AF_INET >> 8) as u8;
    let pbe = port.to_be_bytes();
    sa[2] = pbe[0];
    sa[3] = pbe[1];
    if !ip.is_null() {
        unsafe {
            sa[4] = *ip.add(0);
            sa[5] = *ip.add(1);
            sa[6] = *ip.add(2);
            sa[7] = *ip.add(3);
        }
    }
    sa
}

/// Creates a TCP (AF_INET / SOCK_STREAM) socket. Returns an fd (>= 0) or a
/// negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socket() -> i64 {
    unsafe { sc3(SYS_SOCKET, AF_INET as u64, SOCK_STREAM, 0) }
}

/// Creates a UDP (AF_INET / SOCK_DGRAM) socket. Returns an fd (>= 0) or a
/// negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socket_udp() -> i64 {
    unsafe { sc3(SYS_SOCKET, AF_INET as u64, SOCK_DGRAM, 0) }
}

/// A raw ICMP socket: whole IP packets in, one ICMP message out. Unprivileged,
/// since the kernel writes the IP header. For `ping` and `traceroute`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socket_icmp() -> i64 {
    unsafe { sc3(SYS_SOCKET, AF_INET as u64, SOCK_RAW, IPPROTO_ICMP) }
}

/// Hop limit of this socket's packets, for traceroute
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_set_ttl(fd: i64, ttl: u8) -> i64 {
    unsafe { sc4(SYS_SETSOCKOPT, fd as u64, SOL_IP, IP_TTL, ttl as u64) }
}

/// Reads a socket option, `getsockopt`(114). The value is the return, or a
/// negative errno: `SOL_SOCKET`/`SO_ERROR` (reads and clears a non-blocking
/// connect's error), `SO_TYPE`, `SO_ACCEPTCONN`, and `SOL_IP`/`IP_TTL`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getsockopt(fd: i64, level: u64, name: u64) -> i64 {
    unsafe { sc3(SYS_GETSOCKOPT, fd as u64, level, name) }
}

/// Connects 'fd' to 'ip' (4 bytes, network order a.b.c.d) on 'port' (host
/// order). Returns 0 or a negative errno. Blocks, unless the descriptor is
/// `O_NONBLOCK`: then `-EINPROGRESS`, and `poll` says when done
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_connect(fd: i64, ip: *const u8, port: u16) -> i64 {
    if ip.is_null() {
        return -14; // EFAULT
    }
    // sockaddr_in: family(LE u16) | port(BE u16) | addr[4] | 8 zero bytes
    let mut sa = [0u8; 16];
    sa[0] = (AF_INET & 0xff) as u8;
    sa[1] = (AF_INET >> 8) as u8;
    let pbe = port.to_be_bytes();
    sa[2] = pbe[0];
    sa[3] = pbe[1];
    unsafe {
        sa[4] = *ip.add(0);
        sa[5] = *ip.add(1);
        sa[6] = *ip.add(2);
        sa[7] = *ip.add(3);
    }
    unsafe { sc3(SYS_CONNECT, fd as u64, sa.as_ptr() as u64, sa.len() as u64) }
}

/// Sends up to 'len' bytes. Returns bytes sent or a negative errno. Blocking
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_send(fd: i64, buf: *const u8, len: usize) -> i64 {
    unsafe { sc4(SYS_SEND, fd as u64, buf as u64, len as u64, 0) }
}

/// Receives up to 'len' bytes. Returns bytes read (0 = peer closed) or a
/// negative errno. Blocks until data arrives or the peer closes
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_recv(fd: i64, buf: *mut u8, len: usize) -> i64 {
    unsafe { sc4(SYS_RECV, fd as u64, buf as u64, len as u64, 0) }
}

/// Binds 'fd' to a local port (the IP part is ignored, single-homed stack).
/// Returns 0 or a negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_bind(fd: i64, port: u16) -> i64 {
    let sa = sockaddr_in(core::ptr::null(), port);
    unsafe { sc3(SYS_BIND, fd as u64, sa.as_ptr() as u64, sa.len() as u64) }
}

/// Mark a bound TCP socket as listening. Returns 0 or a negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_listen(fd: i64, backlog: i64) -> i64 {
    unsafe { sc2(SYS_LISTEN, fd as u64, backlog as u64) }
}

/// Accepts one connection on a listening socket. Blocks about 30 s, then
/// -EAGAIN. Returns the new fd; non-null 'ip'/'port' receive the peer address
/// (ip = 4 bytes)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_accept(fd: i64, ip: *mut u8, port: *mut u16) -> i64 {
    let mut sa = [0u8; 16];
    let r = unsafe { sc3(SYS_ACCEPT, fd as u64, sa.as_mut_ptr() as u64, sa.len() as u64) };
    if r >= 0 {
        if !ip.is_null() {
            unsafe { core::ptr::copy_nonoverlapping(sa[4..8].as_ptr(), ip, 4) };
        }
        if !port.is_null() {
            unsafe { *port = u16::from_be_bytes([sa[2], sa[3]]) };
        }
    }
    r
}

/// Sends one UDP datagram to ip:port, auto-binding an ephemeral local port on
/// first use. Returns bytes sent or a negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sendto(fd: i64, buf: *const u8, len: usize, ip: *const u8, port: u16) -> i64 {
    if ip.is_null() {
        return unsafe { sc4(SYS_SENDTO, fd as u64, buf as u64, len as u64, 0) };
    }
    let sa = sockaddr_in(ip, port);
    unsafe { sc4(SYS_SENDTO, fd as u64, buf as u64, len as u64, sa.as_ptr() as u64) }
}

/// Receives one UDP datagram. Blocks about 30 s, then -EAGAIN. Non-null
/// 'src_ip'/'src_port' receive the sender address
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_recvfrom(fd: i64, buf: *mut u8, len: usize, src_ip: *mut u8, src_port: *mut u16) -> i64 {
    let mut sa = [0u8; 16];
    let r = unsafe { sc4(SYS_RECVFROM, fd as u64, buf as u64, len as u64, sa.as_mut_ptr() as u64) };
    if r >= 0 {
        if !src_ip.is_null() {
            unsafe { core::ptr::copy_nonoverlapping(sa[4..8].as_ptr(), src_ip, 4) };
        }
        if !src_port.is_null() {
            unsafe { *src_port = u16::from_be_bytes([sa[2], sa[3]]) };
        }
    }
    r
}

// Local sockets: two processes talking through a byte queue. A named one
// reaches a service; a pair is for a parent and child

pub const AF_UNIX: u16 = miku_abi::socket::AF_UNIX as u16;

/// Longest path a local socket address can carry, as everywhere else
const UN_PATH_MAX: usize = 108;

/// Builds a sockaddr_un: family (LE u16) then a NUL-terminated path. Returns
/// the address and length, or length zero if the path does not fit. The kernel
/// distinguishes families by that first field
fn sockaddr_un(path: *const u8) -> ([u8; 2 + UN_PATH_MAX], u64) {
    let mut sa = [0u8; 2 + UN_PATH_MAX];
    sa[0] = (AF_UNIX & 0xff) as u8;
    sa[1] = (AF_UNIX >> 8) as u8;
    if path.is_null() {
        return (sa, 0);
    }
    let mut n = 0usize;
    unsafe {
        while n < UN_PATH_MAX - 1 {
            let c = *path.add(n);
            if c == 0 { break; }
            sa[2 + n] = c;
            n += 1;
        }
        if *path.add(n) != 0 {
            return (sa, 0); // path too long to name unambiguously
        }
    }
    if n == 0 {
        return (sa, 0);
    }
    (sa, (2 + n + 1) as u64)
}

/// Create an unconnected local (AF_UNIX / SOCK_STREAM) socket
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socket_unix() -> i64 {
    unsafe { sc3(SYS_SOCKET, AF_UNIX as u64, SOCK_STREAM, 0) }
}

/// Names a local socket in the filesystem. Returns 0 or a negative errno; the
/// name goes when the socket closes
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_bind_unix(fd: i64, path: *const u8) -> i64 {
    let (sa, len) = sockaddr_un(path);
    if len == 0 { return -22; } // EINVAL
    unsafe { sc3(SYS_BIND, fd as u64, sa.as_ptr() as u64, len) }
}

/// Connects to a named local socket, blocking until the server accepts or
/// clearly will not. Returns 0 or a negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_connect_unix(fd: i64, path: *const u8) -> i64 {
    let (sa, len) = sockaddr_un(path);
    if len == 0 { return -22; } // EINVAL
    unsafe { sc3(SYS_CONNECT, fd as u64, sa.as_ptr() as u64, len) }
}

/// A connected pair of local sockets with no name. `fds` receives two
/// descriptors. Nothing is looked up, so nothing can be squatted on, as suits a
/// parent and child
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socketpair(fds: *mut u64) -> i64 {
    if fds.is_null() { return -14; } // EFAULT
    unsafe { sc1(SYS_SOCKETPAIR, fds as u64) }
}
