//! Local sockets: `std::os::unix::net` on MikuOS.
//!
//! The kernel's `AF_UNIX` is a stream socket named by a path
//! (`docs/MikuOS_ABI.md` §3.4): `bind` creates the node and closing the socket
//! removes it, `connect` blocks until the server accepts, and `socketpair`(77)
//! gives a connected pair with no name.
//!
//! Refused:
//!
//! - No datagrams. `socket(AF_UNIX, SOCK_DGRAM)` answers `EPROTONOSUPPORT`, so
//!   `UnixDatagram`'s constructors do too. The type exists because crates name
//!   it (`async-io` wraps all three); one comes only from a descriptor made
//!   elsewhere.
//! - No `getsockname`/`getpeername`. A listener remembers the path it was bound
//!   to and a stream the path it connected to. Anything else is unnamed: the
//!   server's end of an accepted stream, either end of a pair, a socket from a
//!   raw descriptor.
//! - No abstract namespace, no `SO_PEERCRED` through here, no ancillary data.
//!   `peercred`(83) and `sendfd`(84)/`recvfd`(85) exist in MikuOS's own shape.
//! - No timeouts, no `shutdown`, no `peek`, as in `std::net` here. `try_clone`
//!   works: `dup`(28) on a socket makes a second descriptor.

#![stable(feature = "unix_socket", since = "1.10.0")]

use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::os::miku::ffi::OsStrExt;
use crate::path::{Path, PathBuf};
use crate::sys::fd::FileDesc;
use crate::sys::net::unix as imp;
use crate::sys::{FromInner, IntoInner};
use crate::time::Duration;
use crate::{fmt, io};

/// `sun_path` is 108 bytes with a terminator, so a path is at most 107
const SUN_PATH_MAX: usize = 107;

/// A path as the `libmiku` calls take it: NUL-terminated and short enough for a
/// `sockaddr_un`
fn cpath(p: &Path) -> io::Result<Vec<u8>> {
    let b = p.as_os_str().as_bytes();
    if b.contains(&0) {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "path contains a NUL byte"));
    }
    if b.is_empty() || b.len() > SUN_PATH_MAX {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "a local socket's path is 1 to 107 bytes",
        ));
    }
    let mut v = Vec::with_capacity(b.len() + 1);
    v.extend_from_slice(b);
    v.push(0);
    Ok(v)
}

fn no_datagrams<T>() -> io::Result<T> {
    Err(io::const_error!(
        io::ErrorKind::Unsupported,
        "MikuOS's local sockets are streams only: AF_UNIX SOCK_DGRAM is not implemented",
    ))
}

fn not_here<T>() -> io::Result<T> {
    Err(io::Error::UNSUPPORTED_PLATFORM)
}

/// The address of a local socket: a path, or nothing.
#[derive(Clone)]
#[stable(feature = "unix_socket", since = "1.10.0")]
pub struct SocketAddr {
    path: Option<PathBuf>,
}

impl SocketAddr {
    fn unnamed() -> SocketAddr {
        SocketAddr { path: None }
    }

    /// An address for a path, checked as `bind` would check it, except that an
    /// empty path is the unnamed address, as on Linux. `mio` builds every
    /// accepted peer's address this way
    #[stable(feature = "unix_socket_creation", since = "1.61.0")]
    pub fn from_pathname<P: AsRef<Path>>(path: P) -> io::Result<SocketAddr> {
        if path.as_ref().as_os_str().is_empty() {
            return Ok(SocketAddr::unnamed());
        }
        cpath(path.as_ref())?;
        Ok(SocketAddr { path: Some(path.as_ref().to_path_buf()) })
    }

    /// True if there is no path; see the module note
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn is_unnamed(&self) -> bool {
        self.path.is_none()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn as_pathname(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl fmt::Debug for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(p) => write!(f, "{p:?} (pathname)"),
            None => write!(f, "(unnamed)"),
        }
    }
}

/// The descriptor traits, the same for all three types
macro_rules! fd_traits {
    ($t:ident) => {
        #[stable(feature = "unix_socket", since = "1.10.0")]
        impl AsRawFd for $t {
            #[inline]
            fn as_raw_fd(&self) -> RawFd {
                self.fd.as_raw_fd()
            }
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        impl FromRawFd for $t {
            #[inline]
            unsafe fn from_raw_fd(fd: RawFd) -> $t {
                $t::from_fd(unsafe { FileDesc::from_raw_fd(fd) })
            }
        }

        #[stable(feature = "unix_socket", since = "1.10.0")]
        impl IntoRawFd for $t {
            #[inline]
            fn into_raw_fd(self) -> RawFd {
                self.fd.into_raw_fd()
            }
        }

        #[stable(feature = "io_safety", since = "1.63.0")]
        impl AsFd for $t {
            #[inline]
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.fd.as_fd()
            }
        }

        #[stable(feature = "io_safety", since = "1.63.0")]
        impl From<$t> for OwnedFd {
            #[inline]
            fn from(s: $t) -> OwnedFd {
                s.fd.into_inner()
            }
        }

        #[stable(feature = "io_safety", since = "1.63.0")]
        impl From<OwnedFd> for $t {
            #[inline]
            fn from(fd: OwnedFd) -> $t {
                $t::from_fd(FileDesc::from_inner(fd))
            }
        }
    };
}

/// A connected local stream.
#[stable(feature = "unix_socket", since = "1.10.0")]
pub struct UnixStream {
    fd: FileDesc,
    /// The path this end connected to, when it did; see the module note
    peer: Option<PathBuf>,
}

impl UnixStream {
    fn from_fd(fd: FileDesc) -> UnixStream {
        UnixStream { fd, peer: None }
    }

    /// Connect to the socket bound at `path`. Blocks until the server accepts,
    /// and fails at once if nothing is listening
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        let c = cpath(path.as_ref())?;
        let fd = imp::stream()?;
        imp::connect(&fd, &c)?;
        Ok(UnixStream { fd, peer: Some(path.as_ref().to_path_buf()) })
    }

    #[stable(feature = "unix_socket_creation", since = "1.61.0")]
    pub fn connect_addr(addr: &SocketAddr) -> io::Result<UnixStream> {
        match &addr.path {
            Some(p) => Self::connect(p),
            None => Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "an unnamed address cannot be connected to",
            )),
        }
    }

    /// A connected pair with no name: `socketpair`(77)
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        let (a, b) = imp::pair()?;
        Ok((UnixStream::from_fd(a), UnixStream::from_fd(b)))
    }

    /// A second descriptor for this stream: `dup`(28), answered with an alias
    /// of the same socket
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn try_clone(&self) -> io::Result<UnixStream> {
        Ok(UnixStream { fd: self.fd.duplicate()?, peer: self.peer.clone() })
    }

    /// Always unnamed: a connecting end has no name of its own here.
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::unnamed())
    }

    /// The path this stream connected to, or unnamed.
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr { path: self.peer.clone() })
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        not_here()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        not_here()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.fd.set_nonblocking(nonblocking)
    }

    /// Nothing is ever pending: the call that had an error returns it
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    /// Refused: the syscall table has no half-close, and `close` would take the
    /// stream from its owner
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn shutdown(&self, _how: crate::net::Shutdown) -> io::Result<()> {
        not_here()
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixStream")
            .field("fd", &self.fd.as_raw_fd())
            .field("peer", &self.peer)
            .finish()
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl io::Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut &*self, buf)
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl<'a> io::Read for &'a UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.fd.read(buf)
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl io::Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::Write::write(&mut &*self, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl<'a> io::Write for &'a UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.fd.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fd_traits!(UnixStream);

/// A local socket bound to a path, accepting streams.
#[stable(feature = "unix_socket", since = "1.10.0")]
pub struct UnixListener {
    fd: FileDesc,
    /// The path it was bound to, when it was bound here
    path: Option<PathBuf>,
}

impl UnixListener {
    fn from_fd(fd: FileDesc) -> UnixListener {
        UnixListener { fd, path: None }
    }

    /// Bind to `path` and listen. The kernel creates the node, visible to `ls`,
    /// and removes it when this closes; binding an existing name fails
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
        let c = cpath(path.as_ref())?;
        let fd = imp::stream()?;
        imp::bind_listen(&fd, &c, 128)?;
        Ok(UnixListener { fd, path: Some(path.as_ref().to_path_buf()) })
    }

    #[stable(feature = "unix_socket_creation", since = "1.61.0")]
    pub fn bind_addr(addr: &SocketAddr) -> io::Result<UnixListener> {
        match &addr.path {
            Some(p) => Self::bind(p),
            None => Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "an unnamed address cannot be bound",
            )),
        }
    }

    /// The next connection. Its address is unnamed: the connecting end has no
    /// name
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        Ok((UnixStream::from_fd(imp::accept(&self.fd)?), SocketAddr::unnamed()))
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn try_clone(&self) -> io::Result<UnixListener> {
        Ok(UnixListener { fd: self.fd.duplicate()?, path: self.path.clone() })
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr { path: self.path.clone() })
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.fd.set_nonblocking(nonblocking)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixListener")
            .field("fd", &self.fd.as_raw_fd())
            .field("path", &self.path)
            .finish()
    }
}

fd_traits!(UnixListener);

#[stable(feature = "unix_socket", since = "1.10.0")]
impl<'a> IntoIterator for &'a UnixListener {
    type Item = io::Result<UnixStream>;
    type IntoIter = Incoming<'a>;

    fn into_iter(self) -> Incoming<'a> {
        self.incoming()
    }
}

/// `accept` in a loop, never ending on its own.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[stable(feature = "unix_socket", since = "1.10.0")]
pub struct Incoming<'a> {
    listener: &'a UnixListener,
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl<'a> Iterator for Incoming<'a> {
    type Item = io::Result<UnixStream>;

    fn next(&mut self) -> Option<io::Result<UnixStream>> {
        Some(self.listener.accept().map(|s| s.0))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::MAX, None)
    }
}

/// A local datagram socket, which MikuOS does not have. See the module note.
///
/// Every constructor answers `Unsupported`. One made from a descriptor through
/// `FromRawFd` or `From<OwnedFd>` moves bytes through whatever that descriptor
/// is.
#[stable(feature = "unix_socket", since = "1.10.0")]
pub struct UnixDatagram {
    fd: FileDesc,
}

impl UnixDatagram {
    fn from_fd(fd: FileDesc) -> UnixDatagram {
        UnixDatagram { fd }
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn bind<P: AsRef<Path>>(_path: P) -> io::Result<UnixDatagram> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket_creation", since = "1.61.0")]
    pub fn bind_addr(_addr: &SocketAddr) -> io::Result<UnixDatagram> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn unbound() -> io::Result<UnixDatagram> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn pair() -> io::Result<(UnixDatagram, UnixDatagram)> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn connect<P: AsRef<Path>>(&self, _path: P) -> io::Result<()> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket_creation", since = "1.61.0")]
    pub fn connect_addr(&self, _addr: &SocketAddr) -> io::Result<()> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn try_clone(&self) -> io::Result<UnixDatagram> {
        Ok(UnixDatagram { fd: self.fd.duplicate()? })
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::unnamed())
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::unnamed())
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn recv_from(&self, _buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.fd.read(buf)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn send_to<P: AsRef<Path>>(&self, _buf: &[u8], _path: P) -> io::Result<usize> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket_creation", since = "1.61.0")]
    pub fn send_to_addr(&self, _buf: &[u8], _addr: &SocketAddr) -> io::Result<usize> {
        no_datagrams()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.fd.write(buf)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        not_here()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        not_here()
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(None)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.fd.set_nonblocking(nonblocking)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub fn shutdown(&self, _how: crate::net::Shutdown) -> io::Result<()> {
        not_here()
    }
}

#[stable(feature = "unix_socket", since = "1.10.0")]
impl fmt::Debug for UnixDatagram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixDatagram").field("fd", &self.fd.as_raw_fd()).finish()
    }
}

fd_traits!(UnixDatagram);
