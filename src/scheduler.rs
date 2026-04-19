extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::process::{
    pid_range, Process, CPU_ALL,
    STATE_BLOCKED, STATE_DEAD, STATE_READY, STATE_RUNNING, STATE_SLEEPING,
};

const CPU_WINDOW_TICKS: u64   = 250;
const TICK_SCALE:       u64   = 1_000_000;
const MAX_PROCS:        usize = 4096;

static PRIO_WEIGHT: [u64; 20] = [
    88761, 71755, 56483, 46273, 36291,
    29154, 23254, 18705, 14949, 11916,
     9548,  7620,  6100,  4904,  3906,
     3121,  2501,  1991,  1586,  1277,
];

#[inline]
fn weight(priority: u8) -> u64 {
    PRIO_WEIGHT[priority.clamp(1, 20) as usize - 1]
}

struct RunQueueInner {
    head: *mut Process,
    len:  usize,
}

struct SingleCpuRunQueue(UnsafeCell<RunQueueInner>);

unsafe impl Sync for SingleCpuRunQueue {}
unsafe impl Send for SingleCpuRunQueue {}

static RUN_QUEUE: SingleCpuRunQueue = SingleCpuRunQueue(UnsafeCell::new(RunQueueInner {
    head: null_mut(),
    len:  0,
}));

impl SingleCpuRunQueue {
    unsafe fn push_raw(&self, p: *mut Process) {
        let inner = &mut *self.0.get();
        let p_vr  = (*p).vruntime.load(Ordering::Relaxed);
        let p_pid = (*p).pid;
        (*p).rq_next.store(null_mut(), Ordering::Relaxed);
        (*p).on_rq.store(true, Ordering::Relaxed);

        let before_head = inner.head.is_null() || {
            let h_vr = (*inner.head).vruntime.load(Ordering::Relaxed);
            p_vr < h_vr || (p_vr == h_vr && p_pid < (*inner.head).pid)
        };

        if before_head {
            (*p).rq_next.store(inner.head, Ordering::Relaxed);
            inner.head = p;
            inner.len += 1;
            return;
        }

        let mut curr = inner.head;
        loop {
            let next = (*curr).rq_next.load(Ordering::Relaxed);
            if next.is_null() {
                (*curr).rq_next.store(p, Ordering::Relaxed);
                break;
            }
            let nv = (*next).vruntime.load(Ordering::Relaxed);
            if p_vr < nv || (p_vr == nv && p_pid < (*next).pid) {
                (*p).rq_next.store(next, Ordering::Relaxed);
                (*curr).rq_next.store(p, Ordering::Relaxed);
                break;
            }
            curr = next;
        }
        inner.len += 1;
    }

    unsafe fn pop_min_raw(&self) -> Option<*mut Process> {
        let inner = &mut *self.0.get();
        if inner.head.is_null() { return None; }

        let mut prev: *mut Process = null_mut();
        let mut curr = inner.head;

        while !curr.is_null() {
            if !(*curr).is_idle {
                let next = (*curr).rq_next.load(Ordering::Relaxed);
                if prev.is_null() {
                    inner.head = next;
                } else {
                    (*prev).rq_next.store(next, Ordering::Relaxed);
                }
                (*curr).rq_next.store(null_mut(), Ordering::Relaxed);
                (*curr).on_rq.store(false, Ordering::Relaxed);
                inner.len -= 1;
                return Some(curr);
            }
            prev = curr;
            curr = (*curr).rq_next.load(Ordering::Relaxed);
        }

        None
    }

    unsafe fn peek_min_vr_raw(&self) -> Option<u64> {
        let inner = &*self.0.get();
        if inner.head.is_null() { return None; }
        Some((*inner.head).vruntime.load(Ordering::Relaxed))
    }

    unsafe fn has_non_idle_raw(&self) -> bool {
        let inner = &*self.0.get();
        let mut curr = inner.head;
        while !curr.is_null() {
            if !(*curr).is_idle { return true; }
            curr = (*curr).rq_next.load(Ordering::Relaxed);
        }
        false
    }

    unsafe fn remove_raw(&self, pid: u64) {
        let inner = &mut *self.0.get();
        if inner.head.is_null() { return; }

        if (*inner.head).pid == pid {
            let p = inner.head;
            inner.head = (*p).rq_next.load(Ordering::Relaxed);
            (*p).rq_next.store(null_mut(), Ordering::Relaxed);
            (*p).on_rq.store(false, Ordering::Relaxed);
            inner.len -= 1;
            return;
        }

        let mut curr = inner.head;
        loop {
            let next = (*curr).rq_next.load(Ordering::Relaxed);
            if next.is_null() { return; }
            if (*next).pid == pid {
                let after = (*next).rq_next.load(Ordering::Relaxed);
                (*curr).rq_next.store(after, Ordering::Relaxed);
                (*next).rq_next.store(null_mut(), Ordering::Relaxed);
                (*next).on_rq.store(false, Ordering::Relaxed);
                inner.len -= 1;
                return;
            }
            curr = next;
        }
    }

    pub fn push(&self, p: *mut Process) {
        interrupts::without_interrupts(|| unsafe { self.push_raw(p) });
    }

    pub fn remove(&self, pid: u64) {
        interrupts::without_interrupts(|| unsafe { self.remove_raw(pid) });
    }

    pub fn len(&self) -> usize {
        interrupts::without_interrupts(|| unsafe { (*self.0.get()).len })
    }
}

struct ProcIndex(UnsafeCell<[*mut Process; MAX_PROCS]>);

unsafe impl Sync for ProcIndex {}
unsafe impl Send for ProcIndex {}

static PROC_INDEX: ProcIndex =
    ProcIndex(UnsafeCell::new([null_mut::<Process>(); MAX_PROCS]));

impl ProcIndex {
    #[inline]
    unsafe fn get_raw(&self, pid: u64) -> *mut Process {
        if pid as usize >= MAX_PROCS { return null_mut(); }
        (*self.0.get())[pid as usize]
    }

    fn set(&self, pid: u64, p: *mut Process) {
        if pid as usize >= MAX_PROCS { return; }
        interrupts::without_interrupts(|| unsafe {
            (*self.0.get())[pid as usize] = p;
        });
    }

    fn clear(&self, pid: u64) {
        self.set(pid, null_mut());
    }
}

static CURRENT_PID:    AtomicU64 = AtomicU64::new(0);
static MIN_VRUNTIME:   AtomicU64 = AtomicU64::new(0);
static TOTAL_SWITCHES: AtomicU64 = AtomicU64::new(0);

static PROC_TABLE: Mutex<BTreeMap<u64, Box<Process>>> = Mutex::new(BTreeMap::new());

pub trait Task: Send {
    fn run(self: Box<Self>);
}

impl<F: FnOnce() + Send> Task for F {
    fn run(self: Box<Self>) { (*self)() }
}

static WORK_QUEUE: Mutex<VecDeque<Box<dyn Task>>> = Mutex::new(VecDeque::new());

pub fn submit_task<F: FnOnce() + Send + 'static>(f: F) {
    WORK_QUEUE.lock().push_back(Box::new(f));
}

fn worker_loop() -> ! {
    x86_64::instructions::interrupts::enable();
    loop {
        let task = interrupts::without_interrupts(|| WORK_QUEUE.lock().pop_front());
        match task {
            Some(t) => t.run(),
            None    => sleep(5),
        }
    }
}

pub fn init_workers(count: usize) {
    for _ in 0..count {
        spawn_named(worker_loop, "worker", 10);
    }
    crate::serial_println!("[sched] {} worker threads started", count);
}

#[derive(Debug, Clone)]
pub struct ThreadStat {
    pub pid:            u64,
    pub name:           &'static str,
    pub state:          &'static str,
    pub priority:       u8,
    pub cpu_mask:       u64,
    pub cpu_time:       u64,
    pub vruntime:       u64,
    pub preempt_count:  u64,
    pub sleep_count:    u64,
    pub switch_in:      u64,
    pub cpu_pct_x10:    u32,
    pub uptime_ticks:   u64,
    pub is_idle:        bool,
    pub stack_alloc_kb: usize,
    pub stack_used_kb:  usize,
}

fn register_process(p: Box<Process>) -> *mut Process {
    let pid = p.pid;
    let mut table = PROC_TABLE.lock();
    table.insert(pid, p);
    let raw: *mut Process = table.get_mut(&pid).unwrap().as_mut();
    drop(table);
    PROC_INDEX.set(pid, raw);
    raw
}

fn add_process(mut p: Box<Process>) {
    let min_vr = MIN_VRUNTIME.load(Ordering::Relaxed);
    p.vruntime.store(min_vr, Ordering::Relaxed);
    let name = p.name;
    let raw  = register_process(p);
    let pid  = unsafe { (*raw).pid };
    interrupts::without_interrupts(|| unsafe { RUN_QUEUE.push_raw(raw) });
    crate::serial_println!("[sched] spawn pid={} name={}", pid, name);
}

pub fn init_main_thread() {
    let tick = crate::interrupts::get_tick();
    let cr3  = crate::vmm::kernel_cr3();
    let raw  = register_process(Process::new_idle(cr3, tick));
    CURRENT_PID.store(0, Ordering::Release);
    crate::serial_println!("[sched] idle thread registered ptr={:p}", raw);
}

pub fn reinit_scheduler() {
    CURRENT_PID.store(0, Ordering::Relaxed);
    MIN_VRUNTIME.store(0, Ordering::Relaxed);
    TOTAL_SWITCHES.store(0, Ordering::Relaxed);

    interrupts::without_interrupts(|| unsafe {
        let inner = &mut *RUN_QUEUE.0.get();
        inner.head = null_mut();
        inner.len  = 0;

        let arr = &mut *PROC_INDEX.0.get();
        for slot in arr.iter_mut() {
            *slot = null_mut();
        }
    });

    PROC_TABLE.lock().clear();
    *WORK_QUEUE.lock() = VecDeque::new();
}

pub fn reap_dead() {
    let curr = CURRENT_PID.load(Ordering::Relaxed);

    let dead_pids: Vec<u64> = {
        let table = PROC_TABLE.lock();
        table.iter()
            .filter(|(_, p)| {
                p.state.load(Ordering::Relaxed) == STATE_DEAD
                    && p.pid != curr
                    && !p.on_rq.load(Ordering::Relaxed)
            })
            .map(|(&pid, _)| pid)
            .collect()
    };

    for pid in dead_pids {
        PROC_INDEX.clear(pid);

        core::sync::atomic::compiler_fence(Ordering::SeqCst);

        let mut table = PROC_TABLE.lock();
        let collectable = table.get(&pid).map_or(false, |p| {
            p.state.load(Ordering::Relaxed) == STATE_DEAD
                && p.pid != curr
                && !p.on_rq.load(Ordering::Relaxed)
        });
        if !collectable { continue; }

        if let Some(p) = table.remove(&pid) {
            drop(table);
            free_process_resources(p, pid);
        }
    }
}

pub fn spawn(entry: fn() -> !) -> u64 {
    spawn_named(entry, "kthread", 10)
}

pub fn spawn_named(entry: fn() -> !, name: &'static str, priority: u8) -> u64 {
    let p = Process::new_kernel_named(entry, name, priority);
    let pid = p.pid;
    reap_dead();
    add_process(p);
    pid
}

pub fn spawn_named_child_of(parent_pid: u64, entry: fn() -> !, name: &'static str, priority: u8) -> u64 {
    let mut p = Process::new_kernel_named(entry, name, priority);
    p.ppid.store(parent_pid, Ordering::Relaxed);
    let pid = p.pid;
    reap_dead();
    add_process(p);
    pid
}

pub fn spawn_child_of(parent_pid: u64, entry: fn() -> !) -> u64 {
    spawn_named_child_of(parent_pid, entry, "kthread", 10)
}

pub fn spawn_named_child(entry: fn() -> !, name: &'static str, priority: u8) -> u64 {
    let parent = current_pid();
    spawn_named_child_of(parent, entry, name, priority)
}

pub fn set_parent(pid: u64, new_parent: u64) {
    let table = PROC_TABLE.lock();
    if let Some(p) = table.get(&pid) {
        p.ppid.store(new_parent, Ordering::Relaxed);
    }
}

pub fn add_user_process(p: Box<Process>) -> u64 {
    let pid = p.pid;
    reap_dead();
    add_process(p);
    pid
}

pub fn waitpid(pid: u64) {
    loop {
        let alive = interrupts::without_interrupts(|| {
            let ptr = unsafe { PROC_INDEX.get_raw(pid) };
            if ptr.is_null() { return false; }
            let state = unsafe { &*ptr }.state.load(Ordering::Relaxed);
            state != STATE_DEAD
        });
        if !alive { break; }
        yield_now();
    }
}

pub fn current_pid() -> u64 {
    CURRENT_PID.load(Ordering::Relaxed)
}

/// Expose raw process pointer for signal delivery (interrupts must be disabled by caller).
pub unsafe fn proc_index_raw(pid: u64) -> *mut Process {
    PROC_INDEX.get_raw(pid)
}

pub fn kill(pid: u64) {
    kill_with_code(pid, 0);
}

pub fn kill_with_code(pid: u64, code: u64) {
    let ppid = interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return 0u64; }
        let p = unsafe { &*ptr };
        p.exit_code.store(code, Ordering::Relaxed);
        p.state.store(STATE_DEAD, Ordering::Relaxed);
        unsafe { RUN_QUEUE.remove_raw(pid) };
        crate::serial_println!("[sched] kill pid={} code={}", pid, code);
        p.ppid.load(Ordering::Relaxed)
    });

    // Wake parent if it's blocking on wait4
    if ppid != 0 {
        wakeup(ppid);
    }
}

pub fn wakeup(pid: u64) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        let p     = unsafe { &*ptr };
        let state = p.state.load(Ordering::Relaxed);
        if state != STATE_SLEEPING && state != STATE_BLOCKED { return; }
        let min_vr = MIN_VRUNTIME.load(Ordering::Relaxed);
        let vr     = p.vruntime.load(Ordering::Relaxed).max(min_vr);
        p.vruntime.store(vr, Ordering::Relaxed);
        p.state.store(STATE_READY, Ordering::Relaxed);
        unsafe { RUN_QUEUE.push_raw(ptr) };
    });
}

pub fn set_affinity(pid: u64, mask: u64) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        unsafe { (*ptr).cpu_mask = if mask == 0 { CPU_ALL } else { mask } };
        crate::serial_println!("[sched] pid={} affinity={:#018x}", pid, mask);
    });
}

pub fn set_priority(pid: u64, priority: u8) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        unsafe { &*ptr }.priority.store(priority.clamp(1, 20), Ordering::Relaxed);
        crate::serial_println!("[sched] pid={} priority={}", pid, priority);
    });
}

pub fn yield_now() {
    interrupts::without_interrupts(|| {
        let curr = CURRENT_PID.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        if p.state.load(Ordering::Relaxed) == STATE_RUNNING {
            // bump vruntime to at least MIN_VRUNTIME so yielding processes
            // don't starve other READY processes with higher vruntime
            let min_vr = MIN_VRUNTIME.load(Ordering::Relaxed);
            let cur_vr = p.vruntime.load(Ordering::Relaxed);
            if cur_vr < min_vr {
                p.vruntime.store(min_vr, Ordering::Relaxed);
            }
            p.state.store(STATE_READY, Ordering::Relaxed);
            unsafe { RUN_QUEUE.push_raw(ptr) };
        }
    });
    unsafe { software_context_switch() }
}

pub fn sleep(ticks: u64) {
    let wake_tick = crate::interrupts::get_tick() + ticks;
    interrupts::without_interrupts(|| {
        let curr = CURRENT_PID.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.sleep_until.store(wake_tick, Ordering::Relaxed);
        p.state.store(STATE_SLEEPING, Ordering::Relaxed);
    });
    unsafe { software_context_switch() }
}

pub fn block_current(cause: &'static str) {
    interrupts::without_interrupts(|| {
        let curr = CURRENT_PID.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.blocked_cause.store(cause.as_ptr() as *mut u8, Ordering::Relaxed);
        p.state.store(STATE_BLOCKED, Ordering::Relaxed);
    });
    unsafe { software_context_switch() }
}

pub fn total_switches() -> u64 {
    TOTAL_SWITCHES.load(Ordering::Relaxed)
}

pub fn thread_count() -> usize {
    PROC_TABLE.lock().len()
}

pub fn get_stats() -> Vec<ThreadStat> {
    let now   = crate::interrupts::get_tick();
    let table = PROC_TABLE.lock();
    let mut out = Vec::with_capacity(table.len());
    for (&pid, p) in table.iter() {
        out.push(ThreadStat {
            pid,
            name:           p.name,
            state:          p.state_name(),
            priority:       p.priority.load(Ordering::Relaxed),
            cpu_mask:       p.cpu_mask,
            cpu_time:       p.cpu_time.load(Ordering::Relaxed),
            vruntime:       p.vruntime.load(Ordering::Relaxed),
            preempt_count:  p.preempt_count.load(Ordering::Relaxed),
            sleep_count:    p.sleep_count.load(Ordering::Relaxed),
            switch_in:      p.switch_in_count.load(Ordering::Relaxed),
            cpu_pct_x10:    p.cpu_percent_window(now),
            uptime_ticks:   p.uptime_ticks(now),
            is_idle:        p.is_idle,
            stack_alloc_kb: p.stack.len() / 1024,
            stack_used_kb:  p.stack_used_bytes() / 1024,
        });
    }
    out.sort_by_key(|s| s.pid);
    out
}

pub fn debug_dump_all() {
    let max = pid_range().min(MAX_PROCS as u64) as usize;
    let arr = unsafe { &*PROC_INDEX.0.get() };
    for i in 0..max {
        let ptr = arr[i];
        if ptr.is_null() { continue; }
        let p = unsafe { &*ptr };
        let state = p.state.load(Ordering::Relaxed);
        let state_str = match state {
            STATE_READY => "READY",
            STATE_RUNNING => "RUN",
            STATE_SLEEPING => "SLEEP",
            STATE_BLOCKED => "BLOCK",
            STATE_DEAD => "DEAD",
            _ => "?",
        };
        crate::serial_println!("[debug] pid={} state={}", i, state_str);
    }
}

#[inline(always)]
unsafe fn wake_sleepers_isr(tick: u64) {
    let max    = pid_range().min(MAX_PROCS as u64) as usize;
    let arr    = &*PROC_INDEX.0.get();
    let min_vr = MIN_VRUNTIME.load(Ordering::Relaxed);

    for i in 0..max {
        let ptr = arr[i];
        if ptr.is_null() { continue; }
        let p = &*ptr;
        if p.state.load(Ordering::Relaxed) != STATE_SLEEPING { continue; }
        if tick < p.sleep_until.load(Ordering::Relaxed) { continue; }

        let vr = p.vruntime.load(Ordering::Relaxed).max(min_vr);
        p.vruntime.store(vr, Ordering::Relaxed);
        p.state.store(STATE_READY, Ordering::Relaxed);
        RUN_QUEUE.push_raw(ptr);
    }
}

#[no_mangle]
pub unsafe extern "C" fn schedule_from_isr(old_rsp: u64) -> u64 {
    let tick     = crate::interrupts::get_tick();
    let curr_pid = CURRENT_PID.load(Ordering::Relaxed);
    let curr_ptr = PROC_INDEX.get_raw(curr_pid);

    let mut need_switch = false;

    if !curr_ptr.is_null() {
        let curr = &*curr_ptr;
        curr.rsp.store(old_rsp, Ordering::Relaxed);

        match curr.state.load(Ordering::Relaxed) {
            STATE_RUNNING if !curr.is_idle => {
                let w      = weight(curr.priority.load(Ordering::Relaxed));
                let dv     = TICK_SCALE / w;
                let new_vr = curr.vruntime.fetch_add(dv, Ordering::Relaxed) + dv;
                curr.cpu_time.fetch_add(1, Ordering::Relaxed);
                curr.window_cpu_ticks.fetch_add(1, Ordering::Relaxed);

                let ws = curr.window_start.load(Ordering::Relaxed);
                if tick.saturating_sub(ws) >= CPU_WINDOW_TICKS {
                    curr.window_cpu_ticks.store(1, Ordering::Relaxed);
                    curr.window_start.store(tick, Ordering::Relaxed);
                }

                if let Some(next_vr) = RUN_QUEUE.peek_min_vr_raw() {
                    if new_vr > next_vr {
                        curr.state.store(STATE_READY, Ordering::Relaxed);
                        curr.preempt_count.fetch_add(1, Ordering::Relaxed);
                        RUN_QUEUE.push_raw(curr_ptr);
                        need_switch = true;
                    }
                }
            }
            STATE_RUNNING => {
                if RUN_QUEUE.has_non_idle_raw() {
                    curr.state.store(STATE_READY, Ordering::Relaxed);
                    need_switch = true;
                }
            }
            STATE_SLEEPING => {
                curr.sleep_count.fetch_add(1, Ordering::Relaxed);
                need_switch = true;
            }
            _ => {
                need_switch = true;
            }
        }
    } else {
        need_switch = true;
    }

    wake_sleepers_isr(tick);

    if !need_switch {
        return old_rsp;
    }

    let next_ptr = match RUN_QUEUE.pop_min_raw() {
        Some(p) => p,
        None => {
            let idle_ptr = PROC_INDEX.get_raw(0);
            if idle_ptr.is_null() { return old_rsp; }
            let idle = &*idle_ptr;
            if idle.state.load(Ordering::Relaxed) == STATE_RUNNING {
                return old_rsp;
            }
            idle_ptr
        }
    };

    let next     = &*next_ptr;
    let old_cr3  = if !curr_ptr.is_null() { (*curr_ptr).cr3 } else { 0 };
    let new_cr3  = next.cr3;
    let new_rsp0 = next.stack_top();
    let new_rsp  = next.rsp.load(Ordering::Relaxed);
    let new_pid  = next.pid;

    next.state.store(STATE_RUNNING, Ordering::Relaxed);
    next.switch_in_count.fetch_add(1, Ordering::Relaxed);
    next.last_run_tick.store(tick, Ordering::Relaxed);

    let min_vr = MIN_VRUNTIME.load(Ordering::Relaxed)
        .max(next.vruntime.load(Ordering::Relaxed));
    MIN_VRUNTIME.store(min_vr, Ordering::Relaxed);
    TOTAL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    CURRENT_PID.store(new_pid, Ordering::Relaxed);

    crate::gdt::set_kernel_stack(new_rsp0);

    if old_cr3 != new_cr3 && new_cr3 != 0 {
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) new_cr3,
            options(nostack, preserves_flags)
        );
    }

    new_rsp
}

#[unsafe(naked)]
unsafe extern "C" fn software_context_switch() {
    core::arch::naked_asm!(
        "cli",
        "mov rax, rsp",
        "push 0x10",
        "push rax",
        "pushfq",
        "or qword ptr [rsp], 0x200",
        "push 0x08",
        "lea rax, [rip + 1f]",
        "push rax",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rbp",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push 0",
        "mov rdi, rsp",
        "call {sched}",
        "mov rsp, rax",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rbp",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "iretq",
        "1:",
        "ret",
        sched = sym schedule_from_isr,
    )
}

// fork/wait4 helpers //

/// Find a zombie child of `parent_pid`. If `target_pid` == u64::MAX, any child
pub fn find_zombie_child(parent_pid: u64, target_pid: u64) -> Option<(u64, u64)> {
    let table = PROC_TABLE.lock();
    for (&pid, p) in table.iter() {
        if p.ppid.load(Ordering::Relaxed) != parent_pid { continue; }
        if target_pid != u64::MAX && pid != target_pid { continue; }
        if p.state.load(Ordering::Relaxed) == STATE_DEAD && !p.collected.load(Ordering::Relaxed) {
            let code = p.exit_code.load(Ordering::Relaxed);
            p.collected.store(true, Ordering::Relaxed);
            return Some((pid, code));
        }
    }
    None
}

/// Reap a collected zombie (free resources)
pub fn reap_zombie(pid: u64) {
    reap_specific(pid);
}

fn free_process_resources(mut p: Box<Process>, pid: u64) {
    crate::mmap::vma_cleanup(p.cr3);
    if let Some(phys) = p.user_stack_phys.take() {
        crate::pmm::free_frames(phys, crate::process::USER_STACK_PAGES);
    }
    if p.cr3 != 0 && p.cr3 != crate::vmm::kernel_cr3() {
        let mut aspace = crate::vmm::AddressSpace { cr3: p.cr3 };
        aspace.free_address_space();
    }
    crate::serial_println!("[sched] reaped pid={}", pid);
}

fn reap_specific(pid: u64) {
    PROC_INDEX.clear(pid);
    core::sync::atomic::compiler_fence(Ordering::SeqCst);

    let mut table = PROC_TABLE.lock();
    if let Some(p) = table.remove(&pid) {
        drop(table);
        free_process_resources(p, pid);
    }
}

/// Check if `parent_pid` has any living (non-dead) or zombie children
pub fn has_children(parent_pid: u64) -> bool {
    let table = PROC_TABLE.lock();
    table.iter().any(|(_, p)| p.ppid.load(Ordering::Relaxed) == parent_pid)
}

/// Check if a process exists (alive or zombie)
pub fn process_exists(pid: u64) -> bool {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        !ptr.is_null()
    })
}

/// Update the CR3 of a running process (used by exec)
pub fn update_process_cr3(pid: u64, new_cr3: u64) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        unsafe { (*ptr).cr3 = new_cr3; }
    });
}

/// Get the parent PID of a process
pub fn get_ppid(pid: u64) -> u64 {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return 0; }
        unsafe { &*ptr }.ppid.load(Ordering::Relaxed)
    })
}
