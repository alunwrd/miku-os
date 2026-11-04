// The kernel's outbound message queue
//
// The encoding is in the shared wire module; this adds a growable queue of
// encoded messages waiting for the transport to drain. Here, not in `wire`:
// clients are built without `alloc`

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use super::wire::{self, Arg, WireError};

/// The most a client may have waiting before it is killed
///
/// A client asking for the window list in a loop and never calling `may_recv`
/// would otherwise grow this until the kernel has no heap. Wayland answers the
/// same way: `wl_client` has a fixed 4 KiB ring, and a client whose events do
/// not fit is disconnected. Dropping is worse than killing: a client that
/// silently missed a `leave` or a `release` has wrong state for ever.
///
/// A quarter of a megabyte is far past anything legitimate: a frame queues a
/// few dozen bytes per surface
pub const MAX_QUEUED: usize = 256 * 1024;

/// An append-only byte queue of encoded messages
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub const fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    #[inline]
    pub fn len(&self) -> usize { self.buf.len() }

    #[inline]
    pub fn is_empty(&self) -> bool { self.buf.is_empty() }

    #[inline]
    pub fn bytes(&self) -> &[u8] { &self.buf }

    /// Hand the queued bytes to the transport and start a fresh queue
    pub fn take(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.buf)
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Encode one message onto the queue
    pub fn message(&mut self, sender: u32, opcode: u16, args: &[Arg])
        -> Result<(), WireError>
    {
        let size = wire::message_len(args);
        if size > wire::MAX_MESSAGE_SIZE {
            return Err(WireError::TooLarge);
        }
        // The caller turns this into a dead connection (see `Client::queue`).
        // The message is refused, not grown for: the client stopped reading
        if self.buf.len() + size > MAX_QUEUED {
            return Err(WireError::TooLarge);
        }
        let start = self.buf.len();
        self.buf.resize(start + size, 0);
        wire::encode(&mut self.buf[start..], sender, opcode, args)?;
        Ok(())
    }
}
