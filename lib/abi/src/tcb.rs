//! The thread control block at the thread pointer. The x86-64 ABI fixes only the
//! first word, the pointer's own value. `ld-miku` builds it for a dynamic program,
//! the kernel for a static one, and `thread_spawn` for every other thread
//! (`docs/MikuOS_ABI.md` 7.6). Byte offsets from the thread pointer

pub const TCB_SIZE: usize = 64;
/// Bytes of thread-local data below the thread pointer
pub const TCB_TLS_SIZE: usize = 8;
/// Alignment of the thread pointer
pub const TCB_TLS_ALIGN: usize = 16;
/// Address of the initial data in the program's image
pub const TCB_TLS_IMAGE: usize = 24;
/// How much of the thread-local data the image covers; the rest is `.tbss`
pub const TCB_TLS_FILESZ: usize = 32;
/// `errno`, four bytes
pub const TCB_ERRNO: usize = 40;
/// The thread's `pthread` block
pub const TCB_PTHREAD: usize = 48;
/// The thread's heap cache
pub const TCB_HEAP_CACHE: usize = 56;
