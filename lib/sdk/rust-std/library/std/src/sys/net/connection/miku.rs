//! `std::net` on MikuOS.
//!
//! The kernel's sockets are BSD-shaped (`socket`(56), `connect`(57),
//! `bind`(62), `listen`(63), `accept`(64)), and `libmiku` wraps them in a
//! narrower interface than a `sockaddr`: four address bytes and a port, since
//! the stack is IPv4. IPv6 is refused by name here.
//!
//! - There is no `getsockname` or `getpeername`. The addresses a socket knows
//!   are the ones it was given, remembered here. `bind` does not choose a port,
//!   so a `TcpListener` on port 0 is refused.
//! - A socket is a descriptor, above `SOCK_FD_BASE` in a table of its own,
//!   reachable by `read`, `write`, `close`, `poll` and `fcntl`(113), which
//!   keeps `O_NONBLOCK` and `FD_CLOEXEC` on the socket's slot. So `Socket` is a
//!   `FileDesc`, and `AsFd`/`OwnedFd` work as on any Unix. `dup`(28) makes an
//!   alias slot in the socket table, so every `try_clone` works. A non-blocking
//!   TCP `connect` answers `EINPROGRESS`, turns writable when done and leaves
//!   any failure in `SO_ERROR`, which `getsockopt`(114) and `take_error` read.
//! - A socket made from a raw descriptor has no addresses;
//!   `peer_addr`/`socket_addr` answer with an error saying so.

use crate::fmt;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, ToSocketAddrs};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, RawFd};
use crate::sys::fd::FileDesc;
use crate::sys::pal::{abi, cvt, unsupported};
use crate::sys::{AsInner, FromInner, IntoInner};
use crate::time::Duration;

/// A socket: a descriptor, closed when dropped. `close`(12) routes one above
/// `SOCK_FD_BASE` to the socket layer
pub struct Socket(FileDesc);

impl Socket {
    fn new(fd: i64) -> Socket {
        // SAFETY: just handed over by the kernel, owned by nobody else
        Socket(unsafe { FileDesc::from_raw_fd(fd as RawFd) })
    }

    /// The number, for the `libmiku` calls that take one
    fn raw(&self) -> i64 {
        self.0.as_raw_fd() as i64
    }

    fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        self.0.set_nonblocking(on)
    }

    /// `dup`(28): an alias descriptor for the same socket, released with the
    /// last of them
    fn duplicate(&self) -> io::Result<Socket> {
        Ok(Socket(self.0.duplicate()?))
    }

    /// `SO_ERROR` through `getsockopt`(114), which reads and clears it
    fn take_error(&self) -> io::Result<Option<io::Error>> {
        let e = cvt(abi::miku_getsockopt(self.raw(), SOL_SOCKET, SO_ERROR))?;
        Ok(if e == 0 { None } else { Some(io::Error::from_raw_os_error(e as i32)) })
    }

    fn ttl(&self) -> io::Result<u32> {
        Ok(cvt(abi::miku_getsockopt(self.raw(), SOL_IP, IP_TTL))? as u32)
    }
}

const SOL_SOCKET: u64 = crate::sys::pal::abi::defs::socket::SOL_SOCKET as u64;
const SO_ERROR: u64 = crate::sys::pal::abi::defs::socket::SO_ERROR as u64;
const SOL_IP: u64 = 0;
const IP_TTL: u64 = crate::sys::pal::abi::defs::inet::IP_TTL as u64;
use crate::sys::pal::abi::defs::errno::EINPROGRESS;

impl AsInner<FileDesc> for Socket {
    #[inline]
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl IntoInner<FileDesc> for Socket {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for Socket {
    fn from_inner(fd: FileDesc) -> Socket {
        Socket(fd)
    }
}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for Socket {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

/// What a socket built from a raw descriptor says when asked an address it
/// was never told
fn unknown_addr() -> io::Error {
    io::const_error!(
        io::ErrorKind::Unsupported,
        "address not known: MikuOS has no getsockname/getpeername, and this socket was not opened through std",
    )
}

/// The four bytes and the port, or an error naming IPv6 as the reason.
fn v4(addr: &SocketAddr) -> io::Result<([u8; 4], u16)> {
    match addr {
        SocketAddr::V4(a) => Ok((a.ip().octets(), a.port())),
        SocketAddr::V6(_) => Err(io::const_error!(
            io::ErrorKind::Unsupported,
            "MikuOS's network stack is IPv4 only",
        )),
    }
}

fn addr_of(ip: [u8; 4], port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), port))
}

/// The first address a `ToSocketAddrs` yields, or an error.
fn first_addr<A: ToSocketAddrs>(a: A) -> io::Result<SocketAddr> {
    a.to_socket_addrs()?.next().ok_or_else(|| {
        io::const_error!(io::ErrorKind::InvalidInput, "no socket addresses to connect to")
    })
}

pub struct TcpStream {
    sock: Socket,
    peer: Option<SocketAddr>,
}

impl TcpStream {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        let addr = first_addr(addr)?;
        let (ip, port) = v4(&addr)?;
        let sock = Socket::new(cvt(abi::miku_socket())?);
        cvt(abi::miku_connect(sock.raw(), ip.as_ptr(), port))?;
        Ok(TcpStream { sock, peer: Some(addr) })
    }

    /// The Unix shape: a non-blocking connect, `poll` for writable within the
    /// budget, `SO_ERROR` for the outcome, then blocking again
    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        if timeout.is_zero() {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "cannot set a 0 duration timeout",
            ));
        }
        let (ip, port) = v4(addr)?;
        let sock = Socket::new(cvt(abi::miku_socket())?);
        sock.set_nonblocking(true)?;
        match cvt(abi::miku_connect(sock.raw(), ip.as_ptr(), port)) {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(EINPROGRESS) => {
                let mut pfd = abi::PollFd { fd: sock.raw() as i32, events: abi::POLLOUT, revents: 0 };
                let ms = timeout.as_millis().clamp(1, i64::MAX as u128) as i64;
                let n = cvt(abi::miku_poll(&mut pfd, 1, ms))?;
                if n == 0 {
                    return Err(io::const_error!(io::ErrorKind::TimedOut, "connection timed out"));
                }
                if let Some(e) = sock.take_error()? {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
        sock.set_nonblocking(false)?;
        Ok(TcpStream { sock, peer: Some(*addr) })
    }

    pub fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }
    pub fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    pub fn peek(&self, _: &mut [u8]) -> io::Result<usize> {
        // `recv` has no MSG_PEEK on this stack, and reading would lose the
        // bytes
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_recv(self.sock.raw(), buf.as_mut_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        io::default_read_buf(|b| self.read(b), cursor)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        io::default_read_vectored(|b| self.read(b), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_send(self.sock.raw(), buf.as_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        io::default_write_vectored(|b| self.write(b), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.peer.ok_or_else(unknown_addr)
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        // No `getsockname`. The stack chose the local end and told nobody
        unsupported()
    }

    pub fn shutdown(&self, _: Shutdown) -> io::Result<()> {
        // No half-close in the syscall table, and `close` here would close a
        // socket the caller still holds
        unsupported()
    }

    pub fn duplicate(&self) -> io::Result<TcpStream> {
        Ok(TcpStream { sock: self.sock.duplicate()?, peer: self.peer })
    }

    pub fn socket(&self) -> &Socket {
        &self.sock
    }

    pub fn into_socket(self) -> Socket {
        self.sock
    }

    pub fn set_linger(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }
    pub fn linger(&self) -> io::Result<Option<Duration>> {
        unsupported()
    }
    pub fn set_keepalive(&self, _: bool) -> io::Result<()> {
        unsupported()
    }
    pub fn keepalive(&self) -> io::Result<bool> {
        unsupported()
    }
    // The kernel's TCP has no Nagle: a write goes out as soon as the window
    // lets it (`pump` in `lib/net/src/stack/conn.rs`). So every socket is
    // already "no delay"
    pub fn set_nodelay(&self, on: bool) -> io::Result<()> {
        if on { Ok(()) } else { unsupported() }
    }
    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(true)
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        cvt(abi::miku_set_ttl(self.sock.raw(), ttl.min(255) as u8))?;
        Ok(())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.sock.ttl()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sock.take_error()
    }

    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        self.sock.set_nonblocking(on)
    }
}

impl FromInner<Socket> for TcpStream {
    fn from_inner(sock: Socket) -> TcpStream {
        TcpStream { sock, peer: None }
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpStream").field("peer", &self.peer).finish()
    }
}

pub struct TcpListener {
    sock: Socket,
    local: Option<SocketAddr>,
}

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        let addr = first_addr(addr)?;
        let (_ip, port) = v4(&addr)?;
        if port == 0 {
            // `bind`(62) takes a port and does not pick one, and there is no
            // `getsockname` to ask afterwards, so "any port" is refused
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "MikuOS cannot bind to port 0: the kernel does not choose a port",
            ));
        }
        let sock = Socket::new(cvt(abi::miku_socket())?);
        // The address is not passed: this stack binds a port on every interface
        cvt(abi::miku_bind(sock.raw(), port))?;
        cvt(abi::miku_listen(sock.raw(), 8))?;
        Ok(TcpListener { sock, local: Some(addr) })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.local.ok_or_else(unknown_addr)
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let mut ip = [0u8; 4];
        let mut port: u16 = 0;
        let fd = cvt(abi::miku_accept(self.sock.raw(), ip.as_mut_ptr(), &mut port))?;
        let peer = addr_of(ip, port);
        Ok((TcpStream { sock: Socket::new(fd), peer: Some(peer) }, peer))
    }

    pub fn duplicate(&self) -> io::Result<TcpListener> {
        Ok(TcpListener { sock: self.sock.duplicate()?, local: self.local })
    }

    pub fn socket(&self) -> &Socket {
        &self.sock
    }

    pub fn into_socket(self) -> Socket {
        self.sock
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        cvt(abi::miku_set_ttl(self.sock.raw(), ttl.min(255) as u8))?;
        Ok(())
    }
    pub fn ttl(&self) -> io::Result<u32> {
        self.sock.ttl()
    }
    pub fn set_only_v6(&self, _: bool) -> io::Result<()> {
        unsupported()
    }
    pub fn only_v6(&self) -> io::Result<bool> {
        unsupported()
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sock.take_error()
    }
    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        self.sock.set_nonblocking(on)
    }
}

impl FromInner<Socket> for TcpListener {
    fn from_inner(sock: Socket) -> TcpListener {
        TcpListener { sock, local: None }
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpListener").field("local", &self.local).finish()
    }
}

pub struct UdpSocket {
    sock: Socket,
    local: Option<SocketAddr>,
    peer: Option<SocketAddr>,
}

impl UdpSocket {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<UdpSocket> {
        let addr = first_addr(addr)?;
        let (_ip, port) = v4(&addr)?;
        let sock = Socket::new(cvt(abi::miku_socket_udp())?);
        if port != 0 {
            cvt(abi::miku_bind(sock.raw(), port))?;
        }
        Ok(UdpSocket { sock, local: Some(addr), peer: None })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.peer.ok_or_else(|| {
            io::const_error!(io::ErrorKind::NotConnected, "socket is not connected")
        })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.local.ok_or_else(unknown_addr)
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut ip = [0u8; 4];
        let mut port: u16 = 0;
        let n = cvt(abi::miku_recvfrom(
            self.sock.raw(),
            buf.as_mut_ptr(),
            buf.len(),
            ip.as_mut_ptr(),
            &mut port,
        ))?;
        Ok((n as usize, addr_of(ip, port)))
    }

    pub fn peek_from(&self, _: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        unsupported()
    }

    pub fn send_to(&self, buf: &[u8], addr: &SocketAddr) -> io::Result<usize> {
        let (ip, port) = v4(addr)?;
        let n = cvt(abi::miku_sendto(self.sock.raw(), buf.as_ptr(), buf.len(), ip.as_ptr(), port))?;
        Ok(n as usize)
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        Ok(UdpSocket { sock: self.sock.duplicate()?, local: self.local, peer: self.peer })
    }

    pub fn socket(&self) -> &Socket {
        &self.sock
    }

    pub fn into_socket(self) -> Socket {
        self.sock
    }

    pub fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }
    pub fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }
    pub fn set_broadcast(&self, _: bool) -> io::Result<()> {
        unsupported()
    }
    pub fn broadcast(&self) -> io::Result<bool> {
        unsupported()
    }
    pub fn set_multicast_loop_v4(&self, _: bool) -> io::Result<()> {
        unsupported()
    }
    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        unsupported()
    }
    pub fn set_multicast_ttl_v4(&self, _: u32) -> io::Result<()> {
        unsupported()
    }
    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        unsupported()
    }
    pub fn set_multicast_loop_v6(&self, _: bool) -> io::Result<()> {
        unsupported()
    }
    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        unsupported()
    }
    pub fn join_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        unsupported()
    }
    pub fn join_multicast_v6(&self, _: &crate::net::Ipv6Addr, _: u32) -> io::Result<()> {
        unsupported()
    }
    pub fn leave_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        unsupported()
    }
    pub fn leave_multicast_v6(&self, _: &crate::net::Ipv6Addr, _: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        cvt(abi::miku_set_ttl(self.sock.raw(), ttl.min(255) as u8))?;
        Ok(())
    }
    pub fn ttl(&self) -> io::Result<u32> {
        self.sock.ttl()
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sock.take_error()
    }
    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        self.sock.set_nonblocking(on)
    }

    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_recv(self.sock.raw(), buf.as_mut_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    pub fn peek(&self, _: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_send(self.sock.raw(), buf.as_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        let addr = first_addr(addr)?;
        let (ip, port) = v4(&addr)?;
        cvt(abi::miku_connect(self.sock.raw(), ip.as_ptr(), port))?;
        Ok(())
    }
}

impl FromInner<Socket> for UdpSocket {
    fn from_inner(sock: Socket) -> UdpSocket {
        UdpSocket { sock, local: None, peer: None }
    }
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpSocket").field("local", &self.local).finish()
    }
}

/// Local sockets, for `std::os::miku::net`, which is outside `sys` and cannot
/// reach `abi`. Descriptors come back owned; paths go in NUL-terminated,
/// checked by the caller.
pub mod unix {
    use super::*;

    pub fn stream() -> io::Result<FileDesc> {
        Ok(Socket::new(cvt(abi::miku_socket_unix())?).into_inner())
    }

    pub fn bind_listen(fd: &FileDesc, path: &[u8], backlog: i64) -> io::Result<()> {
        cvt(abi::miku_bind_unix(fd.as_raw_fd() as i64, path.as_ptr()))?;
        cvt(abi::miku_listen(fd.as_raw_fd() as i64, backlog))?;
        Ok(())
    }

    pub fn connect(fd: &FileDesc, path: &[u8]) -> io::Result<()> {
        cvt(abi::miku_connect_unix(fd.as_raw_fd() as i64, path.as_ptr()))?;
        Ok(())
    }

    /// The next connection. A local socket reports no peer address, so the two
    /// out-parameters `accept` fills for TCP stay as they were
    pub fn accept(fd: &FileDesc) -> io::Result<FileDesc> {
        let mut ip = [0u8; 4];
        let mut port: u16 = 0;
        let n = cvt(abi::miku_accept(fd.as_raw_fd() as i64, ip.as_mut_ptr(), &mut port))?;
        Ok(Socket::new(n).into_inner())
    }

    pub fn pair() -> io::Result<(FileDesc, FileDesc)> {
        let mut fds = [0u64; 2];
        cvt(abi::miku_socketpair(fds.as_mut_ptr()))?;
        Ok((Socket::new(fds[0] as i64).into_inner(), Socket::new(fds[1] as i64).into_inner()))
    }
}

/// Names, through `libc_miku.so`'s `getaddrinfo`.
///
/// The resolver a C program gets: an address literal, `localhost`,
/// `/etc/hosts`, then one DNS query to the resolver the DHCP lease named
/// (`lib/runtime/libmiku/libc/netdb.rs`, wire format in `mikunet`'s `dns_wire`). IPv4
/// only, as the stack is.
pub struct LookupHost(crate::vec::IntoIter<SocketAddr>);

impl Iterator for LookupHost {
    type Item = SocketAddr;
    fn next(&mut self) -> Option<SocketAddr> {
        self.0.next()
    }
}

#[repr(C)]
struct AddrInfo {
    ai_flags: i32,
    ai_family: i32,
    ai_socktype: i32,
    ai_protocol: i32,
    ai_addrlen: u32,
    ai_addr: *mut u8,
    ai_canonname: *mut u8,
    ai_next: *mut AddrInfo,
}

unsafe extern "C" {
    fn getaddrinfo(node: *const u8, service: *const u8, hints: *const AddrInfo, res: *mut *mut AddrInfo) -> i32;
    fn freeaddrinfo(res: *mut AddrInfo);
    fn gai_strerror(code: i32) -> *const u8;
}

pub fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    let c_host = crate::ffi::CString::new(host)
        .map_err(|_| io::const_error!(io::ErrorKind::InvalidInput, "host name contains a NUL byte"))?;
    let hints = AddrInfo {
        ai_flags: 0,
        ai_family: 2, // AF_INET
        ai_socktype: 1, // SOCK_STREAM: one answer per address, not one per type
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_addr: crate::ptr::null_mut(),
        ai_canonname: crate::ptr::null_mut(),
        ai_next: crate::ptr::null_mut(),
    };
    let mut res: *mut AddrInfo = crate::ptr::null_mut();
    let rc = unsafe { getaddrinfo(c_host.as_ptr().cast(), crate::ptr::null(), &hints, &mut res) };
    if rc != 0 {
        if rc == -11 {
            // EAI_SYSTEM: errno says what
            return Err(io::Error::last_os_error());
        }
        let msg = unsafe { crate::ffi::CStr::from_ptr(gai_strerror(rc).cast()) };
        return Err(io::Error::new(
            io::ErrorKind::Uncategorized,
            crate::format!("failed to lookup address information: {}", msg.to_string_lossy()),
        ));
    }
    let mut out = crate::vec::Vec::new();
    let mut cur = res;
    while !cur.is_null() {
        let ai = unsafe { &*cur };
        if ai.ai_family == 2 && ai.ai_addrlen >= 8 && !ai.ai_addr.is_null() {
            let mut ip = [0u8; 4];
            unsafe { crate::ptr::copy_nonoverlapping(ai.ai_addr.add(4), ip.as_mut_ptr(), 4) };
            out.push(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), port)));
        }
        cur = ai.ai_next;
    }
    unsafe { freeaddrinfo(res) };
    Ok(LookupHost(out.into_iter()))
}
