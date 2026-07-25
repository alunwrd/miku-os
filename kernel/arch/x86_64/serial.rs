use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::Port;
use core::fmt;
use x86_64::instructions::interrupts;

pub struct Serial {
    port: u16,
}

lazy_static! {
    pub static ref COM1: Mutex<Serial> = Mutex::new(Serial::new(0x3F8));
}

impl Serial {
    pub fn new(port: u16) -> Self {
        let mut s = Self { port };
        s.init();
        s
    }

    fn init(&mut self) {
        unsafe {
            Port::new(self.port + 1).write(0u8);
            Port::new(self.port + 3).write(0x80u8);
            Port::new(self.port + 0).write(0x03u8);
            Port::new(self.port + 1).write(0u8);
            Port::new(self.port + 3).write(0x03u8);
            Port::new(self.port + 2).write(0xC7u8);
            Port::new(self.port + 4).write(0x0Bu8);
        }
    }

    fn is_transmit_empty(&self) -> bool {
        unsafe { Port::<u8>::new(self.port + 5).read() & 0x20 != 0 }
    }

    pub fn write_byte(&mut self, b: u8) {
        while !self.is_transmit_empty() {}
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
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        let _ = COM1.lock().write_fmt(args);
    });
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::arch::x86_64::serial::_print(format_args!($($arg)*))
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
        $crate::serial_println!("[error] {}", format_args!($($arg)*))
    };
}
