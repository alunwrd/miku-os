// Directory operations: mkdir, rmdir, readdir, rename

use super::MikuVFS;
use crate::fs::vfs::hash::name_hash;
use crate::fs::vfs::types::*;

impl MikuVFS {
    pub fn mkdir(&mut self, parent: usize, name: &str, mode: FileMode) -> VfsResult<usize> {
        Self::validate_name(name)?;
        let pid = self.effective_node(parent);

        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        // What is already there first, as on Linux: EEXIST before EROFS or
        // EACCES. `create_dir_all` walks every parent and treats "exists" as
        // done, so asking `/home` for write permission before noticing
        // `/home/miku` exists fails every save into a user's own home
        self.ensure_no_duplicate(pid, name)?;
        if self.is_readonly_fs(pid) {
            return Err(VfsError::ReadOnly);
        }
        self.check_dir_write(pid)?;

        let id = self.alloc_vnode()?;
        let ts = self.now();
        let applied_mode = mode.apply_umask(self.ctx.umask);

        self.init_vnode(id, 
            id as InodeId,
            pid as InodeId,
            name,
            VNodeKind::Directory,
            self.nodes[pid].fs_type,
            applied_mode,
            self.ctx.cred.euid,
            self.ctx.cred.egid,
            ts,
        );

        // The owner goes in with the creation, in one call, so the vnode and
        // the inode cannot hold different owners
        let disk_mode = applied_mode.0;
        let owner = (self.ctx.cred.euid as u32, self.ctx.cred.egid as u32);
        {
            let result = self.with_backing(pid, |fs, dir| {
                fs.create(
                    dir,
                    name,
                    VNodeKind::Directory,
                    disk_mode,
                    owner.0,
                    owner.1,
                    (0, 0),
                    None,
                )
            });
            match result {
                Some(Ok(meta)) => {
                    self.nodes[id].ext2_ino = meta.id.as_ino();
                    self.nodes[id].ext2_slot = self.nodes[pid].ext2_slot;
                    // Nothing in it yet, so every name it has is cached: a miss
                    // in it needs no disk (`path_resolve`)
                    self.nodes[id].children_loaded = true;
                }
                Some(Err(e)) => {
                    self.free_vnode(id);
                    return Err(e);
                }
                // tmpfs: the vnode is the directory
                None => {}
            }
        }

        if !self.nodes[pid].children.insert(name, id as InodeId) {
            // On the disk and not in the tree: take it back off
            let _ = self.with_backing(pid, |fs, dir| fs.unlink(dir, name, true));
            self.free_vnode(id);
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].nlinks += 1;
        self.nodes[pid].touch_mtime(ts);

        crate::trace_println!(
            "[vfs] mkdir '{}' id={} parent={} ext2_ino={}",
            name,
            id,
            pid,
            self.nodes[id].ext2_ino
        );
        crate::fs::vfs::notify::entry_event(pid, name, true, crate::fs::vfs::notify::IN_CREATE, 0);
        Ok(id)
    }

    pub fn rmdir(&mut self, cwd: usize, path: &str) -> VfsResult<()> {
        let id = self.resolve_path(cwd, path)?;

        if !self.nodes[id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if id == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        if self.has_backing(id) && !self.nodes[id].children_loaded {
            self.ensure_children_loaded(id)?;
        }

        if !self.is_dir_empty(id) {
            return Err(VfsError::NotEmpty);
        }

        let pid = self.nodes[id].parent as usize;
        self.check_unlink(pid, id)?;

        {
            let dir_name = self.nodes[id].get_name();
            let mut name_buf = [0u8; NAME_LEN];
            let nlen = dir_name.len().min(NAME_LEN);
            name_buf[..nlen].copy_from_slice(dir_name.as_bytes());
            let name_str = unsafe { core::str::from_utf8_unchecked(&name_buf[..nlen]) };
            if let Some(r) = self.with_backing(pid, |fs, dir| fs.unlink(dir, name_str, true)) {
                r?;
            }
        }

        let h = name_hash(self.nodes[id].get_name());
        self.nodes[pid].children.remove(h, id as InodeId);

        // Before the vnode goes: the name is read from it
        crate::fs::vfs::notify::entry_event(pid, self.nodes[id].get_name(), true, crate::fs::vfs::notify::IN_DELETE, 0);
        crate::fs::vfs::notify::forget(id, true, true);

        if self.nodes[pid].nlinks > 0 {
            self.nodes[pid].nlinks -= 1;
        }

        let ts = self.now();
        self.nodes[pid].touch_mtime(ts);
        self.nodes[id].nlinks = 0;
        self.free_vnode(id);

        crate::trace_println!("[vfs] rmdir '{}' id={}", path, id);
        Ok(())
    }

    /// Every entry of a directory, from the beginning. A directory with more
    /// entries than the buffer is truncated here; `readdir_at` reads the rest
    pub fn readdir(&mut self, dir_id: usize, entries: &mut [DirEntry]) -> VfsResult<usize> {
        self.readdir_at(dir_id, 0, entries)
    }

    /// Entries from `start` onwards, so a directory can be read in passes.
    ///
    /// `start` counts logical entries including `.` and `..`, which come first
    /// and in that order. The order of the rest is the children map's, stable
    /// while the directory is not modified, as POSIX guarantees; so a cursor is
    /// an integer.
    ///
    /// Returns how many were written. Zero means the end
    pub fn readdir_at(
        &mut self,
        dir_id: usize,
        start: usize,
        entries: &mut [DirEntry],
    ) -> VfsResult<usize> {
        if !self.valid_vnode(dir_id) {
            return Err(VfsError::NotFound);
        }
        if !self.nodes[dir_id].is_dir() {
            return Err(VfsError::NotDirectory);
        }

        if self.nodes[dir_id].fs_type.has_mounted_fs() && !self.nodes[dir_id].children_loaded {
            self.ensure_children_loaded(dir_id)?;
        }
        self.list_cached(dir_id, start, entries)
    }

    /// `readdir_at` of a directory whose names are all cached, under the VFS
    /// lock held to read (`with_vfs_read`). `Deferred` for one not complete
    /// yet: its listing is read from the filesystem with the lock held to write
    pub fn readdir_at_as(&self, dir_id: usize, start: usize, entries: &mut [DirEntry]) -> VfsResult<usize> {
        if !self.valid_vnode(dir_id) {
            return Err(VfsError::NotFound);
        }
        if !self.nodes[dir_id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.nodes[dir_id].fs_type.has_mounted_fs() && !self.nodes[dir_id].children_loaded {
            return Err(VfsError::Deferred);
        }
        self.list_cached(dir_id, start, entries)
    }

    /// The listing from `start`, from the cached children alone, shared by both
    /// `readdir_at`s. Changes only atomics (the counter, the resume point)
    fn list_cached(&self, dir_id: usize, start: usize, entries: &mut [DirEntry]) -> VfsResult<usize> {
        let eff = self.effective_node(dir_id);
        let mut count = 0usize;
        // Position in the directory as a whole, as against in the caller's
        // buffer
        let mut seen = 0usize;

        if seen >= start && count < entries.len() {
            entries[count].set(".", self.nodes[dir_id].ino(), VNodeKind::Directory, 0);
            count += 1;
        }
        seen += 1;
        if seen >= start && count < entries.len() {
            let parent_id = self.nodes[dir_id].parent as usize;
            let par = if self.valid_vnode(parent_id) {
                parent_id
            } else {
                dir_id
            };
            entries[count].set("..", self.nodes[par].ino(), VNodeKind::Directory, 1);
            count += 1;
        }
        seen += 1;

        // Where the last pass stopped, when this pass starts there and the
        // directory has not changed: no walk past the entries already listed
        // (`vfs::children`)
        let mut from = 0;
        if let Some(slot) = self.nodes[eff].children.resume_at(start) {
            if start >= seen {
                seen = start;
                from = slot;
            }
        }
        let mut resume = (seen, self.nodes[eff].children.slots.len());

        // By the entry's own name: a hard link's second name is not its vnode's
        // first
        for (slot, name, child_id) in self.nodes[eff].children.named_from(from) {
            self.dcache.listed_slots.bump();
            let cid = child_id as usize;
            if !self.valid_vnode(cid) {
                continue;
            }
            if count >= entries.len() {
                resume = (seen, slot);
                break;
            }
            // An invalid child is skipped above, before `seen` moves, so it has
            // no position: the numbering is over entries that exist, and a
            // second pass lands where the first stopped even with holes in the
            // table
            if seen < start { seen += 1; continue; }
            // The entry's position in the directory, not in this buffer
            entries[count].set(name, self.nodes[cid].ino(), self.nodes[cid].kind, seen as u32);
            seen += 1;
            count += 1;
            resume = (seen, slot + 1);
        }
        self.nodes[eff].children.stopped_at(resume.0, resume.1);

        Ok(count)
    }

    pub fn rename(&mut self, cwd: usize, old: &str, new_path: &str) -> VfsResult<()> {
        self.rename_at(cwd, old, cwd, new_path)
    }

    /// `rename` with each path relative to its own directory: `renameat`, where
    /// the two may come from different descriptors
    pub fn rename_at(
        &mut self,
        old_start: usize,
        old: &str,
        new_start: usize,
        new_path: &str,
    ) -> VfsResult<()> {
        // The entry the old path names, not following a symlink there (a rename
        // moves the link), and not the vnode's first name, which for a hard
        // link is another directory's (`names.rs`)
        let (old_pid, old_name, id) = self.resolve_entry(old_start, old)?;
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        let (new_pid, new_base) = self.split_path(new_start, new_path)?;
        Self::validate_name(new_base)?;

        if !self.nodes[new_pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.nodes[new_pid].fs_type != self.nodes[id].fs_type {
            return Err(VfsError::CrossDevice);
        }

        // A rename takes the name out of the old directory, which is a removal
        // and judged as one: the sticky bit on /tmp stops a rename as it stops
        // an unlink
        self.check_unlink(old_pid, id)?;
        if new_pid != old_pid {
            self.check_dir_write(new_pid)?;
        }

        // No moving a directory into itself
        if self.nodes[id].is_dir() {
            let mut cur = new_pid;
            loop {
                if cur == id {
                    return Err(VfsError::InvalidArgument);
                }
                let p = self.nodes[cur].parent as usize;
                if p == cur || cur == 0 {
                    break;
                }
                cur = p;
            }
        }

        // A rename replaces what it lands on, without which a safe install is
        // impossible: unlink-then-rename leaves a moment with no name at all.
        // POSIX has two rules about the victim, both about kind; whether a
        // victim directory is empty is the filesystem's answer.
        //
        // A whiteout is a file unlinked while open whose name the disk keeps
        // until its last close: replacing that name on the disk would free the
        // inode under its descriptors
        if self.nodes[new_pid].children.is_whiteout(new_base) {
            return Err(VfsError::Busy);
        }
        // Looked up, not only looked for in the cache: a victim the cache had
        // not met is still a file the rename replaces
        let victim = match self.lookup_child_or_load(new_pid, new_base) {
            // Two names of one file, or a name onto itself: POSIX has rename do
            // nothing and succeed
            Ok(c) if c == id => return Ok(()),
            Ok(c) => Some(c),
            Err(VfsError::NotFound) => None,
            Err(e) => return Err(e),
        };

        if let Some(v) = victim {
            if self.nodes[id].is_dir() != self.nodes[v].is_dir() {
                return Err(if self.nodes[v].is_dir() {
                    VfsError::IsDirectory
                } else {
                    VfsError::NotDirectory
                });
            }
            if self.nodes[v].flags.immutable {
                return Err(VfsError::PermissionDenied);
            }
            // Replacing a name removes one, judged as `unlink` judges it
            self.check_unlink(new_pid, v)?;
        }

        // On a filesystem, but one of the two parents is not: nothing on the
        // medium can move, and an in-memory rename alone would leave the tree
        // and the volume disagreeing
        if self.has_backing(id) && !(self.has_backing(old_pid) && self.has_backing(new_pid)) {
            return Err(VfsError::CrossDevice);
        }

        // Rename or move (which rewrites `..`) is the filesystem's choice. A
        // victim losing its last name loses its inode in the rename: pages of
        // it not written out go first, or a flush would write into whatever
        // reuses the inode (`writeback.rs`)
        if let Some(v) = victim {
            if self.nodes[v].nlinks <= 1 && !self.nodes[v].is_dir() {
                crate::fs::vfs::filecache::lock().discard(v);
            }
        }
        if let Some(r) = self.with_backing_pair(old_pid, new_pid, |fs, src, dst| {
            fs.rename(src, old_name, dst, new_base)
        }) {
            r?;
        }

        // The victim leaves the tree before the new name is inserted, so its
        // slot is the slot the insert takes. The disk has already dropped it,
        // inside the rename's transaction
        if let Some(v) = victim {
            self.nodes[new_pid].children.remove_by_name(new_base);
            self.drop_name(v, new_pid, new_base);
            self.nodes[v].nlinks = self.nodes[v].nlinks.saturating_sub(1);
            // The file the move replaced. Its directory hears only the
            // `IN_MOVED_TO` below, as on Linux; the file itself hears that it
            // is gone, if that was its last name
            if self.nodes[v].nlinks == 0 || self.nodes[v].is_dir() {
                crate::fs::vfs::notify::forget(v, true, self.nodes[v].is_dir());
            }
            // A mapping holds it as a descriptor does (`mm::mmap`)
            if self.nodes[v].nlinks == 0
                && self.nodes[v].refs() == 0
                && !crate::mm::mmap::is_mapped(v as u32)
            {
                self.free_file_pages(v);
                self.free_vnode(v);
            }
        }

        self.nodes[old_pid].children.remove_by_name(old_name);
        if self.has_backing(old_pid) {
            self.add_negative(old_pid, old_name);
        }
        self.move_name(id, old_pid, old_name, new_pid, new_base);

        if !self.nodes[new_pid].children.insert(new_base, id as InodeId) {
            // roll back in-memory changes and ext2 rename
            let rollback_name = old_name;
            self.move_name(id, new_pid, new_base, old_pid, rollback_name);
            let _ = self.nodes[old_pid]
                .children
                .insert(rollback_name, id as InodeId);
            // Undo it by the same route, with the pair reversed. The filesystem
            // picks rename or move again from that, so the two directions
            // cannot disagree
            if let Some(Err(e)) = self.with_backing_pair(new_pid, old_pid, |fs, dst, src| {
                fs.rename(dst, new_base, src, rollback_name)
            }) {
                crate::serial_println!(
                    "[vfs] rollback of rename to {:?} failed: {:?} - disk and tree now disagree",
                    rollback_name, e
                );
            }
            return Err(VfsError::NoSpace);
        }

        let ts = self.now();

        if new_pid != old_pid && self.nodes[id].is_dir() {
            if self.nodes[old_pid].nlinks > 0 {
                self.nodes[old_pid].nlinks -= 1;
            }
            // Unless a directory was replaced here: its own `..` already
            // counted for one, and it has gone. One leaves, one arrives
            if victim.is_none() {
                self.nodes[new_pid].nlinks = self.nodes[new_pid].nlinks.saturating_add(1);
            }
            self.nodes[old_pid].touch_mtime(ts);
        }

        self.nodes[id].touch_ctime(ts);
        self.nodes[new_pid].touch_mtime(ts);

        // One cookie pairs the two halves, so a watcher of both directories
        // can tell a move from a delete and an unrelated create
        if crate::fs::vfs::notify::active() {
            let cookie = crate::fs::vfs::notify::next_cookie();
            let is_dir = self.nodes[id].is_dir();
            crate::fs::vfs::notify::entry_event(old_pid, old_name, is_dir, crate::fs::vfs::notify::IN_MOVED_FROM, cookie);
            crate::fs::vfs::notify::entry_event(new_pid, new_base, is_dir, crate::fs::vfs::notify::IN_MOVED_TO, cookie);
            crate::fs::vfs::notify::raise(id, crate::fs::vfs::notify::IN_MOVE_SELF | if is_dir { crate::fs::vfs::notify::IN_ISDIR } else { 0 }, 0, None);
        }

        Ok(())
    }
}
