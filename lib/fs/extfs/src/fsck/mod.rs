// Checking a filesystem and repairing it
//
// Five passes as in e2fsck, in that order since the questions depend on each
// other: a directory entry needs live inodes, a link count needs all
// directories, and a bitmap needs ownership.
//
//   0  the superblock and group descriptors
//   1  every inode and each block it claims
//   2  every directory: its entries, names, `.` and `..`
//   3  connectivity: everything reachable from the root
//   4  link counts against pass 2's references
//   5  bitmaps and free counters against passes 1 and 2
//
// Repair is the same code with `repair` set: each pass fixes what it found
// before the next runs. Writes go through the ordinary metadata path and
// journal, so a power loss mid-repair replays.
//
// The check runs on a mounted filesystem, unlike e2fsck, as MikuOS has no
// second machine to boot. Repair requires a replayed journal and a writable
// volume; another task writing is the user's risk, and the command says so.

use crate::error::FsError;
use crate::fs::MikuFS;
use crate::host::ExtHost;
use crate::structs::*;

pub mod maps;
mod counts;
mod dirs;
mod scan;

use maps::{BitMap, Bounded, DirParents, LinkCounts};

/// How the check should behave
#[derive(Clone, Copy, Debug)]
pub struct FsckOptions {
    /// Writes the fixes back. Without it the medium is untouched, and the
    /// report says what would be done
    pub repair: bool,
    /// Examples kept per report; counts are exact regardless
    pub examples: usize,
}

impl FsckOptions {
    pub const fn check() -> Self {
        Self { repair: false, examples: 64 }
    }

    pub const fn repair() -> Self {
        Self { repair: true, examples: 64 }
    }
}

impl Default for FsckOptions {
    fn default() -> Self {
        Self::check()
    }
}

/// One thing wrong with the filesystem, carrying its object: "inode 4712 claims
/// block 91004, already claimed by inode 118"
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    // pass 0
    BadMagic,
    BadSuperblockChecksum,
    /// The geometry is inconsistent: group count, blocks per group, and block
    /// count disagree
    BadGeometry,
    GroupDescChecksum { group: u32 },
    /// A group's bitmap or inode table is outside the filesystem
    GroupMetaOutOfRange { group: u32, block: u64 },
    /// Two groups' metadata occupy the same block
    GroupMetaOverlap { group: u32, block: u64 },

    // pass 1
    /// An extent tree node with an unbelievable header: bad magic, too many
    /// entries, impossible depth
    BadExtentHeader { ino: u32, block: u32 },
    BadExtentChecksum { ino: u32, block: u32 },
    /// A block pointer outside the filesystem
    BlockOutOfRange { ino: u32, block: u32 },
    /// A file claiming a block that belongs to the filesystem's own structures
    BlockInMetadata { ino: u32, block: u32 },
    /// Two inodes claiming one block
    BlockClaimedTwice { ino: u32, block: u32 },
    /// `i_blocks` disagrees with the blocks the inode actually reaches
    BlockCountWrong { ino: u32, on_disk: u64, counted: u64 },
    /// An inode whose checksum does not match its contents
    InodeBadChecksum { ino: u32 },
    /// Zero links and no deletion time: an unfinished free the orphan list does
    /// not mention
    InodeZeroLinks { ino: u32 },
    /// A mode that is none of the seven file types
    InodeBadMode { ino: u32, mode: u16 },
    /// Inode 2 is not a directory, so there is no tree
    RootNotDirectory,
    XattrRefcountWrong { block: u32, on_disk: u32, counted: u32 },

    // pass 2
    /// A record length not a multiple of four, shorter than the declared name,
    /// or past the block's end
    DirBadRecLen { ino: u32, block: u32, at: u32 },
    /// The chain of records does not end at the block's end
    DirBadChain { ino: u32, block: u32 },
    DirBadChecksum { ino: u32, block: u32 },
    /// An index node whose header does not describe this size, so its entries
    /// cannot be followed
    DirBadIndex { ino: u32, block: u32 },
    /// An entry pointing at an inode number the filesystem lacks
    DirEntryOutOfRange { ino: u32, child: u32 },
    /// An entry pointing at an inode nothing lives in
    DirEntryFreeInode { ino: u32, child: u32 },
    /// The entry's file type is not the inode's
    DirEntryBadType { ino: u32, child: u32 },
    DirMissingDot { ino: u32 },
    DirMissingDotDot { ino: u32 },
    DirDuplicateName { ino: u32, child: u32 },
    /// A name containing a slash or a NUL, which no name may
    DirBadName { ino: u32, child: u32 },

    // pass 3
    /// A directory that cannot be reached from the root
    DirDisconnected { ino: u32 },
    /// `..` does not point at the directory holding this one's name
    DirDotDotWrong { ino: u32, was: u32, should: u32 },
    /// A directory named from two places, which no operation can produce,
    /// making the tree a graph with a cycle
    DirTwoParents { ino: u32 },

    // pass 4
    LinkCountWrong { ino: u32, on_disk: u16, counted: u32 },
    /// An inode with links but no name anywhere
    InodeUnattached { ino: u32 },

    // pass 5
    BlockBitmapWrong { group: u32, differences: u32 },
    InodeBitmapWrong { group: u32, differences: u32 },
    BitmapChecksumWrong { group: u32, inodes: bool },
    GroupFreeBlocksWrong { group: u32, on_disk: u32, counted: u32 },
    GroupFreeInodesWrong { group: u32, on_disk: u32, counted: u32 },
    GroupUsedDirsWrong { group: u32, on_disk: u32, counted: u32 },
    GroupItableUnusedWrong { group: u32, on_disk: u32, counted: u32 },
    SuperFreeBlocksWrong { on_disk: u64, counted: u64 },
    SuperFreeInodesWrong { on_disk: u32, counted: u32 },
    /// The volume is flagged as having had errors
    FilesystemMarkedBad,
}

impl Problem {
    /// Which object the report is about, for grouping by inode or group without
    /// matching thirty variants
    pub fn subject(&self) -> Subject {
        use Problem::*;
        match *self {
            BadMagic | BadSuperblockChecksum | BadGeometry | FilesystemMarkedBad
            | RootNotDirectory
            | SuperFreeBlocksWrong { .. } | SuperFreeInodesWrong { .. } => Subject::Volume,

            GroupDescChecksum { group }
            | GroupMetaOutOfRange { group, .. }
            | GroupMetaOverlap { group, .. }
            | BlockBitmapWrong { group, .. }
            | InodeBitmapWrong { group, .. }
            | BitmapChecksumWrong { group, .. }
            | GroupFreeBlocksWrong { group, .. }
            | GroupFreeInodesWrong { group, .. }
            | GroupUsedDirsWrong { group, .. }
            | GroupItableUnusedWrong { group, .. } => Subject::Group(group),

            XattrRefcountWrong { block, .. } => Subject::Block(block),

            BadExtentHeader { ino, .. }
            | BadExtentChecksum { ino, .. }
            | BlockOutOfRange { ino, .. }
            | BlockInMetadata { ino, .. }
            | BlockClaimedTwice { ino, .. }
            | BlockCountWrong { ino, .. }
            | InodeBadChecksum { ino }
            | InodeZeroLinks { ino }
            | InodeBadMode { ino, .. }
            | DirBadRecLen { ino, .. }
            | DirBadChain { ino, .. }
            | DirBadChecksum { ino, .. }
            | DirBadIndex { ino, .. }
            | DirEntryOutOfRange { ino, .. }
            | DirEntryFreeInode { ino, .. }
            | DirEntryBadType { ino, .. }
            | DirMissingDot { ino }
            | DirMissingDotDot { ino }
            | DirDuplicateName { ino, .. }
            | DirBadName { ino, .. }
            | DirDisconnected { ino }
            | DirDotDotWrong { ino, .. }
            | DirTwoParents { ino }
            | LinkCountWrong { ino, .. }
            | InodeUnattached { ino } => Subject::Inode(ino),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Volume,
    Group(u32),
    Block(u32),
    Inode(u32),
}

impl core::fmt::Display for Problem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use Problem::*;
        match *self {
            BadMagic => write!(f, "superblock magic is not ext2/3/4"),
            BadSuperblockChecksum => write!(f, "superblock checksum does not match"),
            BadGeometry => write!(f, "group count, blocks per group and block count disagree"),
            GroupDescChecksum { group } => write!(f, "group {group}: descriptor checksum"),
            GroupMetaOutOfRange { group, block } =>
                write!(f, "group {group}: metadata at block {block} is outside the filesystem"),
            GroupMetaOverlap { group, block } =>
                write!(f, "group {group}: block {block} is claimed by another group's metadata"),

            BadExtentHeader { ino, block } =>
                write!(f, "inode {ino}: extent header in block {block} is not usable"),
            BadExtentChecksum { ino, block } =>
                write!(f, "inode {ino}: extent block {block} checksum"),
            BlockOutOfRange { ino, block } =>
                write!(f, "inode {ino}: block {block} is outside the filesystem"),
            BlockInMetadata { ino, block } =>
                write!(f, "inode {ino}: block {block} belongs to the filesystem's own structures"),
            BlockClaimedTwice { ino, block } =>
                write!(f, "inode {ino}: block {block} is already claimed by another inode"),
            BlockCountWrong { ino, on_disk, counted } =>
                write!(f, "inode {ino}: i_blocks is {on_disk}, {counted} are reachable"),
            InodeBadChecksum { ino } =>
                write!(f, "inode {ino}: checksum does not match its contents"),
            InodeZeroLinks { ino } =>
                write!(f, "inode {ino}: no links and no deletion time"),
            InodeBadMode { ino, mode } =>
                write!(f, "inode {ino}: mode {mode:#o} is not a file type"),
            RootNotDirectory => write!(f, "the root inode is not a directory"),
            XattrRefcountWrong { block, on_disk, counted } =>
                write!(f, "xattr block {block}: refcount is {on_disk}, {counted} inodes use it"),

            DirBadRecLen { ino, block, at } =>
                write!(f, "directory {ino}: block {block} offset {at} has an impossible rec_len"),
            DirBadChain { ino, block } =>
                write!(f, "directory {ino}: records in block {block} do not fill it exactly"),
            DirBadChecksum { ino, block } =>
                write!(f, "directory {ino}: block {block} checksum"),
            DirBadIndex { ino, block } =>
                write!(f, "directory {ino}: index node in block {block} is not usable"),
            DirEntryOutOfRange { ino, child } =>
                write!(f, "directory {ino}: entry points at inode {child}, which does not exist"),
            DirEntryFreeInode { ino, child } =>
                write!(f, "directory {ino}: entry points at unused inode {child}"),
            DirEntryBadType { ino, child } =>
                write!(f, "directory {ino}: entry for inode {child} has the wrong file type"),
            DirMissingDot { ino } => write!(f, "directory {ino}: no '.' entry"),
            DirMissingDotDot { ino } => write!(f, "directory {ino}: no '..' entry"),
            DirDuplicateName { ino, child } =>
                write!(f, "directory {ino}: a name appears twice (inode {child})"),
            DirBadName { ino, child } =>
                write!(f, "directory {ino}: entry for inode {child} has an illegal name"),

            DirDisconnected { ino } =>
                write!(f, "directory {ino} cannot be reached from the root"),
            DirDotDotWrong { ino, was, should } =>
                write!(f, "directory {ino}: '..' says {was}, its parent is {should}"),
            DirTwoParents { ino } => write!(f, "directory {ino} is named from two places"),

            LinkCountWrong { ino, on_disk, counted } =>
                write!(f, "inode {ino}: i_links_count is {on_disk}, {counted} names point at it"),
            InodeUnattached { ino } => write!(f, "inode {ino} has links but no name"),

            BlockBitmapWrong { group, differences } =>
                write!(f, "group {group}: block bitmap differs in {differences} places"),
            InodeBitmapWrong { group, differences } =>
                write!(f, "group {group}: inode bitmap differs in {differences} places"),
            BitmapChecksumWrong { group, inodes } =>
                write!(f, "group {group}: {} bitmap checksum",
                       if inodes { "inode" } else { "block" }),
            GroupFreeBlocksWrong { group, on_disk, counted } =>
                write!(f, "group {group}: free blocks is {on_disk}, {counted} are free"),
            GroupFreeInodesWrong { group, on_disk, counted } =>
                write!(f, "group {group}: free inodes is {on_disk}, {counted} are free"),
            GroupUsedDirsWrong { group, on_disk, counted } =>
                write!(f, "group {group}: used dirs is {on_disk}, {counted} directories are here"),
            GroupItableUnusedWrong { group, on_disk, counted } =>
                write!(f, "group {group}: itable_unused is {on_disk}, should be {counted}"),
            SuperFreeBlocksWrong { on_disk, counted } =>
                write!(f, "superblock: free blocks is {on_disk}, {counted} are free"),
            SuperFreeInodesWrong { on_disk, counted } =>
                write!(f, "superblock: free inodes is {on_disk}, {counted} are free"),
            FilesystemMarkedBad => write!(f, "the volume is flagged as having had errors"),
        }
    }
}

/// A problem and what happened to it
#[derive(Clone, Copy, Debug)]
pub struct Finding {
    pub problem: Problem,
    pub fixed: bool,
}

/// What the check found, and what it did about it
pub struct FsckReport {
    /// False when the check could not run: an unreadable superblock, or too
    /// little memory for the maps
    pub checked: bool,
    /// Whether writes were allowed
    pub repairing: bool,

    pub block_size: u32,
    pub inode_size: u32,
    pub total_blocks: u64,
    pub total_inodes: u32,
    /// What the superblock says
    pub free_blocks: u64,
    pub free_inodes: u32,
    /// What the check counted
    pub used_blocks: u64,
    pub used_inodes: u32,
    pub used_dirs: u32,

    pub findings: Bounded<Finding>,
    pub fixed: u32,
    pub left: u32,
}

impl FsckReport {
    fn new(examples: usize, repairing: bool) -> Self {
        Self {
            checked: false,
            repairing,
            block_size: 0,
            inode_size: 0,
            total_blocks: 0,
            total_inodes: 0,
            free_blocks: 0,
            free_inodes: 0,
            used_blocks: 0,
            used_inodes: 0,
            used_dirs: 0,
            findings: Bounded::new(examples),
            fixed: 0,
            left: 0,
        }
    }

    /// Problem count, not the number listed: a million faults of one kind
    /// report a million
    pub fn errors(&self) -> u32 {
        self.findings.total()
    }

    pub fn is_clean(&self) -> bool {
        self.checked && self.findings.total() == 0
    }

    /// The examples kept, in found order
    pub fn examples(&self) -> &[Finding] {
        self.findings.items()
    }

    /// Whether any finding matches, for a test or a caller wanting one fault
    pub fn saw(&self, want: impl Fn(&Problem) -> bool) -> bool {
        self.findings.items().iter().any(|f| want(&f.problem))
    }
}

/// What the passes accumulate. Separate from `MikuFS`, so a pass can hold both
/// mutably
pub(crate) struct Fsck {
    pub opts: FsckOptions,
    pub report: FsckReport,

    /// Blocks claimed by some inode, indexed from `first_data_block`
    pub blocks: BitMap,
    /// Blocks the filesystem's own structures occupy
    pub meta: BitMap,
    /// Blocks claimed more than once
    pub dup: BitMap,
    /// Inodes with contents, indexed by `ino - 1`, so a group's inodes start on
    /// a byte boundary and its bitmap compares whole
    pub inodes: BitMap,
    /// ...and which of those are directories
    pub dirs: BitMap,
    /// Directories that are reachable from the root, filled in pass 3
    pub reachable: BitMap,

    pub links: LinkCounts,
    pub parents: DirParents,
    /// Whether anything was linked into `/lost+found`, which may have grown a
    /// block after pass 1, for pass 5
    pub reconnected: bool,

    pub first_block: u32,
    pub blocks_count: u32,
    pub inodes_count: u32,
    pub first_ino: u32,
}

impl Fsck {
    /// Record a problem. `fixed` is filled in by whoever repairs it
    pub fn found(&mut self, problem: Problem) {
        self.report.findings.push(Finding { problem, fixed: false });
        self.report.left += 1;
    }

    /// Record a problem that has just been repaired
    pub fn fixed(&mut self, problem: Problem) {
        self.report.findings.push(Finding { problem, fixed: true });
        self.report.fixed += 1;
    }

    /// Record a problem, saying whether the repair happened
    pub fn note(&mut self, problem: Problem, repaired: bool) {
        if repaired {
            self.fixed(problem);
        } else {
            self.found(problem);
        }
    }

    /// Inode number to bit index in the three inode maps
    pub fn ino_bit(&self, ino: u32) -> Option<u64> {
        if ino == 0 || ino > self.inodes_count {
            return None;
        }
        Some(ino as u64 - 1)
    }

    pub fn mark_inode(&mut self, ino: u32) {
        if let Some(b) = self.ino_bit(ino) {
            self.inodes.set(b);
        }
    }

    pub fn inode_in_use(&self, ino: u32) -> bool {
        self.ino_bit(ino).map(|b| self.inodes.get(b)).unwrap_or(false)
    }

    pub fn mark_dir(&mut self, ino: u32) {
        if let Some(b) = self.ino_bit(ino) {
            self.dirs.set(b);
        }
    }

    pub fn is_dir(&self, ino: u32) -> bool {
        self.ino_bit(ino).map(|b| self.dirs.get(b)).unwrap_or(false)
    }

    pub fn mark_reachable(&mut self, ino: u32) -> bool {
        match self.ino_bit(ino) {
            Some(b) => self.reachable.set(b),
            None => true,
        }
    }

    pub fn is_reachable(&self, ino: u32) -> bool {
        self.ino_bit(ino).map(|b| self.reachable.get(b)).unwrap_or(false)
    }

    /// Block number to bit index. Numbers below `first_data_block`, block 0 on
    /// a 1 KiB filesystem, are unaddressable by an inode
    pub fn block_bit(&self, block: u32) -> Option<u64> {
        if block < self.first_block || block >= self.blocks_count {
            return None;
        }
        Some((block - self.first_block) as u64)
    }
}

impl<H: ExtHost> MikuFS<H> {
    /// Checks the filesystem, repairing if asked. Returns an error only when
    /// the check could not run; everything wrong with the filesystem is in the
    /// report
    pub fn fsck(&mut self, opts: FsckOptions) -> Result<FsckReport, FsError> {
        // Staged data must reach the medium before it is judged, or the check
        // reports faults between cache and disk
        if !self.reader.read_only {
            self.sync()?;
        }

        let repair = opts.repair && !self.reader.read_only;
        let mut st = self.fsck_begin(opts, repair)?;

        // Each fix enters the log like other metadata. A transaction outgrowing
        // one descriptor block commits and opens the next, so this is one call,
        // and a power loss mid-repair replays
        let logging = repair && self.journal_active;
        if logging {
            self.ext3_begin_txn()?;
        }

        self.fsck_pass0(&mut st)?;
        if st.report.findings.items().iter().any(|f| f.problem == Problem::BadMagic) {
            if logging {
                self.ext3_abort_txn();
            }
            st.report.checked = false;
            return Ok(st.report);
        }
        self.fsck_pass1(&mut st)?;
        self.fsck_pass2(&mut st)?;
        self.fsck_pass3(&mut st)?;
        self.fsck_pass4(&mut st)?;
        self.fsck_pass5(&mut st)?;

        if logging {
            self.ext3_commit_txn()?;
        }
        st.report.checked = true;

        if repair && st.report.fixed > 0 {
            // A repaired volume is a checked one, and in-memory inodes and
            // group descriptors no longer match the medium
            self.fsck_finish()?;
        }

        Ok(st.report)
    }

    fn fsck_begin(&mut self, opts: FsckOptions, repair: bool) -> Result<Fsck, FsError> {
        let first = self.superblock.first_data_block();
        let blocks = self.superblock.blocks_count();
        let inodes = self.superblock.inodes_count();
        let span = blocks.saturating_sub(first) as u64;

        let mut report = FsckReport::new(opts.examples, repair);
        report.block_size = self.block_size;
        report.inode_size = self.inode_size();
        report.total_blocks = self.superblock.blocks_count_full();
        report.total_inodes = inodes;
        report.free_blocks = self.superblock.free_blocks_count_full();
        report.free_inodes = self.superblock.free_inodes_count();

        let first_ino = if self.superblock.rev_level() >= 1 {
            self.superblock.first_ino()
        } else {
            EXT2_FIRST_INO_OLD
        };

        Ok(Fsck {
            opts,
            report,
            blocks: BitMap::new(span)?,
            meta: BitMap::new(span)?,
            dup: BitMap::new(span)?,
            inodes: BitMap::new(inodes as u64)?,
            dirs: BitMap::new(inodes as u64)?,
            reachable: BitMap::new(inodes as u64)?,
            links: LinkCounts::new(inodes)?,
            parents: DirParents::new(),
            reconnected: false,
            first_block: first,
            blocks_count: blocks,
            inodes_count: inodes,
            first_ino,
        })
    }

    /// After a repair: cached data is stale, the volume is checked and no
    /// longer flagged as broken
    fn fsck_finish(&mut self) -> Result<(), FsError> {
        let now = self.get_timestamp();
        self.superblock.write_u16(0x3A, EXT2_VALID_FS);
        self.superblock.write_u32(0x40, now);
        self.superblock_dirty = true;
        self.sync()?;
        if let Some(ref mut c) = self.block_cache {
            c.clear();
        }
        Ok(())
    }
}

/// Names not allowed in a directory entry
pub(crate) fn name_is_legal(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.contains(&b'/')
        && !name.contains(&0)
}

/// The seven file types as the `FT_*` a directory entry should carry. `None`
/// when the mode is no file type
pub(crate) fn ft_for_mode(mode: u16) -> Option<u8> {
    match mode & 0xF000 {
        S_IFREG => Some(FT_REG_FILE),
        S_IFDIR => Some(FT_DIR),
        S_IFLNK => Some(FT_SYMLINK),
        S_IFCHR => Some(FT_CHRDEV),
        S_IFBLK => Some(FT_BLKDEV),
        S_IFIFO => Some(FT_FIFO),
        S_IFSOCK => Some(FT_SOCK),
        _ => None,
    }
}

/// `#<inode>`, a reconnected object's name in `/lost+found`
pub(crate) fn orphan_name(ino: u32) -> ([u8; 12], usize) {
    let mut out = [0u8; 12];
    out[0] = b'#';
    let mut digits = [0u8; 10];
    let mut n = ino;
    let mut d = 0;
    if n == 0 {
        digits[0] = b'0';
        d = 1;
    }
    while n > 0 {
        digits[d] = b'0' + (n % 10) as u8;
        n /= 10;
        d += 1;
    }
    for i in 0..d {
        out[1 + i] = digits[d - 1 - i];
    }
    (out, 1 + d)
}

/// Reserved inode numbers are in use regardless of contents: allocation starts
/// at `s_first_ino`, so an empty one is not free
pub(crate) fn is_reserved(ino: u32, first_ino: u32) -> bool {
    ino < first_ino
}

