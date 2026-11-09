// Sleeping on a word of the program's own memory
//
// The primitive every other blocking primitive in userspace is built from: an
// uncontended lock costs an atomic, and a contended one must not cost a
// spinning core. Taking a thread off the run queue needs the kernel.
//
// `library/std/src/sys/sync/{mutex,condvar,rwlock,once,thread_parking}/` route
// a target with a futex straight at their `futex.rs` implementations, so
// `std::sync` needs no MikuOS-specific code (`dev/std-architecture.md` §2).
//
// The key
//
// (cr3, uaddr), not a physical address. Linux keys a shared futex on the
// physical page so two processes mapping one page agree, and a private one on
// the address space: a copy-on-write fault moves the word to a new frame, and a
// waiter queued on the old frame would never be woken. Everything `std` needs
// is private, and threads share a `cr3` by definition, so no page-table walk is
// needed.
//
// A word in a page mapped `PTE_SHARED` (a tmpfs file's own frame, mapped
// `MAP_SHARED`, `mm::mmap`) is keyed on that frame. Every process mapping the
// file maps the one frame, and a shared page is never copied on write. LMDB
// keeps its reader table's mutexes in such a file.
//
// Spurious wakes
//
// `wait` sleeps with a backstop, like every other wait in this kernel
// (`sched::wait`), so it can return 0 without anybody calling `wake`, and
// callers re-check their word in a loop. `FUTEX_WAIT` promises the same
// everywhere.

extern crate alloc;

use alloc::vec::Vec;
use spin::Mutex;

/// Block until `wake`, until the word changes, or until the timeout.
pub use miku_abi::futex::FUTEX_WAIT;
/// Wake up to `val` waiters on this word.
pub use miku_abi::futex::FUTEX_WAKE;

/// How long a waiter sleeps before waking to look at its word again.
///
/// 100 ms: an uncontended wait costs ten wake-ups a second, and a lost wake
/// slows a program rather than stopping it. A real wake arrives at once through
/// `sched::wakeup`
const BACKSTOP_NS: u64 = 100_000_000;

/// Buckets. A power of two so the modulus is a mask.
///
/// The machine has a few thousand tasks at most and a bucket is a `Vec` walked
/// only when something happens on it. A collision costs a waiter one extra look
/// at its own word
const BUCKETS: usize = 64;
const BUCKET_MASK: usize = BUCKETS - 1;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    /// The address space, or `SHARED` for a key on a frame
    cr3: u64,
    /// The virtual address, or for `SHARED` the physical one
    addr: u64,
}

/// The `cr3` of a key on a physical frame: no page table is ever at the top
/// of the address space
const SHARED: u64 = u64::MAX;

impl Key {
    /// The key for the word at `addr` in `cr3`: on the frame when the page is
    /// one several address spaces share, on the address otherwise
    fn of(cr3: u64, addr: u64) -> Key {
        use crate::mm::vmm::{PTE_ADDR_MASK, PTE_PRESENT, PTE_SHARED};
        match crate::mm::vmm::read_pte_raw(cr3, addr & !0xFFF) {
            Some(pte) if pte & PTE_PRESENT != 0 && pte & PTE_SHARED != 0 => {
                Key { cr3: SHARED, addr: (pte & PTE_ADDR_MASK) | (addr & 0xFFF) }
            }
            _ => Key { cr3, addr },
        }
    }
}

struct Waiter {
    key: Key,
    pid: u64,
    /// Whose address space the waiter sleeps in, which a key on a frame does
    /// not say (see `forget_address_space`)
    owner: u64,
}

struct Bucket(Mutex<Vec<Waiter>>);

impl Bucket {
    const fn new() -> Self { Bucket(Mutex::new(Vec::new())) }
}

static BUCKETS_TABLE: [Bucket; BUCKETS] = [const { Bucket::new() }; BUCKETS];

#[inline]
fn bucket_of(key: &Key) -> &'static Bucket {
    // The address varies; the low four bits do not (a futex word is at least
    // 4-byte aligned and in practice 8), so they are shifted out before mixing.
    // The cr3 separates address spaces using the same addresses
    let mut h = (key.addr >> 4) ^ (key.cr3 >> 12);
    h ^= h >> 17;
    &BUCKETS_TABLE[(h as usize) & BUCKET_MASK]
}

/// Join the queue for `addr`. Call with the word already checked.
fn enqueue(key: Key, pid: u64, owner: u64) {
    bucket_of(&key).0.lock().push(Waiter { key, pid, owner });
}

/// Leave it again, whichever way the sleep ended.
fn dequeue(key: Key, pid: u64) {
    let mut b = bucket_of(&key).0.lock();
    if let Some(i) = b.iter().position(|w| w.pid == pid && w.key == key) {
        b.swap_remove(i);
    }
}

/// Wakes at most `count` waiters on one word. Returns how many were woken.
///
/// The pids are collected under the bucket lock and woken after it is dropped:
/// a woken thread must not land on a lock the waker holds
pub fn wake(cr3: u64, addr: u64, count: u64) -> u64 {
    let key = Key::of(cr3, addr);
    let pending: Vec<u64> = {
        let mut b = bucket_of(&key).0.lock();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() && (out.len() as u64) < count {
            if b[i].key == key {
                out.push(b.swap_remove(i).pid);
            } else {
                i += 1;
            }
        }
        out
    };
    let n = pending.len() as u64;
    for pid in pending {
        super::wakeup(pid);
    }
    n
}

/// Why a wait ended.
pub enum WaitResult {
    /// Woken, or woken for no reason at all. The caller re-checks its word.
    Woken,
    /// The deadline passed.
    TimedOut,
}

/// Sleeps on `addr` for at most `timeout_ns`, or indefinitely when it is
/// `None`.
///
/// `still_waiting` is read after this task has joined the queue and decides
/// whether to sleep at all. A closure rather than a value compared by the
/// caller beforehand: between the caller's check and the `enqueue` below a
/// waker could change the word, find an empty queue and wake nobody, and the
/// task would sleep out the backstop.
///
/// The closure touches user memory: the pointer belongs to the syscall boundary
/// that validated it. It runs with the sleep requested, so it must not sleep
/// itself: a word on a present page is a PTE read and a load, and one that has
/// to be faulted in costs a spurious wake
pub fn wait(
    cr3: u64,
    addr: u64,
    timeout_ns: Option<u64>,
    still_waiting: impl Fn() -> bool,
) -> WaitResult {
    let key = Key::of(cr3, addr);
    let me = super::current_pid();
    let started = crate::kcore::time::now_ns();
    let deadline = timeout_ns.map(|t| started.saturating_add(t));

    enqueue(key, me, cr3);
    // Asked with the sleep already requested (`sleep_until_unless`). Asked
    // before it, a waker changing the word in between finds this task queued,
    // running and asking nothing, and its wake does nothing, so the sleep that
    // follows lasts the whole backstop
    let backstop = started.saturating_add(BACKSTOP_NS);
    let slept = super::sleep_until_unless(deadline.map_or(backstop, |d| d.min(backstop)), || !still_waiting());
    dequeue(key, me);
    if !slept {
        return WaitResult::Woken;
    }

    if deadline.is_some_and(|d| crate::kcore::time::now_ns() >= d) {
        return WaitResult::TimedOut;
    }
    WaitResult::Woken
}

/// Drops every waiter belonging to a dead address space.
///
/// A task killed while queued would otherwise leave its pid in a bucket, and
/// `wake` would hand that pid to `wakeup`, which is wrong once pids are reused
pub fn forget_address_space(cr3: u64) {
    for b in BUCKETS_TABLE.iter() {
        let mut w = b.0.lock();
        w.retain(|x| x.owner != cr3);
    }
}
