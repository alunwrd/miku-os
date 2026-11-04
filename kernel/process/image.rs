// What a running process was loaded from.
//
// The loader knows the path, the ASLR bias and the image bounds, and `ElfImage`
// is consumed to build the initial register frame. When the process faults,
// only the bias can turn `rip` back into a function name.
//
// So it is kept here, keyed by pid, in the shape `mm::mmap` keeps its VMA
// tables in: one global map behind a Mutex, filled at exec and dropped with the
// address space. Symbols are not cached: the path is enough to read them back
// when a crash report needs them.

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;

use crate::process::elf::BUILD_ID_MAX;

pub struct ImageInfo {
    /// The path exec was given. Used to read the file back for symbols,
    /// and reported as-is in a crash line
    pub path:         String,
    /// Subtract this from a runtime address to get a link-time one
    pub load_bias:    u64,
    /// Where the interpreter landed, and how far it reaches. An address in
    /// this range belongs to ld-miku, not to the program
    pub interp_base:  u64,
    /// Biased bounds of the executable's own PT_LOAD span
    pub exe_lo:       u64,
    pub exe_hi:       u64,
    pub entry:        u64,
    pub build_id:     [u8; BUILD_ID_MAX],
    pub build_id_len: usize,
    pub x86_features: u32,
}

impl ImageInfo {
    pub fn build_id(&self) -> Option<&[u8]> {
        if self.build_id_len == 0 { None } else { Some(&self.build_id[..self.build_id_len]) }
    }

    /// True when a runtime address falls inside the executable image
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.exe_lo && addr < self.exe_hi
    }

    /// Runtime address to link-time address, for a symbol lookup
    pub fn unbias(&self, addr: u64) -> Option<u64> {
        addr.checked_sub(self.load_bias)
    }
}

static IMAGES: Mutex<BTreeMap<u64, ImageInfo>> = Mutex::new(BTreeMap::new());

/// Records what pid was loaded from. Replaces any previous entry, as execve
/// wants
pub fn register(pid: u64, info: ImageInfo) {
    IMAGES.lock().insert(pid, info);
}

pub fn forget(pid: u64) {
    IMAGES.lock().remove(&pid);
}

/// Run `f` against the image record for `pid`, if there is one. The map
/// stays locked for the duration, so `f` must not read files or block
pub fn with<R>(pid: u64, f: impl FnOnce(&ImageInfo) -> R) -> Option<R> {
    let map = IMAGES.lock();
    map.get(&pid).map(f)
}

/// (path, load_bias) for `pid`: what a crash report needs before reading
/// symbols, copied out so the lock is not held across the file read
pub fn path_and_bias(pid: u64) -> Option<(String, u64)> {
    with(pid, |i| (i.path.clone(), i.load_bias))
}

pub fn count() -> usize {
    IMAGES.lock().len()
}
