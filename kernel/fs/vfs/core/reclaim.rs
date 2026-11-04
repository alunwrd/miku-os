// Giving cached vnodes back (`dev/vfs-dentries-inodes.md`, "Stage 4: the table
// grows and shrinks")
//
// A vnode on a mounted filesystem is a cache of an inode. The table has no size
// (`vfs::table`): the cache takes what memory allows and gives it back when
// memory is wanted.
//
// When:
// - The vnode cache's shrinker (`VNODE_SHRINKER`, in `mm::shrink::all()`), when
//   frames are short.
// - Inside an operation, when a vnode cannot be allocated (`alloc_vnode`).
//
// What: the LRU's tail (`vfs::lru`), and for each vnode there a verdict:
// - evict: on a mounted filesystem, with an inode number, linked, not a mount
//   point, held by nothing, and a leaf of the cached tree;
// - busy for now, back to the head:
//   - a descriptor holds it, or a working directory (`change_cwd`), or a
//     mapping (`mm::mmap::is_mapped`), or a watch (`notify::is_watched`);
//   - it has cached children: they go first, and a directory whose last cached
//     child went is a leaf the next time round. Negative entries do not count,
//     and go with it;
//   - it has a whiteout: a child unlinked while open, whose name the disk keeps
//     until its last close (`names.rs`);
//   - a directory naming it has a descriptor open, which is being listed and
//     counts its `getdents` offset through the cached children;
//   - it was touched in the last `PROTECT_OPS` operations: whatever the running
//     operation holds, and a number a syscall carries from one `with_vfs` to
//     its next;
// - never, off the list: tmpfs, devfs and procfs, whose vnode is the only copy;
//   an unlinked file, freed by its last holder; a mount point.
//
// Evicting one forgets it: every directory naming it (one, or more for a hard
// link) loses the name and is marked not loaded, so the next listing asks the
// disk again and the next lookup of that name makes a fresh vnode.

extern crate alloc;
use alloc::vec::Vec;

use super::MikuVFS;
use crate::fs::vfs::types::*;
use crate::kcore::irq_lock::IrqMutex;

/// The last this many operations' vnodes are not evicted
const PROTECT_OPS: u64 = 1024;
/// Vnodes a reclaim looks at for each one asked for, before it stops for now:
/// what it rotates is busy, and a list of nothing but busy vnodes must not be
/// walked for ever
const LOOK_PER_WANT: usize = 4;

/// Vnodes whose last mapping went (`mm::mmap`): an unlinked one is freed
/// at the next operation, under the VFS lock the unmap did not hold
static UNMAPPED: IrqMutex<Vec<u32>> = IrqMutex::new(Vec::new());

/// From `mm::mmap`, under its own lock: `vid`'s file is mapped nowhere now
pub fn mapping_released(vid: u32) {
    UNMAPPED.lock().push(vid);
}

/// The vnode cache as a shrinker (`mm::shrink`). It counts and scans in
/// frames, a vnode being about a sixth of one
pub static VNODE_SHRINKER: crate::mm::shrink::Shrinker = crate::mm::shrink::Shrinker {
    name: "vnode cache",
    count: || super::try_vfs(|vfs| frames_of(vfs.lru.len())).unwrap_or(0),
    scan: |frames, direct| {
        let per = PAGE_SIZE / core::mem::size_of::<crate::fs::vfs::vnode::VNode>();
        let want = frames.saturating_mul(per).max(1);
        // A batch a hold: what kswapd asks for at once holds every other
        // filesystem call for 58-85 ms (`dev/vfs-locking.md`). One budget of
        // vnodes to look at for the whole request: a batch that met only busy
        // ones is not the end. A hold looks at a batch's worth too: what it
        // rotates is busy, and a walk's directories with children cached are
        // all busy
        let mut look = None;
        let mut freed = 0;
        while freed < want && look != Some(0) {
            let ask = (want - freed).min(RECLAIM_BATCH);
            let reclaim = |vfs: &mut MikuVFS| {
                let budget = look.get_or_insert_with(|| vfs.look_budget(want));
                let before = *budget;
                let mut this = (*budget).min(vfs.look_budget(ask));
                let got = vfs.reclaim_looking(ask, &mut this);
                let looked = (before.min(vfs.look_budget(ask))) - this;
                *budget -= looked;
                (got, looked)
            };
            let (got, looked) = if direct {
                match super::try_vfs(reclaim) {
                    Some(r) => r,
                    None => break,
                }
            } else {
                super::with_vfs(reclaim)
            };
            freed += got;
            if looked == 0 {
                // The list is empty
                break;
            }
        }
        frames_of(freed)
    },
};

/// Vnodes evicted in one hold of the VFS lock
const RECLAIM_BATCH: usize = 64;

fn frames_of(vnodes: usize) -> usize {
    vnodes * core::mem::size_of::<crate::fs::vfs::vnode::VNode>() / PAGE_SIZE
}

enum Verdict {
    Evict,
    Busy,
    Never,
}

impl MikuVFS {
    /// Evict up to `want` vnodes from the LRU's tail; how many went
    pub fn reclaim(&mut self, want: usize) -> usize {
        let mut look = self.look_budget(want);
        self.reclaim_looking(want, &mut look)
    }

    /// How many vnodes a request for `want` looks at before it stops: twice
    /// round the list at most. A directory rotated because it had children is a
    /// leaf the second time
    fn look_budget(&self, want: usize) -> usize {
        want.saturating_mul(LOOK_PER_WANT).saturating_add(64)
            .min(self.lru.len().saturating_mul(2).saturating_add(1))
    }

    /// `reclaim`, from a budget of looks shared by the batches of one
    /// request (`VNODE_SHRINKER`)
    fn reclaim_looking(&mut self, want: usize, look: &mut usize) -> usize {
        let mut freed = 0;
        while freed < want && *look > 0 {
            let Some(v) = self.lru.tail() else { break };
            *look -= 1;
            match self.verdict(v, true) {
                Verdict::Evict => {
                    self.evict(v);
                    freed += 1;
                }
                Verdict::Busy => self.lru.push_head(&mut self.nodes, v),
                Verdict::Never => self.lru.remove(&mut self.nodes, v),
            }
        }
        freed
    }

    /// `v` evicted now if nothing holds it, however recently it was used. For
    /// the selftest
    pub fn try_evict(&mut self, v: usize) -> bool {
        if !self.nodes[v].active {
            return false;
        }
        match self.verdict(v, false) {
            Verdict::Evict => {
                self.evict(v);
                true
            }
            _ => false,
        }
    }

    /// `recent`: whether the last `PROTECT_OPS` operations' vnodes are spared
    fn verdict(&self, v: usize, recent: bool) -> Verdict {
        let n = &self.nodes[v];
        if !n.active || !n.fs_type.has_mounted_fs() || n.ext2_ino == 0 || n.nlinks == 0 {
            return Verdict::Never;
        }
        let p = n.parent as usize;
        if p == v || !self.valid_vnode(p) || !self.nodes[p].fs_type.has_mounted_fs() {
            return Verdict::Never;
        }
        // Pages not written out yet hold it too. Asked with `try_lock`: a
        // shrinker can get here with the cache's lock held by its own caller,
        // and busy is the safe answer
        let dirty = crate::fs::vfs::filecache::FILE_CACHE.try_lock().map_or(true, |fc| fc.is_dirty(v));
        let held = n.refs() > 0
            || crate::mm::mmap::is_mapped(v as u32)
            || crate::fs::vfs::notify::is_watched(v)
            || dirty;
        // Not `flags.dirty`: the filesystem stores what a write or a change to
        // a directory sets, times included (`refresh_attrs_from_fs`), so a
        // vnode on a disk is never the only copy. Walked through by a reader
        // since the sweep last looked: a clock's second chance, cleared as it
        // is read
        let referenced = n.referenced.swap(false, core::sync::atomic::Ordering::Relaxed);
        if held
            || referenced
            || (recent && self.op.saturating_sub(n.used) < PROTECT_OPS)
            || self.dirs_of(v).any(|d| self.nodes[d].refs() > 0)
            || !n.children.is_empty()
            || n.children.whiteouts() > 0
        {
            return Verdict::Busy;
        }
        Verdict::Evict
    }

    /// Every name of it out of its directories, which can no longer answer a
    /// listing or a miss alone (`free_vnode`, `names.rs`)
    fn evict(&mut self, v: usize) {
        self.free_vnode(v);
    }

    /// Unlinked files whose last mapping went since the last operation, and
    /// that nothing else holds: freed now (`finish_unlinked`)
    pub(super) fn release_unmapped(&mut self) {
        let gone = {
            let mut q = UNMAPPED.lock();
            if q.is_empty() {
                return;
            }
            core::mem::take(&mut *q)
        };
        for vid in gone {
            let v = vid as usize;
            if self.valid_vnode(v)
                && self.nodes[v].refs() == 0
                && self.nodes[v].nlinks == 0
                && !crate::mm::mmap::is_mapped(vid)
            {
                self.finish_unlinked(v);
            }
        }
    }

    /// The calling process's working directory is `id` now: a reference on it,
    /// and the one on the old one given back
    pub fn change_cwd(&mut self, id: usize) {
        let tgid = crate::sched::current_tgid();
        self.nodes[id].inc_ref();
        if let Some(old) = self.cwds.insert(tgid, id as InodeId) {
            self.drop_ref(old as usize);
        }
        self.ctx.cwd = id as InodeId;
        crate::sched::set_current_cwd(id as u64);
    }
}
