// What gives memory back when frames run short (`dev/kernel-core.md`, "Giving
// memory back: shrinkers")
//
// Linux's `register_shrinker`: each cache that keeps memory only to be quick
// says how much it could give (`count`) and gives about as much as it is asked
// (`scan`).
//
// Two callers:
// - kswapd, when free frames are under a sixteenth of the machine
//   (`pmm::running_low`): every shrinker in turn, each for a sixteenth of what
//   it holds, every pass;
// - an allocation that found no frames (`direct`): everything that can be had
//   without waiting. It may be under any lock (the VFS's, an interrupt-masking
//   one), so a shrinker takes only what it can `try_lock`, and one that would
//   have to wait gives nothing this time.

/// A cache that can give frames back
pub struct Shrinker {
    pub name: &'static str,
    /// Frames it could give back now; 0 when it cannot say without waiting
    pub count: fn() -> usize,
    /// Gives back about `frames`; returns how many went. `direct` when called
    /// from an allocation: wait for nothing
    pub scan: fn(frames: usize, direct: bool) -> usize,
}

/// Every shrinker, in the order they are asked: what is cheapest to have
/// again first
fn all() -> [&'static Shrinker; 4] {
    [
        &crate::fs::vfs::core::FILE_CACHE_SHRINKER,
        // Before the heap: the vnodes it frees empty slabs the heap then gives
        &crate::fs::vfs::core::VNODE_SHRINKER,
        &crate::mm::heap::SHRINKER,
        &crate::mm::kstack::SHRINKER,
    ]
}

/// From kswapd: when frames are short, each shrinker gives a sixteenth of
/// what it holds. Frames freed
pub fn run_if_low() -> usize {
    if !crate::mm::pmm::running_low() {
        return 0;
    }
    let mut freed = 0;
    for s in all() {
        let n = (s.count)();
        if n > 0 {
            freed += (s.scan)(n / 16 + 1, false);
        }
    }
    if freed > 0 {
        crate::trace_println!("[shrink] memory low: {} frames given back", freed);
    }
    freed
}

/// One direct reclaim at a time on the machine: a shrinker that allocates
/// and finds no frames either does not start another
static IN_DIRECT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// From an allocation that found no frames: whatever can be had without
/// waiting. Frames freed
pub fn direct() -> usize {
    use core::sync::atomic::Ordering;
    if IN_DIRECT.swap(true, Ordering::Acquire) {
        return 0;
    }
    let freed = all().iter().map(|s| (s.scan)(usize::MAX, true)).sum();
    IN_DIRECT.store(false, Ordering::Release);
    freed
}

/// What each shrinker holds, for `/proc/meminfo` and `info`
pub fn each(mut f: impl FnMut(&'static str, usize)) {
    for s in all() {
        f(s.name, (s.count)());
    }
}
