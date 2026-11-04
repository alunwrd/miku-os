// Threads and futexes: the two syscalls `std` needs
//
// `std::thread`, `std::sync::{Mutex, Condvar, RwLock}` and `mpsc` rest on
// these. `dev/std-architecture.md` §1 and §2 are the design; this file is the
// boundary.
//
// A thread here is a `Process` sharing its leader's `cr3` and carrying the
// leader's pid as its `tgid`. What is process-wide (descriptors, working
// directory, credential, terminal, job) is reached through the leader by
// `sched`, and what a task must have alone (a kernel stack, a register frame,
// `FS_BASE`, the FPU area) is on the task's own struct.

use crate::syscall::errno::{err, EAGAIN, EFAULT, EINVAL, ENOMEM, EPERM, ETIMEDOUT};
use crate::syscall::usercopy::{current_cr3, user_ptr_mapped};
use crate::sched::futex;

/// `thread_create` wants five things and the syscall ABI carries four
/// (`kernel/syscall/mod.rs`: `rdi, rsi, rdx, r10`), so they travel as a struct
/// behind one pointer, as for `mmap_file`(60) and `execve`(67).
///
///   struct ThreadArgs {
///       entry, stack, arg, tls, flags, stack_base, stack_len, reserved
///   } (u64 x8)
///
/// `reserved` is checked to be zero, so it can be given a meaning later
const THREAD_ARGS_WORDS: usize = 8;

/// `stack_base`/`stack_len` name a mapping the kernel unmaps when this thread
/// dies.
///
/// A thread's stack otherwise has nobody to free it: its last instruction is
/// the moment it cannot unmap the stack it stands on, and `std::thread`
/// detaches every thread whose handle is dropped
pub const THREAD_STACK_OWNED: u64 = 1 << 0;

// 106  thread_create(args_ptr) -> tid | -errno
pub fn sys_thread_create(args_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    if cr3 == crate::mm::vmm::kernel_cr3() {
        // A kernel thread has no user stack to be given and no address space
        // to share; `sched::spawn` is how the kernel makes those
        return err(EPERM);
    }

    let n = (THREAD_ARGS_WORDS * 8) as u64;
    let _ua = match user_ptr_mapped(cr3, args_ptr, n) {
        Some(g) => g,
        None => return err(EFAULT),
    };
    let mut a = [0u64; THREAD_ARGS_WORDS];
    unsafe {
        for (i, slot) in a.iter_mut().enumerate() {
            *slot = (args_ptr as *const u64).add(i).read_unaligned();
        }
    }
    let (entry, stack, arg, tls, flags) = (a[0], a[1], a[2], a[3], a[4]);
    let (stack_base, stack_len, reserved) = (a[5], a[6], a[7]);
    if reserved != 0 { return err(EINVAL); }
    if flags & !THREAD_STACK_OWNED != 0 { return err(EINVAL); }

    // A mapping the kernel is asked to own has to be one the caller could have
    // unmapped itself: in the user half, page aligned, not empty. Checked here
    // because the unmap happens later, in the reaper, where a bad range is a
    // kernel fault with no caller to blame
    let owned = if flags & THREAD_STACK_OWNED != 0 {
        if stack_base & 0xFFF != 0 || stack_len == 0 { return err(EINVAL); }
        if !crate::mm::vmm::is_user_range(stack_base, stack_len) { return err(EFAULT); }
        // And it has to be the stack this thread was given, or the kernel could
        // be handed an arbitrary range to unmap
        if stack < stack_base || stack > stack_base + stack_len { return err(EINVAL); }
        (stack_base, stack_len)
    } else {
        (0, 0)
    };

    // The entry point and the stack are addresses this kernel will `iretq` to
    // and load into `rsp`. Both are checked here and nowhere else
    if !crate::mm::vmm::is_user_va(entry) { return err(EFAULT); }
    if !crate::mm::vmm::is_user_va(stack) { return err(EFAULT); }
    // 16-byte alignment: the new thread starts in compiled code assuming SysV,
    // and a misaligned stack would show at the first `movaps` in a library
    if stack & 0xF != 0 { return err(EINVAL); }
    // A TLS base is written to FS_BASE. Non-canonical or kernel-half would
    // `#GP` inside the context switch, where there is nothing to report it to
    if tls != 0 && !crate::mm::vmm::is_user_va(tls) { return err(EFAULT); }

    let leader = crate::sched::current_tgid();
    let child = match crate::process::Process::new_thread(leader, cr3, entry, stack, arg, tls, owned) {
        Some(c) => c,
        None    => return err(ENOMEM),
    };
    let tid = child.pid;

    // The terminal is per-task: the context switch reads `next.ctty` directly
    // to set the percpu terminal. See `sched::set_ctty`
    child.ctty.store(crate::sched::ctty_of(leader), core::sync::atomic::Ordering::Relaxed);
    child.pgid.store(crate::sched::pgid_of(leader), core::sync::atomic::Ordering::Relaxed);

    crate::trace_println!(
        "[thread] tgid={} tid={} entry={:#x} sp={:#x} tls={:#x}",
        leader, tid, entry, stack, tls);

    crate::sched::add_user_process(child);
    // In the table now, so a group kill that starts from here on finds it;
    // one already under way may have walked past, and is what this asks
    if crate::sched::group_exiting(leader) {
        // The caller is told it failed, and a failed spawn unmaps its own stack
        // (std's `Thread::new` does), so the kernel must not also unmap it at
        // the reap
        let p = unsafe { crate::sched::proc_index_raw(tid) };
        if !p.is_null() {
            unsafe {
                (*p).owned_stack.store(0, core::sync::atomic::Ordering::Relaxed);
                (*p).owned_stack_len.store(0, core::sync::atomic::Ordering::Relaxed);
            }
        }
        crate::sched::kill(tid);
        return err(EAGAIN);
    }
    tid
}

// 107  thread_exit(code) -> never returns
//
// When the task calling this leads the group, the whole process ends. POSIX
// lets `pthread_exit` from `main` leave the other threads running, which would
// need a leader that is dead but unreapable until the last thread goes. A
// program wanting that can park its main thread on a futex
pub fn sys_thread_exit(code: u64) -> u64 {
    let pid  = crate::sched::current_pid();
    let tgid = crate::sched::tgid_of(pid);

    if pid == tgid {
        return super::process::sys_exit(code);
    }

    crate::trace_println!("[thread] exit tid={} of tgid={} code={}", pid, tgid, code);
    // No `release_fds_on_exit`: the descriptors are the process's and its
    // other threads are still using them
    crate::sched::kill_with_code(pid, code);
    crate::sched::yield_now();
    0
}

// 108  gettid() -> tid
//
// `getpid`(7) answers with the process, for a thread its leader
pub fn sys_gettid() -> u64 {
    crate::sched::current_pid()
}

// 109  sched_yield() -> 0
pub fn sys_sched_yield() -> u64 {
    crate::sched::yield_now();
    0
}

// 110  futex(uaddr, op, val, timeout_ptr) -> 0 | woken | -errno
//
// `uaddr` is the word, `op` is FUTEX_WAIT or FUTEX_WAKE, `val` is the value
// expected (WAIT) or the number to wake (WAKE), and `timeout_ptr` is a relative
// `struct timespec { i64 sec; i64 nsec; }` or zero for no timeout.
//
// WAIT returns 0 when woken, including for no reason, which callers must expect
// and re-check their word after; -EAGAIN when the word did not hold `val` and
// it never slept; and -ETIMEDOUT when the deadline passed. WAKE returns how
// many it woke
pub fn sys_futex(uaddr: u64, op: u64, val: u64, timeout_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    if uaddr & 0x3 != 0 { return err(EINVAL); }

    // Every look at the word takes its own short-lived access guard, none held
    // across the sleep: the guard lifts SMAP for this range, and carrying one
    // into a context switch would hand the next task a window. `sys_poll`
    // scopes its the same way
    let peek = || -> Option<u64> {
        let _ua = user_ptr_mapped(cr3, uaddr, 4)?;
        Some(unsafe { (uaddr as *const u32).read_volatile() } as u64)
    };
    if peek().is_none() { return err(EFAULT); }

    match op {
        futex::FUTEX_WAKE => {
            let n = if val == 0 { 0 } else { val };
            futex::wake(cr3, uaddr, n)
        }
        futex::FUTEX_WAIT => {
            // The cheap compare: it stops a caller sleeping on a word that has
            // moved, before any queue is touched. The one that has to be right
            // is the second, inside the queue
            let want = val & 0xFFFF_FFFF;
            match peek() {
                Some(cur) if cur == want => {}
                Some(_) => return err(EAGAIN),
                None    => return err(EFAULT),
            }
            let timeout = if timeout_ptr == 0 {
                None
            } else {
                match read_timeout_ns(cr3, timeout_ptr) {
                    Ok(t)  => Some(t),
                    Err(e) => return e,
                }
            };
            // Read once more from inside the queue (see `futex::wait`). A word
            // that became unreadable is a program unmapping its own lock, and
            // this stops waiting on it
            match futex::wait(cr3, uaddr, timeout, || peek() == Some(want)) {
                futex::WaitResult::Woken    => 0,
                futex::WaitResult::TimedOut => err(ETIMEDOUT),
            }
        }
        _ => err(EINVAL),
    }
}

/// A relative `timespec` from userspace, in nanoseconds, waited for as given
/// (`sched::timer`)
fn read_timeout_ns(cr3: u64, ptr: u64) -> Result<u64, u64> {
    let _ua = match user_ptr_mapped(cr3, ptr, 16) {
        Some(g) => g,
        None => return Err(err(EFAULT)),
    };
    let (sec, nsec) = unsafe {
        (
            (ptr as *const i64).read_unaligned(),
            (ptr as *const i64).add(1).read_unaligned(),
        )
    };
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return Err(err(EINVAL));
    }
    Ok((sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64))
}
