//! `<sys/mman.h>`

#![allow(unused_imports)]

use super::prelude::*;

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
// Below: for Rust only, not in the C headers
pub const MAP_FAILED: *mut c_void = !0 as *mut c_void;
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
