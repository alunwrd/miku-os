///////////////////////////////////////////////////////////////////////
// libc compatibility layer for MikuOS                               //
// POSIX/C standard library functions implemented via libmiku        //
//																	 //
// Provides standard C function names (strlen, malloc, printf, etc.) //
// that map to the underlying libmiku implementation                 //
// Allows porting C programs to MikuOS with minimal changes          //
///////////////////////////////////////////////////////////////////////

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

#[no_mangle]
#[inline(never)]
pub extern "C" fn __errno_location() -> *mut i64 {
    // errno is stored in a static - return a pointer to it
    // libmiku uses get/set pattern, we expose a static for C compat
    static mut ERRNO_CELL: i64 = 0;
    unsafe {
        ERRNO_CELL = errno::get_errno();
        &mut ERRNO_CELL as *mut i64
    }
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

#[no_mangle]
#[inline(never)]
pub extern "C" fn exit(code: i32) -> ! {
    proc::miku_exit(code as i64)
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
    // libmiku setenv always overwrites; _overwrite param ignored
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

// FILE structure - wraps an fd with buffered I/O
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

fn file_from_fd(fd: i64, flags: u32) -> *mut FILE {
    let f = heap::miku_calloc(1, core::mem::size_of::<FILE>()) as *mut FILE;
    if f.is_null() { return core::ptr::null_mut(); }
    let fp = unsafe { &mut *f };
    fp.fd = fd;
    fp.flags = flags;
    fp.ungetc_buf = -1;
    fp.buf_mode = _IOFBF;
    file_alloc_bufs(f, flags);
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
    if f.is_null() { return 0; }
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

    // line-buffered: flush on newline
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

    // unbuffered or no write buffer - direct write
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
            // buffer full but flush made no room - abort to avoid infinite loop
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
                // flush failed to drain buffer - stop to avoid infinite loop
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
    if r < 0 { -1 } else { 0 }
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

// printf family delegate to miku_printf/miku_dprintf/miku_snprintf //
// These are variadic and implemented in assembly in fmt.rs         //
// We provide wrappers via global_asm trampolines                  //

core::arch::global_asm!(
    ".global printf",
    "printf:",
    "jmp miku_printf",
);

core::arch::global_asm!(
    ".global fprintf",
    "fprintf:",
    "jmp miku_dprintf",
);

core::arch::global_asm!(
    ".global dprintf",
    "dprintf:",
    "jmp miku_dprintf",
);

core::arch::global_asm!(
    ".global snprintf",
    "snprintf:",
    "jmp miku_snprintf",
);

core::arch::global_asm!(
    ".global sprintf",
    "sprintf:",
    // sprintf(buf, fmt, ...) -> snprintf(buf, SIZE_MAX, fmt, ...)
    // rdi=buf, rsi=fmt, rdx..r9=args
    // need to shift: rdi=buf, rsi=SIZE_MAX, rdx=fmt, rcx..r9=args
    "mov r10, r9",
    "mov r9, r8",
    "mov r8, rcx",
    "mov rcx, rdx",
    "mov rdx, rsi",
    "mov rsi, 0x7FFFFFFFFFFFFFFF",
    "jmp miku_snprintf",
);

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
    io::miku_read(fd as u64, buf, count)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> i64 {
    io::miku_write(fd as u64, buf, count)
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
    file::miku_lseek(fd as i64, offset, whence as u64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn open(path: *const u8, flags: i32, mode: u32) -> i32 {
    if path.is_null() { return -1; }
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
    if r < 0 { -1 } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dup2(old: i32, new: i32) -> i32 {
    let r = file::miku_dup2(old as i64, new as i64);
    if r < 0 { -1 } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pipe(fds: *mut i32) -> i32 {
    if fds.is_null() { return -1; }
    let mut fds64: [i64; 2] = [0; 2];
    let r = file::miku_pipe(fds64.as_mut_ptr());
    if r < 0 { return -1; }
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
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rmdir(path: *const u8) -> i32 {
    let r = file::miku_rmdir(path);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn mkdir(path: *const u8, mode: u32) -> i32 {
    let r = file::miku_mkdir(path, mode);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn link(old: *const u8, new: *const u8) -> i32 {
    let r = file::miku_link(old, new);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn symlink(target: *const u8, linkpath: *const u8) -> i32 {
    let r = file::miku_symlink(target, linkpath);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readlink(path: *const u8, buf: *mut u8, bufsiz: usize) -> i64 {
    file::miku_readlink(path, buf, bufsiz)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rename(old: *const u8, new: *const u8) -> i32 {
    let r = file::miku_rename(old, new);
    if r < 0 { -1 } else { 0 }
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
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpid() -> i32 {
    proc::miku_getpid() as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn access(path: *const u8, _mode: i32) -> i32 {
    if file::miku_access(path) { 0 } else { -1 }
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
    time::miku_sleep_ms((usec as u64 + 999) / 1000);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ftruncate(fd: i32, length: i64) -> i32 {
    let r = file::miku_ftruncate(fd as i64, length as u64);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn truncate(path: *const u8, length: i64) -> i32 {
    let fd = file::miku_open_rw(path);
    if fd < 0 { return -1; }
    let r = file::miku_ftruncate(fd, length as u64);
    file::miku_close(fd);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pread(fd: i32, buf: *mut u8, count: usize, offset: i64) -> i64 {
    file::miku_pread(fd as i64, buf, count, offset)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pwrite(fd: i32, buf: *const u8, count: usize, offset: i64) -> i64 {
    file::miku_pwrite(fd as i64, buf, count, offset)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sched_yield() -> i32 {
    time::miku_yield();
    0
}

// sys/stat.h //

// struct stat - maps to MikuStat
pub type stat = file::MikuStat;

#[no_mangle]
#[inline(never)]
pub extern "C" fn stat_path(path: *const u8, st: *mut file::MikuStat) -> i32 {
    let r = file::miku_stat(path, st);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fstat(fd: i32, st: *mut file::MikuStat) -> i32 {
    let r = file::miku_fstat(fd as i64, st);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chmod(path: *const u8, mode: u32) -> i32 {
    let r = file::miku_chmod(path, mode);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chown(path: *const u8, uid: u32, gid: u32) -> i32 {
    let r = file::miku_chown(path, uid, gid);
    if r < 0 { -1 } else { 0 }
}

// sys/mman.h //

pub const PROT_NONE:  u64 = 0;
pub const PROT_READ:  u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC:  u64 = 4;
pub const MAP_FAILED: *mut u8 = !0 as *mut u8;

#[no_mangle]
#[inline(never)]
pub extern "C" fn mmap(
    addr: *mut u8, length: usize, prot: i32,
    _flags: i32, _fd: i32, _offset: i64,
) -> *mut u8 {
    let p = proc::miku_mmap(addr as u64, length, prot as u64);
    if p.is_null() { MAP_FAILED } else { p }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn munmap(addr: *mut u8, length: usize) -> i32 {
    let r = proc::miku_munmap(addr, length);
    if r < 0 { -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn mprotect(addr: *mut u8, length: usize, prot: i32) -> i32 {
    let r = proc::miku_mprotect(addr as u64, length, prot as u64);
    if r < 0 { -1 } else { 0 }
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

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigaction(
    sig: i32,
    act: *const signal::MikuSigaction,
    oldact: *mut signal::MikuSigaction,
) -> i32 {
    signal::miku_sigaction(sig as u32, act, oldact)
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
    if req.is_null() { return -1; }
    let ts = unsafe { &*req };
    let ms = ts.tv_sec as u64 * 1000 + (ts.tv_nsec as u64 + 999_999) / 1_000_000;
    time::miku_sleep_ms(ms);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn clock_gettime(_clockid: i32, tp: *mut timespec) -> i32 {
    if tp.is_null() { return -1; }
    let ms = time::miku_uptime_ms();
    let tp = unsafe { &mut *tp };
    tp.tv_sec = (ms / 1000) as i64;
    tp.tv_nsec = ((ms % 1000) * 1_000_000) as i64;
    0
}

// dirent.h //

pub type dirent = file::MikuDirent;

// opaque DIR handle
#[repr(C)]
pub struct DIR {
    path: [u8; 256],
    entries: [file::MikuDirent; 128],
    count: usize,
    pos: usize,
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn opendir(path: *const u8) -> *mut DIR {
    if path.is_null() { return core::ptr::null_mut(); }
    let d = heap::miku_calloc(1, core::mem::size_of::<DIR>()) as *mut DIR;
    if d.is_null() { return core::ptr::null_mut(); }
    let dp = unsafe { &mut *d };

    let plen = string::miku_strlen(path);
    let copy_len = if plen < 255 { plen } else { 255 };
    mem::miku_memcpy(dp.path.as_mut_ptr(), path, copy_len);
    dp.path[copy_len] = 0;

    let n = file::miku_readdir(dp.path.as_ptr(), dp.entries.as_mut_ptr(), 128);
    if n < 0 {
        heap::miku_free(d as *mut u8);
        return core::ptr::null_mut();
    }
    dp.count = n as usize;
    dp.pos = 0;
    d
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readdir(d: *mut DIR) -> *const file::MikuDirent {
    if d.is_null() { return core::ptr::null(); }
    let dp = unsafe { &mut *d };
    if dp.pos >= dp.count { return core::ptr::null(); }
    let entry = &dp.entries[dp.pos] as *const file::MikuDirent;
    dp.pos += 1;
    entry
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn closedir(d: *mut DIR) -> i32 {
    if d.is_null() { return -1; }
    heap::miku_free(d as *mut u8);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rewinddir(d: *mut DIR) {
    if d.is_null() { return; }
    unsafe { (*d).pos = 0; }
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

// qsort - basic implementation using libmiku sort infrastructure
#[no_mangle]
#[inline(never)]
pub extern "C" fn qsort(
    base: *mut u8,
    nmemb: usize,
    size: usize,
    compar: extern "C" fn(*const u8, *const u8) -> i32,
) {
    if base.is_null() || nmemb <= 1 || size == 0 { return; }
    // simple insertion sort for small arrays, shell sort for larger
    // uses stack-allocated temp element
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
