// Process registry: PID -> *mut Process index plus the owning BTreeMap
//
// Two cooperating containers:
//   PROC_TABLE - owns the boxed Process; access guarded by a Mutex
//   PROC_INDEX - flat pid-keyed pointer array, lock-free reads from any 
//                    context (including ISRs). Updated only with interrupts
//                    disabled to keep ISR readers consistent with the table

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::Ordering;
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::percpu;
use crate::process::Process;

pub const MAX_PROCS: usize = 4096;

pub struct ProcIndex(pub(super) UnsafeCell<[*mut Process; MAX_PROCS]>);
unsafe impl Sync for ProcIndex {}
unsafe impl Send for ProcIndex {}

pub static PROC_INDEX: ProcIndex =
    ProcIndex(UnsafeCell::new([null_mut::<Process>(); MAX_PROCS]));

pub static PROC_TABLE: Mutex<BTreeMap<u64, Box<Process>>> = Mutex::new(BTreeMap::new());

impl ProcIndex {
    #[inline]
    pub unsafe fn get_raw(&self, pid: u64) -> *mut Process {
        if pid as usize >= MAX_PROCS { return null_mut(); }
        (*self.0.get())[pid as usize]
    }

    pub fn set(&self, pid: u64, p: *mut Process) {
        if pid as usize >= MAX_PROCS { return; }
        interrupts::without_interrupts(|| unsafe {
            (*self.0.get())[pid as usize] = p;
        });
    }

    pub fn clear(&self, pid: u64) { self.set(pid, null_mut()); }

    /// Drop every entry. Safe only when no other CPU can be inspecting the
    /// table - used by reinit_scheduler during shutdown reboots
    pub fn clear_all(&self) {
        interrupts::without_interrupts(|| unsafe {
            for slot in (*self.0.get()).iter_mut() { *slot = null_mut(); }
        });
    }
}

/// Insert a process into the registry and return a stable raw pointer to it
pub fn register_process(p: Box<Process>) -> *mut Process {
    let pid = p.pid;
    let mut table = PROC_TABLE.lock();
    table.insert(pid, p);
    let raw: *mut Process = table.get_mut(&pid).unwrap().as_mut();
    drop(table);
    PROC_INDEX.set(pid, raw);
    raw
}

/// Returns true if any online CPU currently has pid as its current task
pub fn pid_running_anywhere(pid: u64) -> bool {
    for c in percpu::iter_online() {
        if c.current_pid.load(Ordering::Relaxed) == pid { return true; }
    }
    false
}

/// External read-only handle into the index. Kept for callers outside the
/// scheduler that need to peek at a process by PID without locking
pub unsafe fn proc_index_raw(pid: u64) -> *mut Process {
    PROC_INDEX.get_raw(pid)
}
