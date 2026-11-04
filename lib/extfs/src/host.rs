// `ExtHost` is `fshost::BlockHost`, re-exported under the name used in this
// crate

pub use fshost::host::{BlockHost as ExtHost, HostError, MemDisk};

use crate::error::FsError;

/// A failure underneath becomes a failure here. `Unsupported` maps to
/// `UnsupportedFeature`, not `IoError`, so `fstrim` does not report a hardware
/// fault
impl From<HostError> for FsError {
    fn from(e: HostError) -> Self {
        match e {
            HostError::Io => FsError::IoError,
            HostError::Unsupported => FsError::UnsupportedFeature,
        }
    }
}
