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
use crate::syscall::usercopy::{current_cr3, user_ptr_mapped, user_ptr_writable};

/// The process a socket belongs to: the calling task's thread group. A listener
/// bound on one thread can be accepted on by another, as every threaded server
/// does, and `close_all_for_pid(tgid)` finds it at exit. Descriptors are
/// process-wide
fn current_pid() -> u64 {
    crate::sched::current_tgid()
}
use crate::net::socket::{self, SockError};

const MAX_IO_LEN: u64 = 16 * 1024 * 1024;
const SOCKADDR_IN_LEN: u64 = 16;

#[inline]
fn e(code: i64) -> u64 { code as u64 }

/// Map a socket-layer error to a POSIX errno return value
#[inline]
fn sock_err(err: SockError) -> u64 {
    // One table, in the socket layer, because `SO_ERROR` hands out the same
    // numbers from there
    e(socket::sock_errno(err))
}

/// socket(domain, type, protocol) -> fd
pub fn sys_socket(domain: u64, sock_type: u64, protocol: u64) -> u64 {
    // AF_MAYLAND is a connection to the display server. It has no address, so
    // the socket comes back already connected and connect() has nothing to do
    if domain as u16 == socket::AF_MAYLAND {
        return match socket::create_mayland(current_pid()) {
            Some(fd) => fd,
            None => e(EMFILE),
        };
    }
    // A local socket has no protocol and one useful type: a stream. SOCK_DGRAM
    // over AF_UNIX is refused rather than half-implemented
    if domain as u16 == socket::AF_UNIX {
        if sock_type as u32 != socket::SOCK_STREAM {
            return e(EPROTONOSUPPORT);
        }
        return match socket::create_unix(current_pid()) {
            Some(fd) => fd,
            None => e(EMFILE),
        };
    }
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
        // Raw, and only ICMP. Raw IP with a protocol of the caller's choosing
        // would let a program forge anything; ICMP is what `ping` and
        // `traceroute` need. Unprivileged, like Linux's ping socket: the kernel
        // writes the IP header, so the source address cannot be a lie
        socket::SOCK_RAW => {
            if protocol as u32 != socket::IPPROTO_ICMP {
                return e(EPROTONOSUPPORT);
            }
            match socket::create_icmp(current_pid()) {
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
    let _ua = match user_ptr_mapped(cr3, addr_ptr, SOCKADDR_IN_LEN) {
        Some(g) => g,
        None => return Err(e(EFAULT)),
    };
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

/// The path out of a `sockaddr_un`: family (2 bytes) then a NUL-terminated
/// path, up to 108 bytes
///
/// None when the address is not AF_UNIX, so the caller falls through to the
/// sockaddr_in reader. The family field distinguishes them
fn read_sockaddr_un(addr_ptr: u64, addr_len: u64) -> Option<Result<alloc::string::String, u64>> {
    const UN_PATH_MAX: usize = 108;
    if addr_len < 3 { return None; }
    let cr3 = current_cr3();
    let take = (addr_len as usize).min(2 + UN_PATH_MAX);
    let _ua = match user_ptr_mapped(cr3, addr_ptr, take as u64) {
        Some(g) => g,
        None => return Some(Err(e(EFAULT))),
    };
    // Safe: range validated as mapped above
    let raw = unsafe { core::slice::from_raw_parts(addr_ptr as *const u8, take) };
    if u16::from_le_bytes([raw[0], raw[1]]) != socket::AF_UNIX {
        return None;
    }
    let bytes = &raw[2..];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    if end == 0 { return Some(Err(e(EINVAL))); }
    match core::str::from_utf8(&bytes[..end]) {
        Ok(p)  => Some(Ok(alloc::string::String::from(p))),
        Err(_) => Some(Err(e(EINVAL))),
    }
}

/// Write a sockaddr_in describing ip:port to user memory (if ptr is nonzero)
fn write_sockaddr_in(addr_ptr: u64, ip: [u8; 4], port: u16) -> Result<(), u64> {
    if addr_ptr == 0 {
        return Ok(());
    }
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, addr_ptr, SOCKADDR_IN_LEN) {
        Some(g) => g,
        None => return Err(e(EFAULT)),
    };
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
    if let Some(path) = read_sockaddr_un(addr_ptr, addr_len) {
        return match path.and_then(|p| socket::unix_connect(fd, &p).map_err(sock_err)) {
            Ok(()) => 0,
            Err(code) => code,
        };
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
/// ignored (no MSG_OOB / MSG_DONTWAIT)
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
    let _ua = match user_ptr_mapped(cr3, ptr, len) {
        Some(g) => g,
        None => return e(EFAULT),
    };
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
    let _ua = match user_ptr_writable(cr3, ptr, len) {
        Some(g) => g,
        None => return e(EFAULT),
    };
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
    if let Some(path) = read_sockaddr_un(addr_ptr, addr_len) {
        return match path.and_then(|p| socket::unix_bind(fd, &p).map_err(sock_err)) {
            Ok(()) => 0,
            Err(code) => code,
        };
    }
    let (_ip, port) = match read_sockaddr_in(addr_ptr, addr_len) {
        Ok(v) => v,
        Err(code) => return code,
    };
    // Ports below 1024 are reached by number rather than by being told: a
    // program binding 22, 80 or 443 can stand where a service is expected.
    // `CAP_NET_BIND` is that authority
    if port < 1024 && !crate::sched::current_cred().has(crate::process::cred::CAP_NET_BIND) {
        return e(EACCES);
    }
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
    if socket::is_local(fd) {
        return match socket::unix_listen(fd) {
            Ok(()) => 0,
            Err(err) => sock_err(err),
        };
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
    if socket::is_local(fd) {
        return match socket::unix_accept(fd, current_pid()) {
            // A local socket has no address to report, so the caller's sockaddr
            // is left untouched: zeroes would read as 0.0.0.0:0
            Ok(new_fd) => new_fd,
            Err(err) => sock_err(err),
        };
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
    let _ua = match user_ptr_mapped(cr3, ptr, len) {
        Some(g) => g,
        None => return e(EFAULT),
    };
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
    // An ICMP message has no port, so the address is only the host - and the
    // payload is the message itself rather than something to wrap in one
    if socket::is_icmp(fd) {
        return match socket::icmp_send(fd, ip, buf) {
            Ok(n) => n as u64,
            Err(err) => sock_err(err),
        };
    }
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
    let _ua = match user_ptr_writable(cr3, ptr, len) {
        Some(g) => g,
        None => return e(EFAULT),
    };
    let buf = unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) };
    if socket::is_icmp(fd) {
        return match socket::icmp_recv(fd, buf) {
            Ok((n, ip, port)) => {
                if let Err(code) = write_sockaddr_in(addr_ptr, ip, port) {
                    return code;
                }
                n as u64
            }
            Err(err) => sock_err(err),
        };
    }
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

/// socketpair(fds_ptr) -> 0 | errno
///
/// Two connected local sockets and no name, so nothing can squat on the address
/// or race for it: what a parent hands a child across a fork, and what a
/// library wants for a channel
pub fn sys_socketpair(fds_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, fds_ptr, 16) {
        Some(g) => g,
        None => return e(EFAULT),
    };
    let (a, b) = match socket::socketpair(current_pid()) {
        Some(pair) => pair,
        None => return e(EMFILE),
    };
    // Re-validated immediately before the store, as `sys_pipe` does: another
    // thread on this address space could have unmapped it meanwhile
    let _ua = match user_ptr_writable(cr3, fds_ptr, 16) {
        Some(g) => g,
        None => {
            socket::close_fd(a);
            socket::close_fd(b);
            return e(EFAULT);
        }
    };
    unsafe {
        let p = fds_ptr as *mut u64;
        p.write_unaligned(a);
        p.add(1).write_unaligned(b);
    }
    0
}

/// setsockopt(fd, level, optname, value) -> 0 | errno
///
/// IP_TTL, the method of traceroute, and TCP keepalive: `SO_KEEPALIVE` and
/// `TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT`, which every HTTP client with a
/// connection pool sets and `socket2` treats as a failure to report when
/// refused. The value is passed by register: every option here is a small
/// integer.
///
/// Linux's four arguments, so adding SO_RCVBUF or TCP_NODELAY fills in a match
/// arm
pub fn sys_setsockopt(fd: u64, level: u64, optname: u64, value: u64) -> u64 {
    if !socket::is_socket_fd(fd) || !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    match (level as u32, optname as u32) {
        (socket::SOL_IP, socket::IP_TTL) => {
            if value == 0 || value > 255 {
                return e(EINVAL);
            }
            match socket::set_ttl(fd, value as u8) {
                Ok(()) => 0,
                Err(err) => sock_err(err),
            }
        }
        (socket::SOL_SOCKET, socket::SO_KEEPALIVE)
        | (socket::IPPROTO_TCP, socket::TCP_KEEPIDLE | socket::TCP_KEEPINTVL | socket::TCP_KEEPCNT) => {
            match socket::set_keepalive_opt(fd, current_pid(), level as u32, optname as u32, value) {
                Ok(()) => 0,
                Err(SockError::Invalid) => e(ENOPROTOOPT),
                Err(err) => sock_err(err),
            }
        }
        _ => e(EINVAL),
    }
}

// Peer credentials and descriptor passing
//
// The two calls letting a privileged service run in ring 3 without taking
// anything its clients say on trust. AF_UNIX only: there is no process at the
// other end of a TCP connection, and a descriptor cannot be carried over a
// wire.

/// 83  peercred(fd, out) -> 0 | errno
///
/// Writes
/// `{pid: u64, start_tick: u64, uid: u32, gid: u32, euid: u32, egid: u32}`, 40
/// bytes, describing the process at the other end of a connected local socket,
/// as it was when the connection was made.
///
/// The basis of authentication for a local service. mikudod never reads a uid
/// from the request it is handed; it asks here. The kernel wrote this down
/// before the server was told of the connection.
///
/// The start tick is included because a pid alone is not an identity: a caller
/// storing a peer's identity and comparing later must compare both
pub fn sys_peercred(fd: u64, out: u64) -> u64 {
    use miku_abi::cred::MikuPeerCred;
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, out, core::mem::size_of::<MikuPeerCred>() as u64) {
        Some(g) => g,
        None => return e(EFAULT),
    };
    let pc = match socket::peer_cred(fd, current_pid()) {
        Some(c) => c,
        None    => return e(ENOTCONN),
    };
    let rec = MikuPeerCred {
        pid: pc.pid,
        start_tick: pc.start_tick,
        uid: pc.uid as u32,
        gid: pc.gid as u32,
        euid: pc.euid as u32,
        egid: pc.egid as u32,
        _reserved: 0,
    };
    unsafe { (out as *mut MikuPeerCred).write_unaligned(rec) };
    0
}

/// 84  sendfd(sockfd, fd) -> 0 | errno
///
/// Hands one open descriptor to the peer of a connected local socket. For a
/// program that has made a descriptor (a file, a pipe, a device node) and wants
/// the other end of a socket to have it. mikudo does not use this: fds 0, 1 and
/// 2 are a convention the syscall layer routes, and the password comes back
/// over the socket (`Reply::NeedPassword`).
///
/// A socket descriptor cannot be sent, only a VFS one: two processes would own
/// one slot in a table whose entries are freed by owner pid
pub fn sys_sendfd(sockfd: u64, fd: u64) -> u64 {
    if socket::is_socket_fd(fd) { return e(EINVAL); }
    let pid = current_pid();

    // Takes the reference first. If the push fails it is given back below; if
    // it succeeds the queue owns it, so the sender closing its copy at once
    // cannot pull the vnode from under the receiver
    let of = match crate::fs::vfs::core::with_vfs(|vfs| {
        let of = *vfs.fds().get(fd as usize).ok()?;
        let vid = of.vnode_id as usize;
        if !vfs.valid_vnode(vid) { return None; }
        vfs.nodes[vid].inc_ref();
        Some(of)
    }) {
        Some(of) => of,
        None     => return e(EBADF),
    };

    match socket::unix_push_fd(sockfd, pid, of) {
        Ok(())   => 0,
        Err(err) => {
            crate::fs::vfs::core::with_vfs(|vfs| {
                let vid = of.vnode_id as usize;
                vfs.drop_ref(vid);
            });
            sock_err(err)
        }
    }
}

/// 85  recvfd(sockfd) -> fd | errno
///
/// Takes the oldest descriptor the peer sent. `-EAGAIN` when none is waiting.
/// It never blocks: the bytes saying what a descriptor is for travel alongside
/// it.
///
/// The descriptor lands in the lowest free slot, as `open` and `dup` place
/// theirs
pub fn sys_recvfd(sockfd: u64) -> u64 {
    let pid = current_pid();
    let of = match socket::unix_take_fd(sockfd, pid) {
        Ok(of)   => of,
        Err(err) => return sock_err(err),
    };
    match crate::fs::vfs::core::with_vfs(|vfs| vfs.fds().alloc(of.vnode_id, of.flags)) {
        Ok(newfd) => {
            // alloc() starts a fresh description at offset 0; the sender's
            // position is what was meant to come across
            crate::fs::vfs::core::with_vfs(|vfs| {
                if let Ok(mut slot) = vfs.fds().get_mut(newfd) {
                    slot.offset = of.offset;
                }
            });
            newfd as u64
        }
        Err(_) => {
            // No room: the reference the queue held is ours now, and dropping
            // it would leak the vnode
            crate::fs::vfs::core::with_vfs(|vfs| {
                let vid = of.vnode_id as usize;
                vfs.drop_ref(vid);
            });
            e(EMFILE)
        }
    }
}

/// getsockopt(fd, level, optname) -> value | errno  (114)
///
/// The counterpart to 80: every option is a small integer, so the answer is the
/// return value. `SO_ERROR` reads and clears the error a non-blocking connect
/// left behind, which an async runtime asks once the socket turns writable
pub fn sys_getsockopt(fd: u64, level: u64, optname: u64) -> u64 {
    if !socket::is_socket_fd(fd) || !socket::owned_by(fd, current_pid()) {
        return e(EBADF);
    }
    match socket::getsockopt(fd, current_pid(), level as u32, optname as u32) {
        Ok(v) => v,
        Err(SockError::Invalid) => e(ENOPROTOOPT),
        Err(err) => sock_err(err),
    }
}
