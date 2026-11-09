//! `<sys/select.h>`

#![allow(unused_imports)]

use super::prelude::*;

c_structs! {
    pub struct fd_set {
        fds_bits: [u64; 16],
    }
}
