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

// Durability and reserving space (kernel dispatch 37, 41). Write-then-rename is
// safe only if the write reaches the medium before the rename
pub const SYS_FALLOCATE:  u64 = 37;
pub const SYS_FSYNC:      u64 = 41;
/// `utimensat(path, path word, times, flags)`: a file's times (ABI 3.2e)
pub const SYS_UTIMENSAT:  u64 = 40;

// Identity and capabilities (kernel dispatch 47-55, 86-91)
pub const SYS_UMASK:      u64 = 47;
pub const SYS_GETUID:     u64 = 48;
pub const SYS_GETGID:     u64 = 49;
pub const SYS_GETEUID:    u64 = 50;
pub const SYS_GETEGID:    u64 = 51;
pub const SYS_SETUID:     u64 = 52;
pub const SYS_SETGID:     u64 = 53;
pub const SYS_SETEUID:    u64 = 54;
pub const SYS_SETEGID:    u64 = 55;
pub const SYS_SETRESUID:  u64 = 86;
pub const SYS_SETRESGID:  u64 = 87;
pub const SYS_SETGROUPS:  u64 = 88;
pub const SYS_GETGROUPS:  u64 = 89;
pub const SYS_CAPGET:     u64 = 90;
pub const SYS_CAPSET:     u64 = 91;
/// Who is at the console. Sets the shell thread's credential, not the caller's;
/// see the kernel side
pub const SYS_SESSION:     u64 = 92;
pub const SYS_POWER:       u64 = 93;

// Peer credentials and descriptor passing (kernel dispatch 83-85)
/// A line for the kernel's log instead of the screen
pub const SYS_KLOG:       u64 = 75;
/// Terminal control
pub const SYS_IOCTL:      u64 = 71;

pub const SYS_PEERCRED:   u64 = 83;
pub const SYS_SENDFD:     u64 = 84;
pub const SYS_RECVFD:     u64 = 85;

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
pub const SYS_POLL:       u64 = 76;
/// A sleep in nanoseconds, and a `poll` with its timeout in them
/// (`dev/kernel-core.md`, "Time"). `SYS_SLEEP` counts 4 ms ticks
pub const SYS_SLEEP_NS:   u64 = 122;
pub const SYS_POLL_NS:    u64 = 123;
pub const SYS_SOCKETPAIR: u64 = 77;
pub const SYS_GETRANDOM:  u64 = 78;
pub const SYS_SETSOCKOPT: u64 = 80;

/// Terminals. See `kernel/syscall/adapters/tty.rs`
pub const SYS_TTY:        u64 = 99;

// Filesystem info and job control (kernel dispatch 100-102): what a signal
// means to this process and which job it is in
pub const SYS_STATFS:     u64 = 36;
pub const SYS_SIGDISP:    u64 = 100;
pub const SYS_SETPGID:    u64 = 101;
pub const SYS_GETPGID:    u64 = 102;
/// The process table, one fixed-size record per process, for `ps`
pub const SYS_PROCINFO:   u64 = 103;
pub const SYS_PROC_PATH:  u64 = 105;
/// The machine's commands, for a caller holding CAP_SYS_ADMIN. See
/// `kernel/syscall/adapters/admin.rs`
pub const SYS_KCMD:       u64 = 104;

/// Leave a signal at whatever the kernel does about it
pub const SIG_DISP_DEFAULT: u64 = 0;
/// Throw it away
pub const SIG_DISP_IGNORE:  u64 = 1;
/// Deliver it to this process's handler
pub const SIG_DISP_CATCH:   u64 = 2;
/// Changes nothing; `sigdisp` returns the disposition set. Every other value
/// returns the one it replaced
pub const SIG_DISP_QUERY:   u64 = 3;
/// Or'd into `SIG_DISP_CATCH`: the handler runs on the thread's alternate stack
/// (`SA_ONSTACK`). A query returns it when set
pub const SIG_DISP_ONSTACK: u64 = 4;

pub const SYS_CLOCK_GETTIME: u64 = 70;
pub const SYS_CLOCK_SETTIME: u64 = 79;
pub const CLOCK_REALTIME:    u64 = 0;
pub const CLOCK_MONOTONIC:   u64 = 1;

/// Threads and the futex under them. See `dev/std-architecture.md` §1 and §2,
/// and `libmiku/thread.rs`
pub const SYS_THREAD_CREATE: u64 = 106;
pub const SYS_THREAD_EXIT:   u64 = 107;
pub const SYS_GETTID:        u64 = 108;
pub const SYS_SCHED_YIELD:   u64 = 109;
pub const SYS_FUTEX:         u64 = 110;

/// Directory read with a cursor, see `libmiku/dir.rs`
pub const SYS_GETDENTS:      u64 = 111;

/// `stat` not following a symlink: `symlink_metadata`
pub const SYS_LSTAT:         u64 = 112;
/// Changes a descriptor's flags after `open`: FD_CLOEXEC and `O_NONBLOCK`
pub const SYS_FCNTL:         u64 = 113;
/// Reads a socket option, as `SO_ERROR` for a non-blocking connect's outcome.
/// The value is the return
pub const SYS_GETSOCKOPT:    u64 = 114;
/// The thread's alternate signal stack: `(new *stack_t, old *stack_t)`
pub const SYS_SIGALTSTACK:   u64 = 115;
/// inotify, Linux's three calls: `(flags)`, `(fd, path, path_len, mask)`,
/// `(fd, wd)`
pub const SYS_INOTIFY_INIT1:      u64 = 116;
pub const SYS_INOTIFY_ADD_WATCH:  u64 = 117;
pub const SYS_INOTIFY_RM_WATCH:   u64 = 118;
pub const SYS_REALPATH:           u64 = 119;
pub const SYS_PIPE2:              u64 = 120;

pub const NR_SYSCALLS: u64 = 119;

// raw syscall wrappers

// Every wrapper declares the full caller-saved set as clobbered
// (rdi/rsi/rdx/r10/r8/r9 on top of the architectural rax/rcx/r11):
//   - the kernel's syscall epilogue does not restore rdi/rsi/rdx, and
//   - a signal delivered at the syscall's tail records RDI/RSI/RDX as zero
//     in its frame (they were never saved), which `sigreturn` hands back to the
//     instruction after `syscall`

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
