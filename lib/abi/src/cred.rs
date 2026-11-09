//! Identity and capabilities: syscalls 47-55 and 86-91

/// Largest id. 0xFFFF is chown's sentinel
pub const ID_MAX: u16 = 0xFFFE;
/// Leave this field unchanged, for the three-argument setters
pub const KEEP: u64 = 0xFFFF;
/// Supplementary groups a credential holds
pub const MAX_GROUPS: usize = 16;

/// What `peercred`(83) writes: the peer of a connected local socket, at connect
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MikuPeerCred {
    pub pid: u64,
    /// Process start time. Compare with the pid, since pids are recycled
    pub start_tick: u64,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub _reserved: u64,
}

const _: () = assert!(core::mem::size_of::<MikuPeerCred>() == 40);

pub const CAP_SETUID: u8 = 0;
pub const CAP_SETGID: u8 = 1;
pub const CAP_KILL: u8 = 2;
pub const CAP_DAC_OVERRIDE: u8 = 3;
pub const CAP_DAC_READ: u8 = 4;
pub const CAP_FOWNER: u8 = 5;
pub const CAP_CHOWN: u8 = 6;
pub const CAP_SYS_TIME: u8 = 7;
pub const CAP_SYS_ADMIN: u8 = 8;
pub const CAP_SYS_BOOT: u8 = 9;
pub const CAP_NET_RAW: u8 = 10;
pub const CAP_NET_BIND: u8 = 11;
pub const CAP_MKNOD: u8 = 12;
pub const CAP_MAY_PRIV: u8 = 13;
pub const CAP_AUDIT: u8 = 14;
pub const CAP_GPU: u8 = 15;
pub const CAP_LAST: u8 = CAP_GPU;
