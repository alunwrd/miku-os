// Interrupts from the network card (`dev/net-architecture.md`, 3a)
//
// With an MSI the card says when something came. The handler does two things
// and takes no lock (`NET` is a spin lock thread code holds with interrupts on,
// and an interrupt spinning on it on the same CPU spins for ever,
// `dev/kernel-core.md`):
//   - acknowledges the card's own cause register, where the card wants that
//     before it will raise the next one (`Ack`, set up by the driver);
//   - marks a frame pending and wakes `RX`, the queue netd sleeps on.
// netd pumps the ring; the demux wakes whoever the frames are for
// (`sched::wait::key::net_tcp` and its neighbours).
//
// Only message-signalled: a PCI pin's route to an IO-APIC input is in the ACPI
// `_PRT`, which needs an AML interpreter this kernel does not have. A card
// without MSI (QEMU's default e1000) is pumped every tick. `netpoll` on the
// boot line keeps every card that way.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::drivers::bus::msi::{self, PciFn};
use crate::sched::WaitQueue;

/// netd sleeps here between pumps
pub static RX: WaitQueue = WaitQueue::new();

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// A frame was signalled and no pump has run since
static PENDING: AtomicBool = AtomicBool::new(false);

/// What the handler does to the card first, as a function and its argument
/// (an MMIO base); 0 for nothing
static ACK_FN: AtomicUsize = AtomicUsize::new(0);
static ACK_ARG: AtomicU64 = AtomicU64::new(0);

/// How long a wait for the network lasts with nothing to wake it, once the
/// card interrupts: long enough not to matter, short enough for the
/// retransmission timers the waiters run when they wake
const BACKSTOP_IRQ_NS: u64 = 20_000_000;

fn handler(_: usize) {
    let f = ACK_FN.load(Ordering::Acquire);
    if f != 0 {
        let ack: fn(u64) = unsafe { core::mem::transmute(f) };
        ack(ACK_ARG.load(Ordering::Acquire));
    }
    PENDING.store(true, Ordering::Release);
    RX.wake_all();
}

/// Whether the card interrupts
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// How far ahead a network wait's backstop lies: a tick while the card is
/// polled, since netd's pump every tick is what delivers then, and longer once
/// it interrupts
pub fn backstop_ns() -> u64 {
    if active() {
        BACKSTOP_IRQ_NS
    } else {
        crate::sched::tick_deadline(1).saturating_sub(crate::kcore::time::now_ns())
    }
}

/// netd, between pumps: until the card signals or the backstop. A signal
/// that came after the last pump and before this is not lost (`PENDING`)
pub fn wait() {
    if !active() {
        crate::sched::sleep(1);
        return;
    }
    let deadline = crate::kcore::time::now_ns().saturating_add(BACKSTOP_IRQ_NS);
    RX.wait_unless(deadline, || PENDING.load(Ordering::Acquire));
}

/// What netd clears before it pumps: anything signalled after this is
/// another pump's
pub fn take_pending() {
    PENDING.store(false, Ordering::Release);
}

/// A pump that could not take the ring (`NET` held): netd goes round again
/// rather than sleep on frames already there
pub fn rearm() {
    PENDING.store(true, Ordering::Release);
    RX.wake_all();
}

/// How a driver asks for interrupts: the function's MSI, or entry `entry`
/// of its MSI-X table, delivered to the boot CPU. `ack(arg)` runs first in
/// the handler. Whether interrupts are now on
pub fn enable(f: PciFn, msix_entry: Option<u16>, ack: Option<(fn(u64), u64)>) -> bool {
    use crate::arch::x86_64::apic;
    if crate::arch::x86_64::grub::cmdline_flag("netpoll") {
        return false;
    }
    let caps = msi::read_caps(f);
    if let Some((func, arg)) = ack {
        ACK_ARG.store(arg, Ordering::Release);
        ACK_FN.store(func as usize, Ordering::Release);
    }
    let Some(target) = apic::alloc_msi_vector(apic::lapic_id(), handler, 0) else {
        crate::serial_println!("[net] no MSI vector free - polling");
        return false;
    };
    let ok = match msix_entry {
        Some(entry) => match (caps.msix, caps.msix.and_then(|c| msi::map_msix(f, &c))) {
            (Some(cap), Some(table)) if entry < table.len() => {
                table.set(entry, target.address, target.data);
                msi::enable_msix(f, &cap);
                true
            }
            _ => false,
        },
        None => match caps.msi {
            Some(cap) => msi::program_msi(f, &cap, target.address, target.data).is_ok(),
            None => false,
        },
    };
    if !ok {
        apic::free_msi_vector(target);
        ACK_FN.store(0, Ordering::Release);
        return false;
    }
    ACTIVE.store(true, Ordering::Release);
    crate::serial_println!(
        "[net] receive by {} on vector {:#x}",
        if msix_entry.is_some() { "MSI-X" } else { "MSI" },
        target.vector
    );
    true
}

/// Back to polling, for a driver whose card refused what `enable` set up
pub fn disable() {
    ACTIVE.store(false, Ordering::Release);
    ACK_FN.store(0, Ordering::Release);
}

/// Whether `f` has the capability `enable` would use, for a driver that must
/// set the card up differently when it will interrupt
pub fn has_msi(f: PciFn) -> bool {
    !crate::arch::x86_64::grub::cmdline_flag("netpoll") && msi::read_caps(f).msi.is_some()
}

pub fn has_msix(f: PciFn) -> bool {
    !crate::arch::x86_64::grub::cmdline_flag("netpoll") && msi::read_caps(f).msix.is_some()
}
