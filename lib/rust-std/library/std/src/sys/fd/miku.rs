//! A descriptor, and the four things `std` does with one.
//!
//! MikuOS descriptors are small integers in two ranges: files, pipes and
//! terminals from 0, and sockets from `SOCK_FD_BASE` (4096). Both fit in a
//! `c_int`, so `RawFd` is the ordinary one.

#![unstable(reason = "not public", issue = "none", feature = "fd")]

use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, Read};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::sys::pal::{abi, cvt};
use crate::sys::{AsInner, FromInner, IntoInner};

#[derive(Debug)]
pub struct FileDesc(OwnedFd);

impl FileDesc {
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_read(self.as_raw_fd() as u64, buf.as_mut_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        io::default_read_buf(|buf| self.read(buf), cursor)
    }

    /// Vectored reads are a loop.
    ///
    /// There is no `readv`, and `is_read_vectored` says so, so `std` does not
    /// build a vector expecting one call
    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        io::default_read_vectored(|b| self.read(b), bufs)
    }

    pub fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let mut me = self;
        (&mut me).read_to_end(buf)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_write(self.as_raw_fd() as u64, buf.as_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        io::default_write_vectored(|b| self.write(b), bufs)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        false
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        // `fcntl`(113) reaches both kinds: a VFS descriptor keeps the flag in
        // its open-file entry, a socket on its slot in the socket table
        cvt(abi::miku_set_nonblocking(self.as_raw_fd() as i64, nonblocking))?;
        Ok(())
    }

    #[inline]
    pub fn duplicate(&self) -> io::Result<FileDesc> {
        let fd = dup_cloexec(self.as_raw_fd())?;
        // SAFETY: just handed over by the kernel, owned by nobody else
        unsafe { Ok(Self::from_raw_fd(fd as RawFd)) }
    }

    #[inline]
    pub fn try_clone(&self) -> io::Result<Self> {
        self.duplicate()
    }

    /// `sys/process/unsupported.rs` ends its unreachable arms by diverging
    /// through a `Pipe` that is `!`. MikuOS's `Pipe` is a real descriptor, and
    /// `sys/process/miku.rs` does not call this
    pub fn diverge(&self) -> ! {
        panic!("sys::process is unsupported on MikuOS; this pipe cannot exist")
    }
}

impl<'a> Read for &'a FileDesc {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(buf)
    }

    fn read_buf(&mut self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        (**self).read_buf(cursor)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        (**self).read_vectored(bufs)
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        (**self).is_read_vectored()
    }
}

impl AsInner<OwnedFd> for FileDesc {
    #[inline]
    fn as_inner(&self) -> &OwnedFd {
        &self.0
    }
}

impl IntoInner<OwnedFd> for FileDesc {
    fn into_inner(self) -> OwnedFd {
        self.0
    }
}

impl FromInner<OwnedFd> for FileDesc {
    fn from_inner(owned_fd: OwnedFd) -> Self {
        Self(owned_fd)
    }
}

impl AsFd for FileDesc {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for FileDesc {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl IntoRawFd for FileDesc {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl FromRawFd for FileDesc {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        unsafe { Self(FromRawFd::from_raw_fd(raw_fd)) }
    }
}

/// Close a descriptor by number.
///
/// `OwnedFd`'s `Drop` needs this and lives in `os::fd`, which cannot see
/// `sys::pal`.
pub fn close_raw(fd: RawFd) {
    abi::miku_close(fd as i64);
}

/// A duplicate with `FD_CLOEXEC`, for `try_clone` and
/// `BorrowedFd::try_clone_to_owned`: `F_DUPFD_CLOEXEC`, atomically, since
/// threads fork while others clone descriptors
pub fn dup_cloexec(fd: RawFd) -> io::Result<RawFd> {
    const F_DUPFD_CLOEXEC: u64 = 1030;
    let new = abi::miku_fcntl(fd as i64, F_DUPFD_CLOEXEC, 0);
    if new < 0 {
        return Err(io::Error::from_raw_os_error(-new as i32));
    }
    Ok(new as RawFd)
}
