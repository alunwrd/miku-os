// Filesystem-namespace syscalls: stat/fstat, mkdir/rmdir/unlink, readdir,
// rename/link/symlink/readlink, chmod/chown, chdir, statfs, xattr, utimensat

extern crate alloc;

use super::abi::{write_stat_to_user, STAT_SIZE, STATFS_SIZE, UDIRENT_SIZE};
use super::errno::{err, vfs_err, EFAULT, EINVAL, ENOENT, ENOSPC, ENOSYS, ENOTDIR};
use super::user_mem::{current_cr3, read_user_path, user_ptr_mapped, user_ptr_writable};
use crate::vfs::types::{DirEntry, FileMode, NAME_LEN};

// 18  stat(path_ptr, path_len, stat_buf) -> 0
pub fn sys_stat(path_ptr: u64, path_len: u64, stat_ptr: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };
    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, stat_ptr, STAT_SIZE) { return err(EFAULT); }

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.stat(cwd, &path) {
            Ok(st) => { write_stat_to_user(stat_ptr, &st); 0 }
            Err(e) => vfs_err(e),
        }
    })
}

// 19  fstat(fd, stat_buf) -> 0
pub fn sys_fstat(fd: u64, stat_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, stat_ptr, STAT_SIZE) { return err(EFAULT); }

    crate::vfs::core::with_vfs(|vfs| match vfs.fstat(fd as usize) {
        Ok(st) => { write_stat_to_user(stat_ptr, &st); 0 }
        Err(e) => vfs_err(e),
    })
}

// Split a path into (parent_path, basename) for parent-relative ops
fn split_parent(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(pos) if pos > 0 => (&path[..pos], &path[pos + 1..]),
        Some(0) => ("/", &path[1..]),
        _ => ("", path),
    }
}

// 20  mkdir(path_ptr, path_len, mode) -> 0
pub fn sys_mkdir(path_ptr: u64, path_len: u64, mode: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] mkdir '{}'", path);

    let (parent_path, dirname) = split_parent(&path);
    let fmode = if mode == 0 { FileMode::default_dir() } else { FileMode::new(mode as u16) };

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        let parent_id = if parent_path.is_empty() || parent_path == "/" { 0 } else {
            match vfs.resolve_path(cwd, parent_path) {
                Ok(id) => id,
                Err(e) => return vfs_err(e),
            }
        };
        match vfs.mkdir(parent_id, dirname, fmode) {
            Ok(_)  => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 21  rmdir(path_ptr, path_len) -> 0
pub fn sys_rmdir(path_ptr: u64, path_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] rmdir '{}'", path);

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.rmdir(cwd, &path) {
            Ok(())  => 0,
            Err(e)  => vfs_err(e),
        }
    })
}

// 22  unlink(path_ptr, path_len) -> 0
pub fn sys_unlink(path_ptr: u64, path_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] unlink '{}'", path);

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.unlink(cwd, &path) {
            Ok(())  => 0,
            Err(e)  => vfs_err(e),
        }
    })
}

// 23  readdir(path_ptr, path_len, buf_ptr, max_entries) -> count
pub fn sys_readdir(path_ptr: u64, path_len: u64, buf_ptr: u64, max_entries: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    if max_entries == 0 { return 0; }
    let total_size = max_entries.saturating_mul(UDIRENT_SIZE);
    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, buf_ptr, total_size) { return err(EFAULT); }

    let count = max_entries.min(64) as usize;
    let mut entries = alloc::vec![DirEntry::empty(); count];

    let cwd = crate::scheduler::current_cwd() as usize;
    let result = crate::vfs::core::with_vfs(|vfs| {
        let dir_id = vfs.resolve_path(cwd, &path)?;
        vfs.readdir(dir_id, &mut entries)
    });

    match result {
        Ok(n) => {
            for i in 0..n {
                unsafe {
                    let base = (buf_ptr + (i as u64) * UDIRENT_SIZE) as *mut u8;
                    core::ptr::write_bytes(base, 0, UDIRENT_SIZE as usize);
                    let nlen = entries[i].name_len as usize;
                    core::ptr::copy_nonoverlapping(
                        entries[i].name.as_ptr(),
                        base,
                        nlen.min(NAME_LEN - 1),
                    );
                    (base.add(64) as *mut u16).write_unaligned(entries[i].inode_id);
                    *base.add(66) = entries[i].kind as u8;
                    *base.add(67) = entries[i].name_len;
                }
            }
            n as u64
        }
        Err(e) => vfs_err(e),
    }
}

// 24  rename(old_ptr, old_len, new_ptr, new_len) -> 0
pub fn sys_rename(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> u64 {
    let old_path = match read_user_path(old_ptr, old_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let new_path = match read_user_path(new_ptr, new_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::serial_println!("[syscall] rename '{}' -> '{}'", old_path, new_path);

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.rename(cwd, &old_path, &new_path) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 25  link(old_ptr, old_len, new_ptr, new_len) -> 0
pub fn sys_link(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> u64 {
    let old_path = match read_user_path(old_ptr, old_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let new_path = match read_user_path(new_ptr, new_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::serial_println!("[syscall] link '{}' -> '{}'", old_path, new_path);

    let (parent_path, linkname) = split_parent(&new_path);

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        let parent_id = if parent_path.is_empty() || parent_path == "/" { 0 } else {
            match vfs.resolve_path(cwd, parent_path) {
                Ok(id) => id,
                Err(e) => return vfs_err(e),
            }
        };
        match vfs.link(cwd, &old_path, parent_id, linkname) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 26  chmod(path_ptr, path_len, mode) -> 0
pub fn sys_chmod(path_ptr: u64, path_len: u64, mode: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.chmod(cwd, &path, FileMode::new(mode as u16)) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 27  chown(path_ptr, path_len, uid, gid) -> 0   (0xFFFF = "no change")
pub fn sys_chown(path_ptr: u64, path_len: u64, uid: u64, gid: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };

    let o_uid = if uid == 0xFFFF { None } else { Some(uid as u16) };
    let o_gid = if gid == 0xFFFF { None } else { Some(gid as u16) };

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.chown(cwd, &path, o_uid, o_gid) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 32  symlink(target_ptr, target_len, link_ptr, link_len) -> 0
pub fn sys_symlink(target_ptr: u64, target_len: u64, link_ptr: u64, link_len: u64) -> u64 {
    let target = match read_user_path(target_ptr, target_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let linkpath = match read_user_path(link_ptr, link_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::serial_println!("[syscall] symlink '{}' -> '{}'", linkpath, target);

    let (parent_path, linkname) = split_parent(&linkpath);

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        let parent_id = if parent_path.is_empty() || parent_path == "/" { 0 } else {
            match vfs.resolve_path(cwd, parent_path) {
                Ok(id) => id,
                Err(e) => return vfs_err(e),
            }
        };
        match vfs.symlink(parent_id, linkname, &target) {
            Ok(_)  => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 33  readlink(path_ptr, path_len, buf_ptr, buf_len) -> bytes_written
pub fn sys_readlink(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };

    if buf_len == 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, buf_ptr, buf_len) { return err(EFAULT); }

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| match vfs.readlink(cwd, &path) {
        Ok(name) => {
            let copy_len = (name.len as usize).min(buf_len as usize);
            unsafe {
                core::ptr::copy_nonoverlapping(name.data.as_ptr(), buf_ptr as *mut u8, copy_len);
                if copy_len < buf_len as usize {
                    *(buf_ptr as *mut u8).add(copy_len) = 0;
                }
            }
            copy_len as u64
        }
        Err(e) => vfs_err(e),
    })
}

// 35  chdir(path_ptr, path_len) -> 0
pub fn sys_chdir(path_ptr: u64, path_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };

    crate::serial_println!("[syscall] chdir '{}'", path);

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| {
        let id = match vfs.resolve_path(cwd, &path) {
            Ok(id) => id,
            Err(e) => return vfs_err(e),
        };
        if !vfs.nodes[id].is_dir() { return err(ENOTDIR); }
        // Per-process cwd lives on the Process struct; vfs.ctx.cwd is
        // kept in sync only as a fallback for non-process kernel callers
        vfs.ctx.cwd = id as u16;
        crate::scheduler::set_current_cwd(id as u64);
        0
    })
}

// 36  statfs(path_ptr, path_len, buf_ptr) -> 0
pub fn sys_statfs(path_ptr: u64, path_len: u64, buf_ptr: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p, Err(e) => return e,
    };
    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, buf_ptr, STATFS_SIZE) { return err(EFAULT); }

    let cwd = crate::scheduler::current_cwd() as usize;
    crate::vfs::core::with_vfs(|vfs| match vfs.statfs(cwd, &path) {
        Ok(sf) => {
            unsafe {
                let p = buf_ptr as *mut u8;
                (p as *mut u32).write(sf.fs_type.magic());
                (p.add(4)  as *mut u32).write(sf.block_size);
                (p.add(8)  as *mut u64).write(sf.total_blocks);
                (p.add(16) as *mut u64).write(sf.free_blocks);
                (p.add(24) as *mut u64).write(sf.total_inodes);
                (p.add(32) as *mut u64).write(sf.free_inodes);
                (p.add(40) as *mut u32).write(sf.max_name_len);
                (p.add(44) as *mut u32).write(sf.flags);
            }
            0
        }
        Err(e) => vfs_err(e),
    })
}

// 38  getxattr(ino, name_ptr, name_len, buf_ptr) -> value_len
pub fn sys_getxattr(ino: u64, name_ptr: u64, name_len: u64, buf_ptr: u64) -> u64 {
    if ino == 0 || name_len == 0 || name_len > 64 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, name_ptr, name_len) { return err(EFAULT); }
    if !user_ptr_writable(cr3, buf_ptr, 256) { return err(EFAULT); }

    // Copy the attribute name into a kernel buffer before parsing; the
    // user could otherwise rewrite it between validation and ext2 use
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

    // Read the xattr into a kernel buffer, then revalidate user_ptr
    // and copy out. ext2 disk I/O may yield, so the user mapping checked
    // above can become stale by the time we'd write back
    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        let mut vbuf = [0u8; 256];
        match fs.get_xattr(
            ino as u32,
            crate::miku_extfs::xattr::XATTR_INDEX_USER,
            name,
            &mut vbuf,
        ) {
            Ok(len) => Ok((vbuf, len)),
            Err(e) => Err(e),
        }
    });
    match result {
        Some(Ok((vbuf, len))) => {
            if !user_ptr_writable(cr3, buf_ptr, 256) { return err(EFAULT); }
            let copy = len.min(256);
            unsafe {
                core::ptr::copy_nonoverlapping(vbuf.as_ptr(), buf_ptr as *mut u8, copy);
            }
            len as u64
        }
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
    if !user_ptr_mapped(cr3, name_ptr, name_len as u64) { return err(EFAULT); }
    if value_len > 0 && !user_ptr_mapped(cr3, value_ptr, value_len as u64) { return err(EFAULT); }

    // Copy name + value into kernel buffers before parsing/storing -
    // the user could otherwise mutate them after validation
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

    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        fs.set_xattr(
            ino as u32,
            crate::miku_extfs::xattr::XATTR_INDEX_USER,
            name,
            value,
        )
    });
    match result {
        Some(Ok(()))  => 0,
        Some(Err(_))  => err(ENOSPC),
        None          => err(ENOSYS),
    }
}

// 40  utimensat(fd_or_ino, atime, mtime) -> 0
//     atime/mtime: 0 = no change, u32::MAX = current time, else = epoch seconds
pub fn sys_utimensat(fd_or_ino: u64, atime: u64, mtime: u64) -> u64 {
    if fd_or_ino == 0 { return err(EINVAL); }

    let ino = fd_or_ino as u32;
    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        fs.utimensat(ino, atime as u32, mtime as u32)
    });
    match result {
        Some(Ok(()))  => 0,
        Some(Err(_))  => err(EINVAL),
        None          => err(ENOSYS),
    }
}
