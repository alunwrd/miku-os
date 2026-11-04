//! gpui on Mayland, MikuOS's display server.
//!
//! Mayland is the Wayland wire format over a socket of its own (`AF_MAYLAND`),
//! with shared memory named by a kernel handle instead of a file descriptor and
//! a keymap that is a table, not an xkb string. So the Wayland backend beside
//! this cannot be pointed at it, but it has the same shape: a surface with an
//! xdg toplevel role, shm buffers, a frame callback, and a seat with a keyboard
//! and a pointer. `lib/mayland` in the MikuOS tree is the protocol, shared with
//! the display server.
//!
//! MikuOS has no Vulkan or GL for wgpu, so `renderer` draws the scene into the
//! buffer on the processor.

mod client;
mod renderer;
mod window;

pub(crate) use client::MaylandClient;
