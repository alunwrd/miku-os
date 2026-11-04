// Waking the thread that drains what nobody waits for
//
// OpenRM drains events from its interrupt's bottom half (`kgspRpcRecvEvents`, a
// DPC). Here the event thread is the bottom half, and this is how the interrupt
// reaches it. 200 ms is the backstop, for a missed SWGEN0 or an interrupt that
// is off (`nointr`).
//
// An `IsrEvent` (`kcore::isr_event`), as the RPC waiter and the vblank use.
// Waking a `WaitQueue` hands its list back to be freed, and nothing is freed in
// an ISR.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::kcore::isr_event::IsrEvent;

/// Something happened: SWGEN0, a HALT, a FATAL
static EVENT: IsrEvent<1> = IsrEvent::new();
/// How far the worker has looked
static SEEN: AtomicU64 = AtomicU64::new(0);

/// Tell the worker to look. Safe from an ISR
pub fn kick() {
    EVENT.notify();
}

/// For the worker: return when kicked, or after `backstop_ticks`. True if
/// kicked since it last asked, so a kick while busy is not lost
pub fn wait(backstop_ticks: u64) -> bool {
    let seen = SEEN.load(Ordering::Relaxed);
    let kicked = EVENT.seq() != seen || EVENT.sleep_unless(seen, backstop_ticks, || false);
    SEEN.store(EVENT.seq(), Ordering::Relaxed);
    kicked
}
