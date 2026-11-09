//! `<sys/uio.h>`

use crate::libc::*;

// What `libc`'s MikuOS module declares and `rustix` links
//
// `lib/abi/src/` declares these; a declaration
// without a symbol is a link error in the first crate calling it. Implement
// what MikuOS can do and answer `ENOSYS` for what it cannot. A stub returning
// success is not allowed.

/// `struct iovec`. `libc`'s, from `src/unix/mod.rs`, laid out the same
pub use miku_abi::posix::iovec;

/// Scatter/gather as a loop of plain reads and writes, stopping at the first
/// short transfer as `readv` does. A concurrent writer can interleave between
/// segments
pub(crate) unsafe fn iov_loop(
    iov: *const iovec,
    iovcnt: i32,
    mut each: impl FnMut(*mut u8, usize, i64) -> i64,
) -> i64 {
    if iovcnt < 0 || iovcnt > 1024 { return -(EINVAL as i64); }
    if iovcnt > 0 && iov.is_null() { return -(EFAULT as i64); }
    let mut total: i64 = 0;
    for i in 0..iovcnt as usize {
        let v = unsafe { &*iov.add(i) };
        if v.iov_len == 0 { continue; }
        let n = each(v.iov_base.cast(), v.iov_len, total);
        if n < 0 {
            // An error after some bytes moved reports those bytes, per POSIX;
            // the error returns on the next call
            return if total > 0 { total } else { n };
        }
        total += n;
        if (n as usize) < v.iov_len { break; }
    }
    total
}

pub(crate) fn iov_result(r: i64) -> i64 {
    if r < 0 { set_errno(-r as i32); -1 } else { r }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readv(fd: i32, iov: *const iovec, iovcnt: i32) -> i64 {
    iov_result(unsafe { iov_loop(iov, iovcnt, |p, n, _| io::miku_read(fd as u64, p, n)) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn writev(fd: i32, iov: *const iovec, iovcnt: i32) -> i64 {
    iov_result(unsafe { iov_loop(iov, iovcnt, |p, n, _| io::miku_write(fd as u64, p, n)) })
}

/// Positioned, inheriting `miku_pread`: a seek, a read, and a seek back, so not
/// atomic against another thread on this descriptor
#[no_mangle]
#[inline(never)]
pub extern "C" fn preadv(fd: i32, iov: *const iovec, iovcnt: i32, offset: i64) -> i64 {
    if offset < 0 { set_errno(EINVAL); return -1; }
    iov_result(unsafe {
        iov_loop(iov, iovcnt, |p, n, done| file::miku_pread(fd as i64, p, n, offset + done))
    })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pwritev(fd: i32, iov: *const iovec, iovcnt: i32, offset: i64) -> i64 {
    if offset < 0 { set_errno(EINVAL); return -1; }
    iov_result(unsafe {
        iov_loop(iov, iovcnt, |p, n, done| file::miku_pwrite(fd as i64, p, n, offset + done))
    })
}
