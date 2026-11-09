// Diagnostics sink, installed by whoever links the crate.
//
// A function pointer: set once at boot and read from every path in the driver,
// including under the filesystem lock, and a load cannot fail or allocate.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Zero means no listener, as in a host test unless asked
static SINK: AtomicUsize = AtomicUsize::new(0);

/// Installs the sink. The kernel points it at the serial console at boot; a
/// test can use `println!`
pub fn set_log_sink(f: fn(&str)) {
    SINK.store(f as usize, Ordering::Relaxed);
}

/// Drop the sink again. Mainly for tests that assert on quiet
pub fn clear_log_sink() {
    SINK.store(0, Ordering::Relaxed);
}

pub fn log(s: &str) {
    let p = SINK.load(Ordering::Relaxed);
    if p == 0 {
        return;
    }
    // Set only by `set_log_sink`, taking a `fn(&str)`, so the `usize` round
    // trip recovers what was stored
    let f: fn(&str) = unsafe { core::mem::transmute(p) };
    f(s);
}

/// A line of driver output, going nowhere until a sink is installed
#[macro_export]
macro_rules! extlog {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let mut buf = $crate::plat::LogBuf::new();
        let _ = write!(buf, $($arg)*);
        buf.flush();
    }};
}

/// A line at a time on the stack, with no `format!`: these paths may be out of
/// blocks or heap
pub struct LogBuf {
    buf: [u8; 256],
    len: usize,
}

impl Default for LogBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl LogBuf {
    pub fn new() -> Self {
        Self { buf: [0; 256], len: 0 }
    }

    pub fn flush(&mut self) {
        if self.len == 0 {
            return;
        }
        if let Ok(s) = core::str::from_utf8(&self.buf[..self.len]) {
            log(s);
            log("\n");
        }
        self.len = 0;
    }
}

impl core::fmt::Write for LogBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // A too-long line is truncated; the front identifies it
        let room = self.buf.len() - self.len;
        let n = room.min(s.len());
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
