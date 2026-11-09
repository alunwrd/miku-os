// cred: who a process is, and what it is allowed to do
//
// Identity is one value, copied in and out under one lock, so a permission
// check never reads a half-applied change. It carries the saved ids POSIX
// needs, so a process that lowered itself can come back, plus a capability set.
//
// # Capabilities
//
// "root" on MikuOS is not a uid the kernel tests for. It is the identity
// holding every capability. Each privileged call site asks for the one
// capability it needs (the VFS asks CAP_DAC_OVERRIDE, clock_settime asks
// CAP_SYS_TIME, the display server asks CAP_MAY_PRIV), so a program can be
// given exactly the authority it requires. maywm needs to command the
// compositor; it does not need to read /etc/shadow, and under this model it
// cannot.
//
// # The invariant
//
//     Privilege is granted by a privileged process to a child.
//     It is never acquired from a file.
//
// There is no setuid bit, no setgid bit and no file capabilities. exec() reads
// the mode bits, ls renders them, and the loader ignores them. On exec the
// permitted set is intersected with the bounding set and the effective set is
// replaced by it: the sets can only stay the same or shrink.
//
// Privilege-escalation CVEs in sudo needed a process that became root while
// holding input the attacker chose: PwnKit (argc == 0 walking off the end of
// argv), Baron Samedit (an argv unescape overflowing the heap after the setuid
// transition), the LD_PRELOAD, TZ and IFS families, the sudoedit races. Here
// the only code running with authority is code a privileged parent started, and
// that parent builds its child's argv and environment itself.
//
// See kernel/process/selftest_cred.rs, which asserts the invariant against a
// mode-4755 file, and docs/MikuOS_Security.md.

use crate::fs::vfs::types::{MAX_GROUPS, PermWho};

// capabilities

/// A set of capabilities, one bit each.
///
/// A bitset because the question is always whether one bit is set, on paths
/// that run per syscall
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapSet(pub u64);

/// Change identity: setuid/setresuid/seteuid to an arbitrary id
pub use miku_abi::cred::CAP_SETUID;
/// Change group identity, including setgroups
pub use miku_abi::cred::CAP_SETGID;
/// Signal a process you do not own
pub use miku_abi::cred::CAP_KILL;
/// Bypass file read/write/execute permission bits
pub use miku_abi::cred::CAP_DAC_OVERRIDE;
/// Bypass permission bits for reading and directory search only
pub use miku_abi::cred::CAP_DAC_READ;
/// Act as a file's owner without being it (chmod, utimes, xattr)
pub use miku_abi::cred::CAP_FOWNER;
/// Give a file away, or take one that is not yours
pub use miku_abi::cred::CAP_CHOWN;
/// Set the wall clock
pub use miku_abi::cred::CAP_SYS_TIME;
/// Mount, unmount, mkfs, and the machine-level shell commands
pub use miku_abi::cred::CAP_SYS_ADMIN;
/// Reboot and power off
pub use miku_abi::cred::CAP_SYS_BOOT;
/// Open a raw socket
pub use miku_abi::cred::CAP_NET_RAW;
/// Bind a port below 1024
pub use miku_abi::cred::CAP_NET_BIND;
/// Create a device node
pub use miku_abi::cred::CAP_MKNOD;
/// Bind the display server's privileged globals (may_wm, may_console,
/// may_layer)
pub use miku_abi::cred::CAP_MAY_PRIV;
/// Write to the security journal
pub use miku_abi::cred::CAP_AUDIT;
/// Submit work to the GPU directly
pub use miku_abi::cred::CAP_GPU;

/// The highest capability number that exists. Anything above is refused rather
/// than ignored: a capset() dropping an unknown bit would let a future kernel's
/// rule be bypassed by an old one's tolerance
pub use miku_abi::cred::CAP_LAST;

/// Every capability. What uid 0 holds, and what mikuD hands its own children
pub const CAP_ALL: CapSet = CapSet((1u64 << (CAP_LAST + 1)) - 1);

/// No capability at all. What an ordinary program runs with
pub const CAP_NONE: CapSet = CapSet(0);

impl CapSet {
    #[inline]
    pub const fn empty() -> Self { CapSet(0) }

    #[inline]
    pub const fn has(self, cap: u8) -> bool {
        cap <= CAP_LAST && (self.0 & (1u64 << cap)) != 0
    }

    #[inline]
    pub const fn with(self, cap: u8) -> Self {
        if cap > CAP_LAST { return self; }
        CapSet(self.0 | (1u64 << cap))
    }

    #[inline]
    pub const fn without(self, cap: u8) -> Self {
        if cap > CAP_LAST { return self; }
        CapSet(self.0 & !(1u64 << cap))
    }

    #[inline]
    pub const fn intersect(self, other: Self) -> Self { CapSet(self.0 & other.0) }

    #[inline]
    pub const fn is_empty(self) -> bool { self.0 == 0 }

    /// True when every bit in `self` is also in `other`. The test a capset()
    /// makes before accepting a requested set
    #[inline]
    pub const fn subset_of(self, other: Self) -> bool { (self.0 & !other.0) == 0 }

    /// Discard anything that is not a real capability. Applied to every set
    /// that arrives from ring 3
    #[inline]
    pub const fn sanitized(self) -> Self { CapSet(self.0 & CAP_ALL.0) }

    pub fn name(cap: u8) -> &'static str {
        match cap {
            CAP_SETUID       => "setuid",
            CAP_SETGID       => "setgid",
            CAP_KILL         => "kill",
            CAP_DAC_OVERRIDE => "dac_override",
            CAP_DAC_READ     => "dac_read",
            CAP_FOWNER       => "fowner",
            CAP_CHOWN        => "chown",
            CAP_SYS_TIME     => "sys_time",
            CAP_SYS_ADMIN    => "sys_admin",
            CAP_SYS_BOOT     => "sys_boot",
            CAP_NET_RAW      => "net_raw",
            CAP_NET_BIND     => "net_bind",
            CAP_MKNOD        => "mknod",
            CAP_MAY_PRIV     => "may_priv",
            CAP_AUDIT        => "audit",
            CAP_GPU          => "gpu",
            _                => "?",
        }
    }

    /// The capability a unit file named, or `None` if there is no such one
    ///
    /// The inverse of `name`, written against it, so a `Capabilities=` line
    /// grants the capability `id` prints
    pub fn by_name(name: &str) -> Option<u8> {
        (0..=CAP_LAST).find(|c| Self::name(*c) == name)
    }
}

// identity

/// The largest id that may be used. 0xFFFF is reserved as chown's "leave this
/// one alone" sentinel (see sys_chown), so it is not available as a user
pub use miku_abi::cred::ID_MAX;

/// The id a file gets when its on-disk owner does not fit in a u16. An ext4
/// volume written elsewhere can hold uid 65536, and truncating that to 16 bits
/// gives uid 0: a foreign disk would mount with root-owned files on it
pub const UID_NOBODY: u16 = 65534;
pub const GID_NOBODY: u16 = 65534;

/// Everything about who a process is, in one value.
///
/// Copy, and small (about 40 bytes): every path resolution reads one, so the
/// read path copies it out under a short read lock rather than holding a
/// reference into the PCB across a permission check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cred {
    /// Real user: who started this
    pub uid:  u16,
    /// Effective user: who it counts as for permission checks
    pub euid: u16,
    /// Saved user: what euid may be restored to without any capability
    pub suid: u16,
    pub gid:  u16,
    pub egid: u16,
    pub sgid: u16,
    pub groups:  [u16; MAX_GROUPS],
    pub ngroups: u8,
    /// What may be made effective
    pub permitted: CapSet,
    /// What is checked right now
    pub effective: CapSet,
    /// The ceiling. Only ever shrinks, and is inherited across fork and exec,
    /// so a subtree of the process tree can be permanently confined
    pub bounding:  CapSet,
    pub umask: u16,
}

impl Cred {
    /// uid 0 holding every capability. What PID 1 starts as, and what mikuD
    /// hands a service that has no User= of its own
    pub const fn root() -> Self {
        Self {
            uid: 0, euid: 0, suid: 0,
            gid: 0, egid: 0, sgid: 0,
            groups: [0; MAX_GROUPS],
            ngroups: 0,
            permitted: CAP_ALL,
            effective: CAP_ALL,
            bounding:  CAP_ALL,
            umask: 0o022,
        }
    }

    /// An ordinary user: no capability, and a bounding set that permits
    /// anything, because a descendant may be given authority by mikudod.
    /// Confinement is a separate, explicit act
    pub const fn user(uid: u16, gid: u16) -> Self {
        Self {
            uid, euid: uid, suid: uid,
            gid, egid: gid, sgid: gid,
            groups: [0; MAX_GROUPS],
            ngroups: 0,
            permitted: CAP_NONE,
            effective: CAP_NONE,
            bounding:  CAP_ALL,
            umask: 0o022,
        }
    }

    /// True when this credential holds `cap` effectively.
    ///
    /// It does not test euid == 0: a process running as uid 0 that dropped a
    /// capability does not get it back, and one running as uid 1000 that was
    /// granted one has it
    #[inline]
    pub const fn has(&self, cap: u8) -> bool { self.effective.has(cap) }

    /// The "is this root" question, for call sites not yet converted to ask for
    /// the capability they need. New code should not use it
    #[inline]
    pub const fn is_root(&self) -> bool { self.euid == 0 }

    /// Read/search permission may be bypassed by either of two capabilities:
    /// the broad one, or the read-only one that mikudod-style helpers get
    #[inline]
    pub const fn may_bypass_read(&self) -> bool {
        self.has(CAP_DAC_OVERRIDE) || self.has(CAP_DAC_READ)
    }

    #[inline]
    pub const fn may_bypass_write(&self) -> bool { self.has(CAP_DAC_OVERRIDE) }

    /// True when the caller may act as the owner of a file owned by `uid`:
    /// chmod it, stamp it, change its extended attributes
    #[inline]
    pub const fn owns_or_may(&self, file_uid: u16) -> bool {
        self.euid == file_uid || self.has(CAP_FOWNER)
    }

    pub fn in_group(&self, gid: u16) -> bool {
        if self.egid == gid {
            return true;
        }
        let n = (self.ngroups as usize).min(MAX_GROUPS);
        self.groups[..n].contains(&gid)
    }

    /// Which of the three permission triples applies to this credential for
    /// a file owned by (file_uid, file_gid)
    pub fn who_for(&self, file_uid: u16, file_gid: u16) -> PermWho {
        if self.euid == file_uid {
            PermWho::Owner
        } else if self.in_group(file_gid) {
            PermWho::Group
        } else {
            PermWho::Other
        }
    }

    pub fn set_groups(&mut self, src: &[u16]) {
        let n = src.len().min(MAX_GROUPS);
        self.groups = [0; MAX_GROUPS];
        self.groups[..n].copy_from_slice(&src[..n]);
        self.ngroups = n as u8;
    }

    /// What this credential becomes across exec.
    ///
    /// It only ever loses. There is no argument for the file being executed, so
    /// no file's contents or mode can raise anybody
    #[inline]
    pub fn after_exec(&self) -> Self {
        let mut next = *self;
        next.permitted = self.permitted.intersect(self.bounding);
        next.effective = next.permitted;
        next
    }

    /// Drop every capability. What setuid() to a non-zero id does, so that a
    /// privileged process becoming a user really becomes that user
    #[inline]
    pub fn drop_all_caps(&mut self) {
        self.permitted = CAP_NONE;
        self.effective = CAP_NONE;
    }

    /// Whether `target` is a uid this credential may switch its *effective*
    /// user to without holding CAP_SETUID. POSIX's rule: back to any of the
    /// three it already holds
    #[inline]
    pub const fn may_become_euid(&self, target: u16) -> bool {
        target == self.uid || target == self.euid || target == self.suid
    }

    #[inline]
    pub const fn may_become_egid(&self, target: u16) -> bool {
        target == self.gid || target == self.egid || target == self.sgid
    }
}

impl Default for Cred {
    fn default() -> Self { Self::root() }
}

/// Folds an on-disk 32-bit owner into the 16-bit one the VFS carries.
///
/// An ext4 volume formatted on Linux can name uid 100000, and `100000 as u16`
/// is 34464, a different user; 65536 truncates to 0, so a foreign disk would
/// mount with root-owned files on it. Anything that does not fit becomes
/// nobody, which owns nothing and is in no group
#[inline]
pub fn fold_id(id32: u32) -> u16 {
    if id32 > ID_MAX as u32 { UID_NOBODY } else { id32 as u16 }
}

/// Validates an id that arrived from ring 3.
///
/// Its own function so no call site truncates: `setuid(0x10000)` cut to 16 bits
/// stores uid 0, which is CVE-2019-14287 with different arithmetic. Every id
/// crossing the syscall boundary goes through here
#[inline]
pub fn checked_id(raw: u64) -> Option<u16> {
    if raw > ID_MAX as u64 { None } else { Some(raw as u16) }
}
