// Kernel core services (`kcore` since Rust reserves `core`): boot-state
// tracking, power, timing, RNG, firmware handoff/probe/loading
pub mod backtrace;
pub mod boot_state;
pub mod clock;
pub mod firmware;
pub mod irq_lock;
pub mod isr_event;
pub mod klog;
pub mod power;
pub mod pstore;
pub mod random;
pub mod sched_lock;
pub mod selftest;
pub mod stack_guard;
pub mod time;
pub mod words;
