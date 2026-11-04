//! Entropy, from the kernel.
//!
//! `getrandom`(78) caps a call at 1024 bytes, so this loops. The kernel's pool
//! is the only source.

use crate::sys::pal::abi;

/// The kernel's per-call ceiling, from `kernel/syscall/adapters/process.rs`
const GETRANDOM_MAX: usize = 1024;

pub fn fill_bytes(bytes: &mut [u8]) {
    let mut filled = 0;
    while filled < bytes.len() {
        let want = (bytes.len() - filled).min(GETRANDOM_MAX);
        // SAFETY: `filled < bytes.len()`, so the pointer is inside the slice
        // and `want` bytes fit from it
        let n = abi::miku_getrandom(unsafe { bytes.as_mut_ptr().add(filled) }, want);
        if n <= 0 {
            // Every caller is seeding something that must not be guessable, so
            // there is no weaker fallback
            panic!("miku_getrandom failed: {n}");
        }
        filled += n as usize;
    }
}
