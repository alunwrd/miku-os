// POSIX error codes //

pub const EOK:      i64 = 0;
pub const EPERM:    i64 = -1;
pub const ENOENT:   i64 = -2;
pub const ESRCH:    i64 = -3;
pub const EINTR:    i64 = -4;
pub const EIO:      i64 = -5;
pub const ENXIO:    i64 = -6;
pub const E2BIG:    i64 = -7;
pub const ENOEXEC:  i64 = -8;
pub const EBADF:    i64 = -9;
pub const ECHILD:   i64 = -10;
pub const EAGAIN:   i64 = -11;
pub const EWOULDBLOCK: i64 = -11;
pub const ENOMEM:   i64 = -12;
pub const EACCES:   i64 = -13;
pub const EFAULT:   i64 = -14;
pub const EBUSY:    i64 = -16;
pub const EEXIST:   i64 = -17;
pub const EXDEV:    i64 = -18;
pub const ENODEV:   i64 = -19;
pub const ENOTDIR:  i64 = -20;
pub const EISDIR:   i64 = -21;
pub const EINVAL:   i64 = -22;
pub const ENFILE:   i64 = -23;
pub const EMFILE:   i64 = -24;
pub const ENOTTY:   i64 = -25;
pub const EFBIG:    i64 = -27;
pub const ENOSPC:   i64 = -28;
pub const ESPIPE:   i64 = -29;
pub const EROFS:    i64 = -30;
pub const EMLINK:   i64 = -31;
pub const EPIPE:    i64 = -32;
pub const EDOM:     i64 = -33;
pub const ERANGE:   i64 = -34;
pub const EDEADLK:  i64 = -35;
pub const ENAMETOOLONG: i64 = -36;
pub const ENOSYS:   i64 = -38;
pub const ENOTEMPTY: i64 = -39;
pub const ELOOP:    i64 = -40;
pub const EOVERFLOW: i64 = -75;
pub const ENOTSOCK: i64 = -88;
pub const EAFNOSUPPORT: i64 = -97;
pub const EADDRINUSE:   i64 = -98;
pub const ENETUNREACH:  i64 = -101;
pub const ECONNRESET:   i64 = -104;
pub const ENOTCONN:     i64 = -107;
pub const ETIMEDOUT:    i64 = -110;
pub const ECONNREFUSED: i64 = -111;

// thread-local errno //

static mut LAST_ERRNO: i64 = 0;

#[inline]
pub fn set_errno(code: i64) {
    unsafe { LAST_ERRNO = code; }
}

#[inline]
pub fn get_errno() -> i64 {
    unsafe { LAST_ERRNO }
}

// helpers //

#[inline]
pub fn is_error(val: i64) -> bool {
    val < 0
}

#[inline]
pub fn to_errno(val: i64) -> i64 {
    if val < 0 { -val } else { 0 }
}

// returns null-terminated error description
pub fn strerror(code: i64) -> &'static [u8] {
    let abs = code.unsigned_abs();
    match abs {
        0   => b"success\0",
        1   => b"operation not permitted\0",
        2   => b"no such file or directory\0",
        3   => b"no such process\0",
        4   => b"interrupted system call\0",
        5   => b"input/output error\0",
        6   => b"no such device or address\0",
        7   => b"argument list too long\0",
        8   => b"exec format error\0",
        9   => b"bad file descriptor\0",
        10  => b"no child processes\0",
        11  => b"resource temporarily unavailable\0",
        12  => b"out of memory\0",
        13  => b"permission denied\0",
        14  => b"bad address\0",
        16  => b"device or resource busy\0",
        17  => b"file exists\0",
        18  => b"cross-device link\0",
        19  => b"no such device\0",
        20  => b"not a directory\0",
        21  => b"is a directory\0",
        22  => b"invalid argument\0",
        23  => b"too many open files in system\0",
        24  => b"too many open files\0",
        25  => b"inappropriate ioctl for device\0",
        27  => b"file too large\0",
        28  => b"no space left on device\0",
        29  => b"illegal seek\0",
        30  => b"read-only file system\0",
        31  => b"too many links\0",
        32  => b"broken pipe\0",
        33  => b"numerical argument out of domain\0",
        34  => b"numerical result out of range\0",
        35  => b"resource deadlock avoided\0",
        36  => b"file name too long\0",
        38  => b"function not implemented\0",
        39  => b"directory not empty\0",
        40  => b"too many levels of symbolic links\0",
        75  => b"value too large for data type\0",
        88  => b"socket operation on non-socket\0",
        97  => b"address family not supported\0",
        98  => b"address already in use\0",
        101 => b"network is unreachable\0",
        104 => b"connection reset by peer\0",
        107 => b"transport endpoint is not connected\0",
        110 => b"connection timed out\0",
        111 => b"connection refused\0",
        _   => b"unknown error\0",
    }
}

// returns null-terminated short error name
pub fn errno_name(code: i64) -> &'static [u8] {
    let abs = code.unsigned_abs();
    match abs {
        0   => b"EOK\0",
        1   => b"EPERM\0",
        2   => b"ENOENT\0",
        3   => b"ESRCH\0",
        4   => b"EINTR\0",
        5   => b"EIO\0",
        6   => b"ENXIO\0",
        7   => b"E2BIG\0",
        8   => b"ENOEXEC\0",
        9   => b"EBADF\0",
        10  => b"ECHILD\0",
        11  => b"EAGAIN\0",
        12  => b"ENOMEM\0",
        13  => b"EACCES\0",
        14  => b"EFAULT\0",
        16  => b"EBUSY\0",
        17  => b"EEXIST\0",
        18  => b"EXDEV\0",
        19  => b"ENODEV\0",
        20  => b"ENOTDIR\0",
        21  => b"EISDIR\0",
        22  => b"EINVAL\0",
        23  => b"ENFILE\0",
        24  => b"EMFILE\0",
        25  => b"ENOTTY\0",
        27  => b"EFBIG\0",
        28  => b"ENOSPC\0",
        29  => b"ESPIPE\0",
        30  => b"EROFS\0",
        31  => b"EMLINK\0",
        32  => b"EPIPE\0",
        33  => b"EDOM\0",
        34  => b"ERANGE\0",
        35  => b"EDEADLK\0",
        36  => b"ENAMETOOLONG\0",
        38  => b"ENOSYS\0",
        39  => b"ENOTEMPTY\0",
        40  => b"ELOOP\0",
        75  => b"EOVERFLOW\0",
        88  => b"ENOTSOCK\0",
        97  => b"EAFNOSUPPORT\0",
        98  => b"EADDRINUSE\0",
        101 => b"ENETUNREACH\0",
        104 => b"ECONNRESET\0",
        107 => b"ENOTCONN\0",
        110 => b"ETIMEDOUT\0",
        111 => b"ECONNREFUSED\0",
        _   => b"EUNKNOWN\0",
    }
}

// C exports //

#[no_mangle]
pub extern "C" fn miku_strerror(code: i64) -> *const u8 {
    strerror(code).as_ptr()
}

#[no_mangle]
pub extern "C" fn miku_errno_name(code: i64) -> *const u8 {
    errno_name(code).as_ptr()
}

#[no_mangle]
pub extern "C" fn miku_perror(prefix: *const u8) {
    miku_perror_code(prefix, get_errno())
}

// perror with explicit error code
#[no_mangle]
pub extern "C" fn miku_perror_code(prefix: *const u8, code: i64) {
    if !prefix.is_null() {
        let len = crate::string::miku_strlen(prefix);
        if len > 0 {
            crate::io::miku_write(2, prefix, len);
            crate::io::miku_write(2, b": ".as_ptr(), 2);
        }
    }
    let msg = strerror(code);
    // msg is null-terminated, write len-1 (skip NUL)
    crate::io::miku_write(2, msg.as_ptr(), msg.len() - 1);
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
}

#[no_mangle]
pub extern "C" fn miku_is_error(val: i64) -> bool {
    val < 0
}

#[no_mangle]
pub extern "C" fn miku_to_errno(val: i64) -> i64 {
    if val < 0 { -val } else { 0 }
}

#[no_mangle]
pub extern "C" fn miku_set_errno(code: i64) {
    set_errno(code);
}

#[no_mangle]
pub extern "C" fn miku_get_errno() -> i64 {
    get_errno()
}
