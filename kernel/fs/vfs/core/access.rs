// mode/uid/gid checks against the calling process's credentials
//
// Each check asks for the capability it needs, so authority can be handed out
// in the size wanted: the display server can bind a privileged global without
// being able to read /etc/shadow.
//
// Directory search is enforced too: `x` on a directory makes a private
// directory private, in path_resolve.rs through `check_search` below.

use super::MikuVFS;
use crate::fs::vfs::types::*;
use crate::process::cred::{CAP_DAC_OVERRIDE, CAP_FOWNER};

/// The read/write test, against a file's attributes, not a vnode
///
/// `check_access` below is this function with the attributes read out of the
/// node table. One caller has the attributes and no vnode yet:
/// `syscall::adapters::file::open_from_active_ext` is handed an ext inode's
/// mode, uid and gid and creates the vnode after deciding to open it.
///
/// One function, so the rule cannot drift: a capability added to the bypass
/// applies to both callers. The credential is passed so this answers for the
/// caller the VFS is dressed as (`sync_ctx_from_current`), not whoever is
/// running.
pub fn check_attrs(
    cred: &crate::process::cred::Cred,
    mode: FileMode,
    uid: u16,
    gid: u16,
    flags: OpenFlags,
) -> VfsResult<()> {
    let bits = mode.perm_bits_for(cred.who_for(uid, gid));

    if flags.readable() && (bits & 0o4) == 0 && !cred.may_bypass_read() {
        return Err(VfsError::PermissionDenied);
    }
    if flags.writable() && (bits & 0o2) == 0 && !cred.may_bypass_write() {
        return Err(VfsError::PermissionDenied);
    }
    Ok(())
}

impl MikuVFS {
    /// The permission triple that applies to this caller for this node
    fn perm_bits(&self, id: usize) -> u8 {
        let node = &self.nodes[id];
        self.nodes[id].mode.perm_bits_for(self.ctx.cred.who_for(node.uid, node.gid))
    }

    pub(super) fn check_access(&self, id: usize, flags: OpenFlags) -> VfsResult<()> {
        self.check_access_as(&self.ctx.cred, id, flags)
    }

    /// `check_access` for `cred` (`with_vfs_read`)
    pub(super) fn check_access_as(&self, cred: &Credentials, id: usize, flags: OpenFlags) -> VfsResult<()> {
        let node = &self.nodes[id];
        check_attrs(cred, node.mode, node.uid, node.gid, flags)
    }

    /// May the caller change the set of names in this directory?
    ///
    /// Write permission, then the directory's own immutable flag. `/lib` is an
    /// immutable tmpfs directory holding the baked `.so` files; a write into it
    /// would produce a tmpfs file gone at the next boot, with no record of
    /// anything having gone wrong.
    ///
    /// Here since all eight callers pass through: create, mkdir, symlink, link,
    /// unlink, rmdir and both ends of a rename. Grafting is not among them:
    /// `graft_ext_dir` inserts into `children` itself, which keeps
    /// `/lib/firmware` mountable under an immutable `/lib`
    pub(super) fn check_dir_write(&self, dir_id: usize) -> VfsResult<()> {
        if self.nodes[dir_id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }
        self.check_access(dir_id, OpenFlags(OpenFlags::WRITE))
    }

    /// May the caller pass through this directory?
    ///
    /// The `x` bit on a directory grants only the right to name something
    /// inside it. A directory readable but not searchable can be listed and not
    /// entered; one searchable but not readable can be entered by anyone who
    /// knows a name in it and cannot be enumerated.
    ///
    /// CAP_DAC_READ bypasses this, the narrow capability for a backup or an
    /// indexer: it opens every directory and grants no power to change any
    pub(super) fn check_search(&self, dir_id: usize) -> VfsResult<()> {
        self.check_search_as(&self.ctx.cred, dir_id)
    }

    /// `check_search` for `cred`, not the VFS's context: a walk under the
    /// lock held to read carries its caller's own (`path_resolve::Reader`)
    pub(super) fn check_search_as(&self, cred: &Credentials, dir_id: usize) -> VfsResult<()> {
        let node = &self.nodes[dir_id];
        if (node.mode.perm_bits_for(cred.who_for(node.uid, node.gid)) & 0o1) != 0 || cred.may_bypass_read() {
            Ok(())
        } else {
            Err(VfsError::PermissionDenied)
        }
    }

    /// May the caller execute this file?
    ///
    /// The bypass has an extra condition. CAP_DAC_OVERRIDE lets a privileged
    /// process read and write anything, but not execute a file with no execute
    /// bit set anywhere, as on Linux: `exec` of a data file is almost always a
    /// bug, and a privileged process can chmod first
    pub(super) fn check_exec(&self, id: usize) -> VfsResult<()> {
        let bits = self.perm_bits(id);
        if (bits & 0o1) != 0 {
            return Ok(());
        }
        let any_x = self.nodes[id].mode.0 & 0o111 != 0;
        if self.ctx.cred.has(CAP_DAC_OVERRIDE) && any_x {
            return Ok(());
        }
        Err(VfsError::PermissionDenied)
    }

    /// May this path be executed at all: permission bits, and the mount it is
    /// on?
    ///
    /// A program with no execute bit does not run, nor one on a noexec mount,
    /// which matters for a home directory and a removable disk
    pub(super) fn check_exec_node(&self, id: usize) -> VfsResult<()> {
        if self.mount_flags(id) & crate::fs::vfs::mount::MNT_NOEXEC != 0 {
            return Err(VfsError::PermissionDenied);
        }
        self.check_exec(id)
    }

    /// May the caller act as this node's owner: chmod it, stamp it, change its
    /// extended attributes?
    pub(super) fn check_owner(&self, id: usize) -> VfsResult<()> {
        if self.ctx.cred.owns_or_may(self.nodes[id].uid) {
            Ok(())
        } else {
            Err(VfsError::PermissionDenied)
        }
    }

    /// May the caller remove or rename `victim` out of `dir`?
    ///
    /// Write permission on the directory, then the sticky bit. A sticky
    /// directory (/tmp) is world-writable so anyone may create in it, and only
    /// the owner of a file may remove it. Without this, the attack is
    /// replacement: unlink someone's file and put a symlink where it was, just
    /// before they write to it
    pub(super) fn check_unlink(&self, dir_id: usize, victim_id: usize) -> VfsResult<()> {
        self.check_dir_write(dir_id)?;
        if !self.nodes[dir_id].mode.sticky() {
            return Ok(());
        }
        let cred = &self.ctx.cred;
        if cred.euid == self.nodes[victim_id].uid
            || cred.euid == self.nodes[dir_id].uid
            || cred.has(CAP_FOWNER)
        {
            Ok(())
        } else {
            Err(VfsError::PermissionDenied)
        }
    }
}

/// The mode a file may keep when its owner or group changes, or when an
/// unprivileged process writes to it.
///
/// setuid and setgid are cleared. On MikuOS exec ignores them, but they are
/// recorded, shown by `ls`, and meaningful to a disk shared with a system where
/// they raise privilege: a file that changed hands would carry "run this as its
/// new owner"
#[inline]
pub fn strip_setid(mode: FileMode) -> FileMode {
    FileMode(mode.0 & !0o6000)
}
