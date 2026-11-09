//! Files, standard streams, the kernel log

use crate::*;

/// Directory names with kinds, without `.` and `..`, following nothing. Empty
/// if the directory will not open
pub fn read_dir(path: &str) -> alloc::vec::Vec<(alloc::string::String, u8)> {
    let mut c = alloc::vec::Vec::with_capacity(path.len() + 1);
    c.extend_from_slice(path.as_bytes());
    c.push(0);
    let mut out = alloc::vec::Vec::new();
    const BATCH: usize = 64;
    // O_READ | O_DIRECTORY
    let fd = unsafe { miku_open(c.as_ptr(), c.len() - 1, 0x0001 | 0x0040, 0) };
    if fd < 0 {
        return out;
    }
    let mut entries: [MikuDirent; BATCH] = unsafe { core::mem::zeroed() };
    let mut offset = 0usize;
    loop {
        let n = unsafe { miku_getdents(fd, entries.as_mut_ptr(), BATCH, offset) };
        if n <= 0 {
            break;
        }
        for e in entries.iter().take(n as usize) {
            let len = (e.name_len as usize).min(e.name.len());
            if let Ok(name) = core::str::from_utf8(&e.name[..len]) {
                if name != "." && name != ".." {
                    out.push((alloc::string::String::from(name), e.kind));
                }
            }
        }
        offset += n as usize;
    }
    unsafe { miku_close(fd) };
    out
}

pub use miku_abi::posix::pollfd as MikuPollFd;

pub use miku_abi::poll::POLLIN;
pub use miku_abi::poll::POLLOUT;
pub use miku_abi::poll::POLLERR;
/// The far end is gone; reported whether requested or not
pub use miku_abi::poll::POLLHUP;
/// Nothing is open on that descriptor
pub use miku_abi::poll::POLLNVAL;

pub fn print(s: &str) {
    unsafe {
        miku_write(1, s.as_ptr(), s.len());
    }
}
pub fn println(s: &str) {
    print(s);
    print("\n");
}
pub fn eprint(s: &str) {
    unsafe {
        miku_write(2, s.as_ptr(), s.len());
    }
}
pub fn eprintln(s: &str) {
    eprint(s);
    eprint("\n");
}

/// Left-aligned decimal. Returns bytes written, or 0 if it does not fit
pub fn fmt_dec(buf: &mut [u8], v: u64) -> usize {
    let mut tmp = [0u8; 20];
    let mut i = 0;
    let mut n = v;
    if n == 0 {
        tmp[0] = b'0';
        i = 1;
    }
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    if i > buf.len() {
        return 0;
    }
    for j in 0..i {
        buf[j] = tmp[i - 1 - j];
    }
    i
}

/// The same with a sign
pub fn fmt_int(buf: &mut [u8], v: i64) -> usize {
    if v >= 0 {
        return fmt_dec(buf, v as u64);
    }
    if buf.is_empty() {
        return 0;
    }
    buf[0] = b'-';
    let n = fmt_dec(&mut buf[1..], (v as u64).wrapping_neg());
    if n == 0 { 0 } else { n + 1 }
}

fn write_dec_to_stdout(val: i64) {
    let mut buf = [0u8; 24];
    let n = fmt_int(&mut buf, val);
    unsafe {
        miku_write(1, buf.as_ptr(), n);
    }
}

fn write_hex_to_stdout(mut val: u64) {
    let mut buf = [0u8; 18];
    let mut i = buf.len();
    if val == 0 {
        i -= 1;
        buf[i] = b'0';
    } else {
        while val > 0 {
            let d = (val & 0xF) as u8;
            i -= 1;
            buf[i] = if d < 10 { b'0' + d } else { b'a' + (d - 10) };
            val >>= 4;
        }
        i -= 2;
        buf[i] = b'0';
        buf[i + 1] = b'x';
    }
    unsafe {
        miku_write(1, buf.as_ptr().add(i), buf.len() - i);
    }
}

pub fn print_int(val: i64) {
    write_dec_to_stdout(val);
}
pub fn print_hex(val: u64) {
    write_hex_to_stdout(val);
}
pub fn putchar(c: u8) {
    unsafe {
        miku_putchar(c as i32);
    }
}
pub fn getchar() -> Option<u8> {
    let r = unsafe { miku_getchar() };
    if r < 0 {
        None
    } else {
        Some(r as u8)
    }
}
pub fn open(path: &str) -> Result<i64, i64> {
    let fd = unsafe { miku_open(path.as_ptr(), path.len(), 0x0001, 0) };
    if fd < 0 {
        Err(fd)
    } else {
        Ok(fd)
    }
}
pub fn close(fd: i64) {
    unsafe {
        miku_close(fd);
    }
}
pub fn fsize(fd: i64) -> i64 {
    unsafe { miku_fsize(fd) }
}
pub fn seek(fd: i64, offset: u64) {
    unsafe {
        miku_seek(fd, offset);
    }
}
pub fn read(fd: i64, buf: &mut [u8]) -> i64 {
    unsafe { miku_read(fd as u64, buf.as_mut_ptr(), buf.len()) }
}

pub fn read_file(path: &str) -> Option<(*mut u8, usize)> {
    let mut size: usize = 0;
    let mut p = [0u8; 256];
    let len = path.len().min(255);
    p[..len].copy_from_slice(&path.as_bytes()[..len]);
    p[len] = 0;
    let ptr = unsafe { miku_read_file(p.as_ptr(), &mut size as *mut usize) };
    if ptr.is_null() {
        return None;
    }
    if size == 0 {
        // Frees the buffer of an empty file
        unsafe { miku_free(ptr) };
        return None;
    }
    Some((ptr, size))
}

pub fn write(fd: u64, data: &[u8]) -> i64 {
    unsafe { miku_write(fd, data.as_ptr(), data.len()) }
}

/// Writes a line to the kernel log, adding the terminator if missing
pub fn klog(line: &str) {
    if line.ends_with('\n') {
        unsafe { miku_klog(line.as_ptr(), line.len()) };
        return;
    }
    // One call with the terminator, so log lines do not interleave
    const INLINE: usize = 256;
    if line.len() < INLINE {
        let mut buf = [0u8; INLINE];
        buf[..line.len()].copy_from_slice(line.as_bytes());
        buf[line.len()] = b'\n';
        unsafe { miku_klog(buf.as_ptr(), line.len() + 1) };
        return;
    }
    // Longer than the buffer: one allocation
    let mut owned = alloc::string::String::with_capacity(line.len() + 1);
    owned.push_str(line);
    owned.push('\n');
    unsafe { miku_klog(owned.as_ptr(), owned.len()) };
}

/// Reads a whole file as text, or None if unreadable or not UTF-8. The size is
/// a hint, since /proc files report zero
pub fn read_file_string(path: &str) -> Option<alloc::string::String> {
    let (ptr, len) = read_file(path)?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    let out = core::str::from_utf8(bytes).ok().map(alloc::string::String::from);
    unsafe { miku_free(ptr) };
    out
}

/// Replaces a file's contents, truncating. True on success
pub fn write_file(path: &str, data: &[u8]) -> bool {
    const O_WRITE: u32 = miku_abi::fcntl::O_WRONLY as u32;
    const O_CREATE: u32 = miku_abi::fcntl::O_CREAT as u32;
    const O_TRUNC: u32 = miku_abi::fcntl::O_TRUNC as u32;
    let mut p = alloc::vec::Vec::with_capacity(path.len() + 1);
    p.extend_from_slice(path.as_bytes());
    p.push(0);
    let fd = unsafe { miku_open(p.as_ptr(), path.len(), O_WRITE | O_CREATE | O_TRUNC, 0o600) };
    if fd < 0 {
        return false;
    }
    let mut off = 0usize;
    let mut ok = true;
    while off < data.len() {
        let n = unsafe { miku_write(fd as u64, data[off..].as_ptr(), data.len() - off) };
        if n <= 0 {
            ok = false;
            break;
        }
        off += n as usize;
    }
    unsafe { miku_close(fd) };
    ok
}
