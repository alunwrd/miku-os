//! MikuOS-specific extensions to [`std::thread`], reached as
//! `std::os::unix::thread`.
//!
//! The raw handle is the thread's kernel id: the tid `thread_create` answered
//! with, which `gettid` reports from inside the thread. `libc_miku.so`'s
//! `pthread_*` build on the same kernel threads, but this handle is not one of
//! theirs. Crates (`crossbeam-utils` passes it through) need only that it names
//! the thread.

#![stable(feature = "thread_extensions", since = "1.9.0")]

use crate::sys::{AsInner, IntoInner};
use crate::thread::JoinHandle;

/// A thread's kernel id
#[stable(feature = "thread_extensions", since = "1.9.0")]
pub type RawPthread = u64;

/// Access to a [`JoinHandle`]'s kernel thread id
#[stable(feature = "thread_extensions", since = "1.9.0")]
pub trait JoinHandleExt {
    /// The thread's kernel id, without giving the handle up
    #[stable(feature = "thread_extensions", since = "1.9.0")]
    fn as_pthread_t(&self) -> RawPthread;

    /// The thread's kernel id, giving the handle up. A MikuOS thread cannot be
    /// joined by id, so this detaches it, as dropping the handle would
    #[stable(feature = "thread_extensions", since = "1.9.0")]
    fn into_pthread_t(self) -> RawPthread;
}

#[stable(feature = "thread_extensions", since = "1.9.0")]
impl<T> JoinHandleExt for JoinHandle<T> {
    fn as_pthread_t(&self) -> RawPthread {
        self.as_inner().id()
    }

    fn into_pthread_t(self) -> RawPthread {
        self.into_inner().into_id()
    }
}
