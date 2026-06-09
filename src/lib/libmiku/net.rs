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

/// Create a TCP (AF_INET / SOCK_STREAM) socket. Returns an fd (>= 0) or a
/// negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_socket() -> i64 {
    unsafe { sc3(SYS_SOCKET, AF_INET as u64, SOCK_STREAM, 0) }
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
