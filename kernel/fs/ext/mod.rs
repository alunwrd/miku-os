// ext2/3/4 in the kernel: the mount table and the crate's names
//
// The driver is the `extfs` crate. It knows the on-disk format and nothing
// about this kernel: it reads and writes sectors through the `ExtHost` trait,
// which `fs::host::KernelHost` implements over the block layer. So
// `cd lib/extfs && cargo test` builds a filesystem in memory, corrupts it and
// checks the driver, on the code that runs on the machine.
//
// Here:
//
//   host    the block layer and the clock, behind the crate's trait
//   mount   the mount table: which devices are mounted where, probing
//           drives and partitions, grafting a filesystem onto the VFS
//
// The rest is re-exports, as `crate::fs::ext::structs::Superblock` and so on.

pub mod fsimpl;
pub mod mount;
pub mod selftest;

pub use extfs::{cache, ext2, ext3, ext4, fsck, mkfs, reader, structs, trim, xattr};
/// Superblock validation and the feature masks, under a name that does not
/// collide with this module's `mount` (the mount table)
pub use extfs::mount as mount_check;
pub use extfs::error::{self, FsError};

/// The filesystem handle, bound to the kernel's block layer.
///
/// `extfs::MikuFS` is generic over what it sits on; in the kernel it is always
/// this one.
pub type MikuFS = extfs::MikuFS<crate::fs::host::KernelHost>;

/// Reading and writing sectors, as the filesystem sees a device
pub type DiskReader = extfs::reader::DiskReader<crate::fs::host::KernelHost>;
