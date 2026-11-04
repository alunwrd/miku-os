// Waiting for something to happen, off the run queue
//
// A wait checking its condition and calling `yield_now()` never leaves the run
// queue: measured at 370,000 context switches a second per waiting Mayland
// client, each taking the compositor's global lock twice. `sched::sleep` takes
// a wait off the run queue and `sched::wakeup` puts it back; this is where who
// waits for what is written down.
//
// The backstop
//
// `wait` takes a tick count and sleeps at most that long even if nobody wakes
// it. The wake sites below make a wait end promptly; the backstop decides what
// happens when one is wrong: a missed wake shows up as latency rather than a
// thread that never runs again.
//
// `wakeup` closes the other half of the race: a task that has asked to sleep
// and not reached the context switch has its request cancelled by a CAS on
// `pending_state`, so a wake in that window is not lost (see
// `control::wakeup`).
//
// The rule for wakers
//
// Collect the pids under whatever lock protects what changed, drop that lock,
// and only then wake. With the lock held, the woken thread runs straight into
// it.

extern crate alloc;

use alloc::vec::Vec;

use crate::kcore::irq_lock::IrqMutex;

/// Who is waiting on one thing.
///
/// A `Vec` of pids rather than an intrusive list: a queue holds a handful of
/// entries (one reader per connection, one shell per child), and walking it is
/// paid when something happens
///
/// An `IrqMutex`: taken with interrupts on (a syscall that has slept is
/// preemptible) and off (`kill`, fault paths), and a holder the timer took off
/// the processor would leave every CPU spinning on it with interrupts off
pub struct WaitQueue {
    waiters: IrqMutex<Vec<u64>>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self { waiters: IrqMutex::new(Vec::new()) }
    }

    /// Leaves the run queue until woken, or until `backstop` ticks have passed
    ///
    /// The caller must check its condition immediately before this and again
    /// after, in a loop: a wake is a hint that something may have happened
    pub fn wait(&self, backstop: u64) {
        self.wait_until(super::tick_deadline(backstop.max(1)));
    }

    /// `wait`, until `deadline` on `kcore::time::now_ns`'s clock rather than
    /// for a number of ticks: for a timeout a program asked for
    pub fn wait_until(&self, deadline: u64) {
        let me = super::current_pid();
        self.waiters.lock().push(me);
        super::sleep_until(deadline);
        self.leave(me);
    }

    /// `wait_until`, for a waiter that can look at its condition without a
    /// lock: on the queue and asleep-on-request before `ready` is asked, so a
    /// waker changing the condition and then waking this queue is never lost
    /// (`sched::sleep_until_unless`). Returns whether it slept
    pub fn wait_unless(&self, deadline: u64, ready: impl FnOnce() -> bool) -> bool {
        let me = super::current_pid();
        self.waiters.lock().push(me);
        let slept = super::sleep_until_unless(deadline, ready);
        self.leave(me);
        slept
    }

    /// Off the queue whichever way the sleep ended. By value and not by index
    /// because a waker may already have taken this entry
    fn leave(&self, me: u64) {
        let mut w = self.waiters.lock();
        if let Some(i) = w.iter().position(|&p| p == me) {
            w.swap_remove(i);
        }
    }

    /// Wakes everything waiting here
    ///
    /// The list is taken whole and the lock dropped before any wakeup, so a
    /// woken thread never lands on a lock this one holds
    pub fn wake_all(&self) {
        let pending: Vec<u64> = {
            let mut w = self.waiters.lock();
            if w.is_empty() {
                return;
            }
            core::mem::take(&mut *w)
        };
        for pid in pending {
            super::wakeup(pid);
        }
    }

    /// Wakes one waiter, for a queue where exactly one can make progress
    ///
    /// The first that was asleep: a pid whose task died in its wait, or already
    /// awake and leaving the queue, is passed over, or a lock's waiters behind
    /// it would sleep until their backstop
    pub fn wake_one(&self) {
        loop {
            let pid = {
                let mut w = self.waiters.lock();
                if w.is_empty() {
                    return;
                }
                // The oldest: a lock handed round its waiters in turn
                w.remove(0)
            };
            if super::wakeup(pid) {
                return;
            }
        }
    }

    /// Whether anyone is waiting, for a waker that would otherwise do work to
    /// find out there was nobody to tell
    pub fn is_empty(&self) -> bool {
        self.waiters.lock().is_empty()
    }
}

/// Waits on all of `queues` at once, until any is woken or `deadline` passes:
/// what `poll` does with the queues of its descriptors. The rules are
/// `WaitQueue::wait`'s: a wake is a hint, and the caller looks again. A queue
/// appearing twice is joined and left twice
pub fn wait_any(queues: &[&WaitQueue], deadline: u64) {
    let me = super::current_pid();
    for q in queues {
        q.waiters.lock().push(me);
    }
    super::sleep_until(deadline);
    for q in queues {
        q.leave(me);
    }
}

/// `wait_any`, with `ready` asked once the caller is on every queue and has
/// asked to sleep (`WaitQueue::wait_unless`): a wake between the caller's last
/// look and its sleep is not lost. Returns whether it slept
pub fn wait_any_unless(queues: &[&WaitQueue], deadline: u64, ready: impl FnOnce() -> bool) -> bool {
    let me = super::current_pid();
    for q in queues {
        q.waiters.lock().push(me);
    }
    let slept = super::sleep_until_unless(deadline, ready);
    for q in queues {
        q.leave(me);
    }
    slept
}

/// On a set of queues from `join` until dropped, for a waiter whose look at its
/// condition cannot be `wait_any_unless`'s `ready`: a scan of many descriptors,
/// a look under a lock that sleeps.
///
/// The look comes after the join, and `sleep` sleeps unless this task has been
/// sent a wake since (`Process::wakes`), by anyone. A waker changing the
/// condition after the look found this task on its queue, and either counted a
/// wake before the sleep was asked (no sleep then) or found it asked and
/// cancelled it. `wait_any`, with the look before the join, loses that wake to
/// the backstop. A wake for anything else costs one more look.
///
/// A loop takes it as: look; nothing, join, and look again; still nothing,
/// sleep. The second look is the one that counts
pub struct Joined<'a> {
    queues: &'a [&'a WaitQueue],
    me: u64,
    seen: u64,
}

pub fn join<'a>(queues: &'a [&'a WaitQueue]) -> Joined<'a> {
    let me = super::current_pid();
    let seen = super::wake_count();
    for q in queues {
        q.waiters.lock().push(me);
    }
    Joined { queues, me, seen }
}

impl Joined<'_> {
    /// Off the run queue until a wake or `deadline`, unless a wake came since
    /// `join`; then off the queues. Returns whether it slept
    pub fn sleep(self, deadline: u64) -> bool {
        let seen = self.seen;
        super::sleep_until_unless(deadline, || super::wake_count() != seen)
    }
}

impl Drop for Joined<'_> {
    fn drop(&mut self) {
        for q in self.queues {
            q.leave(self.me);
        }
    }
}

/// Waiting for a thread to die: what `waitpid` sits on
///
/// One queue for every death: a waiter re-checks its pid, and there are a
/// handful of them (a shell per foreground program, init reaping). Woken from
/// `lifecycle::kill_with_code`
pub static DEATH: WaitQueue = WaitQueue::new();

/// Waiting for bytes in a pipe, or for the last writer to close
///
/// Also one queue rather than one per pipe, for the same reason and with the
/// same re-check. Woken from the write path and from `release_fds_on_exit`
pub static PIPE: WaitQueue = WaitQueue::new();

/// Waiting in `poll` for something no key below names
///
/// Every poller is on this queue as well as on the queues of its own
/// descriptors (`keyed`). What wakes it is what has no key: a process exiting,
/// a socket closing, a frame for some socket from the network's receive path
pub static POLL: WaitQueue = WaitQueue::new();

/// Tells every `poll` that something changed, for an event with no key of its
/// own. An event that has one wakes that (`wake_key`)
pub fn wake_pollers() {
    POLL.wake_all();
}

/// What a waiter is waiting on, as a number: a kind in the top byte and the
/// thing's own number below it. A waker and a waiter that agree on the number
/// meet on the same queue (`keyed`)
pub mod key {
    use crate::io::tty::TtyId;

    /// Bytes in a pipe, room in it, or its last writer gone: one key for both
    /// ends, as a pipe is one buffer
    pub fn pipe(vnode: usize) -> u64 { 1 << 56 | vnode as u64 }
    /// A local socket's slot: bytes arrived in it, or room in it for a writer
    pub fn socket(slot: usize) -> u64 { 2 << 56 | slot as u64 }
    /// A compositor connection: events queued for that client
    pub fn mayland(client: u32) -> u64 { 3 << 56 | client as u64 }
    /// Typing at a terminal, or its hangup
    pub fn tty_input(id: TtyId) -> u64 { 4 << 56 | id.0 as u64 }
    /// What a terminal's program printed, for its master; or its hangup
    pub fn pty_output(id: TtyId) -> u64 { 5 << 56 | id.0 as u64 }
    /// Segments queued for a TCP connection, by its demux handle
    pub fn net_tcp(handle: usize) -> u64 { 6 << 56 | handle as u64 }
    /// A SYN queued for a listener, by its demux handle
    pub fn net_listen(handle: usize) -> u64 { 7 << 56 | handle as u64 }
    /// A datagram queued for a UDP port, by its demux handle
    pub fn net_udp(handle: usize) -> u64 { 8 << 56 | handle as u64 }
    /// An ICMP message queued for a waiter (ping, traceroute, a raw socket)
    pub fn net_icmp(handle: usize) -> u64 { 9 << 56 | handle as u64 }
}

/// How many queues the keys share
///
/// A fixed set, as `futex` has buckets: a queue per object would have to be
/// made and destroyed with the object and found by whoever wakes it. Two things
/// on one queue cost each other's waiters a look
const KEYED_QUEUES: usize = 256;

static KEYED: [WaitQueue; KEYED_QUEUES] = [const { WaitQueue::new() }; KEYED_QUEUES];

/// The queue `key` waits on
pub fn keyed(key: u64) -> &'static WaitQueue {
    let mut h = key ^ (key >> 29);
    h = h.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    &KEYED[(h >> 56) as usize % KEYED_QUEUES]
}

/// Wake whoever waits on `key`
pub fn wake_key(key: u64) {
    keyed(key).wake_all();
}
