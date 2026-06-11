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
        }
    }

    pub fn model_str(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_len as usize]).unwrap_or("")
    }
}

/// The contract every storage backend must satisfy. Units are 512-byte sectors;
/// 'lba' is 48-bit-capable ('u64') and 'count' is a full request length
pub trait BlockDriver: Send {
    fn read_blocks(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError>;
    fn write_blocks(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError>;
    fn flush(&mut self) -> Result<(), BlkError>;
    fn info(&mut self) -> BlockDevInfo;
}
