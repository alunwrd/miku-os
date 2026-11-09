// Filesystem-namespace syscalls: stat/fstat, mkdir/rmdir/unlink, readdir,
// rename/link/symlink/readlink, chmod/chown, chdir, statfs, xattr, utimensat

extern crate alloc;

use crate::syscall::abi::{
    write_stat_to_user, STAT_SIZE, STATFS_SIZE, UDIRENT_INO_OFF, UDIRENT_KIND_OFF,
    UDIRENT_NAMELEN_OFF, UDIRENT_NAME_LEN, UDIRENT_SIZE,
};
use crate::syscall::errno::{err, vfs_err, EACCES, EFAULT, EINVAL, ENOENT, ENOSYS, ENOTDIR, ERANGE};
use crate::syscall::usercopy::{current_cr3, read_user_path_at, user_ptr_mapped, user_ptr_writable};
use crate::fs::vfs::types::{DirEntry, FileMode, NAME_LEN};

// 18  stat(path_ptr, path_len, stat_buf) -> 0
pub fn sys_stat(path_ptr: u64, path_len: u64, stat_ptr: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, stat_ptr, STAT_SIZE) { Some(g) => g, None => return err(EFAULT) };

    // From the cache beside every other reader (`with_vfs_read`); a name the
    // cache does not have is read from the disk with the VFS lock released
    // (`with_vfs_walk`)
    crate::fs::vfs::core::with_vfs_read(
        |vfs, cred| {
            let start = match base.start(vfs) { Ok(v) => v, Err(e) => return Some(e) };
            read_or_defer(vfs.stat_as(cred, start, &path, Links::All)).map(|r| stat_reply(stat_ptr, r))
        },
        |vfs| {
            let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
            stat_reply(stat_ptr, vfs.stat(start, &path))
        },
    )
}

use crate::fs::vfs::core::path_resolve::Links;

/// What a call under the lock held to read gave: `None` when it gave up for
/// the call to be made again with the lock held to write (`with_vfs_read`)
fn read_or_defer<T>(r: crate::fs::vfs::types::VfsResult<T>) -> Option<crate::fs::vfs::types::VfsResult<T>> {
    match r {
        Err(crate::fs::vfs::types::VfsError::Deferred) => None,
        r => Some(r),
    }
}

fn stat_reply(stat_ptr: u64, r: crate::fs::vfs::types::VfsResult<crate::fs::vfs::types::VNodeStat>) -> u64 {
    match r {
        Ok(st) => { write_stat_to_user(stat_ptr, &st); 0 }
        Err(e) => vfs_err(e),
    }
}

// 112  lstat(path_ptr, path_len, stat_buf) -> 0
//
// `stat`(18) follows a symlink. `std::fs::symlink_metadata` needs this, and a
// recursive remove has to ask whether an entry is itself a link, or it follows
// links out of the directory it was told to empty. `MikuVFS::lstat` is in
// `attr.rs`
pub fn sys_lstat(path_ptr: u64, path_len: u64, stat_ptr: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, stat_ptr, STAT_SIZE) {
        Some(g) => g,
        None    => return err(EFAULT),
    };

    crate::fs::vfs::core::with_vfs_read(
        |vfs, cred| {
            let start = match base.start(vfs) { Ok(v) => v, Err(e) => return Some(e) };
            read_or_defer(vfs.stat_as(cred, start, &path, Links::AllButLast)).map(|r| stat_reply(stat_ptr, r))
        },
        |vfs| {
            let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
            stat_reply(stat_ptr, vfs.lstat(start, &path))
        },
    )
}

// 119  realpath(path_ptr, path_word, buf, size) -> length
//
// The path resolved as `open` would (every `.`, `..` and symlink, the last
// component included), and the vnode it lands on named from the root by the
// VFS's one walk (`MikuVFS::abs_path`), so no resolver in the C library or
// `std` can disagree with it.
//
// The path word is the usual one (docs/MikuOS_ABI.md 3.2e). Length 0 with a
// descriptor as the base names that descriptor's own vnode: the path of an open
// file, which Zed asks to notice a renamed worktree root.
//
// Writes the path and a NUL; returns the length without it. ERANGE when it does
// not fit
pub fn sys_realpath(path_ptr: u64, path_word: u64, buf: u64, size: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_word) {
        Ok(p)  => p,
        Err(e) => return e,
    };
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf, size) {
        Some(g) => g,
        None    => return err(EFAULT),
    };

    let named = crate::fs::vfs::core::with_vfs(|vfs| {
        let start = base.start(vfs)?;
        let id = if path.is_empty() {
            start
        } else {
            vfs.resolve_path_follow(start, &path).map_err(vfs_err)?
        };
        Ok(vfs.abs_path(id))
    });
    let named = match named {
        Ok(p)  => p,
        Err(e) => return e,
    };

    let bytes = named.as_bytes();
    if bytes.len() as u64 + 1 > size { return err(ERANGE); }
    unsafe {
        let p = buf as *mut u8;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        p.add(bytes.len()).write(0);
    }
    bytes.len() as u64
}

// 19  fstat(fd, stat_buf) -> 0
pub fn sys_fstat(fd: u64, stat_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, stat_ptr, STAT_SIZE) { Some(g) => g, None => return err(EFAULT) };

    crate::fs::vfs::core::with_vfs(|vfs| match vfs.fstat(fd as usize) {
        Ok(st) => { write_stat_to_user(stat_ptr, &st); 0 }
        Err(e) => vfs_err(e),
    })
}

// 20  mkdir(path_ptr, path_len, mode) -> 0
pub fn sys_mkdir(path_ptr: u64, path_len: u64, mode: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::trace_println!("[syscall] mkdir '{}'", path);

    let fmode = if mode == 0 { FileMode::default_dir() } else { FileMode::new(mode as u16) };
    // `mkdir("a/b/")` names `b`, as it does everywhere else
    let trimmed = path.trim_end_matches('/');
    let path = if trimmed.is_empty() { "/" } else { trimmed };

    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        // The VFS's own split: a name with no directory in front of it is in
        // the start directory
        let (parent_id, dirname) = match vfs.split_path(start, path) {
            Ok(v) => v,
            Err(e) => return vfs_err(e),
        };
        match vfs.mkdir(parent_id, dirname, fmode) {
            Ok(_)  => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 21  rmdir(path_ptr, path_len) -> 0
pub fn sys_rmdir(path_ptr: u64, path_len: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::trace_println!("[syscall] rmdir '{}'", path);

    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        match vfs.rmdir(start, &path) {
            Ok(())  => 0,
            Err(e)  => vfs_err(e),
        }
    })
}

// 22  unlink(path_ptr, path_len) -> 0
pub fn sys_unlink(path_ptr: u64, path_len: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::trace_println!("[syscall] unlink '{}'", path);

    crate::fs::vfs::core::with_vfs_disk(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        match vfs.unlink(start, &path) {
            Ok(())  => 0,
            Err(e)  => vfs_err(e),
        }
    })
}

/// How many entries one call may carry.
///
/// A `Vec` on the kernel heap for the duration of the call, so a budget rather
/// than a promise: a caller asking for more gets this many and comes back. 120
/// entries of 272 bytes are the heap's largest slab size; more is a `vmap` of
/// fresh pages on every call
const DIRENTS_PER_CALL: usize = 120;

/// Writes `n` entries into a userspace buffer in the `getdents` ABI's shape,
/// for `readdir`(23) and `getdents`(111)
fn write_dirents(buf_ptr: u64, entries: &[DirEntry], n: usize) {
    for i in 0..n {
        unsafe {
            let base = (buf_ptr + (i as u64) * UDIRENT_SIZE) as *mut u8;
            // Clamped to the ABI's width, not the VFS's: `name_len` must
            // describe what was written, or a reader walks off its own buffer
            let nlen = (entries[i].name_len as usize).min(UDIRENT_NAME_LEN - 1);
            core::ptr::copy_nonoverlapping(entries[i].name.as_ptr(), base, nlen);
            // The name, its terminator and the fields after it, and nothing
            // else: the bytes past the terminator stay as the caller left them,
            // which saves zeroing the whole 272 for a short name
            *base.add(nlen) = 0;
            (base.add(UDIRENT_INO_OFF) as *mut u64)
                .write_unaligned(entries[i].inode_id as u64);
            *base.add(UDIRENT_KIND_OFF) = entries[i].kind as u8;
            *base.add(UDIRENT_NAMELEN_OFF) = nlen as u8;
            core::ptr::write_bytes(base.add(UDIRENT_NAMELEN_OFF + 1), 0, UDIRENT_SIZE as usize - UDIRENT_NAMELEN_OFF - 1);
        }
    }
}

// 111  getdents(fd, buf_ptr, max_entries, offset) -> count / -errno
//
// Unlike `readdir`(23):
//
// It takes an open directory, not a path. `readdir` resolves its path on every
// call, so walking a tree re-walks every prefix, and between two calls the path
// may name something else.
//
// It takes an offset, so a directory larger than one buffer can be read. The
// caller advances `offset` by the returned count and stops at zero.
//
// `std::fs::read_dir` is an iterator over an open directory and is built on
// this (see dev/std-architecture.md)
pub fn sys_getdents(fd: u64, buf_ptr: u64, max_entries: u64, offset: u64) -> u64 {
    if max_entries == 0 { return 0; }
    let total_size = max_entries.saturating_mul(UDIRENT_SIZE);
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf_ptr, total_size) {
        Some(g) => g,
        None    => return err(EFAULT),
    };

    let want = (max_entries as usize).min(DIRENTS_PER_CALL);
    let entries = core::cell::RefCell::new(alloc::vec![DirEntry::empty(); want]);

    // A directory the cache has whole is listed beside other readers
    // (`with_vfs_read`); one not listed yet is read with the VFS lock released
    // (`with_vfs_walk`)
    let vid_of = |vfs: &crate::fs::vfs::core::MikuVFS| vfs.fds().get(fd as usize).map(|f| f.vnode_id as usize);
    let result = crate::fs::vfs::core::with_vfs_read(
        |vfs, _| {
            let vid = match vid_of(vfs) { Ok(v) => v, Err(e) => return Some(Err(e)) };
            read_or_defer(vfs.readdir_at_as(vid, offset as usize, &mut entries.borrow_mut()))
        },
        |vfs| vfs.readdir_at(vid_of(vfs)?, offset as usize, &mut entries.borrow_mut()),
    );

    match result {
        Ok(n) => { write_dirents(buf_ptr, &entries.borrow(), n); n as u64 }
        Err(e) => vfs_err(e),
    }
}

// 23  readdir(path_ptr, path_len, buf_ptr, max_entries) -> count
pub fn sys_readdir(path_ptr: u64, path_len: u64, buf_ptr: u64, max_entries: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    if max_entries == 0 { return 0; }
    let total_size = max_entries.saturating_mul(UDIRENT_SIZE);
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf_ptr, total_size) { Some(g) => g, None => return err(EFAULT) };

    // Bounded by the limit `getdents` uses; a caller needing more than one pass
    // has `getdents`(111)
    let count = (max_entries as usize).min(DIRENTS_PER_CALL);
    let mut entries = alloc::vec![DirEntry::empty(); count];

    let result = crate::fs::vfs::core::with_vfs(|vfs| {
        let start = base.start(vfs)?;
        let dir_id = vfs.resolve_path(start, &path).map_err(vfs_err)?;
        vfs.readdir(dir_id, &mut entries).map_err(vfs_err)
    });

    match result {
        Ok(n) => { write_dirents(buf_ptr, &entries, n); n as u64 }
        Err(e) => e,
    }
}

// 24  rename(old_ptr, old_len, new_ptr, new_len) -> 0
pub fn sys_rename(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> u64 {
    let (old_path, old_base) = match read_user_path_at(old_ptr, old_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let (new_path, new_base) = match read_user_path_at(new_ptr, new_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::trace_println!("[syscall] rename '{}' -> '{}'", old_path, new_path);

    crate::fs::vfs::core::with_vfs(|vfs| {
        let old_start = match old_base.start(vfs) { Ok(v) => v, Err(e) => return e };
        let new_start = match new_base.start(vfs) { Ok(v) => v, Err(e) => return e };
        match vfs.rename_at(old_start, &old_path, new_start, &new_path) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 25  link(old_ptr, old_len, new_ptr, new_len) -> 0
pub fn sys_link(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> u64 {
    let (old_path, old_base) = match read_user_path_at(old_ptr, old_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let (new_path, new_base) = match read_user_path_at(new_ptr, new_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::trace_println!("[syscall] link '{}' -> '{}'", old_path, new_path);

    crate::fs::vfs::core::with_vfs(|vfs| {
        let old_start = match old_base.start(vfs) { Ok(v) => v, Err(e) => return e };
        let new_start = match new_base.start(vfs) { Ok(v) => v, Err(e) => return e };
        let (parent_id, linkname) = match vfs.split_path(new_start, &new_path) {
            Ok(v) => v,
            Err(e) => return vfs_err(e),
        };
        match vfs.link(old_start, &old_path, parent_id, linkname) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 26  chmod(path_ptr, path_len, mode) -> 0
pub fn sys_chmod(path_ptr: u64, path_len: u64, mode: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };
    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        match vfs.chmod(start, &path, FileMode::new(mode as u16)) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 27  chown(path_ptr, path_len, uid, gid) -> 0   (0xFFFF = "no change")
pub fn sys_chown(path_ptr: u64, path_len: u64, uid: u64, gid: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };

    let o_uid = if uid == 0xFFFF { None } else { Some(uid as u16) };
    let o_gid = if gid == 0xFFFF { None } else { Some(gid as u16) };

    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        match vfs.chown(start, &path, o_uid, o_gid) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 32  symlink(target_ptr, target_len, link_ptr, link_len) -> 0
pub fn sys_symlink(target_ptr: u64, target_len: u64, link_ptr: u64, link_len: u64) -> u64 {
    // The target is text, stored as given and resolved when the link is
    // followed, so it has no base of its own; only the link's path does
    let target = match crate::syscall::usercopy::read_user_path(target_ptr, target_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let (linkpath, base) = match read_user_path_at(link_ptr, link_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::trace_println!("[syscall] symlink '{}' -> '{}'", linkpath, target);

    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        let (parent_id, linkname) = match vfs.split_path(start, &linkpath) {
            Ok(v) => v,
            Err(e) => return vfs_err(e),
        };
        match vfs.symlink(parent_id, linkname, &target) {
            Ok(_)  => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 33  readlink(path_ptr, path_len, buf_ptr, buf_len) -> bytes_written
pub fn sys_readlink(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };

    if buf_len == 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf_ptr, buf_len) { Some(g) => g, None => return err(EFAULT) };

    let reply = |r: crate::fs::vfs::types::VfsResult<alloc::vec::Vec<u8>>| match r {
        Ok(target) => {
            let copy_len = target.len().min(buf_len as usize);
            unsafe {
                core::ptr::copy_nonoverlapping(target.as_ptr(), buf_ptr as *mut u8, copy_len);
                if copy_len < buf_len as usize {
                    *(buf_ptr as *mut u8).add(copy_len) = 0;
                }
            }
            copy_len as u64
        }
        Err(e) => vfs_err(e),
    };
    crate::fs::vfs::core::with_vfs_read(
        |vfs, cred| {
            let start = match base.start(vfs) { Ok(v) => v, Err(e) => return Some(e) };
            read_or_defer(vfs.readlink_as(cred, start, &path)).map(reply)
        },
        |vfs| {
            let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
            reply(vfs.readlink(start, &path))
        },
    )
}

// 35  chdir(path_ptr, path_len) -> 0
pub fn sys_chdir(path_ptr: u64, path_len: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::trace_println!("[syscall] chdir '{}'", path);

    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        let id = match vfs.resolve_path(start, &path) {
            Ok(id) => id,
            Err(e) => return vfs_err(e),
        };
        if !vfs.nodes[id].is_dir() { return err(ENOTDIR); }
        // Per-process cwd lives on the Process struct, with a reference on
        // the vnode for as long as it is there (`change_cwd`)
        vfs.change_cwd(id);
        0
    })
}

// 36  statfs(path_ptr, path_len, buf_ptr) -> 0
pub fn sys_statfs(path_ptr: u64, path_len: u64, buf_ptr: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf_ptr, STATFS_SIZE) { Some(g) => g, None => return err(EFAULT) };

    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        match vfs.statfs(start, &path) {
            Ok(sf) => {
                let rec = miku_abi::kstat::MikuStatfs {
                    magic: sf.fs_type.magic(),
                    block_size: sf.block_size,
                    total_blocks: sf.total_blocks,
                    free_blocks: sf.free_blocks,
                    total_inodes: sf.total_inodes,
                    free_inodes: sf.free_inodes,
                    max_name_len: sf.max_name_len,
                    flags: sf.flags,
                };
                unsafe { (buf_ptr as *mut miku_abi::kstat::MikuStatfs).write_unaligned(rec) };
                0
            }
            Err(e) => vfs_err(e),
        }
    })
}

// 38  getxattr(ino, name_ptr, name_len, buf_ptr) -> value_len
/// The calling process's identity, for the permission check below.
///
/// The xattr calls take a raw inode number from ring 3 rather than a
/// descriptor, so nothing upstream has checked anything: whoever can guess an
/// inode number could read or write somebody else's attributes. The ABI is what
/// it is, so the check happens here
fn caller_creds() -> crate::fs::vfs::types::Credentials {
    crate::sched::current_cred()
}

/// Whether the caller may touch this inode's attributes
fn may_touch_xattr(
    fs: &mut crate::fs::ext::MikuFS,
    ino: u32,
    creds: &crate::fs::vfs::types::Credentials,
    access: crate::fs::vfs::permissions::AccessMode,
) -> Result<(), crate::fs::ext::FsError> {
    use crate::fs::ext::FsError;
    use crate::fs::vfs::permissions::check_access;
    use crate::fs::vfs::types::FileMode;
    let inode = fs.read_inode(ino)?;
    let mode = FileMode(inode.mode() & 0o7777);
    // fold_id for the same reason the vnode bridge does it: the low 16 bits
    // of a foreign uid are a different user, and 65536 truncates to root
    let uid = crate::process::cred::fold_id(inode.uid_full());
    let gid = crate::process::cred::fold_id(inode.gid_full());
    if check_access(mode, uid, gid, creds, access) {
        Ok(())
    } else {
        Err(FsError::PermissionDenied)
    }
}

pub fn sys_getxattr(ino: u64, name_ptr: u64, name_len: u64, buf_ptr: u64) -> u64 {
    if ino == 0 || name_len == 0 || name_len > 64 { return err(EINVAL); }
    let cr3 = current_cr3();
    let _ua = match user_ptr_mapped(cr3, name_ptr, name_len) { Some(g) => g, None => return err(EFAULT) };
    let _ua = match user_ptr_writable(cr3, buf_ptr, 256) { Some(g) => g, None => return err(EFAULT) };

    // Copies the attribute name into a kernel buffer before parsing; the user
    // could otherwise rewrite it between validation and ext2 use
    let mut name_buf = [0u8; 64];
    unsafe {
        core::ptr::copy_nonoverlapping(
            name_ptr as *const u8,
            name_buf.as_mut_ptr(),
            name_len as usize,
        );
    }
    let name = match core::str::from_utf8(&name_buf[..name_len as usize]) {
        Ok(s)  => s,
        Err(_) => return err(EINVAL),
    };

    // Reads the xattr into a kernel buffer, then revalidates user_ptr and
    // copies out. ext2 disk I/O may yield, so the user mapping checked above
    // can go stale
    let creds = caller_creds();
    let result = crate::fs::ext::mount::with_ext2_pub(|fs| {
        use crate::fs::vfs::permissions::AccessMode;
        may_touch_xattr(fs, ino as u32, &creds, AccessMode::Read)?;
        let mut vbuf = [0u8; 256];
        match fs.get_xattr(
            ino as u32,
            crate::fs::ext::xattr::XATTR_INDEX_USER,
            name,
            &mut vbuf,
        ) {
            Ok(len) => Ok((vbuf, len)),
            Err(e) => Err(e),
        }
    });
    match result {
        Some(Ok((vbuf, len))) => {
            let _ua = match user_ptr_writable(cr3, buf_ptr, 256) { Some(g) => g, None => return err(EFAULT) };
            let copy = len.min(256);
            unsafe {
                core::ptr::copy_nonoverlapping(vbuf.as_ptr(), buf_ptr as *mut u8, copy);
            }
            len as u64
        }
        Some(Err(crate::fs::ext::FsError::PermissionDenied)) => err(EACCES),
        Some(Err(_)) => err(ENOENT),
        None         => err(ENOSYS),
    }
}

// 39  setxattr(ino, name_ptr, value_ptr, sizes) -> 0
//     sizes = (name_len << 16) | value_len
pub fn sys_setxattr(ino: u64, name_ptr: u64, value_ptr: u64, sizes: u64) -> u64 {
    let name_len  = (sizes >> 16) as usize;
    let value_len = (sizes & 0xFFFF) as usize;
    if ino == 0 || name_len == 0 || name_len > 64 || value_len > 256 {
        return err(EINVAL);
    }
    let cr3 = current_cr3();
    let _ua = match user_ptr_mapped(cr3, name_ptr, name_len as u64) { Some(g) => g, None => return err(EFAULT) };
    let _uav = if value_len > 0 {
        match user_ptr_mapped(cr3, value_ptr, value_len as u64) {
            Some(g) => Some(g),
            None    => return err(EFAULT),
        }
    } else {
        None
    };

    // Copies name + value into kernel buffers before parsing and storing: the
    // user could otherwise mutate them after validation
    let mut name_buf = [0u8; 64];
    unsafe {
        core::ptr::copy_nonoverlapping(name_ptr as *const u8, name_buf.as_mut_ptr(), name_len);
    }
    let name = match core::str::from_utf8(&name_buf[..name_len]) {
        Ok(s)  => s,
        Err(_) => return err(EINVAL),
    };
    let mut value_buf = [0u8; 256];
    if value_len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                value_ptr as *const u8,
                value_buf.as_mut_ptr(),
                value_len,
            );
        }
    }
    let value: &[u8] = &value_buf[..value_len];

    let creds = caller_creds();
    let result = crate::fs::ext::mount::with_ext2_pub(|fs| {
        use crate::fs::vfs::permissions::AccessMode;
        may_touch_xattr(fs, ino as u32, &creds, AccessMode::Write)?;
        fs.ext3_set_xattr(
            ino as u32,
            crate::fs::ext::xattr::XATTR_INDEX_USER,
            name,
            value,
        )
    });
    match result {
        Some(Ok(()))  => 0,
        Some(Err(e))  => vfs_err(e.into()),
        None          => err(ENOSYS),
    }
}

// 81  listxattr(ino, buf_ptr, buf_len) -> bytes written / -errno
//
// The names of an inode's user attributes, each NUL-terminated, one after
// another, as Linux's listxattr. A zero-length buffer asks only for the size
pub fn sys_listxattr(ino: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if ino == 0 || buf_len > 4096 {
        return err(EINVAL);
    }
    let cr3 = current_cr3();
    if buf_len > 0 {
        let _ua = match user_ptr_writable(cr3, buf_ptr, buf_len) {
            Some(g) => g,
            None => return err(EFAULT),
        };
    }

    let creds = caller_creds();
    let result = crate::fs::ext::mount::with_ext2_pub(|fs| {
        use crate::fs::vfs::permissions::AccessMode;
        may_touch_xattr(fs, ino as u32, &creds, AccessMode::Read)?;
        let list = fs.read_xattrs(ino as u32)?;
        let mut out = [0u8; 4096];
        let mut at = 0usize;
        for i in 0..list.count {
            let e = &list.entries[i];
            if e.name_index != crate::fs::ext::xattr::XATTR_INDEX_USER {
                continue;
            }
            let n = e.name_len as usize;
            if at + n + 1 > out.len() {
                break;
            }
            out[at..at + n].copy_from_slice(&e.name[..n]);
            at += n;
            out[at] = 0;
            at += 1;
        }
        Ok::<_, crate::fs::ext::FsError>((out, at))
    });

    match result {
        Some(Ok((out, len))) => {
            if buf_len == 0 {
                return len as u64;
            }
            if (len as u64) > buf_len {
                return err(ERANGE);
            }
            let _ua = match user_ptr_writable(cr3, buf_ptr, buf_len) {
                Some(g) => g,
                None => return err(EFAULT),
            };
            unsafe {
                core::ptr::copy_nonoverlapping(out.as_ptr(), buf_ptr as *mut u8, len);
            }
            len as u64
        }
        Some(Err(e)) => vfs_err(e.into()),
        None => err(ENOSYS),
    }
}

// 82  removexattr(ino, name_ptr, name_len) -> 0 / -errno
pub fn sys_removexattr(ino: u64, name_ptr: u64, name_len: u64) -> u64 {
    if ino == 0 || name_len == 0 || name_len > 64 {
        return err(EINVAL);
    }
    let cr3 = current_cr3();
    let _ua = match user_ptr_mapped(cr3, name_ptr, name_len) {
        Some(g) => g,
        None => return err(EFAULT),
    };
    let mut name_buf = [0u8; 64];
    unsafe {
        core::ptr::copy_nonoverlapping(
            name_ptr as *const u8,
            name_buf.as_mut_ptr(),
            name_len as usize,
        );
    }
    let name = match core::str::from_utf8(&name_buf[..name_len as usize]) {
        Ok(s) => s,
        Err(_) => return err(EINVAL),
    };

    let creds = caller_creds();
    let result = crate::fs::ext::mount::with_ext2_pub(|fs| {
        use crate::fs::vfs::permissions::AccessMode;
        may_touch_xattr(fs, ino as u32, &creds, AccessMode::Write)?;
        // Removing something not there is an error, not a no-op, so a caller
        // can tell the two apart
        let mut probe = [0u8; 4];
        match fs.get_xattr(
            ino as u32,
            crate::fs::ext::xattr::XATTR_INDEX_USER,
            name,
            &mut probe,
        ) {
            Ok(_) => {}
            Err(e) => return Err(e),
        }
        fs.ext3_remove_xattr(ino as u32, crate::fs::ext::xattr::XATTR_INDEX_USER, name)
    });
    match result {
        Some(Ok(())) => 0,
        Some(Err(e)) => vfs_err(e.into()),
        None => err(ENOSYS),
    }
}

// 40  utimensat(path_ptr, path_word, times_ptr, flags) -> 0
//
// A file's access and modification times, by path in the usual path word (ABI
// 3.2e); an empty path with a descriptor base is `futimens`. `times_ptr` is two
// `struct timespec` {sec, nsec}, atime then mtime, or 0 for both now; a
// `tv_nsec` of `UTIME_NOW` or `UTIME_OMIT` means what POSIX says. `flags` takes
// `AT_SYMLINK_NOFOLLOW`
pub fn sys_utimensat(path_ptr: u64, path_word: u64, times_ptr: u64, flags: u64) -> u64 {
    const AT_SYMLINK_NOFOLLOW: u64 = miku_abi::fcntl::AT_SYMLINK_NOFOLLOW as u64;
    use miku_abi::stat::UTIME_NOW;
    use miku_abi::stat::UTIME_OMIT;
    if flags & !AT_SYMLINK_NOFOLLOW != 0 { return err(EINVAL); }
    let (path, base) = match read_user_path_at(path_ptr, path_word) {
        Ok(p)  => p,
        Err(e) => return e,
    };
    // Two (sec, nsec) pairs, or none
    let mut times: Option<[(i64, i64); 2]> = None;
    if times_ptr != 0 {
        let cr3 = current_cr3();
        let _ua = match user_ptr_mapped(cr3, times_ptr, 32) {
            Some(g) => g,
            None    => return err(EFAULT),
        };
        let w = |i: u64| unsafe { ((times_ptr + i * 8) as *const i64).read_unaligned() };
        times = Some([(w(0), w(1)), (w(2), w(3))]);
    }

    crate::fs::vfs::core::with_vfs(|vfs| {
        let start = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        let id = if path.is_empty() {
            Ok(start)
        } else if flags & AT_SYMLINK_NOFOLLOW != 0 {
            vfs.resolve_path_lstat(start, &path)
        } else {
            vfs.resolve_path_follow(start, &path)
        };
        let id = match id { Ok(i) => i, Err(e) => return vfs_err(e) };
        let now = vfs.time_now();
        let pick = |(sec, nsec): (i64, i64)| match nsec {
            UTIME_OMIT => None,
            UTIME_NOW => Some(now),
            _ => Some(sec.max(0) as u64),
        };
        let (atime, mtime) = match times {
            None => (Some(now), Some(now)),
            Some([a, m]) => (pick(a), pick(m)),
        };
        if atime.is_none() && mtime.is_none() { return 0; }
        match vfs.set_times(id, atime, mtime) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}
