//! Errors, and where MikuOS keeps them.
//!
//! MikuOS's own calls return `-errno`, and everything `std` does itself builds
//! its `io::Error` at the call site through `sys::pal::cvt`.
//!
//! `libc_miku.so`'s C functions (`connect`, `read`, `pipe2`, everything a crate
//! reaches through `libc`) follow the C convention, returning `-1` and leaving
//! the error in a per-thread cell, the word at `tp + 40` (`docs/MikuOS_ABI.md`
//! §3.2d). `io::Error::last_os_error()` is this function, and `mio` reads it
//! for the `EINPROGRESS` it expects.

use crate::io;

pub fn errno() -> io::RawOsError {
    // SAFETY: the cell is this thread's own and always mapped: `ld-miku` builds
    // the first thread's TCB before any library code runs, and `thread_spawn`
    // builds every other one
    unsafe { *crate::sys::pal::abi::miku_errno_location() }
}

pub fn is_interrupted(code: io::RawOsError) -> bool {
    code == 4 // EINTR
}

pub fn decode_error_kind(code: io::RawOsError) -> io::ErrorKind {
    crate::sys::pal::decode_error_kind(code)
}

pub fn error_string(errno: io::RawOsError) -> String {
    // The names: MikuOS has no `strerror` table, and sentences here would be a
    // second description of each number
    let s = match errno {
        1 => "operation not permitted",
        2 => "no such file or directory",
        3 => "no such process",
        4 => "interrupted system call",
        5 => "input/output error",
        6 => "no such device or address",
        7 => "argument list too long",
        8 => "exec format error",
        9 => "bad file descriptor",
        10 => "no child processes",
        11 => "resource temporarily unavailable",
        12 => "cannot allocate memory",
        13 => "permission denied",
        14 => "bad address",
        16 => "device or resource busy",
        17 => "file exists",
        18 => "invalid cross-device link",
        19 => "no such device",
        20 => "not a directory",
        21 => "is a directory",
        22 => "invalid argument",
        24 => "too many open files",
        25 => "inappropriate ioctl for device",
        27 => "file too large",
        28 => "no space left on device",
        29 => "illegal seek",
        30 => "read-only file system",
        31 => "too many links",
        32 => "broken pipe",
        33 => "numerical argument out of domain",
        34 => "numerical result out of range",
        36 => "file name too long",
        38 => "function not implemented",
        39 => "directory not empty",
        40 => "too many levels of symbolic links",
        95 => "operation not supported",
        98 => "address already in use",
        104 => "connection reset by peer",
        107 => "transport endpoint is not connected",
        110 => "connection timed out",
        111 => "connection refused",
        _ => "unknown error",
    };
    // The name alone, as `strerror` gives it on Linux. `io::Error`'s Display
    // adds " (os error N)" itself
    s.into()
}
