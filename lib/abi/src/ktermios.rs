//! The kernel's terminal settings: no baud rates or modem lines
//!
//! `TCGETS` and `TCSETS` read and write this. `libc_miku` translates Linux's
//! 60-byte `termios` onto it, whose `V*` indices differ from these.

/// Control characters a line discipline acts on
pub const NCCS: usize = 7;

// Slots of `MikuTermios::c_cc`
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VWERASE: usize = 5;
pub const VSUSP: usize = 6;
/// In a slot, turns that character off
pub const VDISABLE: u8 = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MikuTermios {
    pub icanon: u8,
    pub echo: u8,
    pub isig: u8,
    /// How few characters a raw-mode read may return with
    pub vmin: u8,
    /// How long it waits, in tenths of a second
    pub vtime: u8,
    /// Stops a background job that writes here. POSIX's `TOSTOP`
    pub tostop: u8,
    pub c_cc: [u8; NCCS],
    pub _pad: [u8; 1],
}

/// `TIOCGWINSZ` and `TIOCSWINSZ` read the first two fields of a `winsize`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MikuWinSize {
    pub rows: u16,
    pub cols: u16,
}

const _: () = assert!(core::mem::size_of::<MikuTermios>() == 14);
