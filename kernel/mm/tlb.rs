// tlb: keeping every processor's translations honest after an unmap
//
// Clearing a page-table entry changes memory, not what a processor has cached.
// `invlpg` covers the one it runs on. A sibling thread on another processor
// keeps the old translation until something flushes it there, and the frame
// behind it goes back to the allocator when the entry is cleared. The sibling
// then writes into a frame that is somebody's page table, another process's
// page, or the same process's next allocation.
//
// An unmap in an address space loaded elsewhere clears its entries, calls
// `shootdown`, and only then frees the frames. There is no PCID, so a CR3
// switch flushes user translations, and "loaded elsewhere" means another
// processor's CR3 holds it now (`cr3_loaded_elsewhere`, asked after the entries
// changed).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::percpu::{self, MAX_CPUS};

/// One flush request after another, each with a number. A processor records
/// the newest number it has flushed for; the requester waits until every
/// other processor's record has reached its own
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The newest generation each processor has flushed for
static SEEN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// One requester at a time: `SEEN` is compared against a single number
static REQUESTER: AtomicBool = AtomicBool::new(false);

/// How long a requester waits for one processor before going on without it. A
/// processor that never answers is halted (a panic stopped it) or has had
/// interrupts off far too long; waiting for ever would hang every unmap on the
/// machine. Spins, not time: the TSC is not calibrated this early and a spin is
/// a few nanoseconds
const PATIENCE_SPINS: u64 = 200_000_000;

/// Flushes every other processor's TLB and waits until each has.
///
/// Called with no lock held that another processor could be spinning on with
/// interrupts off: that processor could not take the IPI, and the wait would
/// run out its patience
///
/// With interrupts off from taking `REQUESTER` to letting it go. A holder the
/// timer preempted would keep the lock while descheduled, and every other
/// processor arriving from a fault or a `fork` with interrupts off would spin
/// on it, with nothing left to run the holder
pub fn shootdown() {
    if percpu::online_cpus() < 2 {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(shootdown_here);
}

fn shootdown_here() {
    let me = percpu::current_index();
    // Another requester may be waiting on this processor while it waits for the
    // lock, so it answers them from the loop, not only from the interrupt,
    // which it may have masked
    while REQUESTER
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        answer(me);
        core::hint::spin_loop();
    }

    let wanted = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    SEEN[me].fetch_max(wanted, Ordering::SeqCst);
    crate::arch::x86_64::apic::send_ipi_all_but_self(crate::arch::x86_64::apic::VEC_IPI_TLB);

    for cpu in percpu::iter_online() {
        let i = cpu.cpu_index as usize;
        if i == me || i >= MAX_CPUS {
            continue;
        }
        let mut spins = 0u64;
        while SEEN[i].load(Ordering::Acquire) < wanted {
            spins += 1;
            if spins == PATIENCE_SPINS {
                crate::serial_println!("[tlb] cpu {} did not answer shootdown {}", i, wanted);
                break;
            }
            core::hint::spin_loop();
        }
    }
    REQUESTER.store(false, Ordering::Release);
}

/// Whether a processor other than this one has `cr3` loaded, the only kind that
/// can hold translations of it with no PCID. Asked after the entries changed: a
/// processor loading `cr3` later says so first (`Cpu::loaded_cr3`, stored
/// SeqCst before its `mov cr3`), and this fence orders the changed entries
/// before the look, so whoever this misses walks the new tables. One leaving
/// `cr3` may already say otherwise while it is still loaded; it is in the
/// scheduler then, and touches no user address before its `mov cr3` drops them
///
/// `tlbalways` on the boot line answers yes every time: a shootdown for every
/// change, for a machine where this answer is suspected
pub fn cr3_loaded_elsewhere(cr3: u64) -> bool {
    if always() {
        return true;
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    let me = percpu::current_index();
    percpu::iter_online()
        .any(|c| c.cpu_index as usize != me && c.loaded_cr3.load(Ordering::SeqCst) == cr3)
}

/// `tlbalways`, read from the boot line once: this is asked on every unmap
fn always() -> bool {
    use core::sync::atomic::AtomicU8;
    static FLAG: AtomicU8 = AtomicU8::new(0); // 0 not read yet, 1 no, 2 yes
    match FLAG.load(Ordering::Relaxed) {
        0 => {
            let on = crate::arch::x86_64::grub::cmdline_flag("tlbalways");
            if FLAG.swap(if on { 2 } else { 1 }, Ordering::Relaxed) == 0 && on {
                crate::serial_println!("[tlb] tlbalways: every unmap flushes every processor");
            }
            on
        }
        v => v == 2,
    }
}

/// The IPI's side: flush, then say which request that covered. The number is
/// read before the flush, so a request made during it is not claimed
pub fn on_ipi() {
    answer(percpu::current_index());
}

/// The same answer, from a processor spinning with interrupts off on a lock
/// whose holder may itself be waiting in `shootdown` (see `vmm::COW_LOCK`)
pub fn answer_pending() {
    answer(percpu::current_index());
}

fn answer(me: usize) {
    let wanted = GENERATION.load(Ordering::Acquire);
    if me < MAX_CPUS && SEEN[me].load(Ordering::Acquire) < wanted {
        x86_64::instructions::tlb::flush_all();
        SEEN[me].fetch_max(wanted, Ordering::Release);
    }
}
