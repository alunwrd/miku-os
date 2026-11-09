//! `<sys/statvfs.h>`, `<sys/statfs.h>`

#![allow(unused_imports)]

use super::prelude::*;

/// `statvfs`'s block and file counts
pub type fsblkcnt_t = u64;
pub type fsfilcnt_t = u64;
/// `statvfs` flags. MikuOS reports neither (there is no setuid bit and so no
/// `nosuid`), so a caller reads zero
pub const ST_RDONLY: c_ulong = 1;
pub const ST_NOSUID: c_ulong = 2;
c_structs! {
    /// `statvfs`, in POSIX's shape
    ///
    /// The syscall writes 48 bytes laid out in `kernel/syscall/abi.rs`;
    /// `lib/runtime/libmiku/libc/` translates. Four fields have no source here:
    ///
    ///   * `f_frsize` is `f_bsize`: the VFS has one block size.
    ///   * `f_bavail` is `f_bfree`: no blocks are reserved for root.
    ///   * `f_favail` has no field; `rustix` reads `f_ffree` for it.
    ///   * `f_fsid` is 0, no device numbers. See `fsid_t`.
    pub struct statvfs {
        pub f_bsize: c_ulong,
        pub f_frsize: c_ulong,
        pub f_blocks: u64,
        pub f_bfree: u64,
        pub f_bavail: u64,
        pub f_files: u64,
        pub f_ffree: u64,
        pub f_favail: u64,
        pub f_fsid: c_ulong,
        pub f_flag: c_ulong,
        pub f_namemax: c_ulong,
        /// The filesystem magic from `statfs`(36), which POSIX's `statvfs` has
        /// no place for. Tells ext2 from ext4 from tmpfs
        pub f_type: c_ulong,
    }

    /// `statfs`'s filesystem id. Both words are zero: MikuOS has no device
    /// numbers
    pub struct fsid_t {
        pub __val: [c_int; 2],
    }

    /// `statfs`, in Linux's shape
    ///
    /// `statfs`(36) answers a 48-byte structure of its own, translated in
    /// `lib/runtime/libmiku/libc/`. `f_frsize` is the block size, as on Linux.
    /// `f_bavail` is `f_bfree`, since nothing is reserved for root. `f_fsid`
    /// and `f_spare` are zero.
    pub struct statfs {
    pub f_type: i64,
    pub f_bsize: i64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: fsid_t,
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [i64; 4],
    }
}
