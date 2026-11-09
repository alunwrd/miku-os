//! fs_miku: buffered IO and directories

use super::super::kstat::MikuDirent;

// buffered reader
#[repr(C)]
pub struct MikuBufReader {
    pub fd: i64,
    pub buf: *mut u8,
    pub cap: usize,
    pub pos: usize,
    pub filled: usize,
}

// buffered writer
#[repr(C)]
pub struct MikuBufWriter {
    pub fd: i64,
    pub buf: *mut u8,
    pub cap: usize,
    pub pos: usize,
    pub mode: u32,
}

// directory iterator state
#[repr(C)]
pub struct MikuDir {
    pub path: [u8; 256],
    pub path_len: usize,
    pub entries: [MikuDirent; DIR_BATCH],
    pub total: usize,   // entries in the batch we are walking
    pub cursor: usize,  // position within that batch
    /// Position in the whole directory: what `getdents` continues from
    pub offset: usize,
    /// The open directory. -1 before `miku_dir_open` succeeds
    pub fd: i64,
    pub done: bool,
}

pub type WalkCallback = extern "C" fn(*const u8, usize, *const MikuDirent, usize, *mut u8) -> i32;

/// Entries fetched per syscall; the iterator returns for more. Small, since
/// `miku_dir_open` returns a `MikuDir` by value
pub const DIR_BATCH: usize = 8;
