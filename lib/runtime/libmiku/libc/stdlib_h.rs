//! `<stdlib.h>`

use crate::libc::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    heap::miku_malloc(size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn free(ptr: *mut u8) {
    heap::miku_free(ptr);
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    heap::miku_realloc(ptr, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn calloc(count: usize, size: usize) -> *mut u8 {
    heap::miku_calloc(count, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn aligned_alloc(align: usize, size: usize) -> *mut u8 {
    heap::miku_memalign(align, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn posix_memalign(memptr: *mut *mut u8, align: usize, size: usize) -> i32 {
    if memptr.is_null() { return EINVAL; }
    if align == 0 || (align & (align - 1)) != 0 { return EINVAL; }
    let p = heap::miku_memalign(align, size);
    if p.is_null() {
        return ENOMEM;
    }
    unsafe { *memptr = p; }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atoi(s: *const u8) -> i32 {
    num::miku_atoi(s) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atol(s: *const u8) -> i64 {
    num::miku_atoi(s)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atoll(s: *const u8) -> i64 {
    num::miku_atoi(s)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtol(s: *const u8, endptr: *mut *const u8, base: i32) -> i64 {
    convert::miku_strtol(s, endptr, base)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtoul(s: *const u8, endptr: *mut *const u8, base: i32) -> u64 {
    convert::miku_strtoul(s, endptr, base)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtoll(s: *const u8, endptr: *mut *const u8, base: i32) -> i64 {
    convert::miku_strtol(s, endptr, base)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtoull(s: *const u8, endptr: *mut *const u8, base: i32) -> u64 {
    convert::miku_strtoul(s, endptr, base)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn abs(x: i32) -> i32 {
    x.saturating_abs()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn labs(x: i64) -> i64 {
    math::miku_abs(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn llabs(x: i64) -> i64 {
    math::miku_abs(x)
}

/// C's exit: `atexit` handlers, last first, then every open stream flushed,
/// then the process ends. `_exit` and `_Exit` skip both
#[no_mangle]
#[inline(never)]
pub extern "C" fn exit(code: i32) -> ! {
    run_atexit();
    flush_all();
    proc::miku_exit(code as i64)
}

pub(crate) static ATEXIT: [core::sync::atomic::AtomicUsize; 32] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 32];

/// Thirty-two, POSIX's minimum. A thirty-third answers -1
#[no_mangle]
#[inline(never)]
pub extern "C" fn atexit(f: extern "C" fn()) -> i32 {
    use core::sync::atomic::Ordering;
    for slot in &ATEXIT {
        if slot.compare_exchange(0, f as usize, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            return 0;
        }
    }
    -1
}

pub(crate) fn run_atexit() {
    use core::sync::atomic::Ordering;
    for slot in ATEXIT.iter().rev() {
        let f = slot.swap(0, Ordering::AcqRel);
        if f != 0 {
            let f: extern "C" fn() = unsafe { core::mem::transmute(f) };
            f();
        }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn _exit(code: i32) -> ! {
    proc::miku_exit(code as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn _Exit(code: i32) -> ! {
    proc::miku_exit(code as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn abort() -> ! {
    signal::miku_signal_raise(signal::SIG_ABRT);
    proc::miku_exit(134)
}

/// A name `setenv` and `unsetenv` accept: not empty, no `=`
fn env_name_ok(key: *const u8) -> bool {
    if key.is_null() { return false; }
    let mut i = 0;
    loop {
        match unsafe { *key.add(i) } {
            0 => return i > 0,
            b'=' => return false,
            _ => i += 1,
        }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getenv(key: *const u8) -> *const u8 {
    env::miku_getenv(key)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn setenv(key: *const u8, val: *const u8, overwrite: i32) -> i32 {
    if !env_name_ok(key) || val.is_null() { return fail(EINVAL); }
    if overwrite == 0 && env::miku_hasenv(key) { return 0; }
    if env::miku_setenv(key, val) { 0 } else { fail(ENOMEM) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn unsetenv(key: *const u8) -> i32 {
    if !env_name_ok(key) { return fail(EINVAL); }
    // Removing a variable that is not there succeeds
    env::miku_unsetenv(key);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn putenv(s: *const u8) -> i32 {
    if env::miku_putenv(s) { 0 } else { -1 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn srand(seed: u32) {
    random::miku_srand(seed as u64);
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rand() -> i32 {
    (random::miku_rand() & 0x7FFFFFFF) as i32
}

/// `realpath(3)`: the kernel's answer (`miku_realpath`, 119). A null `resolved`
/// gets a buffer of the answer's size, freed by the caller; otherwise
/// `resolved` holds PATH_MAX bytes, per POSIX
#[no_mangle]
#[inline(never)]
pub extern "C" fn realpath(path: *const u8, resolved: *mut u8) -> *mut u8 {
    const PATH_MAX: usize = miku_abi::limits::PATH_MAX as usize;
    if !resolved.is_null() {
        let r = file::miku_realpath(path, resolved, PATH_MAX);
        return if r < 0 { fail(-r as i32); core::ptr::null_mut() } else { resolved };
    }
    let buf = crate::heap::miku_malloc(PATH_MAX);
    if buf.is_null() { fail(ENOMEM); return core::ptr::null_mut(); }
    let r = file::miku_realpath(path, buf, PATH_MAX);
    if r < 0 {
        crate::heap::miku_free(buf);
        fail(-r as i32);
        return core::ptr::null_mut();
    }
    let exact = crate::heap::miku_realloc(buf, r as usize + 1);
    if exact.is_null() { buf } else { exact }
}

// additional POSIX helpers //

#[no_mangle]
#[inline(never)]
pub extern "C" fn remove(path: *const u8) -> i32 {
    if file::miku_isdir(path) {
        rmdir(path)
    } else {
        unlink(path)
    }
}

// qsort using libmiku's sort
#[no_mangle]
#[inline(never)]
pub extern "C" fn qsort(
    base: *mut u8,
    nmemb: usize,
    size: usize,
    compar: extern "C" fn(*const u8, *const u8) -> i32,
) {
    if base.is_null() || nmemb <= 1 || size == 0 { return; }
    // Insertion sort for small arrays, shell sort for larger, with one temp
    // element
    let tmp = heap::miku_malloc(size);
    if tmp.is_null() { return; }

    // shell sort with Ciura gaps
    let gaps: [usize; 8] = [701, 301, 132, 57, 23, 10, 4, 1];
    for &gap in gaps.iter() {
        if gap >= nmemb { continue; }
        for i in gap..nmemb {
            unsafe {
                mem::miku_memcpy(tmp, base.add(i * size), size);
                let mut j = i;
                while j >= gap {
                    let prev = base.add((j - gap) * size);
                    if compar(prev, tmp) <= 0 { break; }
                    mem::miku_memcpy(base.add(j * size), prev, size);
                    j -= gap;
                }
                mem::miku_memcpy(base.add(j * size), tmp, size);
            }
        }
    }

    heap::miku_free(tmp);
}

// bsearch
#[no_mangle]
#[inline(never)]
pub extern "C" fn bsearch(
    key: *const u8,
    base: *const u8,
    nmemb: usize,
    size: usize,
    compar: extern "C" fn(*const u8, *const u8) -> i32,
) -> *const u8 {
    if key.is_null() || base.is_null() || nmemb == 0 { return core::ptr::null(); }
    let mut lo = 0usize;
    let mut hi = nmemb;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let elem = unsafe { base.add(mid * size) };
        let cmp = compar(key, elem);
        if cmp == 0 { return elem; }
        if cmp < 0 { hi = mid; } else { lo = mid + 1; }
    }
    core::ptr::null()
}

pub use miku_abi::libtypes::{DivT, LDivT};

#[no_mangle]
#[inline(never)]
pub extern "C" fn div(a: i32, b: i32) -> DivT {
    DivT { quot: a / b, rem: a % b }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ldiv(a: i64, b: i64) -> LDivT {
    LDivT { quot: a / b, rem: a % b }
}
