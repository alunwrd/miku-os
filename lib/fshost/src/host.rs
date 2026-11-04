// What a filesystem needs underneath it: a disk and a clock
//
// Addressed in 512-byte absolute sectors. Filesystem blocks are in
// `DiskReader`, which owns the partition offset and read-only flag.
//
// write          may stay in cache
//   write_sync     write-through, ordered against other write_sync calls
// write_barrier  on stable media at return (FUA, or write+flush)
// flush          drains the device
//
// The journal uses `write_sync` for descriptors and data, and `write_barrier`
// for its commit record.

use alloc::vec::Vec;

/// Device failed, or the operation is unsupported
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    /// Transfer did not complete
    Io,
    /// Operation not implemented
    Unsupported,
}

/// A block device backend. `&mut self`, so backends need no interior mutability
pub trait BlockHost {
    /// Reads `count` sectors from absolute sector `lba`. `buf` must be at least
    /// `count * 512` bytes, or the backend may return `HostError::Io`
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError>;

    /// Ordinary write; may be buffered until `flush`
    fn write_sectors(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError>;

    /// Write-through, ordered against other `write_sectors_sync` calls
    fn write_sectors_sync(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError>;

    /// On stable media at return
    fn write_sectors_barrier(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError>;

    /// Drain the device's own write cache
    fn flush(&mut self) -> Result<(), HostError>;

    /// Marks sectors unused. Default succeeds and does nothing
    fn discard(&mut self, _lba: u64, _count: u32) -> Result<(), HostError> {
        Ok(())
    }

    /// Capacity in sectors, or 0 if unknown. `mkfs` then binary-searches the
    /// last readable sector
    fn total_sectors(&self) -> u64 {
        0
    }

    /// Monotonic ticks; only differences are used
    fn ticks(&self) -> u64 {
        0
    }

    /// Ticks per second
    fn tick_hz(&self) -> u32 {
        1
    }

    /// Seconds since the Unix epoch, or `None` if the clock is unset. The
    /// caller then uses a tick-derived value
    fn wall_clock(&self) -> Option<u32> {
        None
    }
}

/// A borrowed host is a host, so `mkfs` can borrow a disk that is mounted
/// afterwards
impl<T: BlockHost + ?Sized> BlockHost for &mut T {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        (**self).read_sectors(lba, count, buf)
    }
    fn write_sectors(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        (**self).write_sectors(lba, count, buf)
    }
    fn write_sectors_sync(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        (**self).write_sectors_sync(lba, count, buf)
    }
    fn write_sectors_barrier(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        (**self).write_sectors_barrier(lba, count, buf)
    }
    fn flush(&mut self) -> Result<(), HostError> {
        (**self).flush()
    }
    fn discard(&mut self, lba: u64, count: u32) -> Result<(), HostError> {
        (**self).discard(lba, count)
    }
    fn total_sectors(&self) -> u64 {
        (**self).total_sectors()
    }
    fn ticks(&self) -> u64 {
        (**self).ticks()
    }
    fn tick_hz(&self) -> u32 {
        (**self).tick_hz()
    }
    fn wall_clock(&self) -> Option<u32> {
        (**self).wall_clock()
    }
}

/// In-memory image for tests, modeling a volatile write cache: `write_sectors`
/// goes to `pending`, `write_sectors_sync` and `write_sectors_barrier` bypass
/// it, `flush` commits it, and `crash()` drops it
pub struct MemDisk {
    data: Vec<u8>,
    /// Written but unflushed sectors, in issue order
    pending: Vec<(u64, Vec<u8>)>,
    /// Whether `write_sectors` is buffered. Off by default
    pub volatile: bool,
    /// Counters for test assertions
    pub reads: u64,
    pub writes: u64,
    pub flushes: u64,
    /// Fails the write after this many with `IoError`
    pub fail_after: Option<u64>,
    /// Write operations issued, per call
    pub write_ops: u64,
    /// When set, the power goes out after this many write operations
    pub crash_at: Option<u64>,
    /// Whether that has happened yet
    pub crashed: bool,
}

impl MemDisk {
    pub fn new(sectors: usize) -> Self {
        Self {
            data: alloc::vec![0u8; sectors * 512],
            pending: Vec::new(),
            volatile: false,
            reads: 0,
            writes: 0,
            flushes: 0,
            fail_after: None,
            write_ops: 0,
            crash_at: None,
            crashed: false,
        }
    }

    /// Wraps an existing image
    pub fn from_image(data: Vec<u8>) -> Self {
        Self {
            data,
            pending: Vec::new(),
            volatile: false,
            reads: 0,
            writes: 0,
            flushes: 0,
            fail_after: None,
            write_ops: 0,
            crash_at: None,
            crashed: false,
        }
    }

    pub fn sectors(&self) -> u64 {
        (self.data.len() / 512) as u64
    }

    /// Enables the volatile cache, after `mkfs` and mount so setup is durable
    pub fn set_volatile(&mut self, on: bool) {
        if !on {
            self.apply_pending();
        }
        self.volatile = on;
    }

    /// Drops the volatile cache, as a power cut does
    pub fn crash(&mut self) {
        self.pending.clear();
    }

    /// Cuts power after this many write operations. Later writes silently do
    /// nothing, and the cache is lost
    pub fn crash_at(&mut self, writes: u64) {
        self.crash_at = Some(writes);
        self.crashed = false;
    }

    /// Whether the cut point was reached
    pub fn did_crash(&self) -> bool {
        self.crashed
    }

    /// Accepts writes again and resets the counter; the medium stays
    pub fn power_on(&mut self) {
        self.crash_at = None;
        self.crashed = false;
        self.write_ops = 0;
    }

    /// Whether this write is past the cut; counts it otherwise
    fn power_cut(&mut self) -> bool {
        if let Some(limit) = self.crash_at {
            if self.write_ops >= limit {
                if !self.crashed {
                    self.pending.clear();
                    self.crashed = true;
                }
                return true;
            }
        }
        self.write_ops += 1;
        false
    }

    /// The image as bytes, for handing to a checker or writing to a file
    pub fn image(&self) -> &[u8] {
        &self.data
    }

    /// Image including the volatile cache
    pub fn image_flushed(&mut self) -> &[u8] {
        self.apply_pending();
        &self.data
    }

    /// Drops `count` sectors from `lba` out of the cache, so a write-through is
    /// not shadowed or overwritten by an older cached copy. Entries straddling
    /// the range are split
    fn invalidate(&mut self, lba: u64, count: u32) {
        let (lo, hi) = (lba, lba + count as u64);
        let mut kept: Vec<(u64, Vec<u8>)> = Vec::with_capacity(self.pending.len());
        for (plba, bytes) in core::mem::take(&mut self.pending) {
            let psecs = (bytes.len() / 512) as u64;
            let (plo, phi) = (plba, plba + psecs);
            if phi <= lo || plo >= hi {
                kept.push((plba, bytes));
                continue;
            }
            // Head: sectors below the range
            if plo < lo {
                let n = ((lo - plo) * 512) as usize;
                kept.push((plo, bytes[..n].to_vec()));
            }
            // Tail: sectors above it
            if phi > hi {
                let off = ((hi - plo) * 512) as usize;
                kept.push((hi, bytes[off..].to_vec()));
            }
        }
        self.pending = kept;
    }

    fn apply_pending(&mut self) {
        let staged = core::mem::take(&mut self.pending);
        for (lba, bytes) in staged {
            let start = lba as usize * 512;
            let end = start + bytes.len();
            if end <= self.data.len() {
                self.data[start..end].copy_from_slice(&bytes);
            }
        }
    }

    fn range(&self, lba: u64, count: u32) -> Result<(usize, usize), HostError> {
        let start = (lba as usize).checked_mul(512).ok_or(HostError::Io)?;
        let len = (count as usize).checked_mul(512).ok_or(HostError::Io)?;
        let end = start.checked_add(len).ok_or(HostError::Io)?;
        if end > self.data.len() {
            return Err(HostError::Io);
        }
        Ok((start, end))
    }

    /// Write straight to the medium, past any modelled cache
    fn write_through(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        if self.power_cut() {
            return Ok(());
        }
        if let Some(limit) = self.fail_after {
            if self.writes >= limit {
                return Err(HostError::Io);
            }
        }
        let (start, end) = self.range(lba, count)?;
        let n = end - start;
        if buf.len() < n {
            return Err(HostError::Io);
        }
        self.data[start..end].copy_from_slice(&buf[..n]);
        // Removes any older cached copy
        self.invalidate(lba, count);
        self.writes += count as u64;
        Ok(())
    }
}

impl BlockHost for MemDisk {
    fn total_sectors(&self) -> u64 {
        self.sectors()
    }

    /// Fixed wall clock, 2026-01-01 UTC. A zero `i_dtime` on an unlinked inode
    /// reads as an orphan list pointer to a checker
    fn wall_clock(&self) -> Option<u32> {
        Some(1_767_225_600)
    }

    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        let (start, end) = self.range(lba, count)?;
        let n = end - start;
        if buf.len() < n {
            return Err(HostError::Io);
        }
        buf[..n].copy_from_slice(&self.data[start..end]);
        // Reads see the cache, as on a real device
        for (plba, bytes) in self.pending.iter() {
            let pstart = *plba as usize * 512;
            let pend = pstart + bytes.len();
            // Overlap of [pstart,pend) with [start,end)
            let lo = pstart.max(start);
            let hi = pend.min(end);
            if lo < hi {
                buf[lo - start..hi - start].copy_from_slice(&bytes[lo - pstart..hi - pstart]);
            }
        }
        self.reads += count as u64;
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        if !self.volatile {
            return self.write_through(lba, count, buf);
        }
        if self.power_cut() {
            return Ok(());
        }
        if let Some(limit) = self.fail_after {
            if self.writes >= limit {
                return Err(HostError::Io);
            }
        }
        let (_, _) = self.range(lba, count)?;
        let n = count as usize * 512;
        if buf.len() < n {
            return Err(HostError::Io);
        }
        self.pending.push((lba, buf[..n].to_vec()));
        self.writes += count as u64;
        Ok(())
    }

    fn write_sectors_sync(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.write_through(lba, count, buf)
    }

    fn write_sectors_barrier(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.write_through(lba, count, buf)?;
        self.flushes += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), HostError> {
        if self.crashed {
            return Ok(());
        }
        self.apply_pending();
        self.flushes += 1;
        Ok(())
    }

    fn discard(&mut self, lba: u64, count: u32) -> Result<(), HostError> {
        // Really drops the data, to catch discards of needed blocks
        let (start, end) = self.range(lba, count)?;
        for b in &mut self.data[start..end] {
            *b = 0;
        }
        // Also drops it from the cache
        self.invalidate(lba, count);
        Ok(())
    }
}
