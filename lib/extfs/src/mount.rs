// Mounting: validates what is on the medium
//
// Everything here runs on bytes someone else wrote, and the driver divides by
// these fields, indexes with them, and sizes allocations from them.
//
//   Geometry     fields the driver divides by or indexes with. A zero `s_inodes_per_group`
//                would divide by zero in the first `read_inode`.
//
//   Features     the incompat and ro_compat masks, as Linux treats them. bigalloc changes
//                what block numbers mean; meta_bg moves the descriptor table.

use crate::error::FsError;
use crate::host::ExtHost;
use crate::reader::DiskReader;
use crate::structs::*;
use crate::MikuFS;

/// Incompat features this driver can write. A bit outside this mask means the
/// layout would be misread, so the mount is refused, not made read-only.
///
/// Absent:
///   META_BG      the group descriptor table is not one contiguous run, as
///                `read_group_descriptors` assumes
///   JOURNAL_DEV  the journal is on another device, with no code to open one
///   ENCRYPT      contents are ciphertext, with no key handling
///   COMPRESSION  unimplemented everywhere, including Linux
///   MMP          multi-mount protection needs a heartbeat thread
///   LARGEDIR     a directory index may be three levels deep; `DX_MAX_LEVELS` is 2
pub const EXT_INCOMPAT_SUPP: u32 = FEATURE_INCOMPAT_FILETYPE
    | FEATURE_INCOMPAT_RECOVER
    | FEATURE_INCOMPAT_EXTENTS
    | FEATURE_INCOMPAT_64BIT
    | FEATURE_INCOMPAT_FLEX_BG
    | FEATURE_INCOMPAT_INLINE_DATA
    // Checksums chain from `s_checksum_seed` instead of the UUID, so
    // `tune2fs -U` can change the UUID. `csum_seed()` reads the field when set.
    // mke2fs sets it by default alongside metadata_csum
    | FEATURE_INCOMPAT_CSUM_SEED;

/// Read-only-compat features this driver may write. A bit outside this mask
/// still allows reading, so the mount is forced read-only.
///
/// Absent:
///   BIGALLOC     allocation is in clusters, changing every bitmap bit and free count
///   QUOTA        writes must maintain quota inodes unknown here
///   PROJECT      same, for project ids
///   VERITY       contents are under a Merkle tree a write would break
///   READONLY     the filesystem asks not to be written
pub const EXT_RO_COMPAT_SUPP: u32 = FEATURE_RO_COMPAT_SPARSE_SUPER
    | FEATURE_RO_COMPAT_LARGE_FILE
    | FEATURE_RO_COMPAT_HUGE_FILE
    | FEATURE_RO_COMPAT_GDT_CSUM
    | FEATURE_RO_COMPAT_DIR_NLINK
    | FEATURE_RO_COMPAT_EXTRA_ISIZE
    | FEATURE_RO_COMPAT_METADATA_CSUM;

/// Bound on the heap-allocated group table, sized from the superblock. 2^20
/// groups is 128 TiB at 4 KiB blocks
pub const MAX_BLOCK_GROUPS: u32 = 1 << 20;

/// Why a superblock was rejected, separated since the mount log may be the only
/// evidence
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountReject {
    BadMagic(u16),
    /// `1024 << s_log_block_size` is not an addressable block size
    BadBlockSize(u32),
    /// Zero, and about to be divided by
    ZeroInodesPerGroup,
    ZeroBlocksPerGroup,
    /// `s_inode_size` is not a power of two in [128, block_size]
    BadInodeSize(u32),
    /// More groups than `MAX_BLOCK_GROUPS`
    TooManyGroups(u32),
    /// The group descriptor table would not fit before the first data block
    GroupTableOverrun,
    /// Incompat bits not implemented here, so the layout would be misread. The
    /// value is the unsupported subset
    UnsupportedIncompat(u32),
    /// The superblock does not match its checksum
    BadSuperblockChecksum,
    /// `s_checksum_type` names an algorithm other than crc32c. Computing crc32c
    /// anyway would corrupt every metadata block written
    UnknownChecksumType,
}

/// What `check_superblock` concluded
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub block_size:       u32,
    pub inodes_per_group: u32,
    pub blocks_per_group: u32,
    pub group_count:      u32,
    pub gd_size:          usize,
    /// ro_compat bits not implemented. Empty means writable; non-empty forces
    /// read-only
    pub unsupported_ro:   u32,
}

impl Geometry {
    /// Whether this filesystem may be written to at all
    pub fn writable(&self) -> bool {
        self.unsupported_ro == 0
    }
}

/// Validates a superblock and derives the geometry. Pure, so a test can pass
/// hostile bytes directly
pub fn check_superblock(sb: &Superblock) -> Result<Geometry, MountReject> {
    let magic = sb.magic();
    if magic != EXT2_MAGIC {
        return Err(MountReject::BadMagic(magic));
    }

    // `1024 << n` is an unchecked shift. ext4 tops out at 64 KiB; above 6
    // either overflows or gives a size sector arithmetic cannot express
    let log_bs = sb.log_block_size();
    if log_bs > 6 {
        return Err(MountReject::BadBlockSize(log_bs));
    }
    let block_size = 1024u32 << log_bs;

    // Divided by in `read_inode`, `write_inode`, `alloc_inode`, `free_inode`,
    // and each caller deriving a group from an inode number
    let inodes_per_group = sb.inodes_per_group();
    if inodes_per_group == 0 {
        return Err(MountReject::ZeroInodesPerGroup);
    }
    let blocks_per_group = sb.blocks_per_group();
    if blocks_per_group == 0 {
        return Err(MountReject::ZeroBlocksPerGroup);
    }

    // The inode table is indexed by `inode_size`; a value not a power of two,
    // below 128, or above a block makes each inode offset wrong
    let inode_size = sb.inode_size_val();
    if inode_size < 128 || inode_size > block_size || !inode_size.is_power_of_two() {
        return Err(MountReject::BadInodeSize(inode_size));
    }

    // An inode bitmap is one block, so a group holds at most that block's bits
    // in inodes
    if inodes_per_group > block_size.saturating_mul(8) {
        return Err(MountReject::ZeroInodesPerGroup);
    }

    let blocks_count     = sb.blocks_count();
    let first_data_block = sb.first_data_block();
    let usable           = blocks_count.saturating_sub(first_data_block);
    let group_count      = usable.div_ceil(blocks_per_group);
    if group_count > MAX_BLOCK_GROUPS {
        return Err(MountReject::TooManyGroups(group_count));
    }

    let gd_size = sb.group_desc_size() as usize;
    if gd_size < 32 || gd_size > 64 || !gd_size.is_power_of_two() {
        return Err(MountReject::GroupTableOverrun);
    }

    let incompat   = sb.feature_incompat();
    let unsupported = incompat & !EXT_INCOMPAT_SUPP;
    if unsupported != 0 {
        return Err(MountReject::UnsupportedIncompat(unsupported));
    }

    let unsupported_ro = sb.feature_ro_compat() & !EXT_RO_COMPAT_SUPP;

    Ok(Geometry {
        block_size,
        inodes_per_group,
        blocks_per_group,
        group_count,
        gd_size,
        unsupported_ro,
    })
}

impl<H: ExtHost> MikuFS<H> {
    /// Reads the superblock and group descriptor table, validates both, and
    /// builds a handle, only reading the medium.
    ///
    /// `read_only` forces a read-only mount; a filesystem with unimplemented
    /// ro_compat features is read-only regardless
    pub fn open(host: H, start_lba: u32, read_only: bool) -> Result<Self, MountError> {
        let mut reader = DiskReader::new_partitioned(host, start_lba);

        let mut superblock = Superblock::zeroed();
        let mut sector = [0u8; 512];
        reader.read_sector(2, &mut sector).map_err(MountError::Io)?;
        superblock.data[0..512].copy_from_slice(&sector);
        reader.read_sector(3, &mut sector).map_err(MountError::Io)?;
        superblock.data[512..1024].copy_from_slice(&sector);

        let geo = check_superblock(&superblock).map_err(MountError::Rejected)?;

        // A volume with unimplemented ro_compat bits is readable and must not
        // be written, which would leave quota inodes, verity trees, or cluster
        // bitmaps inconsistent
        let force_ro = read_only || !geo.writable();
        reader.read_only = force_ro;

        let mut fs = MikuFS::with_reader(reader);
        fs.superblock       = superblock;
        fs.block_size       = geo.block_size;
        fs.inodes_per_group = geo.inodes_per_group;
        fs.blocks_per_group = geo.blocks_per_group;
        fs.group_count      = geo.group_count;

        let gc = geo.group_count as usize;
        fs.groups.resize(gc, GroupDesc { data: [0; 64] });
        fs.groups_dirty.resize(gc, false);
        fs.alloc_hint.resize(gc, 0);

        // The table starts in the block after the superblock: block 1 normally,
        // block 2 when 1 KiB blocks put the superblock in block 1
        let gdt_block = if geo.block_size == 1024 { 2 } else { 1 };
        {
            // Splits the borrow: `read_group_descriptors` needs the reader and
            // the group slice mutably at once
            let MikuFS { reader, groups, .. } = &mut fs;
            reader
                .read_group_descriptors(gdt_block, geo.block_size, gc, geo.gd_size, groups)
                .map_err(MountError::Io)?;
        }

        // The superblock and group descriptors carry checksums, checked here,
        // where they are read once
        if fs.superblock.has_metadata_csum() {
            if !fs.csum_algorithm_known() {
                crate::extlog!(
                    "[extfs] superblock declares checksum type {} - not one we can compute",
                    fs.superblock.checksum_type()
                );
                return Err(MountError::Rejected(MountReject::UnknownChecksumType));
            }
            if !fs.verify_superblock_csum() {
                crate::extlog!("[extfs] superblock fails its own checksum");
                return Err(MountError::Rejected(MountReject::BadSuperblockChecksum));
            }
        }
        if fs.has_gdt_csum() {
            let bad = (0..fs.groups.len()).filter(|&g| !fs.verify_group_desc_csum(g)).count();
            if bad > 0 {
                // Not fatal: the rest stays readable, so a damaged descriptor
                // is reported and the mount comes up read-only
                crate::extlog!(
                    "[extfs] {} of {} group descriptors fail their checksum - mounting read-only",
                    bad, fs.groups.len()
                );
                fs.reader.read_only = true;
            }
        }

        fs.init_cache();
        let _ = fs.init_journal();

        Ok(fs)
    }

    /// Everything a mount writes: journal replay, orphan cleanup, and the
    /// mount-state stamp. Separate from `open`, so a read-only mount does not
    /// touch the medium. Returns journal blocks replayed and orphans cleaned
    pub fn activate(&mut self) -> Result<(u32, u32), FsError> {
        if self.reader.read_only {
            return Ok((0, 0));
        }

        let mut replayed = 0;
        if self.journal_active {
            let dirty = self
                .read_journal_superblock()
                .map(|j| !j.is_clean())
                .unwrap_or(false);
            if dirty {
                match self.ext3_recover() {
                    Ok(n) => replayed = n,
                    // A log with unimplemented features. Data is intact, but
                    // the log's changes cannot be applied, so writing is
                    // unsafe; the mount stays read-only and rescuable
                    Err(FsError::UnsupportedFeature) => {
                        crate::extlog!(
                            "[ext3] the journal could not be replayed - mounting read-only"
                        );
                        self.reader.read_only = true;
                        return Ok((0, 0));
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        let orphans = self.cleanup_orphans()?;
        self.update_mount_state();
        self.flush_all_dirty_metadata()?;
        Ok((replayed, orphans))
    }
}

/// Why a mount failed
#[derive(Debug, Clone, Copy)]
pub enum MountError {
    /// The medium could not be read
    Io(FsError),
    /// It was read and may not be mounted
    Rejected(MountReject),
}
