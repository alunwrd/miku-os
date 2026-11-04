/// What went wrong, for the layer above. Variants map to different errnos, so a
/// full disk does not report as a broken one
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    BadMagic,
    IoError,
    UnsupportedVersion,
    TooManyGroups,
    InvalidInode,
    NotDirectory,
    NotFound,
    CorruptedFs,
    BufferTooSmall,
    FileTooLarge,
    NotRegularFile,
    InvalidBlock,
    NoSpace,
    AlreadyExists,
    NotEmpty,
    IsDirectory,
    NoJournal,
    ExtentFull,
    ChecksumError,
    InlineTooBig,
    UnsupportedFeature,
    ReadOnlyFs,
    InvalidExtent,
    JournalFull,
    NotSymlink,
    LinkLoop,
    InvalidArg,
    PermissionDenied,
    /// A name longer than a directory entry can express (255 bytes), or not
    /// fitting in one block
    NameTooLong,
    /// The link count would exceed `EXT2_LINK_MAX`; wrapping the u16 would free
    /// an inode with live references
    TooManyLinks,
    /// A structure proportional to the filesystem did not fit: a check of a
    /// large volume needs a bit per block and per inode
    OutOfMemory,
}
