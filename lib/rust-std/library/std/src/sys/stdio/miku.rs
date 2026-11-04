//! Standard input and output.
//!
//! Descriptors 0, 1 and 2 are the ABI's own: `kernel/syscall/mod.rs` reserves
//! them before any other descriptor is handed out. A `miku` arm is required:
//! upstream's `unsupported` write is `Ok(buf.len())`, which drops the bytes.

use crate::io;
use crate::sys::pal::{abi, cvt};

pub struct Stdin;
pub struct Stdout;
pub struct Stderr;

impl Stdin {
    pub const fn new() -> Stdin {
        Stdin
    }
}

impl io::Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_read(0, buf.as_mut_ptr(), buf.len()))?;
        Ok(n as usize)
    }
}

impl Stdout {
    pub const fn new() -> Stdout {
        Stdout
    }
}

impl io::Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_write(1, buf.as_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        // Nothing is buffered on this side of the call
        Ok(())
    }
}

impl Stderr {
    pub const fn new() -> Stderr {
        Stderr
    }
}

impl io::Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = cvt(abi::miku_write(2, buf.as_ptr(), buf.len()))?;
        Ok(n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What `std` reads at a time when it has to buffer a line.
pub const STDIN_BUF_SIZE: usize = 8 * 1024;

pub fn is_ebadf(err: &io::Error) -> bool {
    err.raw_os_error() == Some(9) // EBADF
}

/// Where a panic goes when the usual path cannot be trusted.
///
/// `Some(Vec::new())`: `None` means "this platform has nowhere to print a
/// panic", and MikuOS has descriptor 2.
pub fn panic_output() -> Option<impl io::Write> {
    Some(Stderr::new())
}
