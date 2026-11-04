// termios.h: Linux's `struct termios` over MikuOS's twelve bytes
//
// The kernel's settings are what a line discipline acts on
// (`kernel/syscall/adapters/io.rs`, `MikuTermios`): canonical mode, echo,
// signals, `TOSTOP`, seven control characters, `VMIN`, and `VTIME`. No baud
// rate, parity, or modem line, as every terminal is a console or pty.
//
// Programs use Linux's 60-byte form, through `rustix`, `nix`, and `serial2` or
// C's `tcgetattr`. `tcgetattr` reports what the terminal does: honored flags as
// set; fixed behavior as the flags describing it (8-bit, receiver on, erase as
// backspace-space-backspace); and a flag the kernel cannot act on is accepted
// by `tcsetattr` and not reported back. `ioctl(TCGETS)` and `ioctl(TCSETS*)`
// from C come here too.
//
// Constants are Linux's, as the `libc` crate declares for MikuOS.

use crate::libc::{fail, EINVAL, ENOSYS, ENOTTY};

const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;

// c_iflag / c_oflag bits this file names
const ICRNL: u32 = 0o400;
const IXON: u32 = 0o2000;
const BRKINT: u32 = 0o2;
const INPCK: u32 = 0o20;
const ISTRIP: u32 = 0o40;
const INLCR: u32 = 0o100;
const IGNCR: u32 = 0o200;
const IGNBRK: u32 = 0o1;
const PARMRK: u32 = 0o10;
const OPOST: u32 = 0o1;
// c_cflag
const CSIZE: u32 = 0o60;
const CS8: u32 = 0o60;
const CREAD: u32 = 0o200;
const PARENB: u32 = 0o400;
const CBAUD: u32 = 0o10017;
const B38400: u32 = 0o17;
// c_lflag
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const ECHOE: u32 = 0o20;
const ECHONL: u32 = 0o100;
const TOSTOP: u32 = 0o400;
const IEXTEN: u32 = 0o100000;
// c_cc indices
const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;
const VSUSP: usize = 10;
const VWERASE: usize = 14;

/// Linux's `struct termios`, 60 bytes, as the `libc` crate declares
#[repr(C)]
pub struct termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

/// The kernel's twelve bytes, `kernel/syscall/adapters/io.rs`
#[repr(C)]
#[derive(Default)]
struct MikuTermios {
    icanon: u8,
    echo: u8,
    isig: u8,
    vmin: u8,
    vtime: u8,
    tostop: u8,
    c_cc: [u8; 7],
    _pad: u8,
}

/// Linux's control-character slot for each of the kernel's seven, in its order
/// (`kernel/io/tty/ldisc.rs`: INTR QUIT ERASE KILL EOF WERASE SUSP)
const CC_MAP: [usize; 7] = [VINTR, VQUIT, VERASE, VKILL, VEOF, VWERASE, VSUSP];

fn get(fd: i32) -> Result<MikuTermios, i32> {
    let mut k = MikuTermios::default();
    let r = unsafe { crate::sys::sc3(crate::sys::SYS_IOCTL, fd as u64, TCGETS, &mut k as *mut _ as u64) };
    if r < 0 { Err(-r as i32) } else { Ok(k) }
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn tcgetattr(fd: i32, t: *mut termios) -> i32 {
    if t.is_null() { return fail(EINVAL); }
    let k = match get(fd) {
        Ok(k) => k,
        Err(e) => return fail(e),
    };
    let mut lflag = 0;
    if k.icanon != 0 { lflag |= ICANON; }
    // Erase is shown as backspace-space-backspace whenever input is echoed
    if k.echo != 0 { lflag |= ECHO | ECHOE; }
    if k.isig != 0 { lflag |= ISIG; }
    if k.tostop != 0 { lflag |= TOSTOP; }
    let mut cc = [0u8; 32];
    for (i, &slot) in CC_MAP.iter().enumerate() {
        cc[slot] = k.c_cc[i];
    }
    cc[VMIN] = k.vmin;
    cc[VTIME] = k.vtime;
    unsafe {
        t.write(termios {
            c_iflag: 0,
            c_oflag: 0,
            // Eight clean bits, receiver always on: a pty
            c_cflag: CS8 | CREAD | B38400,
            c_lflag: lflag,
            c_line: 0,
            c_cc: cc,
            c_ispeed: B38400,
            c_ospeed: B38400,
        });
    }
    0
}

/// `TCSANOW`, `TCSADRAIN`, and `TCSAFLUSH` all apply at once. Output is written
/// through as produced, so nothing drains, and pending input is not discarded
/// for `TCSAFLUSH`, with no kernel call for it
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn tcsetattr(fd: i32, action: i32, t: *const termios) -> i32 {
    if t.is_null() || !(0..=2).contains(&action) { return fail(EINVAL); }
    let t = unsafe { &*t };
    // Read first: the kernel keeps its own `_pad`, and a non-terminal is ENOTTY
    // before any write
    if let Err(e) = get(fd) { return fail(e); }
    let mut k = MikuTermios {
        icanon: (t.c_lflag & ICANON != 0) as u8,
        echo: (t.c_lflag & ECHO != 0) as u8,
        isig: (t.c_lflag & ISIG != 0) as u8,
        vmin: t.c_cc[VMIN],
        vtime: t.c_cc[VTIME],
        tostop: (t.c_lflag & TOSTOP != 0) as u8,
        ..MikuTermios::default()
    };
    for (i, &slot) in CC_MAP.iter().enumerate() {
        k.c_cc[i] = t.c_cc[slot];
    }
    let r = unsafe { crate::sys::sc3(crate::sys::SYS_IOCTL, fd as u64, TCSETS, &k as *const _ as u64) };
    if r < 0 { fail(-r as i32) } else { 0 }
}

/// The BSD definition every libc shares
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn cfmakeraw(t: *mut termios) {
    if t.is_null() { return; }
    let t = unsafe { &mut *t };
    t.c_iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    t.c_oflag &= !OPOST;
    t.c_lflag &= !(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
    t.c_cflag &= !(CSIZE | PARENB);
    t.c_cflag |= CS8;
    t.c_cc[VMIN] = 1;
    t.c_cc[VTIME] = 0;
    let _ = INPCK;
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn cfgetispeed(t: *const termios) -> u32 {
    if t.is_null() { 0 } else { unsafe { (*t).c_ispeed } }
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn cfgetospeed(t: *const termios) -> u32 {
    if t.is_null() { 0 } else { unsafe { (*t).c_ospeed } }
}

/// A speed is stored, as POSIX asks, with no effect when applied
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn cfsetispeed(t: *mut termios, speed: u32) -> i32 {
    if t.is_null() || speed & !CBAUD != 0 { return fail(EINVAL); }
    unsafe { (*t).c_ispeed = speed };
    0
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn cfsetospeed(t: *mut termios, speed: u32) -> i32 {
    if t.is_null() || speed & !CBAUD != 0 { return fail(EINVAL); }
    unsafe {
        (*t).c_ospeed = speed;
        (*t).c_cflag = ((*t).c_cflag & !CBAUD) | speed;
    }
    0
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn cfsetspeed(t: *mut termios, speed: u32) -> i32 {
    let r = unsafe { cfsetospeed(t, speed) };
    if r != 0 { return r; }
    unsafe { cfsetispeed(t, speed) }
}

/// Output is written through as produced, so nothing is queued. A non-terminal
/// descriptor is still `ENOTTY`
#[no_mangle]
#[inline(never)]
pub extern "C" fn tcdrain(fd: i32) -> i32 {
    match get(fd) {
        Ok(_) => 0,
        Err(_) => fail(ENOTTY),
    }
}

/// Discarding queued input or output: no kernel call for it
#[no_mangle]
#[inline(never)]
pub extern "C" fn tcflush(fd: i32, queue: i32) -> i32 {
    if !(0..=2).contains(&queue) { return fail(EINVAL); }
    if get(fd).is_err() { return fail(ENOTTY); }
    fail(ENOSYS)
}

/// Software flow control (XON/XOFF): not supported by a pty here
#[no_mangle]
#[inline(never)]
pub extern "C" fn tcflow(fd: i32, action: i32) -> i32 {
    if !(0..=3).contains(&action) { return fail(EINVAL); }
    if get(fd).is_err() { return fail(ENOTTY); }
    fail(ENOSYS)
}

/// A break is a line condition, and there is no line
#[no_mangle]
#[inline(never)]
pub extern "C" fn tcsendbreak(fd: i32, _duration: i32) -> i32 {
    if get(fd).is_err() { return fail(ENOTTY); }
    fail(ENOSYS)
}

/// `ioctl`'s terminal-settings requests, routed from `libc::ioctl`, so `ioctl`
/// and `tcgetattr` agree. `None` for any other request
pub fn ioctl_termios(fd: i32, request: u64, arg: *mut u8) -> Option<i32> {
    const TCSETSW: u64 = 0x5403;
    const TCSETSF: u64 = 0x5404;
    match request {
        TCGETS => Some(unsafe { tcgetattr(fd, arg as *mut termios) }),
        TCSETS => Some(unsafe { tcsetattr(fd, 0, arg as *const termios) }),
        TCSETSW => Some(unsafe { tcsetattr(fd, 1, arg as *const termios) }),
        TCSETSF => Some(unsafe { tcsetattr(fd, 2, arg as *const termios) }),
        _ => None,
    }
}

// unistd.h: the terminal's job
//
// `tty(TTY_GETFG / TTY_SETFG)` act on the caller's controlling terminal, the
// only one POSIX lets these name. `fd` is checked to be a terminal; one on
// another terminal is answered about the controlling one, since no call says
// which terminal a slave descriptor is on.

const TTY_SETFG: u64 = 4;
const TTY_GETFG: u64 = 5;

fn tty(op: u64, arg: u64) -> i64 {
    unsafe { crate::sys::sc2(crate::sys::SYS_TTY, op, arg) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tcgetpgrp(fd: i32) -> i32 {
    if crate::libc::isatty(fd) == 0 { return fail(ENOTTY); }
    let r = tty(TTY_GETFG, 0);
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tcsetpgrp(fd: i32, pgrp: i32) -> i32 {
    if pgrp <= 0 { return fail(EINVAL); }
    if crate::libc::isatty(fd) == 0 { return fail(ENOTTY); }
    let r = tty(TTY_SETFG, pgrp as u64);
    if r < 0 { fail(-r as i32) } else { 0 }
}

/// `ENOSYS`: no sessions here (see `setsid`)
#[no_mangle]
#[inline(never)]
pub extern "C" fn tcgetsid(fd: i32) -> i32 {
    if crate::libc::isatty(fd) == 0 { return fail(ENOTTY); }
    fail(ENOSYS)
}
