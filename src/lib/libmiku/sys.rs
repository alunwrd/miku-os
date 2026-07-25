use core::arch::asm;

// syscall numbers (matches kernel dispatch table)

pub const SYS_EXIT:       u64 = 0;
pub const SYS_WRITE:      u64 = 1;
pub const SYS_READ:       u64 = 2;
pub const SYS_MMAP:       u64 = 3;
pub const SYS_MUNMAP:     u64 = 4;
pub const SYS_MPROTECT:   u64 = 5;
pub const SYS_BRK:        u64 = 6;
pub const SYS_GETPID:     u64 = 7;
pub const SYS_GETCWD:     u64 = 8;
pub const SYS_SET_TLS:    u64 = 9;
pub const SYS_GET_TLS:    u64 = 10;
pub const SYS_OPEN:       u64 = 11;
pub const SYS_CLOSE:      u64 = 12;
pub const SYS_SEEK:       u64 = 13;
pub const SYS_FSIZE:      u64 = 14;
pub const SYS_MAP_LIB:    u64 = 15;
pub const SYS_SLEEP:      u64 = 16;
pub const SYS_UPTIME:     u64 = 17;
pub const SYS_STAT:       u64 = 18;
pub const SYS_FSTAT:      u64 = 19;
pub const SYS_MKDIR:      u64 = 20;
pub const SYS_RMDIR:      u64 = 21;
pub const SYS_UNLINK:     u64 = 22;
pub const SYS_READDIR:    u64 = 23;
pub const SYS_RENAME:     u64 = 24;
pub const SYS_LINK:       u64 = 25;
pub const SYS_CHMOD:      u64 = 26;
pub const SYS_CHOWN:      u64 = 27;
pub const SYS_DUP:        u64 = 28;
pub const SYS_DUP2:       u64 = 29;
pub const SYS_TRUNCATE:   u64 = 30;
pub const SYS_WRITE_FILE: u64 = 31;
pub const SYS_SYMLINK:    u64 = 32;
pub const SYS_READLINK:   u64 = 33;
pub const SYS_PIPE:       u64 = 34;
pub const SYS_CHDIR:      u64 = 35;

// Process control (kernel dispatch 43-46, 67)
pub const SYS_FORK:       u64 = 43;
pub const SYS_WAIT4:      u64 = 44;
pub const SYS_KILL:       u64 = 45;
pub const SYS_EXEC:       u64 = 46;
pub const SYS_EXECVE:     u64 = 67;
pub const SYS_SIGENTRY:   u64 = 68;
pub const SYS_SIGRETURN:  u64 = 69;

// Socket syscalls (kernel dispatch 56-59, 62-66)
pub const SYS_SOCKET:     u64 = 56;
pub const SYS_CONNECT:    u64 = 57;
pub const SYS_SEND:       u64 = 58;
pub const SYS_RECV:       u64 = 59;
pub const SYS_MMAP_FILE:  u64 = 60;
pub const SYS_MSYNC:      u64 = 61;
pub const SYS_BIND:       u64 = 62;
pub const SYS_LISTEN:     u64 = 63;
pub const SYS_ACCEPT:     u64 = 64;
pub const SYS_SENDTO:     u64 = 65;
pub const SYS_RECVFROM:   u64 = 66;

pub const NR_SYSCALLS: u64 = 36;

// raw syscall wrappers

// Every wrapper declares the full caller-saved set as clobbered
// (rdi/rsi/rdx/r10/r8/r9 on top of the architectural rax/rcx/r11):
//   - the kernel's syscall epilogue does not restore rdi/rsi/rdx, and
//   - a signal handler may run between kernel exit and the instruction
//     after `syscall`, trashing any caller-saved register (the kernel's
//     sigreturn restores only RIP/RSP/RAX/RFLAGS; callee-saved registers
//     are preserved by the handler itself per the C ABI)

#[inline(always)]
pub unsafe fn sc0(nr: u64) -> i64 {
    let r: i64;
    asm!(
        "syscall",
        in("rax") nr,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        lateout("rdi") _, lateout("rsi") _, lateout("rdx") _,
        lateout("r10") _, lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    r
}

#[inline(always)]
pub unsafe fn sc1(nr: u64, a1: u64) -> i64 {
    let r: i64;
    asm!(
        "syscall",
        in("rax") nr, inlateout("rdi") a1 => _,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        lateout("rsi") _, lateout("rdx") _,
        lateout("r10") _, lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    r
}

#[inline(always)]
pub unsafe fn sc2(nr: u64, a1: u64, a2: u64) -> i64 {
    let r: i64;
    asm!(
        "syscall",
        in("rax") nr, inlateout("rdi") a1 => _, inlateout("rsi") a2 => _,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        lateout("rdx") _,
        lateout("r10") _, lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    r
}

#[inline(always)]
pub unsafe fn sc3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let r: i64;
    asm!(
        "syscall",
        in("rax") nr, inlateout("rdi") a1 => _, inlateout("rsi") a2 => _,
        inlateout("rdx") a3 => _,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        lateout("r10") _, lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    r
}

#[inline(always)]
pub unsafe fn sc4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let r: i64;
    asm!(
        "syscall",
        in("rax") nr, inlateout("rdi") a1 => _, inlateout("rsi") a2 => _,
        inlateout("rdx") a3 => _, inlateout("r10") a4 => _,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    r
}

// helper

#[inline(always)]
pub fn is_err(r: i64) -> bool { r < 0 }
