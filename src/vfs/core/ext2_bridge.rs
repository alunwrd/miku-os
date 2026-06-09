// Bridge between MikuVFS and the ext2/ext3 driver:
// lazy lookup, bulk loading of directory children, size refresh,
// low-level file reading/writing via the ext3 filesystem

use super::MikuVFS;
use crate::vfs::types::*;

impl MikuVFS {
    pub(super) fn ext2_lazy_lookup(
        &mut self,
        parent_vnode: usize,
        name: &str,
    ) -> VfsResult<usize> {
        if !self.ext2_mount_active || !crate::commands::ext2_cmds::is_ext2_ready() {
            return Err(VfsError::NotFound);
        }

        let parent_ext2_ino = self.nodes[parent_vnode].ext2_ino;
        if parent_ext2_ino == 0 {
            return Err(VfsError::NotFound);
        }

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            let child_ino = match fs.ext2_lookup_in_dir(parent_ext2_ino, name) {
                Ok(Some(ino)) => ino,
                Ok(None) => return Err(VfsError::NotFound),
                Err(_) => return Err(VfsError::IoError),
            };

            let inode = match fs.read_inode(child_ino) {
                Ok(i) => i,
                Err(_) => return Err(VfsError::IoError),
            };

            let kind = if inode.is_directory() {
                VNodeKind::Directory
            } else if inode.is_symlink() {
                VNodeKind::Symlink
            } else {
                VNodeKind::Regular
            };

            let perm = inode.permissions();
            let size = inode.size() as u64;
            let uid = inode.uid();
            let gid = inode.gid();
            let nlinks = inode.links_count();

            let mut symlink_target = [0u8; NAME_LEN];
            let mut symlink_len = 0u8;
            if inode.is_symlink() {
                if inode.is_fast_symlink() {
                    let target = inode.fast_symlink_target();
                    let l = target.len().min(NAME_LEN);
                    symlink_target[..l].copy_from_slice(&target[..l]);
                    symlink_len = l as u8;
                } else {
                    let read_len = (size as usize).min(NAME_LEN);
                    let n = fs
                        .read_file(&inode, 0, &mut symlink_target[..read_len])
                        .unwrap_or(0);
                    symlink_len = n as u8;
                }
            }

            Ok((
                child_ino,
                kind,
                perm,
                size,
                uid,
                gid,
                nlinks,
                symlink_target,
                symlink_len,
            ))
        });

        let info = match result {
            Some(Ok(info)) => info,
            Some(Err(e)) => return Err(e),
            None => return Err(VfsError::NotFound),
        };

        let (child_ino, kind, perm, size, uid, gid, nlinks, symlink_target, symlink_len) = info;

        let id = self.alloc_vnode()?;
        let ts = self.now();

        self.nodes[id].init(
            id as InodeId,
            parent_vnode as InodeId,
            name,
            kind,
            crate::commands::ext2_cmds::active_fs_type(),
            FileMode::new(perm),
            uid,
            gid,
            ts,
        );
        self.nodes[id].ext2_ino = child_ino;
        self.nodes[id].size = size;
        self.nodes[id].nlinks = nlinks;
        self.nodes[id].children_loaded = false;

        if kind == VNodeKind::Symlink && symlink_len > 0 {
            self.nodes[id].symlink_target.data[..symlink_len as usize]
                .copy_from_slice(&symlink_target[..symlink_len as usize]);
            self.nodes[id].symlink_target.len = symlink_len;
        }

        if !self.nodes[parent_vnode].children.insert(name, id as InodeId) {
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        Ok(id)
    }

    pub fn ext2_ensure_children_loaded(&mut self, dir_vnode: usize) -> VfsResult<()> {
        if !self.nodes[dir_vnode].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if !self.nodes[dir_vnode].fs_type.is_ext_family() {
            return Ok(());
        }
        if self.nodes[dir_vnode].children_loaded {
            return Ok(());
        }
        if self.nodes[dir_vnode].ext2_ino == 0 {
            return Ok(());
        }

        let ext2_ino = self.nodes[dir_vnode].ext2_ino;

        const BATCH: usize = 256;
        struct ChildInfo {
            name: [u8; 255],
            name_len: u8,
            ino: u32,
            file_type: u8,
            mode: u16,
            uid: u16,
            gid: u16,
            size: u64,
            nlinks: u16,
            symlink_target: [u8; 64],
            symlink_len: u8,
        }

        let mut child_infos: [ChildInfo; BATCH] = unsafe { core::mem::zeroed() };
        let mut child_count = 0usize;

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            use crate::miku_extfs::structs::FT_SYMLINK;

            let inode = fs.read_inode(ext2_ino).map_err(|_| VfsError::IoError)?;
            let mut entries = [const { crate::miku_extfs::structs::DirEntry::empty() }; 256];
            let count = fs
                .read_dir(&inode, &mut entries)
                .map_err(|_| VfsError::IoError)?;

            for i in 0..count {
                if child_count >= BATCH {
                    break;
                }
                let e = &entries[i];
                let n = e.name_str();
                if n == "." || n == ".." || n == "lost+found" {
                    continue;
                }
                let child_inode = match fs.read_inode(e.inode) {
                    Ok(ino) => ino,
                    Err(_) => continue,
                };
                let nb = n.as_bytes();
                let l = nb.len().min(255);
                child_infos[child_count].name[..l].copy_from_slice(&nb[..l]);
                child_infos[child_count].name_len = l as u8;
                child_infos[child_count].ino = e.inode;
                child_infos[child_count].file_type = e.file_type;
                child_infos[child_count].mode = child_inode.permissions();
                child_infos[child_count].uid = child_inode.uid();
                child_infos[child_count].gid = child_inode.gid();
                child_infos[child_count].size = child_inode.size();
                child_infos[child_count].nlinks = child_inode.links_count();

                if e.file_type == FT_SYMLINK {
                    if child_inode.is_fast_symlink() {
                        let target = child_inode.fast_symlink_target();
                        let tl = target.len().min(64);
                        child_infos[child_count].symlink_target[..tl]
                            .copy_from_slice(&target[..tl]);
                        child_infos[child_count].symlink_len = tl as u8;
                    } else {
                        let sz = child_inode.size() as usize;
                        let read_len = sz.min(64);
                        let n = fs
                            .read_file(
                                &child_inode,
                                0,
                                &mut child_infos[child_count].symlink_target[..read_len],
                            )
                            .unwrap_or(0);
                        child_infos[child_count].symlink_len = n as u8;
                    }
                }

                child_count += 1;
            }
            Ok::<(), VfsError>(())
        });

        match result {
            Some(Ok(())) => {}
            Some(Err(_)) => return Err(VfsError::IoError),
            None => return Err(VfsError::NotFound),
        }

        for i in 0..child_count {
            let ci = &child_infos[i];
            let name_str = match core::str::from_utf8(&ci.name[..ci.name_len as usize]) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let already = self.nodes[dir_vnode]
                .children
                .find_by_name(name_str)
                .map(|id| {
                    let c = id as usize;
                    c < MAX_VNODES && self.nodes[c].active
                })
                .unwrap_or(false);
            if already {
                continue;
            }

            use crate::miku_extfs::structs::{FT_DIR, FT_SYMLINK};
            let kind = match ci.file_type {
                FT_DIR => VNodeKind::Directory,
                FT_SYMLINK => VNodeKind::Symlink,
                _ => VNodeKind::Regular,
            };

            if let Ok(id) = self.alloc_vnode() {
                let ts = self.now();
                self.nodes[id].init(
                    id as InodeId,
                    dir_vnode as InodeId,
                    name_str,
                    kind,
                    crate::commands::ext2_cmds::active_fs_type(),
                    FileMode::new(ci.mode),
                    ci.uid,
                    ci.gid,
                    ts,
                );
                self.nodes[id].ext2_ino = ci.ino;
                self.nodes[id].size = ci.size;
                self.nodes[id].nlinks = ci.nlinks;
                self.nodes[id].children_loaded = false;

                if kind == VNodeKind::Symlink && ci.symlink_len > 0 {
                    let sl = ci.symlink_len as usize;
                    self.nodes[id].symlink_target.data[..sl]
                        .copy_from_slice(&ci.symlink_target[..sl]);
                    self.nodes[id].symlink_target.len = ci.symlink_len;
                }

                if !self.nodes[dir_vnode].children.insert(name_str, id as InodeId) {
                    self.nodes[id].active = false;
                }
            }
        }

        if child_count < BATCH {
            self.nodes[dir_vnode].children_loaded = true;
        }
        Ok(())
    }

    pub(super) fn ext2_refresh_size(&mut self, id: usize) {
        if !self.nodes[id].is_ext_backed() {
            return;
        }
        let ino = self.nodes[id].ext2_ino;
        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            fs.read_inode(ino).map(|inode| (inode.size(), inode.blocks()))
        });
        if let Some(Ok((size, blocks))) = result {
            self.nodes[id].size = size;
            self.nodes[id].addr_space.nr_pages = blocks;
        }
    }

    pub(super) fn read_ext2_file(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        buf: &mut [u8],
    ) -> VfsResult<usize> {
        let ext2_ino = self.nodes[vid].ext2_ino;

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            let inode = fs.read_inode(ext2_ino).map_err(|_| VfsError::IoError)?;
            let size = inode.size() as u64;
            if offset >= size {
                return Ok((0usize, size));
            }
            let avail = (size - offset) as usize;
            let to_read = buf.len().min(avail);
            let n = fs
                .read_file(&inode, offset, &mut buf[..to_read])
                .map_err(|_| VfsError::IoError)?;
            // Updating atime via relatime: only if atime < mtime or older than 24 hours
            let _ = fs.touch_atime(ext2_ino);
            Ok((n, size))
        });

        let (n, disk_size) = match result {
            Some(Ok(pair)) => pair,
            Some(Err(e)) => return Err(e),
            None => return Err(VfsError::IoError),
        };

        if disk_size > 0 {
            self.nodes[vid].size = disk_size;
        }

        self.fds().get_mut(fd)?.offset += n as u64;
        Ok(n)
    }

    pub(super) fn write_ext2_file(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        data: &[u8],
    ) -> VfsResult<usize> {
        let ext2_ino = self.nodes[vid].ext2_ino;
        let is_sync = self
            .fds()
            .get(fd)
            .map(|f| f.flags.has(OpenFlags::SYNC))
            .unwrap_or(false);

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            fs.ext3_write_file(ext2_ino, data, offset)
                .map_err(|_| VfsError::IoError)
        });

        let n = match result {
            Some(Ok(n)) => n,
            Some(Err(e)) => return Err(e),
            None => return Err(VfsError::IoError),
        };

        let new_end = offset + n as u64;
        if new_end > self.nodes[vid].size {
            self.nodes[vid].size = new_end;
        }

        let ts = self.now();
        self.nodes[vid].touch_mtime(ts);
        self.nodes[vid].flags.dirty = true;

        if is_sync {
            let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.sync());
            self.nodes[vid].flags.dirty = false;
        }

        self.fds().get_mut(fd)?.offset = new_end;
        Ok(n)
    }
}
