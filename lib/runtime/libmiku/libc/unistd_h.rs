//! `<unistd.h>`, `<sys/wait.h>`

use crate::libc::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> i64 {
    ret(io::miku_read(fd as u64, buf, count))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> i64 {
    ret(io::miku_write(fd as u64, buf, count))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn close(fd: i32) -> i32 {
    let r = file::miku_close(fd as i64);
    if r < 0 { errno::set_errno(-r); -1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    ret(file::miku_lseek(fd as i64, offset, whence as u64))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn open(path: *const u8, flags: i32, mode: u32) -> i32 {
    if path.is_null() { return fail(EFAULT); }
    let len = string::miku_strlen(path);
    let r = file::miku_open(path, len, flags as u32, mode);
    if r < 0 { errno::set_errno(-r); -1 } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn creat(path: *const u8, mode: u32) -> i32 {
    open(path, (file::O_WRITE | file::O_CREATE | file::O_TRUNCATE) as i32, mode)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dup(fd: i32) -> i32 {
    let r = file::miku_dup(fd as i64);
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dup2(old: i32, new: i32) -> i32 {
    let r = file::miku_dup2(old as i64, new as i64);
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pipe(fds: *mut i32) -> i32 {
    if fds.is_null() { return fail(EFAULT); }
    let mut fds64: [i64; 2] = [0; 2];
    let r = file::miku_pipe(fds64.as_mut_ptr());
    if r < 0 { return fail(-r as i32); }
    unsafe {
        *fds.add(0) = fds64[0] as i32;
        *fds.add(1) = fds64[1] as i32;
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn unlink(path: *const u8) -> i32 {
    let r = file::miku_unlink(path);
    if r < 0 { fail(-r as i32) } else { 0 }
}

/// Linux's `getrandom`: up to `len` bytes of kernel entropy (`getrandom`(78)).
/// `GRND_NONBLOCK` and `GRND_RANDOM` are accepted and change nothing, since the
/// pool does not block once the machine is up. The `getrandom` crate reaches
/// entropy through this
#[no_mangle]
#[inline(never)]
pub extern "C" fn getrandom(buf: *mut u8, len: usize, _flags: u32) -> i64 {
    if len == 0 { return 0; }
    if buf.is_null() { return fail(EFAULT) as i64; }
    // The kernel serves at most 1024 bytes a call and refuses more; Linux
    // returns a short count. So the request is cut to that
    let n = len.min(1024);
    ret(unsafe { crate::sys::sc2(crate::sys::SYS_GETRANDOM, buf as u64, n as u64) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rmdir(path: *const u8) -> i32 {
    let r = file::miku_rmdir(path);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn link(old: *const u8, new: *const u8) -> i32 {
    let r = file::miku_link(old, new);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn symlink(target: *const u8, linkpath: *const u8) -> i32 {
    let r = file::miku_symlink(target, linkpath);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readlink(path: *const u8, buf: *mut u8, bufsiz: usize) -> i64 {
    ret(file::miku_readlink(path, buf, bufsiz))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    proc::miku_getcwd(buf, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chdir(path: *const u8) -> i32 {
    let r = file::miku_chdir(path);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpid() -> i32 {
    proc::miku_getpid() as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn access(path: *const u8, mode: i32) -> i32 {
    // Honors `mode` and sets errno
    faccessat(AT_FDCWD, path, mode, 0)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sleep(seconds: u32) -> u32 {
    time::miku_sleep_ms(seconds as u64 * 1000);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn usleep(usec: u32) -> i32 {
    time::miku_sleep_ns(usec as u64 * 1000);
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ftruncate(fd: i32, length: i64) -> i32 {
    let r = file::miku_ftruncate(fd as i64, length as u64);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn truncate(path: *const u8, length: i64) -> i32 {
    let fd = file::miku_open_rw(path);
    if fd < 0 { return fail(-fd as i32); }
    let r = file::miku_ftruncate(fd, length as u64);
    file::miku_close(fd);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pread(fd: i32, buf: *mut u8, count: usize, offset: i64) -> i64 {
    ret(file::miku_pread(fd as i64, buf, count, offset))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pwrite(fd: i32, buf: *const u8, count: usize, offset: i64) -> i64 {
    ret(file::miku_pwrite(fd as i64, buf, count, offset))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sched_yield() -> i32 {
    time::miku_yield();
    0
}

// unistd.h / sys/wait.h: processes

#[no_mangle]
#[inline(never)]
pub extern "C" fn fork() -> i32 {
    use crate::pthread::{run_atfork, ForkPhase};
    run_atfork(ForkPhase::Prepare);
    let r = proc::miku_fork();
    if r < 0 {
        run_atfork(ForkPhase::Parent);
        set_errno(-r as i32);
        return -1;
    }
    run_atfork(if r == 0 { ForkPhase::Child } else { ForkPhase::Parent });
    r as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn execve(
    path: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    // C passes these NULL-terminated and MikuOS wants a count
    let argc = count_vec(argv);
    let envc = count_vec(envp);
    let r = proc::miku_execve(path, argv, argc, envp, envc);
    // `execve` returning at all is `execve` failing
    set_errno(if r < 0 { -r as i32 } else { ENOENT });
    -1
}

/// `execve` after a `PATH` search, as `execvp` does, with the given
/// environment. A name with a slash is not searched. Uses the calling process's
/// `PATH` (`/bin` without one), and reports the last error other than "not
/// there", as a shell does
#[no_mangle]
#[inline(never)]
pub extern "C" fn execvpe(
    file: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    if file.is_null() {
        return fail(ENOENT);
    }
    let name = unsafe { core::ffi::CStr::from_ptr(file as *const core::ffi::c_char) }.to_bytes();
    if name.is_empty() {
        return fail(ENOENT);
    }
    if name.contains(&b'/') {
        return execve(file, argv, envp);
    }
    let path = getenv(b"PATH\0".as_ptr());
    let path: &[u8] = if path.is_null() {
        b"/bin"
    } else {
        unsafe { core::ffi::CStr::from_ptr(path as *const core::ffi::c_char) }.to_bytes()
    };
    let mut err = ENOENT;
    let mut full = [0u8; 512];
    for dir in path.split(|&c| c == b':') {
        let dir: &[u8] = if dir.is_empty() { b"." } else { dir };
        let len = dir.len() + 1 + name.len();
        if len + 1 > full.len() {
            continue;
        }
        full[..dir.len()].copy_from_slice(dir);
        full[dir.len()] = b'/';
        full[dir.len() + 1..len].copy_from_slice(name);
        full[len] = 0;
        execve(full.as_ptr(), argv, envp);
        let e = unsafe { *errno::miku_errno_location() };
        if e != ENOENT && e != ENOTDIR {
            err = e;
        }
    }
    fail(err)
}

/// `ENOSYS`: the kernel executes a path, never an open descriptor
#[no_mangle]
#[inline(never)]
pub extern "C" fn fexecve(_fd: i32, _argv: *const *const u8, _envp: *const *const u8) -> i32 {
    fail(ENOSYS)
}

/// Length of a NULL-terminated pointer vector, capped against a missing
/// terminator
pub(crate) fn count_vec(v: *const *const u8) -> usize {
    if v.is_null() { return 0; }
    let mut n = 0usize;
    while n < 4096 {
        if unsafe { *v.add(n) }.is_null() { break; }
        n += 1;
    }
    n
}

pub use miku_abi::wait::WNOHANG;

pub use miku_abi::wait::WUNTRACED;

#[no_mangle]
#[inline(never)]
pub extern "C" fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32 {
    let mut raw: i64 = 0;
    let r = proc::miku_wait4(pid as u64, &mut raw, options as u64);
    if r < 0 { set_errno(-r as i32); return -1; }
    if !status.is_null() {
        // The kernel writes a POSIX status word into an `i64`; C reads an `int`
        unsafe { *status = raw as i32 };
    }
    r as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn kill(pid: i32, sig: i32) -> i32 {
    let r = proc::miku_kill(pid as u64, sig as u64);
    if r < 0 { set_errno(-r as i32); return -1; }
    0
}

/// Every member of the group, each permission-checked on its own
#[no_mangle]
#[inline(never)]
pub extern "C" fn killpg(pgrp: i32, sig: i32) -> i32 {
    if pgrp <= 1 { return fail(EINVAL); }
    kill(-pgrp, sig)
}

// Jobs: `setpgid`(101), `getpgid`(102); docs/MikuOS_ABI.md 3.2b

#[no_mangle]
#[inline(never)]
pub extern "C" fn setpgid(pid: i32, pgid: i32) -> i32 {
    if pid < 0 || pgid < 0 { return fail(EINVAL); }
    let r = unsafe { crate::sys::sc2(crate::sys::SYS_SETPGID, pid as u64, pgid as u64) };
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpgid(pid: i32) -> i32 {
    if pid < 0 { return fail(ESRCH); }
    let r = unsafe { crate::sys::sc1(crate::sys::SYS_GETPGID, pid as u64) };
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpgrp() -> i32 {
    getpgid(0)
}

/// MikuOS has jobs and no sessions. `setsid` here is leading a process group,
/// `setpgid(0, 0)`, so `killpg` on its pid reaches it and its children, and the
/// group is the answer. It cannot drop a controlling terminal. A group leader
/// is refused, as in POSIX
#[no_mangle]
#[inline(never)]
pub extern "C" fn setsid() -> i32 {
    let me = getpid();
    if getpgid(0) == me { return fail(EPERM); }
    if setpgid(0, 0) < 0 { return -1; }
    me
}

/// Until a signal handler has run, then `EINTR`. The kernel runs handlers on
/// the way out of a syscall, so this sleeps in short steps and reads
/// `sys_miku`'s count; a signal is noticed within one step
#[no_mangle]
#[inline(never)]
pub extern "C" fn pause() -> i32 {
    let seen = signal::miku_signal_handled();
    while signal::miku_signal_handled() == seen {
        time::miku_sleep_ms(10);
    }
    fail(EINTR)
}

/// `mask` in place while waiting for a handler, then the old mask back and
/// `EINTR`. The count is read before the mask changes, since a signal the new
/// mask unblocks is delivered on the way out of that call
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigsuspend(mask: *const [u64; 16]) -> i32 {
    let seen = signal::miku_signal_handled();
    let mut old = [0u64; 16];
    if sigprocmask(SIG_SETMASK, mask, &mut old) < 0 {
        return -1;
    }
    while signal::miku_signal_handled() == seen {
        time::miku_sleep_ms(10);
    }
    sigprocmask(SIG_SETMASK, &old, core::ptr::null_mut());
    fail(EINTR)
}

/// `ENOSYS`, returned as `sigwait` returns errors: nothing removes a pending
/// signal without running its handler
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigwait(_set: *const [u64; 16], _sig: *mut i32) -> i32 {
    ENOSYS
}

/// `ENOSYS`: no call changes a process's root directory
#[no_mangle]
#[inline(never)]
pub extern "C" fn chroot(_path: *const u8) -> i32 {
    fail(ENOSYS)
}

/// The signal's name, as `strsignal`; numbers are Linux's
#[no_mangle]
#[inline(never)]
pub extern "C" fn strsignal(sig: i32) -> *const u8 {
    const NAMES: [&[u8]; 32] = [
        b"Unknown signal 0\0", b"Hangup\0", b"Interrupt\0", b"Quit\0",
        b"Illegal instruction\0", b"Trace/breakpoint trap\0", b"Aborted\0", b"Bus error\0",
        b"Floating point exception\0", b"Killed\0", b"User defined signal 1\0",
        b"Segmentation fault\0", b"User defined signal 2\0", b"Broken pipe\0",
        b"Alarm clock\0", b"Terminated\0", b"Stack fault\0", b"Child exited\0",
        b"Continued\0", b"Stopped (signal)\0", b"Stopped\0", b"Stopped (tty input)\0",
        b"Stopped (tty output)\0", b"Urgent I/O condition\0", b"CPU time limit exceeded\0",
        b"File size limit exceeded\0", b"Virtual timer expired\0",
        b"Profiling timer expired\0", b"Window changed\0", b"I/O possible\0",
        b"Power failure\0", b"Bad system call\0",
    ];
    match NAMES.get(sig as usize) {
        Some(n) if sig > 0 => n.as_ptr(),
        _ => b"Unknown signal\0".as_ptr(),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getppid() -> i32 {
    // No `getppid` syscall: the parent is in the `procinfo`(103) table. Answers
    // that it does not know
    set_errno(ENOSYS);
    -1
}

// unistd.h: terminals

/// Whether this descriptor is a terminal: `ioctl(TCGETS)` succeeds on one and
/// fails with `ENOTTY` otherwise
#[no_mangle]
#[inline(never)]
pub extern "C" fn isatty(fd: i32) -> i32 {
    let mut t = [0u8; 32];
    use miku_abi::ioctl::TCGETS;
    let r = unsafe { crate::sys::sc3(crate::sys::SYS_IOCTL, fd as u64, TCGETS, t.as_mut_ptr() as u64) };
    if r < 0 { set_errno(ENOTTY); return 0; }
    1
}

pub use miku_abi::errno::ENOTTY;

/// Sets `errno`, for functions here reporting `-1`. MikuOS's own calls return
/// errors and do not touch this, so only this file sets it
pub(crate) fn set_errno(e: i32) {
    errno::set_errno(e as i64);
}

pub(crate) use miku_abi::fcntl::AT_FDCWD;

pub(crate) use miku_abi::fcntl::AT_SYMLINK_NOFOLLOW;

pub(crate) use miku_abi::fcntl::AT_EACCESS;

pub(crate) use miku_abi::unistd::R_OK;

pub(crate) use miku_abi::unistd::W_OK;

pub(crate) use miku_abi::unistd::X_OK;

pub(crate) use miku_abi::ioctl::FIONBIO;

pub(crate) use miku_abi::unistd::_SC_CLK_TCK;

pub(crate) use miku_abi::unistd::_SC_OPEN_MAX;

pub(crate) use miku_abi::unistd::_SC_PAGESIZE;

pub(crate) use miku_abi::unistd::_SC_GETPW_R_SIZE_MAX;

/// Per-process descriptor ceiling, `MAX_FDS` in `kernel/fs/vfs/types.rs`. A
/// kernel constant, as `getrlimit` and `setrlimit` below report
pub(crate) const OPEN_MAX: u64 = 512;

/// The scheduler tick, the resolution of both clocks `clock_gettime`(70) reads
pub(crate) const TICK_NS: i64 = 1_000_000_000 / 250;

/// `-1` with `errno` set, the C convention. MikuOS returns `-errno`, and
/// `rustix` treats anything but `-1` as a count, so every function here
/// converts through one of these two
pub(crate) fn fail(e: i32) -> i32 { set_errno(e); -1 }

pub(crate) fn ret(r: i64) -> i64 {
    if r < 0 { set_errno(-r as i32); -1 } else { r }
}

// unistd.h / fcntl.h: descriptors

/// `pipe2`(120): the kernel sets the flags as the ends are made, so a `fork` on
/// another thread cannot catch them without `O_CLOEXEC`. Flag values are the
/// kernel's
#[no_mangle]
#[inline(never)]
pub extern "C" fn pipe2(fds: *mut i32, flags: i32) -> i32 {
    if fds.is_null() { return fail(EINVAL); }
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 { return fail(EINVAL); }
    let mut ends: [i64; 2] = [0; 2];
    let r = file::miku_pipe2(ends.as_mut_ptr(), flags as u32);
    if r < 0 { return fail(-r as i32); }
    unsafe {
        *fds = ends[0] as i32;
        *fds.add(1) = ends[1] as i32;
    }
    0
}

/// `O_CLOEXEC` and `O_NONBLOCK` on an open descriptor, for `pipe2` and `dup3`
pub(crate) fn apply_fd_flags(fd: i64, flags: i32) -> i64 {
    if flags & O_CLOEXEC != 0 {
        let r = file::miku_fcntl(fd, F_SETFD as u64, FD_CLOEXEC as i64);
        if r < 0 { return r; }
    }
    if flags & O_NONBLOCK != 0 {
        let r = file::miku_set_nonblocking(fd, true);
        if r < 0 { return r; }
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dup3(oldfd: i32, newfd: i32, flags: i32) -> i32 {
    // `dup3` differs from `dup2`: the same descriptor twice is an error, and
    // the only flag is `O_CLOEXEC`
    if oldfd == newfd { return fail(EINVAL); }
    if flags & !O_CLOEXEC != 0 { return fail(EINVAL); }
    let r = file::miku_dup2(oldfd as i64, newfd as i64);
    if r < 0 { return fail(-r as i32); }
    let f = apply_fd_flags(r, flags);
    if f < 0 { return fail(-f as i32); }
    r as i32
}

/// `fsync`: there is no metadata-free flush, and `fdatasync` may do more than
/// asked
#[no_mangle]
#[inline(never)]
pub extern "C" fn fdatasync(fd: i32) -> i32 {
    fsync(fd)
}

/// Does nothing: no call flushes every filesystem, and `sync` cannot report
/// failure. `fsync` on a descriptor is real
#[no_mangle]
#[inline(never)]
pub extern "C" fn sync() {}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ioctl(fd: i32, request: u64, arg: *mut u8) -> i32 {
    // `FIONBIO` is `fcntl(F_SETFL, O_NONBLOCK)`, answered here since the
    // kernel's ioctl only knows terminals
    if request == FIONBIO {
        if arg.is_null() { return fail(EFAULT); }
        let on = unsafe { (arg as *const i32).read() } != 0;
        let r = file::miku_set_nonblocking(fd as i64, on);
        if r < 0 { return fail(-r as i32); }
        return 0;
    }
    // Terminal settings in Linux's layout: `c/termios.rs` translates the
    // kernel's twelve bytes, so `ioctl(TCGETS)` and `tcgetattr` agree
    if let Some(r) = crate::termios::ioctl_termios(fd, request, arg) {
        return r;
    }
    let r = io::miku_ioctl(fd as u64, request, arg as u64);
    if r < 0 { return fail(-r as i32); }
    r as i32
}

// unistd.h: sysconf

#[no_mangle]
#[inline(never)]
pub extern "C" fn sysconf(name: i32) -> i64 {
    match name {
        _SC_PAGESIZE => 4096,
        _SC_CLK_TCK  => 250,
        _SC_OPEN_MAX => OPEN_MAX as i64,
        // Buffer size for `getpwuid_r`: 1024, as in glibc
        _SC_GETPW_R_SIZE_MAX => 1024,
        // Each answer is where the limit lives; Linux's numbering, as the
        // `libc` crate and `<unistd.h>` send
        69 /* GETGR_R_SIZE_MAX */ => 1024,
        0 /* ARG_MAX: usercopy's MAX_ARGV_TOTAL_BYTES */ => 128 * 1024,
        3 /* NGROUPS_MAX: the credential's group array */ => 16,
        5 /* STREAM_MAX */ => OPEN_MAX as i64,
        6 /* TZNAME_MAX: UTC is the only zone */ => 3,
        60 /* IOV_MAX: readv/writev */ => 1024,
        43 /* LINE_MAX */ => 2048,
        71 /* LOGIN_NAME_MAX */ | 72 /* TTY_NAME_MAX */ => 32,
        180 /* HOST_NAME_MAX */ => 64,
        87 /* ATEXIT_MAX: libc's table */ => 32,
        74 /* THREAD_KEYS_MAX: c/pthread.rs */ => 128,
        73 /* THREAD_DESTRUCTOR_ITERATIONS */ => 4,
        75 /* THREAD_STACK_MIN: pthread_attr_setstacksize's floor */ => 16384,
        // POSIX's minimums, implemented only by this libc's regex
        36 /* BC_BASE_MAX */ | 38 /* BC_SCALE_MAX */ => 99,
        37 /* BC_DIM_MAX */ => 2048,
        39 /* BC_STRING_MAX */ => 1000,
        40 /* COLL_WEIGHTS_MAX */ => 2,
        42 /* EXPR_NEST_MAX */ => 32,
        44 /* RE_DUP_MAX */ => 255,
        // Options this system has, with POSIX.1-2008's version number: job
        // control, saved set-ids, fsync, file mappings, mprotect,
        // CLOCK_MONOTONIC, threads, thread-safe functions, a stack size
        // attribute
        7 | 8 | 15 | 16 | 19 | 149 | 67 | 68 | 78 => 200809,
        29 /* VERSION */ => 200809,
        // A limit this system lacks or an option it does not support: -1 with
        // errno untouched, per POSIX. Realtime signals, timers, AIO,
        // prioritized and synchronized I/O, mlock, message passing, semaphores,
        // shared memory objects, thread stack addresses and priorities,
        // process-shared sync, the POSIX.2 utilities, XSI shared memory
        1 /* CHILD_MAX */ | 76 /* THREAD_THREADS_MAX */
        | 9..=14 | 17 | 18 | 20..=28 | 31..=35 | 77 | 79..=82
        | 46..=52 | 94 | 95 | 97 => -1,
        // `_SC_NPROCESSORS_ONLN` among the rest: the CPU count stays in ring 0,
        // and `std`'s `available_parallelism` agrees
        _ => { set_errno(EINVAL); -1 }
    }
}

/// `ENOSYS`: `wait4`(44) is the only wait, and `waitid`'s `WNOWAIT` cannot be
/// done
#[no_mangle]
#[inline(never)]
pub extern "C" fn waitid(_idtype: i32, _id: u32, _infop: *mut u8, _options: i32) -> i32 {
    fail(ENOSYS)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getuid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETUID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn geteuid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETEUID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getgid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETGID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getegid() -> u32 {
    unsafe { crate::sys::sc0(crate::sys::SYS_GETEGID) as u32 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpagesize() -> i32 {
    4096
}

/// `kcmp(pid, pid, KCMP_FILE, fd1, fd2)` for the caller's descriptors. MikuOS
/// has no open file description apart from the descriptor, so the answer
/// compares what they name: 0 for the same vnode, 1 or 2 to order different
/// ones. Another process's descriptors and other `KCMP_*` are `ENOSYS`
#[no_mangle]
#[inline(never)]
pub extern "C" fn kcmp(pid1: i32, pid2: i32, typ: i32, idx1: u64, idx2: u64) -> i32 {
    let me = getpid();
    if typ != 0 || pid1 != me || pid2 != me { return fail(ENOSYS); }
    let mut a: stat = unsafe { core::mem::zeroed() };
    let mut b: stat = unsafe { core::mem::zeroed() };
    if fstat(idx1 as i32, &mut a) < 0 || fstat(idx2 as i32, &mut b) < 0 {
        return -1;
    }
    match (a.st_dev, a.st_ino).cmp(&(b.st_dev, b.st_ino)) {
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Less => 1,
        core::cmp::Ordering::Greater => 2,
    }
}
