//! `<time.h>`

#![allow(unused_imports)]

use super::prelude::*;

/// `clock_gettime`(70) knows `REALTIME` and `MONOTONIC` and answers `EINVAL` to
/// anything else. No per-process or per-thread CPU clock is exposed;
/// `rustix::time` enumerates the other two ids
pub const CLOCK_REALTIME: clockid_t = 0;
pub const CLOCK_MONOTONIC: clockid_t = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: clockid_t = 2;
pub const CLOCK_THREAD_CPUTIME_ID: clockid_t = 3;
// Below: for Rust only, not in the C headers
pub type time_t = i64;
pub type clock_t = i64;
pub type suseconds_t = i64;
// Clocks, limits, priorities, and the rest of what `rustix::process` names
pub type clockid_t = c_int;
c_structs! {
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
}
