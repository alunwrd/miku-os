pub const MAX_VNODES: usize = 256;
pub const MAX_DENTRIES: usize = 128;
pub const MAX_MOUNTS: usize = 8;
// Per-process FD table size. With per-process tables (vfs.fd_tables
// keyed by pid) this is the upper bound for a single process, not a
// system-wide cap. 128 covers stdio + dynamic linker + heap-allocator
// fds + dozens of open files comfortably
pub const MAX_FDS: usize = 128;
pub const MAX_OPEN_FILES: usize = 128;
// Syscall layer handles 0/1/2 as stdin/stdout/stderr //
pub const RESERVED_STDIO_FDS: usize = 3;
pub const MAX_DATA_PAGES: usize = 128;
pub const MAX_XATTRS_PER_NODE: usize = 8;
pub const MAX_LOCKS: usize = 16;
pub const MAX_WATCHES: usize = 8;
pub const MAX_QUOTA_ENTRIES: usize = 4;
pub const MAX_TRANSACTIONS: usize = 4;
pub const MAX_TX_OPS: usize = 16;
pub const MAX_VERSIONS: usize = 16;
pub const MAX_CAS_OBJECTS: usize = 16;
pub const MAX_JOURNAL_BLOCKS: usize = 16;
pub const MAX_BIO_QUEUE: usize = 8;
pub const MAX_BIO_SEGMENTS: usize = 4;
pub const MAX_BLOCK_DEVICES: usize = 4;
pub const MAX_SLAB_ITEMS: usize = 64;
pub const MAX_SECURITY_LABELS: usize = 8;
pub const MAX_AUDIT_LOG: usize = 32;
pub const MAX_NOTIFY_EVENTS: usize = 16;
pub const MAX_GROUPS: usize = 8;
pub const NAME_LEN: usize = 64;
pub const PAGE_SIZE: usize = 512;
pub const BLOCK_SIZE: usize = 512;
pub const MAX_SYMLINK_DEPTH: usize = 8;
pub const MAX_PATH_DEPTH: usize = 32;
pub const DIRECT_BLOCKS: usize = 64;
pub const INDIRECT_ENTRIES: usize = 64;
pub const MAX_FILE_SIZE: u64 = (DIRECT_BLOCKS as u64) * (PAGE_SIZE as u64);

pub type Timestamp = u64;
pub type VfsResult<T> = Result<T, VfsError>;
pub type InodeId = u16;
pub type PageId = u16;
pub type DentryId = u16;
pub type BlockDevId = u8;

pub const INVALID_ID: u16 = 0xFFFF;
pub const INVALID_U8: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VfsError {
    NotFound = 1,
    PermissionDenied = 2,
    AlreadyExists = 3,
    NotDirectory = 4,
    IsDirectory = 5,
    NotEmpty = 6,
    InvalidPath = 7,
    TooManySymlinks = 8,
    NoSpace = 9,
    ReadOnly = 10,
    Busy = 11,
    IoError = 12,
    CrossDevice = 13,
    NameTooLong = 14,
    InvalidArgument = 15,
    NotSupported = 16,
    BadFd = 17,
    TooManyOpenFiles = 18,
    SeekError = 19,
    Deadlock = 20,
    WouldBlock = 21,
    QuotaExceeded = 22,
    Interrupted = 23,
    FileTooLarge = 24,
    NotSymlink = 25,
    Loop = 26,
    Stale = 27,
    TxConflict = 28,
    Corrupted = 29,
    SecurityViolation = 30,
    NotMounted = 31,
    AlreadyMounted = 32,
    InvalidXattr = 33,
    XattrTooLarge = 34,
    NoLock = 35,
    PipeEmpty = 36,
    PipeFull = 37,
    BrokenPipe = 38,
    InvalidUtf8 = 39,
}

impl VfsError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotFound => "not found",
            Self::PermissionDenied => "permission denied",
            Self::AlreadyExists => "already exists",
            Self::NotDirectory => "not a directory",
            Self::IsDirectory => "is a directory",
            Self::NotEmpty => "directory not empty",
            Self::InvalidPath => "invalid path",
            Self::TooManySymlinks => "too many symlinks",
            Self::NoSpace => "no space left",
            Self::ReadOnly => "read-only filesystem",
            Self::Busy => "resource busy",
            Self::IoError => "I/O error",
            Self::CrossDevice => "cross-device link",
            Self::NameTooLong => "name too long",
            Self::InvalidArgument => "invalid argument",
            Self::NotSupported => "not supported",
            Self::BadFd => "bad file descriptor",
            Self::TooManyOpenFiles => "too many open files",
            Self::SeekError => "seek error",
            Self::Deadlock => "deadlock detected",
            Self::WouldBlock => "would block",
            Self::QuotaExceeded => "quota exceeded",
            Self::Interrupted => "interrupted",
            Self::FileTooLarge => "file too large",
            Self::NotSymlink => "not a symlink",
            Self::Loop => "loop detected",
            Self::Stale => "stale handle",
            Self::TxConflict => "transaction conflict",
            Self::Corrupted => "data corrupted",
            Self::SecurityViolation => "security violation",
            Self::NotMounted => "not mounted",
            Self::AlreadyMounted => "already mounted",
            Self::InvalidXattr => "invalid xattr",
            Self::XattrTooLarge => "xattr too large",
            Self::NoLock => "no lock held",
            Self::PipeEmpty => "pipe empty",
            Self::PipeFull => "pipe full",
            Self::BrokenPipe => "broken pipe",
            Self::InvalidUtf8 => "invalid utf-8 in name",
        }
    }
}

impl core::fmt::Display for VfsError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FsType {
    TmpFS  = 0,
    DevFS  = 1,
    ProcFS = 2,
    Ext2   = 3,
    Ext3   = 4,
    Ext4   = 5,
    CowFS  = 6,
    PipeFS = 7,
}

impl FsType {
    #[inline]
    pub fn is_ext_family(self) -> bool {
        matches!(self, FsType::Ext2 | FsType::Ext3 | FsType::Ext4)
    }

    pub fn magic(self) -> u32 {
        match self {
            FsType::Ext2 | FsType::Ext3 | FsType::Ext4 => 0xEF53,
            FsType::TmpFS  => 0x01021994,
            FsType::DevFS  => 0x1373,
            FsType::ProcFS => 0x9FA0,
            FsType::PipeFS => 0x50495045,
            FsType::CowFS  => 0xC0FF,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FsType::TmpFS  => "tmpfs",
            FsType::DevFS  => "devfs",
            FsType::ProcFS => "procfs",
            FsType::Ext2   => "ext2",
            FsType::Ext3   => "ext3",
            FsType::Ext4   => "ext4",
            FsType::CowFS  => "cowfs",
            FsType::PipeFS => "pipefs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VNodeKind {
    Regular = 0,
    Directory = 1,
    Symlink = 2,
    CharDevice = 3,
    BlockDevice = 4,
    Pipe = 5,
    Socket = 6,
    Fifo = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMode(pub u16);

impl FileMode {
    pub const fn new(mode: u16) -> Self {
        Self(mode & 0o7777)
    }
    pub const fn default_file() -> Self {
        Self(0o644)
    }
    pub const fn default_dir() -> Self {
        Self(0o755)
    }
    pub const fn default_symlink() -> Self {
        Self(0o777)
    }
    pub const fn default_dev() -> Self {
        Self(0o666)
    }
    pub const fn default_pipe() -> Self {
        Self(0o600)
    }

    pub const fn owner_read(&self) -> bool {
        self.0 & 0o400 != 0
    }
    pub const fn owner_write(&self) -> bool {
        self.0 & 0o200 != 0
    }
    pub const fn owner_exec(&self) -> bool {
        self.0 & 0o100 != 0
    }
    pub const fn group_read(&self) -> bool {
        self.0 & 0o040 != 0
    }
    pub const fn group_write(&self) -> bool {
        self.0 & 0o020 != 0
    }
    pub const fn group_exec(&self) -> bool {
        self.0 & 0o010 != 0
    }
    pub const fn other_read(&self) -> bool {
        self.0 & 0o004 != 0
    }
    pub const fn other_write(&self) -> bool {
        self.0 & 0o002 != 0
    }
    pub const fn other_exec(&self) -> bool {
        self.0 & 0o001 != 0
    }
    pub const fn setuid(&self) -> bool {
        self.0 & 0o4000 != 0
    }
    pub const fn setgid(&self) -> bool {
        self.0 & 0o2000 != 0
    }
    pub const fn sticky(&self) -> bool {
        self.0 & 0o1000 != 0
    }

    pub fn apply_umask(self, umask: u16) -> Self {
        Self::new(self.0 & !umask)
    }

    pub const fn perm_bits_for(&self, who: PermWho) -> u8 {
        match who {
            PermWho::Owner => ((self.0 >> 6) & 0o7) as u8,
            PermWho::Group => ((self.0 >> 3) & 0o7) as u8,
            PermWho::Other => (self.0 & 0o7) as u8,
        }
    }

    pub fn format_string(&self, kind: VNodeKind) -> [u8; 10] {
        let mut out = [b'-'; 10];
        out[0] = match kind {
            VNodeKind::Directory => b'd',
            VNodeKind::Symlink => b'l',
            VNodeKind::CharDevice => b'c',
            VNodeKind::BlockDevice => b'b',
            VNodeKind::Pipe | VNodeKind::Fifo => b'p',
            VNodeKind::Socket => b's',
            _ => b'-',
        };
        if self.owner_read() { out[1] = b'r'; }
        if self.owner_write() { out[2] = b'w'; }
        if self.owner_exec() {
            out[3] = if self.setuid() { b's' } else { b'x' };
        } else if self.setuid() {
            out[3] = b'S';
        }
        if self.group_read() { out[4] = b'r'; }
        if self.group_write() { out[5] = b'w'; }
        if self.group_exec() {
            out[6] = if self.setgid() { b's' } else { b'x' };
        } else if self.setgid() {
            out[6] = b'S';
        }
        if self.other_read() { out[7] = b'r'; }
        if self.other_write() { out[8] = b'w'; }
        if self.other_exec() {
            out[9] = if self.sticky() { b't' } else { b'x' };
        } else if self.sticky() {
            out[9] = b'T';
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PermWho {
    Owner,
    Group,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags(pub u32);

impl OpenFlags {
    pub const EMPTY: Self = Self(0);
    pub const READ: u32 = 0x0001;
    pub const WRITE: u32 = 0x0002;
    pub const RDWR: u32 = 0x0003;
    pub const APPEND: u32 = 0x0004;
    pub const CREATE: u32 = 0x0008;
    pub const EXCLUSIVE: u32 = 0x0010;
    pub const TRUNCATE: u32 = 0x0020;
    pub const DIRECTORY: u32 = 0x0040;
    pub const NOFOLLOW: u32 = 0x0080;
    pub const DIRECT: u32 = 0x0100;
    pub const SYNC: u32 = 0x0200;
    pub const NONBLOCK: u32 = 0x0400;
    pub const CLOEXEC: u32 = 0x0800;
    pub const NOATIME: u32 = 0x1000;

    #[inline]
    pub const fn has(&self, flag: u32) -> bool {
        self.0 & flag != 0
    }
    #[inline]
    pub const fn readable(&self) -> bool {
        self.has(Self::READ)
    }
    #[inline]
    pub const fn writable(&self) -> bool {
        self.has(Self::WRITE)
    }
    #[inline]
    pub const fn is_rdwr(&self) -> bool {
        self.0 & Self::RDWR == Self::RDWR
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SeekFrom {
    Start(u64),
    Current(i64),
    End(i64),
}

#[derive(Debug, Clone, Copy)]
pub struct VNodeStat {
    pub id: InodeId,
    pub kind: VNodeKind,
    pub mode: FileMode,
    pub size: u64,
    pub blocks: u32,
    pub nlinks: u16,
    pub uid: u16,
    pub gid: u16,
    pub fs_type: FsType,
    pub dev_major: u8,
    pub dev_minor: u8,
    pub atime: Timestamp,
    pub mtime: Timestamp,
    pub ctime: Timestamp,
    pub btime: Timestamp,
}

#[derive(Debug, Clone, Copy)]
pub struct StatFs {
    pub fs_type: FsType,
    pub block_size: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub max_name_len: u32,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub name: [u8; NAME_LEN],
    pub name_len: u8,
    pub inode_id: InodeId,
    pub kind: VNodeKind,
    pub offset: u32,
}

impl DirEntry {
    pub const fn empty() -> Self {
        Self {
            name: [0; NAME_LEN],
            name_len: 0,
            inode_id: INVALID_ID,
            kind: VNodeKind::Regular,
            offset: 0,
        }
    }

    pub fn from_name(nm: &str, id: InodeId, kind: VNodeKind) -> Self {
        let mut entry = Self::empty();
        let bytes = nm.as_bytes();
        let len = bytes.len().min(NAME_LEN);
        entry.name[..len].copy_from_slice(&bytes[..len]);
        entry.name_len = len as u8;
        entry.inode_id = id;
        entry.kind = kind;
        entry
    }

    pub fn get_name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SetAttr {
    pub mode: Option<FileMode>,
    pub uid: Option<u16>,
    pub gid: Option<u16>,
    pub size: Option<u64>,
    pub atime: Option<Timestamp>,
    pub mtime: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy)]
pub struct Credentials {
    pub uid: u16,
    pub gid: u16,
    pub euid: u16,
    pub egid: u16,
    pub groups: [u16; MAX_GROUPS],
    pub ngroups: u8,
}

impl Credentials {
    pub const fn root() -> Self {
        Self {
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            groups: [0; MAX_GROUPS],
            ngroups: 1,
        }
    }

    pub const fn user(uid: u16, gid: u16) -> Self {
        let mut groups = [0u16; MAX_GROUPS];
        groups[0] = gid;
        Self {
            uid,
            gid,
            euid: uid,
            egid: gid,
            groups,
            ngroups: 1,
        }
    }

    pub const fn is_root(&self) -> bool {
        self.euid == 0
    }

    pub fn in_group(&self, gid: u16) -> bool {
        if self.egid == gid {
            return true;
        }
        for i in 0..self.ngroups as usize {
            if i < MAX_GROUPS && self.groups[i] == gid {
                return true;
            }
        }
        false
    }

    pub fn who_for(&self, file_uid: u16, file_gid: u16) -> PermWho {
        if self.euid == file_uid {
            PermWho::Owner
        } else if self.in_group(file_gid) {
            PermWho::Group
        } else {
            PermWho::Other
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessContext {
    pub pid: u16,
    pub cred: Credentials,
    pub umask: u16,
    pub cwd: InodeId,
    pub root: InodeId,
}

impl ProcessContext {
    pub const fn root_context() -> Self {
        Self {
            pid: 0,
            cred: Credentials::root(),
            umask: 0o022,
            cwd: 0,
            root: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct NameBuf {
    pub data: [u8; NAME_LEN],
    pub len: u8,
}

impl NameBuf {
    pub const fn empty() -> Self {
        Self {
            data: [0; NAME_LEN],
            len: 0,
        }
    }

    pub fn from_str(s: &str) -> Self {
        let mut nb = Self::empty();
        let bytes = s.as_bytes();
        let len = bytes.len().min(NAME_LEN);
        nb.data[..len].copy_from_slice(&bytes[..len]);
        nb.len = len as u8;
        nb
    }

    pub fn from_bytes_checked(bytes: &[u8]) -> Result<Self, VfsError> {
        core::str::from_utf8(bytes).map_err(|_| VfsError::InvalidUtf8)?;
        let mut nb = Self::empty();
        let len = bytes.len().min(NAME_LEN);
        nb.data[..len].copy_from_slice(&bytes[..len]);
        nb.len = len as u8;
        Ok(nb)
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.data[..self.len as usize]).unwrap_or("\u{FFFD}")
    }

    pub fn eq_str(&self, s: &str) -> bool {
        let bytes = s.as_bytes();
        self.len as usize == bytes.len() && &self.data[..self.len as usize] == bytes
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl core::fmt::Debug for NameBuf {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "\"{}\"", self.as_str())
    }
}

pub struct PathBuf {
    pub data: [u8; 256],
    pub len: usize,
}

impl PathBuf {
    pub const fn empty() -> Self {
        Self {
            data: [0; 256],
            len: 0,
        }
    }

    pub fn from_str(s: &str) -> Self {
        let mut pb = Self::empty();
        let bytes = s.as_bytes();
        let len = bytes.len().min(256);
        pb.data[..len].copy_from_slice(&bytes[..len]);
        pb.len = len;
        pb
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.data[..self.len]).unwrap_or("")
    }

    pub fn push(&mut self, component: &str) {
        if self.len > 0 && self.data[self.len - 1] != b'/' {
            if self.len < 255 {
                self.data[self.len] = b'/';
                self.len += 1;
            }
        }
        let bytes = component.as_bytes();
        let avail = 256 - self.len;
        let len = bytes.len().min(avail);
        self.data[self.len..self.len + len].copy_from_slice(&bytes[..len]);
        self.len += len;
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn set_root(&mut self) {
        self.data[0] = b'/';
        self.len = 1;
    }
}
