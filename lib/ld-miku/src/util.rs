use crate::syscall;

pub fn strlen(s: *const u8) -> usize {
    let mut n = 0;
    unsafe { while *s.add(n) != 0 { n += 1; } }
    n
}

pub fn streq(a: *const u8, b: *const u8) -> bool {
    let mut i = 0;
    unsafe {
        loop {
            let ca = *a.add(i);
            let cb = *b.add(i);
            if ca != cb { return false; }
            if ca == 0  { return true; }
            i += 1;
        }
    }
}

pub fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

pub fn memcpy(dst: *mut u8, src: *const u8, n: usize) {
    unsafe { core::ptr::copy_nonoverlapping(src, dst, n); }
}

pub fn memset(dst: *mut u8, val: u8, n: usize) {
    unsafe { core::ptr::write_bytes(dst, val, n); }
}

/// Loader tracing, off unless `LD_DEBUG` is set. About 30 lines per program at
/// 115200 baud with interrupts off would cost 150 ms per command
static mut DEBUG: bool = false;

pub fn set_debug(on: bool) {
    unsafe { DEBUG = on; }
}

#[inline]
pub fn debugging() -> bool {
    unsafe { DEBUG }
}

fn emit(s: &[u8]) {
    syscall::klog(s.as_ptr(), s.len());
}

/// Trace output goes to `klog`, not the console. `note` and `panic` are shown
/// regardless of `LD_DEBUG`
pub fn print(s: &[u8]) {
    if !debugging() { return; }
    emit(s);
}

/// Errors, printed regardless of `LD_DEBUG`
pub fn note(s: &[u8]) {
    emit(s);
}

pub fn noteln(s: &[u8]) {
    emit(s);
    emit(b"\n");
}

pub fn note_usize(v: usize) {
    let (buf, from) = usize_bytes(v);
    emit(&buf[from..]);
}

/// For failures to start the program
fn print_stderr(s: &[u8]) {
    syscall::write(2, s.as_ptr(), s.len());
}

pub fn println(s: &[u8]) {
    print(s);
    print(b"\n");
}

pub fn print_hex(v: u64) {
    let mut buf = [0u8; 18];
    buf[0] = b'0'; buf[1] = b'x';
    let mut n = v;
    for i in (2..18).rev() {
        let d = (n & 0xF) as u8;
        buf[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        n >>= 4;
    }
    print(&buf);
}

// print unsigned decimal number
pub fn print_usize(v: usize) {
    if !debugging() { return; }
    let (buf, from) = usize_bytes(v);
    print(&buf[from..]);
}

/// Digits of a number, right-aligned in a 20-byte buffer, and their start
fn usize_bytes(v: usize) -> ([u8; 20], usize) {
    let mut buf = [0u8; 20];
    if v == 0 {
        buf[19] = b'0';
        return (buf, 19);
    }
    let mut n = v;
    let mut i = 20;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    (buf, i)
}

pub fn panic(msg: &[u8]) -> ! {
    note(b"[ld-miku] fatal: ");
    noteln(msg);
    // Also on the screen
    print_stderr(b"[ld-miku] fatal: ");
    print_stderr(msg);
    print_stderr(b"\n");
    syscall::exit(1);
}

/// Warnings print regardless of `LD_DEBUG`
pub fn warn(msg: &[u8]) {
    note(b"[ld-miku] warn: ");
    noteln(msg);
}

pub fn cstr_to_bytes(ptr: *const u8) -> &'static [u8] {
    let len = strlen(ptr);
    unsafe { core::slice::from_raw_parts(ptr, len) }
}

/// As above, stopping at `max`, for strings in a table of known size (DT_STRSZ)
pub fn cstr_to_bytes_max(ptr: *const u8, max: usize) -> &'static [u8] {
    let mut len = 0usize;
    while len < max && unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    unsafe { core::slice::from_raw_parts(ptr, len) }
}

pub fn page_align_down(v: u64) -> u64 { v & !0xFFF }
pub fn page_align_up(v: u64)   -> u64 { (v + 0xFFF) & !0xFFF }
