// Filesystem stack: the VFS core (with devfs/procfs/tmpfs), the ext2/3/4 family
// (ext), GPT partition parsing, and the in-kernel mkfs
//
// `ext` is a facade over the `extfs` crate, which holds the on-disk format and
// is testable on the build host. `mkfs` comes from the same crate, so one body
// of code describes the filesystem laid down and the one mounted
//
// `memfs` has no disk. It is a full implementation of the VFS's `FileSystem`
// trait in a map, so a second filesystem for VFS routing tests costs an
// allocation, not a second image and device
pub mod btrfs;
pub mod devfs;
pub mod ext;
pub mod registry;
pub mod procfs;
pub mod gpt;
pub mod memfs;
pub mod host;
pub use ext::mkfs;
pub mod tmpfs;
pub mod vfs;
pub mod vfs_read;
