//! `<sys/ioctl.h>`

#![allow(unused_imports)]

use super::prelude::*;

/// `ioctl` requests every libc names. `ioctl`(71) implements the terminal ones;
/// these two are descriptor-level. `libc_miku.so` answers `FIONBIO` as
/// `fcntl(F_SETFL, O_NONBLOCK)` without the kernel. `FIONREAD` returns the
/// kernel's `EINVAL`
pub const FIONBIO: _Opcode = 0x5421;
pub const FIONREAD: _Opcode = 0x541B;
/// `ioctl(TIOCSCTTY)`: make this terminal the controlling one. A session leader
/// claims a terminal this way; `setsid` alone does not
pub const TIOCSCTTY: _Opcode = 0x540E;
pub const TIOCEXCL: _Opcode = 0x540C;
pub const TIOCNXCL: _Opcode = 0x540D;
pub const TIOCGWINSZ: _Opcode = 0x5413;
pub const TIOCSWINSZ: _Opcode = 0x5414;
pub const TIOCMGET: _Opcode = 0x5415;
pub const TIOCMBIS: _Opcode = 0x5416;
pub const TIOCMBIC: _Opcode = 0x5417;
pub const TIOCMSET: _Opcode = 0x5418;
pub const TIOCSBRK: _Opcode = 0x5427;
pub const TIOCCBRK: _Opcode = 0x5428;
pub const TIOCGPTN: _Opcode = 0x8004_5430;
pub const TIOCM_LE: c_int = 0x001;
pub const TIOCM_DTR: c_int = 0x002;
pub const TIOCM_RTS: c_int = 0x004;
pub const TIOCM_ST: c_int = 0x008;
pub const TIOCM_SR: c_int = 0x010;
pub const TIOCM_CTS: c_int = 0x020;
pub const TIOCM_CAR: c_int = 0x040;
pub const TIOCM_CD: c_int = TIOCM_CAR;
pub const TIOCM_RNG: c_int = 0x080;
pub const TIOCM_RI: c_int = TIOCM_RNG;
pub const TIOCM_DSR: c_int = 0x100;
// Below: for Rust only, not in the C headers
/// What `ioctl` takes as its request. Linux's `c_ulong`.
pub type _Opcode = c_ulong;
/// The terminal ioctls, Linux's numbers. `TCGETS`/`TCSETS*` go through
/// `libc_miku.so`'s translation, the window size and `TIOCGPTN` to the kernel
/// (`kernel/syscall/adapters/io.rs`). A pty has no modem or break lines, and
/// the kernel answers those `ENOTTY`
pub const TCGETS: _Opcode = 0x5401;
pub const TCSETS: _Opcode = 0x5402;
pub const TCSETSW: _Opcode = 0x5403;
pub const TCSETSF: _Opcode = 0x5404;
