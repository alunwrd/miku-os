//! libc_miku: stdio and directories, as C sees them

use super::super::kstat::MikuDirent;

// opaque DIR handle
/// An open directory: a descriptor and an offset, refilling in batches of 32
#[repr(C)]
pub struct DIR {
    pub fd: i64,
    pub entries: [MikuDirent; 32],
    pub count: usize,
    pub pos: usize,
    pub offset: usize,
}

#[repr(C)]
pub struct DivT {
    pub quot: i32,
    pub rem: i32,
}

#[repr(C)]
pub struct LDivT {
    pub quot: i64,
    pub rem: i64,
}

// FILE: an fd with buffered I/O
#[repr(C)]
pub struct FILE {
    pub fd: i64,
    pub flags: u32,
    pub error: i32,
    pub eof: i32,
    pub ungetc_buf: i32,    // -1 = empty
    pub read_buf: *mut u8,
    pub read_cap: usize,
    pub read_pos: usize,
    pub read_filled: usize,
    pub write_buf: *mut u8,
    pub write_cap: usize,
    pub write_pos: usize,
    pub buf_mode: u32,      // _IOFBF, _IOLBF, _IONBF
}

pub type sighandler_t = extern "C" fn(u32);

pub type ForkFn = extern "C" fn();
