// btrfs, read-only
//
// The format is in `lib/btrfs`, where `cargo test` builds a volume with the
// real `mkfs.btrfs` and reads it back. Here is the kernel half: a mount table
// and the `FileSystem` implementation.
//
// A btrfs write copies the fs-tree leaf and every node above it, allocates from
// the free-space tree, adds an extent item with a backref and checksum items,
// and lands all of it in one transaction that flips a superblock generation.
// `lib/btrfs` has that machinery (`trans.rs`, `commit.rs`); this driver does
// not call it.

pub mod fsimpl;
pub mod selftest;
pub mod mount;
