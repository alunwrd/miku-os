// Constant-time comparison and buffer wiping. The accumulator is folded over
// the whole slice and read through `read_volatile`, leaving no branch to
// optimize

/// True when equal, in time depending only on length. Different lengths return
/// at once, since a hash's length is public
pub fn eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    // Volatile, so the accumulator cannot be optimized away
    let observed = unsafe { core::ptr::read_volatile(&diff) };
    observed == 0
}

/// Overwrites a secret buffer in a way the compiler cannot remove
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
