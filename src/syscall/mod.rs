// MikuOS syscall surface.
//
// Layout:
//      errno      - POSIX error codes + VfsError -> errno mapping
//      user_mem   - user pointer validation, path copy-in, current cr3/pid
//      abi        - userspace byte layouts (stat, dirent, statfs)
//      io         - read, write, write_file
//      file       - open, close, seek, fsize, dup/dup2, truncate, pipe,
//                   fallocate, fsync, punch_hole
//       fs        - stat, fstat, mkdir/rmdir/unlink, readdir, rename, link,
//                   chmod/chown, symlink/readlink, chdir, statfs, xattr,
//                   utimensat
//   memory        - mmap, munmap, mprotect, brk, set_tls, get_tls, map_lib,getcwd
//   process       - exit, sleep, uptime, fork, wait4, kill, exec
//
// init programs the SYSCALL/SYSRET MSRs and dispatch is the C-ABI entry
// point invoked by the naked syscall_handler. The on-the-wire syscall
// numbers are documented next to dispatch

mod abi;
mod errno;
mod file;
mod fs;
mod io;
mod memory;
mod net;
mod process;
mod user_mem;

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

use errno::{err, ENOSYS};
use user_mem::current_pid;
use crate::gdt;

const SYSCALL_TABLE_SIZE: u32 = 56;

// public entry point

pub fn init() {
    unsafe {
        Efer::update(|f| *f |= EferFlags::SYSTEM_CALL_EXTENSIONS | EferFlags::NO_EXECUTE_ENABLE);
    }
    Star::write(
        gdt::user_code_selector(),
        gdt::user_data_selector(),
        gdt::kernel_code_selector(),
        gdt::kernel_data_selector(),
    ).unwrap();
    LStar::write(VirtAddr::new(syscall_handler as *const () as u64));
    SFMask::write(RFlags::INTERRUPT_FLAG);
    crate::serial_println!(
        "[syscall] MikuOS syscall table ready ({} entries)",
        SYSCALL_TABLE_SIZE
    );
}

// naked SYSCALL/SYSRET bridge

#[unsafe(naked)]
unsafe extern "C" fn syscall_handler() {
    core::arch::naked_asm!(
        "swapgs",
        "mov gs:[0x18], rsp",
        "mov rsp, gs:[0x10]",
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
        "mov rsp, gs:[0x18]",
        "swapgs",
        "sysretq",
        handler = sym dispatch,
    );
}

//                  syscall numbers
//
//  0  exit           16  sleep           32  symlink
//  1  write          17  uptime          33  readlink
//  2  read           18  stat            34  pipe
//  3  mmap           19  fstat           35  chdir
//  4  munmap         20  mkdir           36  statfs
//  5  mprotect       21  rmdir           37  fallocate
//  6  brk            22  unlink          38  getxattr
//  7  getpid         23  readdir         39  setxattr
//  8  getcwd         24  rename          40  utimensat
//  9  set_tls        25  link            41  fsync
// 10  get_tls        26  chmod           42  punch_hole
// 11  open           27  chown           43  fork
// 12  close          28  dup             44  wait4
// 13  seek           29  dup2            45  kill
// 14  fsize          30  truncate        46  exec
// 15  map_lib        31  write_file
//
// 47 umask  48 getuid  49 getgid  50 geteuid  51 getegid
// 52 setuid 53 setgid  54 seteuid 55 setegid
// 56 socket 57 connect 58 send    59 recv

extern "C" fn dispatch(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    match nr {
        0  => process::sys_exit(a1),
        1  => io::sys_write(a1, a2, a3),
        2  => io::sys_read(a1, a2, a3),
        3  => memory::sys_mmap(a1, a2, a3, a4),
        4  => memory::sys_munmap(a1, a2),
        5  => memory::sys_mprotect(a1, a2, a3),
        6  => memory::sys_brk(a1),
        7  => current_pid(),
        8  => memory::sys_getcwd(a1, a2),
        9  => memory::sys_set_tls(a1),
        10 => memory::sys_get_tls(),
        11 => file::sys_open(a1, a2, a3, a4),
        12 => file::sys_close(a1),
        13 => file::sys_seek(a1, a2, a3),
        14 => file::sys_fsize(a1),
        15 => memory::sys_map_lib(a1, a2),
        16 => process::sys_sleep(a1),
        17 => process::sys_uptime(),
        18 => fs::sys_stat(a1, a2, a3),
        19 => fs::sys_fstat(a1, a2),
        20 => fs::sys_mkdir(a1, a2, a3),
        21 => fs::sys_rmdir(a1, a2),
        22 => fs::sys_unlink(a1, a2),
        23 => fs::sys_readdir(a1, a2, a3, a4),
        24 => fs::sys_rename(a1, a2, a3, a4),
        25 => fs::sys_link(a1, a2, a3, a4),
        26 => fs::sys_chmod(a1, a2, a3),
        27 => fs::sys_chown(a1, a2, a3, a4),
        28 => file::sys_dup(a1),
        29 => file::sys_dup2(a1, a2),
        30 => file::sys_truncate(a1, a2),
        31 => io::sys_write_file(a1, a2, a3),
        32 => fs::sys_symlink(a1, a2, a3, a4),
        33 => fs::sys_readlink(a1, a2, a3, a4),
        34 => file::sys_pipe(a1),
        35 => fs::sys_chdir(a1, a2),
        36 => fs::sys_statfs(a1, a2, a3),
        37 => file::sys_fallocate(a1, a2, a3),
        38 => fs::sys_getxattr(a1, a2, a3, a4),
        39 => fs::sys_setxattr(a1, a2, a3, a4),
        40 => fs::sys_utimensat(a1, a2, a3),
        41 => file::sys_fsync(a1),
        42 => file::sys_punch_hole(a1, a2, a3),
        43 => process::sys_fork(),
        44 => process::sys_wait4(a1, a2, a3),
        45 => process::sys_kill(a1, a2),
        46 => process::sys_exec(a1, a2, a3, a4),
        47 => process::sys_umask(a1),
        48 => process::sys_getuid(),
        49 => process::sys_getgid(),
        50 => process::sys_geteuid(),
        51 => process::sys_getegid(),
        52 => process::sys_setuid(a1),
        53 => process::sys_setgid(a1),
        54 => process::sys_seteuid(a1),
        55 => process::sys_setegid(a1),
        56 => net::sys_socket(a1, a2, a3),
        57 => net::sys_connect(a1, a2, a3),
        58 => net::sys_send(a1, a2, a3, a4),
        59 => net::sys_recv(a1, a2, a3, a4),
        // 60  mmap_file(args_ptr) - file-backed mmap; args struct in user
        // memory carries the six fields that don't fit the 4-arg ABI
        60 => memory::sys_mmap_file(a1),
        61 => memory::sys_msync(a1, a2),
        _  => {
            crate::serial_println!("[syscall] unknown nr={}", nr);
            err(ENOSYS)
        }
    }
}
