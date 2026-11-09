// POSIX errno values returned by syscalls (negative ints, encoded as u64)

use crate::fs::vfs::types::VfsError;

pub use miku_abi::errno::neg::*;

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
