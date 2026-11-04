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
//! `lib/miku/build.rs` generates, and at load time against the real ones in
//! `/lib`, so a program still depends on the `miku` crate for its link line.

#![allow(dead_code)]

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

/// A directory entry, as `getdents`(111) writes it: 272 bytes. A 64-bit inode,
/// matching `stat`(18), and a 256-byte name
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MikuDirent {
    pub name: [u8; 256],
    pub inode_id: u64,
    pub kind: u8,
    pub name_len: u8,
    pub _reserved: [u8; 6],
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

/// What `stat`(18), `lstat`(112) and `fstat`(19) write. 64 bytes, laid out in
/// `kernel/syscall/abi.rs`; `lib/libmiku/file.rs` is the other copy.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MikuStat {
    pub size: u64,
    pub mode: u32,
    pub nlinks: u32,
    pub uid: u16,
    pub gid: u16,
    pub kind: u8,
    pub fs_type: u8,
    pub dev_major: u8,
    pub dev_minor: u8,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub inode_id: u64,
    pub blocks: u32,
    pub _reserved: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MikuTimespec {
    pub sec: i64,
    pub nsec: i64,
}

pub const CLOCK_REALTIME: u64 = 0;
pub const CLOCK_MONOTONIC: u64 = 1;

// `open` flags, as `kernel/fs/vfs/types.rs` defines them. Not the Linux
// numbers, and not called O_* so that nothing passes them elsewhere
pub const OPEN_READ: u32 = 0x0001;
pub const OPEN_WRITE: u32 = 0x0002;
pub const OPEN_APPEND: u32 = 0x0004;
pub const OPEN_CREATE: u32 = 0x0008;
pub const OPEN_EXCLUSIVE: u32 = 0x0010;
pub const OPEN_TRUNCATE: u32 = 0x0020;
pub const OPEN_DIRECTORY: u32 = 0x0040;
pub const OPEN_NOFOLLOW: u32 = 0x0080;
pub const OPEN_CLOEXEC: u32 = 0x0800;

/// What a `VNodeKind` is on the wire, for the `kind` byte of a dirent and of a
/// `stat`. The numbers are the kernel's enum
pub const KIND_REGULAR: u8 = 0;
pub const KIND_DIRECTORY: u8 = 1;
pub const KIND_SYMLINK: u8 = 2;
pub const KIND_CHAR_DEVICE: u8 = 3;
pub const KIND_BLOCK_DEVICE: u8 = 4;
pub const KIND_PIPE: u8 = 5;
pub const KIND_SOCKET: u8 = 6;
pub const KIND_FIFO: u8 = 7;

/// `struct pollfd`, the kernel's eight bytes, for the one wait `std` does on a
/// descriptor itself, `TcpStream::connect_timeout`
#[repr(C)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub const POLLOUT: i16 = 0x004;
