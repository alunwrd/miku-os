// Ring 3's heap: `heap_core`'s allocator over `mmap` behind one lock, with the
// C names every program allocates through, and a cache per thread in front of
// the lock (`dev/std-architecture.md`, "The heap's thread caches").
//
// The lock sleeps (`sync::Mutex`). A block a thread frees goes on its own list
// for that size, and the next allocation of the size takes it lock-free, in the
// shape of glibc's tcache. Cached blocks stay "used" to `heap_core`, and the
// list runs through the blocks themselves.

use crate::heap_core::{Heap, Source};
use crate::sync::Mutex;

const SLAB_SIZE: usize = 131072;
const LARGE_THRESHOLD: usize = 32768;
const PROT_RW: u64 = (miku_abi::mman::PROT_READ | miku_abi::mman::PROT_WRITE) as u64;

/// Fresh pages from the kernel, zeroed, wherever it places them
struct Mmap;

impl Source for Mmap {
    unsafe fn grab(&mut self, bytes: usize) -> *mut u8 {
        crate::proc::miku_mmap(0, bytes, PROT_RW)
    }
}

static HEAP: Mutex<Heap<Mmap>> = Mutex::new(Heap::new(Mmap, SLAB_SIZE, LARGE_THRESHOLD));

/// Held across `fork` (`proc::miku_fork`), so the child's heap copy is
/// consistent and its lock free. The forking thread's cache goes with it; other
/// threads' caches are memory the child never uses, as in glibc
pub fn fork_lock() {
    HEAP.lock_raw();
}

/// # Safety
/// After `fork_lock`, once in each process
pub unsafe fn fork_unlock() {
    HEAP.unlock_raw();
}

// classes

/// Sizes a thread cache keeps: 16-byte steps to 1 KiB, then four per power of
/// two to 16 KiB
const SMALL: usize = 1024;
const SMALL_CLASSES: usize = SMALL / 16;
const BIGGEST: usize = 16384;
pub(crate) const CLASSES: usize = SMALL_CLASSES + 4 * 4;

/// The class for a request of `size` bytes and that class's size, so a block
/// fits its class on return
pub(crate) fn class_up(size: usize) -> Option<(usize, usize)> {
    if size == 0 || size > BIGGEST {
        return None;
    }
    if size <= SMALL {
        let c = (size + 15) / 16 - 1;
        return Some((c, (c + 1) * 16));
    }
    let log = (usize::BITS - 1 - (size - 1).leading_zeros()) as usize;
    let base = 1usize << log;
    let step = base / 4;
    let n = (size - base + step - 1) / step;
    Some((SMALL_CLASSES + (log - 10) * 4 + (n - 1), base + n * step))
}

/// The largest class a block holding `usable` bytes can serve
pub(crate) fn class_down(usable: usize) -> Option<usize> {
    if usable < 16 {
        return None;
    }
    if usable >= BIGGEST {
        return Some(CLASSES - 1);
    }
    if usable < SMALL + SMALL / 4 {
        return Some((usable / 16).min(SMALL_CLASSES) - 1);
    }
    let log = (usize::BITS - 1 - usable.leading_zeros()) as usize;
    let base = 1usize << log;
    let n = (usable - base) / (base / 4);
    Some(match n {
        // `base` itself: the last class of the octave below
        0 => SMALL_CLASSES + (log - 11) * 4 + 3,
        n => SMALL_CLASSES + (log - 10) * 4 + (n - 1),
    })
}

pub(crate) fn class_size(c: usize) -> usize {
    if c < SMALL_CLASSES {
        return (c + 1) * 16;
    }
    let (k, j) = ((c - SMALL_CLASSES) / 4, (c - SMALL_CLASSES) % 4);
    let base = SMALL << k;
    base + (j + 1) * (base / 4)
}

/// Blocks kept per class: many small, few big
fn class_cap(c: usize) -> u16 {
    match class_size(c) {
        s if s <= 1024 => 32,
        s if s <= 4096 => 16,
        _ => 8,
    }
}

/// What one thread's cache may hold, all classes together
const CACHE_BYTES: usize = 256 * 1024;

// caches

/// One thread's cache. A cached block holds the next one's address in its first
/// word and the cache's in its second, which detects a double free
#[repr(C)]
struct ThreadCache {
    heads: [*mut u8; CLASSES],
    counts: [u16; CLASSES],
    bytes: usize,
}

/// The thread's cache: the TCB word at `tp + 56`, left zero by every TCB
/// builder (`docs/MikuOS_ABI.md` 3.2d). `+ 40` is errno
#[cfg(not(test))]
fn slot() -> *mut *mut ThreadCache {
    let tp: usize;
    unsafe { core::arch::asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly, preserves_flags)) };
    if tp == 0 { core::ptr::null_mut() } else { (tp + miku_abi::tcb::TCB_HEAP_CACHE) as *mut *mut ThreadCache }
}

#[cfg(test)]
std::thread_local! {
    static SLOT: core::cell::Cell<*mut ThreadCache> = const { core::cell::Cell::new(core::ptr::null_mut()) };
}

#[cfg(test)]
fn slot() -> *mut *mut ThreadCache {
    SLOT.with(|s| s.as_ptr())
}

/// This thread's cache, made on first use; null with no TCB or no memory
fn cache(make: bool) -> *mut ThreadCache {
    let s = slot();
    if s.is_null() {
        return core::ptr::null_mut();
    }
    let tc = unsafe { *s };
    if !tc.is_null() || !make {
        return tc;
    }
    let tc = unsafe { HEAP.lock().alloc(core::mem::size_of::<ThreadCache>()) } as *mut ThreadCache;
    if !tc.is_null() {
        unsafe {
            core::ptr::write_bytes(tc as *mut u8, 0, core::mem::size_of::<ThreadCache>());
            *s = tc;
        }
    }
    tc
}

/// A block of class `c` from this thread's cache
fn take(c: usize) -> *mut u8 {
    let tc = cache(false);
    if tc.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let p = (*tc).heads[c];
        if !p.is_null() {
            (*tc).heads[c] = *(p as *const *mut u8);
            *(p as *mut usize).add(1) = 0;
            (*tc).counts[c] -= 1;
            (*tc).bytes -= class_size(c);
        }
        p
    }
}

/// Keeps `p` in this thread's cache, or false when the heap must have it
fn keep(p: *mut u8) -> bool {
    let Some(c) = (unsafe { Heap::<Mmap>::plain_usable(p) }).and_then(class_down) else { return false };
    let tc = cache(true);
    if tc.is_null() {
        return false;
    }
    unsafe {
        // Freed twice into this cache: ignored, as the heap ignores a block it
        // already has
        if *(p as *const usize).add(1) == tc as usize {
            let mut q = (*tc).heads[c];
            while !q.is_null() {
                if q == p {
                    return true;
                }
                q = *(q as *const *mut u8);
            }
        }
        if (*tc).counts[c] >= class_cap(c) || (*tc).bytes + class_size(c) > CACHE_BYTES {
            return false;
        }
        *(p as *mut *mut u8) = (*tc).heads[c];
        *(p as *mut usize).add(1) = tc as usize;
        (*tc).heads[c] = p;
        (*tc).counts[c] += 1;
        (*tc).bytes += class_size(c);
    }
    true
}

/// This thread is ending: returns its cached blocks and the cache to the heap.
/// Called from the thread trampoline and `miku_thread_exit`
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_heap_thread_done() {
    let s = slot();
    if s.is_null() {
        return;
    }
    let tc = unsafe { core::mem::replace(&mut *s, core::ptr::null_mut()) };
    if tc.is_null() {
        return;
    }
    let mut heap = HEAP.lock();
    unsafe {
        for c in 0..CLASSES {
            let mut p = (*tc).heads[c];
            while !p.is_null() {
                let next = *(p as *const *mut u8);
                heap.free(p);
                p = next;
            }
        }
        heap.free(tc as *mut u8);
    }
}

// C names

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_malloc(size: usize) -> *mut u8 {
    if let Some((c, bytes)) = class_up(size) {
        let p = take(c);
        if !p.is_null() {
            return p;
        }
        return unsafe { HEAP.lock().alloc(bytes) };
    }
    unsafe { HEAP.lock().alloc(size) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_free(ptr: *mut u8) {
    if ptr.is_null() || keep(ptr) {
        return;
    }
    // A block with a mapping of its own is unmapped with the lock down
    let large = unsafe { HEAP.lock().free(ptr) };
    if let Some(l) = large {
        crate::proc::miku_munmap(l.ptr, l.bytes);
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_realloc(ptr: *mut u8, new_size: usize) -> *mut u8 {
    if ptr.is_null() { return miku_malloc(new_size); }
    if new_size == 0 { miku_free(ptr); return core::ptr::null_mut(); }

    // Growing an over-aligned block moves it to an ordinary one with only the
    // default alignment, as C's realloc over aligned_alloc
    let old_data = unsafe { HEAP.lock().usable(ptr) };
    if old_data >= new_size { return ptr; }

    let new_ptr = miku_malloc(new_size);
    if new_ptr.is_null() { return core::ptr::null_mut(); }
    crate::mem::miku_memcpy(new_ptr, ptr, old_data);
    miku_free(ptr);
    new_ptr
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_calloc(count: usize, size: usize) -> *mut u8 {
    let total = match count.checked_mul(size) {
        Some(t) if t > 0 => t,
        _ => return core::ptr::null_mut(),
    };
    let p = miku_malloc(total);
    if !p.is_null() {
        crate::mem::miku_memset(p, 0, total);
    }
    p
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memalign(align: usize, size: usize) -> *mut u8 {
    unsafe { HEAP.lock().memalign(align, size) }
}

/// `heap_core`'s invariants over what the stand-in `mmap` gave, for the host
/// tests (`lib/runtime/libmiku/hosttest`). The caller's cache is returned first, since
/// its blocks count as used
#[cfg(test)]
pub fn check(slabs: &[(usize, usize)]) -> Result<(usize, usize), String> {
    miku_heap_thread_done();
    HEAP.lock().check(slabs)
}

/// What the caller's cache holds, for the host tests: blocks, bytes
#[cfg(test)]
pub fn cached() -> (usize, usize) {
    let tc = cache(false);
    if tc.is_null() {
        return (0, 0);
    }
    unsafe { ((*tc).counts.iter().map(|&n| n as usize).sum(), (*tc).bytes) }
}
