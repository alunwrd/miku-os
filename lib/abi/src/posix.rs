//! The structs upstream `libc` defines for every Unix
//!
//! In `libc`'s tree the overlay replaces this file with a re-export of those, so
//! a `libc::msghdr` holds a `libc::iovec` (builder/src/stdport/overlay.rs).

use super::prelude::*;

c_structs! {
    pub struct timespec {
        pub tv_sec: time_t,
        pub tv_nsec: c_long,
    }

    pub struct timeval {
        pub tv_sec: time_t,
        pub tv_usec: suseconds_t,
    }

    pub struct iovec {
        pub iov_base: *mut c_void,
        pub iov_len: size_t,
    }

    pub struct rlimit {
        pub rlim_cur: rlim_t,
        pub rlim_max: rlim_t,
    }

    pub struct rusage {
        pub ru_utime: timeval,
        pub ru_stime: timeval,
        pub ru_maxrss: c_long,
        pub ru_ixrss: c_long,
        pub ru_idrss: c_long,
        pub ru_isrss: c_long,
        pub ru_minflt: c_long,
        pub ru_majflt: c_long,
        pub ru_nswap: c_long,
        pub ru_inblock: c_long,
        pub ru_oublock: c_long,
        pub ru_msgsnd: c_long,
        pub ru_msgrcv: c_long,
        pub ru_nsignals: c_long,
        pub ru_nvcsw: c_long,
        pub ru_nivcsw: c_long,
    }

    pub struct pollfd {
        pub fd: c_int,
        pub events: c_short,
        pub revents: c_short,
    }

    pub struct winsize {
        pub ws_row: c_ushort,
        pub ws_col: c_ushort,
        pub ws_xpixel: c_ushort,
        pub ws_ypixel: c_ushort,
    }

    pub struct group {
        pub gr_name: *mut c_char,
        pub gr_passwd: *mut c_char,
        pub gr_gid: gid_t,
        pub gr_mem: *mut *mut c_char,
    }

    pub struct in6_addr {
        pub s6_addr: [u8; 16],
    }
}

const _: () = assert!(core::mem::size_of::<timespec>() == 16);
const _: () = assert!(core::mem::size_of::<timeval>() == 16);
const _: () = assert!(core::mem::size_of::<iovec>() == 16);
const _: () = assert!(core::mem::size_of::<rlimit>() == 16);
const _: () = assert!(core::mem::size_of::<rusage>() == 144);
const _: () = assert!(core::mem::size_of::<pollfd>() == 8);
const _: () = assert!(core::mem::size_of::<winsize>() == 8);
