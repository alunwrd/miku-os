//  Block driver abstraction - the lower edge of the block layer
// 
//  Any storage device (ATA/IDE today, AHCI/NVMe/virtio-blk later) implements
//  'BlockDriver'. The block layer ('super') owns the concrete driver instances
//  behind a stable 'BlockDevId' and routes every read/write through them, so the
//  filesystems above never touch a device driver directly - same shape as
//  Linux's 'block_device' / 'request_queue' separation

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlkError {
    /// No device behind this id / device absent
    NoDevice,
    /// Caller buffer smaller than 'count * sector_size'
    BufferTooSmall,
    /// Device reported a fault / error bit
    DeviceFault,
    Timeout,
    ReadOnly,
    /// Operation not implemented by this device (e.g. discard on a
    /// drive without TRIM)
    Unsupported,
    /// Device has been taken offline after too many failures; requests
    /// fail fast instead of waiting out hardware timeouts
    Offline,
}

/// Geometry / identity reported by a device (filled from ATA IDENTIFY etc.)
#[derive(Clone, Copy)]
pub struct BlockDevInfo {
    pub sector_size:   u32,
    pub total_sectors: u64,
    pub model:         [u8; 40],
    pub model_len:     u8,
    pub lba48:         bool,
    pub read_only:     bool,
    /// Device accepts discard/TRIM (NVMe DSM deallocate, ATA TRIM,
    /// virtio-blk discard)
    pub discard:       bool,
}

impl BlockDevInfo {
    pub const fn unknown() -> Self {
        Self {
            sector_size:   512,
            total_sectors: 0,
            model:         [0; 40],
            model_len:     0,
            lba48:         false,
            read_only:     false,
            discard:       false,
        }
    }

    pub fn model_str(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_len as usize]).unwrap_or("")
    }
}

/// Device health snapshot (SMART / NVMe health log). Fields the backend
/// cannot report are left at their 'unknown' sentinel
#[derive(Clone, Copy)]
pub struct HealthInfo {
    /// Overall device verdict (SMART status / critical-warning flags)
    pub healthy: bool,
    /// Composite temperature in Celsius; i16::MIN = unknown
    pub temp_c: i16,
    /// NVMe "percentage used" wear estimate; 0xFF = unknown
    pub percent_used: u8,
    /// Power-on hours; u64::MAX = unknown
    pub power_on_hours: u64,
    /// Lifetime host data read / written in MiB; u64::MAX = unknown
    pub data_read_mb: u64,
    pub data_written_mb: u64,
}

impl HealthInfo {
    pub const fn unknown_ok() -> Self {
        Self {
            healthy: true,
            temp_c: i16::MIN,
            percent_used: 0xFF,
            power_on_hours: u64::MAX,
            data_read_mb: u64::MAX,
            data_written_mb: u64::MAX,
        }
    }
}

/// The contract every storage backend must satisfy. Units are 512-byte sectors;
/// 'lba' is 48-bit-capable ('u64') and 'count' is a full request length.
///
/// Methods take '&self': the block layer dispatches I/O concurrently without
/// holding a device-wide lock, so a driver must be internally synchronized.
/// Single-queue backends (ATA/AHCI/virtio) serialize on an internal lock;
/// NVMe spreads requests across several independent queues for real
/// parallelism. 'Sync' is therefore required as well as 'Send'
pub trait BlockDriver: Send + Sync {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError>;
    fn write_blocks(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError>;
    fn flush(&self) -> Result<(), BlkError>;
    fn info(&self) -> BlockDevInfo;
    /// SMART-style health report; None when the backend has no health source
    fn health(&self) -> Option<HealthInfo> {
        None
    }
    /// Tell the device the sector range no longer holds useful data
    /// (TRIM / deallocate). Contents of the range become indeterminate;
    /// the device may unmap it. Advisory - failure must not corrupt data
    fn discard(&self, _lba: u64, _count: u32) -> Result<(), BlkError> {
        Err(BlkError::Unsupported)
    }
    /// Zero the sector range without transferring data (NVMe Write Zeroes,
    /// virtio WRITE_ZEROES). Unlike 'discard', the range must read back as
    /// zeros afterwards. The block layer falls back to writing zero-filled
    /// buffers when the backend reports 'Unsupported'
    fn write_zeroes(&self, _lba: u64, _count: u32) -> Result<(), BlkError> {
        Err(BlkError::Unsupported)
    }
    /// Write sectors with Force Unit Access: the data is on stable media
    /// before the command completes, without flushing the whole device
    /// cache. Used for journal commit / barrier writes. The default is a
    /// correct (if heavier) fallback - a normal write followed by a full
    /// cache flush - which backends with a real FUA bit override
    fn write_blocks_fua(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        self.write_blocks(lba, count, buf)?;
        self.flush()
    }
}
