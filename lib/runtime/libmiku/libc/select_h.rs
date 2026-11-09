//! `<sys/select.h>`

use crate::libc::*;

/// `select` over `poll`(76), as libcs without their own do. Readable is
/// `POLLIN`, a hangup, or an error; writable is `POLLOUT` or an error; the
/// exception set is never marked, as there is no out-of-band data. The kernel
/// watches 256 descriptors per call
#[no_mangle]
#[inline(never)]
pub extern "C" fn select(
    nfds: i32,
    r: *mut [u64; 16],
    w: *mut [u64; 16],
    e: *mut [u64; 16],
    tv: *const crate::ctime::Timeval,
) -> i32 {
    use miku_abi::poll::POLLIN;
    use miku_abi::poll::POLLOUT;
    use miku_abi::poll::POLLERR;
    use miku_abi::poll::POLLHUP;
    use miku_abi::poll::POLLNVAL;
    if !(0..=1024).contains(&nfds) { return fail(EINVAL); }
    let bit = |set: *mut [u64; 16], fd: i32| -> bool {
        !set.is_null() && unsafe { (*set)[fd as usize / 64] } >> (fd % 64) & 1 != 0
    };
    let mut fds = [pollfd { fd: -1, events: 0, revents: 0 }; 256];
    let mut n = 0;
    for fd in 0..nfds {
        let ev = (if bit(r, fd) { POLLIN } else { 0 }) | (if bit(w, fd) { POLLOUT } else { 0 });
        if ev == 0 && !bit(e, fd) { continue; }
        if n == fds.len() { return fail(EINVAL); }
        fds[n] = pollfd { fd, events: ev, revents: 0 };
        n += 1;
    }
    let timeout = if tv.is_null() {
        -1
    } else {
        let t = unsafe { &*tv };
        if t.tv_sec < 0 || t.tv_usec < 0 { return fail(EINVAL); }
        (t.tv_sec.saturating_mul(1000) + (t.tv_usec + 999) / 1000).min(i32::MAX as i64) as i32
    };
    let got = poll(fds.as_mut_ptr(), n as u64, timeout);
    if got < 0 { return -1; }
    for set in [r, w, e] {
        if !set.is_null() { unsafe { *set = [0; 16] }; }
    }
    let mut marked = 0;
    for p in &fds[..n] {
        if p.revents & POLLNVAL != 0 { return fail(EBADF); }
        let fd = p.fd as usize;
        if p.events & POLLIN != 0 && p.revents & (POLLIN | POLLHUP | POLLERR) != 0 {
            unsafe { (*r)[fd / 64] |= 1 << (fd % 64) };
            marked += 1;
        }
        if p.events & POLLOUT != 0 && p.revents & (POLLOUT | POLLERR) != 0 {
            unsafe { (*w)[fd / 64] |= 1 << (fd % 64) };
            marked += 1;
        }
    }
    marked
}
