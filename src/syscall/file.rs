// File-descriptor syscalls: open, close, seek, fsize, dup/dup2, truncate,
// pipe, fallocate, fsync, punch_hole

use super::errno::{err, vfs_err, EBADF, EFAULT, EINVAL, ENOENT, ENOMEM, ENOSPC, ENOSYS, EIO};
use super::user_mem::{current_cr3, read_user_path, user_ptr_mapped, user_ptr_writable};
use crate::vfs::types::{FileMode, OpenFlags, SeekFrom, VfsError, VNodeKind};

// Try opening a path that lives on the active ext-family filesystem,
// importing its inode metadata into the VFS
fn open_from_active_ext(path: &str, flags: OpenFlags, _mode: FileMode) -> Option<u64> {
    let info = crate::commands::ext2_cmds::with_ext2_pub(|fs| -> Result<_, VfsError> {
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

    if flags.has(OpenFlags::CREATE) && flags.has(OpenFlags::EXCLUSIVE) {
        return Some(vfs_err(VfsError::AlreadyExists));
    }
    if flags.has(OpenFlags::DIRECTORY) && kind != VNodeKind::Directory {
        return Some(vfs_err(VfsError::NotDirectory));
    }

    Some(crate::vfs::core::with_vfs(|vfs| {
        let id = match vfs.alloc_vnode() {
            Ok(id) => id,
            Err(e) => return vfs_err(e),
        };

        let ts = crate::vfs::procfs::uptime_ticks();
        let name = path.rsplit('/').next().unwrap_or(path);
        vfs.nodes[id].init(
            id as crate::vfs::InodeId,
            0,
            name,
            kind,
            crate::commands::ext2_cmds::active_fs_type(),
            FileMode::new(inode_mode),
            uid, gid, ts,
        );
        vfs.nodes[id].ext2_ino = ino;
        vfs.nodes[id].size = size;
        vfs.nodes[id].nlinks = 0;
        vfs.nodes[id].children_loaded = false;

        if flags.has(OpenFlags::TRUNCATE) && flags.writable() && kind == VNodeKind::Regular {
            let truncate = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.ext3_truncate(ino));
            if !matches!(truncate, Some(Ok(()))) {
                vfs.nodes[id].active = false;
                return err(EIO);
            }
            vfs.nodes[id].size = 0;
        }

        let fd = match vfs.fds().alloc(id as crate::vfs::InodeId, flags) {
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
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] open '{}' flags={:#x}", path, flags);

    let oflags = if flags == 0 { OpenFlags(OpenFlags::READ) } else { OpenFlags(flags as u32) };
    let fmode  = if mode == 0  { FileMode::default_file() } else { FileMode::new(mode as u16) };

    let path_str: &str = path.as_str();
    let cwd = crate::scheduler::current_cwd() as usize;
    let vfs_result = crate::vfs::core::with_vfs(|vfs| {
        vfs.open(cwd, path_str, oflags, fmode)
    });

    match vfs_result {
        Ok(fd) => {
            crate::serial_println!("[syscall] open '{}' -> vfs fd={}", path_str, fd);
            fd as u64
        }
        Err(_) => {
            if let Some(fd) = open_from_active_ext(path_str, oflags, fmode) {
                crate::serial_println!("[syscall] open '{}' -> ext fd={}", path_str, fd);
                return fd;
            }

            // fallback: load file from ext2/solib into VFS tmpfs
            let data = match crate::vfs_read::read_file_or_solib(path_str) {
                Some(d) => d,
                None    => return err(ENOENT),
            };
            let file_len = data.len();

            crate::vfs::core::with_vfs(|vfs| {
                let fname = path_str.rsplit('/').next().unwrap_or(path_str);
                let parent = 0; // root

                match vfs.create_file(parent, fname, FileMode::default_file()) {
                    Ok(_) => {}
                    Err(VfsError::AlreadyExists) => {
                        return match vfs.open(0, path_str, oflags, fmode) {
                            Ok(fd) => fd as u64,
                            Err(e) => vfs_err(e),
                        };
                    }
                    Err(e) => return vfs_err(e),
                }

                let fl = OpenFlags(OpenFlags::READ | OpenFlags::WRITE);
                let fd = match vfs.open(0, fname, fl, fmode) {
                    Ok(f)  => f,
                    Err(e) => return vfs_err(e),
                };

                if file_len > crate::vfs::address_space::AddressSpace::max_size() as usize {
                    let _ = vfs.close(fd);
                    let _ = vfs.unlink(0, fname);
                    return vfs_err(VfsError::FileTooLarge);
                }

                let wrote = match vfs.write(fd, &data) {
                    Ok(n) if n == file_len => n,
                    Ok(_) | Err(_) => {
                        let _ = vfs.close(fd);
                        let _ = vfs.unlink(0, fname);
                        return err(EIO);
                    }
                };
                let _ = vfs.seek(fd, SeekFrom::Start(0));

                crate::serial_println!(
                    "[syscall] open '{}' -> loaded {} bytes, vfs fd={}",
                    path, wrote, fd
                );
                fd as u64
            })
        }
    }
}

// 12  close(fd) -> 0
pub fn sys_close(fd: u64) -> u64 {
    if fd <= 2 { return 0; } // stdin/stdout/stderr stay open

    // Socket fds close through the socket layer; only the owner may close
    if crate::net::socket::is_socket_fd(fd) {
        let pid = super::user_mem::current_pid();
        if !crate::net::socket::owned_by(fd, pid) {
            return err(EBADF);
        }
        crate::net::socket::close_fd(fd);
        return 0;
    }

    crate::vfs::core::with_vfs(|vfs| match vfs.close(fd as usize) {
        Ok(())  => 0,
        Err(e)  => vfs_err(e),
    })
}

// 13  seek(fd, offset, whence) -> new_offset   (whence: 0=SET, 1=CUR, 2=END)
pub fn sys_seek(fd: u64, offset: u64, whence: u64) -> u64 {
    let seek = match whence {
        0 => SeekFrom::Start(offset),
        1 => SeekFrom::Current(offset as i64),
        2 => SeekFrom::End(offset as i64),
        _ => return err(EINVAL),
    };

    crate::vfs::core::with_vfs(|vfs| match vfs.seek(fd as usize, seek) {
        Ok(pos) => pos,
        Err(e)  => vfs_err(e),
    })
}

// 14  fsize(fd) -> size
pub fn sys_fsize(fd: u64) -> u64 {
    crate::vfs::core::with_vfs(|vfs| match vfs.fstat(fd as usize) {
        Ok(st) => st.size,
        Err(e) => vfs_err(e),
    })
}

// 28  dup(fd) -> new_fd
pub fn sys_dup(fd: u64) -> u64 {
    crate::vfs::core::with_vfs(|vfs| match vfs.dup(fd as usize) {
        Ok(new_fd) => new_fd as u64,
        Err(e)     => vfs_err(e),
    })
}

// 29  dup2(old_fd, new_fd) -> new_fd
pub fn sys_dup2(old_fd: u64, new_fd: u64) -> u64 {
    if old_fd == new_fd { return new_fd; }

    crate::vfs::core::with_vfs(|vfs| match vfs.dup_to(old_fd as usize, new_fd as usize) {
        Ok(fd) => fd as u64,
        Err(e) => vfs_err(e),
    })
}

// 30  truncate(fd, length) -> 0
pub fn sys_truncate(fd: u64, length: u64) -> u64 {
    crate::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fds().get(fd as usize) {
            Ok(f)  => f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        if !vfs.valid_vnode(vid) { return err(EBADF); }
        vfs.truncate_to(vid, length);
        0
    })
}

// 34  pipe(fds_ptr) -> 0   (writes [read_fd, write_fd] as two u64s)
//
// The pipe is created by materialising a file in the root VFS, opening
// it twice (read + write) and immediately unlinking it. A per-call
// atomic counter gives every invocation a unique name so concurrent
// sys_pipe() calls don't collide. Every error path also unlinks the
// temporary file so a failed pipe() can't leave junk in the namespace
pub fn sys_pipe(fds_ptr: u64) -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static PIPE_SEQ: AtomicU64 = AtomicU64::new(0);

    let cr3 = current_cr3();
    if !user_ptr_writable(cr3, fds_ptr, 16) { return err(EFAULT); }

    // Build "_pipe_<hex>" into a fixed-size buffer; no alloc needed
    let seq = PIPE_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name_buf = [0u8; 24];
    let name_len = {
        let prefix = b"_pipe_";
        name_buf[..prefix.len()].copy_from_slice(prefix);
        let mut n = prefix.len();
        let mut started = false;
        for shift in (0..16).rev() {
            let nyb = ((seq >> (shift * 4)) & 0xF) as u8;
            if nyb != 0 || started || shift == 0 {
                name_buf[n] = if nyb < 10 { b'0' + nyb } else { b'a' + nyb - 10 };
                n += 1;
                started = true;
            }
        }
        n
    };
    let pipe_name = match core::str::from_utf8(&name_buf[..name_len]) {
        Ok(s)  => s,
        Err(_) => return err(EIO),
    };

    let (read_fd, write_fd) = match crate::vfs::core::with_vfs(|vfs| -> Result<(usize, usize), u64> {
        if vfs.create_file(0, pipe_name, FileMode::default_pipe()).is_err() {
            return Err(err(ENOMEM));
        }

        let rflags = OpenFlags(OpenFlags::READ);
        let r_fd = match vfs.open(0, pipe_name, rflags, FileMode::default_pipe()) {
            Ok(fd) => fd,
            Err(e) => {
                let _ = vfs.unlink(0, pipe_name);
                return Err(vfs_err(e));
            }
        };

        let wflags = OpenFlags(OpenFlags::WRITE);
        let w_fd = match vfs.open(0, pipe_name, wflags, FileMode::default_pipe()) {
            Ok(fd) => fd,
            Err(e) => {
                let _ = vfs.close(r_fd);
                let _ = vfs.unlink(0, pipe_name);
                return Err(vfs_err(e));
            }
        };

        // Anonymous: detach name now that both fds hold the vnode
        let _ = vfs.unlink(0, pipe_name);
        Ok((r_fd, w_fd))
    }) {
        Ok(pair) => pair,
        Err(e)   => return e,
    };

    // Re-validate user mapping immediately before the store; a concurrent
    // sibling thread on the same address space could have unmapped fds_ptr
    // while VFS work was running. Two-step write-then-revalidate is the
    // best we can do without a fault-aware copy_to_user
    if !user_ptr_writable(cr3, fds_ptr, 16) {
        crate::vfs::core::with_vfs(|vfs| {
            let _ = vfs.close(read_fd);
            let _ = vfs.close(write_fd);
        });
        return err(EFAULT);
    }
    unsafe {
        let p = fds_ptr as *mut u64;
        p.write_unaligned(read_fd as u64);
        p.add(1).write_unaligned(write_fd as u64);
    }

    crate::serial_println!("[syscall] pipe read_fd={} write_fd={}", read_fd, write_fd);
    0
}

// 37  fallocate(fd, offset, len) -> 0
pub fn sys_fallocate(fd: u64, offset: u64, len: u64) -> u64 {
    if len == 0 { return err(EINVAL); }

    crate::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fds().get(fd as usize) {
            Ok(f)  => f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        let ext2_ino = vfs.nodes[vid].ext2_ino;
        if ext2_ino == 0 { return err(ENOSYS); }

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
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
    let valid = crate::vfs::core::with_vfs(|vfs| vfs.fds().get(fd as usize).is_ok());
    if !valid { return err(EBADF); }

    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.sync());
    match result {
        Some(Ok(()))  => 0,
        Some(Err(_))  => err(EIO),
        None          => 0, // no ext fs = nothing to sync
    }
}

// 42  punch_hole(fd, offset, len) -> 0
pub fn sys_punch_hole(fd: u64, offset: u64, len: u64) -> u64 {
    if len == 0 { return err(EINVAL); }

    crate::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fds().get(fd as usize) {
            Ok(f)  => f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        let ext2_ino = vfs.nodes[vid].ext2_ino;
        if ext2_ino == 0 { return err(ENOSYS); }

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            if fs.superblock.has_extents() {
                // ext4: zero whole blocks within the range; full extent-tree
                // surgery is not implemented yet.
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
