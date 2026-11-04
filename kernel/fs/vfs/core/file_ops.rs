// create/open/close/dup, read/write/seek/fsync, unlink, read procfs

use super::MikuVFS;
use crate::fs::vfs::address_space::AddressSpace;
use crate::fs::devfs;
use crate::fs::procfs;
use crate::fs::vfs::types::*;

impl MikuVFS {
    pub fn create_file(
        &mut self,
        parent: usize,
        name: &str,
        mode: FileMode,
    ) -> VfsResult<usize> {
        Self::validate_name(name)?;
        let pid = self.effective_node(parent);

        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.is_readonly_fs(pid) {
            return Err(VfsError::ReadOnly);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, name)?;

        let id = self.alloc_vnode()?;
        let ts = self.now();
        let applied_mode = mode.apply_umask(self.ctx.umask);

        self.init_vnode(id, 
            id as InodeId,
            pid as InodeId,
            name,
            VNodeKind::Regular,
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
        match self.with_backing(pid, |fs, dir| {
            fs.create(
                dir,
                name,
                VNodeKind::Regular,
                disk_mode,
                owner.0,
                owner.1,
                (0, 0),
                None,
            )
        }) {
            Some(Ok(meta)) => {
                self.nodes[id].ext2_ino = meta.id.as_ino();
                self.nodes[id].ext2_slot = self.nodes[pid].ext2_slot;
            }
            Some(Err(e)) => {
                self.free_vnode(id);
                return Err(e);
            }
            // tmpfs: the vnode is the file
            None => {}
        }

        if !self.nodes[pid].children.insert(name, id as InodeId) {
            {
                let fname = self.nodes[id].get_name();
                let mut fname_buf = [0u8; NAME_LEN];
                let flen = fname.len().min(NAME_LEN);
                fname_buf[..flen].copy_from_slice(&fname.as_bytes()[..flen]);
                let fname_str = unsafe { core::str::from_utf8_unchecked(&fname_buf[..flen]) };
                if let Some(Err(e)) =
                    self.with_backing(pid, |fs, dir| fs.unlink(dir, fname_str, false))
                {
                    crate::serial_println!(
                        "[vfs] rollback of create {:?} failed: {:?} - disk and tree now disagree",
                        fname_str, e
                    );
                }
            }
            self.free_vnode(id);
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].touch_mtime(ts);

        crate::trace_println!(
            "[vfs] create '{}' id={} parent={} ext2_ino={}",
            name,
            id,
            pid,
            self.nodes[id].ext2_ino
        );
        crate::fs::vfs::notify::entry_event(pid, name, false, crate::fs::vfs::notify::IN_CREATE, 0);
        Ok(id)
    }

    pub fn open(
        &mut self,
        cwd: usize,
        path: &str,
        flags: OpenFlags,
        mode: FileMode,
    ) -> VfsResult<usize> {
        crate::trace_println!("[vfs] open '{}' flags=0x{:x}", path, flags.0);

        let nofollow = flags.has(OpenFlags::NOFOLLOW);
        let id = self.resolve_or_create(cwd, path, flags, mode, nofollow)?;

        if flags.writable() && self.is_readonly_fs(id) && self.get_dev_type(id).is_none() {
            return Err(VfsError::ReadOnly);
        }

        // `/dev/net0` is the whole network card: one ethernet frame per read,
        // one per write, nothing between the program and the wire. It needs
        // `CAP_NET_RAW` as well as its 0600 mode
        if matches!(self.get_dev_type(id), Some(crate::fs::devfs::DevType::NetRaw))
            && !self.ctx.cred.has(crate::process::cred::CAP_NET_RAW)
        {
            return Err(VfsError::PermissionDenied);
        }

        // `/dev/tty` means the caller's terminal, and a process at none cannot
        // open it: ENXIO, as on Linux. Falling back to the console would put a
        // daemon's stray prompt on someone else's screen
        if matches!(self.get_dev_type(id), Some(crate::fs::devfs::DevType::Tty))
            && crate::io::input::user_stdin::ctty().is_none()
        {
            return Err(VfsError::NoTerminal);
        }

        // `/dev/ptmx` is asked, not held. Opening it makes a terminal and
        // returns its master node, as `pipe()` creates a nameless node. The
        // descriptor names one terminal for life, so `classify` routes it with
        // no per-open state
        let id = match self.get_dev_type(id) {
            Some(crate::fs::devfs::DevType::Ptmx) => {
                let Some(slot) = crate::io::tty::open_master() else {
                    // Every slot taken
                    return Err(VfsError::TooManyOpenFiles);
                };
                match self.master_vnode(slot) {
                    Some(v) => v,
                    None => {
                        crate::io::tty::abandon_master(slot);
                        return Err(VfsError::NotFound);
                    }
                }
            }
            // Its own node, by name. Mode 0 refuses this to everybody; this is
            // the second lock, as a master is given, not taken from its holder
            Some(crate::fs::devfs::DevType::PtyMaster { .. }) => {
                return Err(VfsError::PermissionDenied);
            }
            // The slave end, by name: a terminal that exists, opened by a
            // process its family rule admits, the one `tty(ATTACH)` asks, so
            // opening the slave is never a way round attaching
            Some(crate::fs::devfs::DevType::PtySlave { slot }) => {
                let Some(tty) = crate::io::tty::master_tty(slot as usize) else {
                    return Err(VfsError::NoTerminal);
                };
                if !crate::io::tty::may_use(crate::sched::current_pid(), tty) {
                    return Err(VfsError::PermissionDenied);
                }
                id
            }
            _ => id,
        };

        // Pages cached from a disk are checked against the disk's idea of the
        // file once per open: a write that went round the VFS shows here
        self.cache_validate(id);

        let fd = self.fds().alloc(id as InodeId, flags)?;
        self.nodes[id].inc_ref();

        crate::trace_println!(
            "[vfs] opened fd={} vnode={} refs={}",
            fd,
            id,
            self.nodes[id].refs()
        );
        Ok(fd)
    }

    /// `open` of a regular file or a directory that creates and truncates
    /// nothing, under the VFS lock held to read (`with_vfs_read`): the walk
    /// from the cache as `cred`, a descriptor, and a reference taken with an
    /// atomic. `Deferred` for anything else: a name not cached, a device (a
    /// terminal, `/dev/ptmx`: their opens do things), a disk file whose
    /// attributes are not known to be the filesystem's
    pub fn open_as(&self, cred: &Credentials, cwd: usize, path: &str, flags: OpenFlags) -> VfsResult<usize> {
        use super::path_resolve::{walk, walk_follow, Links, Reader};
        if flags.has(OpenFlags::CREATE) || flags.has(OpenFlags::TRUNCATE) {
            return Err(VfsError::Deferred);
        }
        let nofollow = flags.has(OpenFlags::NOFOLLOW);
        let mut r = Reader { vfs: self, cred };
        let id = if nofollow { walk(&mut r, cwd, path, Links::All)? } else { walk_follow(&mut r, cwd, path)? };
        if nofollow && self.nodes[id].is_symlink() {
            return Err(VfsError::Loop);
        }
        if flags.has(OpenFlags::DIRECTORY) && !self.nodes[id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if !self.nodes[id].is_regular() && !self.nodes[id].is_dir() {
            return Err(VfsError::Deferred);
        }
        self.check_access_as(cred, id, flags)?;
        if flags.writable() && self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if !self.cache_validate_fresh(id) {
            return Err(VfsError::Deferred);
        }
        let fd = self.fds().alloc(id as InodeId, flags)?;
        self.nodes[id].inc_ref();
        Ok(fd)
    }

    fn resolve_or_create(
        &mut self,
        cwd: usize,
        path: &str,
        flags: OpenFlags,
        mode: FileMode,
        nofollow: bool,
    ) -> VfsResult<usize> {
        let lookup = if nofollow {
            self.resolve_path(cwd, path)
        } else {
            self.resolve_path_follow(cwd, path)
        };

        match lookup {
            Ok(id) => {
                if nofollow && self.nodes[id].is_symlink() {
                    return Err(VfsError::Loop);
                }
                if flags.has(OpenFlags::DIRECTORY) && !self.nodes[id].is_dir() {
                    return Err(VfsError::NotDirectory);
                }
                if flags.has(OpenFlags::CREATE) && flags.has(OpenFlags::EXCLUSIVE) {
                    return Err(VfsError::AlreadyExists);
                }
                self.check_access(id, flags)?;
                if flags.has(OpenFlags::TRUNCATE)
                    && flags.writable()
                    && self.nodes[id].is_regular()
                {
                    // Refused before it is emptied: `open` asks about a
                    // read-only mount once this returns, too late for O_TRUNC
                    if self.is_readonly_fs(id) {
                        return Err(VfsError::ReadOnly);
                    }
                    if self.give_way(id) {
                        return Err(VfsError::Deferred);
                    }
                    self.truncate_file(id)?;
                }
                Ok(id)
            }
            Err(VfsError::NotFound) if flags.has(OpenFlags::CREATE) => {
                let (parent, name) = self.split_path(cwd, path)?;
                if self.give_way(parent) {
                    return Err(VfsError::Deferred);
                }
                self.create_file(parent, name, mode)
            }
            Err(e) => Err(e),
        }
    }

    /// Release one descriptor's hold on `vid`, whatever ended it.
    ///
    /// The last hold on a nameless vnode (a pipe, an inotify instance, a file
    /// unlinked while open) frees it, on every way a descriptor ends: `close`,
    /// the close-on-exec sweep, both exit paths, a descriptor dying in flight
    /// on a socket. `std` makes one such pipe per spawn to report a failed
    /// `exec`, and which of the child's `exec` and the parent's `close` goes
    /// last is a race
    pub fn drop_ref(&mut self, vid: usize) {
        if !self.valid_vnode(vid) || self.nodes[vid].refs() == 0 {
            return;
        }
        self.nodes[vid].dec_ref();
        // An inotify instance ends with its last descriptor (`VNode::dec_ref`)
        if !self.nodes[vid].active {
            self.free_vnode(vid);
            return;
        }

        // An unlinked file goes with its last holder. A mapping holds it too:
        // its last unmap frees it then (`reclaim::release_unmapped`)
        if self.nodes[vid].nlinks == 0
            && self.nodes[vid].refs() == 0
            && !crate::mm::mmap::is_mapped(vid as u32)
        {
            self.finish_unlinked(vid);
        }
    }

    pub fn close(&mut self, fd: usize) -> VfsResult<()> {
        let file = *self.fds().get(fd)?;
        let vid = file.vnode_id as usize;
        self.fds().close(fd)?;
        if crate::fs::vfs::notify::active() && self.valid_vnode(vid) && self.nodes[vid].is_regular() {
            let mask = if file.flags.writable() {
                crate::fs::vfs::notify::IN_CLOSE_WRITE
            } else {
                crate::fs::vfs::notify::IN_CLOSE_NOWRITE
            };
            crate::fs::vfs::notify::node_event(self, vid, mask);
        }
        // POSIX: closing any descriptor on a file drops every record lock this
        // process holds on it; see `vfs::lock`
        crate::fs::vfs::lock::release(crate::sched::current_tgid(), vid as _);

        self.drop_ref(vid);

        crate::trace_println!("[vfs] close fd={} vnode={}", fd, vid);
        Ok(())
    }

    /// `close` beside other readers (`with_vfs_read_only`), for a descriptor on
    /// a file or a directory: the descriptor, the record locks, the inotify
    /// event and the count need nothing the lock held alone guards. `Deferred`,
    /// the descriptor still open, for any other kind (a pipe end, a terminal:
    /// their counts do things). `Closed::Last`, the descriptor gone, for the
    /// last holder of a vnode that has to go with it (an unlinked file, a
    /// nameless one), which `drop_ref` finishes under the lock held alone; the
    /// reference it still has keeps it until then
    pub fn close_as(&self, fd: usize) -> VfsResult<Closed> {
        let file = crate::fs::vfs::fd::peek(fd).ok_or(VfsError::BadFd)?;
        let vid = file.vnode_id as usize;
        if !self.valid_vnode(vid) {
            return Err(VfsError::Deferred);
        }
        let node = &self.nodes[vid];
        if !node.active || !(node.is_regular() || node.is_dir()) {
            return Err(VfsError::Deferred);
        }
        // Another thread may have closed and reused the descriptor between the
        // look and here: what went is what is counted, all under the lock held
        // alone
        let gone = self.fds().close(fd)? as usize;
        if gone != vid {
            return Ok(Closed::Last(gone));
        }
        if crate::fs::vfs::notify::active() && node.is_regular() {
            let mask = if file.flags.writable() {
                crate::fs::vfs::notify::IN_CLOSE_WRITE
            } else {
                crate::fs::vfs::notify::IN_CLOSE_NOWRITE
            };
            crate::fs::vfs::notify::node_event(self, vid, mask);
        }
        crate::fs::vfs::lock::release(crate::sched::current_tgid(), vid as _);
        if node.refs() == 0 || node.dec_ref_quietly() {
            Ok(Closed::Done)
        } else {
            Ok(Closed::Last(vid))
        }
    }

    pub fn dup(&mut self, old_fd: usize) -> VfsResult<usize> {
        let file = *self.fds().get(old_fd)?;
        let new_fd = self.fds().alloc(file.vnode_id, dup_flags(file.flags, false))?;

        let vid = file.vnode_id as usize;
        if self.valid_vnode(vid) {
            self.nodes[vid].inc_ref();
        }

        let offset = file.offset;
        self.fds().get_mut(new_fd)?.offset = offset;

        Ok(new_fd)
    }

    /// `dup`, with the new descriptor the lowest free one at or above `floor`.
    ///
    /// For `fcntl(F_DUPFD)`: a shell moving a pipe end out of the way of fd
    /// 0/1/2 says where to put it, and `dup_to` closes whatever is at the
    /// target
    pub fn dup_from(&mut self, old_fd: usize, floor: usize, cloexec: bool) -> VfsResult<usize> {
        let file = *self.fds().get(old_fd)?;
        let new_fd = self.fds().alloc_from(floor, file.vnode_id, dup_flags(file.flags, cloexec))?;

        let vid = file.vnode_id as usize;
        if self.valid_vnode(vid) {
            self.nodes[vid].inc_ref();
        }
        let offset = file.offset;
        self.fds().get_mut(new_fd)?.offset = offset;
        Ok(new_fd)
    }

    pub fn dup_to(&mut self, old_fd: usize, new_fd: usize) -> VfsResult<usize> {
        let file = *self.fds().get(old_fd)?;

        if self.fds().get(new_fd).is_ok() {
            let _ = self.close(new_fd);
        }

        self.fds().alloc_at(new_fd, file.vnode_id, dup_flags(file.flags, false))?;

        let vid = file.vnode_id as usize;
        if self.valid_vnode(vid) {
            self.nodes[vid].inc_ref();
        }

        self.fds().get_mut(new_fd)?.offset = file.offset;

        Ok(new_fd)
    }

    pub fn read(&mut self, fd: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let file = *self.fds().get(fd)?;
        if !file.flags.readable() {
            return Err(VfsError::PermissionDenied);
        }

        let vid = file.vnode_id as usize;
        let offset = file.offset;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }
        if buf.is_empty() {
            return Ok(0);
        }

        if self.nodes[vid].fs_type == FsType::ProcFS {
            return self.read_procfs(fd, vid, offset, buf);
        }

        if let Some(dt) = self.get_dev_type(vid) {
            let n = devfs::dev_read(dt, buf, offset)?;
            self.fds().get_mut(fd)?.offset += n as u64;
            return Ok(n);
        }

        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }

        // From the pipe's ring, as `read(2)` takes them, and without waiting
        if crate::fs::vfs::pipe::is_end(vid) {
            return match crate::fs::vfs::pipe::try_read(vid, buf) {
                crate::fs::vfs::pipe::PipeRead::Data(n) => Ok(n),
                crate::fs::vfs::pipe::PipeRead::Eof => Ok(0),
                crate::fs::vfs::pipe::PipeRead::WouldBlock => Err(VfsError::WouldBlock),
            };
        }

        if self.has_backing(vid) {
            return self.read_backed_file(fd, vid, offset, buf);
        }

        // Static-backed file (embedded syslib): serve from the kernel image
        if let Some(sd) = self.nodes[vid].static_data {
            let n = Self::read_static(sd, offset, buf);
            if !self.nodes[vid].flags.no_atime {
                let ts = self.now();
                self.nodes[vid].touch_atime(ts);
            }
            self.fds().get_mut(fd)?.offset += n as u64;
            return Ok(n);
        }

        // tmpfs / page-cache
        let size = self.nodes[vid].size;
        if offset >= size {
            return Ok(0);
        }

        let avail = (size - offset) as usize;
        let to_read = buf.len().min(avail);
        let mut done = 0;

        while done < to_read {
            let file_off = offset as usize + done;
            let page_num = file_off / PAGE_SIZE;
            let page_off = file_off % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(to_read - done);

            match self.nodes[vid].addr_space.get_page(&mut self.page_cache, page_num) {
                Some(pid) => {
                    if let Some(data) = self.page_cache.get_page_data(pid) {
                        buf[done..done + chunk]
                            .copy_from_slice(&data[page_off..page_off + chunk]);
                    } else {
                        buf[done..done + chunk].fill(0);
                    }
                }
                None => {
                    buf[done..done + chunk].fill(0);
                }
            }
            done += chunk;
        }

        if !self.nodes[vid].flags.no_atime {
            let ts = self.now();
            self.nodes[vid].touch_atime(ts);
        }

        self.fds().get_mut(fd)?.offset += done as u64;
        Ok(done)
    }

    /// `read` of a tmpfs file or an embedded one, under the VFS lock held to
    /// read (`core::read_fd`): the pages copied, the offset moved by a swap
    /// from where it was (`fd::advance`), so two reads on one descriptor still
    /// come one after the other. `Deferred` for everything else, for a read
    /// another read on the descriptor overtook, and for a due access time: it
    /// moves on Linux's `relatime` rule (older than the last change, or a day
    /// old), and a reader may not move it
    pub fn read_as(&self, fd: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let (file, pos) = crate::fs::vfs::fd::peek_pos(fd).ok_or(VfsError::BadFd)?;
        if !file.flags.readable() {
            return Err(VfsError::PermissionDenied);
        }
        let vid = file.vnode_id as usize;
        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let node = &self.nodes[vid];
        if !node.is_regular()
            || node.fs_type != FsType::TmpFS
            || self.has_backing(vid)
            || crate::fs::vfs::pipe::is_end(vid)
        {
            return Err(VfsError::Deferred);
        }
        if !node.flags.no_atime {
            let now = self.now();
            if node.atime < node.mtime || node.atime < node.ctime || now.saturating_sub(node.atime) >= 86_400 {
                return Err(VfsError::Deferred);
            }
        }
        let offset = file.offset;
        let n = if let Some(sd) = node.static_data {
            Self::read_static(sd, offset, buf)
        } else {
            let size = node.size;
            if offset >= size {
                return Ok(0);
            }
            let to_read = buf.len().min((size - offset) as usize);
            let mut done = 0;
            while done < to_read {
                let file_off = offset as usize + done;
                let page_off = file_off % PAGE_SIZE;
                let chunk = (PAGE_SIZE - page_off).min(to_read - done);
                match node.addr_space.get_page_ro(&self.page_cache, file_off / PAGE_SIZE).and_then(|p| self.page_cache.page_data(p)) {
                    Some(data) => buf[done..done + chunk].copy_from_slice(&data[page_off..page_off + chunk]),
                    None => buf[done..done + chunk].fill(0),
                }
                done += chunk;
            }
            done
        };
        if !crate::fs::vfs::fd::advance(fd, file.vnode_id, pos, offset + n as u64) {
            return Err(VfsError::Deferred);
        }
        Ok(n)
    }

    /// Copy from a static (kernel-image) backing slice at a byte offset.
    /// Shared by the fd read, read_at_vnode and read_path paths
    fn read_static(sd: &'static [u8], offset: u64, buf: &mut [u8]) -> usize {
        let size = sd.len() as u64;
        if offset >= size {
            return 0;
        }
        let n = buf.len().min((size - offset) as usize);
        buf[..n].copy_from_slice(&sd[offset as usize..offset as usize + n]);
        n
    }

    /// Resolve an open fd to '(vnode_id, current file size)'. mmap stores
    /// the vnode id (not the fd) so the mapping survives the fd being closed
    pub fn fd_backing(&mut self, fd: usize) -> Option<(u32, u64)> {
        let file = *self.fds().get(fd).ok()?;
        let vid = file.vnode_id as usize;
        if !self.valid_vnode(vid) || self.nodes[vid].is_dir() {
            return None;
        }
        // Refresh ext-backed size from disk so the mapping sees the real EOF,
        // unless pages not written out yet make the VFS's the real one
        if !self.has_dirty_pages(vid) {
            if let Some(Ok(meta)) = self.with_backing(vid, |fs, node| fs.getattr(node)) {
                if meta.size > 0 {
                    self.nodes[vid].size = meta.size;
                }
            }
        }
        Some((vid as u32, self.nodes[vid].size))
    }

    /// Read up to 'buf.len()' bytes from a vnode at a byte offset, with no
    /// fd and no offset bookkeeping. This is the page-fault fill path for
    /// file-backed mmap: it must work from any process, given only the
    /// stable vnode id stored in the VMA. Handles ext-backed and tmpfs
    /// regular files; bytes past EOF are zero-filled by the caller
    pub fn read_at_vnode(&mut self, vid: usize, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        if !self.valid_vnode(vid) || buf.is_empty() {
            return Ok(0);
        }
        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }

        if self.cacheable(vid) {
            if let Some(n) = self.cache_copy(vid, offset, buf) {
                return Ok(n);
            }
            // Around the cache: what it holds dirty goes to the disk first
            self.settle(vid);
        }
        if let Some(r) = self.with_backing(vid, |fs, node| fs.read_at(node, offset, buf)) {
            return r;
        }

        // Static-backed file (embedded syslib)
        if let Some(sd) = self.nodes[vid].static_data {
            return Ok(Self::read_static(sd, offset, buf));
        }

        // tmpfs / page cache
        let size = self.nodes[vid].size;
        if offset >= size {
            return Ok(0);
        }
        let avail = (size - offset) as usize;
        let to_read = buf.len().min(avail);
        let mut done = 0;
        while done < to_read {
            let file_off = offset as usize + done;
            let page_num = file_off / PAGE_SIZE;
            let page_off = file_off % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(to_read - done);
            match self.nodes[vid].addr_space.get_page(&mut self.page_cache, page_num) {
                Some(pid) => {
                    if let Some(data) = self.page_cache.get_page_data(pid) {
                        buf[done..done + chunk].copy_from_slice(&data[page_off..page_off + chunk]);
                    } else {
                        buf[done..done + chunk].fill(0);
                    }
                }
                None => buf[done..done + chunk].fill(0),
            }
            done += chunk;
        }
        Ok(done)
    }

    /// The page of a tmpfs file at `offset`, as a frame a shared mapping can
    /// map: the file's own, so the mapping, `read`, `write` and every other
    /// mapping are one page. A hole becomes a page here, as a store through the
    /// mapping would make it. The reference taken is the mapping's, and goes
    /// back when it unmaps (`pmm::free_frame_cow`)
    ///
    /// `None` for anything but a tmpfs regular file: a mounted filesystem's
    /// pages are read into a copy per mapping, and a file in the kernel image
    /// is read-only
    pub fn shared_frame(&mut self, vid: usize, offset: u64) -> Option<u64> {
        if !self.valid_vnode(vid) || self.nodes[vid].is_dir() {
            return None;
        }
        if self.nodes[vid].is_ext_backed() || self.nodes[vid].static_data.is_some() {
            return None;
        }
        let page_num = offset as usize / PAGE_SIZE;
        let pid = match self.nodes[vid].addr_space.get_page(&mut self.page_cache, page_num) {
            Some(pid) => pid,
            None => {
                let pid = self.page_cache.alloc_page().ok()?;
                if self.nodes[vid].addr_space.set_page(&mut self.page_cache, page_num, pid).is_err() {
                    self.page_cache.free_page(pid);
                    return None;
                }
                pid
            }
        };
        let phys = self.page_cache.phys(pid)?;
        crate::mm::pmm::ref_inc(phys);
        Some(phys)
    }

    /// Write 'buf' to a vnode at a byte offset with no fd: the writeback path
    /// for dirtied MAP_SHARED pages on munmap/msync. Only regular files are
    /// writable this way
    pub fn write_at_vnode(&mut self, vid: usize, offset: u64, buf: &[u8]) -> VfsResult<usize> {
        if !self.valid_vnode(vid) || buf.is_empty() {
            return Ok(0);
        }
        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.is_readonly_fs(vid) {
            return Err(VfsError::ReadOnly);
        }
        // Static-backed files live in kernel image memory and are immutable
        if self.nodes[vid].static_data.is_some() || self.nodes[vid].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        if let Some(r) = self.with_backing(vid, |fs, node| fs.write_at(node, offset, buf)) {
            match &r {
                Ok(n) => self.cache_written(vid, offset, &buf[..*n], true),
                Err(_) => self.cache_written(vid, offset, &[], false),
            }
            return r;
        }

        // tmpfs: write straight into the page-cache pages backing the vnode
        let max = AddressSpace::max_size() as usize;
        let offset = offset as usize;
        if offset >= max {
            return Err(VfsError::FileTooLarge);
        }
        let to_write = buf.len().min(max - offset);
        let mut done = 0;
        while done < to_write {
            let file_off = offset + done;
            let page_num = file_off / PAGE_SIZE;
            let page_off = file_off % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(to_write - done);
            let pid = match self.nodes[vid].addr_space.get_page(&mut self.page_cache, page_num) {
                Some(pid) => pid,
                None => {
                    let pid = self.page_cache.alloc_page()?;
                    self.nodes[vid].addr_space.set_page(&mut self.page_cache, page_num, pid)?;
                    pid
                }
            };
            if let Some(page_data) = self.page_cache.get_page_data_mut(pid) {
                page_data[page_off..page_off + chunk].copy_from_slice(&buf[done..done + chunk]);
            } else {
                return Err(VfsError::IoError);
            }
            done += chunk;
        }
        if (offset + done) as u64 > self.nodes[vid].size {
            self.nodes[vid].size = (offset + done) as u64;
        }
        Ok(done)
    }

    /// Read an entire file by absolute path into an owned buffer, via normal
    /// VFS path resolution. Kernel-internal: needs no fd table, so it is safe
    /// during early boot. Handles ext-backed and tmpfs regular files. The
    /// firmware loading path (see `fwload.rs`)
    pub fn read_path(&mut self, path: &str) -> VfsResult<alloc::vec::Vec<u8>> {
        let vid = self.resolve_path_follow(0, path)?;
        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        // Read from the disk: what the cache holds dirty goes there first
        self.settle(vid);

        if let Some(r) = self.with_backing(vid, |fs, node| -> VfsResult<alloc::vec::Vec<u8>> {
            let size = fs.getattr(node)?.size as usize;
            let mut out = alloc::vec::Vec::new();
            out.try_reserve_exact(size).map_err(|_| VfsError::NoSpace)?;
            out.resize(size, 0);
            let mut done = 0usize;
            while done < size {
                let n = fs.read_at(node, done as u64, &mut out[done..])?;
                if n == 0 {
                    break;
                }
                done += n;
            }
            out.truncate(done);
            Ok(out)
        }) {
            return r;
        }

        // Static-backed file (embedded syslib)
        if let Some(sd) = self.nodes[vid].static_data {
            let mut out = alloc::vec::Vec::new();
            out.try_reserve_exact(sd.len()).map_err(|_| VfsError::NoSpace)?;
            out.extend_from_slice(sd);
            return Ok(out);
        }

        // tmpfs / page-cache backed file
        let size = self.nodes[vid].size as usize;
        let mut out = alloc::vec::Vec::new();
        out.try_reserve_exact(size).map_err(|_| VfsError::NoSpace)?;
        out.resize(size, 0);
        let mut done = 0usize;
        while done < size {
            let page_num = done / PAGE_SIZE;
            let page_off = done % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(size - done);
            if let Some(pid) = self.nodes[vid].addr_space.get_page(&mut self.page_cache, page_num) {
                if let Some(data) = self.page_cache.get_page_data(pid) {
                    out[done..done + chunk].copy_from_slice(&data[page_off..page_off + chunk]);
                }
            }
            done += chunk;
        }
        Ok(out)
    }

    /// Size of a file by absolute path, without reading its data.
    pub fn path_size(&mut self, path: &str) -> VfsResult<u64> {
        let vid = self.resolve_path_follow(0, path)?;
        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        self.settle(vid);
        if let Some(r) = self.with_backing(vid, |fs, node| fs.getattr(node).map(|m| m.size)) {
            return r;
        }
        Ok(self.nodes[vid].size)
    }

    /// Graft an ext-mounted directory onto a VFS path as a mount point: after
    /// this, resolving 'parent_path'/'name' descends into the ext filesystem at
    /// inode 'ext_ino'. Mirrors how 'cd' into an ext subtree attaches a vnode
    /// (see commands::fs). Mounts the root disk's /lib/firmware onto the VFS,
    /// so firmware reads take the normal path, as on Linux.
    pub fn graft_ext_dir(&mut self, parent_path: &str, name: &str, ext_ino: u32) -> VfsResult<usize> {
        let parent = self.resolve_path(0, parent_path)?;
        if !self.nodes[parent].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        // The slot first, then the flavour of that slot; a second question to
        // the mount table could answer for a different filesystem
        let slot = crate::fs::ext::mount::active_slot_index() as u8;
        let fs_type = crate::fs::ext::mount::fs_type_of(slot).unwrap_or(FsType::Ext2);

        // Grafting an ext directory means an ext filesystem is mounted and its
        // subtree must be lazily walkable: arm lazy_lookup_backed, otherwise
        // gated off (read_path/path_size would be NotFound on every file under
        // the graft, though the listing works via the cmd_ls ext2-direct
        // fallback). Makes fwload's /lib/firmware graft readable
        self.ext2_mount_active = true;

        // Re-grafting an existing mount point just refreshes the target inode.
        if let Ok(existing) = self.resolve_path(parent, name) {
            if self.nodes[existing].is_dir() {
                self.nodes[existing].ext2_ino = ext_ino;
                self.nodes[existing].ext2_slot = slot;
                self.nodes[existing].fs_type = fs_type;
                self.nodes[existing].children_loaded = false;
                self.nodes[existing].children.drop_negatives();
                return Ok(existing);
            }
        }

        let id = self.alloc_vnode()?;
        let ts = self.now();
        self.init_vnode(id, 
            id as InodeId,
            parent as InodeId,
            name,
            VNodeKind::Directory,
            fs_type,
            FileMode::new(0o755),
            0,
            0,
            ts,
        );
        self.nodes[id].ext2_ino = ext_ino;
        self.nodes[id].ext2_slot = slot;
        self.nodes[id].children_loaded = false;
        self.nodes[parent].children.insert(name, id as InodeId);
        Ok(id)
    }

    fn read_procfs(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        buf: &mut [u8],
    ) -> VfsResult<usize> {
        let mut name_copy = [0u8; NAME_LEN];
        let name_bytes = self.nodes[vid].get_name().as_bytes();
        let name_len = name_bytes.len().min(NAME_LEN);
        name_copy[..name_len].copy_from_slice(&name_bytes[..name_len]);
        let name_str = match core::str::from_utf8(&name_copy[..name_len]) {
            Ok(s) => s,
            Err(_) => return Err(VfsError::NotFound),
        };

        // Handed in: `proc_read` runs under the VFS lock, and taking it again
        // would deadlock
        let vfs_now = procfs::VfsNumbers {
            vnodes: self.total_vnodes(),
            fs_lookups: self.dcache.fs_lookups.get(),
            negative_hits: self.dcache.negative_hits.get(),
            complete_misses: self.dcache.complete_misses.get(),
            icache: self.icache.len(),
        };
        // As large as the biggest thing `proc_read` formats, `slabinfo` at
        // 8192. It clamps its answer to the buffer given and that length
        // becomes `total` below, so a shorter buffer ends the read, not splits
        // it across calls
        let mut proc_buf = [0u8; 8192];

        match procfs::proc_read(name_str, &mut proc_buf, &vfs_now) {
            Ok(total) => {
                let off = offset as usize;
                if off >= total {
                    return Ok(0);
                }
                let avail = total - off;
                let to_copy = buf.len().min(avail);
                buf[..to_copy].copy_from_slice(&proc_buf[off..off + to_copy]);
                self.fds().get_mut(fd)?.offset += to_copy as u64;
                Ok(to_copy)
            }
            Err(e) => Err(e),
        }
    }

    /// Write through a descriptor, and tell whoever watches the file. The write
    /// has several ways to finish; the event is raised here, once
    pub fn write(&mut self, fd: usize, data: &[u8]) -> VfsResult<usize> {
        let vid = self.fds().get(fd)?.vnode_id as usize;
        let n = self.write_data(fd, data)?;
        if n > 0 {
            crate::fs::vfs::notify::node_event(self, vid, crate::fs::vfs::notify::IN_MODIFY);
        }
        Ok(n)
    }

    fn write_data(&mut self, fd: usize, data: &[u8]) -> VfsResult<usize> {
        let file = *self.fds().get(fd)?;
        if !file.flags.writable() {
            return Err(VfsError::PermissionDenied);
        }

        let vid = file.vnode_id as usize;
        let is_append = file.flags.has(OpenFlags::APPEND);
        let is_sync = file.flags.has(OpenFlags::SYNC);
        let mut offset = file.offset as usize;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }
        if data.is_empty() {
            return Ok(0);
        }

        if self.nodes[vid].fs_type == FsType::ProcFS {
            return Err(VfsError::ReadOnly);
        }

        if let Some(dt) = self.get_dev_type(vid) {
            let n = devfs::dev_write(dt, data, offset as u64)?;
            self.fds().get_mut(fd)?.offset += n as u64;
            return Ok(n);
        }

        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.nodes[vid].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }
        if self.nodes[vid].flags.append_only && !is_append {
            offset = self.nodes[vid].size as usize;
        }

        if is_append {
            // O_APPEND means the end as it is now, not the end this vnode last
            // saw: another writer may have moved it. Unless pages not written
            // out yet put the end past the disk's
            if !self.has_dirty_pages(vid) {
                if let Some(Ok(meta)) = self.with_backing(vid, |fs, node| fs.getattr(node)) {
                    self.nodes[vid].size = meta.size;
                }
            }
            offset = self.nodes[vid].size as usize;
        }

        // A pipe's bytes go into its ring (`vfs::pipe`), never into pages: a
        // write through here (the kernel's own, a selftest's) must land where
        // `read(2)` takes them from. It does not wait: a full pipe is
        // `WouldBlock`, and `sys_write`'s own loop waits
        if crate::fs::vfs::pipe::is_end(vid) {
            return match crate::fs::vfs::pipe::try_write(vid, data) {
                crate::fs::vfs::pipe::PipeWrite::Wrote(n) => Ok(n),
                crate::fs::vfs::pipe::PipeWrite::Full => Err(VfsError::WouldBlock),
                crate::fs::vfs::pipe::PipeWrite::Broken => Err(VfsError::BrokenPipe),
            };
        }

        if self.has_backing(vid) {
            return self.write_backed_file(fd, vid, offset as u64, data);
        }

        let max = AddressSpace::max_size() as usize;
        if offset >= max {
            return Err(VfsError::FileTooLarge);
        }
        let available = max - offset;
        let to_write = data.len().min(available);

        let mut done = 0;
        while done < to_write {
            let file_off = offset + done;
            let page_num = file_off / PAGE_SIZE;
            let page_off = file_off % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(to_write - done);

            let pid = match self.nodes[vid].addr_space.get_page(&mut self.page_cache, page_num) {
                Some(pid) => pid,
                None => {
                    let pid = self.page_cache.alloc_page()?;
                    self.nodes[vid].addr_space.set_page(&mut self.page_cache, page_num, pid)?;
                    pid
                }
            };

            if let Some(page_data) = self.page_cache.get_page_data_mut(pid) {
                page_data[page_off..page_off + chunk]
                    .copy_from_slice(&data[done..done + chunk]);
            } else {
                return Err(VfsError::IoError);
            }

            done += chunk;
        }

        let new_end = offset + done;
        if new_end as u64 > self.nodes[vid].size {
            self.nodes[vid].size = new_end as u64;
        }

        let ts = self.now();
        self.nodes[vid].touch_mtime(ts);
        self.nodes[vid].flags.dirty = true;

        self.fds().get_mut(fd)?.offset = new_end as u64;

        if is_sync {
            self.nodes[vid].flags.dirty = false;
        }

        Ok(done)
    }

    pub fn seek(&mut self, fd: usize, whence: SeekFrom) -> VfsResult<u64> {
        let file = *self.fds().get(fd)?;
        let vid = file.vnode_id as usize;
        let current = file.offset;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }

        let size = self.nodes[vid].size;

        let new_offset: i64 = match whence {
            SeekFrom::Start(pos) => pos as i64,
            SeekFrom::Current(delta) => current as i64 + delta,
            SeekFrom::End(delta) => size as i64 + delta,
        };

        if new_offset < 0 {
            return Err(VfsError::SeekError);
        }

        self.fds().get_mut(fd)?.offset = new_offset as u64;
        Ok(new_offset as u64)
    }

    pub fn fsync(&mut self, fd: usize) -> VfsResult<()> {
        let file = *self.fds().get(fd)?;
        let vid = file.vnode_id as usize;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }

        self.settle(vid);
        let failed = {
            let mut fc = crate::fs::vfs::filecache::lock();
            if fc.entries[vid].wb_error.is_some() { fc.entries[vid].wb_error.take() } else { None }
        };
        if let Some(e) = failed {
            return Err(e);
        }
        if let Some(r) = self.with_backing(vid, |fs, node| fs.fsync(node)) {
            r?;
        }

        // A raw block-device node commits straight to its device: drain the
        // block layer's dirty cache for that disk and flush its write cache
        if let Some(crate::fs::devfs::DevType::Block { dev, .. }) = self.get_dev_type(vid) {
            crate::io::block::flush(dev).map_err(VfsError::from)?;
        }

        self.nodes[vid].flags.dirty = false;
        Ok(())
    }

    pub fn unlink(&mut self, cwd: usize, path: &str) -> VfsResult<()> {
        // The entry the path names (its directory and its name), not the
        // vnode's first name: they differ for a hard link (`names.rs`)
        let (pid, entry_name, id) = self.resolve_entry(cwd, path)?;

        if self.nodes[id].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        // check_unlink, not check_dir_write: a sticky directory is writable by
        // everyone so that everyone may create in it, and the bit stops them
        // removing what someone else created
        self.check_unlink(pid, id)?;
        if self.give_way(pid) {
            return Err(VfsError::Deferred);
        }

        // Its last name, still open or mapped: the name leaves the tree now and
        // the disk when the last descriptor closes or the last mapping goes
        // (`finish_unlinked`), and a whiteout hides it until then. Any other
        // name leaves the disk now; the inode keeps the rest
        let held = self.nodes[id].refs() > 0 || crate::mm::mmap::is_mapped(id as u32);
        let last = self.nodes[id].nlinks <= 1 && self.has_backing(pid);
        // A big file nothing holds goes the same way, and at once: its blocks
        // are freed after the lock, before this call returns (`unlinked.rs`)
        let drained = !held && last && self.unlink_drains(id);
        let deferred = (held || drained) && last;
        if !deferred {
            // Its last name: pages not written out go before the inode does
            // (`unlinked.rs`)
            if self.nodes[id].nlinks <= 1 {
                crate::fs::vfs::filecache::lock().discard(id);
            }
            if let Some(r) = self.with_backing(pid, |fs, dir| fs.unlink(dir, entry_name, false)) {
                r?;
            }
        }

        self.nodes[pid].children.remove_by_name(entry_name);
        if deferred {
            self.nodes[pid].children.insert_whiteout(entry_name);
        } else if self.has_backing(pid) {
            // Gone from the disk: the next lookup of it need not ask
            self.add_negative(pid, entry_name);
        }
        self.drop_name(id, pid, entry_name);

        let ts = self.now();
        self.nodes[pid].touch_mtime(ts);

        self.nodes[id].nlinks = self.nodes[id].nlinks.saturating_sub(1);
        if self.nodes[id].nlinks == 0 {
            // The name it goes with is this one, whichever it was first
            self.nodes[id].parent = pid as InodeId;
            self.nodes[id].name = NameBuf::from_str(entry_name);
            self.nodes[id].aliases.clear();
        }

        // The directory lost a name; the file lost itself if that was its
        // last one, and otherwise only a link
        crate::fs::vfs::notify::entry_event(pid, entry_name, false, crate::fs::vfs::notify::IN_DELETE, 0);
        if self.nodes[id].nlinks == 0 {
            crate::fs::vfs::notify::forget(id, true, false);
        } else {
            crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_ATTRIB);
        }

        crate::trace_println!(
            "[vfs] unlink '{}' id={} nlinks={} refs={}",
            path,
            id,
            self.nodes[id].nlinks,
            self.nodes[id].refs()
        );

        if self.nodes[id].nlinks == 0 && drained {
            self.finish_unlinked(id);
        } else if self.nodes[id].nlinks == 0 && !held {
            self.free_file_pages(id);
            self.free_vnode(id);
        } else {
            self.nodes[id].touch_ctime(ts);
        }

        Ok(())
    }
}

/// The flags a duplicate starts with: the open file's, with close-on-exec
/// decided by the call and not copied. POSIX clears it on `dup`, `dup2` and
/// `F_DUPFD` and sets it on `F_DUPFD_CLOEXEC`: it belongs to the descriptor,
/// not to what it names. A copy would make `dup2` of a close-on-exec pipe onto
/// a child's stdout close at `exec`
fn dup_flags(flags: OpenFlags, cloexec: bool) -> OpenFlags {
    if cloexec {
        OpenFlags(flags.0 | OpenFlags::CLOEXEC)
    } else {
        OpenFlags(flags.0 & !OpenFlags::CLOEXEC)
    }
}

/// What `close_as` left: nothing, or a reference for `drop_ref` to release
/// under the lock held alone, the last one of a vnode that has to go
pub enum Closed {
    Done,
    Last(usize),
}
