// extfs: the ext2/ext3/ext4 on-disk format
//
// Superblock and group descriptors, inodes, directory entries, allocation
// bitmaps, extent trees, extended attributes, the JBD2 journal and replay, the
// CRC32c covering them, and an mkfs.
//
// The seam is `host::ExtHost`: sectors in, sectors out, plus a clock. Above it
// is format, below it the kernel's block layer or a `Vec<u8>`. `MikuFS<H>` is
// generic over it, so tests exercise the code that runs on the machine.

#![cfg_attr(not(test), no_std)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

#[macro_use]
pub mod plat;

/// What the crate needs from below, and an in-memory disk providing it,
/// including a volatile write cache so a journal can be tested against a crash
pub mod host;

pub mod error;
pub mod structs;

/// Sector-level view of the medium: the partition offset, the read-only
/// chokepoint, and the reads turning raw sectors into a superblock, group
/// descriptor table, or inode
pub mod reader;

/// Write-back staging for blocks the journal has not committed
pub mod cache;

/// The filesystem handle and what is common to the three flavors: geometry,
/// block and inode I/O, dirty tracking, sync
mod fs;

pub mod ext2;
pub mod ext3;
pub mod ext4;

pub mod trim;
pub mod xattr;

/// Laying down a fresh filesystem
pub mod mkfs;

/// Validating what is on the medium, and bringing a filesystem up from it
pub mod mount;

/// Checking a filesystem against itself, and repairing what disagrees
pub mod fsck;

pub use error::FsError;
pub use fs::MikuFS;
pub use host::{ExtHost, MemDisk};
