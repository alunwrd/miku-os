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

pub fn check_access(
    mode: FileMode,
    uid: u16,
    gid: u16,
    cred: &Credentials,
    access: AccessMode,
) -> bool {
    if cred.is_root() {
        return true;
    }

    let who = if cred.euid == uid {
        PermWho::Owner
    } else if cred.in_group(gid) {
        PermWho::Group
    } else {
        PermWho::Other
    };

    let bits = mode.perm_bits_for(who);
    let needed = access as u8;

    (bits & needed) == needed
}

pub fn check_open_flags(
    mode: FileMode,
    uid: u16,
    gid: u16,
    cred: &Credentials,
    flags: OpenFlags,
) -> bool {
    if cred.is_root() {
        return true;
    }

    let who = if cred.euid == uid {
        PermWho::Owner
    } else if cred.in_group(gid) {
        PermWho::Group
    } else {
        PermWho::Other
    };

    let bits = mode.perm_bits_for(who);

    if flags.readable() && (bits & 0o4) == 0 {
        return false;
    }
    if flags.writable() && (bits & 0o2) == 0 {
        return false;
    }

    true
}
