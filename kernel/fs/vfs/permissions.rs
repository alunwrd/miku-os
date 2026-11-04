use crate::fs::vfs::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AccessMode {
    Read = 4,
    Write = 2,
    Exec = 1,
    ReadWrite = 6,
    ReadExec = 5,
    All = 7,
}

/// The free-standing form of the check in `core/access.rs`, for callers holding
/// a mode and an owner, not a vnode: the xattr syscalls, which are handed a raw
/// inode number.
///
/// Capability-based like its counterpart: CAP_DAC_OVERRIDE alone grants, and a
/// uid-0 process with every capability dropped does not pass
pub fn check_access(
    mode: FileMode,
    uid: u16,
    gid: u16,
    cred: &Credentials,
    access: AccessMode,
) -> bool {
    let bits   = mode.perm_bits_for(cred.who_for(uid, gid));
    let needed = access as u8;
    if (bits & needed) == needed {
        return true;
    }
    // Write access needs the broad capability; read and search are also granted
    // by the narrow read-only one. Execute is granted by neither on a file with
    // no execute bit anywhere; see check_exec in core/access.rs
    if (needed & 0o1) != 0 && (mode.0 & 0o111) == 0 {
        return false;
    }
    if (needed & 0o2) != 0 { cred.may_bypass_write() } else { cred.may_bypass_read() }
}

pub fn check_open_flags(
    mode: FileMode,
    uid: u16,
    gid: u16,
    cred: &Credentials,
    flags: OpenFlags,
) -> bool {
    let bits = mode.perm_bits_for(cred.who_for(uid, gid));

    if flags.readable() && (bits & 0o4) == 0 && !cred.may_bypass_read() {
        return false;
    }
    if flags.writable() && (bits & 0o2) == 0 && !cred.may_bypass_write() {
        return false;
    }

    true
}
