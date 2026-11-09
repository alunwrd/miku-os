// ext2/ext3/ext4 as an implementation of `FileSystem`
//
// All the VFS knows about ext. Every method forwards into `lib/fs/extfs`.
//
// `impl FileSystem for MikuFS` with no newtype: the trait is ours and the type
// is foreign, which the orphan rule permits.
//
// Each method picks the ext function. In the pairs below (rename and move,
// truncate and truncate_to, delete_file and delete_dir), the VFS asks for a
// rename and does not learn that ext has two.

use crate::fs::ext::MikuFS;
use crate::fs::vfs::fsops::{FileSystem, FsSetAttr, NodeId, NodeMeta, RawDirent};
use crate::fs::vfs::types::*;
use extfs::structs::{Inode, FT_BLKDEV, FT_CHRDEV, FT_DIR, FT_FIFO, FT_SOCK, FT_SYMLINK};

/// The `FT_*` a directory entry carries, as the VFS names the same thing
fn kind_of_ft(ft: u8) -> VNodeKind {
    match ft {
        FT_DIR => VNodeKind::Directory,
        FT_SYMLINK => VNodeKind::Symlink,
        FT_CHRDEV => VNodeKind::CharDevice,
        FT_BLKDEV => VNodeKind::BlockDevice,
        FT_FIFO => VNodeKind::Fifo,
        FT_SOCK => VNodeKind::Socket,
        _ => VNodeKind::Regular,
    }
}

/// The same question asked of an inode, not of a directory entry.
///
/// They can disagree on a damaged filesystem, and the inode then owns the
/// answer
fn kind_of_inode(inode: &Inode) -> VNodeKind {
    if inode.is_directory() {
        VNodeKind::Directory
    } else if inode.is_symlink() {
        VNodeKind::Symlink
    } else if inode.is_chardev() {
        VNodeKind::CharDevice
    } else if inode.is_blockdev() {
        VNodeKind::BlockDevice
    } else if inode.is_fifo() {
        VNodeKind::Fifo
    } else if inode.is_socket() {
        VNodeKind::Socket
    } else {
        VNodeKind::Regular
    }
}

fn meta_of(ino: u32, inode: &Inode) -> NodeMeta {
    let kind = kind_of_inode(inode);
    NodeMeta {
        id: NodeId::ino(ino),
        kind,
        mode: inode.permissions(),
        // The full width the disk can hold. Folding to what this kernel's
        // credentials can represent is the VFS's decision
        uid: inode.uid_full(),
        gid: inode.gid_full(),
        size: inode.size(),
        blocks: inode.blocks_full(),
        nlinks: inode.links_count(),
        atime: inode.atime() as u64,
        mtime: inode.mtime() as u64,
        ctime: inode.ctime() as u64,
        rdev: if matches!(kind, VNodeKind::CharDevice | VNodeKind::BlockDevice) {
            inode.device_number()
        } else {
            (0, 0)
        },
    }
}

/// The `FT_*` to record in a directory entry for a node of this kind
fn ft_of_kind(kind: VNodeKind) -> Option<u8> {
    match kind {
        VNodeKind::CharDevice => Some(FT_CHRDEV),
        VNodeKind::BlockDevice => Some(FT_BLKDEV),
        VNodeKind::Fifo | VNodeKind::Pipe => Some(FT_FIFO),
        VNodeKind::Socket => Some(FT_SOCK),
        _ => None,
    }
}

impl FileSystem for MikuFS {
    // identity

    fn fs_type(&self) -> FsType {
        if self.superblock.is_ext4() {
            FsType::Ext4
        } else if self.superblock.is_ext3() {
            FsType::Ext3
        } else {
            FsType::Ext2
        }
    }

    fn read_only(&self) -> bool {
        self.reader.read_only
    }

    fn root(&self) -> NodeId {
        NodeId::ino(extfs::structs::EXT2_ROOT_INO)
    }

    fn statfs(&mut self) -> VfsResult<StatFs> {
        let info = self.fs_info();
        Ok(StatFs {
            fs_type: self.fs_type(),
            block_size: info.block_size,
            total_blocks: info.total_blocks as u64,
            free_blocks: info.free_blocks as u64,
            total_inodes: info.total_inodes as u64,
            free_inodes: info.free_inodes as u64,
            max_name_len: extfs::structs::MAX_NAME as u32,
            flags: if self.reader.read_only { 1 } else { 0 },
        })
    }

    fn sync(&mut self) -> VfsResult<()> {
        MikuFS::sync(self).map_err(VfsError::from)
    }

    // namespace

    fn lookup(&mut self, dir: NodeId, name: &str) -> VfsResult<NodeMeta> {
        let child = match self.ext2_lookup_in_dir(dir.as_ino(), name)? {
            Some(ino) => ino,
            None => return Err(VfsError::NotFound),
        };
        let inode = self.read_inode(child)?;
        Ok(meta_of(child, &inode))
    }

    fn readdir(
        &mut self,
        dir: NodeId,
        f: &mut dyn FnMut(RawDirent<'_>) -> bool,
    ) -> VfsResult<()> {
        let inode = self.read_inode(dir.as_ino())?;
        // `read_dir_each`, not `read_dir`: the array form silently stops at the
        // end of the caller's allocation
        self.read_dir_each(&inode, |e| {
            f(RawDirent {
                name: &e.name[..e.name_len as usize],
                id: NodeId::ino(e.inode),
                kind: kind_of_ft(e.file_type),
            })
        })?;
        Ok(())
    }

    fn create(
        &mut self,
        dir: NodeId,
        name: &str,
        kind: VNodeKind,
        mode: u16,
        uid: u32,
        gid: u32,
        rdev: (u32, u32),
        target: Option<&str>,
    ) -> VfsResult<NodeMeta> {
        let parent = dir.as_ino();
        let ino = match kind {
            VNodeKind::Directory => self.ext3_create_dir_as(parent, name, mode, uid, gid)?,
            VNodeKind::Symlink => {
                let target = target.ok_or(VfsError::InvalidArgument)?;
                self.ext3_create_symlink_as(parent, name, target, uid, gid)?
            }
            VNodeKind::Regular => self.ext3_create_file_as(parent, name, mode, uid, gid)?,
            other => {
                // One call covers all four special kinds
                let ft = ft_of_kind(other).ok_or(VfsError::InvalidArgument)?;
                self.ext3_mknod_as(parent, name, ft, mode, rdev.0, rdev.1, uid, gid)?
            }
        };
        let inode = self.read_inode(ino)?;
        Ok(meta_of(ino, &inode))
    }

    fn link(&mut self, dir: NodeId, name: &str, target: NodeId) -> VfsResult<()> {
        self.ext3_hardlink(dir.as_ino(), name, target.as_ino())
            .map_err(VfsError::from)
    }

    fn unlink(&mut self, dir: NodeId, name: &str, is_dir: bool) -> VfsResult<()> {
        let parent = dir.as_ino();
        if is_dir {
            self.ext3_delete_dir(parent, name).map_err(VfsError::from)
        } else {
            self.ext3_delete_file(parent, name).map_err(VfsError::from)
        }
    }

    fn rename(
        &mut self,
        old_dir: NodeId,
        old_name: &str,
        new_dir: NodeId,
        new_name: &str,
    ) -> VfsResult<()> {
        // ext has two calls, and the difference is whether the parent changed:
        // a rename within a directory has no `..` to fix up
        if old_dir == new_dir {
            self.ext3_rename(old_dir.as_ino(), old_name, new_name)
                .map_err(VfsError::from)
        } else {
            self.ext3_move(old_dir.as_ino(), old_name, new_dir.as_ino(), new_name)
                .map_err(VfsError::from)
        }
    }

    // data

    fn read_at(&mut self, id: NodeId, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        let inode = self.read_inode(id.as_ino())?;
        let size = inode.size();
        if offset >= size {
            return Ok(0);
        }
        let avail = (size - offset) as usize;
        let to_read = buf.len().min(avail);
        let n = self.read_file(&inode, offset, &mut buf[..to_read])?;
        Ok(n)
    }

    fn write_at(&mut self, id: NodeId, offset: u64, data: &[u8]) -> VfsResult<usize> {
        self.ext3_write_file(id.as_ino(), data, offset)
            .map_err(VfsError::from)
    }

    fn write_pages(&mut self, id: NodeId, offset: u64, pages: &[&[u8]]) -> VfsResult<()> {
        let want: usize = pages.iter().map(|p| p.len()).sum();
        match self.ext3_write_parts(id.as_ino(), pages, offset).map_err(VfsError::from)? {
            n if n == want => Ok(()),
            _ => Err(VfsError::NoSpace),
        }
    }

    fn truncate(&mut self, id: NodeId, size: u64) -> VfsResult<()> {
        let ino = id.as_ino();
        if size == 0 {
            self.ext3_truncate(ino).map_err(VfsError::from)
        } else {
            self.ext3_truncate_to(ino, size).map_err(VfsError::from)
        }
    }

    fn readlink(&mut self, id: NodeId, buf: &mut [u8]) -> VfsResult<usize> {
        let inode = self.read_inode(id.as_ino())?;
        if !inode.is_symlink() {
            return Err(VfsError::InvalidArgument);
        }
        if inode.is_fast_symlink() {
            let target = inode.fast_symlink_target();
            let n = target.len().min(buf.len());
            buf[..n].copy_from_slice(&target[..n]);
            return Ok(n);
        }
        let want = (inode.size() as usize).min(buf.len());
        let n = self.read_file(&inode, 0, &mut buf[..want])?;
        Ok(n)
    }

    // attributes

    fn getattr(&mut self, id: NodeId) -> VfsResult<NodeMeta> {
        let ino = id.as_ino();
        let inode = self.read_inode(ino)?;
        Ok(meta_of(ino, &inode))
    }

    fn setattr(&mut self, id: NodeId, attr: &FsSetAttr) -> VfsResult<()> {
        let ino = id.as_ino();
        if let Some(mode) = attr.mode {
            self.ext2_chmod(ino, mode)?;
        }
        if attr.uid.is_some() || attr.gid.is_some() {
            // The half not named keeps the inode's value. A default of zero
            // would hand the file to root on every chgrp
            let inode = self.read_inode(ino)?;
            let uid = attr.uid.unwrap_or_else(|| inode.uid_full());
            let gid = attr.gid.unwrap_or_else(|| inode.gid_full());
            self.ext2_chown(ino, uid, gid)?;
        }
        if let Some(size) = attr.size {
            FileSystem::truncate(self, id, size)?;
        }
        if attr.atime.is_some() || attr.mtime.is_some() {
            let mut inode = self.read_inode(ino)?;
            if let Some(t) = attr.atime {
                inode.set_atime(t as u32);
            }
            if let Some(t) = attr.mtime {
                inode.set_mtime(t as u32);
            }
            self.write_inode(ino, &inode)?;
        }
        Ok(())
    }

    fn touch_atime(&mut self, id: NodeId) {
        // Never on a read-only mount: reading a file (exec of /bin/mikudo) must
        // not write anything back
        if self.reader.read_only {
            return;
        }
        let ino = id.as_ino();
        if let Err(e) = MikuFS::touch_atime(self, ino) {
            crate::trace_println!("[vfs] atime of inode {}: {:?}", ino, e);
        }
    }

    // optional

    fn fsync(&mut self, _id: NodeId) -> VfsResult<()> {
        // ext journals the whole filesystem: there is no per-inode commit
        MikuFS::sync(self).map_err(VfsError::from)
    }

    fn fallocate(&mut self, id: NodeId, offset: u64, len: u64) -> VfsResult<()> {
        self.ext4_fallocate(id.as_ino(), offset, len)
            .map_err(VfsError::from)
    }

    fn punch_hole(&mut self, id: NodeId, offset: u64, len: u64) -> VfsResult<()> {
        self.ext2_punch_hole(id.as_ino(), offset, len)
            .map_err(VfsError::from)
    }

    fn getxattr(&mut self, id: NodeId, name: &str, buf: &mut [u8]) -> VfsResult<usize> {
        let (index, rest) = split_xattr_name(name)?;
        self.get_xattr(id.as_ino(), index, rest, buf)
            .map_err(VfsError::from)
    }

    fn setxattr(&mut self, id: NodeId, name: &str, value: &[u8]) -> VfsResult<()> {
        let (index, rest) = split_xattr_name(name)?;
        self.ext3_set_xattr(id.as_ino(), index, rest, value)
            .map_err(VfsError::from)
    }

    fn trim(&mut self) -> VfsResult<u64> {
        let report = self.trim_free_blocks(1)?;
        Ok(report.trimmed_bytes)
    }
}

/// `user.thing` is stored as index 1 and the name `thing`: ext keeps the
/// namespace as a number, one byte instead of five. A name with no namespace is
/// refused
fn split_xattr_name(name: &str) -> VfsResult<(u8, &str)> {
    use extfs::xattr::*;
    let (prefix, rest) = name.split_once('.').ok_or(VfsError::InvalidArgument)?;
    let index = match prefix {
        "user" => XATTR_INDEX_USER,
        "trusted" => XATTR_INDEX_TRUSTED,
        "security" => XATTR_INDEX_SECURITY,
        "system" => XATTR_INDEX_SYSTEM,
        _ => return Err(VfsError::NotSupported),
    };
    if rest.is_empty() {
        return Err(VfsError::InvalidArgument);
    }
    Ok((index, rest))
}
