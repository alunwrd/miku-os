// process: process model, credentials/fd tables, signals, program execution
// (ELF load + exec), dynamic linking (ldso/solib) and relocations. Core process
// logic lives directly in this module

pub mod cred;
pub mod coredump;
pub mod crash;
pub mod elf;
pub mod image;
pub mod imgcache;
pub mod symbols;
pub mod vdso;
pub mod selftest;
pub mod selftest_cred;
pub mod session;
pub mod elf_loader;
pub mod elf_source;
pub mod exec;
pub mod ldso;
pub mod reloc;
pub mod signal;
pub mod solib;

use alloc::boxed::Box;
use alloc::vec;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};
use x86_64::structures::paging::PageTableFlags;
use crate::mm::vmm::AddressSpace;
use crate::process::cred::Cred;

pub const USER_STACK_VIRT_TOP: u64   = 0x0000_7FFF_FFFF_0000;
/// A program's first stack: 512 KiB
pub const USER_STACK_PAGES:    usize = 512 * 1024 / 4096;
pub const CPU_ALL:             u64   = u64::MAX;
pub const FRAME_SIZE:          u64   = 0xA0;

pub const STATE_READY:   u8 = 0;
pub const STATE_RUNNING: u8 = 1;
pub const STATE_SLEEPING: u8 = 2;
pub const STATE_BLOCKED:  u8 = 3;
pub const STATE_DEAD:     u8 = 4;
/// Off the run queue and off the timer, until somebody says to continue
///
/// Not `STATE_BLOCKED`: `wakeup` wakes anything blocked, and `send_signal`
/// calls `wakeup` after every send, so a SIGWINCH would un-stop it. A stop has
/// one way out: `sched::cont`.
///
/// Not `STATE_SLEEPING` either: a sleeper is on the timer, and a Ctrl-Z wearing
/// off after the backstop would be a pause, not a stop
pub const STATE_STOPPED:  u8 = 5;

/// 'pending_state' holds no request. Any other value is the state the task
/// asked to move to at its next context switch
pub const PENDING_NONE:  u8 = u8::MAX;


/// Callee-saved registers preserved across syscall entry, needed by fork()
pub struct SavedSyscallRegs {
    pub rbp: u64,
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub r8:  u64,
    pub r9:  u64,
    pub r10: u64,
    /// Kept across a `syscall` (see `syscall_handler`), so the child of a
    /// `fork` gets them back too
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
}

/// What a process is called, stored inside the process
///
/// Twenty-three bytes and a length: `ProcInfo.name` in the userspace ABI is
/// `[u8; 24]` NUL-terminated, and `sys_procinfo` copies at most 23. A larger
/// buffer would be cut on the way out.
///
/// No allocation and no lifetime, because of where it is read from: the
/// scheduler holding a raw pointer with no lock, and the page-fault handler
/// reporting which process ran off its kernel stack
#[derive(Clone, Copy)]
pub struct Name {
    buf: [u8; Name::CAP],
    len: u8,
}

impl Name {
    pub const CAP: usize = 23;

    pub const fn empty() -> Self {
        Name { buf: [0; Self::CAP], len: 0 }
    }

    /// Truncated rather than refused: a name is a label for a person reading
    /// `ps`, and a shortened one is still the right process
    pub fn new(s: &str) -> Self {
        let mut n = Name::empty();
        // On a character boundary, so the result is still text: half a UTF-8
        // sequence would make `as_str` fall back to "?"
        let mut end = s.len().min(Self::CAP);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        n.buf[..end].copy_from_slice(&s.as_bytes()[..end]);
        n.len = end as u8;
        n
    }

    /// The last component of a path: what `exec` calls the program
    ///
    /// From the path and never from `argv[0]`, which is whatever the caller
    /// passed; `ps` shows what is running. A trailing slash is ignored so
    /// `/bin/` does not come back empty
    pub fn from_path(path: &str) -> Self {
        let trimmed = path.trim_end_matches('/');
        let base = match trimmed.rfind('/') {
            Some(i) => &trimmed[i + 1..],
            None => trimmed,
        };
        Name::new(base)
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("?")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }
}

impl core::fmt::Display for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::fmt::Debug for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

pub struct Process {
    pub pid:             u64,
    /// The thread group this task belongs to: the pid of its leader.
    ///
    /// Zero means "I am my own leader", which lets a constructor not know its
    /// own pid. Read it through `tgid()`, never directly.
    ///
    /// What belongs to the process (the descriptor table, the working
    /// directory, the credential, the terminal, the job) is reached through the
    /// leader; what belongs to the task (its kernel stack, its registers, its
    /// `FS_BASE`, its FPU area) stays on this struct. The rule: if two threads
    /// must agree about it, it is the leader's
    pub tgid:            AtomicU64,
    pub name:            Name,
    pub is_idle:         bool,
    pub priority:        AtomicU8,
    pub cpu_mask:        u64,
    pub cr3:             u64,
    pub wall_start_tick: u64,

    pub rsp:              AtomicU64,
    /// User-mode RSP captured by the syscall entry (percpu Cpu.user_rsp,
    /// gs:[0x18]). The percpu slot is one per CPU; when a process blocks inside
    /// a syscall and another makes syscalls in between, the slot is
    /// overwritten. The scheduler saves and restores it here on every context
    /// switch so the syscall epilogue's 'mov rsp, gs:[0x18]' returns to the
    /// right user stack
    pub saved_user_rsp:   AtomicU64,
    /// IA32_FS_BASE, where a thread-local variable is reached from.
    ///
    /// One MSR per machine state, standing in for per-task state, so it is
    /// saved and restored beside `saved_user_rsp`: two ring-3 programs using
    /// TLS would otherwise read each other's variables. The kernel uses GS for
    /// percpu and never FS, so a kernel thread's zero here is a correct value
    pub saved_fs_base:    AtomicU64,
    /// This task's x87/SSE registers, swapped beside `saved_fs_base` for the
    /// same reason: one register file per core. A stale `xmm0` computes the
    /// wrong number silently. See `arch::x86_64::fpu`
    pub fpu:              crate::arch::x86_64::fpu::FpuState,
    /// A user mapping this task owns, unmapped when it dies. Base and length,
    /// or zero for a task that owns none.
    ///
    /// A thread's stack has nobody else to free it: its last instruction is the
    /// one moment it cannot unmap the stack it stands on, and a detached thread
    /// is never joined (`std::thread::spawn` produces one each time its handle
    /// is dropped).
    ///
    /// The kernel can: by the time this is read the task is off every processor
    /// and the address space still belongs to its siblings. Set by
    /// `thread_create` when asked, and read in `sched::free_process_resources`
    pub owned_stack:      AtomicU64,
    pub owned_stack_len:  AtomicU64,
    /// On the leader: the whole group is being ended (`kill_group_with_code`).
    /// A thread created while that is under way is not in the list of members
    /// it walks. `thread_spawn` reads this after its new thread is in the table
    /// and ends it there
    pub group_exiting:    AtomicBool,
    /// Taken off a run queue and not yet on a processor: set by
    /// `runqueue::take_min` before `on_rq` drops, cleared once `on_cpu` is set
    /// or the task is let go. In that stretch the task is on no queue and no
    /// CPU, and a reaper finding it dead could free the stack the picking CPU
    /// is about to switch onto (`sched::lifecycle::quiesced`)
    pub in_switch:        AtomicBool,
    pub state:            AtomicU8,
    /// State the task wants to enter at its next context switch, applied by
    /// schedule_from_isr once 'rsp' holds a valid frame. yield/sleep/block must
    /// not write 'state' themselves: once a task is READY on a run queue
    /// another CPU may steal it, and until the switch has saved the context it
    /// would resume from a stale 'rsp': two CPUs running one thread on one
    /// stack
    pub pending_state:    AtomicU8,
    /// True from the moment a task is picked to run until the CPU that ran
    /// it has left its stack. A CPU that wants to resume a task spins here
    /// first, so the outgoing CPU is never still using the stack underneath
    pub on_cpu:           AtomicBool,
    /// When the current sleep ends, on `kcore::time::now_ns`'s clock
    pub wake_ns:          AtomicU64,
    /// The sleeping tasks of one CPU, soonest first (`sched::timer`): which
    /// CPU's heap this task is in (`timer::NONE` for none) and where in it.
    /// `lifecycle::quiesced` will not free a task that is in one
    pub timer_cpu:        AtomicU32,
    pub timer_slot:       AtomicU32,
    pub blocked_cause:    AtomicPtr<u8>,
    pub vruntime:         AtomicU64,
    pub cpu_time:         AtomicU64,
    pub window_cpu_ticks: AtomicU64,
    pub window_start:     AtomicU64,
    pub last_run_tick:    AtomicU64,
    pub preempt_count:    AtomicU64,
    pub sleep_count:      AtomicU64,
    pub switch_in_count:  AtomicU64,
    /// Wakes this task has been sent, whatever state they found it in: what
    /// a waiter compares across the look at its condition and the sleep
    /// (`sched::wait::join`)
    pub wakes:            AtomicU64,

    /// The CPU whose run queue holds this task (`slot_heap::NO_CPU` for none)
    /// and where in that queue's heap (`sched::runqueue`). Written under that
    /// queue's lock
    pub rq_cpu:  AtomicU32,
    pub rq_slot: AtomicU32,
    pub on_rq:   AtomicBool,

    /// Guard-page-fenced kernel stack (see mm::kstack)
    pub stack:           crate::mm::kstack::KernelStack,
    pub user_stack_phys: Option<u64>,
    pub brk:             AtomicU64,

    // fork/process hierarchy //
    pub ppid:        AtomicU64,
    pub exit_code:   AtomicU64,
    pub pending_sig: AtomicU32,
    /// Userspace signal-dispatch entry point registered via sys_sigentry.
    /// 0 = none: fatal signals apply their default action in the kernel
    pub sig_entry:       AtomicU64,
    /// True while this task runs a handler: set when a frame is written,
    /// put back by `sigreturn` to what the frame recorded. Holds the next
    /// asynchronous signal until then; a fault is delivered regardless
    pub sig_in_delivery: AtomicBool,
    /// What this process wants done about each signal: two bits per signal,
    /// read through `signal::disposition`.
    ///
    /// Zero, every signal at its default, is what a process that never says
    /// otherwise has
    pub sig_disp:        AtomicU64,
    /// Which signals' handlers run on the alternate stack (`SA_ONSTACK`), one
    /// bit per signal. Process-wide, read through the leader; set by
    /// `sigdisp`(100) with `DISP_ONSTACK`
    pub sig_onstack:     AtomicU32,
    /// This task's alternate signal stack, from `sigaltstack`(115): its lowest
    /// address and its size, zero size for none. Per task, as POSIX says: two
    /// threads overflowing their stacks at once need two places to report from.
    /// A new thread has none; `exec` clears it
    pub sig_alt_sp:      AtomicU64,
    pub sig_alt_size:    AtomicU64,
    pub wait_target: AtomicU64,
    pub collected:   AtomicBool,
    /// A reaper has taken this task and is the only one that will free it.
    ///
    /// `reap_dead` runs from every spawn, on every processor at once. Two
    /// threads of one process removed together could both count zero and both
    /// free the address space. See `sched::lifecycle::reap_one`
    pub reaping:     AtomicBool,
    /// A stop has been asked for and not yet applied
    ///
    /// Not `pending_state`, which is what the task itself asked for at its next
    /// switch and which every voluntary transition overwrites: a thread
    /// spinning on `yield_now` would clobber the stop. A stop is not a request
    /// the task makes
    pub stop_pending:  AtomicBool,
    /// This process has stopped and its parent has already been told
    ///
    /// Distinct from `collected`: a stopped child is reported and left alive,
    /// where a
    /// dead one is reported and reaped. Without this `wait4` would hand back
    /// the same
    /// stop on every call
    pub stop_reported: AtomicBool,
    /// Which signal stopped it, for the status `wait4` reports
    pub stop_sig:      core::sync::atomic::AtomicU32,
    /// Which signal ended it, or `0` for a process that exited of its own
    /// accord
    ///
    /// Recorded when it happens. `kill_with_code` stores `128 + sig` for a
    /// signal death, the convention a shell prints, and as a number it is
    /// ambiguous: `exit(148)` and death by SIGTSTP are the same value. The
    /// status `wait4` reports has to tell those apart
    pub killed_by_sig: core::sync::atomic::AtomicU32,

    /// Per-process current working directory (VFS inode id). 0 = root.
    /// Inherited on fork, preserved across exec. Reads use the scheduler
    /// helper 'crate::sched::current_cwd()'
    pub cwd:         AtomicU64,

    /// The terminal this task's output goes to, as a raw `TtyId`. 0 = the
    /// console.
    ///
    /// Beside `cwd` because it has the same lifetime: inherited on fork,
    /// preserved across exec, and read through the scheduler. It decides where
    /// `print!` lands, so one shell can run on each terminal with no `tty`
    /// argument threaded through every command
    pub ctty:        AtomicU32,

    /// The job this process is part of: a process group id, which is the pid of
    /// the process that started the group.
    ///
    /// The middle of the three levels a terminal has: session, job, terminal.
    /// It decides who a keystroke's signal reaches: Ctrl-C at a terminal goes
    /// to every process in that terminal's foreground group, so `cmd | cmd2` is
    /// interruptible as a unit and a shell waiting on a child does not take the
    /// child's interrupt.
    ///
    /// Inherited by fork and kept across exec, like `ctty`. Set at creation to
    /// the process's own pid, so there is no "no group" state
    pub pgid:        AtomicU64,

    /// Who this process is and what it may do. Synced into vfs.ctx on every
    /// with_vfs() call, so ctx.cred read sites in MikuVFS stay correct.
    ///
    /// One lock around the whole credential: a change of identity has to be
    /// all-or-nothing
    pub cred:        spin::RwLock<Cred>,
}

impl Process {
    /// Kernel-side construction for paths that run at boot, where running
    /// out of memory is not a condition anything could recover from
    pub fn alloc_raw(name: &str, priority: u8, cr3: u64) -> Box<Self> {
        let stack = crate::mm::kstack::KernelStack::new(crate::mm::kstack::KERNEL_THREAD_STACK)
            .expect("out of physical memory allocating a kernel stack");
        // The kernel's own threads, at boot and from the worker pool. No caller
        // here could do anything with a failure, as with the stack above it
        Self::alloc_with_stack(name, priority, cr3, stack)
            .expect("out of pids allocating a kernel thread")
    }

    fn alloc_with_stack(
        name: &str,
        priority: u8,
        cr3: u64,
        stack: crate::mm::kstack::KernelStack,
    ) -> Option<Box<Self>> {
        let tick  = crate::arch::x86_64::interrupts::get_tick();
        let pid   = crate::sched::alloc_pid()?;
        let p = Box::new(Self {
            pid,
            name:            Name::new(name),
            is_idle:         false,
            priority:        AtomicU8::new(priority.clamp(1, 20)),
            cpu_mask:        CPU_ALL,
            cr3,
            wall_start_tick: tick,
            rsp:              AtomicU64::new(0),
            saved_user_rsp:   AtomicU64::new(0),
            saved_fs_base:    AtomicU64::new(0),
            fpu:              crate::arch::x86_64::fpu::FpuState::new(),
            owned_stack:      AtomicU64::new(0),
            owned_stack_len:  AtomicU64::new(0),
            group_exiting:    AtomicBool::new(false),
            in_switch:        AtomicBool::new(false),
            tgid:             AtomicU64::new(0),
            state:            AtomicU8::new(STATE_READY),
            pending_state:    AtomicU8::new(PENDING_NONE),
            on_cpu:           AtomicBool::new(false),
            wake_ns:          AtomicU64::new(0),
            timer_cpu:        AtomicU32::new(crate::sched::timer::NONE),
            timer_slot:       AtomicU32::new(0),
            blocked_cause:    AtomicPtr::new(null_mut()),
            vruntime:         AtomicU64::new(0),
            cpu_time:         AtomicU64::new(0),
            window_cpu_ticks: AtomicU64::new(0),
            window_start:     AtomicU64::new(tick),
            last_run_tick:    AtomicU64::new(tick),
            preempt_count:    AtomicU64::new(0),
            sleep_count:      AtomicU64::new(0),
            wakes:            AtomicU64::new(0),
            switch_in_count:  AtomicU64::new(0),
            rq_cpu:           AtomicU32::new(crate::sched::slot_heap::NO_CPU),
            rq_slot:          AtomicU32::new(0),
            on_rq:            AtomicBool::new(false),
            stack,
            user_stack_phys:  None,
            brk:              AtomicU64::new(0),
            ppid:             AtomicU64::new(0),
            exit_code:        AtomicU64::new(0),
            pending_sig:      AtomicU32::new(0),
            sig_entry:        AtomicU64::new(0),
            sig_in_delivery:  AtomicBool::new(false),
            sig_disp:         AtomicU64::new(0),
            sig_onstack:      AtomicU32::new(0),
            sig_alt_sp:       AtomicU64::new(0),
            sig_alt_size:     AtomicU64::new(0),
            wait_target:      AtomicU64::new(0),
            collected:        AtomicBool::new(false),
            reaping:          AtomicBool::new(false),
            stop_pending:     AtomicBool::new(false),
            stop_reported:    AtomicBool::new(false),
            stop_sig:         core::sync::atomic::AtomicU32::new(0),
            killed_by_sig:    core::sync::atomic::AtomicU32::new(0),
            cwd:              AtomicU64::new(0),
            ctty:             AtomicU32::new(0),
            // Its own group until something says otherwise. The literal
            // cannot read the pid it is assigning, so this is fixed up below
            pgid:             AtomicU64::new(0),
            // Kernel-side construction: PID 1 and the kernel threads. A user
            // process never reaches here with these credentials - exec()
            // preserves the caller's, and login/mikudod set them explicitly
            cred:             spin::RwLock::new(Cred::root()),
        });
        // Every process leads its own job unless somebody moves it into
        // another one. Written here because the pid is only known once the
        // literal above has run
        p.pgid.store(p.pid, Ordering::Relaxed);
        Some(p)
    }

    pub fn new_idle_ap(cr3: u64, tick: u64, name: &str) -> Box<Self> {
        let mut idle = Self::new_idle(cr3, tick);
        idle.pid = crate::sched::alloc_pid()
            .expect("out of pids bringing up an AP idle task");
        idle.name = Name::new(name);
        idle
    }

    pub fn new_idle(cr3: u64, tick: u64) -> Box<Self> {
        let stack = crate::mm::kstack::KernelStack::new(crate::mm::kstack::KERNEL_THREAD_STACK)
            .expect("out of physical memory allocating a kernel stack");
        Box::new(Self {
            pid: 0,
            name: Name::new("idle"),
            is_idle: true,
            priority:        AtomicU8::new(20),
            cpu_mask:        CPU_ALL,
            cr3,
            wall_start_tick: tick,
            rsp:              AtomicU64::new(0),
            saved_user_rsp:   AtomicU64::new(0),
            saved_fs_base:    AtomicU64::new(0),
            fpu:              crate::arch::x86_64::fpu::FpuState::new(),
            owned_stack:      AtomicU64::new(0),
            owned_stack_len:  AtomicU64::new(0),
            group_exiting:    AtomicBool::new(false),
            in_switch:        AtomicBool::new(false),
            tgid:             AtomicU64::new(0),
            state:            AtomicU8::new(STATE_RUNNING),
            pending_state:    AtomicU8::new(PENDING_NONE),
            on_cpu:           AtomicBool::new(false),
            wake_ns:          AtomicU64::new(0),
            timer_cpu:        AtomicU32::new(crate::sched::timer::NONE),
            timer_slot:       AtomicU32::new(0),
            blocked_cause:    AtomicPtr::new(null_mut()),
            vruntime:         AtomicU64::new(0),
            cpu_time:         AtomicU64::new(0),
            window_cpu_ticks: AtomicU64::new(0),
            window_start:     AtomicU64::new(tick),
            last_run_tick:    AtomicU64::new(tick),
            preempt_count:    AtomicU64::new(0),
            sleep_count:      AtomicU64::new(0),
            wakes:            AtomicU64::new(0),
            switch_in_count:  AtomicU64::new(0),
            rq_cpu:           AtomicU32::new(crate::sched::slot_heap::NO_CPU),
            rq_slot:          AtomicU32::new(0),
            on_rq:            AtomicBool::new(false),
            stack,
            user_stack_phys:  None,
            brk:              AtomicU64::new(0),
            ppid:             AtomicU64::new(0),
            exit_code:        AtomicU64::new(0),
            pending_sig:      AtomicU32::new(0),
            sig_entry:        AtomicU64::new(0),
            sig_in_delivery:  AtomicBool::new(false),
            sig_disp:         AtomicU64::new(0),
            sig_onstack:      AtomicU32::new(0),
            sig_alt_sp:       AtomicU64::new(0),
            sig_alt_size:     AtomicU64::new(0),
            wait_target:      AtomicU64::new(0),
            collected:        AtomicBool::new(false),
            reaping:          AtomicBool::new(false),
            stop_pending:     AtomicBool::new(false),
            stop_reported:    AtomicBool::new(false),
            stop_sig:         core::sync::atomic::AtomicU32::new(0),
            killed_by_sig:    core::sync::atomic::AtomicU32::new(0),
            cwd:              AtomicU64::new(0),
            ctty:             AtomicU32::new(0),
            // The idle task is pid 0 and is in no job: it is not anybody's
            // child and nothing signals it
            pgid:             AtomicU64::new(0),
            cred:             spin::RwLock::new(Cred::root()),
        })
    }

    /// The pid of this task's thread-group leader, which for a task that is
    /// not a thread is its own.
    #[inline]
    pub fn tgid(&self) -> u64 {
        let t = self.tgid.load(Ordering::Relaxed);
        if t == 0 { self.pid } else { t }
    }

    /// True for a task sharing an address space with a leader that is not
    /// itself. Decides whether this task's death takes the address space, the
    /// descriptors and the sockets with it.
    #[inline]
    pub fn is_thread(&self) -> bool {
        let t = self.tgid.load(Ordering::Relaxed);
        t != 0 && t != self.pid
    }

    pub fn stack_top(&self) -> u64 {
        (self.stack.as_ptr() as u64 + self.stack.len() as u64) & !0xF
    }

    pub fn stack_used_bytes(&self) -> usize {
        let top = self.stack_top();
        let rsp = self.rsp.load(Ordering::Relaxed);
        if rsp == 0 || rsp > top { return 0; }
        (top - rsp) as usize
    }

    pub fn cpu_percent_window(&self, now: u64) -> u32 {
        let ws  = self.window_start.load(Ordering::Relaxed);
        let wct = self.window_cpu_ticks.load(Ordering::Relaxed);
        ((wct * 1000) / now.saturating_sub(ws).max(1)).min(1000) as u32
    }

    pub fn uptime_ticks(&self, now: u64) -> u64 {
        now.saturating_sub(self.wall_start_tick)
    }

    pub fn state_name(&self) -> &'static str {
        match self.state.load(Ordering::Relaxed) {
            STATE_READY | STATE_SLEEPING => "S",
            STATE_RUNNING                => "R",
            STATE_BLOCKED                => "B",
            STATE_DEAD                   => "X",
            STATE_STOPPED                => "T",
            _                            => "?",
        }
    }

    pub fn new_kernel_named(entry: fn() -> !, name: &str, priority: u8) -> Box<Self> {
        let cr3 = crate::mm::vmm::kernel_cr3();
        let mut p = Self::alloc_raw(name, priority, cr3);
        let top = p.stack_top();
        p.rsp.store(build_kernel_frame(top, entry as u64), Ordering::Relaxed);
        p
    }

    pub fn new_elf(entry: u64, user_rsp: u64, aspace: AddressSpace) -> Option<Box<Self>> {
        // Take the kernel stack before disarming `aspace`: if the
        // allocation fails, `aspace` is still owned here and its Drop
        // releases the address space instead of leaking it
        let stack = crate::mm::kstack::KernelStack::new(crate::mm::kstack::program_stack_size())?;
        let cr3 = aspace.into_raw();
        let mut p = Self::alloc_with_stack("user-elf", 10, cr3, stack)?;
        let top = p.stack_top();
        p.rsp.store(build_user_frame(top, entry, user_rsp), Ordering::Relaxed);
        Some(p)
    }

    /// A second task inside an existing process.
    ///
    /// What makes it a thread is what is not here: no new address space (it
    /// takes the leader's `cr3`), no descriptor table (the VFS is keyed on the
    /// tgid), no credential of its own worth reading (`sched` resolves those
    /// through the leader). It gets what a thread must have alone: a kernel
    /// stack, a register frame, and an `FS_BASE`.
    ///
    /// `ppid` is zero and `collected` true: a thread is not a child, `wait4`
    /// must never report one, and `find_zombie_child` decides that by those two
    /// fields.
    pub fn new_thread(
        leader_pid: u64,
        cr3: u64,
        entry: u64,
        user_rsp: u64,
        arg: u64,
        tls: u64,
        owned_stack: (u64, u64),
    ) -> Option<Box<Self>> {
        let stack = crate::mm::kstack::KernelStack::new(crate::mm::kstack::program_stack_size())?;
        let name  = crate::sched::name_of(leader_pid).unwrap_or(Name::new("thread"));
        let mut p = Self::alloc_with_stack(name.as_str(), 10, cr3, stack)?;
        p.name = name;
        p.tgid.store(leader_pid, Ordering::Relaxed);
        p.ppid.store(0, Ordering::Relaxed);
        p.collected.store(true, Ordering::Relaxed);
        // The leader's identity, copied into the thread's own slot.
        //
        // `sched` reads a credential through the leader, but `leader_raw` falls
        // back to the task's own fields when the leader is no longer in the
        // index, and `alloc_with_stack` fills the field with `Cred::root()`.
        // Left as it came, a thread reading its identity in that window would
        // read as root
        *p.cred.write() = crate::sched::cred_of(leader_pid);
        p.cwd.store(crate::sched::cwd_of(leader_pid), Ordering::Relaxed);
        p.sig_entry.store(crate::sched::sig_entry_of(leader_pid), Ordering::Relaxed);
        p.sig_disp.store(crate::sched::sig_disp_of(leader_pid), Ordering::Relaxed);
        // Its own TLS block. Zero is legal and means the thread has none
        p.saved_fs_base.store(tls, Ordering::Relaxed);
        // The address space is the leader's and this task must never free it.
        // `user_stack_phys` is what `free_process_resources` hands back to the
        // frame allocator; a thread's stack came from the program's own `mmap`
        // and is freed by the program, or by the address space going away
        p.user_stack_phys = None;
        // What the kernel will unmap when this task dies, if the caller asked.
        // `user_stack_phys` above is a different thing - frames the kernel
        // allocated itself for a process's initial stack - and a thread has
        // none
        p.owned_stack.store(owned_stack.0, Ordering::Relaxed);
        p.owned_stack_len.store(owned_stack.1, Ordering::Relaxed);
        let top = p.stack_top();
        p.rsp.store(build_thread_frame(top, entry, user_rsp, arg), Ordering::Relaxed);
        Some(p)
    }

    /// Creates a child process for fork(). Builds an iretq-compatible kernel
    /// frame so the child can be scheduled normally by the timer ISR. The child
    /// sees RAX=0 (fork returns 0 in child)
    pub fn new_fork(
        parent_pid: u64,
        child_cr3: u64,
        child_user_stack_phys: Option<u64>,
        child_brk: u64,
        user_rip: u64,
        user_rsp: u64,
        user_rflags: u64,
        saved_regs: &SavedSyscallRegs,
    ) -> Option<Box<Self>> {
        let tick  = crate::arch::x86_64::interrupts::get_tick();
        let stack = crate::mm::kstack::KernelStack::new(crate::mm::kstack::program_stack_size())
            .expect("out of physical memory allocating a kernel stack");
        // `None` rather than a panic: a program forking in a loop is a program,
        // not a kernel bug, and `fork` has an errno for this
        let pid = crate::sched::alloc_pid()?;
        let mut p = Box::new(Self {
            pid,
            // The parent's name: before `exec` a child is the same program,
            // which Linux reports too
            name:            crate::sched::name_of(parent_pid).unwrap_or(Name::empty()),
            is_idle:         false,
            priority:        AtomicU8::new(10),
            cpu_mask:        CPU_ALL,
            cr3:             child_cr3,
            wall_start_tick: tick,
            rsp:              AtomicU64::new(0),
            saved_user_rsp:   AtomicU64::new(0),
            // fork copies the address space with the TLS block in it, so the
            // child's thread-locals live at the parent's addresses. Read rather
            // than passed in: the only caller is sys_fork, where the parent is
            // the running task and its base is live in the MSR
            saved_fs_base:    AtomicU64::new(
                x86_64::registers::model_specific::FsBase::read().as_u64()),
            // Read from the CPU for the same reason as the line above
            fpu:              crate::arch::x86_64::fpu::FpuState::from_current(),
            owned_stack:      AtomicU64::new(0),
            owned_stack_len:  AtomicU64::new(0),
            group_exiting:    AtomicBool::new(false),
            in_switch:        AtomicBool::new(false),
            // A fork makes a process, never a thread: the child leads its own
            // group, and it is the only member of it
            tgid:             AtomicU64::new(0),
            state:            AtomicU8::new(STATE_READY),
            pending_state:    AtomicU8::new(PENDING_NONE),
            on_cpu:           AtomicBool::new(false),
            wake_ns:          AtomicU64::new(0),
            timer_cpu:        AtomicU32::new(crate::sched::timer::NONE),
            timer_slot:       AtomicU32::new(0),
            blocked_cause:    AtomicPtr::new(null_mut()),
            vruntime:         AtomicU64::new(0),
            cpu_time:         AtomicU64::new(0),
            window_cpu_ticks: AtomicU64::new(0),
            window_start:     AtomicU64::new(tick),
            last_run_tick:    AtomicU64::new(tick),
            preempt_count:    AtomicU64::new(0),
            sleep_count:      AtomicU64::new(0),
            wakes:            AtomicU64::new(0),
            switch_in_count:  AtomicU64::new(0),
            rq_cpu:           AtomicU32::new(crate::sched::slot_heap::NO_CPU),
            rq_slot:          AtomicU32::new(0),
            on_rq:            AtomicBool::new(false),
            stack,
            user_stack_phys:  child_user_stack_phys,
            brk:              AtomicU64::new(child_brk),
            ppid:             AtomicU64::new(parent_pid),
            exit_code:        AtomicU64::new(0),
            pending_sig:      AtomicU32::new(0),
            // The handler is inherited, like the working directory and the
            // terminal below: otherwise a signal the parent catches would be
            // fatal for the child until it exec'd
            sig_entry:        AtomicU64::new(crate::sched::sig_entry_of(parent_pid)),
            sig_in_delivery:  AtomicBool::new(false),
            // And what it does about each of them: a handler without the
            // disposition never runs
            sig_disp:         AtomicU64::new(crate::sched::sig_disp_of(parent_pid)),
            sig_onstack:      AtomicU32::new(crate::sched::sig_onstack_of(parent_pid)),
            // The forking task's alternate stack, which the copied address
            // space still holds at the same address. Read from the running
            // task for the reason `saved_fs_base` above is
            sig_alt_sp:       AtomicU64::new(crate::process::signal::current_altstack().0),
            sig_alt_size:     AtomicU64::new(crate::process::signal::current_altstack().1),
            wait_target:      AtomicU64::new(0),
            collected:        AtomicBool::new(false),
            reaping:          AtomicBool::new(false),
            stop_pending:     AtomicBool::new(false),
            stop_reported:    AtomicBool::new(false),
            stop_sig:         core::sync::atomic::AtomicU32::new(0),
            killed_by_sig:    core::sync::atomic::AtomicU32::new(0),
            cwd:              AtomicU64::new(crate::sched::cwd_of(parent_pid)),
            // The terminal follows the process that forked, as the working
            // directory does
            ctty:             AtomicU32::new(crate::sched::ctty_of(parent_pid)),
            // The child joins its parent's job. A shell forking a pipeline then
            // moves the whole thing into one group with `setpgid`
            pgid:             AtomicU64::new(crate::sched::pgid_of(parent_pid)),
            // The child is the parent, exactly, capabilities included
            cred:             spin::RwLock::new(crate::sched::cred_of(parent_pid)),
        });

        // Builds a timer-ISR-compatible frame for the child (20 u64 slots =
        // 0xA0 bytes). Layout:
        // [rax,rbx,rcx,rdx,rsi,rdi,rbp,r8,r9,r10,r11,r12,r13,r14,r15,
        // rip,cs,rflags,rsp,ss]
        let child_top = p.stack_top();
        let child_rsp = child_top - FRAME_SIZE;
        unsafe {
            let f = child_rsp as *mut u64;
            f.add(0).write(0);                // rax = 0 (fork returns 0 in child)
            f.add(1).write(saved_regs.rbx);
            f.add(2).write(0);                // rcx (clobbered by syscall)
            f.add(3).write(saved_regs.rdx);
            f.add(4).write(saved_regs.rsi);
            f.add(5).write(saved_regs.rdi);
            f.add(6).write(saved_regs.rbp);
            f.add(7).write(saved_regs.r8);
            f.add(8).write(saved_regs.r9);
            f.add(9).write(saved_regs.r10);
            f.add(10).write(0);               // r11 (clobbered by syscall)
            f.add(11).write(saved_regs.r12);
            f.add(12).write(saved_regs.r13);
            f.add(13).write(saved_regs.r14);
            f.add(14).write(saved_regs.r15);
            // iretq frame
            f.add(15).write(user_rip);
            f.add(16).write(crate::arch::x86_64::gdt::user_code_selector().0 as u64);
            f.add(17).write(user_rflags | 0x200); // ensure IF is set
            f.add(18).write(user_rsp);
            f.add(19).write(crate::arch::x86_64::gdt::user_data_selector().0 as u64);
        }
        p.rsp.store(child_rsp, Ordering::Relaxed);
        Some(p)
    }

    pub fn cleanup_user_address_space(&mut self) {
        if self.cr3 == 0 || self.cr3 == crate::mm::vmm::kernel_cr3() { return; }
        let mut aspace = AddressSpace::from_raw(self.cr3);
        aspace.free_address_space();
        self.cr3 = 0;
    }
}

fn build_kernel_frame(kernel_stack_top: u64, rip: u64) -> u64 {
    write_frame(kernel_stack_top, rip, 0x08, kernel_stack_top, 0x10)
}

/// A user frame that starts with an argument in place.
///
/// `write_frame` zeroes all fifteen general-purpose slots, right for a program
/// entered through its ELF entry point. A thread is entered as a function, so
/// its one argument has to be in `rdi` before the first instruction. Slot 5 is
/// `rdi`; the order is the one the interrupt entry stub pushes, and `exec`
/// writing the TLS base into slot 7 (`r8`) is the other user of it.
fn build_thread_frame(kernel_stack_top: u64, rip: u64, user_rsp: u64, arg: u64) -> u64 {
    let rsp = build_user_frame(kernel_stack_top, rip, user_rsp);
    unsafe { (rsp as *mut u64).add(5).write(arg); }
    rsp
}

fn build_user_frame(kernel_stack_top: u64, rip: u64, user_rsp: u64) -> u64 {
    write_frame(
        kernel_stack_top, rip,
        crate::arch::x86_64::gdt::user_code_selector().0 as u64,
        user_rsp,
        crate::arch::x86_64::gdt::user_data_selector().0 as u64,
    )
}

fn write_frame(kernel_stack_top: u64, rip: u64, cs: u64, iret_rsp: u64, ss: u64) -> u64 {
    let rsp = kernel_stack_top - FRAME_SIZE;
    unsafe {
        let f = rsp as *mut u64;
        for i in 0..15 { f.add(i).write(0); }
        f.add(15).write(rip);
        f.add(16).write(cs);
        f.add(17).write(0x202);
        f.add(18).write(iret_rsp);
        f.add(19).write(ss);
    }
    rsp
}
