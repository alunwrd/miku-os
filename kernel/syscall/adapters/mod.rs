// syscall/adapters: per-domain syscall handlers translating the raw ABI into
// calls on the kernel subsystems (fs, io, net, process, memory, file)
pub mod file;
pub mod fs;
pub mod io;
pub mod memory;
pub mod gpu;
pub mod mayland;
pub mod net;
pub mod process;
pub mod admin;
pub mod thread;
pub mod tty;
pub mod notify;
