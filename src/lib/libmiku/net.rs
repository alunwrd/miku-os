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

pub const AF_INET:     u16 = 2;
pub const SOCK_STREAM: u64 = 1;
pub const SOCK_DGRAM:  u64 = 2;

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

/// Create a TCP (AF_INET / SOCK_STREAM) socket. Returns an fd (>= 0) or a
/// negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socket() -> i64 {
    unsafe { sc3(SYS_SOCKET, AF_INET as u64, SOCK_STREAM, 0) }
}

/// Create a UDP (AF_INET / SOCK_DGRAM) socket. Returns an fd (>= 0) or a
/// negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socket_udp() -> i64 {
    unsafe { sc3(SYS_SOCKET, AF_INET as u64, SOCK_DGRAM, 0) }
}

/// Connect 'fd' to 'ip' (4 bytes, network order a.b.c.d) on 'port' (host
/// order). Builds a 16-byte sockaddr_in and issues the connect syscall.
/// Returns 0 on success or a negative errno. Blocking.
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

/// Send up to 'len' bytes. Returns bytes sent or a negative errno. Blocking.
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_send(fd: i64, buf: *const u8, len: usize) -> i64 {
    unsafe { sc4(SYS_SEND, fd as u64, buf as u64, len as u64, 0) }
}

/// Receive up to 'len' bytes. Returns bytes read (0 = peer closed) or a
/// negative errno. Blocking until data arrives or the peer closes.
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_recv(fd: i64, buf: *mut u8, len: usize) -> i64 {
    unsafe { sc4(SYS_RECV, fd as u64, buf as u64, len as u64, 0) }
}

/// Bind 'fd' to a local port (the IP part is ignored, single-homed stack).
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

/// Accept one connection on a listening socket. Blocking (about 30 s, then
/// -EAGAIN). On success returns the new fd; if 'ip'/'port' are non-null the
/// peer address is written there (ip = 4 bytes)
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

/// Send one UDP datagram to ip:port. Auto-binds an ephemeral local port on
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

/// Receive one UDP datagram. Blocking (about 30 s, then -EAGAIN). If
/// 'src_ip'/'src_port' are non-null the sender address is written there
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
