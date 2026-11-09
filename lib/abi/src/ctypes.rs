//! C's base types on x86-64. `libc` has its own names for these and does not
//! re-export this file

pub use core::ffi::c_void;

pub type c_char = i8;
pub type c_schar = i8;
pub type c_uchar = u8;
pub type c_short = i16;
pub type c_ushort = u16;
pub type c_int = i32;
pub type c_uint = u32;
pub type c_long = i64;
pub type c_ulong = u64;
pub type c_longlong = i64;
pub type c_ulonglong = u64;
pub type size_t = usize;
pub type ssize_t = isize;
