// POSIX errno values returned by syscalls (negative ints, encoded as u64)

use crate::vfs::types::VfsError;

pub const EPERM:        i64 = -1;
pub const ENOENT:       i64 = -2;
pub const ESRCH:        i64 = -3;
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
pub const EMFILE:       i64 = -24;
pub const ENOSPC:       i64 = -28;
pub const EPIPE:        i64 = -32;
pub const ENAMETOOLONG: i64 = -36;
pub const ENOSYS:       i64 = -38;
pub const ENOTEMPTY:    i64 = -39;
pub const EAGAIN:          i64 = -11;
pub const EBUSY:           i64 = -16;
pub const EPROTONOSUPPORT: i64 = -93;
pub const EAFNOSUPPORT:    i64 = -97;
pub const ECONNRESET:      i64 = -104;
pub const EISCONN:         i64 = -106;
pub const ENOTCONN:        i64 = -107;
pub const ECONNREFUSED:    i64 = -111;

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
        VfsError::ReadOnly         => EPERM,
        VfsError::InvalidArgument  => EINVAL,
        VfsError::BadFd            => EBADF,
        VfsError::TooManyOpenFiles => EMFILE,
        VfsError::NameTooLong      => ENAMETOOLONG,
        VfsError::BrokenPipe       => EPIPE,
        VfsError::IoError          => EIO,
        VfsError::NotSupported     => ENOSYS,
        _                          => EINVAL,
    };
    err(code)
}
