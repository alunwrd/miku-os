//! `<stdio.h>`

use crate::libc::*;

pub use miku_abi::libtypes::FILE;

pub(crate) const FILE_READ:   u32 = 0x01;

pub(crate) const FILE_WRITE:  u32 = 0x02;

pub(crate) const FILE_APPEND: u32 = 0x04;

pub const _IOFBF: u32 = 0; // fully buffered

pub const _IOLBF: u32 = 1; // line buffered

pub const _IONBF: u32 = 2; // unbuffered

pub(crate) const STDIO_BUF_SIZE: usize = 4096;

pub use miku_abi::unistd::SEEK_SET;

pub use miku_abi::unistd::SEEK_CUR;

pub use miku_abi::unistd::SEEK_END;

pub const EOF: i32 = -1;

pub const STDIN_FILENO:  i32 = 0;

pub const STDOUT_FILENO: i32 = 1;

pub const STDERR_FILENO: i32 = 2;

pub const BUFSIZ: usize = 4096;

// pre-allocated FILE objects for stdin/stdout/stderr
pub(crate) static mut STDIN_FILE: FILE = FILE {
    fd: 0, flags: FILE_READ, error: 0, eof: 0, ungetc_buf: -1,
    read_buf: core::ptr::null_mut(), read_cap: 0, read_pos: 0, read_filled: 0,
    write_buf: core::ptr::null_mut(), write_cap: 0, write_pos: 0,
    buf_mode: _IOLBF,
};

pub(crate) static mut STDOUT_FILE: FILE = FILE {
    fd: 1, flags: FILE_WRITE, error: 0, eof: 0, ungetc_buf: -1,
    read_buf: core::ptr::null_mut(), read_cap: 0, read_pos: 0, read_filled: 0,
    write_buf: core::ptr::null_mut(), write_cap: 0, write_pos: 0,
    buf_mode: _IOLBF,
};

pub(crate) static mut STDERR_FILE: FILE = FILE {
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

pub(crate) fn parse_mode(mode: *const u8) -> (u32, u32) {
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

pub(crate) fn file_alloc_bufs(f: *mut FILE, flags: u32) {
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
pub(crate) static OPEN_STREAMS: [core::sync::atomic::AtomicUsize; 64] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 64];

pub(crate) fn stream_register(f: *mut FILE) {
    use core::sync::atomic::Ordering;
    for slot in &OPEN_STREAMS {
        if slot.compare_exchange(0, f as usize, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            return;
        }
    }
}

pub(crate) fn stream_forget(f: *mut FILE) {
    use core::sync::atomic::Ordering;
    for slot in &OPEN_STREAMS {
        let _ = slot.compare_exchange(f as usize, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}

/// `fflush(NULL)`: the standard streams and every registered one
pub(crate) fn flush_all() {
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

pub(crate) fn file_from_fd(fd: i64, flags: u32) -> *mut FILE {
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

pub(crate) fn file_fill_read(fp: &mut FILE) -> i32 {
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

#[no_mangle]
#[inline(never)]
pub extern "C" fn rename(old: *const u8, new: *const u8) -> i32 {
    let r = file::miku_rename(old, new);
    if r < 0 { fail(-r as i32) } else { 0 }
}

// What C programs need beyond the `libc` crate
//
// `lib/sdk/c/include` declares these; `dev/c-architecture.md` has the rule that a
// header declares nothing this library lacks. Larger areas are files under
// `lib/runtime/libmiku/libc/`.

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
