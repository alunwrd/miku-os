// Per-CPU storage: run queue, current task, scheduler state, TSS
//
// Each CPU has its own 'Cpu' struct. Access is via GS base: the first field
// ('cpu_index') sits at 'gs:[0]' so any CPU can identify itself cheaply
//
// Memory model: all fields are atomic or protected by a per-CPU spinlock.
// A CPU may always touch its own data with interrupts disabled (no lock
// needed), but remote wakeups/pushes MUST take the target's run queue lock

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::sched::slot_heap::SlotHeap;

pub const MAX_CPUS: usize = 64;

// run queue primitive //

/// The tasks ready to run on one CPU, least vruntime first: a heap whose
/// slot in each task is `Process::rq_slot` (`sched::runqueue`)
pub struct RunQueueInner {
    pub heap: SlotHeap,
}

impl RunQueueInner {
    #[inline]
    pub fn len(&self) -> usize {
        self.heap.len()
    }
}

pub struct RunQueue {
    lock:  AtomicBool,
    inner: UnsafeCell<RunQueueInner>,
    /// The heap's length, stored under the lock each time it is let go: what
    /// an idle CPU polls and a wake compares CPUs by, without the lock
    queued: AtomicUsize,
}

unsafe impl Sync for RunQueue {}

impl RunQueue {
    pub const fn new() -> Self {
        Self {
            lock:  AtomicBool::new(false),
            inner: UnsafeCell::new(RunQueueInner { heap: SlotHeap::new(|p| &p.rq_slot) }),
            queued: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn acquire(&self) {
        while self.lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.lock.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    #[inline]
    fn release(&self) {
        self.queued.store(unsafe { (*self.inner.get()).len() }, Ordering::Relaxed);
        self.lock.store(false, Ordering::Release);
    }

    /// The timer ISR walks this queue (schedule_from_isr), so the lock is never
    /// held with interrupts on: the tick would land on the same CPU and spin on
    /// a lock only the interrupted code can release. Disabling here makes that
    /// a property of the lock (nesting is free: IF is restored to what it was)
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut RunQueueInner) -> R,
    {
        x86_64::instructions::interrupts::without_interrupts(|| {
            self.acquire();
            let r = f(unsafe { &mut *self.inner.get() });
            self.release();
            r
        })
    }

    /// Best-effort try-lock. Returns None if contended
    pub fn try_with<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut RunQueueInner) -> R,
    {
        x86_64::instructions::interrupts::without_interrupts(|| {
            if self.lock
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                return None;
            }
            let r = f(unsafe { &mut *self.inner.get() });
            self.release();
            Some(r)
        })
    }

    pub fn len(&self) -> usize {
        self.with(|inner| inner.len())
    }

    /// The length as the last holder left it, without the lock: a hint, or
    /// exact after a full fence that follows the store (`sched::idle`)
    #[inline]
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }
}

// Cpu struct //

#[repr(C, align(128))]
pub struct Cpu {
    /// Logical CPU index (0..MAX_CPUS). Kept at offset 0 for gs:[0] lookup
    pub cpu_index:      u32,            // +0x00
    pub lapic_id:       AtomicU32,      // +0x04
    pub online:         AtomicU32,      // +0x08
    pub _pad0:          u32,            // +0x0C

    /// Kernel stack top used by syscall entry. Updated each context switch
    pub kernel_rsp:     AtomicU64,      // +0x10
    pub user_rsp:       AtomicU64,      // +0x18

    pub current_pid:    AtomicU64,      // +0x20
    pub idle_pid:       AtomicU64,      // +0x28

    pub ticks:          AtomicU64,
    pub total_switches: AtomicU64,
    pub min_vruntime:   AtomicU64,

    pub run_queue:      RunQueue,

    /// Pointer to per-CPU TaskStateSegment (set during gdt::init_cpu)
    pub tss_ptr:        AtomicU64,
    /// Virtual address of double-fault stack top
    pub df_stack:       AtomicU64,
    /// Virtual address of page-fault stack top
    pub pf_stack:       AtomicU64,
    /// Virtual address of syscall-entry stack top
    pub sysc_stack:     AtomicU64,

    /// The terminal the running task's output goes to, as a raw `TtyId`. Zero,
    /// the common case, means the console.
    ///
    /// Here since `print!` reads it, at every print site in the kernel, some
    /// inside interrupt handlers. Swapped by the context switch beside
    /// `current_pid`
    pub current_tty:    AtomicU32,

    /// The address space in CR3 here, or 0 before the first switch writes one.
    /// Stored (SeqCst) just before every `mov cr3`, which makes
    /// `mm::tlb::cr3_loaded_elsewhere` exact: a processor reading anything else
    /// here walks the page tables only after it has been told
    pub loaded_cr3:     AtomicU64,

    /// Idle and watching its run queue rather than halted: a wake for this
    /// CPU needs no IPI (`sched::idle`)
    pub polling:        AtomicBool,
    /// How long idle watches before it halts, adapted to how soon this CPU
    /// has been woken (`sched::idle`)
    pub poll_ns:        AtomicU64,
    /// When this CPU last switched from idle to a task, on `now_ns`'s clock
    pub idle_left_ns:   AtomicU64,
}

impl Cpu {
    pub const fn new(idx: u32) -> Self {
        Self {
            cpu_index:      idx,
            lapic_id:       AtomicU32::new(0),
            online:         AtomicU32::new(0),
            _pad0:          0,
            kernel_rsp:     AtomicU64::new(0),
            user_rsp:       AtomicU64::new(0),
            current_pid:    AtomicU64::new(0),
            idle_pid:       AtomicU64::new(0),
            ticks:          AtomicU64::new(0),
            total_switches: AtomicU64::new(0),
            min_vruntime:   AtomicU64::new(0),
            run_queue:      RunQueue::new(),
            tss_ptr:        AtomicU64::new(0),
            df_stack:       AtomicU64::new(0),
            pf_stack:       AtomicU64::new(0),
            sysc_stack:     AtomicU64::new(0),
            current_tty:    AtomicU32::new(0),
            loaded_cr3:     AtomicU64::new(0),
            polling:        AtomicBool::new(false),
            poll_ns:        AtomicU64::new(0),
            idle_left_ns:   AtomicU64::new(0),
        }
    }
}

// Compile-time check that the field offsets match what the assembly expects.
// syscall_handler (syscall/mod.rs) and sys_fork (syscall/process.rs) hardcode
// gs:[0x00], gs:[0x10] and gs:[0x18]. A field reorder breaks this assertion
// at build time instead of silently corrupting forked processes at runtime
const _: () = {
    assert!(core::mem::offset_of!(Cpu, cpu_index)  == 0x00);
    assert!(core::mem::offset_of!(Cpu, kernel_rsp) == 0x10);
    assert!(core::mem::offset_of!(Cpu, user_rsp)   == 0x18);
};

#[repr(transparent)]
struct CpuArray([Cpu; MAX_CPUS]);
unsafe impl Sync for CpuArray {}

static CPUS: CpuArray = CpuArray({
    const INIT: Cpu = Cpu::new(0);
    [INIT; MAX_CPUS]
});

static CPU_COUNT:    AtomicUsize = AtomicUsize::new(0);
static ONLINE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Must be called once from BSP before starting APs. Sets each CPU's
/// cpu_index and returns a mutable view of CPU 0 for BSP init
pub fn init_array() {
    for i in 0..MAX_CPUS {
        unsafe {
            let c = &CPUS.0[i] as *const Cpu as *mut Cpu;
            (*c).cpu_index = i as u32;
        }
    }
}

#[inline(always)]
pub fn get(idx: usize) -> &'static Cpu {
    &CPUS.0[idx.min(MAX_CPUS - 1)]
}

#[inline(always)]
pub fn current_index() -> usize {
    // Returns the cpu_index stored at gs:[0]. Valid only after install_gs_base
    let idx: u32;
    unsafe {
        core::arch::asm!(
            "mov {:e}, gs:[0]",
            out(reg) idx,
            options(nostack, nomem, preserves_flags)
        );
    }
    idx as usize
}

#[inline(always)]
pub fn current() -> &'static Cpu {
    get(current_index())
}

/// Install GSBASE/KernelGSBASE for 'cpu_index' on the current CPU
pub unsafe fn install_gs_base(cpu_index: usize) {
    use x86_64::registers::model_specific::{GsBase, KernelGsBase};
    let addr = &CPUS.0[cpu_index] as *const Cpu as u64;
    GsBase::write(x86_64::VirtAddr::new(addr));
    KernelGsBase::write(x86_64::VirtAddr::new(addr));
}

pub fn mark_online(cpu_index: usize, lapic_id: u32) {
    let c = get(cpu_index);
    c.lapic_id.store(lapic_id, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::SeqCst);
    if c.online.swap(1, Ordering::SeqCst) == 0 {
        ONLINE_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn register(cpu_index: usize) {
    CPU_COUNT.fetch_max(cpu_index + 1, Ordering::SeqCst);
}

pub fn cpu_count() -> usize { CPU_COUNT.load(Ordering::Relaxed) }
pub fn online_cpus() -> usize { ONLINE_COUNT.load(Ordering::Relaxed) }

/// Iterate online CPUs ('yields' &'static Cpu')
pub fn iter_online() -> impl Iterator<Item = &'static Cpu> {
    let total = cpu_count().min(MAX_CPUS);
    (0..total).filter_map(|i| {
        let c = get(i);
        if c.online.load(Ordering::Relaxed) != 0 { Some(c) } else { None }
    })
}

/// The CPU a woken task should go to: the fewest queued, and of those one that
/// is idle.
///
/// Queue length alone treats a CPU busy with one long task as free as an idle
/// one, and ring 0 is not preempted, so a task woken onto a CPU in the middle
/// of the compositor's frame waits for the frame to end.
/// Respects 'mask' (bit N = CPU N eligible). Returns '0' as fallback
pub fn pick_lightest(mask: u64) -> usize {
    let total = cpu_count().min(MAX_CPUS);
    let mut best_idx = 0usize;
    let mut best_load = usize::MAX;
    for i in 0..total {
        if mask & (1u64 << i) == 0 { continue; }
        let c = get(i);
        if c.online.load(Ordering::Relaxed) == 0 { continue; }
        let len = c.run_queue.queued();
        let busy = c.current_pid.load(Ordering::Relaxed) != c.idle_pid.load(Ordering::Relaxed);
        let load = len * 2 + busy as usize;
        if load < best_load {
            best_load = load;
            best_idx = i;
        }
    }
    best_idx
}
