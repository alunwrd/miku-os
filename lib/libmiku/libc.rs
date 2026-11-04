// libc compatibility layer: POSIX and C standard library names (strlen, malloc,
// printf, ...) mapped to libmiku, for porting C programs

use crate::{
    mem, heap, string, ctype, convert, num, io, file, proc, env,
    signal, time, math, random, errno,
};

// errno.h //

// POSIX errno values //
pub const EPERM:   i32 = 1;
pub const ENOENT:  i32 = 2;
pub const ESRCH:   i32 = 3;
pub const EINTR:   i32 = 4;
pub const EIO:     i32 = 5;
pub const ENXIO:   i32 = 6;
pub const E2BIG:   i32 = 7;
pub const ENOEXEC: i32 = 8;
pub const EBADF:   i32 = 9;
pub const ECHILD:  i32 = 10;
pub const EAGAIN:  i32 = 11;
pub const ENOMEM:  i32 = 12;
pub const EACCES:  i32 = 13;
pub const EFAULT:  i32 = 14;
pub const EBUSY:   i32 = 16;
pub const EEXIST:  i32 = 17;
pub const ENOTDIR: i32 = 20;
pub const EISDIR:  i32 = 21;
pub const EINVAL:  i32 = 22;
pub const EMFILE:  i32 = 24;
pub const EFBIG:   i32 = 27;
pub const ENOSPC:  i32 = 28;
pub const EPIPE:   i32 = 32;
pub const EDOM:    i32 = 33;
pub const ERANGE:  i32 = 34;
pub const ENOSYS:  i32 = 38;

/// This thread's `errno`, the TCB word `errno::errno_cell` names. An `int *`,
/// as C has it. See `errno.rs`
#[no_mangle]
#[inline(never)]
pub extern "C" fn __errno_location() -> *mut i32 {
    errno::miku_errno_location()
}

/// The same cell under the name the `errno` crate links on a system not in its
/// `cfg_attr` list: `__errno_location` for Linux, `__error` for BSDs, else bare
/// `errno_location`, as `rustix` reaches it here
#[no_mangle]
#[inline(never)]
pub extern "C" fn errno_location() -> *mut i32 {
    errno::miku_errno_location()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strerror(errnum: i64) -> *const u8 {
    errno::miku_strerror(errnum)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn perror(s: *const u8) {
    errno::miku_perror(s);
}

// string.h //

#[no_mangle]
#[inline(never)]
pub extern "C" fn strlen(s: *const u8) -> usize {
    string::miku_strlen(s)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strnlen(s: *const u8, maxlen: usize) -> usize {
    string::miku_strnlen(s, maxlen)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcmp(s1: *const u8, s2: *const u8) -> i32 {
    string::miku_strcmp(s1, s2)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    string::miku_strncmp(s1, s2, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcasecmp(s1: *const u8, s2: *const u8) -> i32 {
    string::miku_strcasecmp(s1, s2)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncasecmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    string::miku_strncasecmp(s1, s2, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    string::miku_strcpy(dst, src)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    string::miku_strncpy(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strlcpy(dst: *mut u8, src: *const u8, size: usize) -> usize {
    string::miku_strlcpy(dst, src, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcat(dst: *mut u8, src: *const u8) -> *mut u8 {
    string::miku_strcat(dst, src)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    string::miku_strncat(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strlcat(dst: *mut u8, src: *const u8, size: usize) -> usize {
    string::miku_strlcat(dst, src, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strchr(s: *const u8, c: i32) -> *const u8 {
    string::miku_strchr(s, c)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strrchr(s: *const u8, c: i32) -> *const u8 {
    string::miku_strrchr(s, c)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strstr(haystack: *const u8, needle: *const u8) -> *const u8 {
    string::miku_strstr(haystack, needle)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strpbrk(s: *const u8, accept: *const u8) -> *const u8 {
    string::miku_strpbrk(s, accept)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strspn(s: *const u8, accept: *const u8) -> usize {
    string::miku_strspn(s, accept)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcspn(s: *const u8, reject: *const u8) -> usize {
    string::miku_strcspn(s, reject)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strdup(s: *const u8) -> *mut u8 {
    string::miku_strdup(s)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strndup(s: *const u8, n: usize) -> *mut u8 {
    string::miku_strndup(s, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtok(s: *mut u8, delim: *const u8) -> *mut u8 {
    string::miku_strtok(s, delim)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtok_r(s: *mut u8, delim: *const u8, saveptr: *mut *mut u8) -> *mut u8 {
    string::miku_strtok_r(s, delim, saveptr)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strsep(stringp: *mut *mut u8, delim: *const u8) -> *mut u8 {
    string::miku_strsep(stringp, delim)
}

// string.h (memory functions) //

#[no_mangle]
#[inline(never)]
pub extern "C" fn memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    mem::miku_memset(dst, val, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    mem::miku_memcpy(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    mem::miku_memmove(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    mem::miku_memcmp(a, b, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    mem::miku_memchr(s, c, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memmem(
    haystack: *const u8, hlen: usize,
    needle: *const u8, nlen: usize,
) -> *const u8 {
    mem::miku_memmem(haystack, hlen, needle, nlen)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn bzero(dst: *mut u8, n: usize) {
    mem::miku_bzero(dst, n);
}

// stdlib.h //

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

static ATEXIT: [core::sync::atomic::AtomicUsize; 32] =
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

fn run_atexit() {
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

#[no_mangle]
#[inline(never)]
pub extern "C" fn getenv(key: *const u8) -> *const u8 {
    env::miku_getenv(key)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn setenv(key: *const u8, val: *const u8, _overwrite: i32) -> i32 {
    // libmiku setenv always overwrites; _overwrite is ignored
    if env::miku_setenv(key, val) { 0 } else { -1 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn unsetenv(key: *const u8) -> i32 {
    if env::miku_unsetenv(key) { 0 } else { -1 }
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

// stdio.h //

// FILE: an fd with buffered I/O
#[repr(C)]
pub struct FILE {
    fd: i64,
    flags: u32,
    error: i32,
    eof: i32,
    ungetc_buf: i32,    // -1 = empty
    read_buf: *mut u8,
    read_cap: usize,
    read_pos: usize,
    read_filled: usize,
    write_buf: *mut u8,
    write_cap: usize,
    write_pos: usize,
    buf_mode: u32,      // _IOFBF, _IOLBF, _IONBF
}

const FILE_READ:   u32 = 0x01;
const FILE_WRITE:  u32 = 0x02;
const FILE_APPEND: u32 = 0x04;

pub const _IOFBF: u32 = 0; // fully buffered
pub const _IOLBF: u32 = 1; // line buffered
pub const _IONBF: u32 = 2; // unbuffered

const STDIO_BUF_SIZE: usize = 4096;

pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

pub const EOF: i32 = -1;

pub const STDIN_FILENO:  i32 = 0;
pub const STDOUT_FILENO: i32 = 1;
pub const STDERR_FILENO: i32 = 2;

pub const BUFSIZ: usize = 4096;

// pre-allocated FILE objects for stdin/stdout/stderr
static mut STDIN_FILE: FILE = FILE {
    fd: 0, flags: FILE_READ, error: 0, eof: 0, ungetc_buf: -1,
    read_buf: core::ptr::null_mut(), read_cap: 0, read_pos: 0, read_filled: 0,
    write_buf: core::ptr::null_mut(), write_cap: 0, write_pos: 0,
    buf_mode: _IOLBF,
};

static mut STDOUT_FILE: FILE = FILE {
    fd: 1, flags: FILE_WRITE, error: 0, eof: 0, ungetc_buf: -1,
    read_buf: core::ptr::null_mut(), read_cap: 0, read_pos: 0, read_filled: 0,
    write_buf: core::ptr::null_mut(), write_cap: 0, write_pos: 0,
    buf_mode: _IOLBF,
};

static mut STDERR_FILE: FILE = FILE {
    fd: 2, flags: FILE_WRITE, error: 0, eof: 0, ungetc_buf: -1,
    read_buf: core::ptr::null_mut(), read_cap: 0, read_pos: 0, read_filled: 0,
    write_buf: core::ptr::null_mut(), write_cap: 0, write_pos: 0,
    buf_mode: _IONBF,
};

#[no_mangle]
pub static mut stdin:  *mut FILE = unsafe { &mut STDIN_FILE as *mut FILE };
#[no_mangle]
pub static mut stdout: *mut FILE = unsafe { &mut STDOUT_FILE as *mut FILE };
#[no_mangle]
pub static mut stderr: *mut FILE = unsafe { &mut STDERR_FILE as *mut FILE };

fn parse_mode(mode: *const u8) -> (u32, u32) {
    // returns (open_flags, file_flags)
    if mode.is_null() { return (0, 0); }
    let m0 = unsafe { *mode };
    let m1 = unsafe { *mode.add(1) };
    let has_plus = m1 == b'+' || (m1 != 0 && unsafe { *mode.add(2) } == b'+');

    match m0 {
        b'r' => {
            if has_plus {
                (file::O_RDWR, FILE_READ | FILE_WRITE)
            } else {
                (file::O_READ, FILE_READ)
            }
        }
        b'w' => {
            if has_plus {
                (file::O_RDWR | file::O_CREATE | file::O_TRUNCATE, FILE_READ | FILE_WRITE)
            } else {
                (file::O_WRITE | file::O_CREATE | file::O_TRUNCATE, FILE_WRITE)
            }
        }
        b'a' => {
            if has_plus {
                (file::O_RDWR | file::O_CREATE | file::O_APPEND, FILE_READ | FILE_WRITE | FILE_APPEND)
            } else {
                (file::O_WRITE | file::O_CREATE | file::O_APPEND, FILE_WRITE | FILE_APPEND)
            }
        }
        _ => (0, 0),
    }
}

fn file_alloc_bufs(f: *mut FILE, flags: u32) {
    let f = unsafe { &mut *f };
    if flags & FILE_READ != 0 && f.read_buf.is_null() {
        f.read_buf = heap::miku_malloc(STDIO_BUF_SIZE);
        f.read_cap = STDIO_BUF_SIZE;
        f.read_pos = 0;
        f.read_filled = 0;
    }
    if flags & FILE_WRITE != 0 && f.write_buf.is_null() {
        f.write_buf = heap::miku_malloc(STDIO_BUF_SIZE);
        f.write_cap = STDIO_BUF_SIZE;
        f.write_pos = 0;
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fopen(path: *const u8, mode: *const u8) -> *mut FILE {
    if path.is_null() || mode.is_null() { return core::ptr::null_mut(); }
    let (open_flags, file_flags) = parse_mode(mode);
    if file_flags == 0 { return core::ptr::null_mut(); }

    let plen = string::miku_strlen(path);
    let fd = file::miku_open(path, plen, open_flags, 0o644);
    if fd < 0 {
        errno::set_errno(-fd);
        return core::ptr::null_mut();
    }

    file_from_fd(fd, file_flags)
}

/// Every stream `fopen` or `fdopen` made and `fclose` has not closed, for
/// `fflush(NULL)` and `exit`. `FOPEN_MAX` in `<stdio.h>` is this size; a stream
/// past it works and is flushed only by its own `fclose`
static OPEN_STREAMS: [core::sync::atomic::AtomicUsize; 64] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 64];

fn stream_register(f: *mut FILE) {
    use core::sync::atomic::Ordering;
    for slot in &OPEN_STREAMS {
        if slot.compare_exchange(0, f as usize, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            return;
        }
    }
}

fn stream_forget(f: *mut FILE) {
    use core::sync::atomic::Ordering;
    for slot in &OPEN_STREAMS {
        let _ = slot.compare_exchange(f as usize, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}

/// `fflush(NULL)`: the standard streams and every registered one
fn flush_all() {
    use core::sync::atomic::Ordering;
    unsafe {
        fflush(stdout);
        fflush(stderr);
    }
    for slot in &OPEN_STREAMS {
        let f = slot.load(Ordering::Acquire) as *mut FILE;
        if !f.is_null() {
            fflush(f);
        }
    }
}

fn file_from_fd(fd: i64, flags: u32) -> *mut FILE {
    let f = heap::miku_calloc(1, core::mem::size_of::<FILE>()) as *mut FILE;
    if f.is_null() { return core::ptr::null_mut(); }
    let fp = unsafe { &mut *f };
    fp.fd = fd;
    fp.flags = flags;
    fp.ungetc_buf = -1;
    fp.buf_mode = _IOFBF;
    file_alloc_bufs(f, flags);
    stream_register(f);
    f
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fdopen(fd: i32, mode: *const u8) -> *mut FILE {
    let (_, file_flags) = parse_mode(mode);
    if file_flags == 0 { return core::ptr::null_mut(); }
    file_from_fd(fd as i64, file_flags)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fclose(f: *mut FILE) -> i32 {
    if f.is_null() { return EOF; }
    fflush(f);
    stream_forget(f);
    let fp = unsafe { &mut *f };
    let rc = file::miku_close(fp.fd);
    if !fp.read_buf.is_null() { heap::miku_free(fp.read_buf); }
    if !fp.write_buf.is_null() { heap::miku_free(fp.write_buf); }
    // don't free static stdin/stdout/stderr
    unsafe {
        if f != stdin && f != stdout && f != stderr {
            heap::miku_free(f as *mut u8);
        }
    }
    if rc < 0 { EOF } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fileno(f: *mut FILE) -> i32 {
    if f.is_null() { return -1; }
    unsafe { (*f).fd as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fflush(f: *mut FILE) -> i32 {
    if f.is_null() { flush_all(); return 0; }
    let fp = unsafe { &mut *f };
    if fp.write_pos > 0 && !fp.write_buf.is_null() {
        let n = io::miku_write(fp.fd as u64, fp.write_buf, fp.write_pos);
        if n < 0 {
            fp.error = 1;
            return EOF;
        }
        let written = n as usize;
        if written < fp.write_pos {
            unsafe { crate::mem::miku_memmove(fp.write_buf, fp.write_buf.add(written), fp.write_pos - written); }
            fp.write_pos -= written;
        } else {
            fp.write_pos = 0;
        }
    }
    0
}

fn file_fill_read(fp: &mut FILE) -> i32 {
    if fp.read_buf.is_null() { return EOF; }
    let n = io::miku_read(fp.fd as u64, fp.read_buf, fp.read_cap);
    if n <= 0 {
        fp.eof = 1;
        return EOF;
    }
    fp.read_pos = 0;
    fp.read_filled = n as usize;
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fgetc(f: *mut FILE) -> i32 {
    if f.is_null() { return EOF; }
    let fp = unsafe { &mut *f };

    // check ungetc buffer first
    if fp.ungetc_buf >= 0 {
        let c = fp.ungetc_buf;
        fp.ungetc_buf = -1;
        return c;
    }

    // unbuffered mode
    if fp.buf_mode == _IONBF || fp.read_buf.is_null() {
        let mut b: u8 = 0;
        let n = io::miku_read(fp.fd as u64, &mut b as *mut u8, 1);
        if n <= 0 { fp.eof = 1; return EOF; }
        return b as i32;
    }

    // buffered read
    if fp.read_pos >= fp.read_filled {
        if file_fill_read(fp) == EOF { return EOF; }
    }
    let c = unsafe { *fp.read_buf.add(fp.read_pos) };
    fp.read_pos += 1;
    c as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getc(f: *mut FILE) -> i32 {
    fgetc(f)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getchar() -> i32 {
    fgetc(unsafe { stdin })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ungetc(c: i32, f: *mut FILE) -> i32 {
    if f.is_null() || c == EOF { return EOF; }
    let fp = unsafe { &mut *f };
    fp.ungetc_buf = c;
    fp.eof = 0;
    c
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fputc(c: i32, f: *mut FILE) -> i32 {
    if f.is_null() { return EOF; }
    let fp = unsafe { &mut *f };
    let b = c as u8;

    // unbuffered mode
    if fp.buf_mode == _IONBF || fp.write_buf.is_null() {
        let n = io::miku_write(fp.fd as u64, &b as *const u8, 1);
        if n <= 0 { fp.error = 1; return EOF; }
        return c;
    }

    // buffered write
    unsafe { *fp.write_buf.add(fp.write_pos) = b; }
    fp.write_pos += 1;

    // Line-buffered: flush on newline
    let should_flush = (fp.buf_mode == _IOLBF && b == b'\n')
        || fp.write_pos >= fp.write_cap;
    if should_flush {
        fflush(f);
    }
    c
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn putc(c: i32, f: *mut FILE) -> i32 {
    fputc(c, f)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn putchar(c: i32) -> i32 {
    fputc(c, unsafe { stdout })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fgets(buf: *mut u8, size: i32, f: *mut FILE) -> *mut u8 {
    if buf.is_null() || size <= 0 || f.is_null() { return core::ptr::null_mut(); }
    let mut pos = 0usize;
    let limit = (size - 1) as usize;
    while pos < limit {
        let c = fgetc(f);
        if c == EOF {
            if pos == 0 { return core::ptr::null_mut(); }
            break;
        }
        unsafe { *buf.add(pos) = c as u8; }
        pos += 1;
        if c == b'\n' as i32 { break; }
    }
    unsafe { *buf.add(pos) = 0; }
    buf
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fputs(s: *const u8, f: *mut FILE) -> i32 {
    if s.is_null() || f.is_null() { return EOF; }
    let len = string::miku_strlen(s);
    let n = fwrite(s as *const u8, 1, len, f);
    if n < len { EOF } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn puts(s: *const u8) -> i32 {
    unsafe {
        let r = fputs(s, stdout);
        if r == EOF { return EOF; }
        fputc(b'\n' as i32, stdout);
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fread(ptr: *mut u8, size: usize, nmemb: usize, f: *mut FILE) -> usize {
    if ptr.is_null() || f.is_null() || size == 0 || nmemb == 0 { return 0; }
    let total = size * nmemb;
    let mut done = 0usize;
    while done < total {
        let c = fgetc(f);
        if c == EOF { break; }
        unsafe { *ptr.add(done) = c as u8; }
        done += 1;
    }
    done / size
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fwrite(ptr: *const u8, size: usize, nmemb: usize, f: *mut FILE) -> usize {
    if ptr.is_null() || f.is_null() || size == 0 || nmemb == 0 { return 0; }
    let fp = unsafe { &mut *f };
    let total = size * nmemb;

    // Unbuffered or no write buffer: direct write
    if fp.buf_mode == _IONBF || fp.write_buf.is_null() {
        let n = io::miku_write(fp.fd as u64, ptr, total);
        if n < 0 { fp.error = 1; return 0; }
        return n as usize / size;
    }

    // buffered write
    let mut written = 0usize;
    while written < total {
        let space = fp.write_cap - fp.write_pos;
        if space == 0 {
            // Buffer full and flush made no room: abort to avoid an infinite
            // loop
            fp.error = 1;
            break;
        }
        let chunk = if (total - written) < space { total - written } else { space };
        unsafe { mem::miku_memcpy(fp.write_buf.add(fp.write_pos), ptr.add(written), chunk); }
        fp.write_pos += chunk;
        written += chunk;

        // check line-buffered flush
        if fp.buf_mode == _IOLBF {
            for i in (fp.write_pos - chunk)..fp.write_pos {
                if unsafe { *fp.write_buf.add(i) } == b'\n' {
                    fflush(f);
                    break;
                }
            }
        }

        if fp.write_pos >= fp.write_cap {
            fflush(f);
            if fp.write_pos >= fp.write_cap {
                // Flush failed to drain the buffer: stop to avoid an infinite
                // loop
                fp.error = 1;
                break;
            }
        }
    }
    written / size
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fseek(f: *mut FILE, offset: i64, whence: i32) -> i32 {
    if f.is_null() { return -1; }
    fflush(f);
    let fp = unsafe { &mut *f };
    // discard read buffer
    fp.read_pos = 0;
    fp.read_filled = 0;
    fp.ungetc_buf = -1;
    fp.eof = 0;
    let r = file::miku_lseek(fp.fd, offset, whence as u64);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ftell(f: *mut FILE) -> i64 {
    if f.is_null() { return -1; }
    let fp = unsafe { &mut *f };
    let pos = file::miku_lseek(fp.fd, 0, file::SEEK_CUR);
    if pos < 0 { return -1; }
    // adjust for buffered but unread data (read) and pending data (write)
    let unread = if fp.read_filled > fp.read_pos { fp.read_filled - fp.read_pos } else { 0 };
    pos - unread as i64 + fp.write_pos as i64
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rewind(f: *mut FILE) {
    fseek(f, 0, SEEK_SET as i32);
    if !f.is_null() {
        unsafe { (*f).error = 0; }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn feof(f: *mut FILE) -> i32 {
    if f.is_null() { return 0; }
    unsafe { (*f).eof }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ferror(f: *mut FILE) -> i32 {
    if f.is_null() { return 0; }
    unsafe { (*f).error }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn clearerr(f: *mut FILE) {
    if f.is_null() { return; }
    let fp = unsafe { &mut *f };
    fp.error = 0;
    fp.eof = 0;
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn setvbuf(f: *mut FILE, _buf: *mut u8, mode: i32, _size: usize) -> i32 {
    if f.is_null() { return -1; }
    let fp = unsafe { &mut *f };
    fp.buf_mode = mode as u32;
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn setbuf(f: *mut FILE, buf: *mut u8) {
    if buf.is_null() {
        setvbuf(f, core::ptr::null_mut(), _IONBF as i32, 0);
    } else {
        setvbuf(f, buf, _IOFBF as i32, BUFSIZ);
    }
}

// printf family: `lib/libmiku/c/printf.rs`, over a real `va_list`

// ctype.h //

#[no_mangle]
#[inline(never)]
pub extern "C" fn isdigit(c: i32) -> i32 { ctype::miku_isdigit(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isalpha(c: i32) -> i32 { ctype::miku_isalpha(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isalnum(c: i32) -> i32 { ctype::miku_isalnum(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isspace(c: i32) -> i32 { ctype::miku_isspace(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isupper(c: i32) -> i32 { ctype::miku_isupper(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn islower(c: i32) -> i32 { ctype::miku_islower(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isprint(c: i32) -> i32 { ctype::miku_isprint(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn ispunct(c: i32) -> i32 { ctype::miku_ispunct(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iscntrl(c: i32) -> i32 { ctype::miku_iscntrl(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isxdigit(c: i32) -> i32 { ctype::miku_isxdigit(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isgraph(c: i32) -> i32 { ctype::miku_isgraph(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isblank(c: i32) -> i32 { ctype::miku_isblank(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isascii(c: i32) -> i32 { ctype::miku_isascii(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn toupper(c: i32) -> i32 { ctype::miku_toupper(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn tolower(c: i32) -> i32 { ctype::miku_tolower(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn toascii(c: i32) -> i32 { ctype::miku_toascii(c) }

// unistd.h //

#[no_mangle]
#[inline(never)]
pub extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> i64 {
    ret(io::miku_read(fd as u64, buf, count))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> i64 {
    ret(io::miku_write(fd as u64, buf, count))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn close(fd: i32) -> i32 {
    let r = file::miku_close(fd as i64);
    if r < 0 { errno::set_errno(-r); -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    ret(file::miku_lseek(fd as i64, offset, whence as u64))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn open(path: *const u8, flags: i32, mode: u32) -> i32 {
    if path.is_null() { return fail(EFAULT); }
    let len = string::miku_strlen(path);
    let r = file::miku_open(path, len, flags as u32, mode);
    if r < 0 { errno::set_errno(-r); -1 } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn creat(path: *const u8, mode: u32) -> i32 {
    open(path, (file::O_WRITE | file::O_CREATE | file::O_TRUNCATE) as i32, mode)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dup(fd: i32) -> i32 {
    let r = file::miku_dup(fd as i64);
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dup2(old: i32, new: i32) -> i32 {
    let r = file::miku_dup2(old as i64, new as i64);
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pipe(fds: *mut i32) -> i32 {
    if fds.is_null() { return fail(EFAULT); }
    let mut fds64: [i64; 2] = [0; 2];
    let r = file::miku_pipe(fds64.as_mut_ptr());
    if r < 0 { return fail(-r as i32); }
    unsafe {
        *fds.add(0) = fds64[0] as i32;
        *fds.add(1) = fds64[1] as i32;
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn unlink(path: *const u8) -> i32 {
    let r = file::miku_unlink(path);
    if r < 0 { fail(-r as i32) } else { 0 }
}

// sys/inotify.h
//
// Linux's calls with Linux's meaning (116-118). `inotify-sys` declares these
// three, so the `inotify` crate and `notify` backend link here. `IN_NONBLOCK`
// and `IN_CLOEXEC` are `O_NONBLOCK` and `O_CLOEXEC`, as on Linux

#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_init1(flags: i32) -> i32 {
    ret(unsafe { crate::sys::sc1(crate::sys::SYS_INOTIFY_INIT1, flags as u32 as u64) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_init() -> i32 {
    inotify_init1(0)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_add_watch(fd: i32, path: *const u8, mask: u32) -> i32 {
    if path.is_null() { return fail(EFAULT); }
    let len = strlen(path);
    ret(unsafe {
        crate::sys::sc4(crate::sys::SYS_INOTIFY_ADD_WATCH, fd as u64, path as u64, len as u64, mask as u64)
    }) as i32
}

/// Linux's `getrandom`: up to `len` bytes of kernel entropy (`getrandom`(78)).
/// `GRND_NONBLOCK` and `GRND_RANDOM` are accepted and change nothing, since the
/// pool does not block once the machine is up. The `getrandom` crate reaches
/// entropy through this
#[no_mangle]
#[inline(never)]
pub extern "C" fn getrandom(buf: *mut u8, len: usize, _flags: u32) -> i64 {
    if len == 0 { return 0; }
    if buf.is_null() { return fail(EFAULT) as i64; }
    // The kernel serves at most 1024 bytes a call and refuses more; Linux
    // returns a short count. So the request is cut to that
    let n = len.min(1024);
    ret(unsafe { crate::sys::sc2(crate::sys::SYS_GETRANDOM, buf as u64, n as u64) })
}

/// `kcmp(pid, pid, KCMP_FILE, fd1, fd2)` for the caller's descriptors. MikuOS
/// has no open file description apart from the descriptor, so the answer
/// compares what they name: 0 for the same vnode, 1 or 2 to order different
/// ones. Another process's descriptors and other `KCMP_*` are `ENOSYS`
#[no_mangle]
#[inline(never)]
pub extern "C" fn kcmp(pid1: i32, pid2: i32, typ: i32, idx1: u64, idx2: u64) -> i32 {
    let me = getpid();
    if typ != 0 || pid1 != me || pid2 != me { return fail(ENOSYS); }
    let mut a: stat = unsafe { core::mem::zeroed() };
    let mut b: stat = unsafe { core::mem::zeroed() };
    if fstat(idx1 as i32, &mut a) < 0 || fstat(idx2 as i32, &mut b) < 0 {
        return -1;
    }
    match (a.st_dev, a.st_ino).cmp(&(b.st_dev, b.st_ino)) {
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Less => 1,
        core::cmp::Ordering::Greater => 2,
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_rm_watch(fd: i32, wd: i32) -> i32 {
    ret(unsafe { crate::sys::sc2(crate::sys::SYS_INOTIFY_RM_WATCH, fd as u64, wd as u32 as u64) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rmdir(path: *const u8) -> i32 {
    let r = file::miku_rmdir(path);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn mkdir(path: *const u8, mode: u32) -> i32 {
    let r = file::miku_mkdir(path, mode);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn link(old: *const u8, new: *const u8) -> i32 {
    let r = file::miku_link(old, new);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn symlink(target: *const u8, linkpath: *const u8) -> i32 {
    let r = file::miku_symlink(target, linkpath);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readlink(path: *const u8, buf: *mut u8, bufsiz: usize) -> i64 {
    ret(file::miku_readlink(path, buf, bufsiz))
}

/// `realpath(3)`: the kernel's answer (`miku_realpath`, 119). A null `resolved`
/// gets a buffer of the answer's size, freed by the caller; otherwise
/// `resolved` holds PATH_MAX bytes, per POSIX
#[no_mangle]
#[inline(never)]
pub extern "C" fn realpath(path: *const u8, resolved: *mut u8) -> *mut u8 {
    const PATH_MAX: usize = 4096;
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

#[no_mangle]
#[inline(never)]
pub extern "C" fn rename(old: *const u8, new: *const u8) -> i32 {
    let r = file::miku_rename(old, new);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    proc::miku_getcwd(buf, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chdir(path: *const u8) -> i32 {
    let r = file::miku_chdir(path);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpid() -> i32 {
    proc::miku_getpid() as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn access(path: *const u8, mode: i32) -> i32 {
    // Honors `mode` and sets errno
    faccessat(AT_FDCWD, path, mode, 0)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sleep(seconds: u32) -> u32 {
    time::miku_sleep_ms(seconds as u64 * 1000);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn usleep(usec: u32) -> i32 {
    time::miku_sleep_ns(usec as u64 * 1000);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ftruncate(fd: i32, length: i64) -> i32 {
    let r = file::miku_ftruncate(fd as i64, length as u64);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn truncate(path: *const u8, length: i64) -> i32 {
    let fd = file::miku_open_rw(path);
    if fd < 0 { return fail(-fd as i32); }
    let r = file::miku_ftruncate(fd, length as u64);
    file::miku_close(fd);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pread(fd: i32, buf: *mut u8, count: usize, offset: i64) -> i64 {
    ret(file::miku_pread(fd as i64, buf, count, offset))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pwrite(fd: i32, buf: *const u8, count: usize, offset: i64) -> i64 {
    ret(file::miku_pwrite(fd as i64, buf, count, offset))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sched_yield() -> i32 {
    time::miku_yield();
    0
}

// sys/stat.h //

/// Linux's `struct stat`, 144 bytes, as C and the `libc` crate read.
///
/// MikuOS's own is 64 bytes (`file::MikuStat`, what `stat`(18) writes), which
/// `miku_stat` and `stat_path` use. The C names answer in this one, via
/// `to_c_stat`: crates such as `rustix`, `tempfile`, and `cap-primitives` read
/// `st_dev`, `st_rdev`, and `st_*_nsec` by name
#[repr(C)]
pub struct stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    __pad0: i32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    __unused: [i64; 3],
}

const _: () = assert!(core::mem::size_of::<stat>() == 144);

/// MikuOS's stat to C's.
///
/// - `st_mode` gets the file type from `kind` (the VFS's `VNodeKind`); the
///   kernel's mode is
///   only the permission bits
/// - `st_dev` is the filesystem (the kernel's `fs_type`); with `st_ino`, unique
///   across every
///   mounted filesystem, it names one file
/// - `st_rdev` is the device a node is, `/dev/*`, in Linux's encoding
/// - `st_blksize` is 4096, the page and VFS block; zero would be a divisor in C
///   code sizing
///   its writes by it (SQLite's `fcntlSizeHint`)
/// - times are whole seconds; the nanoseconds are zero
fn to_c_stat(k: &file::MikuStat, out: *mut stat) {
    let ifmt: u32 = match k.kind {
        0 => 0o100000, // regular
        1 => 0o040000, // directory
        2 => 0o120000, // symlink
        3 => 0o020000, // character device
        4 => 0o060000, // block device
        5 | 7 => 0o010000, // pipe, FIFO
        6 => 0o140000, // socket
        _ => 0,
    };
    let rdev = ((k.dev_major as u64 & 0xfff) << 8) | (k.dev_minor as u64 & 0xff);
    unsafe {
        out.write(stat {
            st_dev: k.fs_type as u64,
            st_ino: k.inode_id,
            st_nlink: k.nlinks as u64,
            st_mode: (k.mode & 0o7777) | ifmt,
            st_uid: k.uid as u32,
            st_gid: k.gid as u32,
            __pad0: 0,
            st_rdev: rdev,
            st_size: k.size as i64,
            st_blksize: 4096,
            st_blocks: k.blocks as i64,
            st_atime: k.atime as i64,
            st_atime_nsec: 0,
            st_mtime: k.mtime as i64,
            st_mtime_nsec: 0,
            st_ctime: k.ctime as i64,
            st_ctime_nsec: 0,
            __unused: [0; 3],
        });
    }
}

/// One of the kernel's stat calls, answered in C's layout
pub(crate) fn c_stat_with(st: *mut stat, call: impl FnOnce(*mut file::MikuStat) -> i64) -> i32 {
    if st.is_null() { return fail(EFAULT); }
    let mut k: file::MikuStat = unsafe { core::mem::zeroed() };
    let r = call(&mut k);
    if r < 0 { return fail(-r as i32); }
    to_c_stat(&k, st);
    0
}

/// MikuOS's stat by path in `MikuStat`, called by the `miku` crate. C's `stat`
/// is below
#[no_mangle]
#[inline(never)]
pub extern "C" fn stat_path(path: *const u8, st: *mut file::MikuStat) -> i32 {
    let r = file::miku_stat(path, st);
    if r < 0 { fail(-r as i32) } else { with_blksize(st) }
}

/// `st_blksize`, in the four bytes the kernel's `stat` leaves zero; see the
/// field in the `libc` module. Every stat here goes through this
fn with_blksize(st: *mut file::MikuStat) -> i32 {
    if !st.is_null() {
        unsafe { (*st)._reserved = 4096u32.to_le_bytes() };
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fstat(fd: i32, st: *mut stat) -> i32 {
    c_stat_with(st, |k| file::miku_fstat(fd as i64, k) as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chmod(path: *const u8, mode: u32) -> i32 {
    let r = file::miku_chmod(path, mode);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chown(path: *const u8, uid: u32, gid: u32) -> i32 {
    let r = file::miku_chown(path, uid, gid);
    if r < 0 { fail(-r as i32) } else { 0 }
}

// sys/mman.h //

pub const PROT_NONE:  u64 = 0;
pub const PROT_READ:  u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC:  u64 = 4;
pub const MAP_FAILED: *mut u8 = !0 as *mut u8;

const MAP_ANONYMOUS: i32 = 0x20;

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

// signal.h //

pub const SIGHUP:  u32 = signal::SIG_HUP;
pub const SIGINT:  u32 = signal::SIG_INT;
pub const SIGQUIT: u32 = signal::SIG_QUIT;
pub const SIGILL:  u32 = signal::SIG_ILL;
pub const SIGTRAP: u32 = signal::SIG_TRAP;
pub const SIGABRT: u32 = signal::SIG_ABRT;
pub const SIGFPE:  u32 = signal::SIG_FPE;
pub const SIGKILL: u32 = signal::SIG_KILL;
pub const SIGSEGV: u32 = signal::SIG_SEGV;
pub const SIGPIPE: u32 = signal::SIG_PIPE;
pub const SIGALRM: u32 = signal::SIG_ALRM;
pub const SIGTERM: u32 = signal::SIG_TERM;
pub const SIGCHLD: u32 = signal::SIG_CHLD;
pub const SIGCONT: u32 = signal::SIG_CONT;
pub const SIGSTOP: u32 = signal::SIG_STOP;
pub const SIGUSR1: u32 = signal::SIG_USR1;
pub const SIGUSR2: u32 = signal::SIG_USR2;

pub type sighandler_t = extern "C" fn(u32);

pub const SIG_DFL: Option<sighandler_t> = None;

#[no_mangle]
#[inline(never)]
pub extern "C" fn signal_register(
    sig: i32,
    handler: Option<sighandler_t>,
) -> Option<sighandler_t> {
    signal::miku_signal(sig as u32, handler)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn raise(sig: i32) -> i32 {
    signal::miku_signal_raise(sig as u32)
}

/// `struct sigaction` as `libc` declares it for MikuOS, Linux's shape: handler,
/// a 1024-bit mask, flags, an unused restorer
#[repr(C)]
pub struct c_sigaction {
    pub sa_sigaction: usize,
    pub sa_mask:      [u64; 16],
    pub sa_flags:     i32,
    pub sa_restorer:  usize,
}

// Linux's `sa_flags` bits, as `libc` passes them
const C_SA_NOCLDSTOP: i32 = 0x0000_0001;
const C_SA_SIGINFO:   i32 = 0x0000_0004;
const C_SA_ONSTACK:   i32 = 0x0800_0000;
const C_SA_RESTART:   i32 = 0x1000_0000;
const C_SA_NODEFER:   i32 = 0x4000_0000;
const C_SA_RESETHAND: i32 = 0x8000_0000_u32 as i32;

const C_SIG_DFL: usize = 0;
const C_SIG_IGN: usize = 1;

/// POSIX `sigaction`, translated onto the handler table in `sys_miku.so` and
/// the kernel's disposition (`sigdisp`(100)).
///
/// What does not translate:
/// * `SA_RESTART` is accepted and not honored. The only call a signal
///   interrupts is a blocked
///   `wait4`, answering `EINTR`; every caller of it in `std` retries.
/// * `SA_ONSTACK` runs the handler on the thread's `sigaltstack`, if it has one
///   and is not
///   already on it. The kernel places the frame, so this travels with the disposition.
/// * `SA_NOCLDSTOP` is accepted; `SIGCHLD` is not sent for a stop.
/// * The mask covers signals 1-31, all MikuOS numbers.
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigaction(sig: i32, act: *const c_sigaction, oldact: *mut c_sigaction) -> i32 {
    if sig <= 0 || sig >= 32 { return fail(EINVAL); }
    let s = sig as u32;
    if !act.is_null() && (s == signal::SIG_KILL || s == signal::SIG_STOP) {
        return fail(EINVAL);
    }

    // Current state before any change: the disposition and the handler entry.
    // `sigdisp` with the query value changes nothing
    let disp = proc::miku_sigdisp(s as u64, crate::sys::SIG_DISP_QUERY);
    if disp < 0 { return fail(-disp as i32); }
    let mut cur: signal::MikuSigaction = unsafe { core::mem::zeroed() };
    signal::miku_sigaction(s, core::ptr::null(), &mut cur);

    if !act.is_null() {
        let a = unsafe { &*act };
        let known = C_SA_NOCLDSTOP | C_SA_SIGINFO | C_SA_ONSTACK | C_SA_RESTART
            | C_SA_NODEFER | C_SA_RESETHAND;
        if a.sa_flags & !known != 0 { return fail(EINVAL); }
        let r = match a.sa_sigaction {
            C_SIG_IGN => signal::miku_signal_ignore(s),
            C_SIG_DFL => {
                let none: signal::MikuSigaction = unsafe { core::mem::zeroed() };
                signal::miku_sigaction(s, &none, core::ptr::null_mut())
            }
            h => {
                let mut flags = 0u32;
                if a.sa_flags & C_SA_SIGINFO   != 0 { flags |= signal::SA_SIGINFO; }
                if a.sa_flags & C_SA_NODEFER   != 0 { flags |= signal::SA_NODEFER; }
                if a.sa_flags & C_SA_RESETHAND != 0 { flags |= signal::SA_RESETHAND; }
                if a.sa_flags & C_SA_ONSTACK   != 0 { flags |= signal::SA_ONSTACK; }
                // C's bit n-1 is signal n; the table's bit n is signal n
                let mask = ((a.sa_mask[0] << 1) & 0xffff_fffe) as u32;
                let mut m: signal::MikuSigaction = unsafe { core::mem::zeroed() };
                m.handler = Some(unsafe { core::mem::transmute(h) });
                m.flags = flags;
                m.mask = mask;
                signal::miku_sigaction(s, &m, core::ptr::null_mut())
            }
        };
        if r < 0 { return fail(EINVAL); }
    }

    if !oldact.is_null() {
        let mut o = c_sigaction { sa_sigaction: C_SIG_DFL, sa_mask: [0; 16], sa_flags: 0, sa_restorer: 0 };
        if disp as u64 & !crate::sys::SIG_DISP_ONSTACK == crate::sys::SIG_DISP_IGNORE {
            o.sa_sigaction = C_SIG_IGN;
        } else if let Some(h) = cur.handler {
            o.sa_sigaction = h as usize;
            if cur.flags & signal::SA_SIGINFO   != 0 { o.sa_flags |= C_SA_SIGINFO; }
            if cur.flags & signal::SA_NODEFER   != 0 { o.sa_flags |= C_SA_NODEFER; }
            if cur.flags & signal::SA_RESETHAND != 0 { o.sa_flags |= C_SA_RESETHAND; }
            if cur.flags & signal::SA_ONSTACK   != 0 { o.sa_flags |= C_SA_ONSTACK; }
            o.sa_mask[0] = (cur.mask >> 1) as u64;
        }
        unsafe { oldact.write(o) };
    }
    0
}

/// ISO C's `signal` with glibc's BSD meaning: the handler stays installed after
/// running, and the signal is held off while it runs. Returns the previous
/// handler or `SIG_ERR` (-1)
#[no_mangle]
#[inline(never)]
pub extern "C" fn signal(sig: i32, handler: usize) -> usize {
    let act = c_sigaction { sa_sigaction: handler, sa_mask: [0; 16], sa_flags: C_SA_RESTART, sa_restorer: 0 };
    let mut old = c_sigaction { sa_sigaction: 0, sa_mask: [0; 16], sa_flags: 0, sa_restorer: 0 };
    if sigaction(sig, &act, &mut old) < 0 {
        return usize::MAX;
    }
    old.sa_sigaction
}

/// `stack_t`, Linux's shape
#[repr(C)]
pub struct stack_t {
    pub ss_sp:    *mut u8,
    pub ss_flags: i32,
    pub ss_size:  usize,
}

/// POSIX `sigaltstack`: this thread's alternate signal stack for `SA_ONSTACK`
/// handlers. Per thread, kept by the kernel (`sigaltstack`(115)), which places
/// the frame. `SS_DISABLE` removes it; changing it while on it is `EPERM`; less
/// than `MINSIGSTKSZ` (2048) is `ENOMEM`
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigaltstack(ss: *const stack_t, old: *mut stack_t) -> i32 {
    let r = unsafe { crate::sys::sc2(crate::sys::SYS_SIGALTSTACK, ss as u64, old as u64) };
    ret(r) as i32
}

// arpa/inet.h
//
// Address text, IPv4 only: `AF_INET6` is `EAFNOSUPPORT`. Four decimal parts,
// each at most 255 and unsigned, the same parse as `getaddrinfo`

const AF_INET_C: i32 = 2;

#[no_mangle]
#[inline(never)]
pub extern "C" fn inet_pton(af: i32, src: *const u8, dst: *mut u8) -> i32 {
    if af != AF_INET_C { return fail(97); } // EAFNOSUPPORT
    if src.is_null() || dst.is_null() { return fail(EFAULT); }
    let s = unsafe { core::slice::from_raw_parts(src, strlen(src)) };
    let mut out = [0u8; 4];
    let mut parts = s.split(|&b| b == b'.');
    for o in out.iter_mut() {
        let Some(p) = parts.next() else { return 0 };
        if p.is_empty() || p.len() > 3 || !p.iter().all(u8::is_ascii_digit) { return 0; }
        let v = p.iter().fold(0u32, |a, &d| a * 10 + (d - b'0') as u32);
        if v > 255 { return 0; }
        *o = v as u8;
    }
    if parts.next().is_some() { return 0; }
    unsafe { core::ptr::copy_nonoverlapping(out.as_ptr(), dst, 4) };
    1
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn inet_ntop(af: i32, src: *const u8, dst: *mut u8, size: u32) -> *const u8 {
    if af != AF_INET_C { fail(97); return core::ptr::null(); }
    if src.is_null() || dst.is_null() { fail(EFAULT); return core::ptr::null(); }
    let a = unsafe { core::slice::from_raw_parts(src, 4) };
    let mut text = [0u8; 16];
    let mut n = 0;
    for (i, &b) in a.iter().enumerate() {
        if i > 0 { text[n] = b'.'; n += 1; }
        let mut digits = [0u8; 3];
        let mut k = 0;
        let mut v = b;
        loop {
            digits[k] = b'0' + v % 10;
            k += 1;
            v /= 10;
            if v == 0 { break; }
        }
        while k > 0 { k -= 1; text[n] = digits[k]; n += 1; }
    }
    if n + 1 > size as usize { fail(28); return core::ptr::null(); } // ENOSPC
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), dst, n);
        *dst.add(n) = 0;
    }
    dst
}

// sys/select.h //

/// `select` over `poll`(76), as libcs without their own do. Readable is
/// `POLLIN`, a hangup, or an error; writable is `POLLOUT` or an error; the
/// exception set is never marked, as there is no out-of-band data. The kernel
/// watches 256 descriptors per call
#[no_mangle]
#[inline(never)]
pub extern "C" fn select(
    nfds: i32,
    r: *mut [u64; 16],
    w: *mut [u64; 16],
    e: *mut [u64; 16],
    tv: *const crate::ctime::Timeval,
) -> i32 {
    const POLLIN: i16 = 0x001;
    const POLLOUT: i16 = 0x004;
    const POLLERR: i16 = 0x008;
    const POLLHUP: i16 = 0x010;
    const POLLNVAL: i16 = 0x020;
    if !(0..=1024).contains(&nfds) { return fail(EINVAL); }
    let bit = |set: *mut [u64; 16], fd: i32| -> bool {
        !set.is_null() && unsafe { (*set)[fd as usize / 64] } >> (fd % 64) & 1 != 0
    };
    let mut fds = [pollfd { fd: -1, events: 0, revents: 0 }; 256];
    let mut n = 0;
    for fd in 0..nfds {
        let ev = (if bit(r, fd) { POLLIN } else { 0 }) | (if bit(w, fd) { POLLOUT } else { 0 });
        if ev == 0 && !bit(e, fd) { continue; }
        if n == fds.len() { return fail(EINVAL); }
        fds[n] = pollfd { fd, events: ev, revents: 0 };
        n += 1;
    }
    let timeout = if tv.is_null() {
        -1
    } else {
        let t = unsafe { &*tv };
        if t.tv_sec < 0 || t.tv_usec < 0 { return fail(EINVAL); }
        (t.tv_sec.saturating_mul(1000) + (t.tv_usec + 999) / 1000).min(i32::MAX as i64) as i32
    };
    let got = poll(fds.as_mut_ptr(), n as u64, timeout);
    if got < 0 { return -1; }
    for set in [r, w, e] {
        if !set.is_null() { unsafe { *set = [0; 16] }; }
    }
    let mut marked = 0;
    for p in &fds[..n] {
        if p.revents & POLLNVAL != 0 { return fail(EBADF); }
        let fd = p.fd as usize;
        if p.events & POLLIN != 0 && p.revents & (POLLIN | POLLHUP | POLLERR) != 0 {
            unsafe { (*r)[fd / 64] |= 1 << (fd % 64) };
            marked += 1;
        }
        if p.events & POLLOUT != 0 && p.revents & (POLLOUT | POLLERR) != 0 {
            unsafe { (*w)[fd / 64] |= 1 << (fd % 64) };
            marked += 1;
        }
    }
    marked
}

// signal.h: masks
//
// `sigset_t` is Linux's 1024 bits (the `libc` crate's `[u64; 16]`), bit `n-1`
// for signal `n`. The mask is `sys_miku.so`'s and process-wide: there is no
// per-thread mask (dev/signal-architecture.md, "What is not here"), so
// `pthread_sigmask` is this under another name. Unblocking a signal that
// arrived while blocked delivers it before the call returns, per POSIX

/// C bits (signal n at bit n-1) to the table's (signal n at bit n), 1..31
fn c_set_to_mask(set: *const [u64; 16]) -> u32 {
    let w = unsafe { (*set)[0] };
    let mut m = ((w << 1) & 0xffff_fffe) as u32;
    // SIGKILL and SIGSTOP cannot be blocked; asking is not an error
    m &= !(1 << signal::SIG_KILL) & !(1 << signal::SIG_STOP);
    m
}

fn mask_to_c_set(m: u32, set: *mut [u64; 16]) {
    unsafe {
        *set = [0; 16];
        (*set)[0] = (m >> 1) as u64;
    }
}

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

/// The mask change, with errno-style failure; shared with `pthread_sigmask`
pub(crate) fn change_mask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> Result<(), i32> {
    if !set.is_null() && !(SIG_BLOCK..=SIG_SETMASK).contains(&how) {
        return Err(EINVAL);
    }
    let cur = signal::miku_signal_get_mask();
    if !old.is_null() {
        mask_to_c_set(cur, old);
    }
    if set.is_null() {
        return Ok(());
    }
    let req = c_set_to_mask(set);
    let (block, unblock) = match how {
        SIG_BLOCK => (req, 0),
        SIG_UNBLOCK => (0, req & cur),
        _ => (req & !cur, cur & !req),
    };
    if block != 0 {
        signal::miku_signal_set_mask(signal::miku_signal_get_mask() | block);
    }
    for sig in 1..32 {
        if unblock & (1 << sig) != 0 {
            signal::miku_signal_unblock(sig);
        }
    }
    Ok(())
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigprocmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32 {
    match change_mask(how, set, old) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigemptyset(set: *mut [u64; 16]) -> i32 {
    if set.is_null() { return fail(EINVAL); }
    unsafe { *set = [0; 16] };
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigfillset(set: *mut [u64; 16]) -> i32 {
    if set.is_null() { return fail(EINVAL); }
    unsafe { *set = [u64::MAX; 16] };
    0
}

fn sig_bit(sig: i32) -> Option<(usize, u64)> {
    if !(1..=1024).contains(&sig) { return None; }
    let b = (sig - 1) as usize;
    Some((b / 64, 1u64 << (b % 64)))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigaddset(set: *mut [u64; 16], sig: i32) -> i32 {
    let Some((w, b)) = sig_bit(sig) else { return fail(EINVAL) };
    if set.is_null() { return fail(EINVAL); }
    unsafe { (*set)[w] |= b };
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigdelset(set: *mut [u64; 16], sig: i32) -> i32 {
    let Some((w, b)) = sig_bit(sig) else { return fail(EINVAL) };
    if set.is_null() { return fail(EINVAL); }
    unsafe { (*set)[w] &= !b };
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigismember(set: *const [u64; 16], sig: i32) -> i32 {
    let Some((w, b)) = sig_bit(sig) else { return fail(EINVAL) };
    if set.is_null() { return fail(EINVAL); }
    (unsafe { (*set)[w] } & b != 0) as i32
}

// sys/mman.h: advice

/// Advice, which POSIX lets a system ignore, except `MADV_DONTNEED`, which on
/// Linux drops pages and reads back zeros. That is impossible here, so it is
/// refused
#[no_mangle]
#[inline(never)]
pub extern "C" fn madvise(_addr: *mut u8, _len: usize, advice: i32) -> i32 {
    const MADV_DONTNEED: i32 = 4;
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

// time.h //

#[repr(C)]
pub struct timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn nanosleep(req: *const timespec, _rem: *mut timespec) -> i32 {
    if req.is_null() { return fail(EFAULT); }
    let ts = unsafe { &*req };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 { return fail(EINVAL); }
    time::miku_sleep_ns((ts.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(ts.tv_nsec as u64));
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn clock_gettime(clockid: i32, tp: *mut timespec) -> i32 {
    if tp.is_null() { return fail(EFAULT); }
    // Honors `clockid`, as syscall 70 distinguishes the clocks
    let mut ts = time::MikuTimespec::default();
    let rc = time::miku_clock_gettime(clockid as u64, &mut ts);
    if rc < 0 { return fail(-rc as i32); }
    let tp = unsafe { &mut *tp };
    tp.tv_sec = ts.sec;
    tp.tv_nsec = ts.nsec;
    0
}

// dirent.h //

pub type dirent = file::MikuDirent;

// opaque DIR handle
/// An open directory: a descriptor and an offset, refilling in batches of 32
#[repr(C)]
pub struct DIR {
    fd: i64,
    entries: [file::MikuDirent; 32],
    count: usize,
    pos: usize,
    offset: usize,
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn opendir(path: *const u8) -> *mut DIR {
    if path.is_null() { return core::ptr::null_mut(); }
    let plen = string::miku_strlen(path);
    // O_READ | O_DIRECTORY. `getdents` continues from this descriptor, which
    // also keeps the walk on its starting directory
    let fd = file::miku_open(path, plen, 0x0001 | 0x0040, 0);
    if fd < 0 { return core::ptr::null_mut(); }

    let d = heap::miku_calloc(1, core::mem::size_of::<DIR>()) as *mut DIR;
    if d.is_null() {
        file::miku_close(fd);
        return core::ptr::null_mut();
    }
    let dp = unsafe { &mut *d };
    dp.fd = fd;
    dp.count = 0;
    dp.pos = 0;
    dp.offset = 0;
    d
}

/// A `DIR` over a descriptor the caller opened and now gives up: `closedir`
/// closes it. POSIX's `ENOTDIR` is checked here
#[no_mangle]
#[inline(never)]
pub extern "C" fn fdopendir(fd: i32) -> *mut DIR {
    let mut st: stat = unsafe { core::mem::zeroed() };
    if fstat(fd, &mut st) < 0 { return core::ptr::null_mut(); }
    if st.st_mode & 0o170000 != 0o040000 {
        fail(ENOTDIR);
        return core::ptr::null_mut();
    }
    let d = heap::miku_calloc(1, core::mem::size_of::<DIR>()) as *mut DIR;
    if d.is_null() {
        fail(ENOMEM);
        return core::ptr::null_mut();
    }
    unsafe { (*d).fd = fd as i64 };
    d
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readdir(d: *mut DIR) -> *const file::MikuDirent {
    if d.is_null() { return core::ptr::null(); }
    let dp = unsafe { &mut *d };
    if dp.pos >= dp.count {
        // Refills from where the last batch stopped
        let n = file::miku_getdents(dp.fd, dp.entries.as_mut_ptr(), dp.entries.len(), dp.offset);
        if n <= 0 { return core::ptr::null(); }
        dp.count = n as usize;
        dp.offset += n as usize;
        dp.pos = 0;
    }
    let entry = &dp.entries[dp.pos] as *const file::MikuDirent;
    dp.pos += 1;
    entry
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn closedir(d: *mut DIR) -> i32 {
    if d.is_null() { return fail(EBADF); }
    // A tree walker that kept these would exhaust its 512
    let fd = unsafe { (*d).fd };
    if fd >= 0 { file::miku_close(fd); }
    heap::miku_free(d as *mut u8);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rewinddir(d: *mut DIR) {
    if d.is_null() { return; }
    // Back to the start of the directory, not the batch in hand
    unsafe {
        (*d).pos = 0;
        (*d).count = 0;
        (*d).offset = 0;
    }
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

// The POSIX names Rust crates expect
//
// Written for crates saying `cfg(unix)`. `poll` and `fcntl` are essential:
// `polling`, and so `async-io` and `smol`, is a reactor over `epoll`, `kqueue`,
// or `poll(2)`, and here it is the third.
//
// These declare what MikuOS does. Where it differs (a socket is not a
// descriptor in the VFS table, no `shutdown`, no IPv6), the function is absent;
// see `dev/std-architecture.md`.

/// `strerror_r`, the reentrant one. XSI semantics: 0 on success, `ERANGE` when
/// the buffer is too small, always NUL-terminated
#[no_mangle]
#[inline(never)]
pub extern "C" fn strerror_r(errnum: i32, buf: *mut u8, buflen: usize) -> i32 {
    if buf.is_null() || buflen == 0 { return EINVAL; }
    let src = strerror(errnum as i64);
    let len = string::miku_strlen(src);
    if len + 1 > buflen {
        unsafe {
            mem::miku_memcpy(buf, src, buflen - 1);
            *buf.add(buflen - 1) = 0;
        }
        return ERANGE;
    }
    unsafe {
        mem::miku_memcpy(buf, src, len);
        *buf.add(len) = 0;
    }
    0
}


// poll.h //

pub const POLLIN:   i16 = 0x001;
pub const POLLPRI:  i16 = 0x002;
pub const POLLOUT:  i16 = 0x004;
pub const POLLERR:  i16 = 0x008;
pub const POLLHUP:  i16 = 0x010;
pub const POLLNVAL: i16 = 0x020;

/// `struct pollfd`: the kernel's `MikuPollFd`, eight bytes, under the name
/// crates read
pub type pollfd = io::MikuPollFd;

#[no_mangle]
#[inline(never)]
pub extern "C" fn poll(fds: *mut pollfd, nfds: u64, timeout: i32) -> i32 {
    // The kernel caps a call at `MAX_POLL_FDS` (256) and answers `-EINVAL` past
    // it, which is passed through
    let r = io::miku_poll(fds, nfds, timeout as i64);
    if r < 0 { set_errno(-r as i32); return -1; }
    r as i32
}

// fcntl.h //

pub const F_DUPFD:  i32 = 0;
pub const F_GETFD:  i32 = 1;
pub const F_SETFD:  i32 = 2;
pub const F_GETFL:  i32 = 3;
pub const F_SETFL:  i32 = 4;
pub const FD_CLOEXEC: i32 = 1;
pub const O_NONBLOCK: i32 = 0x0400;
pub const O_CLOEXEC:  i32 = 0x0800;

/// Variadic in C and three-argument here, as every caller passes. A socket
/// answers too, though above `SOCK_FD_BASE` in its own table: the kernel routes
/// it to the socket layer, which keeps `O_NONBLOCK` and `FD_CLOEXEC` on the
/// slot and makes `F_DUPFD` an alias
#[no_mangle]
#[inline(never)]
pub extern "C" fn fcntl(fd: i32, cmd: i32, arg: i64) -> i32 {
    // `F_GETPATH` is this library's: `realpath` (119) on the descriptor itself
    const F_GETPATH: i32 = 50;
    const PATH_MAX: usize = 4096;
    if cmd == F_GETPATH {
        let r = file::miku_fd_path(fd, arg as *mut u8, PATH_MAX);
        return if r < 0 { fail(-r as i32) } else { 0 };
    }
    let r = file::miku_fcntl(fd as i64, cmd as u64, arg);
    if r < 0 { set_errno(-r as i32); return -1; }
    r as i32
}

/// BSD's whole-file lock as a POSIX record lock over the whole file, the
/// kernel's only lock (`fcntl`(113) `F_SETLK`, `fs::vfs::lock`). Differences:
/// it belongs to the process, so any `close` of the file drops it; and a shared
/// lock needs a read descriptor, an exclusive one a write descriptor (`EBADF`
/// otherwise)
#[no_mangle]
#[inline(never)]
pub extern "C" fn flock(fd: i32, op: i32) -> i32 {
    const LOCK_SH: i32 = 1;
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    const LOCK_UN: i32 = 8;
    const F_SETLK: i32 = 6;
    const F_SETLKW: i32 = 7;
    let l_type: i16 = match op & !LOCK_NB {
        LOCK_SH => 0,
        LOCK_EX => 1,
        LOCK_UN => 2,
        _ => return fail(EINVAL),
    };
    // `struct flock`, the 32 bytes `record_lock` reads: type, whence
    // (SEEK_SET), start 0, length 0 to the end
    let mut fl = [0u8; 32];
    fl[0..2].copy_from_slice(&l_type.to_le_bytes());
    let cmd = if op & LOCK_NB != 0 || l_type == 2 { F_SETLK } else { F_SETLKW };
    let r = fcntl(fd, cmd, fl.as_mut_ptr() as i64);
    // A conflict is `EWOULDBLOCK` for `flock`, where `fcntl` may say `EACCES`
    if r < 0 && unsafe { *__errno_location() } == EACCES { return fail(EAGAIN); }
    r
}

// unistd.h / sys/wait.h: processes

#[no_mangle]
#[inline(never)]
pub extern "C" fn fork() -> i32 {
    use crate::pthread::{run_atfork, ForkPhase};
    run_atfork(ForkPhase::Prepare);
    let r = proc::miku_fork();
    if r < 0 {
        run_atfork(ForkPhase::Parent);
        set_errno(-r as i32);
        return -1;
    }
    run_atfork(if r == 0 { ForkPhase::Child } else { ForkPhase::Parent });
    r as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn execve(
    path: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    // C passes these NULL-terminated and MikuOS wants a count
    let argc = count_vec(argv);
    let envc = count_vec(envp);
    let r = proc::miku_execve(path, argv, argc, envp, envc);
    // `execve` returning at all is `execve` failing
    set_errno(if r < 0 { -r as i32 } else { ENOENT });
    -1
}

/// `execve` after a `PATH` search, as `execvp` does, with the given
/// environment. A name with a slash is not searched. Uses the calling process's
/// `PATH` (`/bin` without one), and reports the last error other than "not
/// there", as a shell does
#[no_mangle]
#[inline(never)]
pub extern "C" fn execvpe(
    file: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    if file.is_null() {
        return fail(ENOENT);
    }
    let name = unsafe { core::ffi::CStr::from_ptr(file as *const core::ffi::c_char) }.to_bytes();
    if name.is_empty() {
        return fail(ENOENT);
    }
    if name.contains(&b'/') {
        return execve(file, argv, envp);
    }
    let path = getenv(b"PATH\0".as_ptr());
    let path: &[u8] = if path.is_null() {
        b"/bin"
    } else {
        unsafe { core::ffi::CStr::from_ptr(path as *const core::ffi::c_char) }.to_bytes()
    };
    let mut err = ENOENT;
    let mut full = [0u8; 512];
    for dir in path.split(|&c| c == b':') {
        let dir: &[u8] = if dir.is_empty() { b"." } else { dir };
        let len = dir.len() + 1 + name.len();
        if len + 1 > full.len() {
            continue;
        }
        full[..dir.len()].copy_from_slice(dir);
        full[dir.len()] = b'/';
        full[dir.len() + 1..len].copy_from_slice(name);
        full[len] = 0;
        execve(full.as_ptr(), argv, envp);
        let e = unsafe { *errno::miku_errno_location() };
        if e != ENOENT && e != ENOTDIR {
            err = e;
        }
    }
    fail(err)
}

/// `ENOSYS`: the kernel executes a path, never an open descriptor
#[no_mangle]
#[inline(never)]
pub extern "C" fn fexecve(_fd: i32, _argv: *const *const u8, _envp: *const *const u8) -> i32 {
    fail(ENOSYS)
}

/// Length of a NULL-terminated pointer vector, capped against a missing
/// terminator
fn count_vec(v: *const *const u8) -> usize {
    if v.is_null() { return 0; }
    let mut n = 0usize;
    while n < 4096 {
        if unsafe { *v.add(n) }.is_null() { break; }
        n += 1;
    }
    n
}

pub const WNOHANG:   i32 = 1;
pub const WUNTRACED: i32 = 2;

#[no_mangle]
#[inline(never)]
pub extern "C" fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32 {
    let mut raw: i64 = 0;
    let r = proc::miku_wait4(pid as u64, &mut raw, options as u64);
    if r < 0 { set_errno(-r as i32); return -1; }
    if !status.is_null() {
        // The kernel writes a POSIX status word into an `i64`; C reads an `int`
        unsafe { *status = raw as i32 };
    }
    r as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn kill(pid: i32, sig: i32) -> i32 {
    let r = proc::miku_kill(pid as u64, sig as u64);
    if r < 0 { set_errno(-r as i32); return -1; }
    0
}

/// Every member of the group, each permission-checked on its own
#[no_mangle]
#[inline(never)]
pub extern "C" fn killpg(pgrp: i32, sig: i32) -> i32 {
    if pgrp <= 1 { return fail(EINVAL); }
    kill(-pgrp, sig)
}

// Jobs: `setpgid`(101), `getpgid`(102); docs/MikuOS_ABI.md 3.2b

#[no_mangle]
#[inline(never)]
pub extern "C" fn setpgid(pid: i32, pgid: i32) -> i32 {
    if pid < 0 || pgid < 0 { return fail(EINVAL); }
    let r = unsafe { crate::sys::sc2(crate::sys::SYS_SETPGID, pid as u64, pgid as u64) };
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpgid(pid: i32) -> i32 {
    if pid < 0 { return fail(ESRCH); }
    let r = unsafe { crate::sys::sc1(crate::sys::SYS_GETPGID, pid as u64) };
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpgrp() -> i32 {
    getpgid(0)
}

/// MikuOS has jobs and no sessions. `setsid` here is leading a process group,
/// `setpgid(0, 0)`, so `killpg` on its pid reaches it and its children, and the
/// group is the answer. It cannot drop a controlling terminal. A group leader
/// is refused, as in POSIX
#[no_mangle]
#[inline(never)]
pub extern "C" fn setsid() -> i32 {
    let me = getpid();
    if getpgid(0) == me { return fail(EPERM); }
    if setpgid(0, 0) < 0 { return -1; }
    me
}

/// Until a signal handler has run, then `EINTR`. The kernel runs handlers on
/// the way out of a syscall, so this sleeps in short steps and reads
/// `sys_miku`'s count; a signal is noticed within one step
#[no_mangle]
#[inline(never)]
pub extern "C" fn pause() -> i32 {
    let seen = signal::miku_signal_handled();
    while signal::miku_signal_handled() == seen {
        time::miku_sleep_ms(10);
    }
    fail(EINTR)
}

/// `mask` in place while waiting for a handler, then the old mask back and
/// `EINTR`. The count is read before the mask changes, since a signal the new
/// mask unblocks is delivered on the way out of that call
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigsuspend(mask: *const [u64; 16]) -> i32 {
    let seen = signal::miku_signal_handled();
    let mut old = [0u64; 16];
    if sigprocmask(SIG_SETMASK, mask, &mut old) < 0 {
        return -1;
    }
    while signal::miku_signal_handled() == seen {
        time::miku_sleep_ms(10);
    }
    sigprocmask(SIG_SETMASK, &old, core::ptr::null_mut());
    fail(EINTR)
}

/// `ENOSYS`, returned as `sigwait` returns errors: nothing removes a pending
/// signal without running its handler
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigwait(_set: *const [u64; 16], _sig: *mut i32) -> i32 {
    ENOSYS
}

/// `ENOSYS`: no call changes a process's root directory
#[no_mangle]
#[inline(never)]
pub extern "C" fn chroot(_path: *const u8) -> i32 {
    fail(ENOSYS)
}

/// The signal's name, as `strsignal`; numbers are Linux's
#[no_mangle]
#[inline(never)]
pub extern "C" fn strsignal(sig: i32) -> *const u8 {
    const NAMES: [&[u8]; 32] = [
        b"Unknown signal 0\0", b"Hangup\0", b"Interrupt\0", b"Quit\0",
        b"Illegal instruction\0", b"Trace/breakpoint trap\0", b"Aborted\0", b"Bus error\0",
        b"Floating point exception\0", b"Killed\0", b"User defined signal 1\0",
        b"Segmentation fault\0", b"User defined signal 2\0", b"Broken pipe\0",
        b"Alarm clock\0", b"Terminated\0", b"Stack fault\0", b"Child exited\0",
        b"Continued\0", b"Stopped (signal)\0", b"Stopped\0", b"Stopped (tty input)\0",
        b"Stopped (tty output)\0", b"Urgent I/O condition\0", b"CPU time limit exceeded\0",
        b"File size limit exceeded\0", b"Virtual timer expired\0",
        b"Profiling timer expired\0", b"Window changed\0", b"I/O possible\0",
        b"Power failure\0", b"Bad system call\0",
    ];
    match NAMES.get(sig as usize) {
        Some(n) if sig > 0 => n.as_ptr(),
        _ => b"Unknown signal\0".as_ptr(),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getppid() -> i32 {
    // No `getppid` syscall: the parent is in the `procinfo`(103) table. Answers
    // that it does not know
    set_errno(ENOSYS);
    -1
}

// sys/stat.h //

#[no_mangle]
#[inline(never)]
pub extern "C" fn lstat(path: *const u8, st: *mut stat) -> i32 {
    c_stat_with(st, |k| file::miku_lstat(path, k) as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fsync(fd: i32) -> i32 {
    let r = file::miku_fsync(fd as i64);
    if r < 0 { set_errno(-r as i32); return -1; }
    0
}

// unistd.h: terminals

/// Whether this descriptor is a terminal: `ioctl(TCGETS)` succeeds on one and
/// fails with `ENOTTY` otherwise
#[no_mangle]
#[inline(never)]
pub extern "C" fn isatty(fd: i32) -> i32 {
    let mut t = [0u8; 32];
    const TCGETS: u64 = 0x5401;
    let r = unsafe { crate::sys::sc3(crate::sys::SYS_IOCTL, fd as u64, TCGETS, t.as_mut_ptr() as u64) };
    if r < 0 { set_errno(ENOTTY); return 0; }
    1
}

pub const ENOTTY: i32 = 25;

/// Sets `errno`, for functions here reporting `-1`. MikuOS's own calls return
/// errors and do not touch this, so only this file sets it
pub(crate) fn set_errno(e: i32) {
    errno::set_errno(e as i64);
}

// What `libc`'s MikuOS module declares and `rustix` links
//
// `lib/rust-libc/libc/src/unix/miku/mod.rs` declares these; a declaration
// without a symbol is a link error in the first crate calling it. Implement
// what MikuOS can do and answer `ENOSYS` for what it cannot. A stub returning
// success is not allowed.

/// `struct iovec`. `libc`'s, from `src/unix/mod.rs`, laid out the same
#[repr(C)]
pub struct iovec {
    pub iov_base: *mut u8,
    pub iov_len:  usize,
}

/// `struct rlimit`, likewise `libc`'s own
#[repr(C)]
pub struct rlimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

/// `struct statfs` in the Linux shape the `libc` module declares
#[repr(C)]
pub struct statfs {
    pub f_type:    i64,
    pub f_bsize:   i64,
    pub f_blocks:  u64,
    pub f_bfree:   u64,
    pub f_bavail:  u64,
    pub f_files:   u64,
    pub f_ffree:   u64,
    pub f_fsid:    [i32; 2],
    pub f_namelen: i64,
    pub f_frsize:  i64,
    pub f_flags:   i64,
    pub f_spare:   [i64; 4],
}

pub(crate) const AT_FDCWD:            i32 = -100;
pub(crate) const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
const AT_EACCESS:          i32 = 0x200;

const R_OK: i32 = 4;
const W_OK: i32 = 2;
const X_OK: i32 = 1;

const FIONBIO: u64 = 0x5421;

const _SC_CLK_TCK:          i32 = 2;
const _SC_OPEN_MAX:         i32 = 4;
const _SC_PAGESIZE:         i32 = 30;
const _SC_GETPW_R_SIZE_MAX: i32 = 70;

const RLIMIT_NOFILE: i32 = 7;
const RLIM_INFINITY: u64 = !0;

/// Per-process descriptor ceiling, `MAX_FDS` in `kernel/fs/vfs/types.rs`. A
/// kernel constant, as `getrlimit` and `setrlimit` below report
const OPEN_MAX: u64 = 512;

/// The scheduler tick, the resolution of both clocks `clock_gettime`(70) reads
const TICK_NS: i64 = 1_000_000_000 / 250;

/// `-1` with `errno` set, the C convention. MikuOS returns `-errno`, and
/// `rustix` treats anything but `-1` as a count, so every function here
/// converts through one of these two
pub(crate) fn fail(e: i32) -> i32 { set_errno(e); -1 }

pub(crate) fn ret(r: i64) -> i64 {
    if r < 0 { set_errno(-r as i32); -1 } else { r }
}

// unistd.h / fcntl.h: descriptors

/// `pipe2`(120): the kernel sets the flags as the ends are made, so a `fork` on
/// another thread cannot catch them without `O_CLOEXEC`. Flag values are the
/// kernel's
#[no_mangle]
#[inline(never)]
pub extern "C" fn pipe2(fds: *mut i32, flags: i32) -> i32 {
    if fds.is_null() { return fail(EINVAL); }
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 { return fail(EINVAL); }
    let mut ends: [i64; 2] = [0; 2];
    let r = file::miku_pipe2(ends.as_mut_ptr(), flags as u32);
    if r < 0 { return fail(-r as i32); }
    unsafe {
        *fds = ends[0] as i32;
        *fds.add(1) = ends[1] as i32;
    }
    0
}

/// `O_CLOEXEC` and `O_NONBLOCK` on an open descriptor, for `pipe2` and `dup3`
fn apply_fd_flags(fd: i64, flags: i32) -> i64 {
    if flags & O_CLOEXEC != 0 {
        let r = file::miku_fcntl(fd, F_SETFD as u64, FD_CLOEXEC as i64);
        if r < 0 { return r; }
    }
    if flags & O_NONBLOCK != 0 {
        let r = file::miku_set_nonblocking(fd, true);
        if r < 0 { return r; }
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dup3(oldfd: i32, newfd: i32, flags: i32) -> i32 {
    // `dup3` differs from `dup2`: the same descriptor twice is an error, and
    // the only flag is `O_CLOEXEC`
    if oldfd == newfd { return fail(EINVAL); }
    if flags & !O_CLOEXEC != 0 { return fail(EINVAL); }
    let r = file::miku_dup2(oldfd as i64, newfd as i64);
    if r < 0 { return fail(-r as i32); }
    let f = apply_fd_flags(r, flags);
    if f < 0 { return fail(-f as i32); }
    r as i32
}

/// `fsync`: there is no metadata-free flush, and `fdatasync` may do more than
/// asked
#[no_mangle]
#[inline(never)]
pub extern "C" fn fdatasync(fd: i32) -> i32 {
    fsync(fd)
}

/// Does nothing: no call flushes every filesystem, and `sync` cannot report
/// failure. `fsync` on a descriptor is real
#[no_mangle]
#[inline(never)]
pub extern "C" fn sync() {}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ioctl(fd: i32, request: u64, arg: *mut u8) -> i32 {
    // `FIONBIO` is `fcntl(F_SETFL, O_NONBLOCK)`, answered here since the
    // kernel's ioctl only knows terminals
    if request == FIONBIO {
        if arg.is_null() { return fail(EFAULT); }
        let on = unsafe { (arg as *const i32).read() } != 0;
        let r = file::miku_set_nonblocking(fd as i64, on);
        if r < 0 { return fail(-r as i32); }
        return 0;
    }
    // Terminal settings in Linux's layout: `c/termios.rs` translates the
    // kernel's twelve bytes, so `ioctl(TCGETS)` and `tcgetattr` agree
    if let Some(r) = crate::ctermios::ioctl_termios(fd, request, arg) {
        return r;
    }
    let r = io::miku_ioctl(fd as u64, request, arg as u64);
    if r < 0 { return fail(-r as i32); }
    r as i32
}

// sys/uio.h //

/// Scatter/gather as a loop of plain reads and writes, stopping at the first
/// short transfer as `readv` does. A concurrent writer can interleave between
/// segments
unsafe fn iov_loop(
    iov: *const iovec,
    iovcnt: i32,
    mut each: impl FnMut(*mut u8, usize, i64) -> i64,
) -> i64 {
    if iovcnt < 0 || iovcnt > 1024 { return -(EINVAL as i64); }
    if iovcnt > 0 && iov.is_null() { return -(EFAULT as i64); }
    let mut total: i64 = 0;
    for i in 0..iovcnt as usize {
        let v = unsafe { &*iov.add(i) };
        if v.iov_len == 0 { continue; }
        let n = each(v.iov_base, v.iov_len, total);
        if n < 0 {
            // An error after some bytes moved reports those bytes, per POSIX;
            // the error returns on the next call
            return if total > 0 { total } else { n };
        }
        total += n;
        if (n as usize) < v.iov_len { break; }
    }
    total
}

fn iov_result(r: i64) -> i64 {
    if r < 0 { set_errno(-r as i32); -1 } else { r }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readv(fd: i32, iov: *const iovec, iovcnt: i32) -> i64 {
    iov_result(unsafe { iov_loop(iov, iovcnt, |p, n, _| io::miku_read(fd as u64, p, n)) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn writev(fd: i32, iov: *const iovec, iovcnt: i32) -> i64 {
    iov_result(unsafe { iov_loop(iov, iovcnt, |p, n, _| io::miku_write(fd as u64, p, n)) })
}

/// Positioned, inheriting `miku_pread`: a seek, a read, and a seek back, so not
/// atomic against another thread on this descriptor
#[no_mangle]
#[inline(never)]
pub extern "C" fn preadv(fd: i32, iov: *const iovec, iovcnt: i32, offset: i64) -> i64 {
    if offset < 0 { set_errno(EINVAL); return -1; }
    iov_result(unsafe {
        iov_loop(iov, iovcnt, |p, n, done| file::miku_pread(fd as i64, p, n, offset + done))
    })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pwritev(fd: i32, iov: *const iovec, iovcnt: i32, offset: i64) -> i64 {
    if offset < 0 { set_errno(EINVAL); return -1; }
    iov_result(unsafe {
        iov_loop(iov, iovcnt, |p, n, done| file::miku_pwrite(fd as i64, p, n, offset + done))
    })
}

// dirent.h: positions

#[no_mangle]
#[inline(never)]
pub extern "C" fn dirfd(d: *mut DIR) -> i32 {
    if d.is_null() { return fail(EINVAL); }
    unsafe { (*d).fd as i32 }
}

/// Index of the entry the next `readdir` returns. `offset` is where the batch
/// in hand ends, minus what is left of it
#[no_mangle]
#[inline(never)]
pub extern "C" fn telldir(d: *mut DIR) -> i64 {
    if d.is_null() { return fail(EINVAL) as i64; }
    let dp = unsafe { &*d };
    (dp.offset - (dp.count - dp.pos)) as i64
}

/// Back to a position `telldir` gave; the next `readdir` refills from there. A
/// position, not a cookie: entries added or removed move it
#[no_mangle]
#[inline(never)]
pub extern "C" fn seekdir(d: *mut DIR, loc: i64) {
    if d.is_null() || loc < 0 { return; }
    let dp = unsafe { &mut *d };
    dp.offset = loc as usize;
    dp.count = 0;
    dp.pos = 0;
}

// sys/statfs.h, sys/statvfs.h //

/// `statfs`(36)'s 48-byte answer, documented at `miku_statfs`, shared by
/// `statfs`, `fstatfs`, `statvfs`, and `fstatvfs`
struct FsInfo {
    magic: u64,
    bsize: u64,
    blocks: u64,
    bfree: u64,
    files: u64,
    ffree: u64,
    namelen: u64,
    flags: u64,
}

impl FsInfo {
    fn parse(raw: &[u8; 48]) -> Self {
        let u32_at = |o: usize| u32::from_le_bytes(raw[o..o + 4].try_into().unwrap()) as u64;
        let u64_at = |o: usize| u64::from_le_bytes(raw[o..o + 8].try_into().unwrap());
        FsInfo {
            magic: u32_at(0),
            bsize: u32_at(4),
            blocks: u64_at(8),
            bfree: u64_at(16),
            files: u64_at(24),
            ffree: u64_at(32),
            namelen: u32_at(40),
            flags: u32_at(44),
        }
    }

    fn statfs(&self) -> statfs {
        statfs {
            f_type:    self.magic as i64,
            f_bsize:   self.bsize as i64,
            f_blocks:  self.blocks,
            f_bfree:   self.bfree,
            // No blocks reserved for root, so all free blocks are available
            f_bavail:  self.bfree,
            f_files:   self.files,
            f_ffree:   self.ffree,
            f_fsid:    [0; 2],
            f_namelen: self.namelen as i64,
            f_frsize:  self.bsize as i64,
            f_flags:   self.flags as i64,
            f_spare:   [0; 4],
        }
    }

    /// The fields the `libc` module's `statvfs` documents, in its order
    fn statvfs(&self) -> statvfs {
        statvfs {
            f_bsize: self.bsize,
            f_frsize: self.bsize,
            f_blocks: self.blocks,
            f_bfree: self.bfree,
            f_bavail: self.bfree,
            f_files: self.files,
            f_ffree: self.ffree,
            f_favail: self.ffree,
            f_fsid: 0,
            f_flag: 0,
            f_namemax: self.namelen,
            f_type: self.magic,
        }
    }
}

/// POSIX's `statvfs`, in the `libc` module's layout (see `statvfs` there for
/// the four fields with no source)
#[repr(C)]
pub struct statvfs {
    pub f_bsize: u64,
    pub f_frsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_favail: u64,
    pub f_fsid: u64,
    pub f_flag: u64,
    pub f_namemax: u64,
    pub f_type: u64,
}

fn fs_info(r: Result<(), i32>, raw: &[u8; 48]) -> Result<FsInfo, i32> {
    r.map(|()| FsInfo::parse(raw))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn statfs(path: *const u8, out: *mut statfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = [0u8; 48];
    match fs_info(crate::at::statfs_raw(AT_FDCWD, path, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statfs()) }; 0 }
        Err(e) => fail(e),
    }
}

/// The filesystem the descriptor's file is on, whatever kind of file it is
#[no_mangle]
#[inline(never)]
pub extern "C" fn fstatfs(fd: i32, out: *mut statfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = [0u8; 48];
    match fs_info(crate::at::fstatfs_raw(fd, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statfs()) }; 0 }
        Err(e) => fail(e),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn statvfs(path: *const u8, out: *mut statvfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = [0u8; 48];
    match fs_info(crate::at::statfs_raw(AT_FDCWD, path, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statvfs()) }; 0 }
        Err(e) => fail(e),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fstatvfs(fd: i32, out: *mut statvfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = [0u8; 48];
    match fs_info(crate::at::fstatfs_raw(fd, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statvfs()) }; 0 }
        Err(e) => fail(e),
    }
}

// The *at family; the rest is lib/libmiku/c/at.rs

#[no_mangle]
#[inline(never)]
pub extern "C" fn faccessat(dirfd: i32, path: *const u8, mode: i32, flags: i32) -> i32 {
    if flags & !(AT_EACCESS | AT_SYMLINK_NOFOLLOW) != 0 { return fail(EINVAL); }
    if mode & !(R_OK | W_OK | X_OK) != 0 { return fail(EINVAL); }
    let (p, w) = match crate::at::word(dirfd, path) {
        Ok(w) => w,
        Err(e) => return fail(e),
    };

    let mut st = unsafe { core::mem::zeroed::<file::MikuStat>() };
    let nr = if flags & AT_SYMLINK_NOFOLLOW != 0 { crate::sys::SYS_LSTAT } else { crate::sys::SYS_STAT };
    let r = unsafe { crate::sys::sc3(nr, p, w, &mut st as *mut _ as u64) };
    if r < 0 { return fail(-r as i32); }
    if mode == 0 { return 0; }

    // The real ids unless asked for effective ones, which matters to a program
    // started through mikudo
    let (uid, gid) = if flags & AT_EACCESS != 0 {
        (proc::miku_geteuid(), proc::miku_getegid())
    } else {
        (proc::miku_getuid(), proc::miku_getgid())
    };
    let perm = st.mode & 0o777;
    if uid == 0 {
        // Root reads and writes anything, and executes anything something can
        let any_x = perm & 0o111 != 0 || st.kind == file::KIND_DIRECTORY;
        return if mode & X_OK != 0 && !any_x { fail(EACCES) } else { 0 };
    }
    let bits = if uid == st.uid as u64 {
        (perm >> 6) & 7
    } else if gid == st.gid as u64 {
        (perm >> 3) & 7
    } else {
        perm & 7
    };
    if (mode as u32) & !bits != 0 { fail(EACCES) } else { 0 }
}

/// `ENOSYS`: no `mknod`. Pipes come from `pipe`(34), device nodes from the
/// kernel
#[no_mangle]
#[inline(never)]
pub extern "C" fn mknodat(_dirfd: i32, _path: *const u8, _mode: u32, _dev: u64) -> i32 {
    fail(ENOSYS)
}

/// A file's times via `utimensat`(40), passing `times` through as two
/// `timespec`s or null
#[no_mangle]
#[inline(never)]
pub extern "C" fn utimensat(dirfd: i32, path: *const u8, times: *const timespec, flags: i32) -> i32 {
    let (p, w) = match crate::at::word(dirfd, path) {
        Ok(pw) => pw,
        Err(e) => return fail(e),
    };
    let r = unsafe {
        crate::sys::sc4(crate::sys::SYS_UTIMENSAT, p, w, times as u64, flags as u32 as u64)
    };
    if r < 0 { fail(-r as i32) } else { 0 }
}

/// The same on a descriptor: an empty path with the descriptor as its base
#[no_mangle]
#[inline(never)]
pub extern "C" fn futimens(fd: i32, times: *const timespec) -> i32 {
    if fd < 0 { return fail(EBADF); }
    let w = (fd as u64 + 1) << 32;
    let r = unsafe { crate::sys::sc4(crate::sys::SYS_UTIMENSAT, 0, w, times as u64, 0) };
    if r < 0 { fail(-r as i32) } else { 0 }
}

// fcntl.h: advice and allocation

/// Advisory and ignored, so "done" conforms. Returns the error, not `-1`, as
/// POSIX specifies for this call
#[no_mangle]
#[inline(never)]
pub extern "C" fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32 {
    if offset < 0 || len < 0 || !(0..=5).contains(&advice) { return EINVAL; }
    // A descriptor that is not open is still an error
    let mut st = unsafe { core::mem::zeroed::<file::MikuStat>() };
    if file::miku_fstat(fd as i64, &mut st) < 0 { return EBADF; }
    0
}

/// Through `fallocate`(37). Returns the error without setting errno, per POSIX
#[no_mangle]
#[inline(never)]
pub extern "C" fn posix_fallocate(fd: i32, offset: i64, len: i64) -> i32 {
    if offset < 0 || len <= 0 { return EINVAL; }
    let r = file::miku_fallocate(fd as i64, offset as u64, len as u64);
    if r < 0 { -r as i32 } else { 0 }
}

// dlfcn.h //

/// Always null: `ld-miku` resolves everything before `main` and keeps no symbol
/// table to ask. Callers such as `std`'s weak symbols and `rustix`'s probes
/// have a fallback
#[no_mangle]
#[inline(never)]
pub extern "C" fn dlsym(_handle: *mut u8, _symbol: *const u8) -> *mut u8 {
    core::ptr::null_mut()
}

// unistd.h: sysconf

#[no_mangle]
#[inline(never)]
pub extern "C" fn sysconf(name: i32) -> i64 {
    match name {
        _SC_PAGESIZE => 4096,
        _SC_CLK_TCK  => 250,
        _SC_OPEN_MAX => OPEN_MAX as i64,
        // Buffer size for `getpwuid_r`: 1024, as in glibc
        _SC_GETPW_R_SIZE_MAX => 1024,
        // Each answer is where the limit lives; Linux's numbering, as the
        // `libc` crate and `<unistd.h>` send
        69 /* GETGR_R_SIZE_MAX */ => 1024,
        0 /* ARG_MAX: usercopy's MAX_ARGV_TOTAL_BYTES */ => 128 * 1024,
        3 /* NGROUPS_MAX: the credential's group array */ => 16,
        5 /* STREAM_MAX */ => OPEN_MAX as i64,
        6 /* TZNAME_MAX: UTC is the only zone */ => 3,
        60 /* IOV_MAX: readv/writev */ => 1024,
        43 /* LINE_MAX */ => 2048,
        71 /* LOGIN_NAME_MAX */ | 72 /* TTY_NAME_MAX */ => 32,
        180 /* HOST_NAME_MAX */ => 64,
        87 /* ATEXIT_MAX: libc's table */ => 32,
        74 /* THREAD_KEYS_MAX: c/pthread.rs */ => 128,
        73 /* THREAD_DESTRUCTOR_ITERATIONS */ => 4,
        75 /* THREAD_STACK_MIN: pthread_attr_setstacksize's floor */ => 16384,
        // POSIX's minimums, implemented only by this libc's regex
        36 /* BC_BASE_MAX */ | 38 /* BC_SCALE_MAX */ => 99,
        37 /* BC_DIM_MAX */ => 2048,
        39 /* BC_STRING_MAX */ => 1000,
        40 /* COLL_WEIGHTS_MAX */ => 2,
        42 /* EXPR_NEST_MAX */ => 32,
        44 /* RE_DUP_MAX */ => 255,
        // Options this system has, with POSIX.1-2008's version number: job
        // control, saved set-ids, fsync, file mappings, mprotect,
        // CLOCK_MONOTONIC, threads, thread-safe functions, a stack size
        // attribute
        7 | 8 | 15 | 16 | 19 | 149 | 67 | 68 | 78 => 200809,
        29 /* VERSION */ => 200809,
        // A limit this system lacks or an option it does not support: -1 with
        // errno untouched, per POSIX. Realtime signals, timers, AIO,
        // prioritized and synchronized I/O, mlock, message passing, semaphores,
        // shared memory objects, thread stack addresses and priorities,
        // process-shared sync, the POSIX.2 utilities, XSI shared memory
        1 /* CHILD_MAX */ | 76 /* THREAD_THREADS_MAX */
        | 9..=14 | 17 | 18 | 20..=28 | 31..=35 | 77 | 79..=82
        | 46..=52 | 94 | 95 | 97 => -1,
        // `_SC_NPROCESSORS_ONLN` among the rest: the CPU count stays in ring 0,
        // and `std`'s `available_parallelism` agrees
        _ => { set_errno(EINVAL); -1 }
    }
}

// time.h //

#[no_mangle]
#[inline(never)]
pub extern "C" fn clock_getres(clockid: i32, tp: *mut timespec) -> i32 {
    if clockid != 0 && clockid != 1 { return fail(EINVAL); }
    if !tp.is_null() {
        unsafe { tp.write(timespec { tv_sec: 0, tv_nsec: TICK_NS }) };
    }
    0
}

/// `CLOCK_REALTIME` only, to the second: `clock_settime`(79) is root's, and the
/// wall clock keeps whole seconds
#[no_mangle]
#[inline(never)]
pub extern "C" fn clock_settime(clockid: i32, tp: *const timespec) -> i32 {
    if clockid != 0 { return fail(EINVAL); }
    if tp.is_null() { return fail(EFAULT); }
    let r = time::miku_set_realtime(unsafe { (*tp).tv_sec });
    if r < 0 { return fail(-r as i32); }
    0
}

// sys/resource.h //

/// The descriptor ceiling for `RLIMIT_NOFILE`, and no limit for the rest;
/// nothing else in the table is enforced
#[no_mangle]
#[inline(never)]
pub extern "C" fn getrlimit(resource: i32, rlim: *mut rlimit) -> i32 {
    if !(0..16).contains(&resource) { return fail(EINVAL); }
    if rlim.is_null() { return fail(EFAULT); }
    let v = if resource == RLIMIT_NOFILE { OPEN_MAX } else { RLIM_INFINITY };
    unsafe { rlim.write(rlimit { rlim_cur: v, rlim_max: v }) };
    0
}

/// Succeeds when asked for what `getrlimit` reports, as Zed does at start
/// raising its soft limit to the hard one, and answers `ENOSYS` otherwise
#[no_mangle]
#[inline(never)]
pub extern "C" fn setrlimit(resource: i32, rlim: *const rlimit) -> i32 {
    if !(0..16).contains(&resource) { return fail(EINVAL); }
    if rlim.is_null() { return fail(EFAULT); }
    let want = unsafe { &*rlim };
    if want.rlim_cur > want.rlim_max { return fail(EINVAL); }
    let have = if resource == RLIMIT_NOFILE { OPEN_MAX } else { RLIM_INFINITY };
    if want.rlim_cur == have && want.rlim_max == have { 0 } else { fail(ENOSYS) }
}

/// `ENOSYS` both ways: no syscall reads or sets a task's priority
#[no_mangle]
#[inline(never)]
pub extern "C" fn getpriority(_which: i32, _who: u32) -> i32 {
    fail(ENOSYS)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn setpriority(_which: i32, _who: u32, _prio: i32) -> i32 {
    fail(ENOSYS)
}

// sys/wait.h //

/// `ENOSYS`: `wait4`(44) is the only wait, and `waitid`'s `WNOWAIT` cannot be
/// done
#[no_mangle]
#[inline(never)]
pub extern "C" fn waitid(_idtype: i32, _id: u32, _infop: *mut u8, _options: i32) -> i32 {
    fail(ENOSYS)
}

// sys/socket.h: the BSD calls, over syscalls 56-66, 77, and 80
//
// The kernel's socket calls take BSD's arguments, a `sockaddr` and a length,
// told apart by the family in the first two bytes. Added here: the `SOCK_*`
// flags in the type word, the `MSG_*` flags, unchecked address lengths, and
// `sendmsg`/`recvmsg`, which are not syscalls.

use crate::sys::{sc1, sc2, sc3, sc4};
use crate::sys::{
    SYS_ACCEPT, SYS_BIND, SYS_CONNECT, SYS_LISTEN, SYS_RECV, SYS_RECVFROM, SYS_SEND,
    SYS_GETSOCKOPT, SYS_SENDTO, SYS_SETSOCKOPT, SYS_SOCKET, SYS_SOCKETPAIR,
};

const AF_UNIX: i32 = 1;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_TYPE_FLAGS: i32 = O_NONBLOCK | O_CLOEXEC; // SOCK_NONBLOCK | SOCK_CLOEXEC

const IPPROTO_IP: i32 = 0;
const IP_TTL: i32 = 2;

const MSG_PEEK: i32 = 0x2;
const MSG_DONTWAIT: i32 = 0x40;
const MSG_WAITALL: i32 = 0x100;
const MSG_NOSIGNAL: i32 = 0x4000;
const MSG_CMSG_CLOEXEC: i32 = 0x4000_0000;

const EAFNOSUPPORT: i32 = 97;
const EPROTONOSUPPORT: i32 = 93;
const ENOPROTOOPT: i32 = 92;
pub(crate) const EOPNOTSUPP: i32 = 95;
const EMSGSIZE: i32 = 90;

/// The 16-byte `sockaddr_in` the kernel reads and writes for `AF_INET`
const SOCKADDR_IN_LEN: u32 = 16;

/// `struct msghdr` as `libc` declares it for MikuOS, with POSIX's field types
#[repr(C)]
pub struct msghdr {
    pub msg_name:       *mut u8,
    pub msg_namelen:    u32,
    pub msg_iov:        *mut iovec,
    pub msg_iovlen:     i32,
    pub msg_control:    *mut u8,
    pub msg_controllen: usize,
    pub msg_flags:      i32,
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn socket(domain: i32, ty: i32, protocol: i32) -> i32 {
    // The kernel matches the type exactly, so the two flags `socket` may carry
    // come off here and are applied as `fcntl` would: `O_NONBLOCK` and
    // `FD_CLOEXEC` on the socket's slot
    let flags = ty & SOCK_TYPE_FLAGS;
    let base = ty & !SOCK_TYPE_FLAGS;
    let fd = unsafe { sc3(SYS_SOCKET, domain as u64, base as u64, protocol as u64) };
    if fd < 0 { return fail(-fd as i32); }
    let r = apply_fd_flags(fd, flags);
    if r < 0 {
        file::miku_close(fd);
        return fail(-r as i32);
    }
    fd as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn connect(fd: i32, addr: *const u8, len: u32) -> i32 {
    // Non-blocking: `EINPROGRESS`, then `poll` for writable and `SO_ERROR` for
    // the outcome, as on any Unix. A local socket is writable once the listener
    // accepts. See `docs/MikuOS_ABI.md`
    ret(unsafe { sc3(SYS_CONNECT, fd as u64, addr as u64, len as u64) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn bind(fd: i32, addr: *const u8, len: u32) -> i32 {
    ret(unsafe { sc3(SYS_BIND, fd as u64, addr as u64, len as u64) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn listen(fd: i32, backlog: i32) -> i32 {
    ret(unsafe { sc2(SYS_LISTEN, fd as u64, backlog.max(0) as u64) }) as i32
}

/// The kernel writes a whole `sockaddr_in` for a TCP peer and nothing for a
/// local one, reading neither the caller's length nor its buffer size. So the
/// address lands in a local buffer, marked so "nothing written" is detectable,
/// and is copied out within the caller's room
unsafe fn accept_into(fd: i32, addr: *mut u8, lenp: *mut u32) -> i64 {
    const UNTOUCHED: u16 = 0xffff;
    let mut buf = [0u8; SOCKADDR_IN_LEN as usize];
    buf[0..2].copy_from_slice(&UNTOUCHED.to_le_bytes());
    let r = unsafe { sc3(SYS_ACCEPT, fd as u64, buf.as_mut_ptr() as u64, SOCKADDR_IN_LEN as u64) };
    if r < 0 { return r; }
    if !addr.is_null() && !lenp.is_null() {
        let room = unsafe { *lenp } as usize;
        let family = u16::from_le_bytes([buf[0], buf[1]]);
        let (src, len): (&[u8], u32) = if family == UNTOUCHED {
            // A local socket: unnamed, a bare family
            (&(AF_UNIX as u16).to_le_bytes()[..], 2)
        } else {
            (&buf[..], SOCKADDR_IN_LEN)
        };
        let n = room.min(src.len());
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), addr, n);
            *lenp = len;
        }
    }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn accept(fd: i32, addr: *mut u8, lenp: *mut u32) -> i32 {
    ret(unsafe { accept_into(fd, addr, lenp) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn accept4(fd: i32, addr: *mut u8, lenp: *mut u32, flags: i32) -> i32 {
    if flags & !SOCK_TYPE_FLAGS != 0 { return fail(EINVAL); }
    let r = unsafe { accept_into(fd, addr, lenp) };
    if r < 0 { return fail(-r as i32); }
    let f = apply_fd_flags(r, flags);
    if f < 0 {
        file::miku_close(r);
        return fail(-f as i32);
    }
    r as i32
}

/// `EOPNOTSUPP` both: no `getsockname` or `getpeername` syscall
#[no_mangle]
#[inline(never)]
pub extern "C" fn getsockname(_fd: i32, _addr: *mut u8, _lenp: *mut u32) -> i32 {
    fail(EOPNOTSUPP)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpeername(_fd: i32, _addr: *mut u8, _lenp: *mut u32) -> i32 {
    fail(EOPNOTSUPP)
}

/// `IP_TTL`, the one option of `setsockopt`(80), and `ENOPROTOOPT` for every
/// other
#[no_mangle]
#[inline(never)]
pub extern "C" fn setsockopt(fd: i32, level: i32, name: i32, val: *const u8, len: u32) -> i32 {
    if level == IPPROTO_IP && name == IP_TTL {
        if val.is_null() || len < 4 { return fail(EINVAL); }
        let ttl = unsafe { (val as *const i32).read_unaligned() };
        return ret(unsafe { sc4(SYS_SETSOCKOPT, fd as u64, level as u64, name as u64, ttl as u64) }) as i32;
    }
    // No Nagle in the kernel's TCP: every socket is already TCP_NODELAY, so
    // turning it off cannot be done
    if level == IPPROTO_TCP && name == TCP_NODELAY {
        if val.is_null() || len < 4 { return fail(EINVAL); }
        let on = unsafe { (val as *const i32).read_unaligned() } != 0;
        return if on { 0 } else { fail(ENOPROTOOPT) };
    }
    // Keepalive: the kernel's TCP probes a silent connection. One integer each,
    // by value, as `setsockopt`(80) takes them
    const SO_KEEPALIVE: i32 = 9;
    const TCP_KEEPIDLE: i32 = 4;
    const TCP_KEEPCNT: i32 = 6;
    if (level == SOL_SOCKET && name == SO_KEEPALIVE)
        || (level == IPPROTO_TCP && (TCP_KEEPIDLE..=TCP_KEEPCNT).contains(&name))
    {
        if val.is_null() || len < 4 { return fail(EINVAL); }
        let v = unsafe { (val as *const i32).read_unaligned() };
        if v < 0 { return fail(EINVAL); }
        return ret(unsafe { sc4(SYS_SETSOCKOPT, fd as u64, level as u64, name as u64, v as u64) }) as i32;
    }
    fail(ENOPROTOOPT)
}

const IPPROTO_TCP: i32 = 6;
const TCP_NODELAY: i32 = 1;

/// Through `getsockopt`(114), which answers with the value, since every option
/// it knows is one integer. `SO_ERROR` reads and clears the error of a
/// non-blocking connect, as `mio` and `async-io` use
#[no_mangle]
#[inline(never)]
pub extern "C" fn getsockopt(fd: i32, level: i32, name: i32, val: *mut u8, lenp: *mut u32) -> i32 {
    if val.is_null() || lenp.is_null() || unsafe { *lenp } < 4 { return fail(EINVAL); }
    if level == SOL_SOCKET && name == SO_PEERCRED {
        return peercred_opt(fd, val, lenp);
    }
    if level == IPPROTO_TCP && name == TCP_NODELAY {
        unsafe {
            (val as *mut i32).write_unaligned(1);
            *lenp = 4;
        }
        return 0;
    }
    let r = unsafe { sc3(SYS_GETSOCKOPT, fd as u64, level as u64, name as u64) };
    if r < 0 { return fail(-r as i32); }
    unsafe {
        (val as *mut i32).write_unaligned(r as i32);
        *lenp = 4;
    }
    0
}

const SOL_SOCKET: i32 = 1;
const SO_PEERCRED: i32 = 17;

/// `SO_PEERCRED`, from `peercred`(83): pid, then the peer's effective uid and
/// gid at connect, as Linux reports. The kernel's record also has real ids and
/// a start tick
fn peercred_opt(fd: i32, val: *mut u8, lenp: *mut u32) -> i32 {
    if unsafe { *lenp } < 12 { return fail(EINVAL); }
    let mut rec = [0u8; 40];
    let r = proc::miku_peercred(fd as u64, rec.as_mut_ptr());
    if r < 0 { return fail(-r as i32); }
    let pid = u64::from_le_bytes(rec[0..8].try_into().unwrap()) as i32;
    let euid = u32::from_le_bytes(rec[24..28].try_into().unwrap());
    let egid = u32::from_le_bytes(rec[28..32].try_into().unwrap());
    unsafe {
        (val as *mut i32).write_unaligned(pid);
        (val.add(4) as *mut u32).write_unaligned(euid);
        (val.add(8) as *mut u32).write_unaligned(egid);
        *lenp = 12;
    }
    0
}

/// `EOPNOTSUPP`: half-close is not in the syscall table
#[no_mangle]
#[inline(never)]
pub extern "C" fn shutdown(_fd: i32, _how: i32) -> i32 {
    fail(EOPNOTSUPP)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn socketpair(domain: i32, ty: i32, protocol: i32, sv: *mut i32) -> i32 {
    if sv.is_null() { return fail(EFAULT); }
    if domain != AF_UNIX { return fail(EAFNOSUPPORT); }
    let flags = ty & SOCK_TYPE_FLAGS;
    // `socketpair`(77) makes local streams and nothing else
    if ty & !SOCK_TYPE_FLAGS != SOCK_STREAM || protocol != 0 { return fail(EPROTONOSUPPORT); }
    let mut fds = [0u64; 2];
    let r = unsafe { sc1(SYS_SOCKETPAIR, fds.as_mut_ptr() as u64) };
    if r < 0 { return fail(-r as i32); }
    for &fd in &fds {
        let f = apply_fd_flags(fd as i64, flags);
        if f < 0 {
            file::miku_close(fds[0] as i64);
            file::miku_close(fds[1] as i64);
            return fail(-f as i32);
        }
    }
    unsafe {
        *sv = fds[0] as i32;
        *sv.add(1) = fds[1] as i32;
    }
    0
}

/// `MSG_*` flags `send`(58) and `recv`(59) do not read, handled here or
/// refused. `MSG_NOSIGNAL` is already true: only a pipe raises `SIGPIPE`, and a
/// socket answers `EPIPE`. `MSG_DONTWAIT` makes the descriptor non-blocking for
/// one call, which another thread on the socket can observe
fn with_msg_flags(fd: i32, flags: i32, allowed: i32, f: impl FnOnce() -> i64) -> i64 {
    if flags & !allowed != 0 { return -(EOPNOTSUPP as i64); }
    if flags & MSG_DONTWAIT == 0 { return f(); }
    let cur = file::miku_fcntl(fd as i64, F_GETFL as u64, 0);
    if cur < 0 { return cur; }
    if cur & O_NONBLOCK as i64 != 0 { return f(); }
    let s = file::miku_set_nonblocking(fd as i64, true);
    if s < 0 { return s; }
    let r = f();
    file::miku_set_nonblocking(fd as i64, false);
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn send(fd: i32, buf: *const u8, len: usize, flags: i32) -> i64 {
    let allowed = MSG_DONTWAIT | MSG_NOSIGNAL;
    ret(with_msg_flags(fd, flags, allowed, || unsafe {
        sc4(SYS_SEND, fd as u64, buf as u64, len as u64, 0)
    }))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> i64 {
    // `MSG_PEEK` is refused: there is no way to read without taking
    let allowed = MSG_DONTWAIT | MSG_WAITALL;
    ret(with_msg_flags(fd, flags, allowed, || {
        if flags & MSG_WAITALL == 0 {
            return unsafe { sc4(SYS_RECV, fd as u64, buf as u64, len as u64, 0) };
        }
        // All of it, or end-of-stream, or an error after nothing
        let mut got = 0usize;
        while got < len {
            let r = unsafe { sc4(SYS_RECV, fd as u64, buf.add(got) as u64, (len - got) as u64, 0) };
            if r < 0 { return if got > 0 { got as i64 } else { r }; }
            if r == 0 { break; }
            got += r as usize;
        }
        got as i64
    }))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sendto(
    fd: i32, buf: *const u8, len: usize, flags: i32, addr: *const u8, alen: u32,
) -> i64 {
    if addr.is_null() { return send(fd, buf, len, flags); }
    // `sendto`(65) reads a fixed 16-byte `sockaddr_in` with no length, so the
    // length and family are checked here
    if alen < SOCKADDR_IN_LEN { return fail(EINVAL) as i64; }
    let family = unsafe { (addr as *const u16).read_unaligned() } as i32;
    if family != AF_INET { return fail(EAFNOSUPPORT) as i64; }
    let allowed = MSG_DONTWAIT | MSG_NOSIGNAL;
    ret(with_msg_flags(fd, flags, allowed, || unsafe {
        sc4(SYS_SENDTO, fd as u64, buf as u64, len as u64, addr as u64)
    }))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn recvfrom(
    fd: i32, buf: *mut u8, len: usize, flags: i32, addr: *mut u8, lenp: *mut u32,
) -> i64 {
    // The kernel writes a whole `sockaddr_in` given somewhere to put it; here,
    // then as much as the caller has room for
    let mut sa = [0u8; SOCKADDR_IN_LEN as usize];
    let allowed = MSG_DONTWAIT;
    let r = with_msg_flags(fd, flags, allowed, || unsafe {
        sc4(SYS_RECVFROM, fd as u64, buf as u64, len as u64, sa.as_mut_ptr() as u64)
    });
    if r < 0 { return ret(r); }
    if !addr.is_null() && !lenp.is_null() {
        let n = (unsafe { *lenp } as usize).min(sa.len());
        unsafe {
            core::ptr::copy_nonoverlapping(sa.as_ptr(), addr, n);
            *lenp = SOCKADDR_IN_LEN;
        }
    }
    r
}

/// The iovecs of a `msghdr`, as a slice
unsafe fn msg_iovs<'a>(m: &msghdr) -> Result<&'a [iovec], i32> {
    if m.msg_iovlen < 0 || m.msg_iovlen > 1024 { return Err(EINVAL); }
    if m.msg_iovlen == 0 { return Ok(&[]); }
    if m.msg_iov.is_null() { return Err(EFAULT); }
    Ok(unsafe { core::slice::from_raw_parts(m.msg_iov, m.msg_iovlen as usize) })
}

/// One `send` of all segments gathered in one buffer, so a datagram stays one
/// datagram. Ancillary data is refused: descriptors pass with `sendfd`(84), one
/// per call, not `SCM_RIGHTS`
#[no_mangle]
#[inline(never)]
pub extern "C" fn sendmsg(fd: i32, msg: *const msghdr, flags: i32) -> i64 {
    if msg.is_null() { return fail(EFAULT) as i64; }
    let m = unsafe { &*msg };
    if !m.msg_control.is_null() && m.msg_controllen > 0 { return fail(EOPNOTSUPP) as i64; }
    let iovs = match unsafe { msg_iovs(m) } {
        Ok(v) => v,
        Err(e) => return fail(e) as i64,
    };
    let total: usize = iovs.iter().map(|v| v.iov_len).sum();
    if total > 1 << 20 { return fail(EMSGSIZE) as i64; }
    let one = iovs.iter().filter(|v| v.iov_len > 0).count() <= 1;
    let (ptr, owned) = if one {
        let p = iovs.iter().find(|v| v.iov_len > 0).map(|v| v.iov_base).unwrap_or(core::ptr::null_mut());
        (p, false)
    } else {
        let p = heap::miku_malloc(total.max(1));
        if p.is_null() { return fail(ENOMEM) as i64; }
        let mut off = 0;
        for v in iovs {
            unsafe { core::ptr::copy_nonoverlapping(v.iov_base, p.add(off), v.iov_len) };
            off += v.iov_len;
        }
        (p, true)
    };
    let r = if m.msg_name.is_null() {
        send(fd, ptr, total, flags)
    } else {
        sendto(fd, ptr, total, flags, m.msg_name, m.msg_namelen)
    };
    if owned { heap::miku_free(ptr); }
    r
}

/// One `recv` into the segments, scattered from one buffer. No ancillary data
/// arrives, so a control buffer comes back empty
#[no_mangle]
#[inline(never)]
pub extern "C" fn recvmsg(fd: i32, msg: *mut msghdr, flags: i32) -> i64 {
    if msg.is_null() { return fail(EFAULT) as i64; }
    let m = unsafe { &mut *msg };
    let flags = flags & !MSG_CMSG_CLOEXEC;
    let iovs = match unsafe { msg_iovs(m) } {
        Ok(v) => v,
        Err(e) => return fail(e) as i64,
    };
    let total: usize = iovs.iter().map(|v| v.iov_len).sum();
    let nonempty = iovs.iter().filter(|v| v.iov_len > 0).count();
    let (ptr, owned) = if nonempty <= 1 {
        let p = iovs.iter().find(|v| v.iov_len > 0).map(|v| v.iov_base).unwrap_or(core::ptr::null_mut());
        (p, false)
    } else {
        let p = heap::miku_malloc(total.max(1));
        if p.is_null() { return fail(ENOMEM) as i64; }
        (p, true)
    };
    let r = if m.msg_name.is_null() {
        recv(fd, ptr, total, flags)
    } else {
        recvfrom(fd, ptr, total, flags, m.msg_name, &mut m.msg_namelen)
    };
    if owned {
        if r > 0 {
            let mut left = r as usize;
            let mut off = 0;
            for v in iovs {
                let n = left.min(v.iov_len);
                unsafe { core::ptr::copy_nonoverlapping(ptr.add(off), v.iov_base, n) };
                off += n;
                left -= n;
                if left == 0 { break; }
            }
        }
        heap::miku_free(ptr);
    }
    if r >= 0 {
        m.msg_controllen = 0;
        m.msg_flags = 0;
    }
    r
}

// What C programs need beyond the `libc` crate
//
// `lib/c/include` declares these; `dev/c-architecture.md` has the rule that a
// header declares nothing this library lacks. Larger areas are files under
// `lib/libmiku/c/`.

/// The standard streams, for `<stdio.h>`'s `stdin`/`stdout`/`stderr` macros.
/// Reached through a function, since link stubs carry functions and `ld-miku`
/// does no copy relocations
#[no_mangle]
#[inline(never)]
pub extern "C" fn __miku_stdfile(fd: i32) -> *mut FILE {
    unsafe {
        match fd {
            0 => stdin,
            1 => stdout,
            _ => stderr,
        }
    }
}

/// `assert`'s failure: glibc's message, then `abort`
#[no_mangle]
#[inline(never)]
pub extern "C" fn __assert_fail(expr: *const u8, file: *const u8, line: u32, func: *const u8) -> ! {
    fn put(s: *const u8) {
        if !s.is_null() {
            io::miku_write(2, s, string::miku_strlen(s));
        }
    }
    let mut num = [0u8; 12];
    let mut i = num.len();
    let mut n = line;
    loop {
        i -= 1;
        num[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 { break; }
    }
    unsafe { fflush(stdout); }
    put(file);
    io::miku_write(2, b":".as_ptr(), 1);
    io::miku_write(2, num[i..].as_ptr(), num.len() - i);
    io::miku_write(2, b": ".as_ptr(), 2);
    put(func);
    io::miku_write(2, b": Assertion `".as_ptr(), 13);
    put(expr);
    io::miku_write(2, b"' failed.\n".as_ptr(), 10);
    abort()
}

/// One locale, "C": asking for it, "", "POSIX", or nothing answers "C";
/// anything else is refused
#[no_mangle]
#[inline(never)]
pub extern "C" fn setlocale(_cat: i32, name: *const u8) -> *const u8 {
    const C: &[u8] = b"C\0";
    if name.is_null() {
        return C.as_ptr();
    }
    let n = string::miku_strlen(name);
    let s = unsafe { core::slice::from_raw_parts(name, n) };
    if s.is_empty() || s == b"C" || s == b"POSIX" || s == b"C.UTF-8" {
        C.as_ptr()
    } else {
        core::ptr::null()
    }
}

/// With one locale, collation is byte order
#[no_mangle]
#[inline(never)]
pub extern "C" fn strcoll(a: *const u8, b: *const u8) -> i32 {
    strcmp(a, b)
}

#[repr(C)]
pub struct DivT { quot: i32, rem: i32 }
#[repr(C)]
pub struct LDivT { quot: i64, rem: i64 }

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

/// C's `stat` in C's layout; `stat_path` is MikuOS's own
#[no_mangle]
#[inline(never)]
pub extern "C" fn stat(path: *const u8, st: *mut stat) -> i32 {
    c_stat_with(st, |k| file::miku_stat(path, k) as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn umask(mask: u32) -> u32 {
    unsafe { sc1(crate::sys::SYS_UMASK, mask as u64) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getuid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETUID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn geteuid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETEUID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getgid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETGID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getegid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETEGID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpagesize() -> i32 {
    4096
}

// sys/auxv.h //

/// Value of `key` in the program's auxiliary vector, or 0: glibc's `getauxval`,
/// from what ld-miku gave libmiku (`lib/libmiku/auxv.rs`) before any
/// constructor
#[no_mangle]
#[inline(never)]
pub extern "C" fn getauxval(key: u64) -> u64 {
    crate::auxv::miku_getauxval(key)
}

// wctype.h: Unicode classification from `core`'s tables, the same as Rust code
// in this process. Not a locale; there is one

fn wide(c: u32) -> Option<char> {
    char::from_u32(c)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswalpha(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_alphabetic()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswalnum(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_alphanumeric()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswdigit(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_ascii_digit()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswxdigit(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_ascii_hexdigit()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswspace(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_whitespace()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswblank(c: u32) -> i32 {
    wide(c).is_some_and(|c| c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\x0b' | '\x0c' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'))) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswcntrl(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_control()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswlower(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_lowercase()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswupper(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_uppercase()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswprint(c: u32) -> i32 { wide(c).is_some_and(|c| !c.is_control()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswgraph(c: u32) -> i32 { wide(c).is_some_and(|c| !c.is_control() && !c.is_whitespace()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswpunct(c: u32) -> i32 {
    wide(c).is_some_and(|c| !c.is_control() && !c.is_whitespace() && !c.is_alphanumeric()) as i32
}

/// The one-character mapping. A character mapping to several (German sharp s to
/// "SS") is returned unchanged, as C requires
#[no_mangle]
#[inline(never)]
pub extern "C" fn towlower(c: u32) -> u32 {
    match wide(c) {
        Some(ch) => {
            let mut it = ch.to_lowercase();
            match (it.next(), it.next()) {
                (Some(l), None) => l as u32,
                _ => c,
            }
        }
        None => c,
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn towupper(c: u32) -> u32 {
    match wide(c) {
        Some(ch) => {
            let mut it = ch.to_uppercase();
            match (it.next(), it.next()) {
                (Some(u), None) => u as u32,
                _ => c,
            }
        }
        None => c,
    }
}

/// `ENOSYS`: a descriptor knows no name here, no `/proc/self/fd` exists, and
/// the kernel's `chmod`(26) and `chown`(27) take a path. SQLite, the caller,
/// ignores the answer
#[no_mangle]
#[inline(never)]
pub extern "C" fn fchmod(fd: i32, mode: u32) -> i32 {
    ret(crate::file::miku_fchmod(fd, mode)) as i32
}

/// -1 (`(uid_t)-1`) leaves one unchanged, per POSIX; the kernel's word for that
/// is 0xFFFF
#[no_mangle]
#[inline(never)]
pub extern "C" fn fchown(fd: i32, uid: u32, gid: u32) -> i32 {
    let keep = |v: u32| if v == u32::MAX { 0xFFFF } else { v };
    ret(crate::file::miku_fchown(fd, keep(uid), keep(gid))) as i32
}

/// The microsecond form: two `struct timeval` {sec, usec}, or null for now
#[no_mangle]
#[inline(never)]
pub extern "C" fn utimes(path: *const u8, times: *const u8) -> i32 {
    const AT_FDCWD: i32 = -100;
    if times.is_null() {
        return utimensat(AT_FDCWD, path, core::ptr::null(), 0);
    }
    let tv = times as *const [i64; 2];
    let ts = unsafe {
        [
            timespec { tv_sec: (*tv)[0], tv_nsec: (*tv)[1] * 1000 },
            timespec { tv_sec: (*tv.add(1))[0], tv_nsec: (*tv.add(1))[1] * 1000 },
        ]
    };
    utimensat(AT_FDCWD, path, ts.as_ptr(), 0)
}

// dlfcn.h: no run-time loading, as C in a MikuOS program is linked in. Each
// call fails like a missing library, with a message saying why

#[no_mangle]
#[inline(never)]
pub extern "C" fn dlopen(_path: *const u8, _flags: i32) -> *mut u8 {
    core::ptr::null_mut()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dlerror() -> *const u8 {
    b"dynamic loading is not supported on MikuOS\0".as_ptr()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dlclose(_handle: *mut u8) -> i32 {
    -1
}
