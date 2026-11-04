// A min-heap of tasks whose slots live in the tasks (`dev/kernel-core.md`, "The
// run queue")
//
// Two of the scheduler's structures are this: every CPU's sleepers, by when
// they wake (`sched::timer`), and every CPU's run queue, by vruntime
// (`sched::runqueue`). A task's index in the array is kept in a field of the
// task (which field is the heap's `slot`), so taking a task out from the
// middle, a wake or a kill, is O(log n).
//
// The key is copied into the entry when the task goes in and never read from
// the task again: vruntime is written by more than one path.
//
// No lock here: each user keeps its heaps behind its own lock. The array grows
// by doubling from `START`, under that lock, which is safe because the kernel
// heap is an `IrqMutex` and neither it nor the frame allocator calls back into
// the scheduler.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::process::Process;

/// The CPU field of a task in no heap of that kind (`Process::timer_cpu`,
/// `Process::rq_cpu`)
pub const NO_CPU: u32 = u32::MAX;

/// Slots a heap starts with, so a scheduler path does not allocate in the
/// common case
const START: usize = 256;

struct Entry<T> {
    key: u64,
    /// Second key, for equal `key`s: the run queue's pid
    tie: u64,
    task: *mut T,
}

impl<T> Clone for Entry<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Entry<T> {}

impl<T> Entry<T> {
    #[inline]
    fn before(&self, other: &Entry<T>) -> bool {
        (self.key, self.tie) < (other.key, other.tie)
    }
}

/// Generic over what it holds only so that `sched::selftest` can fill one
/// with something smaller than a task
pub struct SlotHeap<T = Process> {
    items: Vec<Entry<T>>,
    /// Where in a task this heap keeps the task's index
    slot: fn(&T) -> &AtomicU32,
}

unsafe impl<T> Send for SlotHeap<T> {}

impl<T> SlotHeap<T> {
    pub const fn new(slot: fn(&T) -> &AtomicU32) -> Self {
        SlotHeap { items: Vec::new(), slot }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// The smallest entry's task and key
    #[inline]
    pub fn first(&self) -> Option<(*mut T, u64)> {
        self.items.first().map(|e| (e.task, e.key))
    }

    /// Whether slot `i` holds `p`, for a caller that read `p`'s slot before
    /// taking the lock this heap is behind
    #[inline]
    pub fn holds(&self, i: usize, p: *mut T) -> bool {
        self.items.get(i).is_some_and(|e| e.task == p)
    }

    #[inline]
    unsafe fn place(&mut self, i: usize, e: Entry<T>) {
        self.items[i] = e;
        (self.slot)(unsafe { &*e.task }).store(i as u32, Ordering::Relaxed);
    }

    unsafe fn up(&mut self, mut i: usize) {
        let e = self.items[i];
        while i > 0 {
            let parent = (i - 1) / 2;
            let q = self.items[parent];
            if !e.before(&q) { break; }
            unsafe { self.place(i, q) };
            i = parent;
        }
        unsafe { self.place(i, e) };
    }

    unsafe fn down(&mut self, mut i: usize) {
        let n = self.items.len();
        let e = self.items[i];
        loop {
            let l = 2 * i + 1;
            if l >= n { break; }
            let r = l + 1;
            let c = if r < n && self.items[r].before(&self.items[l]) { r } else { l };
            if !self.items[c].before(&e) { break; }
            unsafe { self.place(i, self.items[c]) };
            i = c;
        }
        unsafe { self.place(i, e) };
    }

    /// `p` in at `(key, tie)`. The caller knows it is in no heap that uses
    /// the same slot field
    ///
    /// # Safety
    /// `p` is live for as long as it is in the heap
    pub unsafe fn push(&mut self, p: *mut T, key: u64, tie: u64) {
        if self.items.capacity() == 0 {
            self.items.reserve(START);
        }
        self.items.push(Entry { key, tie, task: p });
        unsafe { self.up(self.items.len() - 1) };
    }

    /// The task in slot `i`, out
    ///
    /// # Safety
    /// `i` is a slot of this heap
    pub unsafe fn remove(&mut self, i: usize) -> *mut T {
        let gone = self.items[i].task;
        let last = self.items.pop().expect("slot heap: removing from an empty heap");
        if i < self.items.len() {
            unsafe {
                self.place(i, last);
                self.down(i);
                self.up(i);
            }
        }
        gone
    }

    /// The smallest entry's task, out
    pub fn pop(&mut self) -> Option<*mut T> {
        if self.items.is_empty() { return None; }
        Some(unsafe { self.remove(0) })
    }

    /// Every entry dropped, for a scheduler starting over: the tasks are
    /// being thrown away with the table
    pub fn clear(&mut self) {
        self.items.clear();
    }
}
