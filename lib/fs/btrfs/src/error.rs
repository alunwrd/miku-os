// What can go wrong reading a btrfs volume

use fshost::HostError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtrfsError {
    /// The device did not complete a transfer
    Io,
    /// A structure on the medium is invalid, whether corrupted or hostile
    Corrupt,
    /// The format is valid, but not implemented here
    Unsupported,
    /// Asked for something absent
    NotFound,
    /// A checksum did not match its bytes
    ChecksumMismatch,
    /// The volume is mounted read-only
    ReadOnly,
    /// A buffer was too small, or a count did not fit
    InvalidArg,
    /// Ran out of memory building a structure
    OutOfMemory,
}

impl From<HostError> for BtrfsError {
    fn from(e: HostError) -> Self {
        match e {
            HostError::Io => BtrfsError::Io,
            HostError::Unsupported => BtrfsError::Unsupported,
        }
    }
}
