//! A `Root` over the MikuOS syscalls
//!
//! Differs from the host backend in two ways. Ownership is applied, since a
//! process with `CAP_CHOWN` can give a file to root. `free_space` is answered
//! by `statfs`, so a transaction that will not fit is refused before
//! downloading.
//!
//! Every path passed to a syscall is NUL-terminated here. `miku_open` takes a
//! length, the others take C strings.
//!
//! The only module allowed `unsafe`, all of it `extern "C"` calls.

#![allow(unsafe_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::root::{Kind, Root, RootError, RootResult, Stat};

/// `lib/libmiku/file.rs`
const O_READ: u32 = 0x0001;
const O_WRITE: u32 = 0x0002;
const O_CREATE: u32 = 0x0008;
const O_TRUNCATE: u32 = 0x0020;
const O_APPEND: u32 = 0x0004;

/// `kernel/fs/vfs/types.rs` calls these `VNodeKind`
const KIND_REGULAR: u8 = 0;
const KIND_DIR: u8 = 1;
const KIND_SYMLINK: u8 = 2;

/// Longest symlink target (`kernel/fs/vfs/core/links.rs`)
const MAX_LINK: usize = 64;

/// The machine's filesystem, rooted where the caller says. `base` is usually
/// empty; `--root` works on a filesystem other than the booted one
pub struct MikuRoot {
    base: String,
}

impl MikuRoot {
    pub fn new() -> MikuRoot {
        MikuRoot { base: String::new() }
    }

    pub fn under(base: &str) -> MikuRoot {
        MikuRoot { base: base.trim_end_matches('/').to_string() }
    }

    /// The absolute path, NUL-terminated, ready for a syscall
    fn c(&self, path: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.base.len() + path.len() + 2);
        out.extend_from_slice(self.base.as_bytes());
        out.push(b'/');
        out.extend_from_slice(path.as_bytes());
        out.push(0);
        out
    }

    fn plain(&self, path: &str) -> String {
        let mut out = String::with_capacity(self.base.len() + path.len() + 1);
        out.push_str(&self.base);
        out.push('/');
        out.push_str(path);
        out
    }
}

impl Default for MikuRoot {
    fn default() -> MikuRoot {
        MikuRoot::new()
    }
}

/// An errno as an applier error. Numbers are from `kernel/syscall/errno.rs`;
/// unnamed ones keep their number
fn err(path: &str, rc: i64) -> RootError {
    let path = path.to_string();
    match -rc {
        2 => RootError::NotFound { path },
        13 | 1 => RootError::PermissionDenied { path },
        17 => RootError::AlreadyExists { path },
        20 => RootError::NotADirectory { path },
        28 => RootError::NoSpace { path },
        30 => RootError::ReadOnly { path },
        39 => RootError::DirectoryNotEmpty { path },
        _ => RootError::Io { path, code: rc },
    }
}

fn stat_of(path: &[u8]) -> Option<miku::MikuStat> {
    let mut st: miku::MikuStat = unsafe { core::mem::zeroed() };
    let rc = unsafe { miku::miku_stat(path.as_ptr(), &mut st) };
    if rc < 0 {
        None
    } else {
        Some(st)
    }
}

impl Root for MikuRoot {
    fn stat(&self, path: &str) -> Option<Stat> {
        let st = stat_of(&self.c(path))?;
        let kind = match st.kind {
            KIND_DIR => Kind::Directory,
            KIND_SYMLINK => Kind::Symlink,
            KIND_REGULAR => Kind::File,
            // A device node or pipe is not package-owned; calling it a file
            // would let the applier replace it
            _ => return None,
        };
        Some(Stat {
            kind,
            mode: (st.mode & 0o7777) as u16,
            uid: st.uid,
            gid: st.gid,
            size: st.size,
        })
    }

    fn read(&self, path: &str) -> RootResult<Vec<u8>> {
        let full = self.plain(path);
        let fd = unsafe {
            miku::miku_open(full.as_ptr(), full.len(), O_READ, 0)
        };
        if fd < 0 {
            return Err(err(path, fd));
        }
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { miku::miku_read_fd(fd, buf.as_mut_ptr(), buf.len()) };
            if n < 0 {
                unsafe { miku::miku_close(fd) };
                return Err(err(path, n));
            }
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
        }
        unsafe { miku::miku_close(fd) };
        Ok(out)
    }

    fn read_link(&self, path: &str) -> RootResult<String> {
        let c = self.c(path);
        let mut buf = [0u8; MAX_LINK + 1];
        let n = unsafe { miku::miku_readlink(c.as_ptr(), buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            return Err(err(path, n));
        }
        core::str::from_utf8(&buf[..n as usize])
            .map(String::from)
            .map_err(|_| RootError::Io { path: path.to_string(), code: -22 })
    }

    fn make_dir(&mut self, path: &str, mode: u16, uid: u16, gid: u16) -> RootResult<()> {
        let c = self.c(path);
        // `mkdir_p`, since the contract makes parents too, as the host backend
        // does
        let rc = unsafe { miku::miku_mkdir_p(c.as_ptr(), mode as u32) };
        if rc < 0 && stat_of(&c).is_none() {
            return Err(err(path, rc));
        }
        // mkdir_p leaves intermediates at `mode`; the leaf still needs its
        // owner
        let rc = unsafe { miku::miku_chmod(c.as_ptr(), mode as u32) };
        if rc < 0 {
            return Err(err(path, rc));
        }
        own(&c, path, uid, gid)
    }

    fn write_new(
        &mut self,
        path: &str,
        content: &[u8],
        mode: u16,
        uid: u16,
        gid: u16,
    ) -> RootResult<()> {
        let full = self.plain(path);
        let fd = unsafe {
            miku::miku_open(
                full.as_ptr(),
                full.len(),
                O_WRITE | O_CREATE | O_TRUNCATE,
                mode as u32,
            )
        };
        if fd < 0 {
            return Err(err(path, fd));
        }
        let mut at = 0usize;
        while at < content.len() {
            let n = unsafe {
                miku::miku_write_fd(fd, content[at..].as_ptr(), content.len() - at)
            };
            if n <= 0 {
                unsafe { miku::miku_close(fd) };
                return Err(err(path, if n == 0 { -28 } else { n }));
            }
            at += n as usize;
        }
        // Durable before the close and the publishing rename. A rename reaching
        // disk ahead of the bytes leaves an empty file under the right name
        // after a power cut
        let rc = unsafe { miku::miku_fsync(fd) };
        unsafe { miku::miku_close(fd) };
        if rc < 0 {
            return Err(err(path, rc));
        }

        let c = self.c(path);
        // O_CREATE sets the mode only when it creates; a file left over from an
        // interrupted run keeps the old one
        let rc = unsafe { miku::miku_chmod(c.as_ptr(), mode as u32) };
        if rc < 0 {
            return Err(err(path, rc));
        }
        own(&c, path, uid, gid)
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> RootResult<()> {
        let full = self.plain(path);
        let fd = unsafe {
            miku::miku_open(full.as_ptr(), full.len(), O_WRITE | O_CREATE | O_APPEND, 0o600)
        };
        if fd < 0 {
            return Err(err(path, fd));
        }
        let mut at = 0usize;
        while at < bytes.len() {
            let n = unsafe { miku::miku_write_fd(fd, bytes[at..].as_ptr(), bytes.len() - at) };
            if n <= 0 {
                unsafe { miku::miku_close(fd) };
                return Err(err(path, if n == 0 { -28 } else { n }));
            }
            at += n as usize;
        }
        let rc = unsafe { miku::miku_fsync(fd) };
        unsafe { miku::miku_close(fd) };
        if rc < 0 {
            return Err(err(path, rc));
        }
        Ok(())
    }

    fn symlink(&mut self, path: &str, target: &str, uid: u16, gid: u16) -> RootResult<()> {
        let c = self.c(path);
        let mut t = Vec::with_capacity(target.len() + 1);
        t.extend_from_slice(target.as_bytes());
        t.push(0);
        // A leftover from an interrupted run must not stop this one; links are
        // staged under the applier's own name and renamed
        if stat_of(&c).is_some() {
            unsafe { miku::miku_unlink(c.as_ptr()) };
        }
        let rc = unsafe { miku::miku_symlink(t.as_ptr(), c.as_ptr()) };
        if rc < 0 {
            return Err(err(path, rc));
        }
        own(&c, path, uid, gid)
    }

    fn rename(&mut self, from: &str, to: &str) -> RootResult<()> {
        let a = self.c(from);
        let b = self.c(to);
        let rc = unsafe { miku::miku_rename(a.as_ptr(), b.as_ptr()) };
        if rc < 0 {
            return Err(err(from, rc));
        }
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> RootResult<()> {
        let c = self.c(path);
        let rc = unsafe { miku::miku_unlink(c.as_ptr()) };
        if rc < 0 {
            return Err(err(path, rc));
        }
        Ok(())
    }

    fn remove_dir(&mut self, path: &str) -> RootResult<()> {
        let c = self.c(path);
        let rc = unsafe { miku::miku_rmdir(c.as_ptr()) };
        if rc < 0 {
            return Err(err(path, rc));
        }
        Ok(())
    }

    fn free_space(&self) -> Option<u64> {
        let c = self.c("");
        let mut buf = [0u8; 64];
        let rc = unsafe { miku::miku_statfs(c.as_ptr(), buf.as_mut_ptr()) };
        if rc < 0 {
            return None;
        }
        let f = unsafe { &*(buf.as_ptr() as *const miku::MikuStatfs) };
        Some(f.free_blocks.saturating_mul(f.block_size as u64))
    }

    fn sync(&mut self) -> RootResult<()> {
        // Each write is already `fsync`ed before publishing; a whole-filesystem
        // sync would add nothing and stall other processes
        Ok(())
    }
}

/// Gives the file to its owner per the package. The filesystem creates inodes
/// owned by root whoever asked (`lib/extfs/src/ext2/write.rs`)
fn own(c: &[u8], path: &str, uid: u16, gid: u16) -> RootResult<()> {
    let rc = unsafe { miku::miku_chown(c.as_ptr(), uid as u32, gid as u32) };
    if rc < 0 {
        return Err(err(path, rc));
    }
    Ok(())
}
