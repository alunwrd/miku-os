// pty.h and the pseudo-terminal half of stdlib.h
//
// Opening `/dev/ptmx` gives the master. `TIOCGPTN` on it gives the terminal's
// id, whose low eight bits are its slot, and the slave is the devfs node
// `/dev/ptsN` (flat, as devfs is: `kernel/fs/devfs.rs`). Opening the slave is
// limited by the terminal's family rule, as with `tty(ATTACH)`; `TIOCSCTTY` on
// it is `tty(ATTACH)`. `openpty` and `login_tty` are called by `rustix-openpty`
// and `portable-pty`.
//
// `grantpt` and `unlockpt` change nothing, since the family rule decides at
// `open`. They still check for a master.

use crate::ctermios::{tcsetattr, termios};
use crate::libc::{close, dup2, fail, ioctl, open, EINVAL, ERANGE};

const O_RDWR: i32 = 0x0003;
const TIOCGPTN: u64 = 0x8004_5430;
const TIOCSWINSZ: u64 = 0x5414;
const TIOCSCTTY: u64 = 0x540E;

/// `/dev/ptsN` for the terminal behind master `fd`, NUL-terminated, and its
/// length without the NUL
fn slave_path(fd: i32) -> Result<([u8; 16], usize), i32> {
    let mut id: u32 = 0;
    if ioctl(fd, TIOCGPTN, &mut id as *mut u32 as *mut u8) < 0 {
        return Err(unsafe { *crate::libc::__errno_location() });
    }
    let slot = (id & 0xff) as u8;
    let mut p = [0u8; 16];
    let pre = b"/dev/pts";
    p[..pre.len()].copy_from_slice(pre);
    let mut n = pre.len();
    if slot >= 100 { p[n] = b'0' + slot / 100; n += 1; }
    if slot >= 10 { p[n] = b'0' + slot / 10 % 10; n += 1; }
    p[n] = b'0' + slot % 10;
    n += 1;
    Ok((p, n))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn posix_openpt(flags: i32) -> i32 {
    open(b"/dev/ptmx\0".as_ptr(), flags, 0)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn grantpt(fd: i32) -> i32 {
    match slave_path(fd) {
        Ok(_) => 0,
        Err(_) => fail(EINVAL),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn unlockpt(fd: i32) -> i32 {
    grantpt(fd)
}

/// Slave's name into `buf`; the error is the return value
#[no_mangle]
#[inline(never)]
pub extern "C" fn ptsname_r(fd: i32, buf: *mut u8, len: usize) -> i32 {
    if buf.is_null() { return EINVAL; }
    let (p, n) = match slave_path(fd) {
        Ok(v) => v,
        Err(_) => return EINVAL,
    };
    if n + 1 > len { return ERANGE; }
    unsafe { core::ptr::copy_nonoverlapping(p.as_ptr(), buf, n + 1) };
    0
}

/// The terminal a descriptor is on, by name: `/dev/console` for the console
/// (slot 0), `/dev/ptsN` for a pty. Names the caller's controlling terminal,
/// `tty(TTY_SELF)`, after checking `fd` is a terminal; see `tcgetpgrp`
#[no_mangle]
#[inline(never)]
pub extern "C" fn ttyname_r(fd: i32, buf: *mut u8, len: usize) -> i32 {
    const TTY_SELF: u64 = 3;
    if buf.is_null() { return EINVAL; }
    if crate::libc::isatty(fd) == 0 { return crate::libc::ENOTTY; }
    let id = unsafe { crate::sys::sc2(crate::sys::SYS_TTY, TTY_SELF, 0) };
    if id <= 0 { return crate::libc::ENOTTY; }
    let slot = (id as u32 & 0xff) as u8;
    let mut p = [0u8; 16];
    let n = if slot == 0 {
        let c = b"/dev/console";
        p[..c.len()].copy_from_slice(c);
        c.len()
    } else {
        let pre = b"/dev/pts";
        p[..pre.len()].copy_from_slice(pre);
        let mut n = pre.len();
        if slot >= 100 { p[n] = b'0' + slot / 100; n += 1; }
        if slot >= 10 { p[n] = b'0' + slot / 10 % 10; n += 1; }
        p[n] = b'0' + slot % 10;
        n + 1
    };
    if n + 1 > len { return ERANGE; }
    unsafe { core::ptr::copy_nonoverlapping(p.as_ptr(), buf, n + 1) };
    0
}

/// The same into a shared buffer. POSIX lets this one be unsafe across threads;
/// use `ptsname_r`
#[no_mangle]
#[inline(never)]
pub extern "C" fn ptsname(fd: i32) -> *mut u8 {
    static mut NAME: [u8; 16] = [0; 16];
    let buf = unsafe { core::ptr::addr_of_mut!(NAME) as *mut u8 };
    let e = ptsname_r(fd, buf, 16);
    if e != 0 {
        fail(e);
        return core::ptr::null_mut();
    }
    buf
}

/// A terminal and both ends. The slave gets `termp` if given, the terminal
/// `winp`'s size, set through the master
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn openpty(
    amaster: *mut i32,
    aslave: *mut i32,
    name: *mut u8,
    termp: *const termios,
    winp: *const u8,
) -> i32 {
    if amaster.is_null() || aslave.is_null() { return fail(EINVAL); }
    let m = posix_openpt(O_RDWR);
    if m < 0 { return -1; }
    let (path, n) = match slave_path(m) {
        Ok(v) => v,
        Err(e) => { close(m); return fail(e); }
    };
    let s = open(path.as_ptr(), O_RDWR, 0);
    if s < 0 {
        let e = unsafe { *crate::libc::__errno_location() };
        close(m);
        return fail(e);
    }
    if !termp.is_null() && unsafe { tcsetattr(s, 0, termp) } < 0
        || !winp.is_null() && ioctl(m, TIOCSWINSZ, winp as *mut u8) < 0
    {
        let e = unsafe { *crate::libc::__errno_location() };
        close(s);
        close(m);
        return fail(e);
    }
    if !name.is_null() {
        unsafe { core::ptr::copy_nonoverlapping(path.as_ptr(), name, n + 1) };
    }
    unsafe {
        *amaster = m;
        *aslave = s;
    }
    0
}

/// Makes slave `fd` the caller's controlling terminal and standard input,
/// output, and error, for a child between `fork` and `exec`
#[no_mangle]
#[inline(never)]
pub extern "C" fn login_tty(fd: i32) -> i32 {
    if ioctl(fd, TIOCSCTTY, core::ptr::null_mut()) < 0 {
        return -1;
    }
    for std in 0..3 {
        if dup2(fd, std) < 0 {
            return -1;
        }
    }
    if fd > 2 {
        close(fd);
    }
    0
}
