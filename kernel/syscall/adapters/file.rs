// File-descriptor syscalls: open, close, seek, fsize, dup/dup2, truncate,
// pipe, fallocate, fsync, punch_hole

use crate::syscall::errno::{err, vfs_err, EAGAIN, EBADF, EFAULT, EINTR, EINVAL, EMFILE, ENOENT, ENOMEM, ENOSPC, ENOSYS, EOPNOTSUPP, EIO};
use crate::syscall::usercopy::{current_cr3, read_user_path_at, user_ptr_mapped, user_ptr_writable, PathBase};
use crate::fs::vfs::types::{FileMode, OpenFlags, SeekFrom, VfsError, VNodeKind};

// Try opening a path that lives on the active ext-family filesystem,
// importing its inode metadata into the VFS
fn open_from_active_ext(path: &str, flags: OpenFlags, _mode: FileMode) -> Option<u64> {
    let info = crate::fs::ext::mount::with_ext2_pub(|fs| -> Result<_, VfsError> {
        let ino = fs.resolve_path(path).map_err(|_| VfsError::NotFound)?;
        let inode = fs.read_inode(ino).map_err(|_| VfsError::IoError)?;
        let kind = if inode.is_directory() { VNodeKind::Directory }
                   else if inode.is_symlink() { VNodeKind::Symlink }
                   else { VNodeKind::Regular };
        Ok((ino, kind, inode.permissions(), inode.uid(), inode.gid(), inode.size()))
    })?;

    let (ino, kind, inode_mode, uid, gid, size) = match info {
        Ok(info) => info,
        Err(_)   => return None,
    };

    // The mode bits on the disk, asked before the vnode exists
    //
    // This path creates its own vnode, so there is nothing for `check_access`
    // to ask about until after the decision. `open` reaches here when the VFS
    // has no such file. The rule is `vfs::core::access::check_attrs`, which is
    // also what `check_access` is
    let cred = crate::sched::current_cred();
    if crate::fs::vfs::core::access::check_attrs(
        &cred, FileMode::new(inode_mode), uid, gid, flags,
    ).is_err() {
        return Some(vfs_err(VfsError::PermissionDenied));
    }

    if flags.has(OpenFlags::CREATE) && flags.has(OpenFlags::EXCLUSIVE) {
        return Some(vfs_err(VfsError::AlreadyExists));
    }
    if flags.has(OpenFlags::DIRECTORY) && kind != VNodeKind::Directory {
        return Some(vfs_err(VfsError::NotDirectory));
    }

    Some(crate::fs::vfs::core::with_vfs(|vfs| {
        let id = match vfs.alloc_vnode() {
            Ok(id) => id,
            Err(e) => return vfs_err(e),
        };

        let ts = crate::fs::procfs::uptime_ticks();
        let name = path.rsplit('/').next().unwrap_or(path);
        let slot = crate::fs::ext::mount::active_slot_index() as u8;
        vfs.nodes[id].init(
            id as crate::fs::vfs::InodeId,
            0,
            name,
            kind,
            crate::fs::ext::mount::fs_type_of(slot)
                .unwrap_or(crate::fs::vfs::types::FsType::Ext2),
            FileMode::new(inode_mode),
            uid, gid, ts,
        );
        vfs.nodes[id].ext2_ino = ino;
        vfs.nodes[id].ext2_slot = slot;
        vfs.nodes[id].size = size;
        vfs.nodes[id].nlinks = 0;
        vfs.nodes[id].children_loaded = false;

        if flags.has(OpenFlags::TRUNCATE) && flags.writable() && kind == VNodeKind::Regular {
            let truncate = crate::fs::ext::mount::with_ext2_pub(|fs| fs.ext3_truncate(ino));
            if !matches!(truncate, Some(Ok(()))) {
                vfs.nodes[id].active = false;
                return err(EIO);
            }
            vfs.nodes[id].size = 0;
        }

        let fd = match vfs.fds().alloc(id as crate::fs::vfs::InodeId, flags) {
            Ok(fd) => fd,
            Err(e) => {
                vfs.nodes[id].active = false;
                return vfs_err(e);
            }
        };

        vfs.nodes[id].nlinks = 0;
        vfs.nodes[id].inc_ref();
        fd as u64
    }))
}

// 11  open(path_ptr, path_len, flags, mode) -> fd
pub fn sys_open(path_ptr: u64, path_len: u64, flags: u64, mode: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::trace_println!("[syscall] open '{}' flags={:#x}", path, flags);

    let oflags = if flags == 0 { OpenFlags(OpenFlags::READ) } else { OpenFlags(flags as u32) };
    let fmode  = if mode == 0  { FileMode::default_file() } else { FileMode::new(mode as u16) };

    let path_str: &str = path.as_str();
    // A base that is not an open directory is an errno already; the VFS's own
    // error stays a `VfsError`, because the fallback below asks which it was.
    //
    // An open creating nothing changes nothing before its walk, so a name the
    // cache does not have can be read with the VFS lock released
    // (`with_vfs_walk`). One that may create runs under the lock throughout
    let open = |vfs: &mut crate::fs::vfs::core::MikuVFS| {
        base.start(vfs).map(|start| vfs.open(start, path_str, oflags, fmode))
    };
    // One creating or truncating nothing, of a file the cache has, runs beside
    // other readers (`with_vfs_read`, `MikuVFS::open_as`)
    let opened = if oflags.has(OpenFlags::CREATE) {
        crate::fs::vfs::core::with_vfs_disk(open)
    } else {
        crate::fs::vfs::core::with_vfs_read(
            |vfs, cred| {
                let start = match base.start(vfs) { Ok(v) => v, Err(e) => return Some(Err(e)) };
                match vfs.open_as(cred, start, path_str, oflags) {
                    Err(VfsError::Deferred) => None,
                    r => Some(Ok(r)),
                }
            },
            open,
        )
    };
    let vfs_result = match opened {
        Ok(r)  => r,
        Err(e) => return e,
    };

    match vfs_result {
        Ok(fd) => {
            crate::trace_println!("[syscall] open '{}' -> vfs fd={}", path_str, fd);
            fd as u64
        }
        // Only "there is no such file" reaches the fallback:
        // `open_from_active_ext` opens from the disk, and a `PermissionDenied`
        // from the VFS is an answer.
        //
        // Not for a path from a directory descriptor: the fallback resolves a
        // string on the mounted filesystem, and knows nothing of the base
        Err(VfsError::NotFound) if matches!(base, PathBase::Cwd) => {
            match open_from_active_ext(path_str, oflags, fmode) {
                Some(fd) => {
                    crate::trace_println!("[syscall] open '{}' -> ext fd={}", path_str, fd);
                    fd
                }
                // Neither the VFS nor the mounted filesystem has it. The
                // embedded libraries are real vnodes (`bootstrap.rs`), so
                // `vfs.open` finds them first
                None => err(ENOENT),
            }
        }
        Err(e) => vfs_err(e),
    }
}

// 12  close(fd) -> 0
pub fn sys_close(fd: u64) -> u64 {
    // stdin/stdout/stderr have no backing entry until something redirects them;
    // closing the terminal itself stays a no-op. Closing a redirected standard
    // descriptor drops the redirection and restores the terminal, which is how
    // a shell puts its stdio back after `cmd > file`
    if fd <= 2 {
        let redirected =
            crate::fs::vfs::core::with_vfs(|vfs| vfs.fds().get(fd as usize).is_ok());
        if !redirected {
            return 0;
        }
        return crate::fs::vfs::core::with_vfs(|vfs| match vfs.close(fd as usize) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        });
    }

    // Socket fds close through the socket layer; only the owner may close
    if crate::net::socket::is_socket_fd(fd) {
        // The thread group: a socket belongs to the process, like any other
        // descriptor (see `adapters/net.rs`)
        let pid = crate::sched::current_tgid();
        if !crate::net::socket::owned_by(fd, pid) {
            return err(EBADF);
        }
        crate::net::socket::close_fd(fd);
        return 0;
    }

    // A master going is a terminal hanging up when it was the last: the
    // `dec_ref` inside `close` counts it, and `with_vfs` hangs up after the
    // lock is released.
    //
    // The key before the close: a last descriptor takes its end of the pipe
    // with it, and the key with that
    let pipe_key = crate::fs::vfs::core::pipe_vnode_of_fd(fd as usize).map(crate::fs::vfs::pipe::wait_key);
    let r = match crate::fs::vfs::core::close_fd(fd as usize) {
        Ok(())  => 0,
        Err(e)  => vfs_err(e),
    };
    // The other end of a pipe learns of it now: a reader whose last writer has
    // gone reads its end of file, a writer whose reader has gone its EPIPE
    if let Some(key) = pipe_key {
        crate::sched::wait::wake_key(key);
    }
    r
}

// 13  seek(fd, offset, whence) -> new_offset   (whence: 0=SET, 1=CUR, 2=END)
pub fn sys_seek(fd: u64, offset: u64, whence: u64) -> u64 {
    let seek = match whence {
        0 => SeekFrom::Start(offset),
        1 => SeekFrom::Current(offset as i64),
        2 => SeekFrom::End(offset as i64),
        _ => return err(EINVAL),
    };

    match crate::fs::vfs::core::seek_fd(fd as usize, seek) {
        Ok(pos) => pos,
        Err(e)  => vfs_err(e),
    }
}

// 14  fsize(fd) -> size
pub fn sys_fsize(fd: u64) -> u64 {
    crate::fs::vfs::core::with_vfs(|vfs| match vfs.fstat(fd as usize) {
        Ok(st) => st.size,
        Err(e) => vfs_err(e),
    })
}

// 28  dup(fd) -> new_fd
pub fn sys_dup(fd: u64) -> u64 {
    // A socket is not in the VFS table; the socket layer makes the second
    // descriptor (an alias slot: see `net::socket::dup_fd`)
    if crate::net::socket::is_socket_fd(fd) {
        return match crate::net::socket::dup_fd(fd, crate::sched::current_tgid()) {
            Ok(n)  => n,
            Err(_) => err(EBADF),
        };
    }
    crate::fs::vfs::core::with_vfs(|vfs| match vfs.dup(fd as usize) {
        Ok(new_fd) => new_fd as u64,
        Err(e)     => vfs_err(e),
    })
}

// 29  dup2(old_fd, new_fd) -> new_fd
pub fn sys_dup2(old_fd: u64, new_fd: u64) -> u64 {
    if old_fd == new_fd { return new_fd; }

    crate::fs::vfs::core::with_vfs(|vfs| match vfs.dup_to(old_fd as usize, new_fd as usize) {
        Ok(fd) => fd as u64,
        Err(e) => vfs_err(e),
    })
}

// 30  truncate(fd, length) -> 0
pub fn sys_truncate(fd: u64, length: u64) -> u64 {
    crate::fs::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fds().get(fd as usize) {
            Ok(f)  => *f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        if !vfs.valid_vnode(vid) { return err(EBADF); }
        match vfs.truncate_to(vid, length) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 34  pipe(fds_ptr) -> 0   (writes [read_fd, write_fd] as two u64s)
pub fn sys_pipe(fds_ptr: u64) -> u64 {
    sys_pipe2(fds_ptr, 0)
}

// 120  pipe2(fds_ptr, flags) -> 0
//
// `pipe` with `O_CLOEXEC` and `O_NONBLOCK` on both ends from the start: set
// afterwards with `fcntl`, a sibling thread's `fork` in between hands the child
// both ends, and a reader then waits for an EOF only that child's exit can
// bring
pub fn sys_pipe2(fds_ptr: u64, flags: u64) -> u64 {
    let allowed = (OpenFlags::CLOEXEC | OpenFlags::NONBLOCK) as u64;
    if flags & !allowed != 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, fds_ptr, 16) { Some(g) => g, None => return err(EFAULT) };

    let (read_fd, write_fd) = match make_pipe(flags as u32) {
        Ok(pair) => pair,
        Err(e)   => return e,
    };

    // Re-validates the user mapping immediately before the store: a sibling
    // thread could have unmapped fds_ptr while VFS work ran.
    // Write-then-revalidate is the best available without a fault-aware
    // copy_to_user
    let _ua = match user_ptr_writable(cr3, fds_ptr, 16) {
        Some(g) => g,
        None => {
            crate::fs::vfs::core::with_vfs(|vfs| {
                let _ = vfs.close(read_fd);
                let _ = vfs.close(write_fd);
            });
            return err(EFAULT);
        }
    };
    unsafe {
        let p = fds_ptr as *mut u64;
        p.write_unaligned(read_fd as u64);
        p.add(1).write_unaligned(write_fd as u64);
    }

    crate::trace_println!("[syscall] pipe read_fd={} write_fd={}", read_fd, write_fd);
    0
}

// 113  fcntl(fd, cmd, arg) -> result / -errno
//
// `FD_CLOEXEC` on a descriptor already open keeps a pipe end out of a child,
// and `O_NONBLOCK` at runtime is what every reactor does: it opens a descriptor
// and then makes it non-blocking. `sys_read` reads the flag.
pub const F_DUPFD: u64 = miku_abi::fcntl::F_DUPFD as u64;
pub const F_GETFD: u64 = miku_abi::fcntl::F_GETFD as u64;
pub const F_SETFD: u64 = miku_abi::fcntl::F_SETFD as u64;
pub const F_GETFL: u64 = miku_abi::fcntl::F_GETFL as u64;
pub const F_SETFL: u64 = miku_abi::fcntl::F_SETFL as u64;
/// Linux's number, which is what `libc` and `std` already send
pub const F_DUPFD_CLOEXEC: u64 = miku_abi::fcntl::F_DUPFD_CLOEXEC as u64;

/// The one bit `F_GETFD`/`F_SETFD` speak in, as every Unix spells it.
const FD_CLOEXEC: u64 = miku_abi::fcntl::FD_CLOEXEC as u64;

/// Flags a program may change on an open descriptor.
///
/// Not the access mode: a descriptor opened read-only does not become writable
/// by asking, and every libc's `fcntl` ignores those bits in `F_SETFL`.
/// `O_APPEND` and `O_NONBLOCK` describe how to use the descriptor rather than
/// what it is
const SETFL_MUTABLE: u32 = OpenFlags::NONBLOCK | OpenFlags::APPEND;

pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64) -> u64 {
    // Socket descriptors live in their own range above `SOCK_FD_BASE` and are
    // not in the VFS table, so the commands below cannot reach them: see
    // `socket_fcntl`
    if crate::net::socket::is_socket_fd(fd) {
        return socket_fcntl(fd, cmd, arg);
    }

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            // The lowest free descriptor at or above `arg`, unlike `dup`(28).
            // The `_CLOEXEC` form is atomic: `dup` then `F_SETFD` leaves a
            // window in which a sibling thread's `fork` copies the descriptor
            // into a child
            crate::fs::vfs::core::with_vfs(|vfs| {
                match vfs.dup_from(fd as usize, arg as usize, cmd == F_DUPFD_CLOEXEC) {
                    Ok(n)  => n as u64,
                    Err(e) => vfs_err(e),
                }
            })
        }
        F_GETFD => crate::fs::vfs::core::with_vfs(|vfs| {
            match vfs.fds().get(fd as usize) {
                Ok(f)  => if f.flags.has(OpenFlags::CLOEXEC) { FD_CLOEXEC } else { 0 },
                Err(e) => vfs_err(e),
            }
        }),
        F_SETFD => crate::fs::vfs::core::with_vfs(|vfs| {
            match vfs.fds().get_mut(fd as usize) {
                Ok(mut f) => {
                    if arg & FD_CLOEXEC != 0 {
                        f.flags = OpenFlags(f.flags.0 | OpenFlags::CLOEXEC);
                    } else {
                        f.flags = OpenFlags(f.flags.0 & !OpenFlags::CLOEXEC);
                    }
                    0
                }
                Err(e) => vfs_err(e),
            }
        }),
        F_GETFL => crate::fs::vfs::core::with_vfs(|vfs| {
            match vfs.fds().get(fd as usize) {
                Ok(f)  => f.flags.0 as u64,
                Err(e) => vfs_err(e),
            }
        }),
        F_SETFL => crate::fs::vfs::core::with_vfs(|vfs| {
            match vfs.fds().get_mut(fd as usize) {
                Ok(mut f) => {
                    let keep = f.flags.0 & !SETFL_MUTABLE;
                    f.flags = OpenFlags(keep | (arg as u32 & SETFL_MUTABLE));
                    0
                }
                Err(e) => vfs_err(e),
            }
        }),
        F_GETLK | F_SETLK | F_SETLKW => record_lock(fd, cmd, arg),
        _ => err(EINVAL),
    }
}

pub const F_GETLK: u64 = miku_abi::fcntl::F_GETLK as u64;
pub const F_SETLK: u64 = miku_abi::fcntl::F_SETLK as u64;
pub const F_SETLKW: u64 = miku_abi::fcntl::F_SETLKW as u64;
use miku_abi::fcntl::F_RDLCK;
use miku_abi::fcntl::F_WRLCK;
use miku_abi::fcntl::F_UNLCK;

/// Record locks: `fs::vfs::lock` has the semantics. `arg` is a `struct flock`,
/// 32 bytes:
///
///     0  i16 l_type     F_RDLCK / F_WRLCK / F_UNLCK
///     2  i16 l_whence   SEEK_SET / SEEK_CUR / SEEK_END
///     8  i64 l_start    relative to l_whence
///    16  i64 l_len      0 = to the end of the file, however long it grows;
///                       negative = the bytes before l_start
///    24  i32 l_pid      F_GETLK's answer: who holds the conflicting lock
///
/// A read lock needs a descriptor open for reading and a write lock one open
/// for writing (`EBADF` otherwise), as POSIX requires
fn record_lock(fd: u64, cmd: u64, arg: u64) -> u64 {
    use crate::fs::vfs::lock::{self, Kind};
    const FLOCK_LEN: u64 = 32;

    let cr3 = crate::syscall::usercopy::current_cr3();
    let (ltype, whence, lstart, llen) = {
        let _ua = match crate::syscall::usercopy::user_ptr_mapped(cr3, arg, FLOCK_LEN) {
            Some(g) => g,
            None => return err(EFAULT),
        };
        unsafe {
            let p = arg as *const u8;
            (
                (p as *const i16).read_unaligned(),
                (p.add(2) as *const i16).read_unaligned(),
                (p.add(8) as *const i64).read_unaligned(),
                (p.add(16) as *const i64).read_unaligned(),
            )
        }
    };

    // Where the range is measured from, and whether the descriptor may take
    // this kind of lock at all
    let found = crate::fs::vfs::core::with_vfs(|vfs| {
        let f = vfs.fds().get(fd as usize).map(|f| *f)?;
        // An id with no vnode reads as an empty one, of size 0
        let size = vfs.nodes[f.vnode_id as usize].size;
        Ok::<_, crate::fs::vfs::types::VfsError>((f, size))
    });
    let (file, size) = match found {
        Ok(x) => x,
        Err(e) => return vfs_err(e),
    };
    let base: i64 = match whence {
        0 => 0,
        1 => file.offset as i64,
        2 => size as i64,
        _ => return err(EINVAL),
    };
    let from = match base.checked_add(lstart) {
        Some(v) => v,
        None => return err(EINVAL),
    };
    let (start, end) = if llen == 0 {
        (from, u64::MAX as i128)
    } else if llen > 0 {
        (from, from as i128 + llen as i128)
    } else {
        match from.checked_add(llen) {
            Some(s) => (s, from as i128),
            None => return err(EINVAL),
        }
    };
    if start < 0 || end > u64::MAX as i128 {
        return err(EINVAL);
    }
    let (start, end) = (start as u64, end as u64);

    let kind = match ltype {
        F_RDLCK => Some(Kind::Read),
        F_WRLCK => Some(Kind::Write),
        F_UNLCK => None,
        _ => return err(EINVAL),
    };
    let owner = crate::sched::current_tgid();
    let vnode = file.vnode_id;

    if cmd == F_GETLK {
        let Some(k) = kind else { return err(EINVAL) };
        let answer = lock::conflict(vnode, owner, start, end, k);
        let _ua = match crate::syscall::usercopy::user_ptr_writable(cr3, arg, FLOCK_LEN) {
            Some(g) => g,
            None => return err(EFAULT),
        };
        unsafe {
            let p = arg as *mut u8;
            match answer {
                None => (p as *mut i16).write_unaligned(F_UNLCK),
                Some(l) => {
                    (p as *mut i16).write_unaligned(if l.kind == Kind::Write { F_WRLCK } else { F_RDLCK });
                    (p.add(2) as *mut i16).write_unaligned(0);
                    (p.add(8) as *mut i64).write_unaligned(l.start as i64);
                    let len = if l.end == u64::MAX { 0 } else { (l.end - l.start) as i64 };
                    (p.add(16) as *mut i64).write_unaligned(len);
                    (p.add(24) as *mut i32).write_unaligned(l.owner as i32);
                }
            }
        }
        return 0;
    }

    match kind {
        Some(Kind::Read) if !file.readable() => return err(EBADF),
        Some(Kind::Write) if !file.writable() => return err(EBADF),
        _ => {}
    }
    match (cmd, kind) {
        (F_SETLKW, Some(k)) => match lock::set_wait(vnode, owner, start, end, k) {
            Ok(()) => 0,
            Err(()) => err(EINTR),
        },
        _ => match lock::set(vnode, owner, start, end, kind) {
            Ok(()) => 0,
            // POSIX allows EACCES or EAGAIN here; SQLite accepts either, and
            // EAGAIN is what Linux says
            Err(_) => err(EAGAIN),
        },
    }
}

/// `fcntl` for the descriptors the VFS does not own.
///
/// What is refused is refused by name rather than accepted and ignored:
///
///   * `F_DUPFD` makes an alias slot, as `dup`(28) does (see
///     `net::socket::dup_fd`). The socket is refcounted by its aliases and
///     released with the last descriptor.
///   * `FD_CLOEXEC` is kept in the socket's slot and `exec` closes what carries
///     it, beside the VFS's own sweep
fn socket_fcntl(fd: u64, cmd: u64, arg: u64) -> u64 {
    // Only the owner's: a server's blocking read turned into `EAGAIN` from
    // outside is a denial of service at best
    if !crate::net::socket::owned_by(fd, crate::sched::current_tgid()) {
        return err(EBADF);
    }
    match cmd {
        F_GETFL => {
            let mut f = OpenFlags::READ | OpenFlags::WRITE;
            if crate::net::socket::is_nonblocking(fd) { f |= OpenFlags::NONBLOCK; }
            f as u64
        }
        F_SETFL => {
            let on = arg as u32 & OpenFlags::NONBLOCK != 0;
            match crate::net::socket::set_nonblocking(fd, on) {
                Ok(())  => 0,
                Err(_)  => err(EBADF),
            }
        }
        F_GETFD => if crate::net::socket::is_cloexec(fd) { FD_CLOEXEC } else { 0 },
        F_SETFD => match crate::net::socket::set_cloexec(fd, arg & FD_CLOEXEC != 0) {
            Ok(())  => 0,
            Err(_)  => err(EBADF),
        },
        // The socket layer's lowest free number; `arg` is a floor in the VFS
        // table's numbering and has nothing to say about this one
        F_DUPFD => match crate::net::socket::dup_fd(fd, crate::sched::current_tgid()) {
            Ok(n)  => n,
            Err(_) => err(EMFILE),
        },
        // A socket's descriptors go to no child whichever way this is set (the
        // table is keyed on the owning process), but `F_GETFD` answers with the
        // flag, so it is set as asked
        F_DUPFD_CLOEXEC => match crate::net::socket::dup_fd(fd, crate::sched::current_tgid()) {
            Ok(n)  => {
                let _ = crate::net::socket::set_cloexec(n, true);
                n
            }
            Err(_) => err(EMFILE),
        },
        _       => err(EINVAL),
    }
}

/// Is this descriptor marked `O_NONBLOCK`?
///
/// Read at the top of a blocking loop rather than once per wait: `fcntl` may
/// have changed it
pub fn fd_is_nonblocking(fd: u64) -> bool {
    if crate::net::socket::is_socket_fd(fd) {
        return crate::net::socket::is_nonblocking(fd);
    }
    // The descriptor alone, with no lock (`vfs::fd::peek`)
    crate::fs::vfs::fd::peek(fd as usize).is_some_and(|f| f.flags.has(OpenFlags::NONBLOCK))
}

/// Makes an anonymous pipe and returns its two descriptors
///
/// Split from `sys_pipe` for callers with no user pointer: the selftest
pub fn make_pipe(extra: u32) -> Result<(usize, usize), u64> {
    // Two vnodes in no directory, the read end's and the write end's, and a
    // ring between them (`fs::vfs::pipe`), with a descriptor on each end. Two,
    // so each vnode's reference count is that end's descriptors: the pipe knows
    // its readers and writers without walking anybody's table
    crate::fs::vfs::core::with_vfs(|vfs| -> Result<(usize, usize), u64> {
        let r_vid = vfs
            .anon_node(VNodeKind::Pipe, "pipe", FileMode::default_pipe())
            .map_err(|_| err(ENOMEM))?;
        let w_vid = match vfs.anon_node(VNodeKind::Pipe, "pipe", FileMode::default_pipe()) {
            Ok(v) => v,
            Err(_) => {
                vfs.nodes[r_vid].active = false;
                return Err(err(ENOMEM));
            }
        };
        crate::fs::vfs::pipe::create(r_vid, w_vid);

        let r_fd = match vfs.fds().alloc(r_vid as _, OpenFlags(OpenFlags::READ | extra)) {
            Ok(fd) => fd,
            Err(e) => {
                crate::fs::vfs::pipe::forget(r_vid);
                crate::fs::vfs::pipe::forget(w_vid);
                vfs.nodes[r_vid].active = false;
                vfs.nodes[w_vid].active = false;
                return Err(vfs_err(e));
            }
        };
        vfs.nodes[r_vid].inc_ref();
        // Bound first: the error path closes the read end, which takes the
        // table again (`vfs::fd` panics on a second take)
        let w_fd = vfs.fds().alloc(w_vid as _, OpenFlags(OpenFlags::WRITE | extra));
        let w_fd = match w_fd {
            Ok(fd) => fd,
            Err(e) => {
                // Closing the read end frees its vnode with its last
                // reference; the write end never had one
                let _ = vfs.close(r_fd);
                crate::fs::vfs::pipe::forget(w_vid);
                vfs.nodes[w_vid].active = false;
                return Err(vfs_err(e));
            }
        };
        vfs.nodes[w_vid].inc_ref();
        Ok((r_fd, w_fd))
    })
}

// 37  fallocate(fd, offset, len) -> 0
pub fn sys_fallocate(fd: u64, offset: u64, len: u64) -> u64 {
    if len == 0 { return err(EINVAL); }

    crate::fs::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fds().get(fd as usize) {
            Ok(f)  => *f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        let ext2_ino = vfs.nodes[vid].ext2_ino;
        if ext2_ino == 0 { return err(ENOSYS); }
        // Below the VFS, by inode: the page cache's copy cannot follow it
        crate::fs::vfs::filecache::lock().drop_file(vid);

        let result = crate::fs::ext::mount::with_ext2_pub(|fs| {
            if fs.superblock.has_extents() {
                fs.ext4_fallocate(ext2_ino, offset, len)
            } else {
                fs.ext2_fallocate(ext2_ino, offset, len)
            }
        });
        match result {
            Some(Ok(()))  => 0,
            Some(Err(_))  => err(ENOSPC),
            None          => err(ENOSYS),
        }
    })
}

// 41  fsync(fd) -> 0
pub fn sys_fsync(fd: u64) -> u64 {
    // The filesystem this descriptor is on, not whichever one is active
    let slot = crate::fs::vfs::core::with_vfs(|vfs| {
        let f = *vfs.fds().get(fd as usize).ok()?;
        let vid = f.vnode_id as usize;
        if !vfs.valid_vnode(vid) {
            return None;
        }
        Some(vfs.nodes[vid].ext2_slot)
    });
    let Some(slot) = slot else { return err(EBADF) };

    // The file's pages still in the page cache first
    // (`fs/vfs/core/writeback.rs`)
    if let Err(e) = crate::fs::vfs::core::writeback::flush_fd(fd as usize) {
        return vfs_err(e);
    }
    let result = crate::fs::ext::mount::with_ext2_for(slot, |fs| fs.sync());
    match result {
        Some(Ok(()))  => 0,
        Some(Err(e))  => vfs_err(e.into()),
        None          => 0, // not an ext file = nothing to sync
    }
}

// 42  punch_hole(fd, offset, len) -> 0
pub fn sys_punch_hole(fd: u64, offset: u64, len: u64) -> u64 {
    if len == 0 { return err(EINVAL); }

    crate::fs::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fds().get(fd as usize) {
            Ok(f)  => *f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        let ext2_ino = vfs.nodes[vid].ext2_ino;
        if ext2_ino == 0 { return err(ENOSYS); }
        // Below the VFS, by inode: the page cache's copy cannot follow it
        crate::fs::vfs::filecache::lock().drop_file(vid);

        let result = crate::fs::ext::mount::with_ext2_pub(|fs| {
            if fs.superblock.has_extents() {
                // ext4: zeroes whole blocks within the range; full extent-tree
                // surgery is not implemented
                let bs = fs.block_size as u64;
                let start_block = ((offset + bs - 1) / bs) as u32;
                let end_block   = ((offset + len) / bs) as u32;
                for logical in start_block..end_block {
                    if let Ok(phys) = fs.get_file_block_any(ext2_ino, logical) {
                        if phys != 0 { let _ = fs.zero_block(phys); }
                    }
                }
                let now = fs.get_timestamp();
                if let Ok(mut inode) = fs.read_inode(ext2_ino) {
                    inode.set_mtime(now);
                    inode.set_ctime(now);
                    let _ = fs.write_inode(ext2_ino, &inode);
                }
                Ok(())
            } else {
                fs.ext2_punch_hole(ext2_ino, offset, len)
            }
        });
        match result {
            Some(Ok(()))  => 0,
            Some(Err(_))  => err(ENOSPC),
            None          => err(ENOSYS),
        }
    })
}
