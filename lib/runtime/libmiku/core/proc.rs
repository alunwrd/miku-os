use crate::sys::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_exit(code: i64) -> ! {
    unsafe { sc1(SYS_EXIT, code as u64); }
    loop {}
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getpid() -> u64 {
    unsafe { sc0(SYS_GETPID) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    let r = unsafe { sc2(SYS_GETCWD, buf as u64, size as u64) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_brk(addr: u64) -> u64 {
    unsafe { sc1(SYS_BRK, addr) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_mmap(addr: u64, len: usize, prot: u64) -> *mut u8 {
    let r = unsafe { sc4(SYS_MMAP, addr, len as u64, prot, 0) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_munmap(addr: *mut u8, len: usize) -> i64 {
    unsafe { sc2(SYS_MUNMAP, addr as u64, len as u64) }
}

/// File-backed mmap. 'flags' is MAP_SHARED (1) or MAP_PRIVATE (2). The six
/// parameters travel in a struct on our stack, since the 4-argument syscall ABI
/// cannot carry them
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_mmap_file(
    addr: u64, len: usize, prot: u64, flags: u64, fd: i64, offset: u64,
) -> *mut u8 {
    let args: [u64; 6] = [addr, len as u64, prot, flags, fd as u64, offset];
    let r = unsafe { sc1(SYS_MMAP_FILE, args.as_ptr() as u64) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_msync(addr: *mut u8, len: usize) -> i64 {
    unsafe { sc2(SYS_MSYNC, addr as u64, len as u64) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_mprotect(addr: u64, len: usize, prot: u64) -> i64 {
    unsafe { sc3(SYS_MPROTECT, addr, len as u64, prot) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_set_tls(addr: u64) -> i64 {
    unsafe { sc1(SYS_SET_TLS, addr) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_get_tls() -> u64 {
    unsafe { sc1(SYS_GET_TLS, 0) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_map_lib(name: *const u8, name_len: usize) -> i64 {
    unsafe { sc2(SYS_MAP_LIB, name as u64, name_len as u64) }
}

// Process control: fork / wait / kill / exec

/// fork(): 0 in the child, child pid in the parent, negative errno on failure.
///
/// The library's locks are taken first and released after, in both processes. A
/// fork copies memory, locks included, and only the calling thread comes
/// across, so a lock another thread held would stay held in the child. The heap
/// copy must also be consistent.
///
/// Order: the hooks' outer locks, then the heap, then their inner ones, as the
/// library nests them (`setenv` allocates under `ENV`).
///
/// Nothing a signal handler takes is held across the syscall, since SIGCHLD can
/// arrive at the end of this `fork` on this thread and the dispatcher takes the
/// signal tables. A hook resets those in the child (`child`)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fork() -> i64 {
    let hooks = fork_hooks();
    for h in hooks { call(h[0]); }
    crate::heap::fork_lock();
    for h in hooks { call(h[1]); }
    let r = unsafe { sc0(SYS_FORK) };
    let after = if r == 0 { 3 } else { 2 };
    for h in hooks.iter().rev() { call(h[after]); }
    unsafe { crate::heap::fork_unlock() };
    r
}

/// What another library adds to `fork`: four `extern "C" fn()`s as addresses (0
/// for none): take its outer locks, take its inner ones, and run after in
/// parent and child. For libraries on this one with locks it cannot name
/// (`sys_miku`: the environment and signal tables). Registered once, from a
/// constructor; 0 on success, -1 when the table is full
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fork_hooks(outer: usize, inner: usize, parent: usize, child: usize) -> i64 {
    use core::sync::atomic::Ordering;
    let i = FORK_HOOKS_N.load(Ordering::Acquire);
    if i >= FORK_HOOKS_MAX { return -1; }
    // Constructors run one at a time, before `main` and any thread
    unsafe { FORK_HOOKS[i] = [outer, inner, parent, child]; }
    FORK_HOOKS_N.store(i + 1, Ordering::Release);
    0
}

const FORK_HOOKS_MAX: usize = 4;
static FORK_HOOKS_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static mut FORK_HOOKS: [[usize; 4]; FORK_HOOKS_MAX] = [[0; 4]; FORK_HOOKS_MAX];

fn fork_hooks() -> &'static [[usize; 4]] {
    let n = FORK_HOOKS_N.load(core::sync::atomic::Ordering::Acquire);
    let all: &'static [[usize; 4]; FORK_HOOKS_MAX] = unsafe { &*core::ptr::addr_of!(FORK_HOOKS) };
    &all[..n]
}

fn call(f: usize) {
    if f != 0 {
        let f: extern "C" fn() = unsafe { core::mem::transmute(f) };
        f();
    }
}

/// waitpid(pid, *status): blocks until the child exits. `u64::MAX` is any
/// child; 0 is the literal pid 0. status (if non-null) receives the raw exit
/// code. Returns the reaped pid or negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_waitpid(pid: u64, status: *mut i64) -> i64 {
    unsafe { sc3(SYS_WAIT4, pid, status as u64, 0) }
}

/// Reaps a child if one has already exited, without waiting: `WNOHANG`. Returns
/// 0 with nothing to reap, the pid, or a negative errno. `pid` `u64::MAX` means
/// any child
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_try_wait(pid: u64, status: *mut i64) -> i64 {
    const WNOHANG: u64 = miku_abi::wait::WNOHANG as u64;
    unsafe { sc3(SYS_WAIT4, pid, status as u64, WNOHANG) }
}

/// setpgid(pid, pgid): puts a process in a job. `pid` 0 is the caller, `pgid` 0
/// makes a new group led by the target. A shell calls it on both sides of a
/// fork, since one call leaves a window where a Ctrl-C reaches the shell
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_setpgid(pid: u64, pgid: u64) -> i64 {
    unsafe { sc2(SYS_SETPGID, pid, pgid) }
}

/// getpgid(pid): which job a process is in. `pid` 0 is the caller
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getpgid(pid: u64) -> i64 {
    unsafe { sc1(SYS_GETPGID, pid) }
}

/// sigdisp(sig, disp): tells the kernel what to do with a signal: default,
/// ignore, or deliver. The handler is this library's. `signal.rs` calls it when
/// a handler is installed or removed
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sigdisp(sig: u64, disp: u64) -> i64 {
    unsafe { sc2(SYS_SIGDISP, sig, disp) }
}

/// procinfo(buf, max): the process table, one 64-byte record each. pid, ppid,
/// pgid, and cpu time as u64; then uid u16, state letter, priority; then tenths
/// of a percent of CPU as u32 and a 24-byte name. Returns the records written,
/// for `ps` and `top`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_procinfo(buf: *mut u8, max: u64) -> i64 {
    if buf.is_null() { return -1; }
    unsafe { sc2(SYS_PROCINFO, buf as u64, max) }
}

/// wait4(pid, *status, options): the general form of `waitpid` and `try_wait`.
/// A shell needs its own options for `WUNTRACED`, to learn a job stopped
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_wait4(pid: u64, status: *mut i64, options: u64) -> i64 {
    unsafe { sc3(SYS_WAIT4, pid, status as u64, options) }
}

/// proc_path(pid, buf, len): the program a process runs, as a path. The long
/// form of `procinfo`'s 24-byte name, one pid at a time. Returns the length
/// written, or 0 for a kernel thread, which has no image
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_proc_path(pid: u64, buf: *mut u8, len: u64) -> i64 {
    if buf.is_null() { return -1; }
    unsafe { sc3(SYS_PROC_PATH, pid, buf as u64, len) }
}

/// kcmd(text, len): runs one of the machine's commands. `nvidia`, `mount`,
/// `mkfs.ext4`, `sv`, and others live in the kernel since each needs ring 0.
/// Needs `CAP_SYS_ADMIN`, so normally goes through `mikudo`. Runs on the
/// caller's thread with its credential and terminal
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_kcmd(text: *const u8, len: usize) -> i64 {
    if text.is_null() || len == 0 { return -1; }
    unsafe { sc2(SYS_KCMD, text as u64, len as u64) }
}

/// killpg(pgid, sig): signals every process in a job. The kernel spells this
/// `kill` with a negative pid
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_killpg(pgid: u64, sig: u64) -> i64 {
    if pgid == 0 { return -1; }
    let neg = (pgid as i64).wrapping_neg() as u64;
    unsafe { sc2(SYS_KILL, neg, sig) }
}

/// kill(pid, sig). sig 9/15 terminate; see kernel sys_kill for the rest
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_kill(pid: u64, sig: u64) -> i64 {
    unsafe { sc2(SYS_KILL, pid, sig) }
}

/// exec(path, argv, argc): replaces the current process image. argv is an array
/// of pointers to NUL-terminated strings (argv[0] = program name). The kernel
/// supplies a default environment. Does not return on success
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_exec(path: *const u8, argv: *const *const u8, argc: usize) -> i64 {
    let len = crate::string::miku_strlen(path);
    unsafe { sc4(SYS_EXEC, path as u64, len as u64, argv as u64, argc as u64) }
}

/// execve(path, argv, argc, envp, envc): exec with an explicit environment.
/// envp entries are "KEY=value" C strings, laid out as argv. The six parameters
/// travel in a u64[6] struct (4-arg syscall ABI)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_execve(
    path: *const u8,
    argv: *const *const u8,
    argc: usize,
    envp: *const *const u8,
    envc: usize,
) -> i64 {
    let len = crate::string::miku_strlen(path);
    let args: [u64; 6] = [
        path as u64, len as u64,
        argv as u64, argc as u64,
        envp as u64, envc as u64,
    ];
    unsafe { sc1(SYS_EXECVE, args.as_ptr() as u64) }
}

// Identity, capabilities, and the two calls a local service needs
//
// Each returns the kernel's value: a count or zero on success, a negative errno
// otherwise. The getters cannot fail and return the id directly.

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_umask(mask: u64) -> u64 {
    unsafe { sc1(SYS_UMASK, mask) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getuid() -> u64 { unsafe { sc0(SYS_GETUID) as u64 } }

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getgid() -> u64 { unsafe { sc0(SYS_GETGID) as u64 } }

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_geteuid() -> u64 { unsafe { sc0(SYS_GETEUID) as u64 } }

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getegid() -> u64 { unsafe { sc0(SYS_GETEGID) as u64 } }

/// setuid(uid): with CAP_SETUID, sets all three ids and drops every capability
/// when the target is not root, the irreversible form a broker uses. Without
/// it, only the effective id moves, to one already held
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_setuid(uid: u64) -> i64 { unsafe { sc1(SYS_SETUID, uid) } }

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_setgid(gid: u64) -> i64 { unsafe { sc1(SYS_SETGID, gid) } }

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_seteuid(uid: u64) -> i64 { unsafe { sc1(SYS_SETEUID, uid) } }

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_setegid(gid: u64) -> i64 { unsafe { sc1(SYS_SETEGID, gid) } }

/// setresuid(r, e, s): all three at once, with no partial identity. 0xFFFF
/// leaves a field alone
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_setresuid(r: u64, e: u64, s: u64) -> i64 {
    unsafe { sc3(SYS_SETRESUID, r, e, s) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_setresgid(r: u64, e: u64, s: u64) -> i64 {
    unsafe { sc3(SYS_SETRESGID, r, e, s) }
}

/// setgroups(list, count): needs CAP_SETGID, since a supplementary group grants
/// access as a primary one does
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_setgroups(list: *const u16, count: u64) -> i64 {
    unsafe { sc2(SYS_SETGROUPS, list as u64, count) }
}

/// getgroups(list, max): with max == 0, returns the count without writing, to
/// size a buffer
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getgroups(list: *mut u16, max: u64) -> i64 {
    unsafe { sc2(SYS_GETGROUPS, list as u64, max) }
}

/// capget(buf): writes u64[3]: permitted, effective, bounding
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_capget(buf: *mut u64) -> i64 {
    unsafe { sc1(SYS_CAPGET, buf as u64) }
}

/// capset(permitted, effective, bounding): only narrows. Each requested set
/// must be a subset of the one held
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_capset(permitted: u64, effective: u64, bounding: u64) -> i64 {
    unsafe { sc3(SYS_CAPSET, permitted, effective, bounding) }
}

/// peercred(fd, out): who is at the other end of a connected local socket, as
/// of connecting.
///
/// `out` is 40 bytes: pid (u64), start_tick (u64), uid, gid, euid, egid (u32
/// each), then 8 reserved. The client cannot influence this
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_peercred(fd: u64, out: *mut u8) -> i64 {
    unsafe { sc2(SYS_PEERCRED, fd, out as u64) }
}

/// sendfd(sockfd, fd): hands one open descriptor to the peer. Lets an elevated
/// command use the user's terminal, pipe, or redirection, and keeps the
/// password out of the unprivileged process, since mikudod prompts on the
/// descriptor it gets
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sendfd(sockfd: u64, fd: u64) -> i64 {
    unsafe { sc2(SYS_SENDFD, sockfd, fd) }
}

/// recvfd(sockfd): takes the oldest descriptor the peer sent, into the lowest
/// free slot. -EAGAIN when none is waiting; never blocks
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_recvfd(sockfd: u64) -> i64 {
    unsafe { sc1(SYS_RECVFD, sockfd) }
}


/// session(op, arg, ptr, len): opens, closes, or lists logins.
///
/// `op` is a `MIKU_SESSION_*` constant; `arg` is a leader pid for OPEN and a
/// session id for CLOSE; `ptr`/`len` are a `MikuSessionRequest` for OPEN and an
/// array of `MikuSessionInfo` for LIST.
///
/// OPEN and CLOSE need CAP_SETUID, so `mikudod` is the caller. LIST and CURRENT
/// need nothing, as `who` uses them.
///
/// A console session's identity goes on the shell thread: the command
/// interpreter is in ring 0 and cannot exec itself, so the password checker
/// publishes the answer
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_session(op: u64, arg: u64, ptr: u64, len: u64) -> i64 {
    unsafe { sc4(SYS_SESSION, op, arg, ptr, len) }
}

/// Terminals: make one, join one, hang one up, ask which one you are on.
///
/// `op` is `TTY_ATTACH`, `TTY_SELF`, `TTY_SETFG`, `TTY_GETFG`, `TTY_CLEAR`, or
/// `TTY_OF_PID`, and `arg` is a terminal id for the middle two. See
/// `kernel/syscall/adapters/tty.rs`.
///
/// No descriptor: `print!` routes to the controlling terminal, and whoever
/// draws it reads the grid over `may_console`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_tty(op: u64, arg: u64) -> i64 {
    unsafe { sc2(SYS_TTY, op, arg) }
}

/// Stops the machine. `action` is 0 for off, 1 for reboot. `CAP_SYS_BOOT`, held
/// by a session and not by a program it starts. Does not return on success
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_power(action: u64) -> i64 {
    unsafe { sc1(SYS_POWER, action) }
}
