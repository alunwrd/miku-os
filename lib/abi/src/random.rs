//! `<sys/random.h>`

#![allow(unused_imports)]

use super::prelude::*;

pub const GRND_NONBLOCK: c_uint = 0x0001;
pub const GRND_RANDOM: c_uint = 0x0002;
pub const GRND_INSECURE: c_uint = 0x0004;
