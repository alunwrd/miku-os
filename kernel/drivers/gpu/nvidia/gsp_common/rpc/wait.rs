// Waiting for GSP-RM, and the interrupt that ends the wait

use crate::kcore::isr_event::IsrEvent;

/// GSP-RM said something.
///
/// Notified by the GSP falcon's interrupt handler (SWGEN0, "the message queue
/// moved"), slept on by whoever waits on a reply. One waiter slot: the RPC
/// layer runs from one thread at a time, the shell thread during bring-up and
/// the RM lock's holder afterwards
static MSGQ: IsrEvent<1> = IsrEvent::new();

/// Called from interrupt context. Atomics and one `sched::wakeup`, safe from an
/// ISR
pub fn note_msgq_interrupt() {
    MSGQ.notify();
}

/// How long a wait spins before it sleeps.
///
/// Almost every RPC on this card answers in well under this, and a timer tick
/// is 4 ms, so an immediate sleep would turn a 50 us round trip into 4 ms and
/// slow the display bring-up's hundreds of small calls. Past the window the
/// wait is long (GSP-RM can take about ten seconds to come up)
const RPC_SPIN_NS: u64 = 200_000;

/// Wait for the message queue to move, without burning a core.
///
/// Spins when it cannot do better: before the GPU's interrupt is up only the
/// backstop would end a sleep
pub(super) fn wait_for_msgq(elapsed_ns: u64, ready: impl Fn() -> bool) {
    if elapsed_ns < RPC_SPIN_NS
        || !crate::drivers::gpu::nvidia::intr::active()
    {
        core::hint::spin_loop();
        return;
    }
    let seen = MSGQ.seq();
    MSGQ.sleep_unless(seen, 1, ready);
}
