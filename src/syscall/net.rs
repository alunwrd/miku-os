// Socket syscalls (userspace networking)
//
// Thin syscall wrappers over the kernel socket layer in net::socket. The fd
// returned by sys_socket lives in the socket range (SOCK_FD_BASE..) and is
// usable with send/recv as well as the generic read/write/close, which route
// to this module by fd range (see io.rs / file.rs).
//
// Address format is the standard 16-byte sockaddr_in:
//   off 0: sa_family  u16 (host order; AF_INET = 2)
//   off 2: sin_port   u16 (network/big-endian)
//   off 4: sin_addr   [u8; 4] (network order, i.e. a.b.c.d as written)
//   off 8: 8 bytes zero padding

use super::errno::*;
use super::user_mem::{current_cr3, current_pid, user_ptr_mapped, user_ptr_writable};
use crate::net::socket::{self, SockError};

const MAX_IO_LEN: u64 = 16 * 1024 * 1024;
const SOCKADDR_IN_LEN: u64 = 16;

#[inline]
fn e(code: i64) -> u64 { code as u64 }

/// Map a socket-layer error to a POSIX errno return value
#[inline]
fn sock_err(err: SockError) -> u64 {
    let code = match err {
        SockError::BadFd            => EBADF,
        SockError::NotConnected     => ENOTCONN,
        SockError::AlreadyConnected => EISCONN,
        SockError::Busy             => EBUSY,
        SockError::ConnRefused      => ECONNREFUSED,
        SockError::BrokenPipe       => EPIPE,
        SockError::ConnReset        => ECONNRESET,
    };
    e(code)
}

/// socket(domain, type, protocol) -> fd
pub fn sys_socket(domain: u64, sock_type: u64, protocol: u64) -> u64 {
    if domain as u16 != socket::AF_INET {
        return e(EAFNOSUPPORT);
    }
    match sock_type as u32 {
        socket::SOCK_STREAM => {
            // 0 (default) or IPPROTO_TCP are both fine for a stream socket
            if protocol != 0 && protocol as u32 != socket::IPPROTO_TCP {
                return e(EPROTONOSUPPORT);
            }
            match socket::create_tcp(current_pid()) {
                Some(fd) => fd,
                None => e(EMFILE),
            }
        }
        // UDP / datagram sockets are not wired yet
        socket::SOCK_DGRAM => e(EPROTONOSUPPORT),
        _ => e(EPROTONOSUPPORT),
    }
}

/// Read and validate a sockaddr_in from user memory, returning (ip, port)
fn read_sockaddr_in(addr_ptr: u64, addr_len: u64) -> Result<([u8; 4], u16), u64> {
    if addr_len < SOCKADDR_IN_LEN {
        return Err(e(EINVAL));
    }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, addr_ptr, SOCKADDR_IN_LEN) {
        return Err(e(EFAULT));
    }
    // Safe: range validated as mapped for SOCKADDR_IN_LEN bytes above
    let raw = unsafe {
        core::slice::from_raw_parts(addr_ptr as *const u8, SOCKADDR_IN_LEN as usize)
    };
    let family = u16::from_le_bytes([raw[0], raw[1]]);
    if family != socket::AF_INET {
        return Err(e(EAFNOSUPPORT));
    }
    let port = u16::from_be_bytes([raw[2], raw[3]]);
    let ip = [raw[4], raw[5], raw[6], raw[7]];
    Ok((ip, port))
}

/// connect(fd, sockaddr*, addrlen) -> 0 | errno
pub fn sys_connect(fd: u64, addr_ptr: u64, addr_len: u64) -> u64 {
    if !socket::is_socket_fd(fd) {
        return e(EBADF);
    }
    if !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    let (ip, port) = match read_sockaddr_in(addr_ptr, addr_len) {
        Ok(v) => v,
        Err(code) => return code,
    };
    match socket::connect(fd, ip, port) {
        Ok(()) => 0,
        Err(err) => sock_err(err),
    }
}

/// send(fd, buf, len, flags) -> bytes_sent | errno. flags are accepted but
/// ignored in phase 1 (no MSG_OOB / MSG_DONTWAIT yet)
pub fn sys_send(fd: u64, ptr: u64, len: u64, _flags: u64) -> u64 {
    if !socket::is_socket_fd(fd) {
        return e(EBADF);
    }
    if !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    if len == 0 {
        return 0;
    }
    if len > MAX_IO_LEN {
        return e(EINVAL);
    }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, ptr, len) {
        return e(EFAULT);
    }
    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    match socket::send(fd, buf) {
        Ok(n) => n as u64,
        Err(err) => sock_err(err),
    }
}

/// recv(fd, buf, len, flags) -> bytes_read (0 = peer closed) | errno
pub fn sys_recv(fd: u64, ptr: u64, len: u64, _flags: u64) -> u64 {
    if !socket::is_socket_fd(fd) {
        return e(EBADF);
    }
    if !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    if len == 0 {
        return 0;
    }
    if len > MAX_IO_LEN {
        return e(EINVAL);
    }
    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, ptr, len) {
        return e(EFAULT);
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) };
    match socket::recv(fd, buf) {
        Ok(n) => n as u64,
        Err(err) => sock_err(err),
    }
}
