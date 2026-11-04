use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::Port;
use core::fmt;
use x86_64::instructions::interrupts;

use crate::kcore::klog::{Level, Site};

/// Divisor off the 1.8432 MHz UART clock: 115200 / DIVISOR is the line rate.
///
/// `_print` holds interrupts off for a whole line, so the rate matters: 115200
/// baud is 87us/char. Every 16550 and USB serial adapter does 115200, and QEMU
/// ignores the divisor
const DIVISOR: u16 = 1;

/// Bytes the transmit FIFO takes in one go once we have seen THRE.
///
/// A 16550A latches sixteen; a plain 16450 has a one-byte holding register and
/// would drop the other fifteen, so this is read back out of IIR after the FIFO
/// enable and falls to 1 if the chip did not light the FIFO-enabled bits
const FIFO_DEPTH: u8 = 16;

pub struct Serial {
    port: u16,
    /// How many bytes may still go out before THRE has to be re-checked.
    /// Counts down from the depth the chip admitted to having
    fifo_room: u8,
    fifo_depth: u8,
}

lazy_static! {
    pub static ref COM1: Mutex<Serial> = Mutex::new(Serial::new(0x3F8));
}

impl Serial {
    pub fn new(port: u16) -> Self {
        let mut s = Self { port, fifo_room: 0, fifo_depth: 1 };
        s.init();
        s
    }

    fn init(&mut self) {
        unsafe {
            Port::new(self.port + 1).write(0u8);              // no interrupts
            Port::new(self.port + 3).write(0x80u8);           // DLAB on
            Port::new(self.port + 0).write(DIVISOR as u8);
            Port::new(self.port + 1).write((DIVISOR >> 8) as u8);
            Port::new(self.port + 3).write(0x03u8);           // 8N1, DLAB off
            Port::new(self.port + 2).write(0xC7u8);           // FIFO on, clear, 14-byte trigger
            Port::new(self.port + 4).write(0x0Bu8);           // DTR/RTS/OUT2

            // IIR bits 7:6 read back as 11 only when the FIFOs came up;
            // anything else means a 16450 (or an emulation of one) holding one
            // byte
            let iir: u8 = Port::<u8>::new(self.port + 2).read();
            self.fifo_depth = if iir & 0xC0 == 0xC0 { FIFO_DEPTH } else { 1 };
        }
        self.fifo_room = 0;
    }

    fn is_transmit_empty(&self) -> bool {
        unsafe { Port::<u8>::new(self.port + 5).read() & 0x20 != 0 }
    }

    pub fn write_byte(&mut self, b: u8) {
        // THRE on a FIFO part means the whole transmit FIFO drained, so
        // re-reading LSR per byte paces the CPU at one character per
        // character-time. Read it once, then hand the chip a full FIFO
        if self.fifo_room == 0 {
            while !self.is_transmit_empty() {
                core::hint::spin_loop();
            }
            self.fifo_room = self.fifo_depth;
        }
        self.fifo_room -= 1;
        unsafe {
            Port::new(self.port).write(b);
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for b in s.bytes() {
            if b == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(b);
        }
    }
}

impl core::fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_str(s);
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    _print_at(Level::Info, Site::UNKNOWN, 0, args);
}

/// A print, once: every piece `args` formats into goes to the UART and to the
/// kernel log's line assembler (`kcore::klog`), so the log is what the wire
/// said, split into lines, with who said it
#[doc(hidden)]
pub fn _print_at(level: Level, site: Site, flags: u8, args: fmt::Arguments) {
    use core::fmt::Write;
    struct Tee<'a> {
        com: &'a mut Serial,
        log: crate::kcore::klog::line::Writer,
    }
    impl fmt::Write for Tee<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.com.write_str(s);
            self.log.write_str(s)
        }
    }
    interrupts::without_interrupts(|| {
        let mut com = COM1.lock();
        let mut tee = Tee { com: &mut com, log: crate::kcore::klog::line::Writer { level, site, flags } };
        let _ = tee.write_fmt(args);
    });
}

/// The console's own output mirrored to the wire: what the kernel shell prints
/// when no window is showing it. Kept in the log as output (`ECHO`), except for
/// the log viewer's, which would feed itself, and what `klog::unrecorded`
/// wraps. Signed with whoever is speaking (`klog::speaking_for`)
pub fn echo(site: Site, args: fmt::Arguments) {
    let site = crate::kcore::klog::effective(site);
    if crate::kcore::klog::silent(site.module) {
        return;
    }
    _print_at(Level::Info, site, crate::kcore::klog::flags::ECHO, args);
}

/// Whether per-operation traces are printed. Off unless the boot line carries
/// the word `trace`.
///
/// `write_byte` spins on the UART's transmit-holding register and `_print`
/// holds interrupts off for the whole line. At 115200 baud a character costs
/// 87us, so a forty-char line costs 3.5ms with interrupts off and no timer
/// tick: a fifth of a compositor frame. Fine once per boot step, not per
/// `open()`.
static TRACE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_trace(on: bool) {
    TRACE.store(on, core::sync::atomic::Ordering::Relaxed);
}

#[inline]
pub fn tracing() -> bool {
    TRACE.load(core::sync::atomic::Ordering::Relaxed)
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::arch::x86_64::serial::_print_at(
            $crate::kcore::klog::Level::Info, $crate::klog_site!(), 0, format_args!($($arg)*))
    };
}

/// A line that prints only when tracing is on; see `TRACE` above.
///
/// For traces per operation: every `open`, every `close`, every path a syscall
/// touches. Errors and boot steps stay on `serial_println!`. In the kernel log
/// at TRACE, and only when printed: formatting hot paths for the log is the
/// cost the flag avoids
#[macro_export]
macro_rules! trace_println {
    ($($arg:tt)*) => {
        if $crate::arch::x86_64::serial::tracing() {
            $crate::arch::x86_64::serial::_print_at(
                $crate::kcore::klog::Level::Trace, $crate::klog_site!(), 0,
                format_args!("{}\n", format_args!($($arg)*)));
        }
    };
}

#[macro_export]
macro_rules! serial_println {
    () => { $crate::serial_print!("\n") };
    ($($arg:tt)*) => {
        $crate::serial_print!("{}\n", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::serial_println!("[log] {}", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_err {
    ($($arg:tt)*) => {
        $crate::arch::x86_64::serial::_print_at(
            $crate::kcore::klog::Level::Error, $crate::klog_site!(), 0,
            format_args!("[error] {}\n", format_args!($($arg)*)))
    };
}
