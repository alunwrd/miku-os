//! MikuOS-specific extensions to primitives in the [`std::fs`] module.
//!
//! [`std::fs`]: crate::fs
//!
//! `symlink` is here for the reason it is in `std::os::unix::fs` on a Unix:
//! Windows has two kinds of link, so the portable API has none.

#![stable(feature = "rust1", since = "1.0.0")]
// Stable: these items are reached through `std::os::unix::{ffi,fs,process}`,
// which is stable on every Unix, and crates on stable Rust (`rustix`, and
// through it `polling`, `async-io`, `smol`) cannot name an unstable item.
// MikuOS-only additions carry `#[unstable]` themselves.

use crate::io;
use crate::path::Path;
use crate::sys::fs as imp;

/// Create a symbolic link at `link` pointing at `original`.
///
/// The target is stored as given and resolved when the link is used, so a
/// relative target is relative to the link's directory.
#[stable(feature = "rust1", since = "1.0.0")]
pub fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    imp::symlink(original.as_ref(), link.as_ref())
}

/// A file's mode bits, as the VFS stores them.
///
/// The nine permission bits and the two directory ones, with no setuid or
/// setgid: `exec` reads a file's mode, `ls` renders it, and the loader ignores
/// those two. Privilege is granted by a broker to a child, see
/// `docs/MikuOS_Security.md`. A mode with them set means nothing.
#[stable(feature = "rust1", since = "1.0.0")]
pub trait PermissionsExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mode(&self) -> u32;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn set_mode(&mut self, mode: u32);
    #[stable(feature = "rust1", since = "1.0.0")]
    fn from_mode(mode: u32) -> Self;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl PermissionsExt for crate::fs::Permissions {
    fn mode(&self) -> u32 {
        use crate::sys::AsInner;
        self.as_inner().mode()
    }

    fn set_mode(&mut self, mode: u32) {
        use crate::sys::FromInner;
        *self = crate::fs::Permissions::from_inner(imp::FilePermissions::from_mode(mode));
    }

    fn from_mode(mode: u32) -> Self {
        use crate::sys::FromInner;
        crate::fs::Permissions::from_inner(imp::FilePermissions::from_mode(mode))
    }
}

// The Unix extension traits
//
// `rustix` reaches for `std::os::unix::fs` since MikuOS declares
// `target_family = "unix"`, and through it `polling`, `async-io` and `smol`,
// Zed's executor.
//
// Where the system has no such concept, the doc comment says so and the value
// is what a Unix reports for "none".

use crate::fs::{self, DirEntry, File, Metadata, OpenOptions};
use crate::sys::AsInner;
use crate::sys::AsInnerMut;

/// Read and write at an offset, without the descriptor's cursor.
///
/// Not atomic on MikuOS: there is no `pread`/`pwrite` syscall, so the
/// implementation saves the cursor, seeks, transfers and seeks back
/// (`sys::fs::miku::File::read_at`). Two threads sharing one `File` can
/// interleave inside a single call.
#[stable(feature = "rust1", since = "1.0.0")]
pub trait FileExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize>;

    #[stable(feature = "rust1", since = "1.0.0")]
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        while !buf.is_empty() {
            match self.read_at(buf, offset) {
                Ok(0) => break,
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if buf.is_empty() {
            Ok(())
        } else {
            Err(io::const_error!(
                io::ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ))
        }
    }

    #[stable(feature = "rust1", since = "1.0.0")]
    fn write_all_at(&self, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
        while !buf.is_empty() {
            match self.write_at(buf, offset) {
                Ok(0) => {
                    return Err(io::const_error!(
                        io::ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ));
                }
                Ok(n) => {
                    buf = &buf[n..];
                    offset += n as u64;
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
impl FileExt for File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.as_inner().read_at(buf, offset)
    }
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.as_inner().write_at(buf, offset)
    }
}

/// Extra flags for `open`, or'd in after the access mode is decided.
///
/// `custom_flags` masks `O_READ` and `O_WRITE` out, as every libc does: a file
/// opened read-only stays so.
#[stable(feature = "rust1", since = "1.0.0")]
pub trait OpenOptionsExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mode(&mut self, mode: u32) -> &mut Self;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn custom_flags(&mut self, flags: i32) -> &mut Self;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl OpenOptionsExt for OpenOptions {
    fn mode(&mut self, mode: u32) -> &mut OpenOptions {
        self.as_inner_mut().mode(mode);
        self
    }
    fn custom_flags(&mut self, flags: i32) -> &mut OpenOptions {
        self.as_inner_mut().custom_flags(flags);
        self
    }
}

/// The `stat` fields, under Unix names.
///
/// MikuOS's `stat` is not a `libc::stat`, so this is a translation, and three
/// answers are constants:
///
///   * `dev` is 0: one filesystem namespace and no device numbers. An inode
///     number is unique only within a filesystem.
///   * `blksize` is the page, 4096, what the VFS moves in.
///   * every `*_nsec` is 0: the VFS stores whole seconds.
#[stable(feature = "rust1", since = "1.0.0")]
pub trait MetadataExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn dev(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn ino(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mode(&self) -> u32;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn nlink(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn uid(&self) -> u32;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn gid(&self) -> u32;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn rdev(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn size(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn atime(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn atime_nsec(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mtime(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn mtime_nsec(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn ctime(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn ctime_nsec(&self) -> i64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn blksize(&self) -> u64;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn blocks(&self) -> u64;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl MetadataExt for Metadata {
    fn dev(&self) -> u64 { 0 }
    fn ino(&self) -> u64 { self.as_inner().ino() }
    fn mode(&self) -> u32 { self.as_inner().mode_raw() }
    fn nlink(&self) -> u64 { self.as_inner().nlink() }
    fn uid(&self) -> u32 { self.as_inner().uid() }
    fn gid(&self) -> u32 { self.as_inner().gid() }
    fn rdev(&self) -> u64 { self.as_inner().rdev() }
    fn size(&self) -> u64 { self.as_inner().size() }
    fn atime(&self) -> i64 { self.as_inner().atime_secs() }
    fn atime_nsec(&self) -> i64 { 0 }
    fn mtime(&self) -> i64 { self.as_inner().mtime_secs() }
    fn mtime_nsec(&self) -> i64 { 0 }
    fn ctime(&self) -> i64 { self.as_inner().ctime_secs() }
    fn ctime_nsec(&self) -> i64 { 0 }
    fn blksize(&self) -> u64 { 4096 }
    fn blocks(&self) -> u64 { self.as_inner().blocks() }
}

/// The kinds beyond file, directory and symlink. `VNodeKind` in
/// `kernel/fs/vfs/types.rs` has char and block devices, pipes, sockets and
/// named fifos
#[stable(feature = "rust1", since = "1.0.0")]
pub trait FileTypeExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn is_block_device(&self) -> bool;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn is_char_device(&self) -> bool;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn is_fifo(&self) -> bool;
    #[stable(feature = "rust1", since = "1.0.0")]
    fn is_socket(&self) -> bool;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl FileTypeExt for fs::FileType {
    fn is_block_device(&self) -> bool { self.as_inner().is_block_device() }
    fn is_char_device(&self) -> bool { self.as_inner().is_char_device() }
    fn is_fifo(&self) -> bool { self.as_inner().is_fifo() }
    fn is_socket(&self) -> bool { self.as_inner().is_socket() }
}

/// The inode number a directory listing already carried, from `getdents`(111),
/// saving a `stat` per name
#[stable(feature = "rust1", since = "1.0.0")]
pub trait DirEntryExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn ino(&self) -> u64;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl DirEntryExt for DirEntry {
    fn ino(&self) -> u64 {
        self.as_inner().ino()
    }
}

/// A directory's permission bits at creation. `mkdir`(20) takes a mode, and the
/// kernel applies `umask`
#[stable(feature = "dir_builder", since = "1.6.0")]
pub trait DirBuilderExt {
    #[stable(feature = "dir_builder", since = "1.6.0")]
    fn mode(&mut self, mode: u32) -> &mut Self;
}

#[stable(feature = "dir_builder", since = "1.6.0")]
impl DirBuilderExt for fs::DirBuilder {
    fn mode(&mut self, mode: u32) -> &mut fs::DirBuilder {
        self.as_inner_mut().set_mode(mode);
        self
    }
}
