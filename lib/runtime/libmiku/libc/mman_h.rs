//! `<sys/mman.h>`

use crate::libc::*;

pub const PROT_NONE: u64 = miku_abi::mman::PROT_NONE as u64;

pub const PROT_READ: u64 = miku_abi::mman::PROT_READ as u64;

pub const PROT_WRITE: u64 = miku_abi::mman::PROT_WRITE as u64;

pub const PROT_EXEC: u64 = miku_abi::mman::PROT_EXEC as u64;

pub const MAP_FAILED: *mut u8 = !0 as *mut u8;

pub(crate) use miku_abi::mman::MAP_ANONYMOUS;

/// Anonymous memory is `mmap`(3); a descriptor without `MAP_ANONYMOUS` is
/// `mmap_file`(60), shared or private per the flags. SQLite's WAL index uses
/// such a mapping
#[no_mangle]
#[inline(never)]
pub extern "C" fn mmap(
    addr: *mut u8, length: usize, prot: i32,
    flags: i32, fd: i32, offset: i64,
) -> *mut u8 {
    if flags & MAP_ANONYMOUS != 0 || fd < 0 {
        let p = proc::miku_mmap(addr as u64, length, prot as u64);
        if p.is_null() { set_errno(ENOMEM); return MAP_FAILED; }
        return p;
    }
    let args: [u64; 6] = [addr as u64, length as u64, prot as u64, flags as u64, fd as u64, offset as u64];
    let r = unsafe { sc1(crate::sys::SYS_MMAP_FILE, args.as_ptr() as u64) };
    if r < 0 { set_errno(-r as i32); MAP_FAILED } else { r as *mut u8 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn msync(addr: *mut u8, length: usize, _flags: i32) -> i32 {
    ret(proc::miku_msync(addr, length)) as i32
}

/// Nothing to do: MikuOS never pages memory out, so every mapped page is
/// effectively locked
#[no_mangle]
#[inline(never)]
pub extern "C" fn mlock(_addr: *const u8, _len: usize) -> i32 { 0 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn munlock(_addr: *const u8, _len: usize) -> i32 { 0 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn mlockall(_flags: i32) -> i32 { 0 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn munlockall() -> i32 { 0 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn munmap(addr: *mut u8, length: usize) -> i32 {
    let r = proc::miku_munmap(addr, length);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn mprotect(addr: *mut u8, length: usize, prot: i32) -> i32 {
    let r = proc::miku_mprotect(addr as u64, length, prot as u64);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn brk(addr: *mut u8) -> i32 {
    let r = proc::miku_brk(addr as u64);
    if r == 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sbrk(increment: i64) -> *mut u8 {
    let cur = proc::miku_brk(0);
    if increment == 0 { return cur as *mut u8; }
    let new_brk = if increment > 0 {
        cur.saturating_add(increment as u64)
    } else {
        // use wrapping_neg to handle i64::MIN safely
        cur.saturating_sub((increment.wrapping_neg() as u64).min(i64::MAX as u64 + 1))
    };
    let r = proc::miku_brk(new_brk);
    if r == cur { MAP_FAILED } else { cur as *mut u8 }
}

// sys/mman.h: advice

/// Advice, which POSIX lets a system ignore, except `MADV_DONTNEED`, which on
/// Linux drops pages and reads back zeros. That is impossible here, so it is
/// refused
#[no_mangle]
#[inline(never)]
pub extern "C" fn madvise(_addr: *mut u8, _len: usize, advice: i32) -> i32 {
    use miku_abi::mman::MADV_DONTNEED;
    match advice {
        0..=3 => 0, // NORMAL, RANDOM, SEQUENTIAL, WILLNEED
        MADV_DONTNEED => fail(ENOSYS),
        _ => fail(EINVAL),
    }
}

/// POSIX's form, advice only in every case: `POSIX_MADV_DONTNEED` does not
/// promise zeros. The error is the return value
#[no_mangle]
#[inline(never)]
pub extern "C" fn posix_madvise(_addr: *mut u8, _len: usize, advice: i32) -> i32 {
    if (0..=4).contains(&advice) { 0 } else { EINVAL }
}

/// Setting a symlink's own times: `ENOSYS`
#[no_mangle]
#[inline(never)]
pub extern "C" fn lutimes(_path: *const u8, _times: *const u8) -> i32 {
    fail(ENOSYS)
}
