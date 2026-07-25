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

use crate::syscall::errno::*;
use crate::syscall::usercopy::{current_cr3, current_pid, user_ptr_mapped, user_ptr_writable};
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
        SockError::AddrInUse        => EADDRINUSE,
        SockError::WouldBlock       => EAGAIN,
        SockError::Invalid          => EINVAL,
        SockError::NotSupported     => EOPNOTSUPP,
        SockError::TableFull        => EMFILE,
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
        socket::SOCK_DGRAM => {
            if protocol != 0 && protocol as u32 != socket::IPPROTO_UDP {
                return e(EPROTONOSUPPORT);
            }
            match socket::create_udp(current_pid()) {
                Some(fd) => fd,
                None => e(EMFILE),
            }
        }
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

/// Write a sockaddr_in describing ip:port to user memory (if ptr is nonzero)
fn write_sockaddr_in(addr_ptr: u64, ip: [u8; 4], port: u16) -> Result<(), u64> {
    if addr_ptr == 0 {
        return Ok(());
    }
    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, addr_ptr, SOCKADDR_IN_LEN) {
        return Err(e(EFAULT));
    }
    let mut sa = [0u8; SOCKADDR_IN_LEN as usize];
    sa[0..2].copy_from_slice(&socket::AF_INET.to_le_bytes());
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&ip);
    // Safe: range validated as writable above
    unsafe {
        core::ptr::copy_nonoverlapping(sa.as_ptr(), addr_ptr as *mut u8, sa.len());
    }
    Ok(())
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

/// bind(fd, sockaddr*, addrlen) -> 0 | errno. The address part is ignored
/// (single-homed stack); only sin_port matters
pub fn sys_bind(fd: u64, addr_ptr: u64, addr_len: u64) -> u64 {
    if !socket::is_socket_fd(fd) || !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    let (_ip, port) = match read_sockaddr_in(addr_ptr, addr_len) {
        Ok(v) => v,
        Err(code) => return code,
    };
    match socket::bind(fd, port) {
        Ok(()) => 0,
        Err(err) => sock_err(err),
    }
}

/// listen(fd, backlog) -> 0 | errno. The backlog hint is accepted but the
/// demux uses its own fixed queue depth
pub fn sys_listen(fd: u64, _backlog: u64) -> u64 {
    if !socket::is_socket_fd(fd) || !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    match socket::listen(fd) {
        Ok(()) => 0,
        Err(err) => sock_err(err),
    }
}

/// accept(fd, sockaddr* out, addrlen) -> new fd | errno. Blocking. Writes
/// the peer address to addr_ptr when it is nonzero
pub fn sys_accept(fd: u64, addr_ptr: u64, _addr_len: u64) -> u64 {
    if !socket::is_socket_fd(fd) || !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    match socket::accept(fd, current_pid()) {
        Ok((new_fd, ip, port)) => {
            if let Err(code) = write_sockaddr_in(addr_ptr, ip, port) {
                socket::close_fd(new_fd);
                return code;
            }
            new_fd
        }
        Err(err) => sock_err(err),
    }
}

/// sendto(fd, buf, len, sockaddr*) -> bytes | errno. The 4-arg syscall ABI
/// has no room for flags/addrlen; the sockaddr is the fixed 16-byte
/// sockaddr_in. A null sockaddr behaves like send() (connected UDP)
pub fn sys_sendto(fd: u64, ptr: u64, len: u64, addr_ptr: u64) -> u64 {
    if !socket::is_socket_fd(fd) || !socket::owned_by(fd, current_pid()) {
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

    if addr_ptr == 0 {
        return match socket::send(fd, buf) {
            Ok(n) => n as u64,
            Err(err) => sock_err(err),
        };
    }
    let (ip, port) = match read_sockaddr_in(addr_ptr, SOCKADDR_IN_LEN) {
        Ok(v) => v,
        Err(code) => return code,
    };
    match socket::sendto(fd, ip, port, buf) {
        Ok(n) => n as u64,
        Err(err) => sock_err(err),
    }
}

/// recvfrom(fd, buf, len, sockaddr* out) -> bytes | errno. Blocking. Writes
/// the datagram source to addr_ptr when it is nonzero
pub fn sys_recvfrom(fd: u64, ptr: u64, len: u64, addr_ptr: u64) -> u64 {
    if !socket::is_socket_fd(fd) || !socket::owned_by(fd, current_pid()) {
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
    match socket::recvfrom(fd, buf) {
        Ok((n, ip, port)) => {
            if let Err(code) = write_sockaddr_in(addr_ptr, ip, port) {
                return code;
            }
            n as u64
        }
        Err(err) => sock_err(err),
    }
}
