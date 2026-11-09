//! Safe wrappers for identity syscalls 47-55 and 83-91

/// Largest id. 0xFFFF is chown's sentinel
pub use miku_abi::cred::ID_MAX;

/// Leave this field unchanged, for the three-argument setters
pub use miku_abi::cred::KEEP;

/// Maximum supplementary groups, as the kernel's MAX_GROUPS
pub use miku_abi::cred::MAX_GROUPS;

// Capability numbers, as in kernel/process/cred.rs
pub use miku_abi::cred::CAP_SETUID;
pub use miku_abi::cred::CAP_SETGID;
pub use miku_abi::cred::CAP_KILL;
pub use miku_abi::cred::CAP_DAC_OVERRIDE;
pub use miku_abi::cred::CAP_DAC_READ;
pub use miku_abi::cred::CAP_FOWNER;
pub use miku_abi::cred::CAP_CHOWN;
pub use miku_abi::cred::CAP_SYS_TIME;
pub use miku_abi::cred::CAP_SYS_ADMIN;
pub use miku_abi::cred::CAP_SYS_BOOT;
pub use miku_abi::cred::CAP_NET_RAW;
pub use miku_abi::cred::CAP_NET_BIND;
pub use miku_abi::cred::CAP_MKNOD;
pub use miku_abi::cred::CAP_MAY_PRIV;
pub use miku_abi::cred::CAP_AUDIT;
pub use miku_abi::cred::CAP_GPU;

#[inline]
pub const fn cap_bit(cap: u8) -> u64 { 1u64 << cap }

pub fn getuid() -> u16 { unsafe { super::miku_getuid() as u16 } }
pub fn getgid() -> u16 { unsafe { super::miku_getgid() as u16 } }
pub fn geteuid() -> u16 { unsafe { super::miku_geteuid() as u16 } }
pub fn getegid() -> u16 { unsafe { super::miku_getegid() as u16 } }
pub fn umask(mask: u16) -> u16 { unsafe { super::miku_umask(mask as u64) as u16 } }

fn ok(r: i64) -> Result<(), i64> {
    if r < 0 { Err(r) } else { Ok(()) }
}

pub fn setuid(uid: u16) -> Result<(), i64> { ok(unsafe { super::miku_setuid(uid as u64) }) }
pub fn setgid(gid: u16) -> Result<(), i64> { ok(unsafe { super::miku_setgid(gid as u64) }) }
pub fn seteuid(uid: u16) -> Result<(), i64> { ok(unsafe { super::miku_seteuid(uid as u64) }) }
pub fn setegid(gid: u16) -> Result<(), i64> { ok(unsafe { super::miku_setegid(gid as u64) }) }

/// Sets all three user ids at once. `None` leaves a field. Preferred when
/// dropping privilege for good
pub fn setresuid(r: Option<u16>, e: Option<u16>, s: Option<u16>) -> Result<(), i64> {
    let f = |v: Option<u16>| v.map(|x| x as u64).unwrap_or(KEEP);
    ok(unsafe { super::miku_setresuid(f(r), f(e), f(s)) })
}

pub fn setresgid(r: Option<u16>, e: Option<u16>, s: Option<u16>) -> Result<(), i64> {
    let f = |v: Option<u16>| v.map(|x| x as u64).unwrap_or(KEEP);
    ok(unsafe { super::miku_setresgid(f(r), f(e), f(s)) })
}

pub fn setgroups(list: &[u16]) -> Result<(), i64> {
    ok(unsafe { super::miku_setgroups(list.as_ptr(), list.len() as u64) })
}

/// Writes supplementary groups to `out`. Returns the count or a negative
/// errno
pub fn getgroups(out: &mut [u16]) -> Result<usize, i64> {
    let r = unsafe { super::miku_getgroups(out.as_mut_ptr(), out.len() as u64) };
    if r < 0 { Err(r) } else { Ok(r as usize) }
}

/// (permitted, effective, bounding)
pub fn capget() -> Result<(u64, u64, u64), i64> {
    let mut buf = [0u64; 3];
    let r = unsafe { super::miku_capget(buf.as_mut_ptr()) };
    if r < 0 { Err(r) } else { Ok((buf[0], buf[1], buf[2])) }
}

/// Narrows capabilities; each set must be a subset of what is held. The
/// bounding set survives fork and exec
pub fn capset(permitted: u64, effective: u64, bounding: u64) -> Result<(), i64> {
    ok(unsafe { super::miku_capset(permitted, effective, bounding) })
}

/// True when this process holds `cap` effectively
pub fn has_cap(cap: u8) -> bool {
    matches!(capget(), Ok((_, eff, _)) if eff & cap_bit(cap) != 0)
}

/// Drops every capability not in `keep` and lowers the bounding set to
/// match
pub fn keep_only_caps(keep: u64) -> Result<(), i64> {
    let (perm, _, bound) = capget()?;
    let want = perm & keep;
    capset(want, want, bound & keep)
}

/// Peer of a connected local socket at connect time, written by the kernel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
    pub pid: u64,
    /// Process start time. Compare with the pid, since pids are recycled
    pub start_tick: u64,
    pub uid: u16,
    pub gid: u16,
    pub euid: u16,
    pub egid: u16,
}

pub fn peercred(sockfd: i64) -> Result<PeerCred, i64> {
    let mut raw = miku_abi::cred::MikuPeerCred::default();
    let r = unsafe { super::miku_peercred(sockfd as u64, &mut raw as *mut _ as *mut u8) };
    if r < 0 {
        return Err(r);
    }
    Ok(PeerCred {
        pid: raw.pid,
        start_tick: raw.start_tick,
        uid: raw.uid as u16,
        gid: raw.gid as u16,
        euid: raw.euid as u16,
        egid: raw.egid as u16,
    })
}

/// Hand one open descriptor to the peer of a local socket
pub fn sendfd(sockfd: i64, fd: i64) -> Result<(), i64> {
    ok(unsafe { super::miku_sendfd(sockfd as u64, fd as u64) })
}

/// Takes the oldest descriptor sent. `Err(-11)` (EAGAIN) when none; never
/// blocks
pub fn recvfd(sockfd: i64) -> Result<i64, i64> {
    let r = unsafe { super::miku_recvfd(sockfd as u64) };
    if r < 0 { Err(r) } else { Ok(r) }
}
