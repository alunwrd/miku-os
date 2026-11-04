use crate::sys::*;
use crate::string;

// open flags (matches kernel)
pub const O_READ:      u32 = 0x0001;
pub const O_WRITE:     u32 = 0x0002;
pub const O_RDWR:      u32 = 0x0003;
pub const O_APPEND:    u32 = 0x0004;
pub const O_CREATE:    u32 = 0x0008;
pub const O_EXCLUSIVE: u32 = 0x0010;
pub const O_TRUNCATE:  u32 = 0x0020;
pub const O_DIRECTORY: u32 = 0x0040;

// seek whence
pub const SEEK_SET: u64 = 0;
pub const SEEK_CUR: u64 = 1;
pub const SEEK_END: u64 = 2;

// file kind
pub const KIND_REGULAR:   u8 = 0;
pub const KIND_DIRECTORY: u8 = 1;
pub const KIND_SYMLINK:   u8 = 2;
pub const KIND_CHARDEV:   u8 = 3;
pub const KIND_BLOCKDEV:  u8 = 4;
pub const KIND_PIPE:      u8 = 5;

// stat structure (64 bytes, matches kernel layout)
#[repr(C)]
pub struct MikuStat {
    pub size:      u64,
    pub mode:      u32,
    pub nlinks:    u32,
    pub uid:       u16,
    pub gid:       u16,
    pub kind:      u8,
    pub fs_type:   u8,
    pub dev_major: u8,
    pub dev_minor: u8,
    pub atime:     u64,
    pub mtime:     u64,
    pub ctime:     u64,
    pub inode_id:  u64,
    pub blocks:    u32,
    pub _reserved: [u8; 4],
}

/// A directory entry as `getdents`(111) writes it: 272 bytes, with a 255-byte
/// name and a u64 inode, as `stat`(18) reports it
#[repr(C)]
pub struct MikuDirent {
    pub name:      [u8; 256],
    pub inode_id:  u64,
    pub kind:      u8,
    pub name_len:  u8,
    pub _reserved: [u8; 6],
}

pub const EMPTY_DIRENT: MikuDirent = MikuDirent {
    name: [0; 256], inode_id: 0, kind: 0, name_len: 0, _reserved: [0; 6],
};

//  Basic file operations //

//  open file with flags and mode
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_open(path: *const u8, path_len: usize, flags: u32, mode: u32) -> i64 {
    unsafe { sc4(SYS_OPEN, path as u64, path_len as u64, flags as u64, mode as u64) }
}

// Opens a file by C string (read-only, default mode)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_open_cstr(path: *const u8) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    miku_open(path, len, O_READ, 0)
}

// open file for reading and writing
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_open_rw(path: *const u8) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    miku_open(path, len, O_RDWR, 0)
}

// create file and open for writing
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_create(path: *const u8, mode: u32) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    miku_open(path, len, O_WRITE | O_CREATE | O_TRUNCATE, mode)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_close(fd: i64) -> i64 {
    unsafe { sc1(SYS_CLOSE, fd as u64) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_seek(fd: i64, offset: u64) -> i64 {
    unsafe { sc3(SYS_SEEK, fd as u64, offset, SEEK_SET) }
}

// seek with whence parameter (offset is signed for SEEK_CUR/SEEK_END)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_lseek(fd: i64, offset: i64, whence: u64) -> i64 {
    unsafe { sc3(SYS_SEEK, fd as u64, offset as u64, whence) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fsize(fd: i64) -> i64 {
    unsafe { sc1(SYS_FSIZE, fd as u64) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_read_fd(fd: i64, buf: *mut u8, len: usize) -> i64 {
    if buf.is_null() || len == 0 { return 0; }
    crate::io::miku_read(fd as u64, buf, len)
}

// write to VFS file
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_write_fd(fd: i64, buf: *const u8, len: usize) -> i64 {
    if buf.is_null() || len == 0 { return 0; }
    unsafe { sc3(SYS_WRITE, fd as u64, buf as u64, len as u64) }
}

/// Largest file this call returns. A character device never reports EOF
pub const READ_FILE_MAX: usize = 16 * 1024 * 1024;

/// Assumed size of a file with no size, until it says otherwise
const READ_FILE_CHUNK: usize = 4096;

/// Reads a whole file into one heap buffer, NUL-terminated. `out_size` gets the
/// length without the NUL; the caller frees with `miku_free`.
///
/// `fsize` is a hint, since a procfs file has no size: reads until EOF, growing
/// when the buffer fills
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_read_file(path: *const u8, out_size: *mut usize) -> *mut u8 {
    if path.is_null() { return core::ptr::null_mut(); }

    let fd = miku_open_cstr(path);
    if fd < 0 { return core::ptr::null_mut(); }

    let hint = miku_fsize(fd);
    let mut cap = if hint > 0 { hint as usize } else { READ_FILE_CHUNK };
    if cap > READ_FILE_MAX { cap = READ_FILE_MAX; }

    // One byte over, for the terminator
    let mut buf = crate::heap::miku_malloc(cap + 1);
    if buf.is_null() {
        miku_close(fd);
        return core::ptr::null_mut();
    }

    miku_seek(fd, 0);
    let mut done = 0usize;
    loop {
        if done == cap {
            if cap >= READ_FILE_MAX {
                // At the ceiling with a full buffer. One more byte separates a
                // file ending here from one that never ends
                let mut probe = 0u8;
                if crate::io::miku_read(fd as u64, &mut probe as *mut u8, 1) <= 0 {
                    break;
                }
                // Refused instead of returning part of a file the caller would
                // treat as whole
                crate::heap::miku_free(buf);
                miku_close(fd);
                return core::ptr::null_mut();
            }
            let want = (cap * 2).min(READ_FILE_MAX);
            let grown = crate::heap::miku_realloc(buf, want + 1);
            if grown.is_null() {
                crate::heap::miku_free(buf);
                miku_close(fd);
                return core::ptr::null_mut();
            }
            buf = grown;
            cap = want;
        }

        let n = crate::io::miku_read(fd as u64, unsafe { buf.add(done) }, cap - done);
        if n <= 0 { break; }
        done += n as usize;
    }
    miku_close(fd);

    unsafe { *buf.add(done) = 0; }
    if !out_size.is_null() {
        unsafe { *out_size = done; }
    }
    buf
}

//  Filesystem operations //

//  stat by path
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_stat(path: *const u8, st: *mut MikuStat) -> i64 {
    if path.is_null() || st.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc3(SYS_STAT, path as u64, len as u64, st as u64) }
}

/// Room on a filesystem, by any path on it.
///
/// `buf` is 48 bytes: magic u32, block size u32, then total blocks, free
/// blocks, total inodes, and free inodes as u64, then max name length and flags
/// as u32. `sys_statfs`'s layout, read directly by `df`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_statfs(path: *const u8, buf: *mut u8) -> i64 {
    if path.is_null() || buf.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc3(SYS_STATFS, path as u64, len as u64, buf as u64) }
}

// stat by fd
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fstat(fd: i64, st: *mut MikuStat) -> i64 {
    if st.is_null() { return crate::errno::EINVAL; }
    unsafe { sc2(SYS_FSTAT, fd as u64, st as u64) }
}

// create directory
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_mkdir(path: *const u8, mode: u32) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc3(SYS_MKDIR, path as u64, len as u64, mode as u64) }
}

// remove directory
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_rmdir(path: *const u8) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc2(SYS_RMDIR, path as u64, len as u64) }
}

// remove file
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_unlink(path: *const u8) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc2(SYS_UNLINK, path as u64, len as u64) }
}

// Reads directory entries, returning the count or a negative error
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_readdir(
    path: *const u8,
    entries: *mut MikuDirent,
    max_entries: usize,
) -> i64 {
    if path.is_null() || entries.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc4(SYS_READDIR, path as u64, len as u64, entries as u64, max_entries as u64) }
}

/// Entries of an open directory, from `offset` on. Use this: `miku_readdir`
/// takes a path, resolves it on each call, and has no cursor. Returns the
/// entries written, zero at the end; the caller advances `offset` by that
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getdents(
    fd: i64,
    entries: *mut MikuDirent,
    max_entries: usize,
    offset: usize,
) -> i64 {
    if entries.is_null() || fd < 0 { return crate::errno::EINVAL; }
    unsafe { sc4(SYS_GETDENTS, fd as u64, entries as u64, max_entries as u64, offset as u64) }
}

/// Commands for `miku_fcntl`.
pub const F_DUPFD: u64 = 0;
pub const F_GETFD: u64 = 1;
pub const F_SETFD: u64 = 2;
pub const F_GETFL: u64 = 3;
pub const F_SETFL: u64 = 4;
pub const FD_CLOEXEC: i64 = 1;
pub const O_NONBLOCK_FLAG: i64 = 0x0400;

/// Changes what an open descriptor does.
///
/// `F_SETFD` with `FD_CLOEXEC` keeps a descriptor out of a child after it is
/// open, and `F_SETFL` with `O_NONBLOCK` makes it non-blocking. Socket
/// descriptors are not in the VFS table and answer `-EOPNOTSUPP`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fcntl(fd: i64, cmd: u64, arg: i64) -> i64 {
    if fd < 0 { return crate::errno::EBADF; }
    unsafe { sc3(SYS_FCNTL, fd as u64, cmd, arg as u64) }
}

/// Marks a descriptor non-blocking or blocking. `F_SETFL` replaces the whole
/// mutable set, so this keeps `O_APPEND`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_set_nonblocking(fd: i64, on: bool) -> i64 {
    let cur = miku_fcntl(fd, F_GETFL, 0);
    if cur < 0 { return cur; }
    let want = if on { cur | O_NONBLOCK_FLAG } else { cur & !O_NONBLOCK_FLAG };
    miku_fcntl(fd, F_SETFL, want)
}

/// `stat` that does not follow a symlink, as a recursive delete needs
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_lstat(path: *const u8, st: *mut MikuStat) -> i64 {
    if path.is_null() || st.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc3(SYS_LSTAT, path as u64, len as u64, st as u64) }
}

// rename file or directory
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_rename(old: *const u8, new: *const u8) -> i64 {
    if old.is_null() || new.is_null() { return crate::errno::EINVAL; }
    let old_len = string::miku_strlen(old);
    let new_len = string::miku_strlen(new);
    unsafe { sc4(SYS_RENAME, old as u64, old_len as u64, new as u64, new_len as u64) }
}

// create hard link
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_link(old: *const u8, new: *const u8) -> i64 {
    if old.is_null() || new.is_null() { return crate::errno::EINVAL; }
    let old_len = string::miku_strlen(old);
    let new_len = string::miku_strlen(new);
    unsafe { sc4(SYS_LINK, old as u64, old_len as u64, new as u64, new_len as u64) }
}

// create symbolic link
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_symlink(target: *const u8, linkpath: *const u8) -> i64 {
    if target.is_null() || linkpath.is_null() { return crate::errno::EINVAL; }
    let tlen = string::miku_strlen(target);
    let llen = string::miku_strlen(linkpath);
    unsafe { sc4(SYS_SYMLINK, target as u64, tlen as u64, linkpath as u64, llen as u64) }
}

// Reads a symbolic link, returning the target length or a negative error
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_readlink(
    path: *const u8,
    buf: *mut u8,
    buf_len: usize,
) -> i64 {
    if path.is_null() || buf.is_null() { return crate::errno::EINVAL; }
    let plen = string::miku_strlen(path);
    unsafe { sc4(SYS_READLINK, path as u64, plen as u64, buf as u64, buf_len as u64) }
}

// realpath(path): the path the kernel resolved it to (119). Writes it and a NUL
// into `buf`; returns its length or a negative error (-ERANGE when it does not
// fit)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_realpath(path: *const u8, buf: *mut u8, buf_len: usize) -> i64 {
    if path.is_null() || buf.is_null() { return crate::errno::EINVAL; }
    let plen = string::miku_strlen(path);
    if plen == 0 { return crate::errno::ENOENT; }
    unsafe { sc4(SYS_REALPATH, path as u64, plen as u64, buf as u64, buf_len as u64) }
}

// Path of an open descriptor: the same call with an empty path and the
// descriptor as its base, naming its vnode (ABI 3.2e). The kernel keeps no path
// with a descriptor, so a renamed file answers with its new name
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fd_path(fd: i32, buf: *mut u8, buf_len: usize) -> i64 {
    if fd < 0 { return crate::errno::EBADF; }
    if buf.is_null() { return crate::errno::EINVAL; }
    let word = (fd as u64 + 1) << 32;
    unsafe { sc4(SYS_REALPATH, 0, word, buf as u64, buf_len as u64) }
}

// A file's access and modification times (`utimensat`, 40): `path`, or if null
// the file open on `fd`. `times` is two (sec, nsec) pairs, atime then mtime,
// `nsec` = UTIME_OMIT leaving one alone; null means both now. `flags` takes
// AT_SYMLINK_NOFOLLOW. 0 or a negative error
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_utimensat(path: *const u8, fd: i32, times: *const i64, flags: i32) -> i64 {
    let (p, word) = if path.is_null() {
        if fd < 0 { return crate::errno::EBADF; }
        (0u64, (fd as u64 + 1) << 32)
    } else {
        let plen = string::miku_strlen(path);
        if plen == 0 { return crate::errno::ENOENT; }
        (path as u64, plen as u64)
    };
    unsafe { sc4(SYS_UTIMENSAT, p, word, times as u64, flags as u32 as u64) }
}

// change file mode
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_chmod(path: *const u8, mode: u32) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc3(SYS_CHMOD, path as u64, len as u64, mode as u64) }
}

// Mode of the file open on `fd`: `chmod`(26) with an empty path and the
// descriptor as its base (ABI 3.2e), as `miku_fd_path` does
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fchmod(fd: i32, mode: u32) -> i64 {
    if fd < 0 { return crate::errno::EBADF; }
    let word = (fd as u64 + 1) << 32;
    unsafe { sc3(SYS_CHMOD, 0, word, mode as u64) }
}

// The owner of the file open on `fd`, the same way through `chown`(27)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fchown(fd: i32, uid: u32, gid: u32) -> i64 {
    if fd < 0 { return crate::errno::EBADF; }
    let word = (fd as u64 + 1) << 32;
    unsafe { sc4(SYS_CHOWN, 0, word, uid as u64, gid as u64) }
}

// Changes a file's owner. uid or gid of 0xFFFF leaves it unchanged
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_chown(path: *const u8, uid: u32, gid: u32) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc4(SYS_CHOWN, path as u64, len as u64, uid as u64, gid as u64) }
}

// duplicate file descriptor
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_dup(fd: i64) -> i64 {
    unsafe { sc1(SYS_DUP, fd as u64) }
}

// duplicate fd to specific number
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_dup2(old_fd: i64, new_fd: i64) -> i64 {
    unsafe { sc2(SYS_DUP2, old_fd as u64, new_fd as u64) }
}

// Creates a pipe. fds must point to [i64; 2]. fds[0] = read end, fds[1] = write
// end
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_pipe(fds: *mut i64) -> i64 {
    if fds.is_null() { return crate::errno::EINVAL; }
    unsafe { sc1(SYS_PIPE, fds as u64) }
}

// `pipe` with `O_CLOEXEC` (0x800) and `O_NONBLOCK` (0x400) set on both ends by
// the kernel, before another thread can `fork`: `pipe2`(120)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_pipe2(fds: *mut i64, flags: u32) -> i64 {
    if fds.is_null() { return crate::errno::EINVAL; }
    unsafe { sc2(SYS_PIPE2, fds as u64, flags as u64) }
}

// change current directory
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_chdir(path: *const u8) -> i64 {
    if path.is_null() { return crate::errno::EINVAL; }
    let len = string::miku_strlen(path);
    unsafe { sc2(SYS_CHDIR, path as u64, len as u64) }
}

// check if file exists
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_access(path: *const u8) -> bool {
    if path.is_null() { return false; }
    let mut st = unsafe { core::mem::zeroed::<MikuStat>() };
    miku_stat(path, &mut st) == 0
}

// check if path is a directory
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isdir(path: *const u8) -> bool {
    if path.is_null() { return false; }
    let mut st = unsafe { core::mem::zeroed::<MikuStat>() };
    if miku_stat(path, &mut st) != 0 { return false; }
    st.kind == KIND_DIRECTORY
}

// get file size by path
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_filesize(path: *const u8) -> i64 {
    if path.is_null() { return -1; }
    let mut st = unsafe { core::mem::zeroed::<MikuStat>() };
    if miku_stat(path, &mut st) != 0 { return -1; }
    st.size as i64
}

// truncate file to given length
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_ftruncate(fd: i64, length: u64) -> i64 {
    unsafe { sc2(SYS_TRUNCATE, fd as u64, length) }
}

/// Puts everything written to `fd` on the medium and returns once it is there.
/// An installer writes a new file beside the old, syncs it with this, and
/// renames over it, so the rename cannot reach the disk before the data
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fsync(fd: i64) -> i64 {
    unsafe { sc1(SYS_FSYNC, fd as u64) }
}

/// Reserves `len` bytes of disk from `offset` without writing, so a later write
/// cannot fail for want of space
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fallocate(fd: i64, offset: u64, len: u64) -> i64 {
    unsafe { sc3(SYS_FALLOCATE, fd as u64, offset, len) }
}

// read at offset without changing file position
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_pread(fd: i64, buf: *mut u8, len: usize, offset: i64) -> i64 {
    if buf.is_null() || len == 0 { return 0; }
    // save position, seek, read, restore
    let saved = miku_lseek(fd, 0, SEEK_CUR);
    if saved < 0 { return saved; }
    let r = miku_lseek(fd, offset, SEEK_SET);
    if r < 0 { return r; }
    let n = crate::io::miku_read(fd as u64, buf, len);
    miku_lseek(fd, saved, SEEK_SET);
    n
}

// write at offset without changing file position
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_pwrite(fd: i64, buf: *const u8, len: usize, offset: i64) -> i64 {
    if buf.is_null() || len == 0 { return 0; }
    let saved = miku_lseek(fd, 0, SEEK_CUR);
    if saved < 0 { return saved; }
    let r = miku_lseek(fd, offset, SEEK_SET);
    if r < 0 { return r; }
    let n = miku_write_fd(fd, buf, len);
    miku_lseek(fd, saved, SEEK_SET);
    n
}

// Writes a C string to a file: open, write, close
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_write_file_cstr(path: *const u8, data: *const u8) -> i64 {
    if path.is_null() || data.is_null() { return crate::errno::EINVAL; }
    let fd = miku_create(path, 0o644);
    if fd < 0 { return fd; }
    let len = string::miku_strlen(data);
    let n = miku_write_fd(fd, data, len);
    miku_close(fd);
    n
}

// check if path is a regular file
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isfile(path: *const u8) -> bool {
    if path.is_null() { return false; }
    let mut st = unsafe { core::mem::zeroed::<MikuStat>() };
    if miku_stat(path, &mut st) != 0 { return false; }
    st.kind == KIND_REGULAR
}

// check if path is a symlink
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_issymlink(path: *const u8) -> bool {
    if path.is_null() { return false; }
    let mut st = unsafe { core::mem::zeroed::<MikuStat>() };
    if miku_stat(path, &mut st) != 0 { return false; }
    st.kind == KIND_SYMLINK
}
