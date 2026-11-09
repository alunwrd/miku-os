//! MikuOS: the platform layer.
//!
//! `std` picks `fs`, `net`, `thread`, `time` and the rest separately, so this
//! file holds what belongs to the platform as a whole: how a program starts,
//! how it stops, and how a MikuOS errno becomes an `io::Error`.
//!
//! Everything here calls `libmiku` through `abi.rs`, the shared libraries every
//! MikuOS program links. See `dev/std-architecture.md`.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod abi;
pub mod futex;

use crate::io::ErrorKind;

/// Start-up.
///
/// The environment is imported from the initial stack before anything reads it:
/// `libmiku` keeps it in its own heap-backed table, and `miku_env_init` fills
/// that from the `envp` the kernel left on the stack.
///
/// # Safety
/// Must be called exactly once, during runtime initialization.
pub unsafe fn init(_argc: isize, argv: *const *const u8, sigpipe: u8) {
    reset_sigpipe(sigpipe);
    // SAFETY: the caller promises this is the real initial stack, where argv is
    // NULL-terminated and envp follows it, the SysV layout of
    // `docs/MikuOS_ABI.md` §4.4
    unsafe {
        crate::sys::args::init(_argc, argv);

        if !argv.is_null() {
            let mut end = argv;
            while !(*end).is_null() {
                end = end.add(1);
            }
            let envp = end.add(1);
            abi::miku_env_init(envp);
        }
    }
}

const SIGPIPE: u32 = crate::sys::pal::abi::defs::signal::SIGPIPE as u32;

/// Whether the program chose its SIGPIPE (`#[unix_sigpipe]` /
/// `-Zon-broken-pipe`), in which case a spawned child inherits the choice.
/// Upstream's rule, `sys/pal/unix`
static ON_BROKEN_PIPE_USED: crate::sync::atomic::Atomic<bool> =
    crate::sync::atomic::AtomicBool::new(false);

/// SIGPIPE ignored unless the program said otherwise, as on every unix: a write
/// to a pipe or socket nobody reads is then `EPIPE`, which `std` turns into an
/// error
fn reset_sigpipe(sigpipe: u8) {
    // rustc's numbering, `compiler/rustc_session/src/config/sigpipe.rs`
    const DEFAULT: u8 = 0;
    const INHERIT: u8 = 1;
    const SIG_IGN: u8 = 2;
    const SIG_DFL: u8 = 3;
    let used = sigpipe != DEFAULT;
    ON_BROKEN_PIPE_USED.store(used, crate::sync::atomic::Ordering::Relaxed);
    match sigpipe {
        DEFAULT | SIG_IGN => {
            abi::miku_signal_ignore(SIGPIPE);
        }
        SIG_DFL => {
            abi::miku_signal(SIGPIPE, 0);
        }
        INHERIT | _ => {}
    }
}

/// What a spawned child does before its `exec`: SIGPIPE back to the default
/// unless the program chose it. The kernel keeps an ignored signal across
/// `exec`, so a child would inherit the ignore and never end at a closed pipe
pub(crate) fn child_sigpipe_default() {
    if !ON_BROKEN_PIPE_USED.load(crate::sync::atomic::Ordering::Relaxed) {
        abi::miku_signal(SIGPIPE, 0);
    }
}

/// Where a MikuOS program starts.
///
/// The kernel enters the ELF's entry point with the SysV initial stack below
/// it: `argc`, then `argv` NUL-terminated, then `envp`, then the auxiliary
/// vector. There is no `crt0` and no libc start files.
///
/// `lib/sdk/miku` has its own `_start` for `#![no_std] #![no_main]` programs, which
/// calls `_start_main`. Only one of the two is ever linked.
///
/// `main` here is the shim rustc generates around the user's `fn main`, with
/// signature `(argc, argv) -> i32`. rustc's value to `lang_start` decides
/// SIGPIPE, and `init` carries it out.
#[cfg(not(test))]
core::arch::global_asm!(
    ".global _start",
    ".type _start,@function",
    "_start:",
    "  xor rbp, rbp",                    // end the frame-pointer chain here
    "  mov rdi, [rsp]",                  // argc
    "  lea rsi, [rsp + 8]",              // argv
    "  mov [rip + MIKU_STD_INITIAL_SP], rsp",
    "  xor edx, edx",                    // no third argument; see `init`
    "  and rsp, -16",                    // SysV: aligned before the call
    "  call main",
    "  movsxd rdi, eax",
    "  call miku_exit",
    "  ud2",                             // miku_exit does not return
    ".size _start,.-_start",
);

/// The stack pointer as the kernel left it.
///
/// The auxiliary vector is above `envp` on that stack and nowhere else, so
/// anything wanting `AT_SYSINFO_EHDR` (the vDSO, how `clock_gettime` avoids a
/// syscall) has to be told where the stack started. `lib/sdk/miku/src/time.rs`
/// keeps the same word.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub static mut MIKU_STD_INITIAL_SP: usize = 0;

/// # Safety
/// Must be called exactly once, during runtime cleanup. Not guaranteed to run.
pub unsafe fn cleanup() {}

pub fn unsupported<T>() -> crate::io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> crate::io::Error {
    crate::io::Error::UNSUPPORTED_PLATFORM
}

/// The last resort. `miku_exit`, since `core::intrinsics::abort` is a `ud2`: a
/// crash report and a core file
pub fn abort_internal() -> ! {
    abi::miku_exit(134) // 128 + SIGABRT, as a shell prints it
}

/// MikuOS errno to `ErrorKind`.
///
/// Linux's values, the same as `kernel/syscall/errno.rs`. `ENOTSUP` is
/// `EOPNOTSUPP`, and `EWOULDBLOCK` is `EAGAIN`.
pub fn decode_error_kind(errno: i32) -> ErrorKind {
    use ErrorKind::*;
    match errno {
        1 => PermissionDenied,     // EPERM
        2 => NotFound,             // ENOENT
        4 => Interrupted,          // EINTR
        5 => Uncategorized,        // EIO
        9 => Uncategorized,        // EBADF
        11 => WouldBlock,          // EAGAIN
        12 => OutOfMemory,         // ENOMEM
        13 => PermissionDenied,    // EACCES
        14 => InvalidInput,        // EFAULT
        16 => ResourceBusy,        // EBUSY
        17 => AlreadyExists,       // EEXIST
        18 => CrossesDevices,      // EXDEV
        20 => NotADirectory,       // ENOTDIR
        21 => IsADirectory,        // EISDIR
        22 => InvalidInput,        // EINVAL
        24 => Uncategorized,       // EMFILE
        28 => StorageFull,         // ENOSPC
        30 => ReadOnlyFilesystem,  // EROFS
        32 => BrokenPipe,          // EPIPE
        36 => InvalidFilename,     // ENAMETOOLONG
        39 => DirectoryNotEmpty,   // ENOTEMPTY
        38 => Unsupported,         // ENOSYS
        40 => FilesystemLoop,      // ELOOP
        95 => Unsupported,         // EOPNOTSUPP
        98 => AddrInUse,           // EADDRINUSE
        104 => ConnectionReset,    // ECONNRESET
        110 => TimedOut,           // ETIMEDOUT
        111 => ConnectionRefused,  // ECONNREFUSED
        _ => Uncategorized,
    }
}

/// Turn a negative return from `libmiku` into an `io::Error`. Every call in
/// `abi.rs` answers with `-errno`
pub fn cvt(r: i64) -> crate::io::Result<i64> {
    if r < 0 { Err(crate::io::Error::from_raw_os_error(-r as i32)) } else { Ok(r) }
}
