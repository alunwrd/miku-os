// Shared work queue serviced by a fixed pool of kernel worker threads
//
// Anyone can submit_task a closure; the worker pool drains the queue and
// runs each task on a kernel-mode thread. Workers sleep briefly when the
// queue is empty rather than spinning, freeing the CPU for real work

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use spin::Mutex;
use x86_64::instructions::interrupts;

use super::control::sleep;
use super::lifecycle::spawn_named;

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

/// Reset the work queue. Called by reinit_scheduler when the system is
/// torn down for a soft reboot
pub(super) fn drain_queue() {
    *WORK_QUEUE.lock() = VecDeque::new();
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

/// How many workers to run on this machine.
///
/// The count used to be hardwired to 4, so a 16- or 32-core box drained the
/// shared work queue with the same four threads as a dual-core one and the
/// extra CPUs simply idled. One worker per CPU tracks the machine; the lower
/// bound keeps small machines responsive and the upper bound stops a very
/// wide box from spending all its memory on worker stacks.
pub fn default_worker_count() -> usize {
    const MIN_WORKERS: usize = 4;
    const MAX_WORKERS: usize = 32;
    // Count CPUs from the ACPI topology, not from percpu::cpu_count():
    // workers are started well before smp::start_aps(), so at this point
    // exactly one CPU is online and sizing the pool by that would pin every
    // machine to the minimum - which is precisely how the count stayed at 4
    let cpus = crate::arch::x86_64::acpi::topology()
        .as_ref()
        .map(|t| t.cpus.iter().filter(|c| c.enabled).count())
        .unwrap_or(0);
    let cpus = if cpus == 0 {
        crate::arch::x86_64::percpu::cpu_count()
    } else {
        cpus
    };
    cpus.clamp(MIN_WORKERS, MAX_WORKERS)
}
