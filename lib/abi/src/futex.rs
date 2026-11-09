//! `futex`(110) and `thread_create`(106)

pub const FUTEX_WAIT: u64 = 0;
pub const FUTEX_WAKE: u64 = 1;

/// The thread frees its own stack on exit
pub const THREAD_STACK_OWNED: u64 = 1 << 0;
