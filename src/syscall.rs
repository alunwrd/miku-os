extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::{Efer, EferFlags, LStar, Star, SFMask};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;
use crate::gdt;
use crate::mmap;
use crate::vmm::AddressSpace;
use crate::vfs::types::{
    OpenFlags, FileMode, VfsError, SeekFrom, VNodeStat, DirEntry,
    VNodeKind, NAME_LEN,
};

const PAGE_SIZE: u64 = 4096;
const USER_MAX: u64 = 0x0000_7FFF_FFFF_FFFF;

// Saved kernel RSP during syscall dispatch (for fork)
static SYSCALL_FRAME_RSP: AtomicU64 = AtomicU64::new(0);

// error codes (POSIX) //

const EPERM:   i64 = -1;
const ENOENT:  i64 = -2;
const ESRCH:   i64 = -3;
const EBADF:   i64 = -9;
const ENOMEM:  i64 = -12;
const EACCES:  i64 = -13;
const EFAULT:  i64 = -14;
const EEXIST:  i64 = -17;
const ENOTDIR: i64 = -20;
const EISDIR:  i64 = -21;
const EINVAL:  i64 = -22;
const EMFILE:  i64 = -24;
const ENOSPC:  i64 = -28;
const EPIPE:   i64 = -32;
const ERANGE:  i64 = -34;
const ENOSYS:  i64 = -38;
const ENOTEMPTY: i64 = -39;
const ENAMETOOLONG: i64 = -36;

fn err(code: i64) -> u64 { code as u64 }

fn vfs_err(e: VfsError) -> u64 {
    let code = match e {
        VfsError::NotFound        => ENOENT,
        VfsError::PermissionDenied=> EACCES,
        VfsError::AlreadyExists   => EEXIST,
        VfsError::NotDirectory    => ENOTDIR,
        VfsError::IsDirectory     => EISDIR,
        VfsError::NotEmpty        => ENOTEMPTY,
        VfsError::InvalidPath     => EINVAL,
        VfsError::NoSpace         => ENOSPC,
        VfsError::ReadOnly        => EPERM,
        VfsError::InvalidArgument => EINVAL,
        VfsError::BadFd           => EBADF,
        VfsError::TooManyOpenFiles=> EMFILE,
        VfsError::NameTooLong     => ENAMETOOLONG,
        VfsError::BrokenPipe      => EPIPE,
        VfsError::IoError         => -5,  // EIO
        VfsError::NotSupported    => ENOSYS,
        _                         => EINVAL,
    };
    err(code)
}

// init //

pub fn init() {
    unsafe {
        Efer::update(|f| *f |= EferFlags::SYSTEM_CALL_EXTENSIONS | EferFlags::NO_EXECUTE_ENABLE);
    }
    Star::write(
        gdt::GDT.1.user_code,
        gdt::user_data_selector(),
        gdt::kernel_code_selector(),
        gdt::kernel_data_selector(),
    ).unwrap();
    LStar::write(VirtAddr::new(syscall_handler as *const () as u64));
    SFMask::write(RFlags::INTERRUPT_FLAG);
    crate::serial_println!("[syscall] MikuOS syscall table ready (46 entries)");
}

// naked handler (ABI bridge) //

#[unsafe(naked)]
unsafe extern "C" fn syscall_handler() {
    core::arch::naked_asm!(
        "swapgs",
        "mov gs:[8], rsp",
        "mov rsp, gs:[0]",
        "push rcx",
        "push r11",
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push r10",
        "push r9",
        "push r8",
        "mov r8,  r10",
        "mov rcx, rdx",
        "mov rdx, rsi",
        "mov rsi, rdi",
        "mov rdi, rax",
        "call {handler}",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",
        "pop rcx",
        "mov rsp, gs:[8]",
        "swapgs",
        "sysretq",
        handler = sym dispatch,
    );
}

// helpers //

fn current_cr3() -> u64 {
    let (frame, _) = x86_64::registers::control::Cr3::read();
    frame.start_address().as_u64()
}

fn current_pid() -> u64 {
    crate::scheduler::current_pid()
}

fn user_ptr_mapped(cr3: u64, ptr: u64, len: u64) -> bool {
    if ptr == 0 || len == 0 { return false; }
    if ptr > USER_MAX { return false; }
    let end = match ptr.checked_add(len) {
        Some(e) if e <= USER_MAX + 1 => e,
        _ => return false,
    };
    let aspace = AddressSpace::from_raw(cr3);
    let start_page = ptr & !0xFFF;
    let end_page = (end + 0xFFF) & !0xFFF;
    let mut va = start_page;
    let mut ok = true;
    while va < end_page {
        if aspace.virt_to_phys(va).is_none() { ok = false; break; }
        va += PAGE_SIZE;
    }
    let _ = aspace.into_raw();
    ok
}

fn read_user_path(ptr: u64, len: u64) -> Result<&'static str, u64> {
    if len == 0 || len > 4096 { return Err(err(EINVAL)); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, ptr, len) { return Err(err(EFAULT)); }
    let bytes = unsafe {
        core::slice::from_raw_parts(ptr as *const u8, len as usize)
    };
    match core::str::from_utf8(bytes) {
        Ok(s) => Ok(s.trim_end_matches('\0')),
        Err(_) => Err(err(EINVAL)),
    }
}

////////////////////////////////////////////////////////////////////////////
//                       dispatch table                                   //
////////////////////////////////////////////////////////////////////////////
// Syscall numbers:														  //
//  0  exit           16  sleep           32  symlink					  //
//  1  write          17  uptime          33  readlink                    //
//  2  read           18  stat            34  pipe (reserved)             //
//  3  mmap           19  fstat           35  kill (reserved)             //
//  4  munmap         20  mkdir											  //
//  5  mprotect       21  rmdir											  //
//  6  brk            22  unlink                                          //
//  7  getpid         23  readdir                                         //
//  8  getcwd         24  rename                                          //
//  9  set_tls        25  link                                            //
// 10  get_tls        26  chmod											  //
// 11  open           27  chown                                           //
// 12  close          28  dup                                             //
// 13  seek           29  dup2                                            //
// 14  fsize          30  truncate                                        //
// 15  map_lib        31  write_file                                      //
////////////////////////////////////////////////////////////////////////////

extern "C" fn dispatch(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    match nr {
        0  => sys_exit(a1),
        1  => sys_write(a1, a2, a3),
        2  => sys_read(a1, a2, a3),
        3  => sys_mmap(a1, a2, a3, a4),
        4  => sys_munmap(a1, a2),
        5  => sys_mprotect(a1, a2, a3),
        6  => sys_brk(a1),
        7  => current_pid(),
        8  => sys_getcwd(a1, a2),
        9  => sys_set_tls(a1),
        10 => sys_get_tls(),
        11 => sys_open(a1, a2, a3, a4),
        12 => sys_close(a1),
        13 => sys_seek(a1, a2, a3),
        14 => sys_fsize(a1),
        15 => sys_map_lib(a1, a2),
        16 => sys_sleep(a1),
        17 => sys_uptime(),
        18 => sys_stat(a1, a2, a3),
        19 => sys_fstat(a1, a2),
        20 => sys_mkdir(a1, a2, a3),
        21 => sys_rmdir(a1, a2),
        22 => sys_unlink(a1, a2),
        23 => sys_readdir(a1, a2, a3, a4),
        24 => sys_rename(a1, a2, a3, a4),
        25 => sys_link(a1, a2, a3, a4),
        26 => sys_chmod(a1, a2, a3),
        27 => sys_chown(a1, a2, a3, a4),
        28 => sys_dup(a1),
        29 => sys_dup2(a1, a2),
        30 => sys_truncate(a1, a2),
        31 => sys_write_file(a1, a2, a3),
        32 => sys_symlink(a1, a2, a3, a4),
        33 => sys_readlink(a1, a2, a3, a4),
        34 => sys_pipe(a1),
        35 => sys_chdir(a1, a2),
        36 => sys_statfs(a1, a2, a3),
        37 => sys_fallocate(a1, a2, a3),
        38 => sys_getxattr(a1, a2, a3, a4),
        39 => sys_setxattr(a1, a2, a3, a4),
        40 => sys_utimensat(a1, a2, a3),
        41 => sys_fsync(a1),
        42 => sys_punch_hole(a1, a2, a3),
        43 => sys_fork(),
        44 => sys_wait4(a1, a2, a3),
        45 => sys_kill(a1, a2),
        46 => sys_exec(a1, a2, a3, a4),
        _ => {
            crate::serial_println!("[syscall] unknown nr={}", nr);
            err(ENOSYS)
        }
    }
}

//  0  exit  //

fn sys_exit(code: u64) -> u64 {
    let pid = current_pid();
    crate::serial_println!("[syscall] exit pid={} code={}", pid, code);
    crate::scheduler::kill_with_code(pid, code);
    crate::signal::send_sigchld(pid);
    crate::scheduler::yield_now();
    0
}

//  1  write(fd, buf_ptr, len) -> bytes_written //

fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    if len > 65536 { return err(EINVAL); }

    let cr3 = current_cr3();
    let mapped = user_ptr_mapped(cr3, ptr, len);

    if !mapped { return err(EFAULT); }

    // stdout/stderr - write to console
    if fd == 1 || fd == 2 {
        let s = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        match core::str::from_utf8(s) {
            Ok(t) => crate::print!("{}", t),
            Err(_) => {
                for &b in s { crate::print!("{}", b as char); }
            }
        }
        return len;
    }

    // VFS file write
    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.write(fd as usize, buf) {
            Ok(n) => n as u64,
            Err(e) => vfs_err(e),
        }
    })
}

//  2  read(fd, buf_ptr, len) -> bytes_read  //

fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    let cr3 = current_cr3();
    let mapped = user_ptr_mapped(cr3, buf, len);
    crate::serial_println!("[sys_read] fd={} buf={:#x} len={} mapped={}", fd, buf, len, mapped);
    if !mapped { return err(EFAULT); }

    // stdin - keyboard input
    if fd == 0 {
        return crate::user_stdin::read(buf, len);
    }

    let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
    let result = crate::vfs::core::with_vfs(|vfs| {
        match vfs.read(fd as usize, slice) {
            Ok(n) => n as u64,
            Err(e) => { crate::serial_println!("[sys_read] vfs err {:?}", e); vfs_err(e) },
        }
    });
    crate::serial_println!("[sys_read] fd={} -> {}", fd, result as i64);
    result
}

fn open_from_active_ext(path: &str, flags: OpenFlags, _mode: FileMode) -> Option<u64> {
    let info = crate::commands::ext2_cmds::with_ext2_pub(|fs| -> Result<_, VfsError> {
        let ino = fs.resolve_path(path).map_err(|_| VfsError::NotFound)?;
        let inode = fs.read_inode(ino).map_err(|_| VfsError::IoError)?;
        let kind = if inode.is_directory() {
            VNodeKind::Directory
        } else if inode.is_symlink() {
            VNodeKind::Symlink
        } else {
            VNodeKind::Regular
        };
        Ok((
            ino,
            kind,
            inode.permissions(),
            inode.uid(),
            inode.gid(),
            inode.size(),
        ))
    })?;

    let (ino, kind, inode_mode, uid, gid, size) = match info {
        Ok(info) => info,
        Err(_) => return None,
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
            uid,
            gid,
            ts,
        );
        vfs.nodes[id].ext2_ino = ino;
        vfs.nodes[id].size = size;
        vfs.nodes[id].nlinks = 0;
        vfs.nodes[id].children_loaded = false;

        if flags.has(OpenFlags::TRUNCATE) && flags.writable() && kind == VNodeKind::Regular {
            let truncate = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.ext3_truncate(ino));
            if !matches!(truncate, Some(Ok(()))) {
                vfs.nodes[id].active = false;
                return err(-5);
            }
            vfs.nodes[id].size = 0;
        }

        let fd = match vfs.fd_table.alloc(id as crate::vfs::InodeId, flags) {
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

// 11  open(path_ptr, path_len, flags, mode) -> fd //

fn sys_open(path_ptr: u64, path_len: u64, flags: u64, mode: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] open '{}' flags={:#x}", path, flags);

    // first try VFS
    let oflags = if flags == 0 {
        OpenFlags(OpenFlags::READ)
    } else {
        OpenFlags(flags as u32)
    };
    let fmode = if mode == 0 {
        FileMode::default_file()
    } else {
        FileMode::new(mode as u16)
    };

    let vfs_result = crate::vfs::core::with_vfs(|vfs| {
        vfs.open(0, path, oflags, fmode)
    });

    match vfs_result {
        Ok(fd) => {
            crate::serial_println!("[syscall] open '{}' -> vfs fd={}", path, fd);
            fd as u64
        }
        Err(_) => {
            if let Some(fd) = open_from_active_ext(path, oflags, fmode) {
                crate::serial_println!("[syscall] open '{}' -> ext fd={}", path, fd);
                return fd;
            }

            // fallback: try loading file from ext2/solib into VFS tmpfs
            let data = match crate::vfs_read::read_file_or_solib(path) {
                Some(d) => d,
                None => return err(ENOENT),
            };
            let file_len = data.len();

            // create file in VFS tmpfs and return fd
            crate::vfs::core::with_vfs(|vfs| {
                // create temporary node under root
                let fname = path.rsplit('/').next().unwrap_or(path);
                let parent = 0; // root

                let vid = match vfs.create_file(parent, fname, FileMode::default_file()) {
                    Ok(id) => id,
                    Err(VfsError::AlreadyExists) => {
                        // file already in VFS, just open it
                        match vfs.open(0, path, oflags, fmode) {
                            Ok(fd) => return fd as u64,
                            Err(e) => return vfs_err(e),
                        }
                    }
                    Err(e) => return vfs_err(e),
                };

                // write data into VFS node
                let fl = OpenFlags(OpenFlags::READ | OpenFlags::WRITE);
                let fd = match vfs.open(0, fname, fl, fmode) {
                    Ok(f) => f,
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
                        return err(-5);
                    }
                };
                // rewind to beginning
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

// 12  close(fd) -> 0 //

fn sys_close(fd: u64) -> u64 {
    // don't close stdin/stdout/stderr
    if fd <= 2 { return 0; }

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.close(fd as usize) {
            Ok(()) => 0u64,
            Err(e) => vfs_err(e),
        }
    })
}

// 13  seek(fd, offset, whence) -> new_offset   //
//     whence: 0=SET, 1=CUR, 2=END				//

fn sys_seek(fd: u64, offset: u64, whence: u64) -> u64 {
    let seek = match whence {
        0 => SeekFrom::Start(offset),
        1 => SeekFrom::Current(offset as i64),
        2 => SeekFrom::End(offset as i64),
        _ => return err(EINVAL),
    };

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.seek(fd as usize, seek) {
            Ok(pos) => pos,
            Err(e) => vfs_err(e),
        }
    })
}

// 14  fsize(fd) -> size //

fn sys_fsize(fd: u64) -> u64 {
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.fstat(fd as usize) {
            Ok(st) => st.size,
            Err(e) => vfs_err(e),
        }
    })
}

// 18  stat(path_ptr, path_len, stat_buf) -> 0 //

// Userspace stat struct layout (C ABI, 64 bytes):
// offset  0: u64  size
// offset  8: u32  mode
// offset 12: u32  nlinks
// offset 16: u16  uid
// offset 18: u16  gid
// offset 20: u8   kind
// offset 21: u8   fs_type
// offset 22: u8   dev_major
// offset 23: u8   dev_minor
// offset 24: u64  atime
// offset 32: u64  mtime
// offset 40: u64  ctime
// offset 48: u64  inode_id
// offset 56: u32  blocks
// offset 60: [4 reserved]

const STAT_SIZE: u64 = 64;

fn write_stat_to_user(ptr: u64, st: &VNodeStat) {
    unsafe {
        let p = ptr as *mut u8;
        (p as *mut u64).write_unaligned(st.size);
        (p.add(8) as *mut u32).write_unaligned(st.mode.0 as u32);
        (p.add(12) as *mut u32).write_unaligned(st.nlinks as u32);
        (p.add(16) as *mut u16).write_unaligned(st.uid);
        (p.add(18) as *mut u16).write_unaligned(st.gid);
        *p.add(20) = st.kind as u8;
        *p.add(21) = st.fs_type as u8;
        *p.add(22) = st.dev_major;
        *p.add(23) = st.dev_minor;
        (p.add(24) as *mut u64).write_unaligned(st.atime);
        (p.add(32) as *mut u64).write_unaligned(st.mtime);
        (p.add(40) as *mut u64).write_unaligned(st.ctime);
        (p.add(48) as *mut u64).write_unaligned(st.id as u64);
        (p.add(56) as *mut u32).write_unaligned(st.blocks);
    }
}

fn sys_stat(path_ptr: u64, path_len: u64, stat_ptr: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, stat_ptr, STAT_SIZE) { return err(EFAULT); }

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.stat(0, path) {
            Ok(st) => {
                write_stat_to_user(stat_ptr, &st);
                0
            }
            Err(e) => vfs_err(e),
        }
    })
}

// 19  fstat(fd, stat_buf) -> 0 //

fn sys_fstat(fd: u64, stat_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, stat_ptr, STAT_SIZE) { return err(EFAULT); }

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.fstat(fd as usize) {
            Ok(st) => {
                write_stat_to_user(stat_ptr, &st);
                0
            }
            Err(e) => vfs_err(e),
        }
    })
}

// 20  mkdir(path_ptr, path_len, mode) -> 0 //

fn sys_mkdir(path_ptr: u64, path_len: u64, mode: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] mkdir '{}'", path);

    // split into parent path and dirname
    let (parent_path, dirname) = match path.rfind('/') {
        Some(pos) if pos > 0 => (&path[..pos], &path[pos + 1..]),
        Some(0) => ("/", &path[1..]),
        _ => ("", path),
    };

    let fmode = if mode == 0 {
        FileMode::default_dir()
    } else {
        FileMode::new(mode as u16)
    };

    crate::vfs::core::with_vfs(|vfs| {
        let parent_id = if parent_path.is_empty() || parent_path == "/" {
            0
        } else {
            match vfs.resolve_path(0, parent_path) {
                Ok(id) => id,
                Err(e) => return vfs_err(e),
            }
        };
        match vfs.mkdir(parent_id, dirname, fmode) {
            Ok(_) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 21  rmdir(path_ptr, path_len) -> 0 //

fn sys_rmdir(path_ptr: u64, path_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] rmdir '{}'", path);

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.rmdir(0, path) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 22  unlink(path_ptr, path_len) -> 0 //

fn sys_unlink(path_ptr: u64, path_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] unlink '{}'", path);

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.unlink(0, path) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 23  readdir(fd_or_path_ptr, path_len, buf_ptr, max_entries) -> count //
//
//  Userspace DirEntry layout (72 bytes each):
//  offset  0: [64] name (null-terminated)
//  offset 64: u16  inode_id
//  offset 66: u8   kind
//  offset 67: u8   name_len
//  offset 68: u32  reserved

const UDIRENT_SIZE: u64 = 72;

fn sys_readdir(path_ptr: u64, path_len: u64, buf_ptr: u64, max_entries: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if max_entries == 0 { return 0; }
    let total_size = max_entries.saturating_mul(UDIRENT_SIZE);
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, buf_ptr, total_size) { return err(EFAULT); }

    let count = max_entries.min(64) as usize;
    let mut entries = alloc::vec![DirEntry::empty(); count];

    let result = crate::vfs::core::with_vfs(|vfs| {
        let dir_id = match vfs.resolve_path(0, path) {
            Ok(id) => id,
            Err(e) => return Err(e),
        };
        vfs.readdir(dir_id, &mut entries)
    });

    match result {
        Ok(n) => {
            // copy entries to user buffer
            for i in 0..n {
                unsafe {
                    let base = (buf_ptr + (i as u64) * UDIRENT_SIZE) as *mut u8;
                    // zero the entry first
                    core::ptr::write_bytes(base, 0, UDIRENT_SIZE as usize);
                    // copy name
                    let nlen = entries[i].name_len as usize;
                    core::ptr::copy_nonoverlapping(
                        entries[i].name.as_ptr(),
                        base,
                        nlen.min(NAME_LEN - 1),
                    );
                    // inode_id
                    (base.add(64) as *mut u16).write_unaligned(entries[i].inode_id);
                    // kind
                    *base.add(66) = entries[i].kind as u8;
                    // name_len
                    *base.add(67) = entries[i].name_len;
                }
            }
            n as u64
        }
        Err(e) => vfs_err(e),
    }
}

// 24  rename(old_ptr, old_len, new_ptr, new_len) -> 0 //

fn sys_rename(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> u64 {
    let old_path = match read_user_path(old_ptr, old_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let new_path = match read_user_path(new_ptr, new_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] rename '{}' -> '{}'", old_path, new_path);

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.rename(0, old_path, new_path) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 25  link(old_ptr, old_len, new_ptr, new_len) -> 0 //

fn sys_link(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> u64 {
    let old_path = match read_user_path(old_ptr, old_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let new_path = match read_user_path(new_ptr, new_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] link '{}' -> '{}'", old_path, new_path);

    let (parent_path, linkname) = match new_path.rfind('/') {
        Some(pos) if pos > 0 => (&new_path[..pos], &new_path[pos + 1..]),
        Some(0) => ("/", &new_path[1..]),
        _ => ("", new_path),
    };

    crate::vfs::core::with_vfs(|vfs| {
        let parent_id = if parent_path.is_empty() || parent_path == "/" {
            0
        } else {
            match vfs.resolve_path(0, parent_path) {
                Ok(id) => id,
                Err(e) => return vfs_err(e),
            }
        };
        match vfs.link(0, old_path, parent_id, linkname) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 26  chmod(path_ptr, path_len, mode) -> 0 //

fn sys_chmod(path_ptr: u64, path_len: u64, mode: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.chmod(0, path, FileMode::new(mode as u16)) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 27  chown(path_ptr, path_len, uid, gid) -> 0   //
//     uid/gid = 0xFFFF means "don't change"      //

fn sys_chown(path_ptr: u64, path_len: u64, uid: u64, gid: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let o_uid = if uid == 0xFFFF { None } else { Some(uid as u16) };
    let o_gid = if gid == 0xFFFF { None } else { Some(gid as u16) };

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.chown(0, path, o_uid, o_gid) {
            Ok(()) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 28  dup(fd) -> new_fd //

fn sys_dup(fd: u64) -> u64 {
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.dup(fd as usize) {
            Ok(new_fd) => new_fd as u64,
            Err(e) => vfs_err(e),
        }
    })
}

// 29  dup2(old_fd, new_fd) -> new_fd //

fn sys_dup2(old_fd: u64, new_fd: u64) -> u64 {
    if old_fd == new_fd { return new_fd; }

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.dup_to(old_fd as usize, new_fd as usize) {
            Ok(fd) => fd as u64,
            Err(e) => vfs_err(e),
        }
    })
}

// 30  truncate(fd, length) -> 0 //

fn sys_truncate(fd: u64, length: u64) -> u64 {
    crate::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fd_table.get(fd as usize) {
            Ok(f) => f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        if !vfs.valid_vnode(vid) { return err(EBADF); }
        vfs.truncate_to(vid, length);
        0
    })
}

// 31  write_file(fd, buf_ptr, len) -> bytes_written               //
//     (same as write but specifically for VFS files, no console)  //

fn sys_write_file(fd: u64, ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    if len > 65536 { return err(EINVAL); }

    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, ptr, len) { return err(EFAULT); }

    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::vfs::core::with_vfs(|vfs| {
        match vfs.write(fd as usize, buf) {
            Ok(n) => n as u64,
            Err(e) => vfs_err(e),
        }
    })
}

// 32  symlink(target_ptr, target_len, link_ptr, link_len) -> 0 //

fn sys_symlink(target_ptr: u64, target_len: u64, link_ptr: u64, link_len: u64) -> u64 {
    let target = match read_user_path(target_ptr, target_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let linkpath = match read_user_path(link_ptr, link_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] symlink '{}' -> '{}'", linkpath, target);

    let (parent_path, linkname) = match linkpath.rfind('/') {
        Some(pos) if pos > 0 => (&linkpath[..pos], &linkpath[pos + 1..]),
        Some(0) => ("/", &linkpath[1..]),
        _ => ("", linkpath),
    };

    crate::vfs::core::with_vfs(|vfs| {
        let parent_id = if parent_path.is_empty() || parent_path == "/" {
            0
        } else {
            match vfs.resolve_path(0, parent_path) {
                Ok(id) => id,
                Err(e) => return vfs_err(e),
            }
        };
        match vfs.symlink(parent_id, linkname, target) {
            Ok(_) => 0,
            Err(e) => vfs_err(e),
        }
    })
}

// 33  readlink(path_ptr, path_len, buf_ptr, buf_len) -> target_len //

fn sys_readlink(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if buf_len == 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, buf_ptr, buf_len) { return err(EFAULT); }

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.readlink(0, path) {
            Ok(name) => {
                let copy_len = (name.len as usize).min(buf_len as usize);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        name.data.as_ptr(),
                        buf_ptr as *mut u8,
                        copy_len,
                    );
                    // null-terminate if space
                    if copy_len < buf_len as usize {
                        *(buf_ptr as *mut u8).add(copy_len) = 0;
                    }
                }
                copy_len as u64
            }
            Err(e) => vfs_err(e),
        }
    })
}

// 34  pipe(fds_ptr) -> 0                             //
//     fds_ptr points to [u64; 2]: read_fd, write_fd  //

fn sys_pipe(fds_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, fds_ptr, 16) { return err(EFAULT); }

    // Create a pipe via VFS - pipe is a special vnode
    crate::vfs::core::with_vfs(|vfs| {
        // create pipe node
        let pipe_name = "_pipe";
        let vid = match vfs.create_file(0, pipe_name, FileMode::default_pipe()) {
            Ok(id) => id,
            Err(_) => return err(ENOMEM),
        };

        // open read end
        let rflags = OpenFlags(OpenFlags::READ);
        let read_fd = match vfs.open(0, pipe_name, rflags, FileMode::default_pipe()) {
            Ok(fd) => fd,
            Err(e) => return vfs_err(e),
        };

        // open write end
        let wflags = OpenFlags(OpenFlags::WRITE);
        let write_fd = match vfs.open(0, pipe_name, wflags, FileMode::default_pipe()) {
            Ok(fd) => fd,
            Err(e) => {
                let _ = vfs.close(read_fd);
                return vfs_err(e);
            }
        };

        // remove from directory (anonymous pipe)
        let _ = vfs.unlink(0, pipe_name);

        unsafe {
            let p = fds_ptr as *mut u64;
            p.write_unaligned(read_fd as u64);
            p.add(1).write_unaligned(write_fd as u64);
        }

        crate::serial_println!("[syscall] pipe read_fd={} write_fd={}", read_fd, write_fd);
        0
    })
}

// 35  chdir(path_ptr, path_len) -> 0 //

fn sys_chdir(path_ptr: u64, path_len: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    crate::serial_println!("[syscall] chdir '{}'", path);

    crate::vfs::core::with_vfs(|vfs| {
        let id = match vfs.resolve_path(0, path) {
            Ok(id) => id,
            Err(e) => return vfs_err(e),
        };
        // check it's a directory
        if !vfs.nodes[id].is_dir() {
            return err(ENOTDIR);
        }
        vfs.ctx.cwd = id as u16;
        0
    })
}

// 36  statfs(path_ptr, path_len, buf_ptr) -> 0 //

// user-space StatFs layout (48 bytes):
//   0: u32 fs_type
//   4: u32 block_size
//   8: u64 total_blocks
//  16: u64 free_blocks
//  24: u64 total_inodes
//  32: u64 free_inodes
//  40: u32 max_name_len
//  44: u32 flags
const STATFS_SIZE: u64 = 48;

fn sys_statfs(path_ptr: u64, path_len: u64, buf_ptr: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, buf_ptr, STATFS_SIZE) { return err(EFAULT); }

    crate::vfs::core::with_vfs(|vfs| {
        match vfs.statfs(0, path) {
            Ok(sf) => {
                unsafe {
                    let p = buf_ptr as *mut u8;
                    (p as *mut u32).write(sf.fs_type.magic());
                    (p.add(4) as *mut u32).write(sf.block_size);
                    (p.add(8) as *mut u64).write(sf.total_blocks);
                    (p.add(16) as *mut u64).write(sf.free_blocks);
                    (p.add(24) as *mut u64).write(sf.total_inodes);
                    (p.add(32) as *mut u64).write(sf.free_inodes);
                    (p.add(40) as *mut u32).write(sf.max_name_len);
                    (p.add(44) as *mut u32).write(sf.flags);
                }
                0
            }
            Err(e) => vfs_err(e),
        }
    })
}

// 37  fallocate(fd, offset, len) -> 0 //

fn sys_fallocate(fd: u64, offset: u64, len: u64) -> u64 {
    if len == 0 { return err(EINVAL); }

    crate::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fd_table.get(fd as usize) {
            Ok(f) => f,
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
            Some(Ok(())) => 0,
            Some(Err(_)) => err(ENOSPC),
            None => err(ENOSYS),
        }
    })
}

// 38  getxattr(inode_num, name_ptr, name_len, buf_ptr) -> value_len or -errno //

fn sys_getxattr(ino: u64, name_ptr: u64, name_len: u64, buf_ptr: u64) -> u64 {
    if ino == 0 || name_len == 0 || name_len > 64 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, name_ptr, name_len) { return err(EFAULT); }
    if !user_ptr_mapped(cr3, buf_ptr, 256) { return err(EFAULT); }

    let name_bytes = unsafe {
        core::slice::from_raw_parts(name_ptr as *const u8, name_len as usize)
    };
    let name = match core::str::from_utf8(name_bytes) {
        Ok(s) => s,
        Err(_) => return err(EINVAL),
    };

    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        let mut vbuf = [0u8; 256];
        match fs.get_xattr(ino as u32, crate::miku_extfs::xattr::XATTR_INDEX_USER, name, &mut vbuf) {
            Ok(len) => {
                unsafe {
                    let dst = buf_ptr as *mut u8;
                    let copy = len.min(256);
                    core::ptr::copy_nonoverlapping(vbuf.as_ptr(), dst, copy);
                }
                Ok(len)
            }
            Err(e) => Err(e),
        }
    });
    match result {
        Some(Ok(len)) => len as u64,
        Some(Err(_)) => err(ENOENT),
        None => err(ENOSYS),
    }
}

// 39  setxattr(inode_num, name_ptr, value_ptr, sizes) -> 0     //
//     sizes = (name_len << 16) | value_len                     //

fn sys_setxattr(ino: u64, name_ptr: u64, value_ptr: u64, sizes: u64) -> u64 {
    let name_len = (sizes >> 16) as usize;
    let value_len = (sizes & 0xFFFF) as usize;
    if ino == 0 || name_len == 0 || name_len > 64 || value_len > 256 {
        return err(EINVAL);
    }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, name_ptr, name_len as u64) { return err(EFAULT); }
    if value_len > 0 && !user_ptr_mapped(cr3, value_ptr, value_len as u64) { return err(EFAULT); }

    let name_bytes = unsafe {
        core::slice::from_raw_parts(name_ptr as *const u8, name_len)
    };
    let name = match core::str::from_utf8(name_bytes) {
        Ok(s) => s,
        Err(_) => return err(EINVAL),
    };
    let value = if value_len > 0 {
        unsafe { core::slice::from_raw_parts(value_ptr as *const u8, value_len) }
    } else {
        &[]
    };

    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        fs.set_xattr(ino as u32, crate::miku_extfs::xattr::XATTR_INDEX_USER, name, value)
    });
    match result {
        Some(Ok(())) => 0,
        Some(Err(_)) => err(ENOSPC),
        None => err(ENOSYS),
    }
}

// 40  utimensat(fd_or_ino, atime, mtime) -> 0                                       //
//     atime/mtime: 0 = no change, u32::MAX = current time, else = epoch seconds     //

fn sys_utimensat(fd_or_ino: u64, atime: u64, mtime: u64) -> u64 {
    if fd_or_ino == 0 { return err(EINVAL); }

    // interpret fd_or_ino as ext2 inode number directly for now
    let ino = fd_or_ino as u32;
    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        fs.utimensat(ino, atime as u32, mtime as u32)
    });
    match result {
        Some(Ok(())) => 0,
        Some(Err(_)) => err(EINVAL),
        None => err(ENOSYS),
    }
}

// 41  fsync(fd) -> 0 //

fn sys_fsync(fd: u64) -> u64 {
    // validate fd exists
    let valid = crate::vfs::core::with_vfs(|vfs| {
        vfs.fd_table.get(fd as usize).is_ok()
    });
    if !valid { return err(EBADF); }

    // flush all dirty blocks to disk (per-file granularity not yet supported)
    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        fs.sync()
    });
    match result {
        Some(Ok(())) => 0,
        Some(Err(_)) => err(-5i64), // EIO
        None => 0, // no ext fs = nothing to sync
    }
}

// 42  punch_hole(fd, offset, len) -> 0  //

fn sys_punch_hole(fd: u64, offset: u64, len: u64) -> u64 {
    if len == 0 { return err(EINVAL); }

    crate::vfs::core::with_vfs(|vfs| {
        let f = match vfs.fd_table.get(fd as usize) {
            Ok(f) => f,
            Err(e) => return vfs_err(e),
        };
        let vid = f.vnode_id as usize;
        let ext2_ino = vfs.nodes[vid].ext2_ino;
        if ext2_ino == 0 { return err(ENOSYS); }

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            if fs.superblock.has_extents() {
                // ext4: for now just zero the range (full extent punch is complex)
                let bs = fs.block_size as u64;
                let start_block = ((offset + bs - 1) / bs) as u32;
                let end_block = ((offset + len) / bs) as u32;
                for logical in start_block..end_block {
                    if let Ok(phys) = fs.get_file_block_any(ext2_ino, logical) {
                        if phys != 0 {
                            let _ = fs.zero_block(phys);
                        }
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
            Some(Ok(())) => 0,
            Some(Err(_)) => err(ENOSPC),
            None => err(ENOSYS),
        }
    })
}

fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64) -> u64 {
    if len == 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let mflags = (flags as u32) | 0x20;
    let result = mmap::sys_mmap(cr3, addr, len, prot as u32, mflags, -1, 0);
    if result < 0 { err(result as i64) } else { result as u64 }
}

fn sys_munmap(addr: u64, len: u64) -> u64 {
    if addr & 0xFFF != 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let result = mmap::sys_munmap(cr3, addr, len);
    if result < 0 { err(result as i64) } else { 0 }
}

fn sys_mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    if addr & 0xFFF != 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let result = mmap::sys_mprotect(cr3, addr, len, prot as u32);
    if result < 0 { err(result as i64) } else { 0 }
}

fn sys_brk(addr: u64) -> u64 {
    mmap::sys_brk(current_cr3(), addr)
}

fn sys_getcwd(buf: u64, size: u64) -> u64 {
    if size < 2 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, buf, size) { return err(EFAULT); }
    unsafe {
        let p = buf as *mut u8;
        p.write(b'/');
        p.add(1).write(0);
    }
    buf
}

fn sys_set_tls(addr: u64) -> u64 {
    x86_64::registers::model_specific::FsBase::write(VirtAddr::new(addr));
    crate::serial_println!("[syscall] set_tls={:#x}", addr);
    0
}

fn sys_get_tls() -> u64 {
    x86_64::registers::model_specific::FsBase::read().as_u64()
}

fn sys_map_lib(name_ptr: u64, name_len: u64) -> u64 {
    if name_len == 0 || name_len > 256 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, name_ptr, name_len) { return err(EFAULT); }
    let name_bytes = unsafe {
        core::slice::from_raw_parts(name_ptr as *const u8, name_len as usize)
    };
    let soname = match core::str::from_utf8(name_bytes) {
        Ok(s) => s.trim_end_matches('\0'),
        Err(_) => return err(EINVAL),
    };
    match crate::solib::map_into_process(soname, cr3) {
        Ok(base) => base,
        Err(e) => e as u64,
    }
}

fn sys_sleep(ticks: u64) -> u64 {
    if ticks == 0 {
        crate::scheduler::yield_now();
        return 0;
    }
    let clamped = ticks.min(100_000);
    crate::scheduler::sleep(clamped);
    0
}

fn sys_uptime() -> u64 {
    crate::interrupts::get_tick()
}

//  43  fork() -> child_pid (parent) / 0 (child)  //

fn sys_fork() -> u64 {
    let cr3 = current_cr3();
    let pid = current_pid();

    // Cannot fork kernel threads
    if cr3 == crate::vmm::kernel_cr3() {
        return err(EPERM);
    }

    // Read saved registers from syscall handler's kernel stack
    // gs:[0] = kernel stack top, gs:[8] = user RSP
    // The syscall_handler pushes in order:
    //   rcx, r11, rbp, rbx, r12, r13, r14, r15, r10, r9, r8
    // So from kernel_stack_top: [top-8]=rcx, [top-16]=r11, [top-24]=rbp, ...
    let kernel_stack_top: u64;
    let user_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) kernel_stack_top);
        core::arch::asm!("mov {}, gs:[8]", out(reg) user_rsp);
    }
    let user_rip = unsafe { *((kernel_stack_top - 8) as *const u64) };   // saved rcx
    let user_rflags = unsafe { *((kernel_stack_top - 16) as *const u64) }; // saved r11

    let saved = crate::process::SavedSyscallRegs {
        rbp: unsafe { *((kernel_stack_top - 24) as *const u64) },
        rbx: unsafe { *((kernel_stack_top - 32) as *const u64) },
        r12: unsafe { *((kernel_stack_top - 40) as *const u64) },
        r13: unsafe { *((kernel_stack_top - 48) as *const u64) },
        r14: unsafe { *((kernel_stack_top - 56) as *const u64) },
        r15: unsafe { *((kernel_stack_top - 64) as *const u64) },
        r10: unsafe { *((kernel_stack_top - 72) as *const u64) },
        r9:  unsafe { *((kernel_stack_top - 80) as *const u64) },
        r8:  unsafe { *((kernel_stack_top - 88) as *const u64) },
    };

    // Clone address space with COW
    let parent_aspace = AddressSpace::from_raw(cr3);
    let child_aspace = match parent_aspace.clone_cow() {
        Some(a) => a,
        None => {
            let _ = parent_aspace.into_raw();
            return err(ENOMEM);
        }
    };
    let _ = parent_aspace.into_raw();

    let child_cr3 = child_aspace.into_raw();

    // Clone VMAs
    crate::mmap::vma_clone(cr3, child_cr3);

    // Get parent's brk
    let parent_brk = crate::mmap::sys_brk(cr3, 0);

    // Create child process
    let child = crate::process::Process::new_fork(
        pid,
        child_cr3,
        None, // user_stack_phys is COW-shared, not separately tracked
        parent_brk,
        user_rip,
        user_rsp,
        user_rflags,
        &saved,
    );
    let child_pid = child.pid;

    crate::serial_println!("[fork] parent={} child={} cr3={:#x}", pid, child_pid, child_cr3);

    // Add child to scheduler
    crate::scheduler::add_user_process(child);

    // Parent returns child PID
    child_pid
}

//  44  wait4(pid, status_ptr, options) -> child_pid / -errno //

const WNOHANG: u64 = 1;
const ECHILD: i64 = -10;

fn sys_wait4(target_pid: u64, status_ptr: u64, options: u64) -> u64 {
    let my_pid = current_pid();
    let cr3 = current_cr3();

    loop {
        // Search for a matching zombie child
        let found = crate::scheduler::find_zombie_child(my_pid, target_pid);

        match found {
            Some((child_pid, exit_code)) => {
                // Write exit status to user buffer if provided
                if status_ptr != 0 && user_ptr_mapped(cr3, status_ptr, 8) {
                    unsafe { *(status_ptr as *mut u64) = exit_code; }
                }
                // Reap the zombie
                crate::scheduler::reap_zombie(child_pid);
                return child_pid;
            }
            None => {
                // Check if we have any children at all
                if !crate::scheduler::has_children(my_pid) {
                    return err(ECHILD);
                }
                if options & WNOHANG != 0 {
                    return 0; // No zombie yet, non-blocking
                }
                // Block until a child exits
                crate::scheduler::block_current("wait4");
            }
        }
    }
}

//  45  kill(pid, sig) -> 0 / -errno  //

fn sys_kill(target_pid: u64, sig: u64) -> u64 {
    if target_pid == 0 { return err(EINVAL); }

    match sig {
        9 | 15 => { // SIGKILL, SIGTERM
            crate::scheduler::kill(target_pid);
            // Send SIGCHLD to parent
            crate::signal::send_sigchld(target_pid);
            0
        }
        0 => {
            // Signal 0: check if process exists
            if crate::scheduler::process_exists(target_pid) { 0 } else { err(ESRCH) }
        }
        _ => {
            // Store signal as pending
            crate::signal::send_signal(target_pid, sig as u32);
            0
        }
    }
}

//  46  exec(path_ptr, path_len, argv_ptr, argc)                //
//      Replaces current process image with a new ELF.          //
//      On success, does not return. On error, returns -errno.  //

fn sys_exec(path_ptr: u64, path_len: u64, _argv_ptr: u64, _argc: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let cr3 = current_cr3();
    let pid = current_pid();

    // Read the ELF file from VFS
    let file_data = match crate::vfs_read::read_file(path) {
        Some(data) => data,
        None => return err(ENOENT),
    };

    // Create new address space
    let new_aspace = match AddressSpace::new_user() {
        Some(a) => a,
        None => return err(ENOMEM),
    };

    // Load ELF into new address space
    let read_file = |interp_path: &str| -> Option<alloc::vec::Vec<u8>> {
        if interp_path.contains("ld-miku") || interp_path.contains("ld.so") {
            return Some(crate::ldso::LDSO_BYTES.to_vec());
        }
        crate::vfs_read::read_file(interp_path)
    };

    let image = match crate::elf_loader::load(&file_data, &new_aspace, &[path], Some(&read_file)) {
        Ok(img) => img,
        Err(e) => {
            crate::serial_println!("[exec] ELF load failed: {}", e.as_str());
            // new_aspace is dropped -> freed
            return err(ENOENT);
        }
    };

    let new_cr3 = new_aspace.into_raw();

    // Free old address space VMAs
    crate::mmap::vma_cleanup(cr3);
    // Set up new brk
    crate::mmap::vma_set_brk(new_cr3, image.brk);

    // Update current process's CR3 and build new user frame
    crate::scheduler::update_process_cr3(pid, new_cr3);

    // Switch to new address space BEFORE freeing old one
    // to avoid use-after-free if an interrupt fires between free and switch
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) new_cr3, options(nostack, preserves_flags));
    }

    // Now safe to free old address space
    if cr3 != 0 && cr3 != crate::vmm::kernel_cr3() {
        let mut old = AddressSpace::from_raw(cr3);
        old.free_address_space();
    }

    // Set new TLS if available
    if image.tls_base != 0 {
        x86_64::registers::model_specific::FsBase::write(
            x86_64::VirtAddr::new(image.tls_base),
        );
    }

    // Write new entry point and stack pointer into the syscall handler's saved regs
    // When syscall_handler does sysretq, RCX is loaded as RIP and gs:[8] as RS
    let kernel_stack_top: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) kernel_stack_top);
        // Overwrite saved RCX (user RIP) at [top-8]
        *((kernel_stack_top - 8) as *mut u64) = image.entry;
        // Overwrite user RSP at gs:[8]
        core::arch::asm!("mov gs:[8], {}", in(reg) image.stack_top, options(nostack, preserves_flags));
    }

    crate::serial_println!(
        "[exec] pid={} replaced with '{}': entry={:#x} sp={:#x}",
        pid, path, image.entry, image.stack_top
    );

    // Return 0 — but the sysretq will jump to the new entry, not the old caller
    0
}
