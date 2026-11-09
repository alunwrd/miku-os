//! MikuOS libc
//!
//! MikuOS declares `target_family = "unix"` because its C interface is POSIX:
//! `fork`/`execve`/`wait4`, `open`/`read`/`write`/`lseek`, `poll`, `fcntl` and
//! BSD sockets, exported by `libc_miku.so` under their POSIX names. See
//! `dev/std-architecture.md`.
//!
//! * Headers: none. The interface is `lib/runtime/libmiku/libc/`, and this module is
//!   written against that file.

pub(crate) mod unistd;
