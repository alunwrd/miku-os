//! core_miku: synchronisation, threads

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32};

#[repr(C)]
pub struct MikuAtomic {
    pub val: AtomicI64,
}

#[repr(C)]
pub struct MikuMutex {
    pub state: AtomicU32,
}

impl MikuMutex {
    pub const fn new() -> Self {
        Self { state: AtomicU32::new(0) }
    }
}

#[repr(C)]
pub struct MikuOnce {
    pub done: AtomicBool,
    pub running: AtomicBool,
}

/// What a thread runs. Returns an exit code, and returning is how it ends.
pub type ThreadFn = extern "C" fn(*mut u8) -> i32;
