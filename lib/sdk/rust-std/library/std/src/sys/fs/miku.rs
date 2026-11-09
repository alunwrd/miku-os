//! `std::fs` on MikuOS.
//!
//! Paths go to the kernel as NUL-terminated bytes, as `libmiku` takes them and
//! the VFS stores them: a name is 255 bytes and need not be UTF-8.
//! `with_native_path` for a platform neither unix nor windows is the identity,
//! so every function here converts a `&Path` once.
//!
//! A directory walk names paths, not descriptors, so it is not race-free.
//! `File::lock` and its kin are unsupported.

use crate::ffi::{OsStr, OsString};
use crate::fmt;
use crate::fs::TryLockError;
use crate::hash::Hash;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::os::miku::ffi::{OsStrExt, OsStringExt};
use crate::path::{Path, PathBuf};
use crate::sync::Arc;
use crate::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd};
use crate::sys::fd::FileDesc;
use crate::sys::pal::{abi, cvt, unsupported};
use crate::sys::time::SystemTime;
use crate::sys::{AsInner, FromInner, IntoInner};

pub use crate::sys::fs::common::{Dir, copy, exists, remove_dir_all};

/// A path as the kernel takes it: bytes, NUL-terminated.
///
/// An embedded NUL is refused: a path that ends early is a different path.
fn cpath(p: &Path) -> io::Result<Vec<u8>> {
    let b = p.as_os_str().as_bytes();
    if b.iter().any(|&c| c == 0) {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "path contains a NUL byte",
        ));
    }
    let mut v = Vec::with_capacity(b.len() + 1);
    v.extend_from_slice(b);
    v.push(0);
    Ok(v)
}

fn stat_at(path: &Path, follow: bool) -> io::Result<FileAttr> {
    let c = cpath(path)?;
    let mut st = abi::MikuStat::default();
    let r = if follow {
        abi::miku_stat(c.as_ptr(), &mut st)
    } else {
        abi::miku_lstat(c.as_ptr(), &mut st)
    };
    cvt(r)?;
    Ok(FileAttr(st))
}

// file attributes

#[derive(Clone)]
pub struct FileAttr(abi::MikuStat);

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.0.size
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions(self.0.mode)
    }

    pub fn file_type(&self) -> FileType {
        FileType(self.0.kind)
    }

    /// The raw `stat` fields, for `std::os::unix::fs::MetadataExt`.
    ///
    /// MikuOS's `stat` is 64 bytes of its own shape, so these are accessors:
    /// `dev` is 0 (no device numbers), `blksize` is the page, and every
    /// `*_nsec` is 0, since the VFS stores whole seconds.
    pub fn ino(&self) -> u64 { self.0.inode_id }
    pub fn mode_raw(&self) -> u32 { self.0.mode }
    pub fn nlink(&self) -> u64 { self.0.nlinks as u64 }
    pub fn uid(&self) -> u32 { self.0.uid as u32 }
    pub fn gid(&self) -> u32 { self.0.gid as u32 }
    pub fn rdev(&self) -> u64 {
        // A device node's major and minor, packed as `makedev` does. Zero for
        // anything else, as on Unix
        ((self.0.dev_major as u64) << 8) | self.0.dev_minor as u64
    }
    pub fn atime_secs(&self) -> i64 { self.0.atime as i64 }
    pub fn mtime_secs(&self) -> i64 { self.0.mtime as i64 }
    pub fn ctime_secs(&self) -> i64 { self.0.ctime as i64 }
    pub fn blocks(&self) -> u64 { self.0.blocks as u64 }

    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(seconds(self.0.mtime))
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        Ok(seconds(self.0.atime))
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        // `ctime` on a Unix is the inode change time, not a creation time, and
        // MikuOS stores the same thing, so there is no creation time to answer
        // with
        unsupported()
    }
}

/// A whole-second timestamp as a `SystemTime`. See `SystemTime::from_secs`.
fn seconds(secs: u64) -> SystemTime {
    SystemTime::from_secs(secs)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePermissions(u32);

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        // No write bit for anybody, the same test and approximation as every
        // Unix `std`
        self.0 & 0o222 == 0
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.0 &= !0o222;
        } else {
            self.0 |= 0o222;
        }
    }

    pub fn mode(&self) -> u32 {
        self.0
    }

    pub fn from_mode(mode: u32) -> FilePermissions {
        FilePermissions(mode)
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
}

impl FileTimes {
    pub fn set_accessed(&mut self, t: SystemTime) {
        self.accessed = Some(t);
    }
    pub fn set_modified(&mut self, t: SystemTime) {
        self.modified = Some(t);
    }

    /// As `utimensat`(40) reads them: (sec, nsec) for atime, then mtime, a time
    /// not set being `UTIME_OMIT`. Whole seconds, as the VFS keeps
    fn to_raw(&self) -> [i64; 4] {
        use crate::sys::pal::abi::defs::stat::UTIME_OMIT;
        let one = |t: Option<SystemTime>| match t {
            Some(t) => [t.as_secs() as i64, 0],
            None => [0, UTIME_OMIT],
        };
        let [a0, a1] = one(self.accessed);
        let [m0, m1] = one(self.modified);
        [a0, a1, m0, m1]
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileType(u8);

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.0 == abi::KIND_DIRECTORY
    }
    pub fn is_file(&self) -> bool {
        self.0 == abi::KIND_REGULAR
    }
    pub fn is_symlink(&self) -> bool {
        self.0 == abi::KIND_SYMLINK
    }

    /// The four kinds `std::os::unix::fs::FileTypeExt` asks about.
    ///
    /// `VNodeKind` in `kernel/fs/vfs/types.rs` runs Regular, Directory,
    /// Symlink, CharDevice, BlockDevice, Pipe, Socket, Fifo. `is_fifo` accepts
    /// both `Pipe` and `Fifo`: a `pipe`(34) is the first, a named one in the
    /// filesystem the second, and `S_ISFIFO` is true of both on a Unix.
    pub fn is_block_device(&self) -> bool { self.0 == abi::KIND_BLOCK_DEVICE }
    pub fn is_char_device(&self) -> bool { self.0 == abi::KIND_CHAR_DEVICE }
    pub fn is_fifo(&self) -> bool {
        self.0 == abi::KIND_PIPE || self.0 == abi::KIND_FIFO
    }
    pub fn is_socket(&self) -> bool { self.0 == abi::KIND_SOCKET }
}

// directories

/// A directory, read in full before the first entry is handed out.
///
/// `getdents`(111) continues from an offset, a position in the directory as it
/// is now (`readdir_at` counts children as it walks them), not a stable cookie
/// like Linux's `d_off`. An iterator reading batch by batch is correct only
/// while nobody changes the directory between calls, and `remove_dir_all`
/// deletes each entry as it walks: continuing "from 32" skips the 32 entries
/// that just moved down, and `remove_dir` then answers `ENOTEMPTY`.
///
/// Reading it all up front leaves no caller code between the calls. A caller
/// sees the directory as it was when it asked, as on other platforms. A stable
/// cookie in the VFS would be the real fix (`dev/std-architecture.md`).
///
/// The cost is memory, so this keeps name bytes and a kind, about
/// `name.len() + 40` bytes an entry. Handing an entry out moves its name, and
/// the directory's path is shared.
pub struct ReadDir {
    root: Arc<PathBuf>,
    entries: crate::vec::IntoIter<DirEntryData>,
}

struct DirEntryData {
    name: Vec<u8>,
    kind: u8,
    ino: u64,
}

/// Entries per syscall while the snapshot is taken. The buffer lives for one
/// `readdir`
const BATCH: usize = 128;

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadDir").field("root", &self.root).finish_non_exhaustive()
    }
}

pub struct DirEntry {
    root: Arc<PathBuf>,
    name: Vec<u8>,
    kind: u8,
    ino: u64,
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        let e = self.entries.next()?;
        Some(Ok(DirEntry { root: Arc::clone(&self.root), name: e.name, kind: e.kind, ino: e.ino }))
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.root.join(self.file_name())
    }

    pub fn file_name(&self) -> OsString {
        OsString::from_vec(self.name.clone())
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        // By path: a second lookup, and a race with anything renaming the entry
        stat_at(&self.path(), true)
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        // From the directory entry itself
        Ok(FileType(self.kind))
    }

    /// The inode number the directory listed, for `DirEntryExt::ino`, saving a
    /// `stat` per name
    pub fn ino(&self) -> u64 {
        self.ino
    }
}

// opening

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    mode: u32,
    custom: u32,
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            mode: 0o666,
            custom: 0,
        }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }
    pub fn write(&mut self, write: bool) {
        self.write = write;
    }
    pub fn append(&mut self, append: bool) {
        self.append = append;
    }
    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }
    pub fn create(&mut self, create: bool) {
        self.create = create;
    }
    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }
    pub fn mode(&mut self, mode: u32) {
        self.mode = mode;
    }

    /// Extra open flags, for `std::os::unix::fs::OpenOptionsExt`.
    ///
    /// Or'd into the word `flags()` builds after the access mode is decided, so
    /// a custom flag can add `O_NOFOLLOW` or `O_CLOEXEC` and cannot change read
    /// into write.
    pub fn custom_flags(&mut self, flags: i32) {
        self.custom = flags as u32 & !(abi::OPEN_READ | abi::OPEN_WRITE);
    }

    /// The same validity table as every `std`: the combinations for which the
    /// flag word alone would be ambiguous
    fn flags(&self) -> io::Result<u32> {
        let mut f = 0u32;
        match (self.read, self.write, self.append) {
            (true, false, false) => f |= abi::OPEN_READ,
            (false, true, false) => f |= abi::OPEN_WRITE,
            (true, true, false) => f |= abi::OPEN_READ | abi::OPEN_WRITE,
            (false, _, true) => f |= abi::OPEN_WRITE | abi::OPEN_APPEND,
            (true, _, true) => f |= abi::OPEN_READ | abi::OPEN_WRITE | abi::OPEN_APPEND,
            (false, false, false) => {
                return Err(io::const_error!(
                    io::ErrorKind::InvalidInput,
                    "invalid access mode: neither read nor write was asked for",
                ));
            }
        }
        match (self.create, self.truncate, self.create_new) {
            (false, false, false) => {}
            (true, false, false) => f |= abi::OPEN_CREATE,
            (false, true, false) | (true, true, false) => {
                if !(self.write || self.append) {
                    return Err(io::const_error!(
                        io::ErrorKind::InvalidInput,
                        "cannot truncate a file opened for reading only",
                    ));
                }
                f |= abi::OPEN_TRUNCATE;
                if self.create {
                    f |= abi::OPEN_CREATE;
                }
            }
            (_, _, true) => f |= abi::OPEN_CREATE | abi::OPEN_EXCLUSIVE,
        }
        // Last, after the access mode is settled; see `custom_flags`
        Ok(f | self.custom)
    }
}

pub struct File(FileDesc);

impl File {
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        let c = cpath(path)?;
        let flags = opts.flags()?;
        // `c.len() - 1`: the kernel's `open` takes a length, and the terminator
        // is not part of the name.
        // Close-on-exec always, as in upstream's `open`: a file this program
        // opened is not a child's business.
        let fd = cvt(abi::miku_open(c.as_ptr(), c.len() - 1, flags | abi::OPEN_CLOEXEC, opts.mode))?;
        // SAFETY: the kernel just handed this over and nobody else owns it
        Ok(File(unsafe { FileDesc::from_raw_fd(fd as crate::os::fd::RawFd) }))
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        let mut st = abi::MikuStat::default();
        cvt(abi::miku_fstat(self.0.as_raw_fd() as i64, &mut st))?;
        Ok(FileAttr(st))
    }

    pub fn fsync(&self) -> io::Result<()> {
        cvt(abi::miku_fsync(self.0.as_raw_fd() as i64))?;
        Ok(())
    }

    pub fn datasync(&self) -> io::Result<()> {
        // No `fdatasync`: `fsync` also covers metadata, and the stronger one is
        // correct
        self.fsync()
    }

    pub fn lock(&self) -> io::Result<()> {
        unsupported()
    }
    pub fn lock_shared(&self) -> io::Result<()> {
        unsupported()
    }
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::Error::UNSUPPORTED_PLATFORM))
    }
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::Error::UNSUPPORTED_PLATFORM))
    }
    pub fn unlock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn truncate(&self, size: u64) -> io::Result<()> {
        cvt(abi::miku_ftruncate(self.0.as_raw_fd() as i64, size))?;
        Ok(())
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    /// `pread`/`pwrite`, which MikuOS does not have.
    ///
    /// Not atomic: this saves the cursor, seeks, reads and seeks back, so two
    /// threads sharing one `File` can interleave inside it and a signal handler
    /// can see the cursor moved. The cursor is restored even when the read
    /// fails, or the next ordinary read would be wrong.
    ///
    /// `FileExt` is what `rustix` and everything above it require.
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let saved = self.tell()?;
        let r = self.seek(SeekFrom::Start(offset)).and_then(|_| self.read(buf));
        self.seek(SeekFrom::Start(saved))?;
        r
    }

    pub fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        let saved = self.tell()?;
        let r = self.seek(SeekFrom::Start(offset)).and_then(|_| self.write(buf));
        self.seek(SeekFrom::Start(saved))?;
        r
    }
    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.0.read_vectored(bufs)
    }
    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        self.0.is_read_vectored()
    }
    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        self.0.read_buf(cursor)
    }
    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }
    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    pub fn flush(&self) -> io::Result<()> {
        // Nothing is buffered on this side of the call
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (whence, off) = match pos {
            SeekFrom::Start(n) => (0u64, n as i64),
            SeekFrom::Current(n) => (1u64, n),
            SeekFrom::End(n) => (2u64, n),
        };
        let n = cvt(abi::miku_lseek(self.0.as_raw_fd() as i64, off, whence))?;
        Ok(n as u64)
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        None
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        self.0.duplicate().map(File)
    }

    pub fn set_permissions(&self, perm: FilePermissions) -> io::Result<()> {
        // `chmod`(26) on the descriptor itself: an empty path with the
        // descriptor as its base (ABI 3.2e)
        cvt(abi::miku_fchmod(self.0.as_raw_fd(), perm.0))?;
        Ok(())
    }

    pub fn set_times(&self, times: FileTimes) -> io::Result<()> {
        let raw = times.to_raw();
        cvt(abi::miku_utimensat(crate::ptr::null(), self.as_raw_fd(), raw.as_ptr(), 0))?;
        Ok(())
    }
}

impl AsInner<FileDesc> for File {
    #[inline]
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl IntoInner<FileDesc> for File {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for File {
    fn from_inner(fd: FileDesc) -> File {
        File(fd)
    }
}

impl crate::os::fd::AsFd for File {
    #[inline]
    fn as_fd(&self) -> crate::os::fd::BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl crate::os::fd::AsRawFd for File {
    #[inline]
    fn as_raw_fd(&self) -> crate::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

impl crate::os::fd::IntoRawFd for File {
    #[inline]
    fn into_raw_fd(self) -> crate::os::fd::RawFd {
        self.0.into_raw_fd()
    }
}

impl crate::os::fd::FromRawFd for File {
    #[inline]
    unsafe fn from_raw_fd(fd: crate::os::fd::RawFd) -> File {
        File(unsafe { FileDesc::from_raw_fd(fd) })
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File").field("fd", &self.0.as_raw_fd()).finish()
    }
}

#[derive(Debug)]
pub struct DirBuilder {
    mode: u32,
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { mode: 0o777 }
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        let c = cpath(p)?;
        cvt(abi::miku_mkdir(c.as_ptr(), self.mode))?;
        Ok(())
    }

    pub fn set_mode(&mut self, mode: u32) {
        self.mode = mode;
    }
}

// the free functions

pub fn readdir(p: &Path) -> io::Result<ReadDir> {
    let c = cpath(p)?;
    let fd = cvt(abi::miku_open(
        c.as_ptr(),
        c.len() - 1,
        abi::OPEN_READ | abi::OPEN_DIRECTORY | abi::OPEN_CLOEXEC,
        0,
    ))?;
    // SAFETY: the kernel just handed this over
    let fd = unsafe { FileDesc::from_raw_fd(fd as crate::os::fd::RawFd) };

    // The whole directory, now, with nothing of the caller's running in
    // between; see `ReadDir`
    let mut batch = vec![abi::MikuDirent::zeroed(); BATCH];
    let mut entries: Vec<DirEntryData> = Vec::new();
    let mut offset = 0usize;
    loop {
        let n = abi::miku_getdents(fd.as_raw_fd() as i64, batch.as_mut_ptr(), BATCH, offset);
        if n < 0 {
            return Err(io::Error::from_raw_os_error(-n as i32));
        }
        if n == 0 {
            break;
        }
        for e in batch[..n as usize].iter() {
            let name = e.name_bytes();
            // `.` and `..` are entries to the kernel, not to `read_dir`
            if name == b"." || name == b".." {
                continue;
            }
            entries.push(DirEntryData {
                name: name.to_vec(),
                kind: e.kind,
                ino: e.inode_id as u64,
            });
        }
        offset += n as usize;
    }
    drop(fd);

    Ok(ReadDir { root: Arc::new(p.to_path_buf()), entries: entries.into_iter() })
}

pub fn unlink(p: &Path) -> io::Result<()> {
    let c = cpath(p)?;
    cvt(abi::miku_unlink(c.as_ptr()))?;
    Ok(())
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    let a = cpath(old)?;
    let b = cpath(new)?;
    cvt(abi::miku_rename(a.as_ptr(), b.as_ptr()))?;
    Ok(())
}

pub fn set_perm(p: &Path, perm: FilePermissions) -> io::Result<()> {
    let c = cpath(p)?;
    cvt(abi::miku_chmod(c.as_ptr(), perm.0))?;
    Ok(())
}

pub fn set_times(p: &Path, times: FileTimes) -> io::Result<()> {
    set_times_with(p, times, 0)
}

pub fn set_times_nofollow(p: &Path, times: FileTimes) -> io::Result<()> {
    use crate::sys::pal::abi::defs::fcntl::AT_SYMLINK_NOFOLLOW;
    set_times_with(p, times, AT_SYMLINK_NOFOLLOW)
}

/// `utimensat`(40), by path
fn set_times_with(p: &Path, times: FileTimes, flags: i32) -> io::Result<()> {
    let c = cpath(p)?;
    let raw = times.to_raw();
    cvt(abi::miku_utimensat(c.as_ptr(), -1, raw.as_ptr(), flags))?;
    Ok(())
}

pub fn rmdir(p: &Path) -> io::Result<()> {
    let c = cpath(p)?;
    cvt(abi::miku_rmdir(c.as_ptr()))?;
    Ok(())
}

pub fn readlink(p: &Path) -> io::Result<PathBuf> {
    let c = cpath(p)?;
    // `SYMLINK_MAX` in the VFS. A longer target cannot exist, so a full buffer
    // is a full answer
    let mut buf = vec![0u8; 4096];
    let n = cvt(abi::miku_readlink(c.as_ptr(), buf.as_mut_ptr(), buf.len()))?;
    buf.truncate(n as usize);
    Ok(PathBuf::from(OsString::from_vec(buf)))
}

pub fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    let a = cpath(original)?;
    let b = cpath(link)?;
    cvt(abi::miku_symlink(a.as_ptr(), b.as_ptr()))?;
    Ok(())
}

pub fn link(src: &Path, dst: &Path) -> io::Result<()> {
    let a = cpath(src)?;
    let b = cpath(dst)?;
    cvt(abi::miku_link(a.as_ptr(), b.as_ptr()))?;
    Ok(())
}

pub fn stat(p: &Path) -> io::Result<FileAttr> {
    stat_at(p, true)
}

pub fn lstat(p: &Path) -> io::Result<FileAttr> {
    stat_at(p, false)
}

pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    // The kernel's own answer (`realpath`, 119): the path resolved the way
    // `open` resolves it, named from the root. A resolver here could disagree
    // with the VFS
    let c = cpath(p)?;
    let mut buf = vec![0u8; 4096];
    let n = cvt(abi::miku_realpath(c.as_ptr(), buf.as_mut_ptr(), buf.len()))?;
    buf.truncate(n as usize);
    Ok(PathBuf::from(OsString::from_vec(buf)))
}
