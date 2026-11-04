// Kernel stacks with a guard page (`dev/kernel-core.md`, "Kernel stacks").
//
// A plain 'vec![0u8; 512 KiB]' on the kernel heap separates nothing from the
// next allocation, and with the kernel image mapped by a single 1 GiB huge page
// there is no fault: a thread running off the bottom of its stack keeps writing
// into whatever the allocator handed out below it.
//
// A stack comes from the mapped area (`mm::vmap`): single frames mapped side by
// side, as Linux's `CONFIG_VMAP_STACK` does. The page below every object there
// is never mapped (the guard at the end of the span before it, or free space),
// so an overflow faults at the instruction that did it, with the address in
// CR2.
//
// Before the mapped area exists (the boot stacks), a stack is `pages + 1`
// contiguous frames with the lowest unmapped from the HHDM. A stack of that
// kind given back keeps its guard unmapped and waits in `SPARE` for the next
// one of its size: unmapping a guard is a shootdown of every processor
// (`vmm::hhdm_unmap_guard_frame`), 43 us per thread in QEMU, and on hardware it
// wakes every sleeping core. kswapd gives the spares back when memory runs low.
//
// Sizes. A program's task runs only its syscalls on its kernel stack, and has
// 128 KiB (`program_stack_size`); `bigstacks` on the boot line makes that 512
// KiB. A kernel thread has 512 KiB (`KERNEL_THREAD_STACK`): there are a few
// dozen, and they run the deepest paths. The kernel's frames are large
// (`dev/kernel-core.md`, "Kernel stacks").
//
// A new stack is painted (`PAINT`), so how deep it was used can be read off it
// when it goes: always in its last `RED_ZONE` bytes, logged, and over the whole
// stack with `stackdepth` on the boot line, which `/proc/meminfo` reports as
// `KernelStackMax`.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::arch::x86_64::grub;
use crate::kcore::irq_lock::IrqMutex;
use crate::mm::{pmm, vmm};

const PAGE_SIZE: usize = 4096;

/// Stacks kept for reuse, at most: 16 MiB of the default 512 KiB ones
const SPARE_MAX: usize = 32;

/// A whole allocation, guard frame first and already unmapped
struct Spare {
    base_phys: u64,
    frames: usize,
}

static SPARE: IrqMutex<Vec<Spare>> = IrqMutex::new(Vec::new());

/// A spare of exactly `frames` frames
fn take_spare(frames: usize) -> Option<u64> {
    let mut spare = SPARE.lock();
    let i = spare.iter().position(|s| s.frames == frames)?;
    Some(spare.swap_remove(i).base_phys)
}

/// Keeps `base_phys` for the next stack of its size, unless there are enough or
/// memory is short. Returns whether it was kept
fn give_spare(base_phys: u64, frames: usize) -> bool {
    if pmm::running_low() {
        return false;
    }
    let mut spare = SPARE.lock();
    if spare.len() >= SPARE_MAX {
        return false;
    }
    if spare.capacity() == 0 {
        spare.reserve(SPARE_MAX);
    }
    spare.push(Spare { base_phys, frames });
    true
}

/// The spare stacks as a shrinker (`mm::shrink`): every spare back to the frame
/// allocator, its guard mapped again first. Only from kswapd: a guard's mapping
/// goes through the direct map's lock, which an allocation out of frames may be
/// under
pub static SHRINKER: crate::mm::shrink::Shrinker = crate::mm::shrink::Shrinker {
    name: "spare kernel stacks",
    count: || SPARE.try_lock().map_or(0, |s| s.iter().map(|x| x.frames).sum()),
    scan: |_, direct| {
        if direct {
            return 0;
        }
        let gone: Vec<Spare> = core::mem::take(&mut *SPARE.lock());
        let mut frames = 0;
        for s in gone {
            vmm::hhdm_remap_guard_frame(s.base_phys);
            pmm::free_frames(s.base_phys, s.frames);
            frames += s.frames;
        }
        frames
    },
};

/// A kernel thread's stack, in bytes
pub const KERNEL_THREAD_STACK: usize = 512 * 1024;

/// The kernel stack of a program's task, in bytes: 128 KiB, or 512 KiB with
/// `bigstacks`
pub fn program_stack_size() -> usize {
    if flag(&BIG, "bigstacks") { KERNEL_THREAD_STACK } else { 128 * 1024 }
}

/// A boot-line flag, read once
fn flag(cache: &AtomicU8, name: &str) -> bool {
    match cache.load(Ordering::Relaxed) {
        0 => {
            let on = grub::cmdline_flag(name);
            cache.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        v => v == 2,
    }
}

static BIG: AtomicU8 = AtomicU8::new(0);
static DEPTH: AtomicU8 = AtomicU8::new(0);

/// What a new stack is filled with, so that how deep it was used can be read
/// back off it. Not zero: a frame that happens to hold zeros would read as
/// never touched
const PAINT: u64 = 0x5354_4b5f_4d49_4b55;

/// The lowest bytes of a stack: a task reaching into them is logged when it
/// goes, `stackdepth` or not
const RED_ZONE: usize = 4096;

/// Bytes of kernel stack held by live tasks, for `/proc/meminfo`
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
/// The deepest any task that has gone used its stack, in bytes
static DEEPEST: AtomicUsize = AtomicUsize::new(0);

enum Backing {
    /// From the mapped area: `pages` frames, side by side
    Mapped { pages: usize },
    /// One run of frames in the direct map, guard frame first
    Direct { base_phys: u64, frames: usize, guarded: bool },
}

pub struct KernelStack {
    /// The lowest usable byte; the page below it is the guard
    stack_virt: u64,
    stack_bytes: usize,
    back: Backing,
}

impl KernelStack {
    /// A painted stack of at least 'size' bytes, fenced below by an unmapped
    /// guard page. Returns None when physical memory is exhausted
    pub fn new(size: usize) -> Option<Self> {
        let stack_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let stack_bytes = stack_pages * PAGE_SIZE;
        let s = if crate::mm::vmap::ready() {
            match crate::mm::vmap::alloc(stack_pages) {
                p if p.is_null() => return None,
                p => Self { stack_virt: p as u64, stack_bytes, back: Backing::Mapped { pages: stack_pages } },
            }
        } else {
            Self::direct(stack_pages)?
        };
        // Painted like a new one, so what a stack holds never depends on whose
        // it was
        unsafe {
            core::slice::from_raw_parts_mut(s.stack_virt as *mut u64, stack_bytes / 8).fill(PAINT);
        }
        LIVE_BYTES.fetch_add(stack_bytes, Ordering::Relaxed);
        Some(s)
    }

    /// `pages` in one run of the direct map, with a guard frame below
    fn direct(stack_pages: usize) -> Option<Self> {
        let frames = stack_pages + 1; // +1 guard
        let stack_bytes = stack_pages * PAGE_SIZE;
        if let Some(base_phys) = take_spare(frames) {
            let stack_virt = grub::phys_to_virt(base_phys + PAGE_SIZE as u64);
            return Some(Self { stack_virt, stack_bytes, back: Backing::Direct { base_phys, frames, guarded: true } });
        }
        let base_phys = pmm::alloc_frames(frames)?;
        let stack_virt = grub::phys_to_virt(base_phys + PAGE_SIZE as u64);

        // Losing the guard is not fatal: the stack still works without one, so
        // a failure here degrades instead of refusing to spawn
        let guarded = vmm::hhdm_unmap_guard_frame(base_phys);
        if !guarded {
            crate::serial_println!(
                "[kstack] warn: could not unmap guard frame {:#x}; stack is unguarded",
                base_phys
            );
        }
        Some(Self { stack_virt, stack_bytes, back: Backing::Direct { base_phys, frames, guarded } })
    }

    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.stack_virt as *const u8
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.stack_bytes
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.stack_bytes == 0
    }

    /// [lowest, highest) addresses this stack occupies. The backtrace walk
    /// refuses any frame pointer outside them, so a smashed RBP ends the
    /// walk instead of sending it through unrelated memory
    #[inline]
    pub fn bounds(&self) -> (u64, u64) {
        (self.stack_virt, self.stack_virt + self.stack_bytes as u64)
    }

    /// Lowest address a healthy stack may reach. One page below this is the
    /// guard; the page-fault handler uses it to name an overflow instead of
    /// reporting an anonymous bad access
    #[inline]
    pub fn guard_virt(&self) -> u64 {
        self.stack_virt - PAGE_SIZE as u64
    }

    /// The words of the stack, lowest first
    fn words(&self) -> &[u64] {
        // SAFETY: mapped for as long as `self` is
        unsafe { core::slice::from_raw_parts(self.stack_virt as *const u64, self.stack_bytes / 8) }
    }

    /// Bytes of it ever used: from the top down to the lowest word that is
    /// not the paint
    pub fn used(&self) -> usize {
        self.stack_bytes - self.words().iter().take_while(|&&w| w == PAINT).count() * 8
    }

    /// Whether anything reached into its last `RED_ZONE` bytes
    fn touched_red_zone(&self) -> bool {
        self.words()[..RED_ZONE.min(self.stack_bytes) / 8].iter().any(|&w| w != PAINT)
    }
}

/// Task `pid`'s stack is about to go: how deep it went, counted, and logged
/// when it came near the end (`stackdepth`: the whole stack read, and every new
/// deepest logged)
pub fn account(stack: &KernelStack, pid: u64, name: &crate::process::Name) {
    let all = flag(&DEPTH, "stackdepth");
    if !all && !stack.touched_red_zone() {
        return;
    }
    let used = stack.used();
    let was = DEEPEST.fetch_max(used, Ordering::Relaxed);
    if used + RED_ZONE > stack.len() {
        crate::kwarn!("[kstack] pid {} ({}) used {} of its {} bytes of kernel stack", pid, name, used, stack.len());
    } else if all && used > was {
        crate::kinfo!("[kstack] deepest so far: {} bytes, pid {} ({})", used, pid, name);
    }
}

/// Kernel stack held now and the deepest any task used, in bytes, for
/// `/proc/meminfo`. With `stackdepth` the live tasks' stacks are read too
pub fn stats() -> (usize, usize) {
    let mut deepest = DEEPEST.load(Ordering::Relaxed);
    if flag(&DEPTH, "stackdepth") {
        crate::sched::for_each_task(|p| deepest = deepest.max(p.stack.used()));
    }
    (LIVE_BYTES.load(Ordering::Relaxed), deepest)
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        LIVE_BYTES.fetch_sub(self.stack_bytes, Ordering::Relaxed);
        match self.back {
            Backing::Mapped { pages } => crate::mm::vmap::free(self.stack_virt as *mut u8, pages),
            Backing::Direct { base_phys, frames, guarded } => {
                // Puts the HHDM mapping back before the frame returns to the
                // pool, or the next owner faults on its first touch through the
                // direct map, unless it waits for the next stack, guard and all
                if guarded {
                    if give_spare(base_phys, frames) {
                        return;
                    }
                    vmm::hhdm_remap_guard_frame(base_phys);
                }
                pmm::free_frames(base_phys, frames);
            }
        }
    }
}

unsafe impl Send for KernelStack {}
unsafe impl Sync for KernelStack {}

/// If 'addr' lands on the guard page of a live kernel stack, name its owner.
/// Only called from the page-fault handler; the walk of the live tasks takes
/// no lock (`sched::find_task`)
pub fn guard_fault_owner(addr: u64) -> Option<(u64, crate::process::Name)> {
    let page = addr & !(PAGE_SIZE as u64 - 1);
    let ptr = crate::sched::find_task(|p| p.stack.guard_virt() == page)?;
    let p = unsafe { &*ptr };
    Some((p.pid, p.name))
}
