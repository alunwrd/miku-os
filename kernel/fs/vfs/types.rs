// The vnode table has no size: it grows with what is cached and gives it back
// (`vfs::table`, `dev/vfs-dentries-inodes.md`)

pub const MAX_MOUNTS: usize = 8;
// Per-process, both of them: `MAX_OPEN_FILES` is the length of the array inside
// one `FdTable`, one table per thread group (`vfs::fd`).
//
// 512: a program with a file watcher and a few language servers under it needs
// more than a shell does, and `getdents` takes a descriptor, so every directory
// read opens one
pub const MAX_FDS: usize = 512;
pub const MAX_OPEN_FILES: usize = 512;
// 0/1/2 reserved for stdin/stdout/stderr by the syscall layer.
pub const RESERVED_STDIO_FDS: usize = 3;
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
pub const MAX_BLOCK_DEVICES: usize = 8;
pub const MAX_SLAB_ITEMS: usize = 64;
pub const MAX_SECURITY_LABELS: usize = 8;
pub const MAX_AUDIT_LOG: usize = 32;
pub const MAX_NOTIFY_EVENTS: usize = 16;
/// Supplementary groups a process may carry. wheel, users, video, audio, input,
/// disk, shadow and a primary group already make eight
pub const MAX_GROUPS: usize = 16;
/// The widest single path component this VFS stores.
///
/// 255, what ext allows, and required for correctness: `rmdir`, `rename` and
/// `unlink` hand the vnode's own name back to the filesystem as authoritative,
/// so a truncated name would ask the disk to delete a name that does not exist,
/// and two names sharing a prefix would be one.
///
/// The `getdents` ABI has its own limit (`abi::UDIRENT_NAME_LEN`) and truncates
/// there, visibly.
pub const NAME_LEN: usize = 255;

/// The longest symlink target, which is a path and not a name. Targets are on
/// the heap and only symlinks pay for them
pub const SYMLINK_MAX: usize = 4096;
/// A tmpfs page, which is a frame (`vfs::pages`)
pub const PAGE_SIZE: usize = 4096;
pub const BLOCK_SIZE: usize = 512;
pub const MAX_SYMLINK_DEPTH: usize = 8;
pub const MAX_PATH_DEPTH: usize = 32;

pub type Timestamp = u64;
pub type VfsResult<T> = Result<T, VfsError>;
/// A vnode's id in the table. Thirty-two bits: the table grows (`vfs::table`)
pub type InodeId = u32;
/// A page of the tmpfs cache (`vfs::pages`)
pub type PageId = u32;
pub type BlockDevId = u8;

pub const INVALID_ID: InodeId = InodeId::MAX;
pub const INVALID_PAGE: PageId = PageId::MAX;
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
    /// The link count would pass what `i_links_count` can express
    TooManyLinks = 40,
    /// A structure the operation needs did not fit in memory
    OutOfMemory = 41,
    /// The node names a terminal and the caller is at none: what `/dev/tty`
    /// answers a process with no controlling terminal. `ENXIO`
    NoTerminal = 42,
    /// A walk stopped at a name it would have read from the disk with the
    /// VFS lock held, and left the read for `core::with_vfs_walk` to make
    /// without it. Never reaches a program: the walk runs again
    Deferred = 43,
}

/// What the ext driver's errors mean to the layer above.
///
/// A full disk, a directory with files in it, an existing name and a broken
/// sector are different answers, and ring 3 sees only what comes out of here:
/// `ENOSPC` told as `EIO` reports a hardware fault when the answer is to delete
/// something.
impl From<crate::fs::ext::FsError> for VfsError {
    fn from(e: crate::fs::ext::FsError) -> Self {
        use crate::fs::ext::FsError as F;
        match e {
            F::NotFound | F::InvalidInode => VfsError::NotFound,
            F::NotDirectory => VfsError::NotDirectory,
            F::IsDirectory => VfsError::IsDirectory,
            F::NotEmpty => VfsError::NotEmpty,
            F::AlreadyExists => VfsError::AlreadyExists,
            F::NoSpace | F::ExtentFull | F::JournalFull => VfsError::NoSpace,
            F::ReadOnlyFs => VfsError::ReadOnly,
            F::PermissionDenied => VfsError::PermissionDenied,
            F::NameTooLong => VfsError::NameTooLong,
            F::TooManyLinks => VfsError::TooManyLinks,
            F::FileTooLarge => VfsError::FileTooLarge,
            F::NotSymlink => VfsError::NotSymlink,
            F::LinkLoop => VfsError::Loop,
            F::NotRegularFile => VfsError::InvalidArgument,
            F::InvalidArg | F::BufferTooSmall => VfsError::InvalidArgument,
            F::InlineTooBig => VfsError::XattrTooLarge,
            // The filesystem is not what it claims to be. Distinct from an
            // I/O error: the read succeeded and what came back was wrong
            F::CorruptedFs
            | F::ChecksumError
            | F::BadMagic
            | F::InvalidBlock
            | F::InvalidExtent
            | F::TooManyGroups => VfsError::Corrupted,
            F::UnsupportedFeature | F::UnsupportedVersion | F::NoJournal => {
                VfsError::NotSupported
            }
            F::OutOfMemory => VfsError::OutOfMemory,
            F::IoError => VfsError::IoError,
        }
    }
}

/// What the block layer's errors mean. A device that is not there, one that
/// timed out and one that refuses the operation are three different answers
impl From<crate::io::block::driver::BlkError> for VfsError {
    fn from(e: crate::io::block::driver::BlkError) -> Self {
        use crate::io::block::driver::BlkError as B;
        match e {
            B::NoDevice | B::Offline => VfsError::NotMounted,
            B::BufferTooSmall => VfsError::InvalidArgument,
            B::ReadOnly => VfsError::ReadOnly,
            B::Unsupported => VfsError::NotSupported,
            B::DeviceFault | B::Timeout => VfsError::IoError,
        }
    }
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
            Self::TooManyLinks => "too many links",
            Self::OutOfMemory => "out of memory",
            Self::NoTerminal => "no controlling terminal",
            Self::Deferred => "deferred to a read without the VFS lock",
        }
    }
}

impl core::fmt::Display for VfsError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Which filesystem a vnode belongs to.
///
/// Adding a filesystem means implementing `fsops::FileSystem` and a line in
/// `fs::registry::PROBERS`. The variant here is a label. Discriminants 6 and 7
/// once meant something else and are not reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FsType {
    TmpFS  = 0,
    DevFS  = 1,
    ProcFS = 2,
    Ext2   = 3,
    Ext3   = 4,
    Ext4   = 5,
    /// 8: discriminants 6 and 7 are not reused, as a stale stamp on a vnode
    /// would come back as the wrong answer
    Btrfs  = 8,
    /// 9, by the same rule
    MemFs  = 9,
}

impl FsType {
    #[inline]
    pub fn is_ext_family(self) -> bool {
        matches!(self, FsType::Ext2 | FsType::Ext3 | FsType::Ext4)
    }

    /// Whether a mounted filesystem stands behind this, as against the vnode
    /// table being the storage itself. Includes `MemFs`: mounted, reached
    /// through `registry::with_mounted` as ext is, with no disk.
    #[inline]
    pub fn has_mounted_fs(self) -> bool {
        !matches!(self, FsType::TmpFS | FsType::DevFS | FsType::ProcFS)
    }

    pub fn magic(self) -> u32 {
        match self {
            FsType::Ext2 | FsType::Ext3 | FsType::Ext4 => 0xEF53,
            FsType::TmpFS  => 0x01021994,
            FsType::DevFS  => 0x1373,
            FsType::ProcFS => 0x9FA0,
            // `_BHRfS_M`, truncated to the 32 bits of this field. The whole
            // eight bytes are in `lib/fs/btrfs`; this is only what statfs prints
            FsType::Btrfs  => 0x9123_683E,
            // `MEMF`. With no superblock, this is a name for `statfs` to print
            FsType::MemFs  => 0x4D45_4D46,
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
            FsType::Btrfs  => "btrfs",
            FsType::MemFs  => "memfs",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermWho {
    Owner,
    Group,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags(pub u32);

impl OpenFlags {
    pub const EMPTY: Self = Self(0);
    pub const READ: u32 = miku_abi::fcntl::O_RDONLY as u32;
    pub const WRITE: u32 = miku_abi::fcntl::O_WRONLY as u32;
    pub const RDWR: u32 = miku_abi::fcntl::O_RDWR as u32;
    pub const APPEND: u32 = miku_abi::fcntl::O_APPEND as u32;
    pub const CREATE: u32 = miku_abi::fcntl::O_CREAT as u32;
    pub const EXCLUSIVE: u32 = miku_abi::fcntl::O_EXCL as u32;
    pub const TRUNCATE: u32 = miku_abi::fcntl::O_TRUNC as u32;
    pub const DIRECTORY: u32 = miku_abi::fcntl::O_DIRECTORY as u32;
    pub const NOFOLLOW: u32 = miku_abi::fcntl::O_NOFOLLOW as u32;
    pub const DIRECT: u32 = 0x0100;
    pub const SYNC: u32 = miku_abi::fcntl::O_SYNC as u32;
    pub const NONBLOCK: u32 = miku_abi::fcntl::O_NONBLOCK as u32;
    pub const CLOEXEC: u32 = miku_abi::fcntl::O_CLOEXEC as u32;
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
    /// `VNode::ino`: stable, and the same for every name of a file
    pub ino: u64,
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
    /// `VNode::ino`, as `stat` says it
    pub inode_id: u64,
    pub kind: VNodeKind,
    pub offset: u32,
}

impl DirEntry {
    pub const fn empty() -> Self {
        Self {
            name: [0; NAME_LEN],
            name_len: 0,
            inode_id: 0,
            kind: VNodeKind::Regular,
            offset: 0,
        }
    }

    /// This entry made `nm`, written in place: only the name's bytes move, and
    /// the rest of the array keeps what it held, unread past `name_len`.
    /// Building each entry by value would zero 255 bytes for a name of six
    pub fn set(&mut self, nm: &str, id: u64, kind: VNodeKind, offset: u32) {
        let bytes = nm.as_bytes();
        let len = bytes.len().min(NAME_LEN);
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name_len = len as u8;
        self.inode_id = id;
        self.kind = kind;
        self.offset = offset;
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

// Who a process is, for the permission checks below: the same value the
// scheduler holds
pub use crate::process::cred::Cred as Credentials;

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

/// Where a symlink points.
///
/// A path, not a name, on the heap: only symlinks pay, and they can hold a
/// whole path.
#[derive(Debug, Default)]
pub struct SymlinkTarget(Option<alloc::boxed::Box<[u8]>>);

impl SymlinkTarget {
    pub const fn none() -> Self {
        Self(None)
    }

    pub fn set(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            self.0 = None;
            return;
        }
        let n = bytes.len().min(SYMLINK_MAX);
        self.0 = Some(bytes[..n].into());
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }

    pub fn bytes(&self) -> &[u8] {
        match &self.0 {
            Some(b) => b,
            None => &[],
        }
    }

    pub fn len(&self) -> usize {
        self.bytes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The target as text. A target that is not UTF-8 is not a path this kernel
    /// can walk, and the replacement character makes the failure a `NotFound`
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.bytes()).unwrap_or("\u{FFFD}")
    }
}

/// A single path component, stored by value.
///
/// `len` is a `u8` and `NAME_LEN` is 255, the largest value a `u8` holds
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
