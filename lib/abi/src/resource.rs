//! `<sys/resource.h>`

#![allow(unused_imports)]

use super::prelude::*;

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
// Below: for Rust only, not in the C headers
pub type rlim_t = u64;
/// `id_t`: what `getpriority`, `setpriority` and `waitid` take: a pid, a pgid
/// or a uid
pub type id_t = u32;
pub const RLIM_INFINITY: rlim_t = !0;
/// `getpriority`/`setpriority` targets. A task's priority is set at
/// `spawn_named` and no syscall changes it, so both calls answer `ENOSYS`
pub const PRIO_PROCESS: c_int = 0;
pub const PRIO_PGRP: c_int = 1;
pub const PRIO_USER: c_int = 2;
pub const RUSAGE_SELF: c_int = 0;
pub const RUSAGE_CHILDREN: c_int = -1;
pub const RUSAGE_THREAD: c_int = 1;
