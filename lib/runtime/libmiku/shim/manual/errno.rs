// Rust-side helpers: errno state lives in core_miku.so, always go through
// the C accessors so every library sees the same errno.

#[inline]
pub fn set_errno(code: i64) { miku_set_errno(code); }

#[inline]
pub fn get_errno() -> i64 { miku_get_errno() }

#[inline]
pub fn is_error(val: i64) -> bool { val < 0 }

#[inline]
pub fn to_errno(val: i64) -> i64 { if val < 0 { -val } else { 0 } }
