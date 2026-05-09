// Access checks based on mode/UID/GID and process context

use super::MikuVFS;
use crate::vfs::types::*;

impl MikuVFS {
    pub(super) fn check_access(&self, id: usize, flags: OpenFlags) -> VfsResult<()> {
        if self.ctx.cred.is_root() {
            return Ok(());
        }
        let node = &self.nodes[id];
        let who = if self.ctx.cred.euid == node.uid {
            PermWho::Owner
        } else if self.ctx.cred.in_group(node.gid) {
            PermWho::Group
        } else {
            PermWho::Other
        };
        let bits = node.mode.perm_bits_for(who);

        if flags.readable() && (bits & 0o4) == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if flags.writable() && (bits & 0o2) == 0 {
            return Err(VfsError::PermissionDenied);
        }
        Ok(())
    }

    pub(super) fn check_dir_write(&self, dir_id: usize) -> VfsResult<()> {
        self.check_access(dir_id, OpenFlags(OpenFlags::WRITE))
    }
}
