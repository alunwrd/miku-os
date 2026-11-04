// Engines come up in a fixed order, and go down in the reverse one
//
// `gpuStateLoad_IMPL` (`gpu.c:2289`) walks the engines in the order of the
// chip's engine list, and `gpuStateUnload_IMPL` (`:3015`) walks it backwards.
// The engines, their order and what they are handed are the chip's
// (`blackwell/engines.rs`), so the context is a type parameter.

/// One engine: a name for the log, how it comes up, and how it goes down
pub trait Engine<C> {
    fn name(&self) -> &'static str;
    fn load(&self, cx: &mut C) -> Result<(), &'static str>;
    /// Undo `load`. Called for every engine, including one whose load failed,
    /// so it copes with nothing to undo, as OpenRM's `engstateStateUnload` runs
    /// over the whole list
    fn unload(&self, cx: &mut C);
}

/// Bring every engine up, in order. One failing is reported and the later ones
/// still load, the "skip, not fail" rule of the boot (`nvidia::autoboot`).
/// Returns how many came up
pub fn load_all<C>(engines: &[&dyn Engine<C>], cx: &mut C, say: &dyn Fn(&str)) -> usize {
    let mut up = 0;
    for e in engines {
        match e.load(cx) {
            Ok(()) => up += 1,
            Err(why) => crate::drivers::gpu::nvidia::said(crate::kcore::klog::Level::Warn, say,
                &alloc::format!("ENGINE {}: {} - the rest still load", e.name(), why)),
        }
    }
    up
}

/// Take every engine down, last loaded first
pub fn unload_all<C>(engines: &[&dyn Engine<C>], cx: &mut C, say: &dyn Fn(&str)) {
    for e in engines.iter().rev() {
        say(&alloc::format!("ENGINE {}: unloading", e.name()));
        e.unload(cx);
    }
}
