// Kernel I/O subsystems (block layer, console, input)
pub mod block;
pub mod console;
pub mod framebuffer;
pub mod input;
/// Terminals. tty0 is the console above; the rest are pseudo-terminals
pub mod tty;
