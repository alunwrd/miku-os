// A file whose last name has gone, and which nothing holds
// (`dev/vfs-locking.md`, "A big file leaves the disk after the lock")
//
// `unlink` takes the name out of the tree at once. The disk releases the file
// (its name there, its blocks, its inode) when nothing holds it: at the
// `unlink`, or at the last close or the last unmap. For a big file that is the
// filesystem freeing every block, 37 ms for 64 MiB on ext4.
//
// A regular file on a disk at least `DRAIN_MIN` long goes in three steps:
// 1. under the lock (`finish_unlinked`): its name stays hidden behind the
//    whiteout, one reference keeps the vnode, and it goes on `PENDING`;
// 2. after the lock is released, by whoever released it (`drain`, from
//    `with_vfs`): the filesystem removes the name and frees the file, under its
//    own slot lock;
// 3. under the lock again (`drained`): the whiteout goes, a negative entry
//    takes its place, and the pages and the vnode are freed.
//
// While the whiteout is there the VFS refuses a create or a rename onto the
// name, so nothing else touches that disk entry in step 2.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use super::{MikuVFS, VFS_LOCK};
use crate::fs::vfs::fsops::NodeId;
use crate::fs::vfs::types::*;
use crate::kcore::sched_lock::SchedMutex;

/// A file this long or longer gives its blocks back after the lock: about 150
/// µs of ext4's freeing, below which the extra round trip is not worth it
pub(super) const DRAIN_MIN: u64 = 256 * 1024;

/// Step 2's work: which disk entry, and the vnode that waits for it
struct Drain {
    vid: usize,
    fs_type: FsType,
    slot: u8,
    dir: NodeId,
    name: String,
    ino: u32,
}

/// Pushed under `VFS_LOCK`, taken without it: a leaf
static PENDING: SchedMutex<Vec<Drain>> = SchedMutex::named("unlinked", Vec::new());

impl MikuVFS {
    /// An unlinked file nothing holds: off the disk and its vnode freed, now or
    /// after the lock for a big file on a disk
    pub(super) fn finish_unlinked(&mut self, vid: usize) {
        // Pages not written out go before the disk lets go of the inode: a
        // later flush would write into whatever reuses it
        crate::fs::vfs::filecache::lock().discard(vid);
        // Its name is still on the disk if `unlink` left a whiteout for it. It
        // is taken off only if it is still this file's: a rename onto it since
        // made it another file's. Without a whiteout the disk has released the
        // name already: a rename replaced it, or it was a directory
        let pid = self.nodes[vid].parent as usize;
        let name = String::from(self.nodes[vid].get_name());
        let named = pid != vid && self.valid_vnode(pid) && self.nodes[pid].children.is_whiteout(&name);
        if named && self.nodes[vid].is_regular() && self.nodes[vid].size >= DRAIN_MIN {
            if let Some((fs_type, slot, dir)) = self.backing_of(pid) {
                self.nodes[vid].refcount.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let ino = self.nodes[vid].ext2_ino;
                PENDING.lock().push(Drain { vid, fs_type, slot, dir, name, ino });
                return;
            }
        }
        if named {
            self.nodes[pid].children.remove_whiteout(&name);
            let ino = self.nodes[vid].ext2_ino;
            let ours = matches!(
                self.with_backing(pid, |fs, dir| fs.lookup(dir, &name)),
                Some(Ok(meta)) if meta.id.as_ino() == ino
            );
            if ours {
                match self.with_backing(pid, |fs, dir| fs.unlink(dir, &name, false)) {
                    Some(Err(e)) => {
                        crate::serial_println!("[vfs] deferred unlink of {:?} failed: {:?}", name, e)
                    }
                    _ => self.add_negative(pid, &name),
                }
            }
        }
        self.forget_unlinked(vid);
    }

    /// Step 3: the disk is done with `d`'s file, and `gone` says whether it
    /// took the name away
    fn drained(&mut self, d: &Drain, gone: bool) {
        let vid = d.vid;
        self.nodes[vid].refcount.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
        let pid = self.nodes[vid].parent as usize;
        if pid != vid && self.valid_vnode(pid) && self.nodes[pid].children.remove_whiteout(&d.name) && gone {
            self.add_negative(pid, &d.name);
        }
        self.forget_unlinked(vid);
    }

    fn forget_unlinked(&mut self, vid: usize) {
        crate::trace_println!("[vfs] deferred free vnode {}", vid);
        self.free_file_pages(vid);
        crate::fs::vfs::pipe::forget(vid);
        self.free_vnode(vid);
    }

    /// Whether `unlink` should leave a file nothing holds to `finish_unlinked`
    /// instead of asking the disk inline: a big one, whose freeing would hold
    /// the lock
    pub(super) fn unlink_drains(&self, id: usize) -> bool {
        self.nodes[id].is_regular() && self.nodes[id].size >= DRAIN_MIN
    }
}

/// Step 2 for every file waiting for it, then step 3. Called with no VFS lock
/// held, after each `with_vfs`, so the caller whose `close` or `unlink` let
/// the file go pays for it before going back to its program
pub(super) fn drain() {
    loop {
        let Some(d) = PENDING.lock().pop() else { return };
        let r = crate::fs::registry::with_mounted(d.fs_type, d.slot, |fs| {
            match fs.lookup(d.dir, &d.name) {
                Ok(meta) if meta.id.as_ino() == d.ino => fs.unlink(d.dir, &d.name, false).map(|()| true),
                Ok(_) | Err(VfsError::NotFound) => Ok(false),
                Err(e) => Err(e),
            }
        });
        let gone = match r {
            Some(Ok(gone)) => gone,
            Some(Err(e)) => {
                crate::serial_println!("[vfs] deferred unlink of {:?} failed: {:?}", d.name, e);
                false
            }
            None => false,
        };
        // Not `with_vfs`, whose epilogue is this function
        let _guard = VFS_LOCK.lock();
        let vfs = super::get_vfs();
        vfs.op += 1;
        vfs.drained(&d, gone);
    }
}
