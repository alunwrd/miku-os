// Wraps raw file descriptors with read/write buffers
// Reduces syscall overhead for small reads and writes
// Supports line-buffered and fully-buffered modes

use crate::io;
use crate::mem;
use crate::heap;
use crate::string;

const DEFAULT_BUF_SIZE: usize = 4096;

// buffer flush mode
pub const BUFIO_FULL: u32 = 0;
pub const BUFIO_LINE: u32 = 1;
pub const BUFIO_NONE: u32 = 2;

// buffered reader
#[repr(C)]
pub struct MikuBufReader {
    fd: i64,
    buf: *mut u8,
    cap: usize,
    pos: usize,
    filled: usize,
}

// create buffered reader
#[no_mangle]
pub extern "C" fn miku_bufreader_new(fd: i64) -> MikuBufReader {
    miku_bufreader_with_capacity(fd, DEFAULT_BUF_SIZE)
}

// create buffered reader with custom capacity
#[no_mangle]
pub extern "C" fn miku_bufreader_with_capacity(fd: i64, cap: usize) -> MikuBufReader {
    let real_cap = if cap == 0 { DEFAULT_BUF_SIZE } else { cap };
    let buf = heap::miku_malloc(real_cap) as *mut u8;
    MikuBufReader {
        fd,
        buf,
        cap: real_cap,
        pos: 0,
        filled: 0,
    }
}

// free buffered reader
#[no_mangle]
pub extern "C" fn miku_bufreader_free(r: *mut MikuBufReader) {
    if r.is_null() { return; }
    let r = unsafe { &mut *r };
    if !r.buf.is_null() {
        heap::miku_free(r.buf as *mut u8);
        r.buf = core::ptr::null_mut();
    }
}

// read into user buffer
#[no_mangle]
pub extern "C" fn miku_bufreader_read(
    r: *mut MikuBufReader,
    dst: *mut u8,
    len: usize,
) -> i64 {
    if r.is_null() || dst.is_null() || len == 0 { return -1; }
    let r = unsafe { &mut *r };
    let mut written = 0usize;

    while written < len {
        // serve from buffer first
        if r.pos < r.filled {
            let avail = r.filled - r.pos;
            let take = if avail < (len - written) { avail } else { len - written };
            unsafe { mem::miku_memcpy(dst.add(written), r.buf.add(r.pos), take); }
            r.pos += take;
            written += take;
        } else {
            // refill buffer
            if r.buf.is_null() { return written as i64; }
            let n = io::miku_read(r.fd as u64, r.buf, r.cap);
            if n <= 0 {
                return if written > 0 { written as i64 } else { n };
            }
            r.pos = 0;
            r.filled = n as usize;
        }
    }

    written as i64
}

// read single byte, returns -1 on EOF
#[no_mangle]
pub extern "C" fn miku_bufreader_getc(r: *mut MikuBufReader) -> i32 {
    let mut c = 0u8;
    let n = miku_bufreader_read(r, &mut c as *mut u8, 1);
    if n <= 0 { -1 } else { c as i32 }
}

// read line into buffer, returns bytes read
// Reads until newline or buffer full, includes newline if present
#[no_mangle]
pub extern "C" fn miku_bufreader_readline(
    r: *mut MikuBufReader,
    dst: *mut u8,
    max: usize,
) -> i64 {
    if r.is_null() || dst.is_null() || max == 0 { return -1; }
    let mut pos = 0usize;
    while pos < max - 1 {
        let c = miku_bufreader_getc(r);
        if c < 0 {
            break;
        }
        unsafe { *dst.add(pos) = c as u8; }
        pos += 1;
        if c as u8 == b'\n' { break; }
    }
    unsafe { *dst.add(pos) = 0; }
    pos as i64
}

// peek at next byte without consuming
#[no_mangle]
pub extern "C" fn miku_bufreader_peek(r: *mut MikuBufReader) -> i32 {
    if r.is_null() { return -1; }
    let r = unsafe { &mut *r };
    if r.pos < r.filled {
        return unsafe { *r.buf.add(r.pos) } as i32;
    }
    // refill
    if r.buf.is_null() { return -1; }
    let n = io::miku_read(r.fd as u64, r.buf, r.cap);
    if n <= 0 { return -1; }
    r.pos = 0;
    r.filled = n as usize;
    (unsafe { *r.buf.add(0) }) as i32
}

// bytes available in buffer
#[no_mangle]
pub extern "C" fn miku_bufreader_buffered(r: *const MikuBufReader) -> usize {
    if r.is_null() { return 0; }
    let r = unsafe { &*r };
    r.filled - r.pos
}

// buffered writer
#[repr(C)]
pub struct MikuBufWriter {
    fd: i64,
    buf: *mut u8,
    cap: usize,
    pos: usize,
    mode: u32,
}

// create buffered writer
#[no_mangle]
pub extern "C" fn miku_bufwriter_new(fd: i64) -> MikuBufWriter {
    miku_bufwriter_with_capacity(fd, DEFAULT_BUF_SIZE)
}

// create buffered writer with custom capacity
#[no_mangle]
pub extern "C" fn miku_bufwriter_with_capacity(fd: i64, cap: usize) -> MikuBufWriter {
    let real_cap = if cap == 0 { DEFAULT_BUF_SIZE } else { cap };
    let buf = heap::miku_malloc(real_cap) as *mut u8;
    MikuBufWriter {
        fd,
        buf,
        cap: real_cap,
        pos: 0,
        mode: BUFIO_FULL,
    }
}

// set flush mode
#[no_mangle]
pub extern "C" fn miku_bufwriter_set_mode(w: *mut MikuBufWriter, mode: u32) {
    if w.is_null() { return; }
    unsafe { (*w).mode = mode; }
}

// flush buffer to fd
#[no_mangle]
pub extern "C" fn miku_bufwriter_flush(w: *mut MikuBufWriter) -> i64 {
    if w.is_null() { return -1; }
    let w = unsafe { &mut *w };
    if w.pos == 0 || w.buf.is_null() { return 0; }
    let n = io::miku_write(w.fd as u64, w.buf, w.pos);
    if n > 0 {
        let written = n as usize;
        if written < w.pos {
            unsafe { mem::miku_memmove(w.buf, w.buf.add(written), w.pos - written); }
            w.pos -= written;
        } else {
            w.pos = 0;
        }
    }
    n
}

// write bytes
#[no_mangle]
pub extern "C" fn miku_bufwriter_write(
    w: *mut MikuBufWriter,
    src: *const u8,
    len: usize,
) -> i64 {
    if w.is_null() || src.is_null() { return -1; }
    let w = unsafe { &mut *w };
    if w.buf.is_null() { return -1; }

    // unbuffered mode: write directly
    if w.mode == BUFIO_NONE {
        return io::miku_write(w.fd as u64, src, len);
    }

    let mut written = 0usize;
    while written < len {
        let space = w.cap - w.pos;
        let chunk = if (len - written) < space { len - written } else { space };
        unsafe { mem::miku_memcpy(w.buf.add(w.pos), src.add(written), chunk); }
        w.pos += chunk;
        written += chunk;

        // check for line-buffered newline
        if w.mode == BUFIO_LINE {
            let mut has_nl = false;
            for i in (w.pos - chunk)..w.pos {
                if unsafe { *w.buf.add(i) } == b'\n' {
                    has_nl = true;
                    break;
                }
            }
            if has_nl {
                let n = io::miku_write(w.fd as u64, w.buf, w.pos);
                if n > 0 {
                    let written = n as usize;
                    if written < w.pos {
                        unsafe { mem::miku_memmove(w.buf, w.buf.add(written), w.pos - written); }
                        w.pos -= written;
                    } else { w.pos = 0; }
                }
            }
        }

        // flush if buffer full
        if w.pos >= w.cap {
            let n = io::miku_write(w.fd as u64, w.buf, w.pos);
            if n > 0 {
                let written = n as usize;
                if written < w.pos {
                    unsafe { mem::miku_memmove(w.buf, w.buf.add(written), w.pos - written); }
                    w.pos -= written;
                } else { w.pos = 0; }
            }
        }
    }

    written as i64
}

// write single byte
#[no_mangle]
pub extern "C" fn miku_bufwriter_putc(w: *mut MikuBufWriter, c: u8) -> i32 {
    let n = miku_bufwriter_write(w, &c as *const u8, 1);
    if n <= 0 { -1 } else { c as i32 }
}

// write null-terminated string
#[no_mangle]
pub extern "C" fn miku_bufwriter_puts(w: *mut MikuBufWriter, s: *const u8) -> i64 {
    if s.is_null() { return -1; }
    let len = string::miku_strlen(s);
    miku_bufwriter_write(w, s, len)
}

// bytes pending in write buffer
#[no_mangle]
pub extern "C" fn miku_bufwriter_pending(w: *const MikuBufWriter) -> usize {
    if w.is_null() { return 0; }
    unsafe { (*w).pos }
}

// free buffered writer (flushes first)
#[no_mangle]
pub extern "C" fn miku_bufwriter_free(w: *mut MikuBufWriter) {
    if w.is_null() { return; }
    miku_bufwriter_flush(w);
    let w = unsafe { &mut *w };
    if !w.buf.is_null() {
        heap::miku_free(w.buf as *mut u8);
        w.buf = core::ptr::null_mut();
    }
}
