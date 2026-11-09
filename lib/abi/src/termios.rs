//! `<termios.h>`: Linux's 60-byte form. The kernel's own is `ktermios`

#![allow(unused_imports)]

use super::prelude::*;

// termios, Linux's values. `libc_miku.so` translates Linux's `struct termios`
// onto the kernel's (`lib/runtime/libmiku/libc/termios.rs`): `ICANON`, `ECHO`, `ISIG`,
// `TOSTOP`, the control characters, `VMIN` and `VTIME` act. Speeds, parity and
// output post-processing are accepted by `tcsetattr` and not reported back by
// `tcgetattr`
pub const NCCS: usize = 32;
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSWTC: usize = 7;
pub const VSTART: usize = 8;
pub const VSTOP: usize = 9;
pub const VSUSP: usize = 10;
pub const VEOL: usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE: usize = 14;
pub const VLNEXT: usize = 15;
pub const VEOL2: usize = 16;
pub const IGNBRK: tcflag_t = 0o1;
pub const BRKINT: tcflag_t = 0o2;
pub const IGNPAR: tcflag_t = 0o4;
pub const PARMRK: tcflag_t = 0o10;
pub const INPCK: tcflag_t = 0o20;
pub const ISTRIP: tcflag_t = 0o40;
pub const INLCR: tcflag_t = 0o100;
pub const IGNCR: tcflag_t = 0o200;
pub const ICRNL: tcflag_t = 0o400;
pub const IUCLC: tcflag_t = 0o1000;
pub const IXON: tcflag_t = 0o2000;
pub const IXANY: tcflag_t = 0o4000;
pub const IXOFF: tcflag_t = 0o10000;
pub const IMAXBEL: tcflag_t = 0o20000;
pub const IUTF8: tcflag_t = 0o40000;
pub const OPOST: tcflag_t = 0o1;
pub const OLCUC: tcflag_t = 0o2;
pub const ONLCR: tcflag_t = 0o4;
pub const OCRNL: tcflag_t = 0o10;
pub const ONOCR: tcflag_t = 0o20;
pub const ONLRET: tcflag_t = 0o40;
pub const OFILL: tcflag_t = 0o100;
pub const OFDEL: tcflag_t = 0o200;
pub const NLDLY: tcflag_t = 0o400;
pub const NL0: tcflag_t = 0;
pub const NL1: tcflag_t = 0o400;
pub const CRDLY: tcflag_t = 0o3000;
pub const CR0: tcflag_t = 0;
pub const CR1: tcflag_t = 0o1000;
pub const CR2: tcflag_t = 0o2000;
pub const CR3: tcflag_t = 0o3000;
pub const TABDLY: tcflag_t = 0o14000;
pub const TAB0: tcflag_t = 0;
pub const TAB1: tcflag_t = 0o4000;
pub const TAB2: tcflag_t = 0o10000;
pub const TAB3: tcflag_t = 0o14000;
pub const XTABS: tcflag_t = 0o14000;
pub const BSDLY: tcflag_t = 0o20000;
pub const BS0: tcflag_t = 0;
pub const BS1: tcflag_t = 0o20000;
pub const VTDLY: tcflag_t = 0o40000;
pub const VT0: tcflag_t = 0;
pub const VT1: tcflag_t = 0o40000;
pub const FFDLY: tcflag_t = 0o100000;
pub const FF0: tcflag_t = 0;
pub const FF1: tcflag_t = 0o100000;
pub const CBAUD: tcflag_t = 0o10017;
pub const CSIZE: tcflag_t = 0o60;
pub const CS5: tcflag_t = 0;
pub const CS6: tcflag_t = 0o20;
pub const CS7: tcflag_t = 0o40;
pub const CS8: tcflag_t = 0o60;
pub const CSTOPB: tcflag_t = 0o100;
pub const CREAD: tcflag_t = 0o200;
pub const PARENB: tcflag_t = 0o400;
pub const PARODD: tcflag_t = 0o1000;
pub const HUPCL: tcflag_t = 0o2000;
pub const CLOCAL: tcflag_t = 0o4000;
pub const CBAUDEX: tcflag_t = 0o10000;
pub const CMSPAR: tcflag_t = 0o10000000000;
pub const CRTSCTS: tcflag_t = 0o20000000000;
pub const ISIG: tcflag_t = 0o1;
pub const ICANON: tcflag_t = 0o2;
pub const XCASE: tcflag_t = 0o4;
pub const ECHO: tcflag_t = 0o10;
pub const ECHOE: tcflag_t = 0o20;
pub const ECHOK: tcflag_t = 0o40;
pub const ECHONL: tcflag_t = 0o100;
pub const NOFLSH: tcflag_t = 0o200;
pub const TOSTOP: tcflag_t = 0o400;
pub const ECHOCTL: tcflag_t = 0o1000;
pub const ECHOPRT: tcflag_t = 0o2000;
pub const ECHOKE: tcflag_t = 0o4000;
pub const FLUSHO: tcflag_t = 0o10000;
pub const PENDIN: tcflag_t = 0o40000;
pub const IEXTEN: tcflag_t = 0o100000;
pub const EXTPROC: tcflag_t = 0o200000;
pub const B0: speed_t = 0;
pub const B50: speed_t = 1;
pub const B75: speed_t = 2;
pub const B110: speed_t = 3;
pub const B134: speed_t = 4;
pub const B150: speed_t = 5;
pub const B200: speed_t = 6;
pub const B300: speed_t = 7;
pub const B600: speed_t = 0o10;
pub const B1200: speed_t = 0o11;
pub const B1800: speed_t = 0o12;
pub const B2400: speed_t = 0o13;
pub const B4800: speed_t = 0o14;
pub const B9600: speed_t = 0o15;
pub const B19200: speed_t = 0o16;
pub const B38400: speed_t = 0o17;
pub const B57600: speed_t = 0o10001;
pub const B115200: speed_t = 0o10002;
pub const B230400: speed_t = 0o10003;
pub const B460800: speed_t = 0o10004;
pub const B500000: speed_t = 0o10005;
pub const B576000: speed_t = 0o10006;
pub const B921600: speed_t = 0o10007;
pub const B1000000: speed_t = 0o10010;
pub const B1152000: speed_t = 0o10011;
pub const B1500000: speed_t = 0o10012;
pub const B2000000: speed_t = 0o10013;
pub const B2500000: speed_t = 0o10014;
pub const B3000000: speed_t = 0o10015;
pub const B3500000: speed_t = 0o10016;
pub const B4000000: speed_t = 0o10017;
pub const TCSANOW: c_int = 0;
pub const TCSADRAIN: c_int = 1;
pub const TCSAFLUSH: c_int = 2;
pub const TCIFLUSH: c_int = 0;
pub const TCOFLUSH: c_int = 1;
pub const TCIOFLUSH: c_int = 2;
pub const TCOOFF: c_int = 0;
pub const TCOON: c_int = 1;
pub const TCIOFF: c_int = 2;
pub const TCION: c_int = 3;
// Below: for Rust only, not in the C headers
pub type speed_t = u32;
pub type tcflag_t = u32;
pub type cc_t = u8;
c_structs! {
    /// `termios`, smaller than a Linux one
    ///
    /// `ioctl(TCGETS)` fills `icanon`, `echo`, `isig`, `c_cc`, `vmin`, `vtime`
    /// and `tostop`. No baud rates, modem lines, or
    /// `c_iflag`/`c_oflag`/`c_cflag` bitmasks. The honoured flags are in
    /// `kernel/syscall/adapters/io.rs`
    pub struct termios {
        pub c_iflag: tcflag_t,
        pub c_oflag: tcflag_t,
        pub c_cflag: tcflag_t,
        pub c_lflag: tcflag_t,
        pub c_line: cc_t,
        pub c_cc: [cc_t; 32],
        pub c_ispeed: speed_t,
        pub c_ospeed: speed_t,
    }
}
