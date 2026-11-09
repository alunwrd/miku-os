pub const EXT2_MAGIC: u16 = 0xEF53;

pub const MAX_NAME: usize = 255;

/// Most hard links on an inode. `i_links_count` is a u16, and ext2 stops at
/// 65000 for headroom. A wrap would free blocks with entries still pointing at
/// them
pub const EXT2_LINK_MAX: u16 = 65000;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Superblock {
    pub data: [u8; 1024],
}

impl Superblock {
    pub const fn zeroed() -> Self {
        Self { data: [0; 1024] }
    }

    pub fn read_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
    }

    pub fn read_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    pub fn write_u16(&mut self, offset: usize, val: u16) {
        let bytes = val.to_le_bytes();
        self.data[offset..offset + 2].copy_from_slice(&bytes);
    }

    pub fn write_u32(&mut self, offset: usize, val: u32) {
        let bytes = val.to_le_bytes();
        self.data[offset..offset + 4].copy_from_slice(&bytes);
    }

    pub fn inodes_count(&self) -> u32 {
        self.read_u32(0)
    }
    pub fn blocks_count(&self) -> u32 {
        self.read_u32(4)
    }
    pub fn r_blocks_count(&self) -> u32 {
        self.read_u32(8)
    }
    pub fn free_blocks_count(&self) -> u32 {
        self.read_u32(12)
    }
    pub fn free_inodes_count(&self) -> u32 {
        self.read_u32(16)
    }
    pub fn first_data_block(&self) -> u32 {
        self.read_u32(20)
    }
    pub fn log_block_size(&self) -> u32 {
        self.read_u32(24)
    }
    pub fn log_frag_size(&self) -> u32 {
        self.read_u32(28)
    }
    pub fn blocks_per_group(&self) -> u32 {
        self.read_u32(32)
    }
    pub fn frags_per_group(&self) -> u32 {
        self.read_u32(36)
    }
    pub fn inodes_per_group(&self) -> u32 {
        self.read_u32(40)
    }
    pub fn mtime(&self) -> u32 {
        self.read_u32(44)
    }
    pub fn wtime(&self) -> u32 {
        self.read_u32(48)
    }
    pub fn mnt_count(&self) -> u16 {
        self.read_u16(52)
    }
    pub fn max_mnt_count(&self) -> u16 {
        self.read_u16(54)
    }
    pub fn magic(&self) -> u16 {
        self.read_u16(56)
    }
    pub fn state(&self) -> u16 {
        self.read_u16(58)
    }
    pub fn errors(&self) -> u16 {
        self.read_u16(60)
    }
    pub fn minor_rev_level(&self) -> u16 {
        self.read_u16(62)
    }
    pub fn lastcheck(&self) -> u32 {
        self.read_u32(64)
    }
    pub fn checkinterval(&self) -> u32 {
        self.read_u32(68)
    }
    pub fn creator_os(&self) -> u32 {
        self.read_u32(72)
    }
    pub fn rev_level(&self) -> u32 {
        self.read_u32(76)
    }
    pub fn def_resuid(&self) -> u16 {
        self.read_u16(80)
    }
    pub fn def_resgid(&self) -> u16 {
        self.read_u16(82)
    }
    /// 0xCE. Blocks kept after the group descriptor table, so the filesystem
    /// can grow without moving what follows
    pub fn reserved_gdt_blocks(&self) -> u16 {
        self.read_u16(206)
    }
    pub fn first_ino(&self) -> u32 {
        self.read_u32(84)
    }
    pub fn inode_size_raw(&self) -> u16 {
        self.read_u16(88)
    }
    pub fn block_group_nr(&self) -> u16 {
        self.read_u16(90)
    }
    pub fn feature_compat(&self) -> u32 {
        self.read_u32(92)
    }
    pub fn feature_incompat(&self) -> u32 {
        self.read_u32(96)
    }
    pub fn feature_ro_compat(&self) -> u32 {
        self.read_u32(100)
    }
    pub fn uuid(&self) -> &[u8] {
        &self.data[104..120]
    }
    pub fn journal_uuid(&self) -> &[u8] {
        &self.data[208..224]
    }
    pub fn journal_inum(&self) -> u32 {
        self.read_u32(224)
    }
    pub fn journal_dev(&self) -> u32 {
        self.read_u32(228)
    }
    pub fn last_orphan(&self) -> u32 {
        self.read_u32(232)
    }
    pub fn hash_seed(&self, idx: usize) -> u32 {
        self.read_u32(236 + idx * 4)
    }
    pub fn def_hash_version(&self) -> u8 {
        self.data[252]
    }
    pub fn default_mount_opts(&self) -> u32 {
        self.read_u32(256)
    }
    pub fn first_meta_bg(&self) -> u32 {
        self.read_u32(260)
    }
    pub fn mkfs_time(&self) -> u32 {
        self.read_u32(264)
    }
    pub fn desc_size(&self) -> u16 {
        self.read_u16(254)
    }
    // These follow the three 64-bit count halves at 0x150-0x158, which follow
    // s_jnl_blocks[17] at 0x10C
    /// 0x15C
    pub fn min_extra_isize(&self) -> u16 {
        self.read_u16(348)
    }
    /// 0x15E
    pub fn want_extra_isize(&self) -> u16 {
        self.read_u16(350)
    }
    /// 0x160
    pub fn flags_sb(&self) -> u32 {
        self.read_u32(352)
    }
    /// 0x164
    pub fn raid_stride(&self) -> u16 {
        self.read_u16(356)
    }
    /// 0x166
    pub fn mmp_interval(&self) -> u16 {
        self.read_u16(358)
    }
    /// 0x168
    pub fn mmp_block(&self) -> u64 {
        let lo = self.read_u32(360) as u64;
        let hi = self.read_u32(364) as u64;
        lo | (hi << 32)
    }
    /// 0x170
    pub fn raid_stripe_width(&self) -> u32 {
        self.read_u32(368)
    }
    /// 0x174
    pub fn log_groups_per_flex(&self) -> u8 {
        self.data[372]
    }
    /// 0x175. Only crc32c (1) is defined; anything else is uncomputable here
    pub fn checksum_type(&self) -> u8 {
        self.data[373]
    }
    /// 0x270. Seed for metadata checksums when INCOMPAT_CSUM_SEED is set, so
    /// `tune2fs -U` can change the UUID
    pub fn checksum_seed(&self) -> u32 {
        self.read_u32(624)
    }
    /// 0x3FC, the superblock's last four bytes, covering everything before
    pub fn checksum(&self) -> u32 {
        self.read_u32(1020)
    }
    pub fn set_checksum(&mut self, v: u32) {
        self.write_u32(1020, v);
    }

    /// Inodes the filesystem uses itself and no directory names. `orphan_file`
    /// and the quota files are ordinary numbers past `s_first_ino`, recorded
    /// only here. A tree walk must know them, or they read as lost files
    pub fn usr_quota_inum(&self) -> u32 {
        self.read_u32(576)
    }
    pub fn grp_quota_inum(&self) -> u32 {
        self.read_u32(580)
    }
    pub fn prj_quota_inum(&self) -> u32 {
        self.read_u32(620)
    }
    /// 0x280
    pub fn orphan_file_inum(&self) -> u32 {
        self.read_u32(640)
    }

    pub fn has_orphan_file(&self) -> bool {
        self.feature_compat() & FEATURE_COMPAT_ORPHAN_FILE != 0
    }
    pub fn has_quota(&self) -> bool {
        self.feature_ro_compat() & FEATURE_RO_COMPAT_QUOTA != 0
    }
    pub fn has_project(&self) -> bool {
        self.feature_ro_compat() & FEATURE_RO_COMPAT_PROJECT != 0
    }

    /// Whether this inode is one of them
    pub fn is_hidden_inode(&self, ino: u32) -> bool {
        if ino == 0 {
            return false;
        }
        if self.has_journal() && ino == self.journal_inum() {
            return true;
        }
        if self.has_orphan_file() && ino == self.orphan_file_inum() {
            return true;
        }
        if self.has_quota() && (ino == self.usr_quota_inum() || ino == self.grp_quota_inum()) {
            return true;
        }
        if self.has_project() && ino == self.prj_quota_inum() {
            return true;
        }
        false
    }
    pub fn blocks_count_hi(&self) -> u32 {
        self.read_u32(336)
    }
    pub fn r_blocks_count_hi(&self) -> u32 {
        self.read_u32(340)
    }
    pub fn free_blocks_count_hi(&self) -> u32 {
        self.read_u32(344)
    }

    pub fn block_size(&self) -> u32 {
        1024u32 << self.log_block_size()
    }

    pub fn inode_size_val(&self) -> u32 {
        if self.rev_level() >= 1 {
            self.inode_size_raw() as u32
        } else {
            128
        }
    }

    pub fn group_desc_size(&self) -> u32 {
        let raw = if self.has_64bit() && self.desc_size() > 0 {
            self.desc_size() as u32
        } else {
            32
        };
        if raw > 64 { 64 } else { raw }
    }

    pub fn blocks_count_full(&self) -> u64 {
        let lo = self.blocks_count() as u64;
        if self.has_64bit() {
            let hi = self.blocks_count_hi() as u64;
            lo | (hi << 32)
        } else {
            lo
        }
    }

    pub fn free_blocks_count_full(&self) -> u64 {
        let lo = self.free_blocks_count() as u64;
        if self.has_64bit() {
            let hi = self.free_blocks_count_hi() as u64;
            lo | (hi << 32)
        } else {
            lo
        }
    }

    pub fn groups_per_flex(&self) -> u32 {
        let v = self.log_groups_per_flex();
        if v > 0 && v < 32 {
            1u32 << v
        } else {
            0
        }
    }

    pub fn volume_name(&self) -> &str {
        let start = 120;
        let end = start + 16;
        let len = self.data[start..end]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(16);
        core::str::from_utf8(&self.data[start..start + len]).unwrap_or("")
    }

    pub fn has_journal(&self) -> bool {
        self.feature_compat() & FEATURE_COMPAT_HAS_JOURNAL != 0
    }

    pub fn has_extents(&self) -> bool {
        self.feature_incompat() & FEATURE_INCOMPAT_EXTENTS != 0
    }

    pub fn has_64bit(&self) -> bool {
        self.feature_incompat() & FEATURE_INCOMPAT_64BIT != 0
    }

    pub fn has_flex_bg(&self) -> bool {
        self.feature_incompat() & FEATURE_INCOMPAT_FLEX_BG != 0
    }

    pub fn has_filetype(&self) -> bool {
        self.feature_incompat() & FEATURE_INCOMPAT_FILETYPE != 0
    }

    pub fn has_sparse_super(&self) -> bool {
        self.feature_ro_compat() & FEATURE_RO_COMPAT_SPARSE_SUPER != 0
    }

    pub fn has_large_file(&self) -> bool {
        self.feature_ro_compat() & FEATURE_RO_COMPAT_LARGE_FILE != 0
    }

    pub fn has_huge_file(&self) -> bool {
        self.feature_ro_compat() & FEATURE_RO_COMPAT_HUGE_FILE != 0
    }

    pub fn has_dir_index(&self) -> bool {
        self.feature_compat() & FEATURE_COMPAT_DIR_INDEX != 0
    }

    pub fn has_ext_attr(&self) -> bool {
        self.feature_compat() & FEATURE_COMPAT_EXT_ATTR != 0
    }

    pub fn has_metadata_csum(&self) -> bool {
        self.feature_ro_compat() & FEATURE_RO_COMPAT_METADATA_CSUM != 0
    }

    /// Whether metadata checksums chain from `s_checksum_seed` instead of the
    /// UUID
    pub fn has_csum_seed(&self) -> bool {
        self.feature_incompat() & FEATURE_INCOMPAT_CSUM_SEED != 0
    }

    /// Whether this volume is ext4, by e2fsprogs's rule: it has a feature ext3
    /// lacked. A volume with `metadata_csum` and no extents is ext4, which
    /// `df`, `statfs()`, and `/proc/mounts` report
    pub fn is_ext4(&self) -> bool {
        self.feature_incompat() & !EXT3_INCOMPAT_SET != 0
            || self.feature_ro_compat() & !EXT3_RO_COMPAT_SET != 0
    }

    pub fn is_ext3(&self) -> bool {
        self.has_journal() && !self.is_ext4()
    }

    pub fn fs_version_str(&self) -> &'static str {
        if self.is_ext4() {
            "ext4"
        } else if self.is_ext3() {
            "ext3"
        } else {
            "ext2"
        }
    }

    pub fn has_gdt_csum(&self) -> bool {
        self.feature_ro_compat() & FEATURE_RO_COMPAT_GDT_CSUM != 0
    }
}

/// The incompat bits ext3 could have. Anything else means ext4
pub const EXT3_INCOMPAT_SET: u32 = FEATURE_INCOMPAT_FILETYPE
    | FEATURE_INCOMPAT_RECOVER
    | FEATURE_INCOMPAT_JOURNAL_DEV
    | FEATURE_INCOMPAT_META_BG;

/// The ro_compat bits ext3 could have. `btree_dir` (0x0004) predates ext4,
/// though nothing implemented it
pub const EXT3_RO_COMPAT_SET: u32 =
    FEATURE_RO_COMPAT_SPARSE_SUPER | FEATURE_RO_COMPAT_LARGE_FILE | 0x0004;

pub const FEATURE_COMPAT_DIR_PREALLOC: u32 = 0x0001;
pub const FEATURE_COMPAT_IMAGIC_INODES: u32 = 0x0002;
pub const FEATURE_COMPAT_HAS_JOURNAL: u32 = 0x0004;
pub const FEATURE_COMPAT_EXT_ATTR: u32 = 0x0008;
pub const FEATURE_COMPAT_RESIZE_INO: u32 = 0x0010;
/// The orphan list is a file instead of a chain through `s_last_orphan`. Its
/// inode is named only by `s_orphan_file_inum`
pub const FEATURE_COMPAT_ORPHAN_FILE: u32 = 0x1000;
pub const FEATURE_COMPAT_DIR_INDEX: u32 = 0x0020;
pub const FEATURE_COMPAT_SPARSE_SUPER2: u32 = 0x0200;

/// `bg_flags`: the group's inode bitmap and the tail of its table were never
/// written and hold old disk contents
pub const EXT4_BG_INODE_UNINIT: u16 = 0x0001;
/// `bg_flags`: the group's block bitmap was never written
pub const EXT4_BG_BLOCK_UNINIT: u16 = 0x0002;
/// `bg_flags`: the inode table has been zeroed
pub const EXT4_BG_INODE_ZEROED: u16 = 0x0004;

pub const FEATURE_INCOMPAT_COMPRESSION: u32 = 0x0001;
pub const FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;
pub const FEATURE_INCOMPAT_RECOVER: u32 = 0x0004;
pub const FEATURE_INCOMPAT_JOURNAL_DEV: u32 = 0x0008;
pub const FEATURE_INCOMPAT_META_BG: u32 = 0x0010;
pub const FEATURE_INCOMPAT_EXTENTS: u32 = 0x0040;
pub const FEATURE_INCOMPAT_64BIT: u32 = 0x0080;
pub const FEATURE_INCOMPAT_MMP: u32 = 0x0100;
pub const FEATURE_INCOMPAT_FLEX_BG: u32 = 0x0200;
pub const FEATURE_INCOMPAT_EA_INODE: u32 = 0x0400;
pub const FEATURE_INCOMPAT_DIRDATA: u32 = 0x1000;
pub const FEATURE_INCOMPAT_CSUM_SEED: u32 = 0x2000;
pub const FEATURE_INCOMPAT_LARGEDIR: u32 = 0x4000;
pub const FEATURE_INCOMPAT_INLINE_DATA: u32 = 0x8000;
pub const FEATURE_INCOMPAT_ENCRYPT: u32 = 0x10000;

pub const FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
pub const FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0002;
pub const FEATURE_RO_COMPAT_BTREE_DIR: u32 = 0x0004;
pub const FEATURE_RO_COMPAT_HUGE_FILE: u32 = 0x0008;
pub const FEATURE_RO_COMPAT_GDT_CSUM: u32 = 0x0010;
pub const FEATURE_RO_COMPAT_DIR_NLINK: u32 = 0x0020;
pub const FEATURE_RO_COMPAT_EXTRA_ISIZE: u32 = 0x0040;
pub const FEATURE_RO_COMPAT_QUOTA: u32 = 0x0100;
pub const FEATURE_RO_COMPAT_BIGALLOC: u32 = 0x0200;
pub const FEATURE_RO_COMPAT_METADATA_CSUM: u32 = 0x0400;
pub const FEATURE_RO_COMPAT_READONLY: u32 = 0x1000;
pub const FEATURE_RO_COMPAT_PROJECT: u32 = 0x2000;
pub const FEATURE_RO_COMPAT_VERITY: u32 = 0x8000;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GroupDesc {
    pub data: [u8; 64],
}

impl GroupDesc {
    pub const fn zeroed() -> Self {
        Self { data: [0; 64] }
    }

    pub fn read_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
    }

    pub fn read_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    pub fn write_u16(&mut self, offset: usize, val: u16) {
        let bytes = val.to_le_bytes();
        self.data[offset..offset + 2].copy_from_slice(&bytes);
    }

    pub fn write_u32(&mut self, offset: usize, val: u32) {
        let bytes = val.to_le_bytes();
        self.data[offset..offset + 4].copy_from_slice(&bytes);
    }

    pub fn block_bitmap_lo(&self) -> u32 {
        self.read_u32(0)
    }
    pub fn inode_bitmap_lo(&self) -> u32 {
        self.read_u32(4)
    }
    pub fn inode_table_lo(&self) -> u32 {
        self.read_u32(8)
    }
    pub fn free_blocks_lo(&self) -> u16 {
        self.read_u16(12)
    }
    pub fn free_inodes_lo(&self) -> u16 {
        self.read_u16(14)
    }
    pub fn used_dirs_lo(&self) -> u16 {
        self.read_u16(16)
    }
    pub fn flags_gd(&self) -> u16 {
        self.read_u16(18)
    }
    pub fn exclude_bitmap_lo(&self) -> u32 {
        self.read_u32(20)
    }
    pub fn block_bitmap_csum_lo(&self) -> u16 {
        self.read_u16(24)
    }
    pub fn inode_bitmap_csum_lo(&self) -> u16 {
        self.read_u16(26)
    }
    pub fn itable_unused_lo(&self) -> u16 {
        self.read_u16(28)
    }
    pub fn checksum(&self) -> u16 {
        self.read_u16(30)
    }

    pub fn block_bitmap_hi(&self) -> u32 {
        self.read_u32(32)
    }
    pub fn inode_bitmap_hi(&self) -> u32 {
        self.read_u32(36)
    }
    pub fn inode_table_hi(&self) -> u32 {
        self.read_u32(40)
    }
    pub fn free_blocks_hi(&self) -> u16 {
        self.read_u16(44)
    }
    pub fn free_inodes_hi(&self) -> u16 {
        self.read_u16(46)
    }
    pub fn used_dirs_hi(&self) -> u16 {
        self.read_u16(48)
    }
    pub fn itable_unused_hi(&self) -> u16 {
        self.read_u16(50)
    }
    pub fn exclude_bitmap_hi(&self) -> u32 {
        self.read_u32(52)
    }
    pub fn block_bitmap_csum_hi(&self) -> u16 {
        self.read_u16(56)
    }
    pub fn inode_bitmap_csum_hi(&self) -> u16 {
        self.read_u16(58)
    }

    pub fn block_bitmap(&self) -> u32 {
        self.block_bitmap_lo()
    }
    pub fn inode_bitmap(&self) -> u32 {
        self.inode_bitmap_lo()
    }
    pub fn inode_table(&self) -> u32 {
        self.inode_table_lo()
    }
    pub fn free_blocks(&self) -> u16 {
        self.free_blocks_lo()
    }
    pub fn free_inodes(&self) -> u16 {
        self.free_inodes_lo()
    }
    pub fn used_dirs(&self) -> u16 {
        self.used_dirs_lo()
    }

    pub fn block_bitmap_full(&self) -> u64 {
        let lo = self.block_bitmap_lo() as u64;
        let hi = self.block_bitmap_hi() as u64;
        lo | (hi << 32)
    }

    pub fn inode_bitmap_full(&self) -> u64 {
        let lo = self.inode_bitmap_lo() as u64;
        let hi = self.inode_bitmap_hi() as u64;
        lo | (hi << 32)
    }

    pub fn inode_table_full(&self) -> u64 {
        let lo = self.inode_table_lo() as u64;
        let hi = self.inode_table_hi() as u64;
        lo | (hi << 32)
    }

    pub fn free_blocks_full(&self) -> u32 {
        let lo = self.free_blocks_lo() as u32;
        let hi = self.free_blocks_hi() as u32;
        lo | (hi << 16)
    }

    pub fn free_inodes_full(&self) -> u32 {
        let lo = self.free_inodes_lo() as u32;
        let hi = self.free_inodes_hi() as u32;
        lo | (hi << 16)
    }

    /// Inodes at the end of this group's table never used. mke2fs writes this
    /// so a checker or mount can skip the uninitialized tail. An allocator
    /// using one must lower the number
    pub fn itable_unused(&self) -> u32 {
        let lo = self.itable_unused_lo() as u32;
        let hi = self.itable_unused_hi() as u32;
        lo | (hi << 16)
    }

    pub fn set_itable_unused(&mut self, v: u32) {
        self.write_u16(28, v as u16);
        self.write_u16(50, (v >> 16) as u16);
    }

    pub fn set_flags_gd(&mut self, flags: u16) {
        self.write_u16(18, flags);
    }

    pub fn inc_used_dirs(&mut self) {
        let lo = self.used_dirs_lo();
        let hi = self.used_dirs_hi();
        let val = (lo as u32) | ((hi as u32) << 16);
        let new_val = val + 1;
        self.write_u16(16, new_val as u16);
        self.write_u16(48, (new_val >> 16) as u16);
    }

    pub fn dec_used_dirs(&mut self) {
        let lo = self.used_dirs_lo();
        let hi = self.used_dirs_hi();
        let val = (lo as u32) | ((hi as u32) << 16);
        if val > 0 {
            let new_val = val - 1;
            self.write_u16(16, new_val as u16);
            self.write_u16(48, (new_val >> 16) as u16);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Inode {
    pub data: [u8; 256],
    pub on_disk_size: u16,
}

impl Inode {
    pub const fn zeroed() -> Self {
        Self {
            data: [0; 256],
            on_disk_size: 128,
        }
    }

    // Reads past the inode return zero. Every offset should be in range; this
    // is a second defense, since a crafted image must yield a value the caller
    // handles and not a panic
    fn read_u16(&self, offset: usize) -> u16 {
        if offset + 2 > self.data.len() {
            return 0;
        }
        u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
    }

    fn read_u32(&self, offset: usize) -> u32 {
        if offset + 4 > self.data.len() {
            return 0;
        }
        u32::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    pub fn write_u16(&mut self, offset: usize, val: u16) {
        let bytes = val.to_le_bytes();
        self.data[offset..offset + 2].copy_from_slice(&bytes);
    }

    pub fn write_u32(&mut self, offset: usize, val: u32) {
        let bytes = val.to_le_bytes();
        self.data[offset..offset + 4].copy_from_slice(&bytes);
    }

    pub fn mode(&self) -> u16 {
        self.read_u16(0)
    }
    pub fn uid(&self) -> u16 {
        self.read_u16(2)
    }
    pub fn size_lo(&self) -> u32 {
        self.read_u32(4)
    }
    pub fn atime(&self) -> u32 {
        self.read_u32(8)
    }
    pub fn ctime(&self) -> u32 {
        self.read_u32(12)
    }
    pub fn mtime(&self) -> u32 {
        self.read_u32(16)
    }
    pub fn dtime(&self) -> u32 {
        self.read_u32(20)
    }
    pub fn gid(&self) -> u16 {
        self.read_u16(24)
    }
    pub fn links_count(&self) -> u16 {
        self.read_u16(26)
    }
    pub fn blocks(&self) -> u32 {
        self.read_u32(28)
    }
    pub fn flags(&self) -> u32 {
        self.read_u32(32)
    }
    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags() & flag != 0
    }
    pub fn osd1(&self) -> u32 {
        self.read_u32(36)
    }

    pub fn block(&self, index: usize) -> u32 {
        if index >= 15 {
            return 0;
        }
        self.read_u32(40 + index * 4)
    }

    pub fn generation(&self) -> u32 {
        self.read_u32(100)
    }
    pub fn file_acl_lo(&self) -> u32 {
        self.read_u32(104)
    }
    pub fn size_hi(&self) -> u32 {
        self.read_u32(108)
    }
    pub fn obso_faddr(&self) -> u32 {
        self.read_u32(112)
    }

    pub fn blocks_hi(&self) -> u16 {
        self.read_u16(116)
    }
    pub fn file_acl_hi(&self) -> u16 {
        self.read_u16(118)
    }
    pub fn uid_hi(&self) -> u16 {
        self.read_u16(120)
    }
    pub fn gid_hi(&self) -> u16 {
        self.read_u16(122)
    }
    pub fn checksum_lo(&self) -> u16 {
        self.read_u16(124)
    }

    pub fn extra_isize(&self) -> u16 {
        if self.on_disk_size > 128 {
            self.read_u16(128)
        } else {
            0
        }
    }

    pub fn checksum_hi(&self) -> u16 {
        if self.on_disk_size >= 132 {
            self.read_u16(130)
        } else {
            0
        }
    }

    pub fn ctime_extra(&self) -> u32 {
        if self.on_disk_size >= 136 {
            self.read_u32(132)
        } else {
            0
        }
    }

    pub fn mtime_extra(&self) -> u32 {
        if self.on_disk_size >= 140 {
            self.read_u32(136)
        } else {
            0
        }
    }

    pub fn atime_extra(&self) -> u32 {
        if self.on_disk_size >= 144 {
            self.read_u32(140)
        } else {
            0
        }
    }

    // Timestamps at full width
    //
    // `i_[cma]time` is 32 bits of seconds; on a large inode each has an
    // `_extra` word whose low two bits extend the seconds to 34 and whose upper
    // thirty hold nanoseconds. Seconds produced here come from a 32-bit clock,
    // so written epoch bits are zero, but ones read from a volume shared with
    // Linux may not be.

    /// Seconds and nanoseconds out of a timestamp and its `_extra` word
    fn decode_time(seconds: u32, extra: u32) -> (u64, u32) {
        let epoch = (extra & 3) as u64;
        ((seconds as u64) | (epoch << 32), extra >> 2)
    }

    /// The `_extra` word for a given time
    fn encode_extra(seconds: u64, nsec: u32) -> u32 {
        (((seconds >> 32) & 3) as u32) | (nsec << 2)
    }

    pub fn atime_full(&self) -> (u64, u32) {
        Self::decode_time(self.atime(), self.atime_extra())
    }
    pub fn ctime_full(&self) -> (u64, u32) {
        Self::decode_time(self.ctime(), self.ctime_extra())
    }
    pub fn mtime_full(&self) -> (u64, u32) {
        Self::decode_time(self.mtime(), self.mtime_extra())
    }
    pub fn crtime_full(&self) -> (u64, u32) {
        Self::decode_time(self.crtime(), self.crtime_extra())
    }

    pub fn set_atime_full(&mut self, seconds: u64, nsec: u32) {
        self.write_u32(8, seconds as u32);
        if self.on_disk_size >= 144 {
            self.write_u32(140, Self::encode_extra(seconds, nsec));
        }
    }
    pub fn set_ctime_full(&mut self, seconds: u64, nsec: u32) {
        self.write_u32(12, seconds as u32);
        if self.on_disk_size >= 136 {
            self.write_u32(132, Self::encode_extra(seconds, nsec));
        }
    }
    pub fn set_mtime_full(&mut self, seconds: u64, nsec: u32) {
        self.write_u32(16, seconds as u32);
        if self.on_disk_size >= 140 {
            self.write_u32(136, Self::encode_extra(seconds, nsec));
        }
    }
    pub fn set_crtime_full(&mut self, seconds: u64, nsec: u32) {
        if self.on_disk_size >= 148 {
            self.write_u32(144, seconds as u32);
        }
        if self.on_disk_size >= 152 {
            self.write_u32(148, Self::encode_extra(seconds, nsec));
        }
    }

    pub fn crtime(&self) -> u32 {
        if self.on_disk_size >= 148 {
            self.read_u32(144)
        } else {
            0
        }
    }

    pub fn crtime_extra(&self) -> u32 {
        if self.on_disk_size >= 152 {
            self.read_u32(148)
        } else {
            0
        }
    }

    pub fn version_hi(&self) -> u32 {
        if self.on_disk_size >= 156 {
            self.read_u32(152)
        } else {
            0
        }
    }

    pub fn projid(&self) -> u32 {
        if self.on_disk_size >= 160 {
            self.read_u32(156)
        } else {
            0
        }
    }

    pub fn uid_full(&self) -> u32 {
        let lo = self.uid() as u32;
        let hi = self.uid_hi() as u32;
        lo | (hi << 16)
    }

    pub fn gid_full(&self) -> u32 {
        let lo = self.gid() as u32;
        let hi = self.gid_hi() as u32;
        lo | (hi << 16)
    }

    /// Writes the whole 32-bit owner, both halves. `set_uid` alone leaves
    /// `l_i_uid_high`, so chowning from uid 100000 to 0 would read back 65536
    pub fn set_uid_full(&mut self, uid: u32) {
        self.write_u16(2, (uid & 0xFFFF) as u16);
        self.write_u16(120, (uid >> 16) as u16);
    }

    pub fn set_gid_full(&mut self, gid: u32) {
        self.write_u16(24, (gid & 0xFFFF) as u16);
        self.write_u16(122, (gid >> 16) as u16);
    }

    pub fn blocks_full(&self) -> u64 {
        let lo = self.blocks() as u64;
        let hi = self.blocks_hi() as u64;
        lo | (hi << 32)
    }

    pub fn file_acl_full(&self) -> u64 {
        let lo = self.file_acl_lo() as u64;
        let hi = self.file_acl_hi() as u64;
        lo | (hi << 32)
    }

    pub fn size(&self) -> u64 {
        let lo = self.size_lo() as u64;
        if self.is_regular() {
            let hi = self.size_hi() as u64;
            lo | (hi << 32)
        } else {
            lo
        }
    }

    /// Sets the full 64-bit size. The high half exists only for regular files:
    /// offset 108 is `i_size_high` there and `i_dir_acl` otherwise. `size()`
    /// reads it under the same condition
    pub fn set_size_full(&mut self, size: u64) {
        self.set_size(size as u32);
        if self.is_regular() && self.on_disk_size >= 112 {
            self.write_u32(108, (size >> 32) as u32);
        }
    }

    pub fn is_regular(&self) -> bool {
        self.mode() & 0xF000 == S_IFREG
    }
    pub fn is_directory(&self) -> bool {
        self.mode() & 0xF000 == S_IFDIR
    }
    pub fn is_symlink(&self) -> bool {
        self.mode() & 0xF000 == S_IFLNK
    }
    pub fn is_chardev(&self) -> bool {
        self.mode() & 0xF000 == S_IFCHR
    }
    pub fn is_blockdev(&self) -> bool {
        self.mode() & 0xF000 == S_IFBLK
    }
    pub fn is_fifo(&self) -> bool {
        self.mode() & 0xF000 == S_IFIFO
    }
    pub fn is_socket(&self) -> bool {
        self.mode() & 0xF000 == S_IFSOCK
    }

    /// A device node, FIFO, or socket: no contents. `i_block` holds a device
    /// number or nothing
    pub fn is_special(&self) -> bool {
        matches!(self.mode() & 0xF000, S_IFCHR | S_IFBLK | S_IFIFO | S_IFSOCK)
    }

    pub fn file_type(&self) -> InodeType {
        match self.mode() & 0xF000 {
            S_IFREG => InodeType::Regular,
            S_IFDIR => InodeType::Directory,
            S_IFLNK => InodeType::Symlink,
            S_IFCHR => InodeType::CharDevice,
            S_IFBLK => InodeType::BlockDevice,
            S_IFIFO => InodeType::Fifo,
            S_IFSOCK => InodeType::Socket,
            _ => InodeType::Unknown,
        }
    }

    pub fn permissions(&self) -> u16 {
        self.mode() & 0o7777
    }

    pub fn is_fast_symlink(&self) -> bool {
        self.is_symlink() && self.blocks() == 0 && self.size_lo() <= 60
    }

    pub fn fast_symlink_target(&self) -> &[u8] {
        let len = (self.size_lo() as usize).min(60);
        &self.data[40..40 + len]
    }

    pub fn uses_extents(&self) -> bool {
        self.flags() & EXT4_EXTENTS_FL != 0
    }

    pub fn has_inline_data(&self) -> bool {
        self.flags() & EXT4_INLINE_DATA_FL != 0
    }

    pub fn is_huge_file(&self) -> bool {
        self.flags() & EXT4_HUGE_FILE_FL != 0
    }

    pub fn extent_header(&self) -> ExtentHeader {
        ExtentHeader {
            magic: self.read_u16(40),
            entries: self.read_u16(42),
            max: self.read_u16(44),
            depth: self.read_u16(46),
            generation: self.read_u32(48),
        }
    }

    pub fn extent_at(&self, idx: usize) -> Extent {
        let base = 52 + idx * 12;
        Extent {
            block: self.read_u32(base),
            len: self.read_u16(base + 4),
            start_hi: self.read_u16(base + 6),
            start_lo: self.read_u32(base + 8),
        }
    }

    pub fn extent_idx_at(&self, idx: usize) -> ExtentIdx {
        let base = 52 + idx * 12;
        ExtentIdx {
            block: self.read_u32(base),
            leaf_lo: self.read_u32(base + 4),
            leaf_hi: self.read_u16(base + 8),
        }
    }

    pub fn set_mode(&mut self, mode: u16) {
        self.write_u16(0, mode);
    }
    pub fn set_uid(&mut self, uid: u16) {
        self.write_u16(2, uid);
    }
    pub fn set_size(&mut self, size: u32) {
        self.write_u32(4, size);
    }
    pub fn set_atime(&mut self, t: u32) {
        self.set_atime_full(t as u64, 0);
    }
    pub fn set_ctime(&mut self, t: u32) {
        self.set_ctime_full(t as u64, 0);
    }
    pub fn set_mtime(&mut self, t: u32) {
        self.set_mtime_full(t as u64, 0);
    }
    pub fn set_dtime(&mut self, t: u32) {
        self.write_u32(20, t);
    }
    pub fn set_gid(&mut self, gid: u16) {
        self.write_u16(24, gid);
    }
    pub fn set_links_count(&mut self, count: u16) {
        self.write_u16(26, count);
    }
    pub fn set_blocks(&mut self, blocks: u32) {
        self.write_u32(28, blocks);
    }

    /// The full 48-bit sector count, low half in `i_blocks_lo`, high half in
    /// `l_i_blocks_high`
    pub fn set_blocks_full(&mut self, blocks: u64) {
        self.write_u32(28, (blocks & 0xFFFF_FFFF) as u32);
        if self.on_disk_size > 116 {
            self.write_u16(116, ((blocks >> 32) & 0xFFFF) as u16);
        }
    }
    pub fn set_flags(&mut self, flags: u32) {
        self.write_u32(32, flags);
    }

    pub fn set_block(&mut self, index: usize, val: u32) {
        if index < 15 {
            self.write_u32(40 + index * 4, val);
        }
    }

    pub fn set_generation(&mut self, gen: u32) {
        self.write_u32(100, gen);
    }
    pub fn set_file_acl_lo(&mut self, acl: u32) {
        self.write_u32(104, acl);
    }

    /// Blanks the inode, keeping `i_extra_isize`, which declares that fields
    /// past byte 128 exist
    fn wipe(&mut self) {
        let extra = self.extra_isize();
        self.data = [0; 256];
        if extra != 0 {
            self.write_u16(128, extra);
        }
    }

    pub fn init_file(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.wipe();
        self.set_mode(S_IFREG | mode);
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(1);
    }

    pub fn init_dir(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.wipe();
        self.set_mode(S_IFDIR | mode);
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(2);
    }

    pub fn init_symlink(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.wipe();
        self.set_mode(S_IFLNK | mode);
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(1);
    }

    pub fn init_extent_header(&mut self, max_entries: u16) {
        self.write_u16(40, EXT4_EXT_MAGIC);
        self.write_u16(42, 0);
        self.write_u16(44, max_entries);
        self.write_u16(46, 0);
        self.write_u32(48, 0);
        self.set_flags(self.flags() | EXT4_EXTENTS_FL);
    }

    pub fn set_extent_entries(&mut self, count: u16) {
        self.write_u16(42, count);
    }

    pub fn set_extent_depth(&mut self, depth: u16) {
        self.write_u16(46, depth);
    }

    pub fn set_extent_generation(&mut self, gen: u32) {
        self.write_u32(48, gen);
    }

    pub fn set_extent_at_raw(
        &mut self,
        idx: usize,
        block: u32,
        len: u16,
        start_hi: u16,
        start_lo: u32,
    ) {
        let base = 52 + idx * 12;
        if base + 12 > 100 {
            return;
        }
        self.write_u32(base, block);
        self.write_u16(base + 4, len);
        self.write_u16(base + 6, start_hi);
        self.write_u32(base + 8, start_lo);
    }

    pub fn set_extent_idx_at_raw(&mut self, idx: usize, block: u32, leaf_lo: u32, leaf_hi: u16) {
        let base = 52 + idx * 12;
        if base + 12 > 100 {
            return;
        }
        self.write_u32(base, block);
        self.write_u32(base + 4, leaf_lo);
        self.write_u16(base + 8, leaf_hi);
        self.write_u16(base + 10, 0);
    }

    pub fn set_extent_len_at(&mut self, idx: usize, len: u16) {
        let base = 52 + idx * 12;
        if base + 6 > 100 {
            return;
        }
        self.write_u16(base + 4, len);
    }

    pub fn clear_block_pointers(&mut self) {
        for i in 0..15 {
            self.set_block(i, 0);
        }
    }

    pub fn write_inline_data(&mut self, data: &[u8]) {
        let len = data.len().min(60);
        self.data[40..40 + len].copy_from_slice(&data[..len]);
    }

    pub fn read_inline_data(&self, size: usize) -> &[u8] {
        let len = size.min(60);
        &self.data[40..40 + len]
    }

    /// A device, FIFO, or socket inode, with no data or blocks. Linux has two
    /// encodings by minor number: the old packs both halves into `i_block[0]`,
    /// the new spreads them over `i_block[1]` for a minor past 255. The old
    /// form is written when it fits, as other implementations read
    pub fn init_special(
        &mut self, type_bits: u16, mode: u16, uid: u16, gid: u16, now: u32,
        major: u32, minor: u32,
    ) {
        self.wipe();
        self.set_mode(type_bits | (mode & 0o7777));
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(1);
        if type_bits == S_IFCHR || type_bits == S_IFBLK {
            if major < 256 && minor < 256 {
                self.set_block(0, (major << 8) | minor);
            } else {
                self.set_block(1, (minor & 0xFF) | (major << 8) | ((minor & !0xFF) << 12));
            }
        }
    }

    /// The device number an `init_special` inode carries, as (major, minor).
    pub fn device_number(&self) -> (u32, u32) {
        let old = self.block(0);
        if old != 0 {
            return ((old >> 8) & 0xFF, old & 0xFF);
        }
        let new = self.block(1);
        ((new >> 8) & 0xFFF, (new & 0xFF) | ((new >> 12) & 0xFFF00))
    }

    pub fn init_file_ext4(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.wipe();
        self.set_mode(S_IFREG | mode);
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(1);
        self.init_extent_header(4);
    }

    pub fn init_dir_ext4(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.wipe();
        self.set_mode(S_IFDIR | mode);
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(2);
        self.init_extent_header(4);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExtentHeader {
    pub magic: u16,
    pub entries: u16,
    pub max: u16,
    pub depth: u16,
    pub generation: u32,
}

/// Bytes of extent storage in an inode: `i_block` is 15 pointers of 4 bytes,
/// and a tree root uses all of it
pub const EXTENT_ROOT_SPACE: usize = 60;

/// ext4's limit. A deeper header is corrupt, and believing it would recurse
/// with a block-sized buffer per frame
pub const EXTENT_MAX_DEPTH: u16 = 5;

impl ExtentHeader {
    /// Entries fitting in `space` bytes: the 12-byte header, then 12 bytes each
    pub const fn capacity(space: usize) -> u16 {
        if space < 12 {
            return 0;
        }
        ((space - 12) / 12) as u16
    }

    /// Magic only, for a caller asking whether an inode uses extents
    pub fn valid(&self) -> bool {
        self.magic == EXT4_EXT_MAGIC
    }

    /// What must hold before this header's entries may be indexed. `space` is
    /// the entries' storage: `EXTENT_ROOT_SPACE` for a tree root in an inode,
    /// the block size for a node on disk. `entries` comes from disk and bounds
    /// a loop over a fixed-size array
    pub fn check(&self, space: usize, expected_depth: Option<u16>) -> bool {
        if self.magic != EXT4_EXT_MAGIC {
            return false;
        }
        if self.depth > EXTENT_MAX_DEPTH {
            return false;
        }
        // A node's depth must be what its parent said, or a cycle is walkable:
        // every node claims to be interior and no walk reaches a leaf
        if let Some(want) = expected_depth {
            if self.depth != want {
                return false;
            }
        }
        let cap = Self::capacity(space);
        if self.max > cap {
            return false;
        }
        self.entries <= self.max
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Extent {
    pub block: u32,
    pub len: u16,
    pub start_hi: u16,
    pub start_lo: u32,
}

impl Extent {
    pub fn start(&self) -> u64 {
        let lo = self.start_lo as u64;
        let hi = self.start_hi as u64;
        lo | (hi << 32)
    }

    pub fn is_uninitialized(&self) -> bool {
        self.len > 32768
    }

    pub fn actual_len(&self) -> u32 {
        if self.is_uninitialized() {
            (self.len - 32768) as u32
        } else {
            self.len as u32
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExtentIdx {
    pub block: u32,
    pub leaf_lo: u32,
    pub leaf_hi: u16,
}

impl ExtentIdx {
    pub fn leaf(&self) -> u64 {
        let lo = self.leaf_lo as u64;
        let hi = self.leaf_hi as u64;
        lo | (hi << 32)
    }
}

pub const EXT4_EXT_MAGIC: u16 = 0xF30A;

pub const S_IFIFO: u16 = 0x1000;
pub const S_IFCHR: u16 = 0x2000;
pub const S_IFDIR: u16 = 0x4000;
pub const S_IFBLK: u16 = 0x6000;
pub const S_IFREG: u16 = 0x8000;
pub const S_IFLNK: u16 = 0xA000;
pub const S_IFSOCK: u16 = 0xC000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    Unknown,
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

pub const FT_UNKNOWN: u8 = 0;
pub const FT_REG_FILE: u8 = 1;
pub const FT_DIR: u8 = 2;
pub const FT_CHRDEV: u8 = 3;
pub const FT_BLKDEV: u8 = 4;
pub const FT_FIFO: u8 = 5;
pub const FT_SOCK: u8 = 6;
pub const FT_SYMLINK: u8 = 7;

pub const EXT4_EXTENTS_FL: u32 = 0x00080000;
pub const EXT4_INLINE_DATA_FL: u32 = 0x10000000;
pub const EXT4_HUGE_FILE_FL: u32 = 0x00040000;
pub const EXT4_EA_INODE_FL: u32 = 0x00200000;
pub const EXT4_INDEX_FL: u32 = 0x00001000;
pub const EXT4_ENCRYPT_FL: u32 = 0x00000800;
pub const EXT4_IMMUTABLE_FL: u32 = 0x00000010;
pub const EXT4_APPEND_FL: u32 = 0x00000020;
pub const EXT4_NODUMP_FL: u32 = 0x00000040;
pub const EXT4_NOATIME_FL: u32 = 0x00000080;

#[derive(Clone, Copy)]
pub struct DirEntry {
    pub inode: u32,
    pub file_type: u8,
    pub name: [u8; MAX_NAME],
    pub name_len: u8,
}

impl DirEntry {
    pub const fn empty() -> Self {
        Self {
            inode: 0,
            file_type: FT_UNKNOWN,
            name: [0; MAX_NAME],
            name_len: 0,
        }
    }

    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }

    pub fn type_char(&self) -> char {
        match self.file_type {
            FT_DIR => 'd',
            FT_REG_FILE => '-',
            FT_SYMLINK => 'l',
            FT_CHRDEV => 'c',
            FT_BLKDEV => 'b',
            FT_FIFO => 'p',
            FT_SOCK => 's',
            _ => '?',
        }
    }
}

pub const EXT2_ROOT_INO: u32 = 2;
pub const EXT2_BAD_INO: u32 = 1;
pub const EXT2_USR_QUOTA_INO: u32 = 3;
pub const EXT2_GRP_QUOTA_INO: u32 = 4;
pub const EXT2_BOOT_LOADER_INO: u32 = 5;
pub const EXT2_UNDEL_DIR_INO: u32 = 6;
pub const EXT2_RESIZE_INO: u32 = 7;
pub const EXT2_JOURNAL_INO: u32 = 8;
pub const EXT2_EXCLUDE_INO: u32 = 9;
pub const EXT2_REPLICA_INO: u32 = 10;
pub const EXT2_FIRST_INO_OLD: u32 = 11;

/// `s_state`: unmounted cleanly, with no fault found since
pub const EXT2_VALID_FS: u16 = 1;
/// `s_state`: mounted, or left mounted by a crash, and untrusted until checked
pub const EXT2_ERROR_FS: u16 = 2;

pub const EXT2_GOOD_OLD_REV: u32 = 0;
pub const EXT2_DYNAMIC_REV: u32 = 1;

pub const EXT2_STATE_VALID: u16 = 0x0001;
pub const EXT2_STATE_ERROR: u16 = 0x0002;
pub const EXT2_STATE_ORPHAN: u16 = 0x0004;

pub const EXT2_ERRORS_CONTINUE: u16 = 1;
pub const EXT2_ERRORS_RO: u16 = 2;
pub const EXT2_ERRORS_PANIC: u16 = 3;
