// The display layer says something changed
//
// K3 of `dev/kms-architecture.md`. One serial and one listener: an output
// configuration is made against the serial (one built on a state that has moved
// on is `cancelled`, as in wlr-output-management), and the listener is the
// compositor, which reads `kms::state()` and tells its clients. A plain
// function, since the layer does not know the compositor.
//
// `changed` is called after the change is recorded and the layer's lock is
// released: the listener takes the server's lock, so the order is KMS ->
// SERVER.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

static SERIAL: AtomicU32 = AtomicU32::new(1);

/// The listener, as a function address; 0 when nobody listens
static LISTENER: AtomicUsize = AtomicUsize::new(0);

/// The state's serial: different every time something about an output changed
pub fn serial() -> u32 {
    SERIAL.load(Ordering::Acquire)
}

/// Who hears about it. One listener, the compositor, set once at its init
pub fn listen(f: fn()) {
    LISTENER.store(f as usize, Ordering::Release);
}

/// Something about an output changed: a head, a monitor, a cable
pub fn changed() {
    SERIAL.fetch_add(1, Ordering::AcqRel);
    let f = LISTENER.load(Ordering::Acquire);
    if f != 0 {
        // Safety: only `listen` stores here, and it stores a `fn()`
        let f: fn() = unsafe { core::mem::transmute::<usize, fn()>(f) };
        f();
    }
}
