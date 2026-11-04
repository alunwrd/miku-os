// What a CPU does with nothing to run (`dev/kernel-core.md`, "Bottlenecks
// round", "Waking an idle processor")
//
// It watches its own run queue for a while, then halts. A CPU watching needs no
// IPI to learn of a task queued for it, and under KVM an IPI to a halted vCPU
// is a VM exit on each side, four microseconds of a pipe round trip that takes
// ten. How long it watches is learned from how soon it has been woken, as
// Linux's `haltpoll` governor does in a guest: a CPU that sleeps for ticks
// stops watching, one woken every few microseconds keeps on.
//
// The BSP and the APs both run this.

use core::sync::atomic::{fence, AtomicU8, Ordering};

use x86_64::instructions::interrupts;

use crate::arch::x86_64::percpu::{self, Cpu};
use crate::kcore::time::now_ns;

/// A wake within this long of going idle grows the window; a longer sleep
/// shrinks it. Linux's default for a guest
const POLL_MAX_NS: u64 = 200_000;
/// The first window a CPU grows to, and the smallest it keeps
const POLL_START_NS: u64 = 50_000;

/// `nopoll` on the boot line: 0 not read yet, 1 watch, 2 halt at once
static MODE: AtomicU8 = AtomicU8::new(0);

fn polls() -> bool {
    match MODE.load(Ordering::Relaxed) {
        0 => {
            let off = crate::arch::x86_64::grub::cmdline_flag("nopoll");
            MODE.store(if off { 2 } else { 1 }, Ordering::Relaxed);
            !off
        }
        m => m == 1,
    }
}

/// The idle task's body on every CPU, from `kernel_main` on the BSP and the
/// AP entry on the rest. The timer and the reschedule IPI switch away from
/// idle on their own; the yield here is how a task found by watching, or
/// queued by an interrupt on this CPU, gets the processor
pub fn run() -> ! {
    interrupts::enable();
    loop {
        super::yield_now();
        idle_once(percpu::current());
    }
}

fn idle_once(cpu: &Cpu) {
    let start = now_ns();
    let window = cpu.poll_ns.load(Ordering::Relaxed);
    if window > 0 {
        cpu.polling.store(true, Ordering::SeqCst);
        // `polling` cleared under it is a switch away and back meanwhile
        // (`left`): the window is spent
        while cpu.run_queue.queued() == 0
            && cpu.polling.load(Ordering::Relaxed)
            && now_ns().wrapping_sub(start) < window
        {
            core::hint::spin_loop();
        }
        // A full fence before the queue is looked at again: a waker that
        // read `polling` before this sent no IPI, and had queued first
        // (`kick`)
        cpu.polling.store(false, Ordering::SeqCst);
    }
    interrupts::disable();
    if cpu.run_queue.queued() == 0 {
        // `sti; hlt` is one step for interrupts: an IPI sent from here on
        // ends the halt rather than landing before it
        interrupts::enable_and_hlt();
    } else {
        interrupts::enable();
    }
    // When a task came: the switch away from idle stamps it, and a halt
    // that ended with nothing to do took until now
    let left = cpu.idle_left_ns.load(Ordering::Relaxed);
    let woke = if left > start { left } else { now_ns() };
    adapt(cpu, woke.saturating_sub(start));
}

/// The window, after a CPU idle for `blocked` nanoseconds was woken
fn adapt(cpu: &Cpu, blocked: u64) {
    if !polls() {
        return;
    }
    let w = cpu.poll_ns.load(Ordering::Relaxed);
    let next = if blocked > w && blocked <= POLL_MAX_NS {
        (w * 2).clamp(POLL_START_NS, POLL_MAX_NS)
    } else if blocked > POLL_MAX_NS {
        if w / 2 < POLL_START_NS { 0 } else { w / 2 }
    } else {
        w
    };
    if next != w {
        cpu.poll_ns.store(next, Ordering::Relaxed);
    }
}

/// `cpu` is switching from idle to a task: when, for `idle_once`'s measure, and
/// not watching any more. The timer or an IPI can take the processor from idle
/// mid-window, and a flag left up for the next task would keep every other CPU
/// from interrupting it
#[inline]
pub(super) fn left(cpu: &Cpu) {
    cpu.polling.store(false, Ordering::Relaxed);
    cpu.idle_left_ns.store(now_ns(), Ordering::Relaxed);
}

/// Tell `cpu` a task is queued for it: nothing when it is watching its
/// queue, the reschedule IPI when it may be halted or running something
/// else. The caller has queued the task already
pub fn kick(cpu: &Cpu) {
    // The queue's store before `polling` is read: the other half of the
    // fence in `idle_once`, so one side always sees the other
    fence(Ordering::SeqCst);
    if cpu.polling.load(Ordering::Relaxed) {
        return;
    }
    crate::arch::x86_64::apic::send_ipi(
        cpu.lapic_id.load(Ordering::Relaxed),
        crate::arch::x86_64::apic::VEC_IPI_RESCHED,
    );
}
