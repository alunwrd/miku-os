// fatfs: FAT32, laid down and filled once
//
// The EFI system partition of an installed disk: `format`, then `mkdir` and
// `write_file`, then `finish`. Names are 8.3. There is no mount and no read
// path: firmware and bootloaders are the readers.
//
// The seam is `fshost::BlockHost`, as in `extfs`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod layout;
pub mod name;
mod volume;

pub use fshost::{BlockHost, HostError, MemDisk};
pub use volume::{format, Dir, Volume};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    /// Fewer clusters than FAT32 has by definition
    TooSmall,
    /// More sectors than a FAT32 volume counts, or a file over 4 GiB
    TooBig,
    /// Not an 8.3 name
    BadName,
    /// The directory already has that name
    Exists,
    /// The directory's one cluster is full
    DirFull,
    /// No clusters left
    NoSpace,
    /// The device failed
    Io,
}

impl From<HostError> for FatError {
    fn from(_: HostError) -> Self {
        FatError::Io
    }
}
