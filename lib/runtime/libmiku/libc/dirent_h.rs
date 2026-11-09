//! `<dirent.h>`

use crate::libc::*;

pub type dirent = file::MikuDirent;

pub use miku_abi::libtypes::DIR;

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
