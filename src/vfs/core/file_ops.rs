// create/open/close/dup, read/write/seek/fsync, unlink, read procfs

use super::MikuVFS;
use crate::vfs::address_space::AddressSpace;
use crate::vfs::devfs;
use crate::vfs::procfs;
use crate::vfs::types::*;

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

        self.nodes[id].init(
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

        if self.nodes[pid].is_ext_backed() {
            let parent_ino = self.nodes[pid].ext2_ino;
            let disk_mode = applied_mode.0;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_create_file(parent_ino, name, disk_mode)
            });
            match result {
                Some(Ok(new_ino)) => {
                    self.nodes[id].ext2_ino = new_ino;
                }
                Some(Err(_)) | None => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
            }
        }

        if !self.nodes[pid].children.insert(name, id as InodeId) {
            if self.nodes[id].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let fname = self.nodes[id].get_name();
                let mut fname_buf = [0u8; NAME_LEN];
                let flen = fname.len().min(NAME_LEN);
                fname_buf[..flen].copy_from_slice(&fname.as_bytes()[..flen]);
                let fname_str = unsafe { core::str::from_utf8_unchecked(&fname_buf[..flen]) };
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_file(parent_ino, fname_str)
                });
            }
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!(
            "[vfs] create '{}' id={} parent={} ext2_ino={}",
            name,
            id,
            pid,
            self.nodes[id].ext2_ino
        );
        Ok(id)
    }

    pub fn open(
        &mut self,
        cwd: usize,
        path: &str,
        flags: OpenFlags,
        mode: FileMode,
    ) -> VfsResult<usize> {
        crate::serial_println!("[vfs] open '{}' flags=0x{:x}", path, flags.0);

        let nofollow = flags.has(OpenFlags::NOFOLLOW);
        let id = self.resolve_or_create(cwd, path, flags, mode, nofollow)?;

        if flags.writable() && self.is_readonly_fs(id) && self.get_dev_type(id).is_none() {
            return Err(VfsError::ReadOnly);
        }

        let fd = self.fds().alloc(id as InodeId, flags)?;
        self.nodes[id].inc_ref();

        crate::serial_println!(
            "[vfs] opened fd={} vnode={} refs={}",
            fd,
            id,
            self.nodes[id].refcount
        );
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
                    self.truncate_file(id);
                }
                Ok(id)
            }
            Err(VfsError::NotFound) if flags.has(OpenFlags::CREATE) => {
                let (parent, name) = self.split_path(cwd, path)?;
                self.create_file(parent, name, mode)
            }
            Err(e) => Err(e),
        }
    }

    pub fn close(&mut self, fd: usize) -> VfsResult<()> {
        let vid = self.fds().get(fd)?.vnode_id as usize;
        self.fds().close(fd)?;

        if self.valid_vnode(vid) && self.nodes[vid].refcount > 0 {
            self.nodes[vid].dec_ref();

            if self.nodes[vid].nlinks == 0 && self.nodes[vid].refcount == 0 {
                if self.nodes[vid].is_ext_backed() {
                    let pid = self.nodes[vid].parent as usize;
                    if self.valid_vnode(pid) && self.nodes[pid].ext2_ino != 0 {
                        let parent_ino = self.nodes[pid].ext2_ino;
                        let file_name = self.nodes[vid].get_name();
                        let mut name_buf = [0u8; NAME_LEN];
                        let nlen = file_name.len().min(NAME_LEN);
                        name_buf[..nlen].copy_from_slice(file_name.as_bytes());
                        let name_str =
                            unsafe { core::str::from_utf8_unchecked(&name_buf[..nlen]) };
                        let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                            fs.ext3_delete_file(parent_ino, name_str)
                        });
                    }
                }

                crate::serial_println!("[vfs] deferred free vnode {}", vid);
                self.free_file_pages(vid);
                self.nodes[vid].active = false;
                if vid < self.vnode_free_hint {
                    self.vnode_free_hint = vid;
                }
            }
        }

        crate::serial_println!("[vfs] close fd={} vnode={}", fd, vid);
        Ok(())
    }

    pub fn dup(&mut self, old_fd: usize) -> VfsResult<usize> {
        let file = *self.fds().get(old_fd)?;
        let new_fd = self.fds().alloc(file.vnode_id, file.flags)?;

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

        self.fds().alloc_at(new_fd, file.vnode_id, file.flags)?;

        let vid = file.vnode_id as usize;
        if self.valid_vnode(vid) {
            self.nodes[vid].inc_ref();
        }

        self.fds().get_mut(new_fd)?.offset = file.offset;

        Ok(new_fd)
    }

    pub fn read(&mut self, fd: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let file = self.fds().get(fd)?;
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

        if self.nodes[vid].is_ext_backed() {
            return self.read_ext2_file(fd, vid, offset, buf);
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

            match self.nodes[vid].addr_space.get_page(page_num) {
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

    /// Resolve an open fd to '(vnode_id, current file size)'. mmap stores
    /// the vnode id (not the fd) so the mapping survives the fd being closed
    pub fn fd_backing(&mut self, fd: usize) -> Option<(u32, u64)> {
        let file = self.fds().get(fd).ok()?;
        let vid = file.vnode_id as usize;
        if !self.valid_vnode(vid) || self.nodes[vid].is_dir() {
            return None;
        }
        // Refresh ext-backed size from disk so the mapping sees the real EOF
        if self.nodes[vid].is_ext_backed() {
            let ino = self.nodes[vid].ext2_ino;
            if let Some(sz) = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.read_inode(ino).map(|i| i.size()).unwrap_or(0)
            }) {
                if sz > 0 { self.nodes[vid].size = sz; }
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

        if self.nodes[vid].is_ext_backed() {
            let ext2_ino = self.nodes[vid].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                let inode = fs.read_inode(ext2_ino).map_err(|_| VfsError::IoError)?;
                let size = inode.size() as u64;
                if offset >= size {
                    return Ok(0usize);
                }
                let avail = (size - offset) as usize;
                let to_read = buf.len().min(avail);
                fs.read_file(&inode, offset, &mut buf[..to_read]).map_err(|_| VfsError::IoError)
            });
            return match result {
                Some(Ok(n)) => Ok(n),
                Some(Err(e)) => Err(e),
                None => Err(VfsError::IoError),
            };
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
            match self.nodes[vid].addr_space.get_page(page_num) {
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

    /// Write 'buf' to a vnode at a byte offset with no fd - the writeback
    /// path for dirtied MAP_SHARED pages on munmap/msync. Only regular
    /// files are writable this way
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

        if self.nodes[vid].is_ext_backed() {
            let ext2_ino = self.nodes[vid].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext2_write_file(ext2_ino, buf, offset).map_err(|_| VfsError::IoError)
            });
            return match result {
                Some(Ok(n)) => Ok(n),
                Some(Err(e)) => Err(e),
                None => Err(VfsError::IoError),
            };
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
            let pid = match self.nodes[vid].addr_space.get_page(page_num) {
                Some(pid) => pid,
                None => {
                    let pid = self.page_cache.alloc_page()?;
                    self.nodes[vid].addr_space.set_page(page_num, pid)?;
                    pid
                }
            };
            if let Some(page_data) = self.page_cache.get_page_data_mut(pid) {
                page_data[page_off..page_off + chunk].copy_from_slice(&buf[done..done + chunk]);
                self.page_cache.mark_dirty(pid);
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
    /// to call during early boot. Handles ext-backed and tmpfs regular files.
    /// This is the path firmware loading takes (see src/fwload.rs) - the same
    /// resolve_path machinery sys_read uses, just without a process fd.
    pub fn read_path(&mut self, path: &str) -> VfsResult<alloc::vec::Vec<u8>> {
        let vid = self.resolve_path_follow(0, path)?;
        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }

        if self.nodes[vid].is_ext_backed() {
            let ext2_ino = self.nodes[vid].ext2_ino;
            let res = crate::commands::ext2_cmds::with_ext2_pub(
                |fs| -> VfsResult<alloc::vec::Vec<u8>> {
                    let inode = fs.read_inode(ext2_ino).map_err(|_| VfsError::IoError)?;
                    let size = inode.size() as usize;
                    let mut out = alloc::vec::Vec::new();
                    out.try_reserve_exact(size).map_err(|_| VfsError::NoSpace)?;
                    out.resize(size, 0);
                    let mut done = 0usize;
                    while done < size {
                        let n = fs
                            .read_file(&inode, done as u64, &mut out[done..])
                            .map_err(|_| VfsError::IoError)?;
                        if n == 0 {
                            break;
                        }
                        done += n;
                    }
                    out.truncate(done);
                    Ok(out)
                },
            );
            return match res {
                Some(r) => r,
                None => Err(VfsError::IoError),
            };
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
            if let Some(pid) = self.nodes[vid].addr_space.get_page(page_num) {
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
        if self.nodes[vid].is_ext_backed() {
            let ext2_ino = self.nodes[vid].ext2_ino;
            let res = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.read_inode(ext2_ino).map(|i| i.size()).map_err(|_| VfsError::IoError)
            });
            return match res {
                Some(r) => r,
                None => Err(VfsError::IoError),
            };
        }
        Ok(self.nodes[vid].size)
    }

    /// Graft an ext-mounted directory onto a VFS path as a mount point: after
    /// this, resolving 'parent_path'/'name' descends into the ext filesystem
    /// at inode 'ext_ino'. Mirrors how 'cd' into an ext subtree attaches a
    /// vnode (see commands::fs). Used to mount the root disk's /lib/firmware
    /// onto the VFS so firmware reads go through the normal path, like Linux.
    pub fn graft_ext_dir(&mut self, parent_path: &str, name: &str, ext_ino: u32) -> VfsResult<usize> {
        let parent = self.resolve_path(0, parent_path)?;
        if !self.nodes[parent].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        let fs_type = crate::commands::ext2_cmds::active_fs_type();

        // Re-grafting an existing mount point just refreshes the target inode.
        if let Ok(existing) = self.resolve_path(parent, name) {
            if self.nodes[existing].is_dir() {
                self.nodes[existing].ext2_ino = ext_ino;
                self.nodes[existing].fs_type = fs_type;
                self.nodes[existing].children_loaded = false;
                return Ok(existing);
            }
        }

        let id = self.alloc_vnode()?;
        let ts = self.now();
        self.nodes[id].init(
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

        let vnode_used = self.total_vnodes();
        let mut proc_buf = [0u8; 192];

        match procfs::proc_read(name_str, &mut proc_buf, vnode_used) {
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

    pub fn write(&mut self, fd: usize, data: &[u8]) -> VfsResult<usize> {
        let file = self.fds().get(fd)?;
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
            if self.nodes[vid].is_ext2_backed() {
                let ino = self.nodes[vid].ext2_ino;
                let disk_size = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.read_inode(ino).map(|i| i.size()).unwrap_or(0)
                });
                if let Some(sz) = disk_size {
                    self.nodes[vid].size = sz;
                }
            }
            offset = self.nodes[vid].size as usize;
        }

        if self.nodes[vid].is_ext_backed() {
            return self.write_ext2_file(fd, vid, offset as u64, data);
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

            let pid = match self.nodes[vid].addr_space.get_page(page_num) {
                Some(pid) => pid,
                None => {
                    let pid = self.page_cache.alloc_page()?;
                    self.nodes[vid].addr_space.set_page(page_num, pid)?;
                    pid
                }
            };

            if let Some(page_data) = self.page_cache.get_page_data_mut(pid) {
                page_data[page_off..page_off + chunk]
                    .copy_from_slice(&data[done..done + chunk]);
                self.page_cache.mark_dirty(pid);
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
        let file = self.fds().get(fd)?;
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
        let file = self.fds().get(fd)?;
        let vid = file.vnode_id as usize;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }

        if self.nodes[vid].fs_type.is_ext_family() {
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.sync().map_err(|_| VfsError::IoError)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(e)) => return Err(e),
                None => return Err(VfsError::IoError),
            }
        }

        // A raw block-device node commits straight to its device: drain the
        // block layer's dirty cache for that disk and flush its write cache
        if let Some(crate::vfs::devfs::DevType::Block { dev, .. }) = self.get_dev_type(vid) {
            crate::block::flush(dev).map_err(|_| VfsError::IoError)?;
        }

        self.nodes[vid].flags.dirty = false;
        Ok(())
    }

    pub fn unlink(&mut self, cwd: usize, path: &str) -> VfsResult<()> {
        let id = self.resolve_path_lstat(cwd, path)?;

        if self.nodes[id].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        let entry_name = match path.rfind('/') {
            Some(pos) => &path[pos + 1..],
            None => path,
        };

        let pid = self.nodes[id].parent as usize;
        self.check_dir_write(pid)?;

        if self.nodes[id].is_ext_backed()
            && self.nodes[pid].ext2_ino != 0
            && self.nodes[id].refcount == 0
        {
            let parent_ino = self.nodes[pid].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_delete_file(parent_ino, entry_name)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) | None => return Err(VfsError::IoError),
            }
        }

        self.nodes[pid].children.remove_by_name(entry_name);

        let ts = self.now();
        self.nodes[pid].touch_mtime(ts);

        self.nodes[id].nlinks = self.nodes[id].nlinks.saturating_sub(1);

        crate::serial_println!(
            "[vfs] unlink '{}' id={} nlinks={} refs={}",
            path,
            id,
            self.nodes[id].nlinks,
            self.nodes[id].refcount
        );

        if self.nodes[id].nlinks == 0 && self.nodes[id].refcount == 0 {
            self.free_file_pages(id);
            self.nodes[id].active = false;
            if id < self.vnode_free_hint {
                self.vnode_free_hint = id;
            }
        } else {
            self.nodes[id].touch_ctime(ts);
        }

        Ok(())
    }
}
