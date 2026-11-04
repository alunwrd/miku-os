// POSIX errno values returned by syscalls (negative ints, encoded as u64)

use crate::fs::vfs::types::VfsError;

pub const EPERM:        i64 = -1;
pub const ENOENT:       i64 = -2;
pub const ESRCH:        i64 = -3;
pub const EINTR:        i64 = -4;
pub const EIO:          i64 = -5;
pub const ECHILD:       i64 = -10;
pub const EBADF:        i64 = -9;
pub const ENOMEM:       i64 = -12;
pub const EACCES:       i64 = -13;
pub const EFAULT:       i64 = -14;
pub const EEXIST:       i64 = -17;
pub const ENOTDIR:      i64 = -20;
pub const EISDIR:       i64 = -21;
pub const EINVAL:       i64 = -22;
pub const ENOTTY:       i64 = -25;
pub const EMFILE:       i64 = -24;
pub const ENOSPC:       i64 = -28;
pub const EPIPE:        i64 = -32;
pub const ENAMETOOLONG: i64 = -36;
pub const ENOSYS:       i64 = -38;
pub const ENOTEMPTY:    i64 = -39;
pub const EXDEV:        i64 = -18;
pub const ENODEV:       i64 = -19;
/// No such device or address. What `/dev/tty` answers a process that has no
/// controlling terminal: the node exists, and there is nothing for it to name
pub const ENXIO:        i64 = -6;
pub const EFBIG:        i64 = -27;
pub const EROFS:        i64 = -30;
pub const EMLINK:       i64 = -31;
pub const ERANGE:       i64 = -34;
pub const ELOOP:        i64 = -40;
pub const ESTALE:       i64 = -116;
/// Structure needs cleaning: the read succeeded and what came back was not a
/// filesystem. Linux returns this for EFSCORRUPTED
pub const EUCLEAN:      i64 = -117;
pub const EAGAIN:          i64 = -11;
pub const EBUSY:           i64 = -16;
pub const EPROTONOSUPPORT: i64 = -93;
pub const EOPNOTSUPP:      i64 = -95;
pub const EAFNOSUPPORT:    i64 = -97;
pub const EADDRINUSE:      i64 = -98;
pub const ECONNRESET:      i64 = -104;
pub const EISCONN:         i64 = -106;
/// A non-blocking connect has started and not finished; `poll` for writable
pub const EINPROGRESS:     i64 = -115;
/// And `connect` was called on it again before it did
pub const EALREADY:        i64 = -114;
/// No such option at that level (`getsockopt`)
pub const ENOPROTOOPT:     i64 = -92;
pub const ENOTCONN:        i64 = -107;
pub const ECONNREFUSED:    i64 = -111;
/// A wait that was given a budget and used it up. Distinct from EAGAIN: the
/// caller asked to be kept waiting and this is how long it got
pub const ETIMEDOUT:       i64 = -110;
/// Over a quota, Linux's number. `gpu_alloc` past what all programs together
/// may hold of video memory
pub const EDQUOT:          i64 = -122;
/// The thing was there and could not be made to run. Used where a GPU launch
/// descriptor cannot be built for a program that exists
pub const ENOEXEC:         i64 = -8;

#[inline]
pub fn err(code: i64) -> u64 { code as u64 }

pub fn vfs_err(e: VfsError) -> u64 {
    let code = match e {
        VfsError::NotFound         => ENOENT,
        VfsError::PermissionDenied => EACCES,
        VfsError::AlreadyExists    => EEXIST,
        VfsError::NotDirectory     => ENOTDIR,
        VfsError::IsDirectory      => EISDIR,
        VfsError::NotEmpty         => ENOTEMPTY,
        VfsError::InvalidPath      => EINVAL,
        VfsError::NoSpace          => ENOSPC,
        // EROFS, not EPERM. "You may not write here" and "this filesystem is
        // read-only" send a program to different places
        VfsError::ReadOnly         => EROFS,
        VfsError::InvalidArgument  => EINVAL,
        VfsError::BadFd            => EBADF,
        VfsError::TooManyOpenFiles => EMFILE,
        VfsError::NameTooLong      => ENAMETOOLONG,
        VfsError::BrokenPipe       => EPIPE,
        VfsError::IoError          => EIO,
        VfsError::NotSupported     => ENOSYS,
        VfsError::TooManyLinks     => EMLINK,
        VfsError::OutOfMemory      => ENOMEM,
        VfsError::Loop | VfsError::TooManySymlinks => ELOOP,
        VfsError::FileTooLarge     => EFBIG,
        VfsError::Corrupted        => EUCLEAN,
        VfsError::XattrTooLarge    => ERANGE,
        VfsError::NotMounted       => ENODEV,
        VfsError::NoTerminal       => ENXIO,
        VfsError::Deferred         => EAGAIN,
        VfsError::CrossDevice      => EXDEV,
        VfsError::Busy             => EBUSY,
        VfsError::Stale            => ESTALE,
        VfsError::WouldBlock       => EAGAIN,
        VfsError::Interrupted      => EINTR,
        _                          => EINVAL,
    };
    err(code)
}
