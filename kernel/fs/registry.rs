// Which filesystems this kernel has
//
// Two questions, the same one asked before and after a mount:
//
//   * which filesystem is on this device?   (`probe`)
//   * which filesystem answers for this mount slot?   (`with_mounted`)
//
// The shape is the one `kernel/io/block/driver.rs` uses for block drivers: a
// list. Adding a filesystem is a line in `PROBERS`, an arm in `with_mounted`,
// and an implementation of `FileSystem`.

use crate::fs::vfs::fsops::FileSystem;
use crate::fs::vfs::types::FsType;

/// What a prober found. The fields are what a person choosing a root needs to
/// see, hence `label`: `root=LABEL=miku` has to work before anything is
/// mounted.
#[derive(Clone, Copy)]
pub struct FsProbe {
    pub fs_type: FsType,
    pub drive: usize,
    /// Where the filesystem starts on that drive. 0 for a whole-disk
    /// filesystem, otherwise the partition's first LBA, as for every installed
    /// root
    pub start_lba: u32,
    pub block_size: u32,
    pub version: &'static str,
    pub label: [u8; 16],
    pub label_len: u8,
}

impl FsProbe {
    pub fn label_str(&self) -> &str {
        core::str::from_utf8(&self.label[..self.label_len as usize]).unwrap_or("")
    }
}

/// One filesystem's answer to "is this yours?".
///
/// A plain function pointer: a prober holds no state, runs before anything is
/// mounted, and the table has to be `static`
pub struct Prober {
    pub name: &'static str,
    pub probe: fn(drive: usize, start_lba: u32) -> Option<FsProbe>,
}

/// Every filesystem that can be recognised, in the order they are asked.
///
/// Order matters only if two probers could claim one volume, which would be a
/// bug in one of them: a magic number is unambiguous.
pub static PROBERS: &[Prober] = &[
    Prober { name: "ext", probe: crate::fs::ext::mount::probe_ext },
    Prober { name: "btrfs", probe: crate::fs::btrfs::mount::probe_btrfs },
];

/// What, if anything, is at `start_lba` on this drive.
pub fn probe(drive: usize, start_lba: u32) -> Option<FsProbe> {
    for p in PROBERS {
        if let Some(found) = (p.probe)(drive, start_lba) {
            return Some(found);
        }
    }
    None
}

/// Whether the lock `with_mounted` takes for this kind of filesystem is held
/// now: a call under the VFS lock that would wait for it there gives up first
/// (`MikuVFS::give_way`)
pub fn busy(fs_type: FsType) -> bool {
    match fs_type {
        FsType::Ext2 | FsType::Ext3 | FsType::Ext4 => crate::fs::ext::mount::STATE.is_locked(),
        FsType::Btrfs => crate::fs::btrfs::mount::STATE.is_locked(),
        _ => false,
    }
}

/// Until that lock is free, without keeping it
pub fn wait_idle(fs_type: FsType) {
    match fs_type {
        FsType::Ext2 | FsType::Ext3 | FsType::Ext4 => drop(crate::fs::ext::mount::STATE.lock()),
        FsType::Btrfs => drop(crate::fs::btrfs::mount::STATE.lock()),
        _ => {}
    }
}

/// Run `f` against the filesystem mounted in `slot` of `fs_type`'s table.
///
/// The one place that maps a mount to the code that serves it. A vnode carries
/// the type and the slot; what those index into is decided here.
///
/// `None` means the slot holds nothing, which is not an error: the vnode may
/// have no filesystem behind it
pub fn with_mounted<R>(
    fs_type: FsType,
    slot: u8,
    f: impl FnOnce(&mut dyn FileSystem) -> R,
) -> Option<R> {
    match fs_type {
        FsType::Ext2 | FsType::Ext3 | FsType::Ext4 => {
            crate::fs::ext::mount::with_ext2_for(slot, |fs| f(fs))
        }
        FsType::Btrfs => crate::fs::btrfs::mount::with_btrfs_for(slot, |fs| f(fs)),
        // In memory, and still a mount: a table, a slot and a filesystem in it,
        // so nothing above this line can tell it apart from one with a disk
        FsType::MemFs => crate::fs::memfs::with_memfs_for(slot, |fs| f(fs)),
        // tmpfs, devfs and procfs are not mounted filesystems: their contents
        // are the vnode table itself. See `dev/fs-architecture.md`
        FsType::TmpFS | FsType::DevFS | FsType::ProcFS => None,
    }
}
