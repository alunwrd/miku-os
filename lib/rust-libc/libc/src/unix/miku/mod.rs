//! MikuOS
//!
//! MikuOS declares `target_family = "unix"` because its C interface is POSIX:
//! `fork`/`execve`/`wait4`, `open`/`read`/`write`/`lseek`, `poll`, `fcntl` and
//! BSD sockets, all exported by `libc_miku.so` under their POSIX names. See
//! `dev/std-architecture.md`.
//!
//! There is no header. `lib/libmiku/libc.rs` and `lib/libmiku/c/` are the
//! interface, and this file is written against them. If the two differ, the
//! MikuOS side is the definition.

use crate::prelude::*;

pub type wchar_t = i32;

pub type time_t = i64;
pub type clock_t = i64;
pub type suseconds_t = i64;
pub type off_t = i64;
pub type dev_t = u64;
pub type ino_t = u64;
pub type mode_t = u32;
pub type nlink_t = u64;
pub type blksize_t = i64;
pub type blkcnt_t = i64;
pub type socklen_t = u32;
pub type nfds_t = u64;
pub type rlim_t = u64;
pub type speed_t = u32;
/// `statvfs`'s block and file counts
pub type fsblkcnt_t = u64;
pub type fsfilcnt_t = u64;
pub type tcflag_t = u32;
pub type cc_t = u8;
pub type sa_family_t = u16;
pub type in_port_t = u16;
pub type in_addr_t = u32;
/// `id_t`: what `getpriority`, `setpriority` and `waitid` take: a pid, a pgid
/// or a uid
pub type id_t = u32;
pub type pthread_t = usize;
pub type pthread_key_t = u32;

/// `sigset_t`: 1024 bits, as on Linux. MikuOS numbers 31 signals, and
/// `change_mask` in `lib/libmiku/libc.rs` reads the low ones
pub type sigset_t = [u64; 16];

/// `sem_t`: opaque and unimplemented
pub type sem_t = [u64; 4];

// The pthread types, sized as an ELF built elsewhere expects. Implemented by
// `lib/libmiku/c/pthread.rs`; `lib/c/check/layout.c` pins the sizes
pub type pthread_mutex_t = [u64; 5];
pub type pthread_mutexattr_t = [u64; 4];
/// Private only: `pthread_mutexattr_setpshared` refuses SHARED with `ENOTSUP`
pub const PTHREAD_PROCESS_PRIVATE: c_int = 0;
pub const PTHREAD_PROCESS_SHARED: c_int = 1;
pub type pthread_cond_t = [u64; 6];
pub type pthread_condattr_t = [u64; 4];
pub type pthread_rwlock_t = [u64; 7];
pub type pthread_rwlockattr_t = [u64; 4];
pub type pthread_attr_t = [u64; 7];

s! {
    /// A directory entry, 272 bytes: what `getdents`(111) writes
    ///
    /// `readdir` in `lib/libmiku/libc.rs` returns a pointer into the batch the
    /// syscall filled, so the C view and the kernel's share one layout.
    ///
    /// The name is 256 bytes; the inode is 64 bits, the same as `stat` reports
    /// (`kernel/syscall/abi.rs`).
    pub struct dirent {
        pub d_name: [c_char; 256],
        pub d_ino: u64,
        pub d_type: u8,
        pub d_namlen: u8,
        pub d_reserved: [u8; 6],
    }

    pub struct sockaddr {
        pub sa_family: sa_family_t,
        pub sa_data: [c_char; 14],
    }

    pub struct in_addr {
        pub s_addr: in_addr_t,
    }

    pub struct sockaddr_in {
        pub sin_family: sa_family_t,
        pub sin_port: in_port_t,
        pub sin_addr: in_addr,
        pub sin_zero: [u8; 8],
    }

    pub struct addrinfo {
        pub ai_flags: c_int,
        pub ai_family: c_int,
        pub ai_socktype: c_int,
        pub ai_protocol: c_int,
        pub ai_addrlen: socklen_t,
        pub ai_addr: *mut sockaddr,
        pub ai_canonname: *mut c_char,
        pub ai_next: *mut addrinfo,
    }

    pub struct tm {
        pub tm_sec: c_int,
        pub tm_min: c_int,
        pub tm_hour: c_int,
        pub tm_mday: c_int,
        pub tm_mon: c_int,
        pub tm_year: c_int,
        pub tm_wday: c_int,
        pub tm_yday: c_int,
        pub tm_isdst: c_int,
    }

    pub struct Dl_info {
        pub dli_fname: *const c_char,
        pub dli_fbase: *mut c_void,
        pub dli_sname: *const c_char,
        pub dli_saddr: *mut c_void,
    }

    pub struct fd_set {
        fds_bits: [u64; 16],
    }

    /// `termios`, smaller than a Linux one
    ///
    /// `ioctl(TCGETS)` fills `icanon`, `echo`, `isig`, `c_cc`, `vmin`, `vtime`
    /// and `tostop`. No baud rates, modem lines, or
    /// `c_iflag`/`c_oflag`/`c_cflag` bitmasks. The honoured flags are in
    /// `kernel/syscall/adapters/io.rs`
    pub struct termios {
        pub c_iflag: tcflag_t,
        pub c_oflag: tcflag_t,
        pub c_cflag: tcflag_t,
        pub c_lflag: tcflag_t,
        pub c_line: cc_t,
        pub c_cc: [cc_t; 32],
        pub c_ispeed: speed_t,
        pub c_ospeed: speed_t,
    }

    /// `passwd`. Accounts are `/etc/passwd`; `libc_miku.so` has the reentrant
    /// lookups (`getpwuid_r`, `getpwnam_r`), not the static-buffer ones
    pub struct passwd {
        pub pw_name: *mut c_char,
        pub pw_passwd: *mut c_char,
        pub pw_uid: crate::uid_t,
        pub pw_gid: crate::gid_t,
        pub pw_gecos: *mut c_char,
        pub pw_dir: *mut c_char,
        pub pw_shell: *mut c_char,
    }

    /// `stat`, in Linux's 144-byte layout. The kernel's is 64 bytes, and
    /// `libc_miku.so`'s `stat`, `fstat` and `lstat` translate it (`to_c_stat`
    /// in `lib/libmiku/libc.rs`): the file type goes into `st_mode`, `st_dev`
    /// is the filesystem, `st_rdev` the device of a `/dev` node, `st_blksize`
    /// 4096, and the nanosecond fields zero, since the kernel keeps whole
    /// seconds.
    pub struct stat {
        pub st_dev: crate::dev_t,
        pub st_ino: crate::ino_t,
        pub st_nlink: crate::nlink_t,
        pub st_mode: crate::mode_t,
        pub st_uid: crate::uid_t,
        pub st_gid: crate::gid_t,
        __pad0: c_int,
        pub st_rdev: crate::dev_t,
        pub st_size: crate::off_t,
        pub st_blksize: crate::blksize_t,
        pub st_blocks: crate::blkcnt_t,
        pub st_atime: crate::time_t,
        pub st_atime_nsec: i64,
        pub st_mtime: crate::time_t,
        pub st_mtime_nsec: i64,
        pub st_ctime: crate::time_t,
        pub st_ctime_nsec: i64,
        __unused: [i64; 3],
    }

    /// `statvfs`, in POSIX's shape
    ///
    /// The syscall writes 48 bytes laid out in `kernel/syscall/abi.rs`;
    /// `lib/libmiku/libc.rs` translates. Four fields have no source here:
    ///
    ///   * `f_frsize` is `f_bsize`: the VFS has one block size.
    ///   * `f_bavail` is `f_bfree`: no blocks are reserved for root.
    ///   * `f_favail` has no field; `rustix` reads `f_ffree` for it.
    ///   * `f_fsid` is 0, no device numbers. See `fsid_t`.
    pub struct statvfs {
        pub f_bsize: c_ulong,
        pub f_frsize: c_ulong,
        pub f_blocks: u64,
        pub f_bfree: u64,
        pub f_bavail: u64,
        pub f_files: u64,
        pub f_ffree: u64,
        pub f_favail: u64,
        pub f_fsid: c_ulong,
        pub f_flag: c_ulong,
        pub f_namemax: c_ulong,
        /// The filesystem magic from `statfs`(36), which POSIX's `statvfs` has
        /// no place for. Tells ext2 from ext4 from tmpfs
        pub f_type: c_ulong,
    }

    /// `sigaction`, in Linux's shape. A signal has a disposition in the kernel
    /// (`sigdisp`(100)) and a handler table in `sys_miku.so`, and
    /// `libc_miku.so`'s `sigaction` translates onto both. `SA_SIGINFO`,
    /// `SA_NODEFER`, `SA_RESETHAND` and `SA_ONSTACK` work; `SA_RESTART` and
    /// `SA_NOCLDSTOP` are accepted and have no effect (see
    /// `lib/libmiku/libc.rs`). The mask covers the 31 signals MikuOS numbers.
    pub struct sigaction {
        pub sa_sigaction: usize,
        pub sa_mask: sigset_t,
        pub sa_flags: c_int,
        pub sa_restorer: Option<extern "C" fn()>,
    }

    pub struct lconv {
        pub decimal_point: *mut c_char,
        pub thousands_sep: *mut c_char,
        pub grouping: *mut c_char,
        pub int_curr_symbol: *mut c_char,
        pub currency_symbol: *mut c_char,
        pub mon_decimal_point: *mut c_char,
        pub mon_thousands_sep: *mut c_char,
        pub mon_grouping: *mut c_char,
        pub positive_sign: *mut c_char,
        pub negative_sign: *mut c_char,
        pub int_frac_digits: c_char,
        pub frac_digits: c_char,
        pub p_cs_precedes: c_char,
        pub p_sep_by_space: c_char,
        pub n_cs_precedes: c_char,
        pub n_sep_by_space: c_char,
        pub p_sign_posn: c_char,
        pub n_sign_posn: c_char,
        pub int_p_cs_precedes: c_char,
        pub int_p_sep_by_space: c_char,
        pub int_n_cs_precedes: c_char,
        pub int_n_sep_by_space: c_char,
        pub int_p_sign_posn: c_char,
        pub int_n_sign_posn: c_char,
    }
}

// The errno numbers
//
// Linux's values, the same as `kernel/syscall/errno.rs`. `EWOULDBLOCK` is
// `EAGAIN`, and `ENOTSUP` is `EOPNOTSUPP`.
pub const EPERM: c_int = 1;
pub const ENOENT: c_int = 2;
pub const ESRCH: c_int = 3;
pub const EINTR: c_int = 4;
pub const EIO: c_int = 5;
pub const ENXIO: c_int = 6;
pub const E2BIG: c_int = 7;
pub const ENOEXEC: c_int = 8;
pub const EBADF: c_int = 9;
pub const ECHILD: c_int = 10;
pub const EAGAIN: c_int = 11;
pub const EWOULDBLOCK: c_int = EAGAIN;
pub const ENOMEM: c_int = 12;
pub const EACCES: c_int = 13;
pub const EFAULT: c_int = 14;
pub const EBUSY: c_int = 16;
pub const EEXIST: c_int = 17;
pub const EXDEV: c_int = 18;
pub const ENODEV: c_int = 19;
pub const ENOTDIR: c_int = 20;
pub const EISDIR: c_int = 21;
pub const EINVAL: c_int = 22;
pub const ENFILE: c_int = 23;
pub const EMFILE: c_int = 24;
pub const ENOTTY: c_int = 25;
pub const EFBIG: c_int = 27;
pub const ENOSPC: c_int = 28;
pub const ESPIPE: c_int = 29;
pub const EROFS: c_int = 30;
pub const EMLINK: c_int = 31;
pub const EPIPE: c_int = 32;
pub const EDOM: c_int = 33;
pub const ERANGE: c_int = 34;
pub const ENAMETOOLONG: c_int = 36;
pub const ENOSYS: c_int = 38;
pub const ENOTEMPTY: c_int = 39;
pub const ELOOP: c_int = 40;
pub const EOPNOTSUPP: c_int = 95;
pub const ENOTSUP: c_int = EOPNOTSUPP;
pub const EADDRINUSE: c_int = 98;
pub const EADDRNOTAVAIL: c_int = 99;
pub const ECONNABORTED: c_int = 103;
pub const ECONNRESET: c_int = 104;
pub const ENOTCONN: c_int = 107;
pub const ETIMEDOUT: c_int = 110;
pub const ECONNREFUSED: c_int = 111;

// `poll`(76), rescanned each call
/// `mmap`'s protections and flags, in Linux's values, as `mmap_file`(60) reads
/// them (`kernel/syscall/adapters/memory.rs`). An anonymous mapping is
/// `mmap`(3); one with a descriptor and no `MAP_ANONYMOUS` is `mmap_file`,
/// shared or private.
pub const PROT_NONE: c_int = 0;
pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const PROT_EXEC: c_int = 4;
pub const MAP_SHARED: c_int = 0x01;
pub const MAP_PRIVATE: c_int = 0x02;
pub const MAP_FIXED: c_int = 0x10;
pub const MAP_ANONYMOUS: c_int = 0x20;
pub const MAP_ANON: c_int = MAP_ANONYMOUS;
pub const MAP_FAILED: *mut c_void = !0 as *mut c_void;

pub const POLLIN: c_short = 0x001;
pub const POLLPRI: c_short = 0x002;
pub const POLLOUT: c_short = 0x004;
pub const POLLERR: c_short = 0x008;
pub const POLLHUP: c_short = 0x010;
pub const POLLNVAL: c_short = 0x020;

// `fcntl`, and the two flags changeable on an open descriptor. The access mode
// is not
pub const F_DUPFD: c_int = 0;
pub const F_GETFD: c_int = 1;
pub const F_SETFD: c_int = 2;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const FD_CLOEXEC: c_int = 1;

// `open` flags, as `kernel/fs/vfs/types.rs` defines them, not the Linux numbers
pub const O_RDONLY: c_int = 0x0001;
pub const O_WRONLY: c_int = 0x0002;
pub const O_RDWR: c_int = 0x0003;
pub const O_APPEND: c_int = 0x0004;
pub const O_CREAT: c_int = 0x0008;
pub const O_EXCL: c_int = 0x0010;
pub const O_TRUNC: c_int = 0x0020;
pub const O_DIRECTORY: c_int = 0x0040;
pub const O_NOFOLLOW: c_int = 0x0080;
pub const O_SYNC: c_int = 0x0200;
pub const O_NONBLOCK: c_int = 0x0400;
pub const O_CLOEXEC: c_int = 0x0800;

pub const WNOHANG: c_int = 1;
pub const WUNTRACED: c_int = 2;

pub const AF_UNIX: c_int = 1;
pub const AF_INET: c_int = 2;
pub const SOCK_STREAM: c_int = 1;
pub const SOCK_DGRAM: c_int = 2;

extern "C" {
    /// The reentrant `strerror`. XSI semantics: 0, or `ERANGE` when the
    /// buffer is too small; the buffer is always NUL-terminated
    pub fn strerror_r(errnum: c_int, buf: *mut c_char, buflen: size_t) -> c_int;
}

// The rest of the errno table
//
// Linux's numbers, the same space as `kernel/syscall/errno.rs`. Declared in
// full because crates name constants the system never produces: `rustix`'s
// `Errno` enumerates the whole table in one `match`.
//
// A name here is no promise that the kernel returns it; see
// `kernel/syscall/errno.rs`.
pub const ENOTBLK: c_int = 15;
pub const ETXTBSY: c_int = 26;
pub const EDEADLK: c_int = 35;
pub const EDEADLOCK: c_int = EDEADLK;
pub const ENOLCK: c_int = 37;
pub const ENOMSG: c_int = 42;
pub const EIDRM: c_int = 43;
pub const ECHRNG: c_int = 44;
pub const EL2NSYNC: c_int = 45;
pub const EL3HLT: c_int = 46;
pub const EL3RST: c_int = 47;
pub const ELNRNG: c_int = 48;
pub const EUNATCH: c_int = 49;
pub const ENOCSI: c_int = 50;
pub const EL2HLT: c_int = 51;
pub const EBADE: c_int = 52;
pub const EBADR: c_int = 53;
pub const EXFULL: c_int = 54;
pub const ENOANO: c_int = 55;
pub const EBADRQC: c_int = 56;
pub const EBADSLT: c_int = 57;
pub const EBFONT: c_int = 59;
pub const ENOSTR: c_int = 60;
pub const ENODATA: c_int = 61;
pub const ETIME: c_int = 62;
pub const ENOSR: c_int = 63;
pub const ENONET: c_int = 64;
pub const ENOPKG: c_int = 65;
pub const EREMOTE: c_int = 66;
pub const ENOLINK: c_int = 67;
pub const EADV: c_int = 68;
pub const ESRMNT: c_int = 69;
pub const ECOMM: c_int = 70;
pub const EPROTO: c_int = 71;
pub const EMULTIHOP: c_int = 72;
pub const EDOTDOT: c_int = 73;
pub const EBADMSG: c_int = 74;
pub const EOVERFLOW: c_int = 75;
pub const ENOTUNIQ: c_int = 76;
pub const EBADFD: c_int = 77;
pub const EREMCHG: c_int = 78;
pub const ELIBACC: c_int = 79;
pub const ELIBBAD: c_int = 80;
pub const ELIBSCN: c_int = 81;
pub const ELIBMAX: c_int = 82;
pub const ELIBEXEC: c_int = 83;
pub const EILSEQ: c_int = 84;
pub const ERESTART: c_int = 85;
pub const ESTRPIPE: c_int = 86;
pub const EUSERS: c_int = 87;
pub const ENOTSOCK: c_int = 88;
pub const EDESTADDRREQ: c_int = 89;
pub const EMSGSIZE: c_int = 90;
pub const EPROTOTYPE: c_int = 91;
pub const ENOPROTOOPT: c_int = 92;
pub const EPROTONOSUPPORT: c_int = 93;
pub const ESOCKTNOSUPPORT: c_int = 94;
pub const EPFNOSUPPORT: c_int = 96;
pub const EAFNOSUPPORT: c_int = 97;
pub const ENETDOWN: c_int = 100;
pub const ENETUNREACH: c_int = 101;
pub const ENETRESET: c_int = 102;
pub const ENOBUFS: c_int = 105;
pub const EISCONN: c_int = 106;
pub const ESHUTDOWN: c_int = 108;
pub const ETOOMANYREFS: c_int = 109;
pub const EALREADY: c_int = 114;
pub const EINPROGRESS: c_int = 115;
pub const ESTALE: c_int = 116;
pub const EUCLEAN: c_int = 117;
pub const ENOTNAM: c_int = 118;
pub const ENAVAIL: c_int = 119;
pub const EISNAM: c_int = 120;
pub const EREMOTEIO: c_int = 121;
pub const EDQUOT: c_int = 122;
pub const ENOMEDIUM: c_int = 123;
pub const EMEDIUMTYPE: c_int = 124;
pub const ECANCELED: c_int = 125;
pub const ENOKEY: c_int = 126;
pub const EKEYEXPIRED: c_int = 127;
pub const EKEYREVOKED: c_int = 128;
pub const EKEYREJECTED: c_int = 129;
pub const EOWNERDEAD: c_int = 130;
pub const ENOTRECOVERABLE: c_int = 131;
pub const ERFKILL: c_int = 132;
pub const EHWPOISON: c_int = 133;

// BSD's and Mach's errno names, which Hurd inherits and `nix` lists for it;
// MikuOS sits beside Hurd there. The kernel never returns them, and they are
// numbered above anything it produces
pub const EAUTH: c_int = 1001;
pub const EBACKGROUND: c_int = 1002;
pub const EBADRPC: c_int = 1003;
pub const EDIED: c_int = 1004;
pub const EFTYPE: c_int = 1005;
pub const EGRATUITOUS: c_int = 1006;
pub const EGREGIOUS: c_int = 1007;
pub const EIEIO: c_int = 1008;
pub const ENEEDAUTH: c_int = 1009;
pub const EPROCLIM: c_int = 1010;
pub const EPROCUNAVAIL: c_int = 1011;
pub const EPROGMISMATCH: c_int = 1012;
pub const EPROGUNAVAIL: c_int = 1013;
pub const ERPCMISMATCH: c_int = 1014;
pub const EHOSTDOWN: c_int = 112;
pub const EHOSTUNREACH: c_int = 113;

// File modes, `seek`, `access`, and the rest of the POSIX constants

/// The file-type bits of a mode word, Linux's values
///
/// `stat` reports the kind in a byte of its own (`VNodeKind`) and `mode` holds
/// permissions only, so the kernel produces none of these. Unix-shaped crates
/// compose and match on them.
pub const S_IFMT: mode_t = 0o170000;
pub const S_IFIFO: mode_t = 0o010000;
pub const S_IFCHR: mode_t = 0o020000;
pub const S_IFDIR: mode_t = 0o040000;
pub const S_IFBLK: mode_t = 0o060000;
pub const S_IFREG: mode_t = 0o100000;
pub const S_IFLNK: mode_t = 0o120000;
pub const S_IFSOCK: mode_t = 0o140000;

pub const S_IRWXU: mode_t = 0o700;
pub const S_IRUSR: mode_t = 0o400;
pub const S_IWUSR: mode_t = 0o200;
pub const S_IXUSR: mode_t = 0o100;
pub const S_IRWXG: mode_t = 0o070;
pub const S_IRGRP: mode_t = 0o040;
pub const S_IWGRP: mode_t = 0o020;
pub const S_IXGRP: mode_t = 0o010;
pub const S_IRWXO: mode_t = 0o007;
pub const S_IROTH: mode_t = 0o004;
pub const S_IWOTH: mode_t = 0o002;
pub const S_IXOTH: mode_t = 0o001;

/// `lseek` whence, the kernel's own values (`lseek`(31))
pub const SEEK_SET: c_int = 0;
pub const SEEK_CUR: c_int = 1;
pub const SEEK_END: c_int = 2;

/// `access` modes. `F_OK` asks whether the name resolves
pub const F_OK: c_int = 0;
pub const X_OK: c_int = 1;
pub const W_OK: c_int = 2;
pub const R_OK: c_int = 4;

/// The access mode bits of an open flag word
///
/// `O_RDONLY` and `O_WRONLY` are separate bits (1 and 2) and `O_RDWR` is both,
/// unlike Linux's 0/1/2. The mask is 3 in both.
pub const O_ACCMODE: c_int = 0x0003;

/// Open flags `open`(11) does not implement
///
/// Named by crates that build a flag word, and zero so they collide with no
/// real bit. Opening a terminal here does not make it the controlling one
/// (`setsid`/`ioctl(TIOCSCTTY)` does), so `O_NOCTTY` holds anyway.
pub const O_NOCTTY: c_int = 0;
pub const O_ASYNC: c_int = 0;
pub const O_DSYNC: c_int = 0;

/// `fcntl` again: the CLOEXEC duplicate and the record locks, both the kernel's
/// (`fcntl`(113), `fs::vfs::lock`)
pub const F_DUPFD_CLOEXEC: c_int = 1030;
pub const F_GETLK: c_int = 5;
pub const F_SETLK: c_int = 6;
pub const F_SETLKW: c_int = 7;
/// The path of an open descriptor, into a `PATH_MAX` buffer: macOS's and
/// NetBSD's command, with macOS's number. `libc_miku.so` answers it with
/// `realpath` (119) on the descriptor. The kernel keeps no path with a
/// descriptor, so a file renamed since it was opened answers with its new name
pub const F_GETPATH: c_int = 50;
/// The longest path `realpath` (119) and `F_GETPATH` will write, NUL included
pub const PATH_MAX: c_int = 4096;
pub const F_RDLCK: c_short = 0;
pub const F_WRLCK: c_short = 1;
pub const F_UNLCK: c_short = 2;

/// `flock` operations. `flock` in `lib/libmiku/libc.rs` maps them onto a
/// whole-file record lock
pub const LOCK_SH: c_int = 1;
pub const LOCK_EX: c_int = 2;
pub const LOCK_NB: c_int = 4;
pub const LOCK_UN: c_int = 8;

/// The `*at` family's directory sentinel and flags
///
/// There are no `openat`-style syscall numbers: a path's length word carries
/// the base directory in its high half (`docs/MikuOS_ABI.md`, 3.2e), and
/// `lib/libmiku/c/at.rs` is the `*at` family over that. `AT_FDCWD` is a base of
/// nothing, the working directory; an absolute path ignores `dirfd`, as POSIX
/// says.
pub const AT_FDCWD: c_int = -100;
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
pub const AT_REMOVEDIR: c_int = 0x200;
pub const AT_SYMLINK_FOLLOW: c_int = 0x400;
pub const AT_EACCESS: c_int = 0x200;

/// `utimensat`'s two special nanosecond values.
pub const UTIME_NOW: c_long = (1 << 30) - 1;
pub const UTIME_OMIT: c_long = (1 << 30) - 2;

/// `ioctl` requests every libc names. `ioctl`(71) implements the terminal ones;
/// these two are descriptor-level. `libc_miku.so` answers `FIONBIO` as
/// `fcntl(F_SETFL, O_NONBLOCK)` without the kernel. `FIONREAD` returns the
/// kernel's `EINVAL`
pub const FIONBIO: _Opcode = 0x5421;
pub const FIONREAD: _Opcode = 0x541B;

/// `statvfs` flags. MikuOS reports neither (there is no setuid bit and so no
/// `nosuid`), so a caller reads zero
pub const ST_RDONLY: c_ulong = 1;
pub const ST_NOSUID: c_ulong = 2;

/// `posix_fadvise` hints, and the `fallocate` modes
///
/// The hints are advisory, so answering "done" conforms. MikuOS implements none
/// of the `FALLOC_FL_*` modes; `fallocate` with any of them set answers
/// `EOPNOTSUPP`.
pub const POSIX_FADV_NORMAL: c_int = 0;
pub const POSIX_FADV_RANDOM: c_int = 1;
pub const POSIX_FADV_SEQUENTIAL: c_int = 2;
pub const POSIX_FADV_WILLNEED: c_int = 3;
pub const POSIX_FADV_DONTNEED: c_int = 4;
pub const POSIX_FADV_NOREUSE: c_int = 5;

pub const FALLOC_FL_KEEP_SIZE: c_int = 0x01;
pub const FALLOC_FL_PUNCH_HOLE: c_int = 0x02;
pub const FALLOC_FL_NO_HIDE_STALE: c_int = 0x04;
pub const FALLOC_FL_COLLAPSE_RANGE: c_int = 0x08;
pub const FALLOC_FL_ZERO_RANGE: c_int = 0x10;
pub const FALLOC_FL_INSERT_RANGE: c_int = 0x20;
pub const FALLOC_FL_UNSHARE_RANGE: c_int = 0x40;

/// `dlsym`'s "search everything already loaded" handle
///
/// `ld-miku` resolves every `DT_NEEDED` at load and there is no `dlopen`.
/// `dlsym` always answers null, the same as for a symbol it cannot find, which
/// crates probing for an optional libc function handle.
pub const RTLD_DEFAULT: *mut c_void = core::ptr::null_mut();

/// What `ioctl` takes as its request. Linux's `c_ulong`.
#[allow(non_camel_case_types)]
pub type _Opcode = c_ulong;

s! {
    /// `statfs`'s filesystem id. Both words are zero: MikuOS has no device
    /// numbers
    #[allow(non_camel_case_types)]
    pub struct fsid_t {
        pub __val: [c_int; 2],
    }
}

s! {
    /// `struct flock`, for `fcntl`'s record locks: `F_GETLK`, `F_SETLK` and
    /// `F_SETLKW`, POSIX's byte-range advisory locks, owned by the process
    /// and dropped when it closes any descriptor on the file
    /// (`kernel/fs/vfs/lock.rs`). `F_SETLK` against another process's lock
    /// is `EAGAIN`.
    #[allow(non_camel_case_types)]
    pub struct flock {
        pub l_type: c_short,
        pub l_whence: c_short,
        pub l_start: off_t,
        pub l_len: off_t,
        pub l_pid: crate::pid_t,
    }

    /// `statfs`, in Linux's shape
    ///
    /// `statfs`(36) answers a 48-byte structure of its own, translated in
    /// `lib/libmiku/libc.rs`. `f_frsize` is the block size, as on Linux.
    /// `f_bavail` is `f_bfree`, since nothing is reserved for root. `f_fsid`
    /// and `f_spare` are zero.
    #[allow(non_camel_case_types)]
    pub struct statfs {
    pub f_type: i64,
    pub f_bsize: i64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: fsid_t,
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [i64; 4],
    }
}

/// Device numbers, packed as on Linux
///
/// `const fn` here, not symbols in `libc_miku.so`. `stat` carries a major and a
/// minor byte separately and never a packed `dev_t`, so only a caller wanting
/// Unix's shape composes one.
pub const fn major(dev: dev_t) -> c_uint {
    (((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff)) as c_uint
}

pub const fn minor(dev: dev_t) -> c_uint {
    (((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff)) as c_uint
}

pub const fn makedev(major: c_uint, minor: c_uint) -> dev_t {
    let (major, minor) = (major as dev_t, minor as dev_t);
    ((major & 0x0000_0fff) << 8)
        | ((major & 0xffff_f000) << 32)
        | ((minor & 0x0000_00ff) << 0)
        | ((minor & 0xffff_ff00) << 12)
}

// The functions `rustix` reaches for
//
// Each is a real symbol in `libc_miku.so`. A missing definition is a link
// error; `lib/libmiku/libc.rs` implements what MikuOS can do and returns
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

// Signals, and the status word `wait4`(44) answers with

/// The signals MikuOS delivers, with Linux's numbers
///
/// Every number `kernel/process/signal/mod.rs` does not name is fatal by
/// default. `SIGSEGV`, `SIGILL` and `SIGFPE` are raised by the CPU and
/// delivered to a handler with a `ucontext_t` it may rewrite
/// (`dev/signal-architecture.md`); uncaught, the process dies. Nothing raises
/// `SIGBUS` or `SIGTRAP`; crates match on them.
pub const SIGHUP: c_int = 1;
pub const SIGINT: c_int = 2;
pub const SIGQUIT: c_int = 3;
pub const SIGILL: c_int = 4;
pub const SIGTRAP: c_int = 5;
pub const SIGABRT: c_int = 6;
pub const SIGIOT: c_int = SIGABRT;
pub const SIGBUS: c_int = 7;
pub const SIGFPE: c_int = 8;
pub const SIGKILL: c_int = 9;
pub const SIGUSR1: c_int = 10;
pub const SIGSEGV: c_int = 11;
pub const SIGUSR2: c_int = 12;
pub const SIGPIPE: c_int = 13;
pub const SIGALRM: c_int = 14;
pub const SIGTERM: c_int = 15;
pub const SIGSTKFLT: c_int = 16;
pub const SIGCHLD: c_int = 17;
pub const SIGCONT: c_int = 18;
pub const SIGSTOP: c_int = 19;
pub const SIGTSTP: c_int = 20;
pub const SIGTTIN: c_int = 21;
pub const SIGTTOU: c_int = 22;
pub const SIGURG: c_int = 23;
pub const SIGXCPU: c_int = 24;
pub const SIGXFSZ: c_int = 25;
pub const SIGVTALRM: c_int = 26;
pub const SIGPROF: c_int = 27;
pub const SIGWINCH: c_int = 28;
pub const SIGIO: c_int = 29;
pub const SIGPOLL: c_int = SIGIO;
pub const SIGPWR: c_int = 30;
pub const SIGSYS: c_int = 31;
pub const NSIG: c_int = 32;

/// `waitpid` options. `WNOHANG` and `WUNTRACED` are above; these three belong
/// to `waitid`, which MikuOS lacks. `wait4`(44) is the only wait call.
/// `rustix::process` enumerates them
pub const WCONTINUED: c_int = 8;
pub const WEXITED: c_int = 4;
pub const WSTOPPED: c_int = 2;
pub const WNOWAIT: c_int = 0x0100_0000;

/// Taking a status word apart
///
/// POSIX's encoding, written by the kernel in
/// `kernel/syscall/adapters/process.rs`:
///
/// ```text
///   exited     code << 8
///   signalled  sig                (the low seven bits)
///   stopped    0x7f | (sig << 8)
/// ```
///
/// `const fn` here, since in C they are macros over one integer.
pub const fn WIFEXITED(status: c_int) -> bool {
    (status & 0x7f) == 0
}

pub const fn WEXITSTATUS(status: c_int) -> c_int {
    (status >> 8) & 0xff
}

pub const fn WIFSIGNALED(status: c_int) -> bool {
    ((status & 0x7f) + 1) as i8 >= 2
}

pub const fn WTERMSIG(status: c_int) -> c_int {
    status & 0x7f
}

pub const fn WIFSTOPPED(status: c_int) -> bool {
    (status & 0xff) == 0x7f
}

pub const fn WSTOPSIG(status: c_int) -> c_int {
    (status >> 8) & 0xff
}

/// Always false. `SIGCONT` resumes a stopped task, and `wait4`(44) reports
/// nothing when it does
pub const fn WIFCONTINUED(_status: c_int) -> bool {
    false
}

pub const fn WCOREDUMP(status: c_int) -> bool {
    (status & 0x80) != 0
}

/// `poll` again: the `*NORM`/`*BAND` events
///
/// `poll`(76) reports `POLLIN`, `POLLOUT`, `POLLERR`, `POLLHUP` and `POLLNVAL`.
/// No descriptor has out-of-band or priority data. The normal-data pair alias
/// the plain ones, as on Linux; the band pair are zero, since a caller could
/// wait forever on a bit the kernel never sets.
pub const POLLRDNORM: c_short = POLLIN;
pub const POLLWRNORM: c_short = POLLOUT;
pub const POLLRDBAND: c_short = 0;
pub const POLLWRBAND: c_short = 0;

// Clocks, limits, priorities, and the rest of what `rustix::process` names

#[allow(non_camel_case_types)]
pub type clockid_t = c_int;

/// `clock_gettime`(70) knows `REALTIME` and `MONOTONIC` and answers `EINVAL` to
/// anything else. No per-process or per-thread CPU clock is exposed;
/// `rustix::time` enumerates the other two ids
pub const CLOCK_REALTIME: clockid_t = 0;
pub const CLOCK_MONOTONIC: clockid_t = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: clockid_t = 2;
pub const CLOCK_THREAD_CPUTIME_ID: clockid_t = 3;

/// `sysconf` selectors; see `sysconf` below
pub const _SC_PAGESIZE: c_int = 30;
pub const _SC_CLK_TCK: c_int = 2;
pub const _SC_OPEN_MAX: c_int = 4;
pub const _SC_NPROCESSORS_ONLN: c_int = 84;

pub const EXIT_SUCCESS: c_int = 0;
pub const EXIT_FAILURE: c_int = 1;

/// `PIPE_BUF`, a pipe's capacity
///
/// POSIX requires 512. A pipe is a 64 KiB ring, and the kernel keeps a write
/// whole up to `ATOMIC_WRITE`, 4096 (`kernel/fs/vfs/pipe.rs`).
pub const PIPE_BUF: usize = 65536;

/// `O_DIRECT` is zero: every read goes through the page cache
pub const O_DIRECT: c_int = 0;

/// `ioctl(TIOCSCTTY)`: make this terminal the controlling one. A session leader
/// claims a terminal this way; `setsid` alone does not
pub const TIOCSCTTY: _Opcode = 0x540E;

/// Resource limits; MikuOS enforces one
///
/// `RLIMIT_NOFILE` is real: `MAX_FDS` is 512 per process and `open` refuses
/// past it. The others have no enforcement behind them, and `getrlimit` answers
/// `RLIM_INFINITY`.
pub const RLIMIT_CPU: c_int = 0;
pub const RLIMIT_FSIZE: c_int = 1;
pub const RLIMIT_DATA: c_int = 2;
pub const RLIMIT_STACK: c_int = 3;
pub const RLIMIT_CORE: c_int = 4;
pub const RLIMIT_RSS: c_int = 5;
pub const RLIMIT_NPROC: c_int = 6;
pub const RLIMIT_NOFILE: c_int = 7;
pub const RLIMIT_MEMLOCK: c_int = 8;
pub const RLIMIT_AS: c_int = 9;
pub const RLIMIT_LOCKS: c_int = 10;
pub const RLIMIT_SIGPENDING: c_int = 11;
pub const RLIMIT_MSGQUEUE: c_int = 12;
pub const RLIMIT_NICE: c_int = 13;
pub const RLIMIT_RTPRIO: c_int = 14;
pub const RLIMIT_RTTIME: c_int = 15;
pub const RLIM_INFINITY: rlim_t = !0;

/// `getpriority`/`setpriority` targets. A task's priority is set at
/// `spawn_named` and no syscall changes it, so both calls answer `ENOSYS`
pub const PRIO_PROCESS: c_int = 0;
pub const PRIO_PGRP: c_int = 1;
pub const PRIO_USER: c_int = 2;

/// `waitid`'s `idtype`. `waitid` answers `ENOSYS`; `wait4`(44) is the only wait
/// call
pub const P_ALL: c_int = 0;
pub const P_PID: c_int = 1;
pub const P_PGID: c_int = 2;

/// `siginfo_t`'s `si_code` for a child that changed state. Named, not delivered
pub const CLD_EXITED: c_int = 1;
pub const CLD_KILLED: c_int = 2;
pub const CLD_DUMPED: c_int = 3;
pub const CLD_TRAPPED: c_int = 4;
pub const CLD_STOPPED: c_int = 5;
pub const CLD_CONTINUED: c_int = 6;

s! {
    /// `siginfo_t`, in the usual shape
    ///
    /// For a signal the CPU raised (`SIGSEGV`, `SIGILL`, `SIGFPE`) the kernel
    /// fills `si_code` (`SEGV_MAPERR`, `ILL_ILLOPN`, ...) and the address at
    /// +16, read by `si_addr()`. For a sent signal `si_signo` is set and the
    /// rest is zero: it reaches ring 3 as a number, with no sender. `waitid`
    /// answers `ENOSYS` without touching the structure, so `rustix`'s
    /// `si_signo == 0` check reads the zero it supplied.
    #[allow(non_camel_case_types)]
    pub struct siginfo_t {
        pub si_signo: c_int,
        pub si_errno: c_int,
        pub si_code: c_int,
        pub _pad: [c_int; 29],
        _align: [u64; 0],
    }
}

impl siginfo_t {
    /// The child's exit status, as `waitid` would fill it. Always zero; see
    /// `siginfo_t`
    pub unsafe fn si_status(&self) -> c_int {
        self._pad[0]
    }

    pub unsafe fn si_pid(&self) -> crate::unix::pid_t {
        self._pad[1] as crate::unix::pid_t
    }

    /// The faulting address, for a signal the CPU raised: CR2 for `SIGSEGV`
    /// from a page fault, the instruction for `SIGILL` and `SIGFPE`, zero for a
    /// `SIGSEGV` from a general protection fault. At +16, as on Linux
    pub unsafe fn si_addr(&self) -> *mut c_void {
        (self as *const Self as *const u8).add(16).cast::<*mut c_void>().read()
    }
}

s! {
    /// A thread's alternate signal stack, for `sigaltstack`. Linux's shape.
    pub struct stack_t {
        pub ss_sp: *mut c_void,
        pub ss_flags: c_int,
        pub ss_size: size_t,
    }

    /// The machine context a signal handler gets, Linux's layout: the general
    /// registers indexed by `REG_*`, then a pointer to the FPU image.
    /// `sigreturn` restores `gregs` as written, RIP and RSP included
    /// (`dev/signal-architecture.md`)
    pub struct mcontext_t {
        pub gregs: gregset_t,
        pub fpregs: *mut c_void,
        __reserved1: [c_ulonglong; 8],
    }

    /// The third argument of a `SA_SIGINFO` handler, byte for byte Linux's up
    /// to the end of `uc_sigmask`. `libmiku`'s entry stub saves and restores
    /// the FPU image `uc_mcontext.fpregs` points at
    pub struct ucontext_t {
        pub uc_flags: c_ulong,
        pub uc_link: *mut ucontext_t,
        pub uc_stack: stack_t,
        pub uc_mcontext: mcontext_t,
        pub uc_sigmask: sigset_t,
    }
}

pub type greg_t = i64;
pub type gregset_t = [greg_t; 23];

pub const REG_R8: c_int = 0;
pub const REG_R9: c_int = 1;
pub const REG_R10: c_int = 2;
pub const REG_R11: c_int = 3;
pub const REG_R12: c_int = 4;
pub const REG_R13: c_int = 5;
pub const REG_R14: c_int = 6;
pub const REG_R15: c_int = 7;
pub const REG_RDI: c_int = 8;
pub const REG_RSI: c_int = 9;
pub const REG_RBP: c_int = 10;
pub const REG_RBX: c_int = 11;
pub const REG_RDX: c_int = 12;
pub const REG_RAX: c_int = 13;
pub const REG_RCX: c_int = 14;
pub const REG_RSP: c_int = 15;
pub const REG_RIP: c_int = 16;
pub const REG_EFL: c_int = 17;
pub const REG_CSGSFS: c_int = 18;
pub const REG_ERR: c_int = 19;
pub const REG_TRAPNO: c_int = 20;
pub const REG_OLDMASK: c_int = 21;
pub const REG_CR2: c_int = 22;

/// `ss_flags`
pub const SS_ONSTACK: c_int = 1;
pub const SS_DISABLE: c_int = 2;
/// The kernel refuses an alternate stack smaller than this
pub const MINSIGSTKSZ: size_t = 2048;
pub const SIGSTKSZ: size_t = 8192;

/// `si_code` for the signals the CPU raises
pub const ILL_ILLOPN: c_int = 2;
pub const FPE_INTDIV: c_int = 1;
pub const SEGV_MAPERR: c_int = 1;
pub const SEGV_ACCERR: c_int = 2;
/// `si_code` of a `SIGSEGV` from a general protection fault, which has no
/// address to report
pub const SI_KERNEL: c_int = 0x80;
/// `si_code` of a signal sent by `kill`
pub const SI_USER: c_int = 0;

s! {
    /// One record a `read` on an inotify descriptor returns, followed by `len`
    /// bytes of NUL-padded name. Linux's shape.
    pub struct inotify_event {
        pub wd: c_int,
        pub mask: u32,
        pub cookie: u32,
        pub len: u32,
    }
}

// inotify, Linux's values and semantics (`inotify_init1`(116),
// `inotify_add_watch`(117), `inotify_rm_watch`(118)). `IN_ACCESS` and `IN_OPEN`
// are accepted in a mask and never raised
pub const IN_ACCESS: u32 = 0x0000_0001;
pub const IN_MODIFY: u32 = 0x0000_0002;
pub const IN_ATTRIB: u32 = 0x0000_0004;
pub const IN_CLOSE_WRITE: u32 = 0x0000_0008;
pub const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
pub const IN_CLOSE: u32 = IN_CLOSE_WRITE | IN_CLOSE_NOWRITE;
pub const IN_OPEN: u32 = 0x0000_0020;
pub const IN_MOVED_FROM: u32 = 0x0000_0040;
pub const IN_MOVED_TO: u32 = 0x0000_0080;
pub const IN_MOVE: u32 = IN_MOVED_FROM | IN_MOVED_TO;
pub const IN_CREATE: u32 = 0x0000_0100;
pub const IN_DELETE: u32 = 0x0000_0200;
pub const IN_DELETE_SELF: u32 = 0x0000_0400;
pub const IN_MOVE_SELF: u32 = 0x0000_0800;
pub const IN_UNMOUNT: u32 = 0x0000_2000;
pub const IN_Q_OVERFLOW: u32 = 0x0000_4000;
pub const IN_IGNORED: u32 = 0x0000_8000;
pub const IN_ONLYDIR: u32 = 0x0100_0000;
pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
pub const IN_MASK_CREATE: u32 = 0x1000_0000;
pub const IN_MASK_ADD: u32 = 0x2000_0000;
pub const IN_ISDIR: u32 = 0x4000_0000;
pub const IN_ONESHOT: u32 = 0x8000_0000;
pub const IN_ALL_EVENTS: u32 = 0x0000_0fff;
pub const IN_CLOEXEC: c_int = O_CLOEXEC;
pub const IN_NONBLOCK: c_int = O_NONBLOCK;

// `getaddrinfo`, Linux's values. `libc_miku.so` resolves IPv4 only:
// a literal, `localhost`, `/etc/hosts`, then DNS to the lease's resolver
// (`lib/libmiku/c/netdb.rs`)
pub const EAI_BADFLAGS: c_int = -1;
pub const EAI_NONAME: c_int = -2;
pub const EAI_AGAIN: c_int = -3;
pub const EAI_FAIL: c_int = -4;
pub const EAI_FAMILY: c_int = -6;
pub const EAI_SOCKTYPE: c_int = -7;
pub const EAI_SERVICE: c_int = -8;
pub const EAI_MEMORY: c_int = -10;
pub const EAI_SYSTEM: c_int = -11;
pub const AI_PASSIVE: c_int = 0x0001;
pub const AI_CANONNAME: c_int = 0x0002;
pub const AI_NUMERICHOST: c_int = 0x0004;
pub const AI_V4MAPPED: c_int = 0x0008;
pub const AI_ALL: c_int = 0x0010;
pub const AI_ADDRCONFIG: c_int = 0x0020;
pub const AI_NUMERICSERV: c_int = 0x0400;

pub const GRND_NONBLOCK: c_uint = 0x0001;
pub const GRND_RANDOM: c_uint = 0x0002;
pub const GRND_INSECURE: c_uint = 0x0004;

// termios, Linux's values. `libc_miku.so` translates Linux's `struct termios`
// onto the kernel's (`lib/libmiku/c/termios.rs`): `ICANON`, `ECHO`, `ISIG`,
// `TOSTOP`, the control characters, `VMIN` and `VTIME` act. Speeds, parity and
// output post-processing are accepted by `tcsetattr` and not reported back by
// `tcgetattr`
pub const NCCS: usize = 32;
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSWTC: usize = 7;
pub const VSTART: usize = 8;
pub const VSTOP: usize = 9;
pub const VSUSP: usize = 10;
pub const VEOL: usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE: usize = 14;
pub const VLNEXT: usize = 15;
pub const VEOL2: usize = 16;
pub const IGNBRK: tcflag_t = 0o1;
pub const BRKINT: tcflag_t = 0o2;
pub const IGNPAR: tcflag_t = 0o4;
pub const PARMRK: tcflag_t = 0o10;
pub const INPCK: tcflag_t = 0o20;
pub const ISTRIP: tcflag_t = 0o40;
pub const INLCR: tcflag_t = 0o100;
pub const IGNCR: tcflag_t = 0o200;
pub const ICRNL: tcflag_t = 0o400;
pub const IUCLC: tcflag_t = 0o1000;
pub const IXON: tcflag_t = 0o2000;
pub const IXANY: tcflag_t = 0o4000;
pub const IXOFF: tcflag_t = 0o10000;
pub const IMAXBEL: tcflag_t = 0o20000;
pub const IUTF8: tcflag_t = 0o40000;
pub const OPOST: tcflag_t = 0o1;
pub const OLCUC: tcflag_t = 0o2;
pub const ONLCR: tcflag_t = 0o4;
pub const OCRNL: tcflag_t = 0o10;
pub const ONOCR: tcflag_t = 0o20;
pub const ONLRET: tcflag_t = 0o40;
pub const OFILL: tcflag_t = 0o100;
pub const OFDEL: tcflag_t = 0o200;
pub const NLDLY: tcflag_t = 0o400;
pub const NL0: tcflag_t = 0;
pub const NL1: tcflag_t = 0o400;
pub const CRDLY: tcflag_t = 0o3000;
pub const CR0: tcflag_t = 0;
pub const CR1: tcflag_t = 0o1000;
pub const CR2: tcflag_t = 0o2000;
pub const CR3: tcflag_t = 0o3000;
pub const TABDLY: tcflag_t = 0o14000;
pub const TAB0: tcflag_t = 0;
pub const TAB1: tcflag_t = 0o4000;
pub const TAB2: tcflag_t = 0o10000;
pub const TAB3: tcflag_t = 0o14000;
pub const XTABS: tcflag_t = 0o14000;
pub const BSDLY: tcflag_t = 0o20000;
pub const BS0: tcflag_t = 0;
pub const BS1: tcflag_t = 0o20000;
pub const VTDLY: tcflag_t = 0o40000;
pub const VT0: tcflag_t = 0;
pub const VT1: tcflag_t = 0o40000;
pub const FFDLY: tcflag_t = 0o100000;
pub const FF0: tcflag_t = 0;
pub const FF1: tcflag_t = 0o100000;
pub const CBAUD: tcflag_t = 0o10017;
pub const CSIZE: tcflag_t = 0o60;
pub const CS5: tcflag_t = 0;
pub const CS6: tcflag_t = 0o20;
pub const CS7: tcflag_t = 0o40;
pub const CS8: tcflag_t = 0o60;
pub const CSTOPB: tcflag_t = 0o100;
pub const CREAD: tcflag_t = 0o200;
pub const PARENB: tcflag_t = 0o400;
pub const PARODD: tcflag_t = 0o1000;
pub const HUPCL: tcflag_t = 0o2000;
pub const CLOCAL: tcflag_t = 0o4000;
pub const CBAUDEX: tcflag_t = 0o10000;
pub const CMSPAR: tcflag_t = 0o10000000000;
pub const CRTSCTS: tcflag_t = 0o20000000000;
pub const ISIG: tcflag_t = 0o1;
pub const ICANON: tcflag_t = 0o2;
pub const XCASE: tcflag_t = 0o4;
pub const ECHO: tcflag_t = 0o10;
pub const ECHOE: tcflag_t = 0o20;
pub const ECHOK: tcflag_t = 0o40;
pub const ECHONL: tcflag_t = 0o100;
pub const NOFLSH: tcflag_t = 0o200;
pub const TOSTOP: tcflag_t = 0o400;
pub const ECHOCTL: tcflag_t = 0o1000;
pub const ECHOPRT: tcflag_t = 0o2000;
pub const ECHOKE: tcflag_t = 0o4000;
pub const FLUSHO: tcflag_t = 0o10000;
pub const PENDIN: tcflag_t = 0o40000;
pub const IEXTEN: tcflag_t = 0o100000;
pub const EXTPROC: tcflag_t = 0o200000;
pub const B0: speed_t = 0;
pub const B50: speed_t = 1;
pub const B75: speed_t = 2;
pub const B110: speed_t = 3;
pub const B134: speed_t = 4;
pub const B150: speed_t = 5;
pub const B200: speed_t = 6;
pub const B300: speed_t = 7;
pub const B600: speed_t = 0o10;
pub const B1200: speed_t = 0o11;
pub const B1800: speed_t = 0o12;
pub const B2400: speed_t = 0o13;
pub const B4800: speed_t = 0o14;
pub const B9600: speed_t = 0o15;
pub const B19200: speed_t = 0o16;
pub const B38400: speed_t = 0o17;
pub const B57600: speed_t = 0o10001;
pub const B115200: speed_t = 0o10002;
pub const B230400: speed_t = 0o10003;
pub const B460800: speed_t = 0o10004;
pub const B500000: speed_t = 0o10005;
pub const B576000: speed_t = 0o10006;
pub const B921600: speed_t = 0o10007;
pub const B1000000: speed_t = 0o10010;
pub const B1152000: speed_t = 0o10011;
pub const B1500000: speed_t = 0o10012;
pub const B2000000: speed_t = 0o10013;
pub const B2500000: speed_t = 0o10014;
pub const B3000000: speed_t = 0o10015;
pub const B3500000: speed_t = 0o10016;
pub const B4000000: speed_t = 0o10017;
pub const TCSANOW: c_int = 0;
pub const TCSADRAIN: c_int = 1;
pub const TCSAFLUSH: c_int = 2;
pub const TCIFLUSH: c_int = 0;
pub const TCOFLUSH: c_int = 1;
pub const TCIOFLUSH: c_int = 2;
pub const TCOOFF: c_int = 0;
pub const TCOON: c_int = 1;
pub const TCIOFF: c_int = 2;
pub const TCION: c_int = 3;
/// The terminal ioctls, Linux's numbers. `TCGETS`/`TCSETS*` go through
/// `libc_miku.so`'s translation, the window size and `TIOCGPTN` to the kernel
/// (`kernel/syscall/adapters/io.rs`). A pty has no modem or break lines, and
/// the kernel answers those `ENOTTY`
pub const TCGETS: _Opcode = 0x5401;
pub const TCSETS: _Opcode = 0x5402;
pub const TCSETSW: _Opcode = 0x5403;
pub const TCSETSF: _Opcode = 0x5404;
pub const TIOCEXCL: _Opcode = 0x540C;
pub const TIOCNXCL: _Opcode = 0x540D;
pub const TIOCGWINSZ: _Opcode = 0x5413;
pub const TIOCSWINSZ: _Opcode = 0x5414;
pub const TIOCMGET: _Opcode = 0x5415;
pub const TIOCMBIS: _Opcode = 0x5416;
pub const TIOCMBIC: _Opcode = 0x5417;
pub const TIOCMSET: _Opcode = 0x5418;
pub const TIOCSBRK: _Opcode = 0x5427;
pub const TIOCCBRK: _Opcode = 0x5428;
pub const TIOCGPTN: _Opcode = 0x8004_5430;
pub const TIOCM_LE: c_int = 0x001;
pub const TIOCM_DTR: c_int = 0x002;
pub const TIOCM_RTS: c_int = 0x004;
pub const TIOCM_ST: c_int = 0x008;
pub const TIOCM_SR: c_int = 0x010;
pub const TIOCM_CTS: c_int = 0x020;
pub const TIOCM_CAR: c_int = 0x040;
pub const TIOCM_CD: c_int = TIOCM_CAR;
pub const TIOCM_RNG: c_int = 0x080;
pub const TIOCM_RI: c_int = TIOCM_RNG;
pub const TIOCM_DSR: c_int = 0x100;

// Memory advice and sync, Linux's values. `madvise` takes every advice except
// `MADV_DONTNEED`, whose Linux meaning (the pages read back as zeros) is
// `ENOSYS` here. `posix_madvise` is advice in every case. `msync` writes back a
// shared file mapping whatever the flags
pub const MADV_NORMAL: c_int = 0;
pub const MADV_RANDOM: c_int = 1;
pub const MADV_SEQUENTIAL: c_int = 2;
pub const MADV_WILLNEED: c_int = 3;
pub const MADV_DONTNEED: c_int = 4;
pub const POSIX_MADV_NORMAL: c_int = 0;
pub const POSIX_MADV_RANDOM: c_int = 1;
pub const POSIX_MADV_SEQUENTIAL: c_int = 2;
pub const POSIX_MADV_WILLNEED: c_int = 3;
pub const POSIX_MADV_DONTNEED: c_int = 4;
pub const MS_ASYNC: c_int = 1;
pub const MS_INVALIDATE: c_int = 2;
pub const MS_SYNC: c_int = 4;
// `mmap` flags named for code that ORs them in. The kernel's `mmap`(3) takes
// `MAP_ANONYMOUS | MAP_PRIVATE`-shaped requests and ignores the rest
pub const MAP_GROWSDOWN: c_int = 0x0100;
pub const MAP_DENYWRITE: c_int = 0x0800;
pub const MAP_LOCKED: c_int = 0x2000;
pub const MAP_NORESERVE: c_int = 0x4000;
pub const MAP_POPULATE: c_int = 0x8000;
pub const MAP_STACK: c_int = 0x20000;
pub const MAP_HUGETLB: c_int = 0x40000;
pub const MAP_SYNC: c_int = 0x80000;
pub const MAP_FIXED_NOREPLACE: c_int = 0x100000;
pub const MAP_SHARED_VALIDATE: c_int = 0x3;
pub const MAP_HUGE_2MB: c_int = 21 << 26;
pub const MAP_HUGE_1GB: c_int = 30 << 26;

/// The static initialisers: every pthread object in `libc_miku.so` is valid
/// all-zero (lib/libmiku/c/pthread.rs)
pub const PTHREAD_MUTEX_INITIALIZER: pthread_mutex_t = [0; 5];
pub const PTHREAD_COND_INITIALIZER: pthread_cond_t = [0; 6];
pub const PTHREAD_RWLOCK_INITIALIZER: pthread_rwlock_t = [0; 7];

/// `sigprocmask`'s `how`. The mask is process-wide, so `pthread_sigmask` is the
/// same call
pub const SIG_BLOCK: c_int = 0;
pub const SIG_UNBLOCK: c_int = 1;
pub const SIG_SETMASK: c_int = 2;

/// Linux spells the page size both ways
pub const _SC_PAGE_SIZE: c_int = _SC_PAGESIZE;
/// Answered with 1024: a line of `/etc/passwd` is a few dozen bytes
pub const _SC_GETPW_R_SIZE_MAX: c_int = 70;

/// The rest of `sysconf`'s and `pathconf`'s names, in Linux's numbering.
/// `libc_miku.so` answers those that describe this system (`sysconf` in
/// `lib/libmiku/libc.rs`, `pathconf` in `lib/libmiku/c/sysinfo.rs`) and
/// `EINVAL` for the rest
pub const _PC_LINK_MAX: c_int = 0;
pub const _PC_MAX_CANON: c_int = 1;
pub const _PC_MAX_INPUT: c_int = 2;
pub const _PC_NAME_MAX: c_int = 3;
pub const _PC_PATH_MAX: c_int = 4;
pub const _PC_PIPE_BUF: c_int = 5;
pub const _PC_CHOWN_RESTRICTED: c_int = 6;
pub const _PC_NO_TRUNC: c_int = 7;
pub const _PC_VDISABLE: c_int = 8;
pub const _PC_SYNC_IO: c_int = 9;
pub const _PC_ASYNC_IO: c_int = 10;
pub const _PC_PRIO_IO: c_int = 11;
pub const _PC_SOCK_MAXBUF: c_int = 12;
pub const _PC_FILESIZEBITS: c_int = 13;
pub const _PC_REC_INCR_XFER_SIZE: c_int = 14;
pub const _PC_REC_MAX_XFER_SIZE: c_int = 15;
pub const _PC_REC_MIN_XFER_SIZE: c_int = 16;
pub const _PC_REC_XFER_ALIGN: c_int = 17;
pub const _PC_ALLOC_SIZE_MIN: c_int = 18;
pub const _PC_SYMLINK_MAX: c_int = 19;
pub const _PC_2_SYMLINKS: c_int = 20;
pub const _SC_ARG_MAX: c_int = 0;
pub const _SC_CHILD_MAX: c_int = 1;
pub const _SC_NGROUPS_MAX: c_int = 3;
pub const _SC_STREAM_MAX: c_int = 5;
pub const _SC_TZNAME_MAX: c_int = 6;
pub const _SC_JOB_CONTROL: c_int = 7;
pub const _SC_SAVED_IDS: c_int = 8;
pub const _SC_REALTIME_SIGNALS: c_int = 9;
pub const _SC_PRIORITY_SCHEDULING: c_int = 10;
pub const _SC_TIMERS: c_int = 11;
pub const _SC_ASYNCHRONOUS_IO: c_int = 12;
pub const _SC_PRIORITIZED_IO: c_int = 13;
pub const _SC_SYNCHRONIZED_IO: c_int = 14;
pub const _SC_FSYNC: c_int = 15;
pub const _SC_MAPPED_FILES: c_int = 16;
pub const _SC_MEMLOCK: c_int = 17;
pub const _SC_MEMLOCK_RANGE: c_int = 18;
pub const _SC_MEMORY_PROTECTION: c_int = 19;
pub const _SC_MESSAGE_PASSING: c_int = 20;
pub const _SC_SEMAPHORES: c_int = 21;
pub const _SC_SHARED_MEMORY_OBJECTS: c_int = 22;
pub const _SC_AIO_LISTIO_MAX: c_int = 23;
pub const _SC_AIO_MAX: c_int = 24;
pub const _SC_AIO_PRIO_DELTA_MAX: c_int = 25;
pub const _SC_DELAYTIMER_MAX: c_int = 26;
pub const _SC_MQ_OPEN_MAX: c_int = 27;
pub const _SC_MQ_PRIO_MAX: c_int = 28;
pub const _SC_VERSION: c_int = 29;
pub const _SC_RTSIG_MAX: c_int = 31;
pub const _SC_SEM_NSEMS_MAX: c_int = 32;
pub const _SC_SEM_VALUE_MAX: c_int = 33;
pub const _SC_SIGQUEUE_MAX: c_int = 34;
pub const _SC_TIMER_MAX: c_int = 35;
pub const _SC_BC_BASE_MAX: c_int = 36;
pub const _SC_BC_DIM_MAX: c_int = 37;
pub const _SC_BC_SCALE_MAX: c_int = 38;
pub const _SC_BC_STRING_MAX: c_int = 39;
pub const _SC_COLL_WEIGHTS_MAX: c_int = 40;
pub const _SC_EXPR_NEST_MAX: c_int = 42;
pub const _SC_LINE_MAX: c_int = 43;
pub const _SC_RE_DUP_MAX: c_int = 44;
pub const _SC_2_VERSION: c_int = 46;
pub const _SC_2_C_BIND: c_int = 47;
pub const _SC_2_C_DEV: c_int = 48;
pub const _SC_2_FORT_DEV: c_int = 49;
pub const _SC_2_FORT_RUN: c_int = 50;
pub const _SC_2_SW_DEV: c_int = 51;
pub const _SC_2_LOCALEDEF: c_int = 52;
pub const _SC_UIO_MAXIOV: c_int = 60;
pub const _SC_IOV_MAX: c_int = 60;
pub const _SC_THREADS: c_int = 67;
pub const _SC_THREAD_SAFE_FUNCTIONS: c_int = 68;
pub const _SC_GETGR_R_SIZE_MAX: c_int = 69;
pub const _SC_LOGIN_NAME_MAX: c_int = 71;
pub const _SC_TTY_NAME_MAX: c_int = 72;
pub const _SC_THREAD_DESTRUCTOR_ITERATIONS: c_int = 73;
pub const _SC_THREAD_KEYS_MAX: c_int = 74;
pub const _SC_THREAD_STACK_MIN: c_int = 75;
pub const _SC_THREAD_THREADS_MAX: c_int = 76;
pub const _SC_THREAD_ATTR_STACKADDR: c_int = 77;
pub const _SC_THREAD_ATTR_STACKSIZE: c_int = 78;
pub const _SC_THREAD_PRIORITY_SCHEDULING: c_int = 79;
pub const _SC_THREAD_PRIO_INHERIT: c_int = 80;
pub const _SC_THREAD_PRIO_PROTECT: c_int = 81;
pub const _SC_THREAD_PROCESS_SHARED: c_int = 82;
pub const _SC_NPROCESSORS_CONF: c_int = 83;
pub const _SC_PHYS_PAGES: c_int = 85;
pub const _SC_AVPHYS_PAGES: c_int = 86;
pub const _SC_ATEXIT_MAX: c_int = 87;
pub const _SC_PASS_MAX: c_int = 88;
pub const _SC_XOPEN_VERSION: c_int = 89;
pub const _SC_XOPEN_XCU_VERSION: c_int = 90;
pub const _SC_XOPEN_UNIX: c_int = 91;
pub const _SC_XOPEN_CRYPT: c_int = 92;
pub const _SC_XOPEN_ENH_I18N: c_int = 93;
pub const _SC_XOPEN_SHM: c_int = 94;
pub const _SC_2_CHAR_TERM: c_int = 95;
pub const _SC_2_UPE: c_int = 97;
pub const _SC_XOPEN_XPG2: c_int = 98;
pub const _SC_XOPEN_XPG3: c_int = 99;
pub const _SC_XOPEN_XPG4: c_int = 100;
pub const _SC_NZERO: c_int = 109;
pub const _SC_XBS5_ILP32_OFF32: c_int = 125;
pub const _SC_XBS5_ILP32_OFFBIG: c_int = 126;
pub const _SC_XBS5_LP64_OFF64: c_int = 127;
pub const _SC_XBS5_LPBIG_OFFBIG: c_int = 128;
pub const _SC_XOPEN_LEGACY: c_int = 129;
pub const _SC_XOPEN_REALTIME: c_int = 130;
pub const _SC_XOPEN_REALTIME_THREADS: c_int = 131;
pub const _SC_ADVISORY_INFO: c_int = 132;
pub const _SC_BARRIERS: c_int = 133;
pub const _SC_CLOCK_SELECTION: c_int = 137;
pub const _SC_CPUTIME: c_int = 138;
pub const _SC_THREAD_CPUTIME: c_int = 139;
pub const _SC_MONOTONIC_CLOCK: c_int = 149;
pub const _SC_READER_WRITER_LOCKS: c_int = 153;
pub const _SC_SPIN_LOCKS: c_int = 154;
pub const _SC_REGEXP: c_int = 155;
pub const _SC_SHELL: c_int = 157;
pub const _SC_SPAWN: c_int = 159;
pub const _SC_SPORADIC_SERVER: c_int = 160;
pub const _SC_THREAD_SPORADIC_SERVER: c_int = 161;
pub const _SC_TIMEOUTS: c_int = 164;
pub const _SC_TYPED_MEMORY_OBJECTS: c_int = 165;
pub const _SC_2_PBS: c_int = 168;
pub const _SC_2_PBS_ACCOUNTING: c_int = 169;
pub const _SC_2_PBS_LOCATE: c_int = 170;
pub const _SC_2_PBS_MESSAGE: c_int = 171;
pub const _SC_2_PBS_TRACK: c_int = 172;
pub const _SC_SYMLOOP_MAX: c_int = 173;
pub const _SC_STREAMS: c_int = 174;
pub const _SC_2_PBS_CHECKPOINT: c_int = 175;
pub const _SC_V6_ILP32_OFF32: c_int = 176;
pub const _SC_V6_ILP32_OFFBIG: c_int = 177;
pub const _SC_V6_LP64_OFF64: c_int = 178;
pub const _SC_V6_LPBIG_OFFBIG: c_int = 179;
pub const _SC_HOST_NAME_MAX: c_int = 180;
pub const _SC_TRACE: c_int = 181;
pub const _SC_TRACE_EVENT_FILTER: c_int = 182;
pub const _SC_TRACE_INHERIT: c_int = 183;
pub const _SC_TRACE_LOG: c_int = 184;
pub const _SC_IPV6: c_int = 235;
pub const _SC_RAW_SOCKETS: c_int = 236;
pub const _SC_V7_ILP32_OFF32: c_int = 237;
pub const _SC_V7_ILP32_OFFBIG: c_int = 238;
pub const _SC_V7_LP64_OFF64: c_int = 239;
pub const _SC_V7_LPBIG_OFFBIG: c_int = 240;
pub const _SC_SS_REPL_MAX: c_int = 241;
pub const _SC_TRACE_EVENT_NAME_MAX: c_int = 242;
pub const _SC_TRACE_NAME_MAX: c_int = 243;
pub const _SC_TRACE_SYS_MAX: c_int = 244;
pub const _SC_TRACE_USER_EVENT_MAX: c_int = 245;
pub const _SC_XOPEN_STREAMS: c_int = 246;
pub const _SC_THREAD_ROBUST_PRIO_INHERIT: c_int = 247;
pub const _SC_THREAD_ROBUST_PRIO_PROTECT: c_int = 248;

extern "C" {
    pub fn madvise(addr: *mut c_void, len: size_t, advice: c_int) -> c_int;
    pub fn posix_madvise(addr: *mut c_void, len: size_t, advice: c_int) -> c_int;
    /// `ENOSYS`: a symlink's own times cannot be set
    pub fn lutimes(file: *const c_char, times: *const crate::timeval) -> c_int;
    pub fn pthread_sigmask(how: c_int, set: *const sigset_t, oldset: *mut sigset_t) -> c_int;
    pub fn pthread_mutexattr_setpshared(attr: *mut pthread_mutexattr_t, pshared: c_int) -> c_int;
    pub fn pthread_mutexattr_getpshared(attr: *const pthread_mutexattr_t, pshared: *mut c_int) -> c_int;
    /// `execvp` with an environment. `fexecve` is `ENOSYS`: the kernel
    /// executes paths, not descriptors (lib/libmiku/libc.rs)
    pub fn execvpe(file: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    pub fn fexecve(fd: c_int, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    pub fn sigsuspend(mask: *const sigset_t) -> c_int;
    /// Returns `ENOSYS`: nothing takes a signal off the queue unhandled
    pub fn sigwait(set: *const sigset_t, sig: *mut c_int) -> c_int;
    pub fn pthread_condattr_setclock(attr: *mut pthread_condattr_t, clock_id: clockid_t) -> c_int;
    pub fn pthread_condattr_getclock(attr: *const pthread_condattr_t, clock_id: *mut clockid_t) -> c_int;
    /// A terminal and both its ends: the master from `/dev/ptmx`, the slave
    /// `/dev/ptsN` (lib/libmiku/c/pty.rs). `winp` sets the terminal's size
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

/// `grp.h`, from `/etc/group` (`lib/libmiku/c/accounts.rs`). The kernel's
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

s! {
    /// Linux's, six 65-byte fields (`lib/libmiku/c/sysinfo.rs`)
    pub struct utsname {
        pub sysname: [c_char; 65],
        pub nodename: [c_char; 65],
        pub release: [c_char; 65],
        pub version: [c_char; 65],
        pub machine: [c_char; 65],
        pub domainname: [c_char; 65],
    }
}

pub const RUSAGE_SELF: c_int = 0;
pub const RUSAGE_CHILDREN: c_int = -1;
pub const RUSAGE_THREAD: c_int = 1;

/// `lseek`'s hole-finding whences. The kernel answers `EINVAL` for both, as
/// Linux does on a filesystem without the operation
pub const SEEK_DATA: c_int = 3;
pub const SEEK_HOLE: c_int = 4;

/// `O_NONBLOCK`'s old name
pub const O_NDELAY: c_int = O_NONBLOCK;
/// The descriptor itself, not a path from it: `fstatat` and `fchownat` take it.
/// It is a length of zero with a base in the path word
pub const AT_EMPTY_PATH: c_int = 0x1000;

extern "C" {
    /// The release comes from `/proc/version`, the node name from
    /// `/etc/hostname`
    pub fn uname(buf: *mut utsname) -> c_int;
    /// `fork`'s handlers (`lib/libmiku/c/pthread.rs`)
    pub fn pthread_atfork(
        prepare: Option<unsafe extern "C" fn()>,
        parent: Option<unsafe extern "C" fn()>,
        child: Option<unsafe extern "C" fn()>,
    ) -> c_int;
    /// Every thread is `SCHED_OTHER` at priority 0; a realtime policy is
    /// `ENOTSUP` (`lib/libmiku/c/pthread.rs`)
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

s! {
    pub struct sched_param {
        pub sched_priority: c_int,
    }
}

/// Linux's policy numbers. Only `SCHED_OTHER` is one a thread can have
pub const SCHED_OTHER: c_int = 0;
pub const SCHED_FIFO: c_int = 1;
pub const SCHED_RR: c_int = 2;

/// `kcmp`'s one comparison MikuOS answers
pub const KCMP_FILE: c_int = 0;

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
    /// Answers the selectors in `lib/libmiku/libc.rs`; anything else is `-1`
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

// Sockets, as `rustix::net` enumerates them
//
// Linux's numbers; `rustix` names every family and option in one `match`. The
// kernel's `net/socket.rs` implements `AF_INET` stream and datagram, raw ICMP,
// and `AF_UNIX` stream; one option to set (`IP_TTL`) and four to read
// (`SO_ERROR`, `SO_TYPE`, `SO_ACCEPTCONN`, `IP_TTL`, through
// `getsockopt`(114)). The rest answer `EAFNOSUPPORT`, `EPROTONOSUPPORT` or
// `ENOPROTOOPT`.

pub const AF_UNSPEC: c_int = 0;
pub const AF_AX25: c_int = 3;
pub const AF_IPX: c_int = 4;
pub const AF_APPLETALK: c_int = 5;
pub const AF_NETROM: c_int = 6;
pub const AF_BRIDGE: c_int = 7;
pub const AF_ATMPVC: c_int = 8;
pub const AF_X25: c_int = 9;
pub const AF_INET6: c_int = 10;
pub const AF_ROSE: c_int = 11;
pub const AF_DECnet: c_int = 12;
pub const AF_NETBEUI: c_int = 13;
pub const AF_SECURITY: c_int = 14;
pub const AF_KEY: c_int = 15;
pub const AF_NETLINK: c_int = 16;
pub const AF_PACKET: c_int = 17;
pub const AF_ASH: c_int = 18;
pub const AF_ECONET: c_int = 19;
pub const AF_ATMSVC: c_int = 20;
pub const AF_RDS: c_int = 21;
pub const AF_SNA: c_int = 22;
pub const AF_IRDA: c_int = 23;
pub const AF_PPPOX: c_int = 24;
pub const AF_WANPIPE: c_int = 25;
pub const AF_LLC: c_int = 26;
pub const AF_CAN: c_int = 29;
pub const AF_TIPC: c_int = 30;
pub const AF_BLUETOOTH: c_int = 31;
pub const AF_IUCV: c_int = 32;
pub const AF_RXRPC: c_int = 33;
pub const AF_ISDN: c_int = 34;
pub const AF_PHONET: c_int = 35;
pub const AF_IEEE802154: c_int = 36;

pub const SOCK_RAW: c_int = 3;
pub const SOCK_RDM: c_int = 4;
pub const SOCK_SEQPACKET: c_int = 5;
/// The two flags `socket` and `accept4` take in the type word: `O_NONBLOCK`'s
/// and `O_CLOEXEC`'s values, as on Linux. `libc_miku.so` strips them and
/// applies them with `fcntl`, since `socket`(56) matches the type exactly
pub const SOCK_NONBLOCK: c_int = O_NONBLOCK;
pub const SOCK_CLOEXEC: c_int = O_CLOEXEC;

pub const IPPROTO_IGMP: c_int = 2;
pub const IPPROTO_IPIP: c_int = 4;
pub const IPPROTO_EGP: c_int = 8;
pub const IPPROTO_PUP: c_int = 12;
pub const IPPROTO_IDP: c_int = 22;
pub const IPPROTO_TP: c_int = 29;
pub const IPPROTO_DCCP: c_int = 33;
pub const IPPROTO_ROUTING: c_int = 43;
pub const IPPROTO_FRAGMENT: c_int = 44;
pub const IPPROTO_RSVP: c_int = 46;
pub const IPPROTO_GRE: c_int = 47;
pub const IPPROTO_ESP: c_int = 50;
pub const IPPROTO_AH: c_int = 51;
pub const IPPROTO_MTP: c_int = 92;
pub const IPPROTO_BEETPH: c_int = 94;
pub const IPPROTO_ENCAP: c_int = 98;
pub const IPPROTO_PIM: c_int = 103;
pub const IPPROTO_COMP: c_int = 108;
pub const IPPROTO_SCTP: c_int = 132;
pub const IPPROTO_MH: c_int = 135;
pub const IPPROTO_UDPLITE: c_int = 136;
pub const IPPROTO_MPLS: c_int = 137;
pub const IPPROTO_RAW: c_int = 255;
pub const IPPROTO_MPTCP: c_int = 262;

pub const SOL_SOCKET: c_int = 1;
pub const SO_REUSEADDR: c_int = 2;
pub const SO_TYPE: c_int = 3;
pub const SO_ERROR: c_int = 4;
pub const SO_BROADCAST: c_int = 6;
pub const SO_SNDBUF: c_int = 7;
pub const SO_RCVBUF: c_int = 8;
pub const SO_KEEPALIVE: c_int = 9;
pub const SO_OOBINLINE: c_int = 10;
pub const SO_LINGER: c_int = 13;
/// Who is at the other end of a local socket, from `peercred`(83), recorded
/// when the connection was made
pub const SO_PEERCRED: c_int = 17;
pub const SO_REUSEPORT: c_int = 15;
pub const SO_RCVTIMEO: c_int = 20;
pub const SO_SNDTIMEO: c_int = 21;
pub const SO_ACCEPTCONN: c_int = 30;
pub const SO_DOMAIN: c_int = 39;

/// `IP_TTL` is the one implemented option (`setsockopt`(80), for `traceroute`);
/// the rest are names
pub const IP_TOS: c_int = 1;
pub const IP_TTL: c_int = 2;
pub const IP_HDRINCL: c_int = 3;
pub const IP_MULTICAST_IF: c_int = 32;
pub const IP_MULTICAST_TTL: c_int = 33;
pub const IP_MULTICAST_LOOP: c_int = 34;
pub const IP_ADD_MEMBERSHIP: c_int = 35;
pub const IP_DROP_MEMBERSHIP: c_int = 36;

pub const IPV6_UNICAST_HOPS: c_int = 16;
pub const IPV6_MULTICAST_IF: c_int = 17;
pub const IPV6_MULTICAST_HOPS: c_int = 18;
pub const IPV6_MULTICAST_LOOP: c_int = 19;
pub const IPV6_ADD_MEMBERSHIP: c_int = 20;
pub const IPV6_DROP_MEMBERSHIP: c_int = 21;
pub const IPV6_V6ONLY: c_int = 26;
pub const IPV6_TCLASS: c_int = 67;

pub const TCP_NODELAY: c_int = 1;
pub const TCP_MAXSEG: c_int = 2;
pub const TCP_KEEPIDLE: c_int = 4;
pub const TCP_KEEPINTVL: c_int = 5;
pub const TCP_KEEPCNT: c_int = 6;

/// `send`/`recv` flags. `send`(58) and `recv`(59) read none of their flags
/// word; `libc_miku.so` handles `MSG_DONTWAIT` and `MSG_NOSIGNAL` and answers
/// `EOPNOTSUPP` for the rest
pub const MSG_OOB: c_int = 0x1;
pub const MSG_PEEK: c_int = 0x2;
pub const MSG_DONTROUTE: c_int = 0x4;
pub const MSG_CTRUNC: c_int = 0x8;
pub const MSG_TRUNC: c_int = 0x20;
pub const MSG_DONTWAIT: c_int = 0x40;
pub const MSG_EOR: c_int = 0x80;
pub const MSG_WAITALL: c_int = 0x100;
pub const MSG_CONFIRM: c_int = 0x800;
pub const MSG_ERRQUEUE: c_int = 0x2000;
pub const MSG_NOSIGNAL: c_int = 0x4000;
pub const MSG_MORE: c_int = 0x8000;
pub const MSG_CMSG_CLOEXEC: c_int = 0x4000_0000;

/// What `listen` is told for "as many as the system allows". The kernel reads
/// no backlog
pub const SOMAXCONN: c_int = 128;

pub const SHUT_RD: c_int = 0;
pub const SHUT_WR: c_int = 1;
pub const SHUT_RDWR: c_int = 2;

/// Descriptor passing is `sendfd`(84)/`recvfd`(85), one descriptor per call
/// over an `AF_UNIX` socket, not ancillary data, so `sendmsg`/`recvmsg` with a
/// control buffer answer `EOPNOTSUPP`
pub const SCM_RIGHTS: c_int = 1;

pub const SA_NOCLDSTOP: c_int = 0x0000_0001;
/// `sigaction` refuses it with `EINVAL`: the kernel keeps every child until it
/// is waited for
pub const SA_NOCLDWAIT: c_int = 0x0000_0002;
pub const SA_SIGINFO: c_int = 0x0000_0004;
pub const SA_ONSTACK: c_int = 0x0800_0000;
pub const SA_RESTART: c_int = 0x1000_0000;
pub const SA_NODEFER: c_int = 0x4000_0000;
pub const SA_RESETHAND: c_int = 0x8000_0000_u32 as c_int;

s! {
    /// Declared for naming only; the kernel has no IPv6 and answers
    /// `EAFNOSUPPORT`
    pub struct sockaddr_in6 {
        pub sin6_family: sa_family_t,
        pub sin6_port: in_port_t,
        pub sin6_flowinfo: u32,
        pub sin6_addr: crate::unix::in6_addr,
        pub sin6_scope_id: u32,
    }

    /// The form `socket`(56)'s `AF_UNIX` takes, byte for byte. See
    /// `docs/MikuOS_ABI.md` §3.4
    pub struct sockaddr_un {
        pub sun_family: sa_family_t,
        pub sun_path: [c_char; 108],
    }

    /// Room for any address, 128 bytes aligned to eight: Linux's size
    pub struct sockaddr_storage {
        pub ss_family: sa_family_t,
        __ss_pad2: [u8; 118],
        __ss_align: usize,
    }

    pub struct ip_mreq {
        pub imr_multiaddr: in_addr,
        pub imr_interface: in_addr,
    }

    /// `SO_PEERCRED`'s answer, in Linux's shape: the peer's pid and its
    /// effective ids at the moment it connected, as `peercred`(83) keeps them
    pub struct ucred {
        pub pid: crate::unix::pid_t,
        pub uid: crate::unix::uid_t,
        pub gid: crate::unix::gid_t,
    }

    pub struct ip_mreqn {
        pub imr_multiaddr: in_addr,
        pub imr_address: in_addr,
        pub imr_ifindex: c_int,
    }

    /// POSIX's field types: `msg_iovlen` is an `int`, as in musl and the BSDs,
    /// not glibc's `size_t`. `rustix` assumes this for a system it does not
    /// list
    pub struct msghdr {
        pub msg_name: *mut c_void,
        pub msg_namelen: socklen_t,
        pub msg_iov: *mut crate::unix::iovec,
        pub msg_iovlen: c_int,
        pub msg_control: *mut c_void,
        pub msg_controllen: size_t,
        pub msg_flags: c_int,
    }

    pub struct cmsghdr {
        pub cmsg_len: size_t,
        pub cmsg_level: c_int,
        pub cmsg_type: c_int,
    }
}

const fn CMSG_ALIGN(len: usize) -> usize {
    (len + core::mem::size_of::<usize>() - 1) & !(core::mem::size_of::<usize>() - 1)
}

/// The control-message walkers, as arithmetic, matching Linux's layout above.
/// MikuOS never fills a control buffer
pub unsafe fn CMSG_FIRSTHDR(mhdr: *const msghdr) -> *mut cmsghdr {
    if (*mhdr).msg_controllen as usize >= core::mem::size_of::<cmsghdr>() {
        (*mhdr).msg_control.cast()
    } else {
        core::ptr::null_mut()
    }
}

pub unsafe fn CMSG_NXTHDR(mhdr: *const msghdr, cmsg: *const cmsghdr) -> *mut cmsghdr {
    if ((*cmsg).cmsg_len as usize) < core::mem::size_of::<cmsghdr>() {
        return core::ptr::null_mut();
    }
    let next = (cmsg as usize + CMSG_ALIGN((*cmsg).cmsg_len as usize)) as *mut cmsghdr;
    let max = (*mhdr).msg_control as usize + (*mhdr).msg_controllen as usize;
    if (next.wrapping_offset(1)) as usize > max
        || next as usize + CMSG_ALIGN((*next).cmsg_len as usize) > max
    {
        core::ptr::null_mut()
    } else {
        next
    }
}

pub unsafe fn CMSG_DATA(cmsg: *const cmsghdr) -> *mut c_uchar {
    cmsg.offset(1) as *mut c_uchar
}

pub const unsafe fn CMSG_SPACE(length: c_uint) -> c_uint {
    (CMSG_ALIGN(length as usize) + CMSG_ALIGN(core::mem::size_of::<cmsghdr>())) as c_uint
}

pub const unsafe fn CMSG_LEN(length: c_uint) -> c_uint {
    CMSG_ALIGN(core::mem::size_of::<cmsghdr>()) as c_uint + length
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
