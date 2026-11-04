// The VFS's half of inotify: making an instance's descriptor, and deciding
// what a watch may name. The instances and their events are
// `crate::fs::vfs::notify`'s.

use super::MikuVFS;
use crate::fs::vfs::notify;
use crate::fs::vfs::types::*;

impl MikuVFS {
    /// `inotify_init1`: an instance, and a descriptor on it. `flags` carries
    /// `NONBLOCK` and `CLOEXEC`, which are `IN_NONBLOCK` and `IN_CLOEXEC`:
    /// Linux defines those as the `O_*` values, as does MikuOS
    pub fn inotify_init(&mut self, flags: u32) -> VfsResult<usize> {
        let id = self.anon_node(VNodeKind::CharDevice, "inotify", FileMode::new(0o600))?;
        (self.nodes[id].dev_major, self.nodes[id].dev_minor) = notify::DEV;
        if !notify::create(id) {
            self.free_vnode(id);
            return Err(VfsError::TooManyOpenFiles);
        }
        let keep = flags & (OpenFlags::NONBLOCK | OpenFlags::CLOEXEC);
        // Bound first: the table's guard must be gone before the error path
        // frees the vnode
        let fd = self.fds().alloc(id as InodeId, OpenFlags(OpenFlags::READ | keep));
        let fd = match fd {
            Ok(fd) => fd,
            Err(e) => {
                notify::destroy(id);
                self.free_vnode(id);
                return Err(e);
            }
        };
        self.nodes[id].inc_ref();
        Ok(fd)
    }

    /// The instance a descriptor is on, or `InvalidArgument`, as Linux answers
    /// for a descriptor that is not an inotify one
    pub fn inotify_of(&mut self, fd: usize) -> VfsResult<usize> {
        let vid = self.fds().get(fd)?.vnode_id as usize;
        if self.valid_vnode(vid) && self.nodes[vid].is_notify_instance() {
            Ok(vid)
        } else {
            Err(VfsError::InvalidArgument)
        }
    }

    /// What `inotify_add_watch` may watch at `path`: anything the caller may
    /// read, as on Linux. A watch reports names and changes, so a directory
    /// nobody may list must not be listable by watching it
    pub fn watch_target(&mut self, cwd: usize, path: &str, follow: bool, only_dir: bool) -> VfsResult<usize> {
        let id = if follow {
            self.resolve_path_follow(cwd, path)?
        } else {
            self.resolve_path_lstat(cwd, path)?
        };
        if only_dir && !self.nodes[id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        self.check_access(id, OpenFlags(OpenFlags::READ))?;
        Ok(id)
    }
}
