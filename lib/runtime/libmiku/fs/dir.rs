// Higher-level directory operations on `getdents`
//
// `getdents`(111) takes an open directory and an offset, so the iterator
// refills from where it stopped and ends when the kernel says there is nothing
// more.

use crate::file;
use crate::string;
use crate::mem;

const MAX_WALK_DEPTH: usize = 16;
use miku_abi::libtypes::DIR_BATCH;

pub use miku_abi::libtypes::MikuDir;

// open directory for iteration
#[no_mangle]
pub extern "C" fn miku_dir_open(dir_path: *const u8, path_len: usize) -> MikuDir {
    let mut d = MikuDir {
        path: [0u8; 256],
        path_len: 0,
        entries: [file::EMPTY_DIRENT; DIR_BATCH],
        total: 0,
        cursor: 0,
        offset: 0,
        fd: -1,
        done: false,
    };

    if dir_path.is_null() || path_len == 0 {
        d.done = true;
        return d;
    }

    let copy = if path_len > 255 { 255 } else { path_len };
    unsafe { mem::miku_memcpy(d.path.as_mut_ptr(), dir_path, copy); }
    d.path[copy] = 0;
    d.path_len = copy;

    // O_READ | O_DIRECTORY. The path resolves once here, so a directory renamed
    // mid-walk is still the one walked
    let fd = unsafe {
        crate::sys::sc4(crate::sys::SYS_OPEN, d.path.as_ptr() as u64, copy as u64, 0x0001 | 0x0040, 0)
    };
    if fd < 0 {
        d.done = true;
        return d;
    }
    d.fd = fd;

    if !refill(&mut d) {
        d.done = true;
    }
    d
}

/// Fetches the next batch from where the last stopped. False when exhausted.
/// The offset advances by what was returned; the kernel counts logical entries,
/// so only a zero batch is an end
fn refill(d: &mut MikuDir) -> bool {
    if d.fd < 0 { return false; }
    let n = unsafe {
        crate::sys::sc4(
            crate::sys::SYS_GETDENTS,
            d.fd as u64,
            d.entries.as_mut_ptr() as u64,
            DIR_BATCH as u64,
            d.offset as u64,
        )
    };
    if n <= 0 {
        d.total = 0;
        d.cursor = 0;
        return false;
    }
    d.total = n as usize;
    d.cursor = 0;
    d.offset += n as usize;
    true
}

// close directory (reset state)
#[no_mangle]
pub extern "C" fn miku_dir_close(d: *mut MikuDir) {
    if d.is_null() { return; }
    let d = unsafe { &mut *d };
    // A tree walk leaking one descriptor per directory would exhaust its 128
    if d.fd >= 0 {
        unsafe { crate::sys::sc1(crate::sys::SYS_CLOSE, d.fd as u64) };
        d.fd = -1;
    }
    d.done = true;
    d.cursor = 0;
    d.total = 0;
}

// Reads the next directory entry; false when done
#[no_mangle]
pub extern "C" fn miku_dir_next(d: *mut MikuDir, ent: *mut file::MikuDirent) -> bool {
    if d.is_null() || ent.is_null() { return false; }
    let d = unsafe { &mut *d };
    if d.done { return false; }

    if d.cursor >= d.total && !refill(d) {
        // The real end, as the kernel reports it
        miku_dir_close(d);
        return false;
    }

    unsafe {
        mem::miku_memcpy(
            ent as *mut u8,
            &d.entries[d.cursor] as *const file::MikuDirent as *const u8,
            core::mem::size_of::<file::MikuDirent>(),
        );
    }
    d.cursor += 1;
    true
}

// check if directory has more entries
#[no_mangle]
pub extern "C" fn miku_dir_is_open(d: *const MikuDir) -> bool {
    if d.is_null() { return false; }
    let d = unsafe { &*d };
    !d.done
}

// count entries in directory
#[no_mangle]
pub extern "C" fn miku_dir_count(dir_path: *const u8, path_len: usize) -> i64 {
    let mut d = miku_dir_open(dir_path, path_len);
    if d.done { return -1; }
    let mut count = 0i64;
    let mut ent = file::EMPTY_DIRENT;
    while miku_dir_next(&mut d, &mut ent) {
        count += 1;
    }
    count
}

// Directory walk callback. Args: (path_ptr, path_len, dirent_ptr, depth,
// context). Return: 0 = continue, nonzero = stop
use miku_abi::libtypes::WalkCallback;

// recursive directory walk
#[no_mangle]
pub extern "C" fn miku_dir_walk(
    dir_path: *const u8,
    path_len: usize,
    cb: WalkCallback,
    ctx: *mut u8,
) -> i32 {
    walk_recursive(dir_path, path_len, cb, ctx, 0)
}

fn walk_recursive(
    dir_path: *const u8,
    path_len: usize,
    cb: WalkCallback,
    ctx: *mut u8,
    depth: usize,
) -> i32 {
    if depth >= MAX_WALK_DEPTH { return 0; }

    let mut d = miku_dir_open(dir_path, path_len);
    if d.done { return -1; }

    let mut ent = file::EMPTY_DIRENT;
    let mut child_path = [0u8; 256];

    while miku_dir_next(&mut d, &mut ent) {
        // skip . and ..
        let name_ptr = ent.name.as_ptr();
        if unsafe { *name_ptr } == b'.' {
            let second = unsafe { *name_ptr.add(1) };
            if second == 0 { continue; }
            if second == b'.' && unsafe { *name_ptr.add(2) } == 0 { continue; }
        }

        // build full path
        let name_len = ent.name_len as usize;
        let child_len = build_child_path(
            &mut child_path,
            dir_path,
            path_len,
            name_ptr,
            name_len,
        );

        // call user callback
        let ret = cb(child_path.as_ptr(), child_len, &ent, depth, ctx);
        if ret != 0 {
            return ret;
        }

        // recurse into directories
        if ent.kind == file::KIND_DIRECTORY {
            let sub = walk_recursive(child_path.as_ptr(), child_len, cb, ctx, depth + 1);
            if sub != 0 {
                return sub;
            }
        }
    }

    0
}

fn build_child_path(
    out: &mut [u8; 256],
    parent: *const u8,
    parent_len: usize,
    name: *const u8,
    name_len: usize,
) -> usize {
    let mut pos = 0usize;
    let copy_parent = if parent_len > 240 { 240 } else { parent_len };
    unsafe { mem::miku_memcpy(out.as_mut_ptr(), parent, copy_parent); }
    pos = copy_parent;

    // add separator
    if pos > 0 && out[pos - 1] != b'/' {
        out[pos] = b'/';
        pos += 1;
    }

    let copy_name = if name_len > (255 - pos) { 255 - pos } else { name_len };
    unsafe { mem::miku_memcpy(out.as_mut_ptr().add(pos), name, copy_name); }
    pos += copy_name;
    out[pos] = 0;
    pos
}

// check if path is a directory
#[no_mangle]
pub extern "C" fn miku_is_directory(path: *const u8) -> bool {
    file::miku_isdir(path)
}

// Creates a directory and its parents (mkdir -p). path must be null-terminated
#[no_mangle]
pub extern "C" fn miku_mkdir_p(path: *const u8, mode: u32) -> i64 {
    if path.is_null() { return -1; }
    let path_len = string::miku_strlen(path);
    if path_len == 0 { return -1; }

    let mut tmp = [0u8; 256];
    let copy = if path_len > 255 { 255 } else { path_len };
    unsafe { mem::miku_memcpy(tmp.as_mut_ptr(), path, copy); }
    tmp[copy] = 0;

    // try creating each path component
    let mut i = 1usize; // skip leading /
    while i < copy {
        if tmp[i] == b'/' {
            tmp[i] = 0;
            let _ = file::miku_mkdir(tmp.as_ptr(), mode);
            tmp[i] = b'/';
        }
        i += 1;
    }

    // create final directory
    file::miku_mkdir(tmp.as_ptr(), mode)
}
