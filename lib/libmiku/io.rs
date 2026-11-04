use crate::sys::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64 {
    unsafe { sc3(SYS_WRITE, fd, buf as u64, len as u64) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_read(fd: u64, buf: *mut u8, len: usize) -> i64 {
    unsafe { sc3(SYS_READ, fd, buf as u64, len as u64) }
}

pub fn write_all(fd: u64, data: &[u8]) -> i64 {
    if data.is_empty() { return 0; }
    let mut done = 0usize;
    while done < data.len() {
        let n = miku_write(fd, unsafe { data.as_ptr().add(done) }, data.len() - done);
        if n <= 0 { return n; }
        done += n as usize;
    }
    done as i64
}

pub fn read_all(fd: u64, buf: &mut [u8]) -> i64 {
    if buf.is_empty() { return 0; }
    let mut done = 0usize;
    while done < buf.len() {
        let n = miku_read(fd, unsafe { buf.as_mut_ptr().add(done) }, buf.len() - done);
        if n < 0 { return n; }
        if n == 0 { break; }
        done += n as usize;
    }
    done as i64
}

// poll: waiting on several descriptors at once

/// The kernel's `struct pollfd`, laid out as on Linux
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MikuPollFd {
    /// Descriptor to watch. Negative skips this slot
    pub fd: i32,
    /// What to watch for
    pub events: i16,
    /// What actually happened. Written by the kernel on every call
    pub revents: i16,
}

pub const POLLIN:   i16 = 0x001;
pub const POLLOUT:  i16 = 0x004;
pub const POLLERR:  i16 = 0x008;
/// The far end has gone. Reported whether or not asked for
pub const POLLHUP:  i16 = 0x010;
/// Nothing is open on that descriptor
pub const POLLNVAL: i16 = 0x020;

/// Waits until one of `fds` is ready or `timeout_ms` passes. Returns entries
/// with non-zero `revents`, 0 on timeout, or a negative errno. A negative
/// `timeout_ms` waits indefinitely; zero returns at once. At most 64
/// descriptors per call
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_poll(fds: *mut MikuPollFd, nfds: u64, timeout_ms: i64) -> i64 {
    unsafe { sc3(SYS_POLL, fds as u64, nfds, timeout_ms as u64) }
}

/// klog(buf, len): a line for the kernel's log, for a service, whose stdout
/// would interrupt a terminal
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_klog(buf: *const u8, len: usize) -> i64 {
    unsafe { crate::sys::sc2(crate::sys::SYS_KLOG, buf as u64, len as u64) }
}

/// ioctl(fd, request, argp): terminal control. TCGETS (0x5401), TCSETS
/// (0x5402), TIOCGWINSZ (0x5413), TIOCSWINSZ (0x5414), for turning echo off and
/// reading the window size.
///
/// `fd` must be a terminal; a redirected descriptor answers `ENOTTY`.
/// `/dev/tty` reaches the terminal when stdin is not it
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_ioctl(fd: u64, request: u64, argp: u64) -> i64 {
    unsafe { crate::sys::sc3(crate::sys::SYS_IOCTL, fd, request, argp) }
}
