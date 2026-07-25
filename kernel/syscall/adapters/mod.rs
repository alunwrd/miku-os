// syscall/adapters - per-domain syscall handlers that translate the raw ABI
// into calls on the kernel subsystems (fs, io, net, process, memory, file)
pub mod file;
pub mod fs;
pub mod io;
pub mod memory;
pub mod net;
pub mod process;
