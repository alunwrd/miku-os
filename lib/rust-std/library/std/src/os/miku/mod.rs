//! MikuOS-specific extensions to the standard library.
//!
//! The parts of the system with no equivalent elsewhere: no `setuid` bit,
//! privilege granted by a broker, a session as a kernel object, descriptors
//! passed over a local socket by number. `std::os::unix` is `os/miku/unix`,
//! over the modules here.

#![unstable(feature = "miku_ext", issue = "none")]

pub mod ffi;
pub mod fs;
pub mod net;
pub mod process;
pub mod thread;
