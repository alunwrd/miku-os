//! `<sys/statfs.h>`, `<sys/statvfs.h>`

use crate::libc::*;

/// `struct statfs` in the Linux shape the `libc` module declares
pub use miku_abi::statvfs::statfs;

// sys/statfs.h, sys/statvfs.h //

pub(crate) use miku_abi::kstat::MikuStatfs;

/// `statfs`(36)'s answer, shared by `statfs`, `fstatfs`, `statvfs`, and
/// `fstatvfs`
pub(crate) struct FsInfo {
    magic: u64,
    bsize: u64,
    blocks: u64,
    bfree: u64,
    files: u64,
    ffree: u64,
    namelen: u64,
    flags: u64,
}

impl FsInfo {
    fn parse(raw: &MikuStatfs) -> Self {
        FsInfo {
            magic: raw.magic as u64,
            bsize: raw.block_size as u64,
            blocks: raw.total_blocks,
            bfree: raw.free_blocks,
            files: raw.total_inodes,
            ffree: raw.free_inodes,
            namelen: raw.max_name_len as u64,
            flags: raw.flags as u64,
        }
    }

    fn statfs(&self) -> statfs {
        statfs {
            f_type:    self.magic as i64,
            f_bsize:   self.bsize as i64,
            f_blocks:  self.blocks,
            f_bfree:   self.bfree,
            // No blocks reserved for root, so all free blocks are available
            f_bavail:  self.bfree,
            f_files:   self.files,
            f_ffree:   self.ffree,
            f_fsid:    unsafe { core::mem::zeroed() },
            f_namelen: self.namelen as i64,
            f_frsize:  self.bsize as i64,
            f_flags:   self.flags as i64,
            f_spare:   [0; 4],
        }
    }

    /// The fields the `libc` module's `statvfs` documents, in its order
    fn statvfs(&self) -> statvfs {
        statvfs {
            f_bsize: self.bsize,
            f_frsize: self.bsize,
            f_blocks: self.blocks,
            f_bfree: self.bfree,
            f_bavail: self.bfree,
            f_files: self.files,
            f_ffree: self.ffree,
            f_favail: self.ffree,
            f_fsid: 0,
            f_flag: 0,
            f_namemax: self.namelen,
            f_type: self.magic,
        }
    }
}

/// POSIX's `statvfs`, in the `libc` module's layout (see `statvfs` there for
/// the four fields with no source)
pub use miku_abi::statvfs::statvfs;

pub(crate) fn fs_info(r: Result<(), i32>, raw: &MikuStatfs) -> Result<FsInfo, i32> {
    r.map(|()| FsInfo::parse(raw))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn statfs(path: *const u8, out: *mut statfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = MikuStatfs::default();
    match fs_info(crate::at::statfs_raw(AT_FDCWD, path, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statfs()) }; 0 }
        Err(e) => fail(e),
    }
}

/// The filesystem the descriptor's file is on, whatever kind of file it is
#[no_mangle]
#[inline(never)]
pub extern "C" fn fstatfs(fd: i32, out: *mut statfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = MikuStatfs::default();
    match fs_info(crate::at::fstatfs_raw(fd, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statfs()) }; 0 }
        Err(e) => fail(e),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn statvfs(path: *const u8, out: *mut statvfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = MikuStatfs::default();
    match fs_info(crate::at::statfs_raw(AT_FDCWD, path, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statvfs()) }; 0 }
        Err(e) => fail(e),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fstatvfs(fd: i32, out: *mut statvfs) -> i32 {
    if out.is_null() { return fail(EFAULT); }
    let mut raw = MikuStatfs::default();
    match fs_info(crate::at::fstatfs_raw(fd, &mut raw), &raw) {
        Ok(fs) => { unsafe { out.write(fs.statvfs()) }; 0 }
        Err(e) => fail(e),
    }
}
