//! `<pthread.h>`. `lib/sdk/c/check/layout.c` pins the sizes

#![allow(unused_imports)]

use super::prelude::*;

pub type pthread_t = usize;
pub type pthread_key_t = u32;
/// `sem_t`: opaque and unimplemented
pub type sem_t = [u64; 4];
// The pthread types, sized as an ELF built elsewhere expects. Implemented by
// `lib/runtime/libmiku/libc/pthread.rs`; `lib/sdk/c/check/layout.c` pins the sizes
pub type pthread_mutex_t = [u64; 5];
pub type pthread_mutexattr_t = [u64; 4];
/// Private only: `pthread_mutexattr_setpshared` refuses SHARED with `ENOTSUP`
pub const PTHREAD_PROCESS_PRIVATE: c_int = 0;
pub const PTHREAD_PROCESS_SHARED: c_int = 1;
pub type pthread_cond_t = [u64; 6];
pub type pthread_condattr_t = [u64; 4];
pub type pthread_rwlock_t = [u64; 7];
pub type pthread_rwlockattr_t = [u64; 4];
pub type pthread_attr_t = [u64; 7];
/// The static initialisers: every pthread object in `libc_miku.so` is valid
/// all-zero (lib/runtime/libmiku/libc/pthread.rs)
pub const PTHREAD_MUTEX_INITIALIZER: pthread_mutex_t = [0; 5];
pub const PTHREAD_COND_INITIALIZER: pthread_cond_t = [0; 6];
pub const PTHREAD_RWLOCK_INITIALIZER: pthread_rwlock_t = [0; 7];
