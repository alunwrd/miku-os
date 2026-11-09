//! MikuOS
//!
//! MikuOS declares `target_family = "unix"` because its C interface is POSIX:
//! `fork`/`execve`/`wait4`, `open`/`read`/`write`/`lseek`, `poll`, `fcntl` and
//! BSD sockets, all exported by `libc_miku.so` under their POSIX names. See
//! `dev/std-architecture.md`.
//!
//! Types and constants are `lib/abi`, whose C-facing files the overlay copies to
//! `abi/` here (`builder/src/stdport/overlay.rs`). This file declares the
//! functions, against `lib/runtime/libmiku/libc/`.

use crate::prelude::*;

mod abi;

pub use self::abi::dirent::*;
pub use self::abi::dlfcn::*;
pub use self::abi::errno::*;
pub use self::abi::fcntl::*;
pub use self::abi::inet::*;
pub use self::abi::inotify::*;
pub use self::abi::ioctl::*;
pub use self::abi::limits::*;
pub use self::abi::locale::*;
pub use self::abi::mman::*;
pub use self::abi::netdb::*;
pub use self::abi::poll::*;
pub use self::abi::pthread::*;
pub use self::abi::pwd::*;
pub use self::abi::random::*;
pub use self::abi::resource::*;
pub use self::abi::sched::*;
pub use self::abi::select::*;
pub use self::abi::signal::*;
pub use self::abi::socket::*;
pub use self::abi::stat::*;
pub use self::abi::statvfs::*;
pub use self::abi::stdlib::*;
pub use self::abi::termios::*;
pub use self::abi::time::*;
pub use self::abi::types::*;
pub use self::abi::ucontext::*;
pub use self::abi::unistd::*;
pub use self::abi::utsname::*;
pub use self::abi::wait::*;


extern "C" {
    /// The reentrant `strerror`. XSI semantics: 0, or `ERANGE` when the
    /// buffer is too small; the buffer is always NUL-terminated
    pub fn strerror_r(errnum: c_int, buf: *mut c_char, buflen: size_t) -> c_int;
}

// The functions `rustix` reaches for
//
// Each is a real symbol in `libc_miku.so`. A missing definition is a link
// error; `lib/runtime/libmiku/libc/` implements what MikuOS can do and returns
// `ENOSYS` for the rest.
extern "C" {
    /// The descriptor inside a `DIR`, a field read: a `DIR` is a descriptor and
    /// an offset
    pub fn dirfd(dirp: *mut crate::unix::DIR) -> c_int;
    /// Rewind or reposition a `DIR` to an offset from `telldir`. The offset is
    /// an entry index, as is `getdents`(111)'s cursor: a position in the
    /// listing, not a stable cookie
    pub fn seekdir(dirp: *mut crate::unix::DIR, loc: c_long);
    pub fn telldir(dirp: *mut crate::unix::DIR) -> c_long;

    pub fn dup3(oldfd: c_int, newfd: c_int, flags: c_int) -> c_int;
    pub fn fdatasync(fd: c_int) -> c_int;
    pub fn sync();

    /// `mprotect`(5). `PROT_WRITE | PROT_EXEC` together loses `PROT_EXEC`
    /// (W^X), so a JIT writes RW and then asks for RX
    pub fn mprotect(addr: *mut c_void, len: size_t, prot: c_int) -> c_int;
    pub fn msync(addr: *mut c_void, len: size_t, flags: c_int) -> c_int;

    pub fn readv(fd: c_int, iov: *const crate::unix::iovec, iovcnt: c_int) -> ssize_t;
    pub fn writev(fd: c_int, iov: *const crate::unix::iovec, iovcnt: c_int) -> ssize_t;
    pub fn preadv(fd: c_int, iov: *const crate::unix::iovec, iovcnt: c_int, offset: off_t) -> ssize_t;
    pub fn pwritev(fd: c_int, iov: *const crate::unix::iovec, iovcnt: c_int, offset: off_t) -> ssize_t;

    /// Variadic, as in every libc: callers pass `&c_int` for `FIONBIO`, a
    /// struct pointer for a terminal. The definition in `libc_miku.so` takes
    /// three fixed arguments, which agrees: on SysV x86_64 the third arrives in
    /// `rdx` either way
    pub fn ioctl(fd: c_int, request: _Opcode, ...) -> c_int;

    pub fn statfs(path: *const c_char, buf: *mut statfs) -> c_int;
    pub fn fstatfs(fd: c_int, buf: *mut statfs) -> c_int;

    /// `faccessat` takes any `dirfd`, see `AT_FDCWD`. `mknodat` answers
    /// `ENOSYS` whatever it is
    pub fn faccessat(dirfd: c_int, path: *const c_char, mode: c_int, flags: c_int) -> c_int;
    pub fn mknodat(dirfd: c_int, path: *const c_char, mode: mode_t, dev: dev_t) -> c_int;
    pub fn utimensat(
        dirfd: c_int,
        path: *const c_char,
        times: *const crate::unix::timespec,
        flags: c_int,
    ) -> c_int;
    pub fn futimens(fd: c_int, times: *const crate::unix::timespec) -> c_int;

    pub fn posix_fadvise(fd: c_int, offset: off_t, len: off_t, advice: c_int) -> c_int;
    pub fn posix_fallocate(fd: c_int, offset: off_t, len: off_t) -> c_int;

    /// Always null; see `RTLD_DEFAULT`
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

extern "C" {
    pub fn madvise(addr: *mut c_void, len: size_t, advice: c_int) -> c_int;
    pub fn posix_madvise(addr: *mut c_void, len: size_t, advice: c_int) -> c_int;
    /// `ENOSYS`: a symlink's own times cannot be set
    pub fn lutimes(file: *const c_char, times: *const crate::timeval) -> c_int;
    pub fn pthread_sigmask(how: c_int, set: *const sigset_t, oldset: *mut sigset_t) -> c_int;
    pub fn pthread_mutexattr_setpshared(attr: *mut pthread_mutexattr_t, pshared: c_int) -> c_int;
    pub fn pthread_mutexattr_getpshared(attr: *const pthread_mutexattr_t, pshared: *mut c_int) -> c_int;
    /// `execvp` with an environment. `fexecve` is `ENOSYS`: the kernel
    /// executes paths, not descriptors (lib/runtime/libmiku/libc/)
    pub fn execvpe(file: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    pub fn fexecve(fd: c_int, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    pub fn sigsuspend(mask: *const sigset_t) -> c_int;
    /// Returns `ENOSYS`: nothing takes a signal off the queue unhandled
    pub fn sigwait(set: *const sigset_t, sig: *mut c_int) -> c_int;
    pub fn pthread_condattr_setclock(attr: *mut pthread_condattr_t, clock_id: clockid_t) -> c_int;
    pub fn pthread_condattr_getclock(attr: *const pthread_condattr_t, clock_id: *mut clockid_t) -> c_int;
    /// A terminal and both its ends: the master from `/dev/ptmx`, the slave
    /// `/dev/ptsN` (lib/runtime/libmiku/libc/pty.rs). `winp` sets the terminal's size
    pub fn openpty(
        amaster: *mut c_int,
        aslave: *mut c_int,
        name: *mut c_char,
        termp: *const termios,
        winp: *const crate::winsize,
    ) -> c_int;
    /// The slave as controlling terminal and fds 0-2: `TIOCSCTTY`, which is
    /// `tty(ATTACH)`
    pub fn login_tty(fd: c_int) -> c_int;
    pub fn ptsname_r(fd: c_int, buf: *mut c_char, buflen: size_t) -> c_int;
    /// `/etc/passwd`, into the caller's buffer
    pub fn getpwuid_r(
        uid: crate::uid_t,
        pwd: *mut passwd,
        buf: *mut c_char,
        buflen: size_t,
        result: *mut *mut passwd,
    ) -> c_int;
    pub fn getpwnam_r(
        name: *const c_char,
        pwd: *mut passwd,
        buf: *mut c_char,
        buflen: size_t,
        result: *mut *mut passwd,
    ) -> c_int;
}

/// `grp.h`, from `/etc/group` (`lib/runtime/libmiku/libc/accounts.rs`). The kernel's
/// supplementary list holds sixteen 16-bit groups; `setgroups` and `initgroups`
/// need `CAP_SETGID`
extern "C" {
    pub fn getgrgid_r(
        gid: crate::gid_t,
        grp: *mut crate::unix::group,
        buf: *mut c_char,
        buflen: size_t,
        result: *mut *mut crate::unix::group,
    ) -> c_int;
    pub fn getgrnam_r(
        name: *const c_char,
        grp: *mut crate::unix::group,
        buf: *mut c_char,
        buflen: size_t,
        result: *mut *mut crate::unix::group,
    ) -> c_int;
    pub fn getgrouplist(
        user: *const c_char,
        group: crate::gid_t,
        groups: *mut crate::gid_t,
        ngroups: *mut c_int,
    ) -> c_int;
    pub fn initgroups(user: *const c_char, group: crate::gid_t) -> c_int;
    pub fn setgroups(ngroups: size_t, ptr: *const crate::gid_t) -> c_int;
}

extern "C" {
    /// The release comes from `/proc/version`, the node name from
    /// `/etc/hostname`
    pub fn uname(buf: *mut utsname) -> c_int;
    /// `fork`'s handlers (`lib/runtime/libmiku/libc/pthread.rs`)
    pub fn pthread_atfork(
        prepare: Option<unsafe extern "C" fn()>,
        parent: Option<unsafe extern "C" fn()>,
        child: Option<unsafe extern "C" fn()>,
    ) -> c_int;
    /// Every thread is `SCHED_OTHER` at priority 0; a realtime policy is
    /// `ENOTSUP` (`lib/runtime/libmiku/libc/pthread.rs`)
    pub fn pthread_getschedparam(
        native: crate::pthread_t,
        policy: *mut c_int,
        param: *mut sched_param,
    ) -> c_int;
    pub fn pthread_setschedparam(
        native: crate::pthread_t,
        policy: c_int,
        param: *const sched_param,
    ) -> c_int;
    /// `ENOSYS`: no filesystem here stores a FIFO, and there is no
    /// per-filesystem flush
    pub fn mkfifoat(dirfd: c_int, pathname: *const c_char, mode: mode_t) -> c_int;
    pub fn syncfs(fd: c_int) -> c_int;
}

extern "C" {
    /// Only `KCMP_FILE`, only for the caller's own descriptors: 0 when both
    /// name the same thing, 1 or 2 to order them. `dup` copies the entry, so
    /// "the same file" is the same vnode. `inotify`'s `FdGuard` asks this
    pub fn kcmp(pid1: crate::unix::pid_t, pid2: crate::unix::pid_t, type_: c_int, idx1: c_ulong, idx2: c_ulong) -> c_int;

    /// Kernel entropy, `getrandom`(78). The flags change nothing: the pool does
    /// not block once the machine is up
    pub fn getrandom(buf: *mut c_void, buflen: size_t, flags: c_uint) -> ssize_t;
    /// This thread's `errno`, at `tp + 40` (`docs/MikuOS_ABI.md`, "The thread
    /// control block").
    pub fn __errno_location() -> *mut c_int;

    pub fn inotify_init() -> c_int;
    pub fn inotify_init1(flags: c_int) -> c_int;
    pub fn inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int;
    pub fn inotify_rm_watch(fd: c_int, wd: c_int) -> c_int;
}

extern "C" {
    /// This thread's alternate signal stack. Kept by the kernel per thread
    /// (`sigaltstack`(115)); `EPERM` while running on it, `ENOMEM` below
    /// `MINSIGSTKSZ`.
    pub fn sigaltstack(ss: *const stack_t, oss: *mut stack_t) -> c_int;
}

extern "C" {
    /// Answers the selectors in `lib/runtime/libmiku/libc/`; anything else is `-1`
    /// with `EINVAL`, `_SC_NPROCESSORS_ONLN` included, since the CPU count
    /// stays in ring 0
    pub fn sysconf(name: c_int) -> c_long;

    pub fn clock_gettime(clk_id: clockid_t, tp: *mut crate::unix::timespec) -> c_int;
    pub fn clock_getres(clk_id: clockid_t, tp: *mut crate::unix::timespec) -> c_int;
    pub fn clock_settime(clk_id: clockid_t, tp: *const crate::unix::timespec) -> c_int;

    /// One of the sixteen limits is enforced; see `RLIMIT_NOFILE`. `rlimit` is
    /// `libc`'s own, from `src/unix/mod.rs`
    pub fn getrlimit(resource: c_int, rlim: *mut crate::unix::rlimit) -> c_int;
    pub fn setrlimit(resource: c_int, rlim: *const crate::unix::rlimit) -> c_int;

    pub fn getpriority(which: c_int, who: id_t) -> c_int;
    pub fn setpriority(which: c_int, who: id_t, prio: c_int) -> c_int;

    pub fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;

    pub fn waitid(
        idtype: c_int,
        id: id_t,
        infop: *mut siginfo_t,
        options: c_int,
    ) -> c_int;
}

extern "C" {
    /// Each a real symbol in `libc_miku.so`, over syscalls 56-66 and 77. See
    /// the block comment above for what they refuse
    pub fn bind(socket: c_int, address: *const sockaddr, address_len: socklen_t) -> c_int;
    pub fn accept4(
        socket: c_int,
        address: *mut sockaddr,
        address_len: *mut socklen_t,
        flags: c_int,
    ) -> c_int;
    pub fn recvfrom(
        socket: c_int,
        buf: *mut c_void,
        len: size_t,
        flags: c_int,
        addr: *mut sockaddr,
        addrlen: *mut socklen_t,
    ) -> ssize_t;
    pub fn sendmsg(fd: c_int, msg: *const msghdr, flags: c_int) -> ssize_t;
    pub fn recvmsg(fd: c_int, msg: *mut msghdr, flags: c_int) -> ssize_t;
}
