// Input event sources: kernel keyboard buffer (stdin) and the userspace-facing
// stdin bridge
pub mod keyboard;
pub mod layout;
pub mod mods;
pub mod stdin;
pub mod user_stdin;
