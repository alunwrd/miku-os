// What a filesystem sits on inside the kernel
//
// A format crate is written against `fshost::BlockHost`, so the same code runs
// here and under `cargo test` on the build host with a `Vec<u8>` underneath.
// This is the kernel half: a block device id, the block layer's four write
// flavours, and the clock.
//
// Geometry, validation, allocation, the journal and the checksums are in the
// crate, where a test can reach them.

use crate::fs::vfs::types::BlockDevId;
use fshost::{BlockHost, HostError};

/// A block device, as the filesystem sees it.
///
/// `Copy` and const-constructible: the mount table holds a fixed array of
/// filesystem slots.
#[derive(Clone, Copy)]
pub struct KernelHost {
    pub dev: BlockDevId,
}

impl KernelHost {
    pub const fn new(dev: BlockDevId) -> Self {
        Self { dev }
    }

    /// The placeholder in an unused mount slot. Every operation on it fails
    pub const fn invalid() -> Self {
        Self { dev: crate::fs::vfs::types::INVALID_U8 }
    }
}

/// The filesystem only needs to know the medium failed; the format crate
/// chooses which of its errors that becomes
#[inline]
fn io<T>(r: Result<T, crate::io::block::driver::BlkError>) -> Result<T, HostError> {
    r.map_err(|e| match e {
        crate::io::block::driver::BlkError::Unsupported => HostError::Unsupported,
        _ => HostError::Io,
    })
}

impl BlockHost for KernelHost {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        io(crate::io::block::read(self.dev, lba, count, buf))
    }

    fn write_sectors(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        io(crate::io::block::write(self.dev, lba, count, buf))
    }

    /// Write-through: ordered against other write-through writes on this
    /// device. The journal's descriptor and data records go this way
    fn write_sectors_sync(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        io(crate::io::block::write_sync(self.dev, lba, count, buf))
    }

    /// FUA where the device supports it, write-plus-flush otherwise. The
    /// journal commit record goes this way; crash consistency needs it durable
    /// when this returns
    fn write_sectors_barrier(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        io(crate::io::block::write_barrier(self.dev, lba, count, buf))
    }

    fn flush(&mut self) -> Result<(), HostError> {
        io(crate::io::block::flush(self.dev))
    }

    fn discard(&mut self, lba: u64, count: u32) -> Result<(), HostError> {
        io(crate::io::block::discard(self.dev, lba, count))
    }

    fn total_sectors(&self) -> u64 {
        crate::io::block::info(self.dev)
            .map(|i| i.total_sectors)
            .unwrap_or(0)
    }

    fn ticks(&self) -> u64 {
        crate::fs::procfs::uptime_ticks()
    }

    fn tick_hz(&self) -> u32 {
        crate::arch::x86_64::interrupts::PIT_HZ as u32
    }

    /// `None` when the clock was never set: the filesystem then stamps a
    /// tick-derived value, not 1970
    fn wall_clock(&self) -> Option<u32> {
        let w = crate::fs::procfs::wall_clock();
        if w > 0 { Some(w as u32) } else { None }
    }
}
