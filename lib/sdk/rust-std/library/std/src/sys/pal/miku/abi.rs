//! What MikuOS provides, declared once.
//!
//! The PAL calls `libmiku`, not the syscalls: the `*_miku.so` libraries already
//! wrap them, with one errno translation and one path discipline
//! (`dev/std-architecture.md`).
//!
//! Upstream's example of this shape is `target_os = "motor"`, whose PAL is a
//! thin adapter over a `moto-rt` crate declared in `library/std/Cargo.toml`.
//! MikuOS uses an `extern` block instead of a cargo dependency.
//!
//! The symbols resolve at link time against the stub libraries
//! `lib/sdk/miku/build.rs` generates, and at load time against the real ones in
//! `/lib`, so a program still depends on the `miku` crate for its link line.

#![allow(dead_code)]

/// `lib/abi`, copied here by the overlay (`builder/src/stdport/overlay.rs`)
#[path = "defs/mod.rs"]
#[allow(unsafe_op_in_unsafe_fn, fuzzy_provenance_casts, lossy_provenance_casts, unused, missing_docs)]
pub mod defs;

pub use defs::kstat::{MikuDirent, MikuStat, MikuTimespec};

unsafe extern "C" {
    // io
    pub safe fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64;
    pub safe fn miku_read(fd: u64, buf: *mut u8, len: usize) -> i64;

    // files
    pub safe fn miku_open(path: *const u8, len: usize, flags: u32, mode: u32) -> i64;
    pub safe fn miku_close(fd: i64) -> i64;
    pub safe fn miku_dup(fd: i64) -> i64;
    pub safe fn miku_pipe(fds: *mut i64) -> i64;
    pub safe fn miku_pipe2(fds: *mut i64, flags: u32) -> i64;
    pub safe fn miku_fcntl(fd: i64, cmd: u64, arg: i64) -> i64;
    pub safe fn miku_set_nonblocking(fd: i64, on: bool) -> i64;
    pub safe fn miku_stat(path: *const u8, st: *mut MikuStat) -> i64;
    pub safe fn miku_lstat(path: *const u8, st: *mut MikuStat) -> i64;
    pub safe fn miku_fstat(fd: i64, st: *mut MikuStat) -> i64;
    pub safe fn miku_isdir(path: *const u8) -> bool;
    /// `ioctl(TCGETS)`, which succeeds on a terminal and fails on anything
    /// else. A C name, as `libc_miku` exports it
    pub safe fn isatty(fd: i32) -> i32;
    pub safe fn miku_fsync(fd: i64) -> i64;
    pub safe fn miku_ftruncate(fd: i64, size: u64) -> i64;
    pub safe fn miku_unlink(path: *const u8) -> i64;
    pub safe fn miku_rmdir(path: *const u8) -> i64;
    pub safe fn miku_mkdir(path: *const u8, mode: u32) -> i64;
    pub safe fn miku_rename(old: *const u8, new: *const u8) -> i64;
    pub safe fn miku_lseek(fd: i64, offset: i64, whence: u64) -> i64;
    pub safe fn miku_symlink(target: *const u8, linkpath: *const u8) -> i64;
    pub safe fn miku_readlink(path: *const u8, buf: *mut u8, size: usize) -> i64;
    pub safe fn miku_realpath(path: *const u8, buf: *mut u8, size: usize) -> i64;
    pub safe fn miku_utimensat(path: *const u8, fd: i32, times: *const i64, flags: i32) -> i64;
    pub safe fn miku_fchmod(fd: i32, mode: u32) -> i64;
    pub safe fn miku_link(target: *const u8, linkpath: *const u8) -> i64;
    pub safe fn miku_chmod(path: *const u8, mode: u32) -> i64;
    pub safe fn miku_getdents(fd: i64, entries: *mut MikuDirent, max: usize, offset: usize) -> i64;
    pub safe fn miku_getcwd(buf: *mut u8, size: usize) -> *mut u8;
    pub safe fn miku_chdir(path: *const u8) -> i64;
    pub safe fn miku_proc_path(pid: u64, buf: *mut u8, len: u64) -> i64;

    // memory
    pub safe fn miku_malloc(size: usize) -> *mut u8;
    pub safe fn miku_free(ptr: *mut u8);
    pub safe fn miku_realloc(ptr: *mut u8, new_size: usize) -> *mut u8;
    pub safe fn miku_calloc(count: usize, size: usize) -> *mut u8;
    pub safe fn miku_memalign(align: usize, size: usize) -> *mut u8;

    // process
    pub safe fn miku_exit(code: i64) -> !;
    /// `SIG_IGN` in the kernel, for SIGPIPE at start (`init`)
    pub safe fn miku_signal_ignore(sig: u32) -> i32;
    /// A handler, or 0 for the default, which is how a spawned child gets
    /// SIGPIPE back. Returns the previous handler, or 0
    pub safe fn miku_signal(sig: u32, handler: usize) -> usize;
    pub safe fn miku_getpid() -> u64;
    pub safe fn miku_fork() -> i64;
    pub safe fn miku_execve(
        path: *const u8,
        argv: *const *const u8,
        argc: usize,
        envp: *const *const u8,
        envc: usize,
    ) -> i64;
    pub safe fn miku_wait4(pid: u64, status: *mut i64, options: u64) -> i64;
    pub safe fn miku_kill(pid: u64, sig: u64) -> i64;
    pub safe fn miku_dup2(old_fd: i64, new_fd: i64) -> i64;
    // what `CommandExt` asks the child to become before it execs
    pub safe fn miku_setuid(uid: u64) -> i64;
    pub safe fn miku_setgid(gid: u64) -> i64;
    pub safe fn miku_setpgid(pid: u64, pgid: u64) -> i64;

    // environment
    pub safe fn miku_getenv(key: *const u8) -> *const u8;
    pub safe fn miku_setenv(key: *const u8, val: *const u8) -> bool;
    pub safe fn miku_unsetenv(key: *const u8) -> bool;
    pub safe fn miku_environ() -> *mut *mut u8;
    pub safe fn miku_env_init(envp: *const *const u8) -> usize;

    // sockets. IPv4 only; an address is four bytes and a port, `libmiku`'s
    // shape and the stack's
    pub safe fn miku_socket() -> i64;
    pub safe fn miku_socket_udp() -> i64;
    pub safe fn miku_connect(fd: i64, ip: *const u8, port: u16) -> i64;
    pub safe fn miku_send(fd: i64, buf: *const u8, len: usize) -> i64;
    pub safe fn miku_recv(fd: i64, buf: *mut u8, len: usize) -> i64;
    pub safe fn miku_bind(fd: i64, port: u16) -> i64;
    pub safe fn miku_listen(fd: i64, backlog: i64) -> i64;
    pub safe fn miku_accept(fd: i64, ip: *mut u8, port: *mut u16) -> i64;
    pub safe fn miku_sendto(fd: i64, buf: *const u8, len: usize, ip: *const u8, port: u16) -> i64;
    pub safe fn miku_recvfrom(
        fd: i64, buf: *mut u8, len: usize, src_ip: *mut u8, src_port: *mut u16,
    ) -> i64;
    pub safe fn miku_set_ttl(fd: i64, ttl: u8) -> i64;
    pub safe fn miku_getsockopt(fd: i64, level: u64, name: u64) -> i64;

    // this thread's errno: set by `libc_miku.so`'s C-convention functions,
    // read by `io::Error::last_os_error`
    pub safe fn miku_errno_location() -> *mut i32;
    pub safe fn miku_poll(fds: *mut PollFd, nfds: u64, timeout_ms: i64) -> i64;

    // local sockets: AF_UNIX stream, named by a NUL-terminated path
    pub safe fn miku_socket_unix() -> i64;
    pub safe fn miku_bind_unix(fd: i64, path: *const u8) -> i64;
    pub safe fn miku_connect_unix(fd: i64, path: *const u8) -> i64;
    pub safe fn miku_socketpair(fds: *mut u64) -> i64;

    // memory mapping, for what the allocator does not cover: a thread's
    // stack, and the join word that has to outlive it
    pub safe fn miku_mmap(addr: u64, len: usize, prot: u64) -> *mut u8;
    pub safe fn miku_munmap(addr: *mut u8, len: usize) -> i64;

    // threads, and the primitive they rest on
    pub safe fn miku_thread_spawn_owned(
        entry: extern "C" fn(*mut u8) -> i32,
        arg: *mut u8,
        stack: *mut u8,
        stack_size: usize,
    ) -> i64;
    pub safe fn miku_gettid() -> i64;
    pub safe fn miku_sched_yield() -> i64;
    pub safe fn miku_futex_wait(addr: *const u32, expected: u32, timeout_ns: u64) -> i64;
    pub safe fn miku_futex_wake(addr: *const u32, count: u32) -> i64;

    // entropy and the clock
    pub safe fn miku_getrandom(buf: *mut u8, len: usize) -> i64;
    pub safe fn miku_clock_gettime(clock: u64, ts: *mut MikuTimespec) -> i64;
    pub safe fn miku_sleep_ms(ms: u64);
    pub safe fn miku_sleep_ns(ns: u64);

    // the string helpers the interfaces above are specified in terms of
    pub safe fn miku_strlen(s: *const u8) -> usize;
}


impl MikuDirent {
    pub const fn zeroed() -> Self {
        MikuDirent { name: [0; 256], inode_id: 0, kind: 0, name_len: 0, _reserved: [0; 6] }
    }

    /// The name, without the padding and without the terminator.
    pub fn name_bytes(&self) -> &[u8] {
        let n = (self.name_len as usize).min(self.name.len() - 1);
        &self.name[..n]
    }
}



pub const CLOCK_REALTIME: u64 = defs::time::CLOCK_REALTIME as u64;
pub const CLOCK_MONOTONIC: u64 = defs::time::CLOCK_MONOTONIC as u64;

// `open` flags, as `kernel/fs/vfs/types.rs` defines them. Not the Linux
// numbers, and not called O_* so that nothing passes them elsewhere
pub const OPEN_READ: u32 = defs::fcntl::O_RDONLY as u32;
pub const OPEN_WRITE: u32 = defs::fcntl::O_WRONLY as u32;
pub const OPEN_APPEND: u32 = defs::fcntl::O_APPEND as u32;
pub const OPEN_CREATE: u32 = defs::fcntl::O_CREAT as u32;
pub const OPEN_EXCLUSIVE: u32 = defs::fcntl::O_EXCL as u32;
pub const OPEN_TRUNCATE: u32 = defs::fcntl::O_TRUNC as u32;
pub const OPEN_DIRECTORY: u32 = defs::fcntl::O_DIRECTORY as u32;
pub const OPEN_NOFOLLOW: u32 = defs::fcntl::O_NOFOLLOW as u32;
pub const OPEN_CLOEXEC: u32 = defs::fcntl::O_CLOEXEC as u32;

/// What a `VNodeKind` is on the wire, for the `kind` byte of a dirent and of a
/// `stat`. The numbers are the kernel's enum
pub use defs::kstat::KIND_REGULAR;
pub use defs::kstat::KIND_DIRECTORY;
pub use defs::kstat::KIND_SYMLINK;
pub use defs::kstat::KIND_CHARDEV as KIND_CHAR_DEVICE;
pub use defs::kstat::KIND_BLOCKDEV as KIND_BLOCK_DEVICE;
pub use defs::kstat::KIND_PIPE;
pub use defs::kstat::KIND_SOCKET;
pub use defs::kstat::KIND_FIFO;

pub use defs::posix::pollfd as PollFd;

pub use defs::poll::POLLOUT;
