// Threads and the futex under them
//
// A thread is a task sharing its creator's address space, descriptors, working
// directory, and identity, with its own stack, registers, and TLS. Kernel side:
// syscalls 106-110; see dev/std-architecture.md.
//
// Nothing here allocates. The caller provides and frees a thread's stack, since
// a dying thread cannot safely touch a shared heap.

use crate::sys;

pub use miku_abi::futex::FUTEX_WAIT;
pub use miku_abi::futex::FUTEX_WAKE;

pub use miku_abi::libtypes::ThreadFn;

/// The trampoline every thread starts in.
///
/// The kernel enters a new thread with one argument in `rdi` and an empty
/// stack, with no return address, so the caller's function cannot run first.
///
/// `rdi` points at a two-word block the creating thread wrote to the top of the
/// new stack, holding the function and its argument. A heap block would have to
/// be freed by the new thread under a shared lock.
///
/// After `call`, `rsp % 16 == 8`, as SysV promises on entry: the kernel's stack
/// is 16-aligned and the call pushes eight bytes.
core::arch::global_asm!(
    ".global __miku_thread_entry",
    ".type __miku_thread_entry,@function",
    "__miku_thread_entry:",
    "  mov rax, [rdi]",        // fn
    "  mov rdi, [rdi + 8]",    // arg
    "  call rax",
    "  mov ebx, eax",          // the function's return value is the exit code
    "  call miku_heap_thread_done", // this thread's heap cache back (`heap.rs`)
    "  movsxd rdi, ebx",
    "  mov eax, 107",          // SYS_THREAD_EXIT
    "  syscall",
    "  ud2",                   // thread_exit does not return
    ".size __miku_thread_entry,.-__miku_thread_entry",
);

extern "C" {
    fn __miku_thread_entry();
}

/// The MikuOS thread control block as `ld-miku` leaves it at the thread
/// pointer. Defined in `lib/runtime/ld-miku/src/loader.rs`
use miku_abi::tcb::{TCB_SIZE, TCB_TLS_ALIGN, TCB_TLS_FILESZ, TCB_TLS_IMAGE, TCB_TLS_SIZE};

/// Stack required under the thread's TLS block: what the smallest program here
/// uses before its first call. A smaller region is refused
const MIN_USABLE_STACK: usize = 8192;

/// This thread's pointer, read from `%fs:0`. `%fs` cannot be read directly in
/// ring 3 (`rdfsbase` needs `CR4.FSGSBASE`), so the x86-64 ABI puts the value
/// at offset zero of what it points at
#[inline(always)]
fn thread_pointer() -> usize {
    let tp: usize;
    unsafe { core::arch::asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly, preserves_flags)) };
    tp
}

/// Starts a thread on a stack the caller owns.
///
/// `stack` is the region's low address and `stack_size` its length. From the
/// top down it holds the trampoline's start block, this thread's TLS block and
/// TCB, and the stack. All are freed with the region, which must outlive the
/// thread.
///
/// Returns the thread id or a negative errno: `-EINVAL` for a region too small
/// for a stack and this program's thread-local data, `-ENOMEM` when the kernel
/// has no kernel stack for it
#[no_mangle]
pub extern "C" fn miku_thread_spawn(
    entry: ThreadFn,
    arg: *mut u8,
    stack: *mut u8,
    stack_size: usize,
) -> i64 {
    match thread_layout(entry, arg, stack, stack_size) {
        Ok((block, rsp, tp)) => spawn_raw(block, rsp, tp, 0, 0, 0),
        Err(e) => e,
    }
}

/// Lays out a thread's start block, TCB, and thread-local data at the top of
/// its region and says where the stack starts. Returns
/// `(arg_block, rsp, thread_pointer)`. Shared by both spawns, which differ only
/// in who frees the region
fn thread_layout(
    entry: ThreadFn,
    arg: *mut u8,
    stack: *mut u8,
    stack_size: usize,
) -> Result<(usize, usize, usize), i64> {
    if stack.is_null() || stack_size < MIN_USABLE_STACK {
        return Err(-22); // EINVAL
    }

    // This program's thread-locals, from the TCB `ld-miku` built for the first
    // thread. Zero size means no PT_TLS and no block, but a thread still gets a
    // TCB, as `%fs:0` must be readable
    let tp = thread_pointer();
    let (tls_size, tls_align, tls_image, tls_filesz) = if tp == 0 {
        (0usize, 16usize, 0usize, 0usize)
    } else {
        unsafe {
            (
                ((tp + TCB_TLS_SIZE)   as *const u64).read() as usize,
                (((tp + TCB_TLS_ALIGN) as *const u64).read() as usize).max(16),
                ((tp + TCB_TLS_IMAGE)  as *const u64).read() as usize,
                ((tp + TCB_TLS_FILESZ) as *const u64).read() as usize,
            )
        }
    };

    let top = (stack as usize + stack_size) & !0xF;
    // The start block for the trampoline: two words at the very top
    let block = top - 16;
    // The TCB under it, aligned as the program's TLS segment demands, and
    // thread-local data under that: variant II, the shape `ld-miku` builds
    let new_tp = (block - TCB_SIZE) & !(tls_align - 1);
    let tls_lo = new_tp.saturating_sub(tls_size);
    let rsp = tls_lo & !0xF;

    // Everything above must fit, or a thread overruns into its own
    // thread-locals
    if rsp < stack as usize + MIN_USABLE_STACK {
        return Err(-22); // EINVAL
    }

    unsafe {
        (block as *mut u64).write(entry as *const () as usize as u64);
        ((block + 8) as *mut u64).write(arg as u64);

        // `.tdata` copied, `.tbss` zeroed: the difference between filesz and
        // memsz
        let copy = tls_filesz.min(tls_size);
        if tls_size > copy {
            crate::mem::miku_memset((tls_lo + copy) as *mut u8, 0, tls_size - copy);
        }
        if copy > 0 && tls_image != 0 {
            crate::mem::miku_memcpy(tls_lo as *mut u8, tls_image as *const u8, copy);
        }

        // The TCB as `ld-miku` built the first. Template fields carry forward,
        // so a thread can spawn a thread
        core::ptr::write_bytes(new_tp as *mut u8, 0, TCB_SIZE);
        (new_tp as *mut u64).write(new_tp as u64);
        ((new_tp + TCB_TLS_SIZE)   as *mut u64).write(tls_size as u64);
        ((new_tp + TCB_TLS_ALIGN)  as *mut u64).write(tls_align as u64);
        ((new_tp + TCB_TLS_IMAGE)  as *mut u64).write(tls_image as u64);
        ((new_tp + TCB_TLS_FILESZ) as *mut u64).write(tls_filesz as u64);
    }

    Ok((block, rsp, new_tp))
}

/// `flags` bit 0: the kernel unmaps `[stack_base, stack_base+stack_len)` when
/// this thread dies. See `kernel/syscall/adapters/thread.rs`
pub use miku_abi::futex::THREAD_STACK_OWNED;

/// The syscall itself, for both spawns.
///
///   struct ThreadArgs { entry, stack, arg, tls, flags, stack_base, stack_len, reserved }
fn spawn_raw(arg_block: usize, rsp: usize, tp: usize, flags: u64, base: usize, len: usize) -> i64 {
    let args: [u64; 8] = [
        __miku_thread_entry as *const () as usize as u64,
        rsp as u64,
        arg_block as u64,
        tp as u64,
        flags,
        base as u64,
        len as u64,
        0,
    ];
    unsafe { sys::sc1(sys::SYS_THREAD_CREATE, args.as_ptr() as u64) }
}

/// Starts a thread on a region the kernel unmaps when it dies.
///
/// Same layout as `miku_thread_spawn`, which leaves the stack to the caller
/// after a join. A detached thread has no such moment and cannot unmap its own
/// stack, so the kernel does it when the task is off every processor and the
/// address space still belongs to its siblings.
///
/// `base`/`len` must be the whole mapping, page aligned, and the region this
/// thread runs on. Returns the tid or a negative errno.
///
/// The lowest page becomes the stack's guard, `PROT_NONE`, so an overflow
/// faults there. Both callers (`std::thread`, `pthread_create`) hand over a
/// fresh mapping, and `stack_size` includes the page
#[no_mangle]
pub extern "C" fn miku_thread_spawn_owned(
    entry: ThreadFn,
    arg: *mut u8,
    stack: *mut u8,
    stack_size: usize,
) -> i64 {
    if stack.is_null() || stack_size <= GUARD {
        return -22; // EINVAL
    }
    let usable = unsafe { stack.add(GUARD) };
    match thread_layout(entry, arg, usable, stack_size - GUARD) {
        Ok((block, rsp, tp)) => {
            // The stack is lazy and the bottom page untouched, so this only
            // records the protection; failure leaves a thread with no guard
            unsafe { sys::sc3(sys::SYS_MPROTECT, stack as u64, GUARD as u64, 0) };
            spawn_raw(block, rsp, tp, THREAD_STACK_OWNED, stack as usize, stack_size)
        }
        Err(e) => e,
    }
}

/// The guard under a thread's stack, one page
pub const GUARD: usize = 4096;

/// Ends the calling thread; does not return. From the thread-group leader this
/// ends the whole process, per `sys_thread_exit`
#[no_mangle]
pub extern "C" fn miku_thread_exit(code: i32) -> ! {
    // Its heap cache's blocks belong to the program
    crate::heap::miku_heap_thread_done();
    unsafe { sys::sc1(sys::SYS_THREAD_EXIT, code as i64 as u64) };
    loop { core::hint::spin_loop() }
}

/// This thread's id. `miku_getpid` answers with the process.
#[no_mangle]
pub extern "C" fn miku_gettid() -> i64 {
    unsafe { sys::sc0(sys::SYS_GETTID) }
}

/// Yields the processor
#[no_mangle]
pub extern "C" fn miku_sched_yield() -> i64 {
    unsafe { sys::sc0(sys::SYS_SCHED_YIELD) }
}

/// Sleeps on `addr` while it holds `expected`.
///
/// `timeout_ns` of zero means none. A return of 0 means the sleep ended, not
/// that the word changed; callers re-check in a loop, as with any futex.
///
/// `-EAGAIN` means the word did not hold `expected` and it did not sleep
#[no_mangle]
pub extern "C" fn miku_futex_wait(addr: *const u32, expected: u32, timeout_ns: u64) -> i64 {
    let ts: [i64; 2] = [
        (timeout_ns / 1_000_000_000) as i64,
        (timeout_ns % 1_000_000_000) as i64,
    ];
    let tp = if timeout_ns == 0 { 0 } else { ts.as_ptr() as u64 };
    unsafe { sys::sc4(sys::SYS_FUTEX, addr as u64, FUTEX_WAIT, expected as u64, tp) }
}

/// Wakes at most `count` sleepers on `addr`, returning the number woken
#[no_mangle]
pub extern "C" fn miku_futex_wake(addr: *const u32, count: u32) -> i64 {
    unsafe { sys::sc4(sys::SYS_FUTEX, addr as u64, FUTEX_WAKE, count as u64, 0) }
}
