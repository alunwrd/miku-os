// btrfs: the on-disk format
//
// Shaped like `lib/extfs`: format and arithmetic here, the disk behind
// `fshost::BlockHost`, and a `cargo test` that builds and corrupts a volume.
//
// The specification is the kernel, vendored at `refsrc/linux/fs/btrfs`,
// especially `tree-checker.c`.
//
// Offsets in `structs.rs` came from the compiler over
// `include/uapi/linux/btrfs_tree.h`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod chunk;
pub mod commit;
pub mod error;
pub mod extent;
pub mod file;
pub mod fs_tree;
pub mod mount;
pub mod node;
pub mod shape;
pub mod space;
pub mod structs;
pub mod trans;
pub mod volume;

pub use error::BtrfsError;
pub use chunk::{Chunk, ChunkMap, Physical, Profile};
pub use mount::{check_superblock, Geometry, MountReject, Superblock};
pub use node::Node;
pub use structs::Key;
pub use file::Extent;
pub use fs_tree::{DirEntry, Inode, Root};
pub use volume::Volume;
