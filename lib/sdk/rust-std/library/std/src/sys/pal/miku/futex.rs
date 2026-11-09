//! The futex, as `std::sync` wants it.
//!
//! With it, `Mutex`, `Condvar`, `RwLock`, `Once` and the thread parker are
//! upstream's `futex.rs` implementations, unchanged.
//!
//! `futex`(110) is the syscall; see `dev/std-architecture.md` §2.

use crate::sync::atomic::Atomic;
use crate::sys::pal::abi;
use crate::time::Duration;

/// An atomic for use as a futex that is at least 32 bits.
pub type Futex = Atomic<Primitive>;
pub type Primitive = u32;

/// The same, for the places that would settle for eight. The kernel compares
/// four bytes, so these are the same type
pub type SmallFutex = Atomic<SmallPrimitive>;
pub type SmallPrimitive = u32;

/// Sleep while `futex` holds `expected`.
///
/// Returns false only on timeout, which upstream's `Condvar` relies on. A wake,
/// a spurious return, or the word having moved before the sleep began is
/// `true`, and the caller re-reads the word.
pub fn futex_wait(futex: &Atomic<u32>, expected: u32, timeout: Option<Duration>) -> bool {
    let ns = match timeout {
        // Zero means "no timeout" to the syscall, so a zero-length wait becomes
        // the shortest real one
        Some(d) => d.as_nanos().min(u64::MAX as u128).max(1) as u64,
        None => 0,
    };
    let r = abi::miku_futex_wait(futex.as_ptr() as *const u32, expected, ns);
    r != -110 // -ETIMEDOUT
}

#[inline]
pub fn futex_wake(futex: &Atomic<u32>) -> bool {
    abi::miku_futex_wake(futex.as_ptr() as *const u32, 1) > 0
}

#[inline]
pub fn futex_wake_all(futex: &Atomic<u32>) {
    abi::miku_futex_wake(futex.as_ptr() as *const u32, u32::MAX);
}
