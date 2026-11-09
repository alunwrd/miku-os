//! `<sched.h>`

#![allow(unused_imports)]

use super::prelude::*;

/// Linux's policy numbers. Only `SCHED_OTHER` is one a thread can have
pub const SCHED_OTHER: c_int = 0;
pub const SCHED_FIFO: c_int = 1;
pub const SCHED_RR: c_int = 2;
c_structs! {
    pub struct sched_param {
        pub sched_priority: c_int,
    }
}
