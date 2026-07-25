// kcore - kernel core services (named 'kcore' because Rust reserves 'core').
// boot-state tracking, power, timing, RNG, firmware handoff/probe/loading
pub mod boot_state;
pub mod clock;
pub mod firmware;
pub mod irq_lock;
pub mod power;
pub mod random;
pub mod sched_lock;
pub mod stack_guard;
pub mod time;
