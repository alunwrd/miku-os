// What this crate needs from below: entropy, the wall clock, a monotonic
// millisecond counter, and a diagnostic sink.
//
// Syscalls in ring 3, stubs on a build host, so `cargo test` can run the pure
// code.

#[cfg(feature = "userspace")]
mod imp {
    /// Unpredictable bytes, or false. No weaker fallback: a guessable key looks
    /// encrypted to the caller and is not
    pub fn random(buf: &mut [u8]) -> bool {
        miku::getrandom(buf)
    }

    /// Seconds since the Unix epoch, or None when the clock was never set. None
    /// means validity dates cannot be judged
    pub fn now_unix() -> Option<u64> {
        miku::realtime()
    }

    pub fn uptime_ms() -> u64 {
        unsafe { miku::miku_uptime_ms() }
    }

    pub fn sleep_ms(ms: u64) {
        unsafe { miku::miku_sleep_ms(ms) }
    }

    /// Diagnostics go to stderr, so stdout stays the program's output
    pub fn log(s: &str) {
        miku::eprint(s);
    }
}

#[cfg(not(feature = "userspace"))]
mod imp {
    pub fn random(buf: &mut [u8]) -> bool {
        // Host builds are for tests and need repeatable bytes. Deterministic
        // and useful only for tests, so a connecting program needs `userspace`
        static mut STATE: u64 = 0x2545_F491_4F6C_DD1D;
        for b in buf.iter_mut() {
            unsafe {
                STATE ^= STATE << 13;
                STATE ^= STATE >> 7;
                STATE ^= STATE << 17;
                *b = STATE as u8;
            }
        }
        true
    }

    /// No wall clock on the host. `verify_chain` takes the time as an argument
    /// and tests pin it
    pub fn now_unix() -> Option<u64> {
        None
    }

    pub fn uptime_ms() -> u64 { 0 }
    pub fn sleep_ms(_ms: u64) {}

    pub fn log(_s: &str) {}
}

pub use imp::*;

use core::sync::atomic::{AtomicBool, Ordering};

/// Whether informational logging prints. Failures always print; the rest
/// (handshake, cipher, chain length) only with `-v`, which sets this
static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// The running commentary. Silent unless `set_verbose(true)`
#[macro_export]
macro_rules! netlog {
    ($($arg:tt)*) => {{
        if $crate::plat::verbose() {
            use core::fmt::Write;
            let mut buf = $crate::plat::LogBuf::new();
            let _ = write!(buf, $($arg)*);
            buf.flush();
        }
    }};
}

/// Failure message; always printed
#[macro_export]
macro_rules! netlog_err {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let mut buf = $crate::plat::LogBuf::new();
        let _ = write!(buf, $($arg)*);
        buf.flush();
    }};
}

/// One line on the stack; a diagnostic must not allocate
pub struct LogBuf {
    buf: [u8; 256],
    len: usize,
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
        // Truncates on overflow
        let room = self.buf.len() - self.len;
        let take = core::cmp::min(room, s.len());
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}
