// Per-CPU storage: run queue, current task, scheduler state, TSS
//
// Each CPU has its own 'Cpu' struct. Access is via GS base: the first field
// ('cpu_index') sits at 'gs:[0]' so any CPU can identify itself cheaply
//
// // Memory model
//
// All fields are atomic or protected by a per-CPU spinlock. A CPU is always
// allowed to touch its own data with interrupts disabled (no lock needed),  but remote wakeups/pushes MUST take the target's run queue lock

use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::process::Process;

pub const MAX_CPUS: usize = 64;

// run queue primitive //

#[repr(C)]
pub struct RunQueueInner {
    pub head: *mut Process,
    pub len:  usize,
}

unsafe impl Send for RunQueueInner {}

pub struct RunQueue {
    lock:  AtomicBool,
    inner: UnsafeCell<RunQueueInner>,
}

unsafe impl Sync for RunQueue {}

impl RunQueue {
    pub const fn new() -> Self {
        Self {
            lock:  AtomicBool::new(false),
            inner: UnsafeCell::new(RunQueueInner { head: null_mut(), len: 0 }),
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
        self.lock.store(false, Ordering::Release);
    }

    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut RunQueueInner) -> R,
    {
        self.acquire();
        let r = f(unsafe { &mut *self.inner.get() });
        self.release();
        r
    }

    /// Best-effort try-lock. Returns None if contended
    pub fn try_with<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut RunQueueInner) -> R,
    {
        if self.lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        let r = f(unsafe { &mut *self.inner.get() });
        self.release();
        Some(r)
    }

    pub fn len(&self) -> usize {
        self.with(|inner| inner.len)
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
        }
    }
}

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

/// Pick the CPU with shortest run queue for load balancing.
/// Respects 'mask' (bit N = CPU N eligible). Returns '0' as fallback
pub fn pick_lightest(mask: u64) -> usize {
    let total = cpu_count().min(MAX_CPUS);
    let mut best_idx = 0usize;
    let mut best_len = usize::MAX;
    for i in 0..total {
        if mask & (1u64 << i) == 0 { continue; }
        let c = get(i);
        if c.online.load(Ordering::Relaxed) == 0 { continue; }
        let len = c.run_queue.with(|q| q.len);
        if len < best_len {
            best_len = len;
            best_idx = i;
        }
    }
    best_idx
}
