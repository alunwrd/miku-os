//! `sigdisp`(100): what a signal means to this process

/// Leave a signal at whatever the kernel does about it
pub const SIG_DISP_DEFAULT: u64 = 0;
/// Throw it away
pub const SIG_DISP_IGNORE: u64 = 1;
/// Deliver it to this process's handler
pub const SIG_DISP_CATCH: u64 = 2;
/// Changes nothing and returns the disposition set. Every other value returns
/// the one it replaced
pub const SIG_DISP_QUERY: u64 = 3;
/// Or'd into `SIG_DISP_CATCH`: the handler runs on the thread's alternate stack.
/// A query returns it when set
pub const SIG_DISP_ONSTACK: u64 = 4;

/// `uc_flags` bit the kernel sets on a frame for a fault it raised
pub const UC_MIKU_FAULT: u64 = 1;
