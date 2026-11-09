// Memory, in words rather than bytes (`dev/kernel-core.md`, "libc's memory
// functions")
//
// These were loops through `read_volatile`/`write_volatile`: the volatile is
// what kept the compiler from seeing a copy loop and calling `memcpy` - which
// is this function - but it also kept it from doing anything else, and every
// C program on the machine copied a byte a cycle at best. The copies are the
// string instructions now, which the compiler cannot turn back into a call,
// and the scans read a word at a time.
//
// `rep movsb` and `rep stosb` are the fast path on every x86 since Ivy Bridge
// (ERMSB). The direction flag is clear on entry to any function (SysV), which
// is what they count on.

/// A byte repeated across a word
const ONES: u64 = 0x0101_0101_0101_0101;
const HIGHS: u64 = 0x8080_8080_8080_8080;

/// Whether a byte of `w` is zero
#[inline(always)]
fn has_zero(w: u64) -> bool {
    w.wrapping_sub(ONES) & !w & HIGHS != 0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    if dst.is_null() || n == 0 { return dst; }
    unsafe {
        core::arch::asm!(
            "rep stosb",
            inout("rdi") dst => _, inout("rcx") n => _, in("al") val as u8,
            options(nostack, preserves_flags),
        );
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dst.is_null() || src.is_null() || n == 0 { return dst; }
    unsafe {
        core::arch::asm!(
            "rep movsb",
            inout("rdi") dst => _, inout("rsi") src => _, inout("rcx") n => _,
            options(nostack, preserves_flags),
        );
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dst.is_null() || src.is_null() || n == 0 { return dst; }
    // Forwards is right whenever the destination starts first, overlapping
    // or not
    if (dst as usize) <= (src as usize) || (dst as usize) >= (src as usize) + n {
        return miku_memcpy(dst, src, n);
    }
    // Backwards, a word at a time from the end: each word is read before
    // anything is written over it, however close the two are. A backward
    // `rep movsb` would do it too, and is not the fast path on any CPU
    let words = n / 8;
    let head = n % 8;
    unsafe {
        if words > 0 {
            core::arch::asm!(
                "2:",
                "sub {s}, 8",
                "sub {d}, 8",
                "mov {t}, qword ptr [{s}]",
                "mov qword ptr [{d}], {t}",
                "dec {n}",
                "jnz 2b",
                s = inout(reg) src.add(n) => _,
                d = inout(reg) dst.add(n) => _,
                n = inout(reg) words => _,
                t = out(reg) _,
                options(nostack),
            );
        }
        let mut i = head;
        while i > 0 {
            i -= 1;
            core::ptr::write_volatile(dst.add(i), core::ptr::read_volatile(src.add(i)));
        }
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0usize;
    unsafe {
        while i + 8 <= n {
            let wa = (a.add(i) as *const u64).read_unaligned();
            let wb = (b.add(i) as *const u64).read_unaligned();
            if wa != wb {
                // Little-endian: the lowest set bit is the first byte that
                // differs
                let k = ((wa ^ wb).trailing_zeros() / 8) as usize;
                return *a.add(i + k) as i32 - *b.add(i + k) as i32;
            }
            i += 8;
        }
        while i < n {
            let ca = core::ptr::read_volatile(a.add(i));
            let cb = core::ptr::read_volatile(b.add(i));
            if ca != cb { return ca as i32 - cb as i32; }
            i += 1;
        }
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_bzero(dst: *mut u8, n: usize) {
    miku_memset(dst, 0, n);
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    if s.is_null() { return core::ptr::null(); }
    let target = c as u8;
    let pattern = ONES.wrapping_mul(target as u64);
    let mut i = 0usize;
    unsafe {
        // Whole words inside the buffer only: a scan bounded by `n` reads
        // nothing past it
        while i + 8 <= n {
            let w = (s.add(i) as *const u64).read_unaligned();
            if has_zero(w ^ pattern) { break; }
            i += 8;
        }
        while i < n {
            if core::ptr::read_volatile(s.add(i)) == target {
                return s.add(i);
            }
            i += 1;
        }
    }
    core::ptr::null()
}

// reverse memchr - search from end
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memrchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    if s.is_null() || n == 0 { return core::ptr::null(); }
    let target = c as u8;
    let mut i = n;
    while i > 0 {
        i -= 1;
        if unsafe { core::ptr::read_volatile(s.add(i)) } == target {
            return unsafe { s.add(i) };
        }
    }
    core::ptr::null()
}

// find byte sequence in memory (like strstr but for raw bytes)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memmem(
    haystack: *const u8, hlen: usize,
    needle: *const u8, nlen: usize,
) -> *const u8 {
    if haystack.is_null() || needle.is_null() { return core::ptr::null(); }
    if nlen == 0 { return haystack; }
    if nlen > hlen { return core::ptr::null(); }

    let limit = hlen - nlen;
    for i in 0..=limit {
        if miku_memcmp(unsafe { haystack.add(i) }, needle, nlen) == 0 {
            return unsafe { haystack.add(i) };
        }
    }
    core::ptr::null()
}
