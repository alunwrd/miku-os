pub const EXT2_MAGIC: u16 = 0xEF53;

pub const MAX_NAME: usize = 255;

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
    pub fn min_extra_isize(&self) -> u16 {
        self.read_u16(276)
    }
    pub fn want_extra_isize(&self) -> u16 {
        self.read_u16(278)
    }
    pub fn flags_sb(&self) -> u32 {
        self.read_u32(280)
    }
    pub fn raid_stride(&self) -> u16 {
        self.read_u16(284)
    }
    pub fn mmp_interval(&self) -> u16 {
        self.read_u16(286)
    }
    pub fn mmp_block(&self) -> u64 {
        let lo = self.read_u32(288) as u64;
        let hi = self.read_u32(292) as u64;
        lo | (hi << 32)
    }
    pub fn raid_stripe_width(&self) -> u32 {
        self.read_u32(296)
    }
    pub fn log_groups_per_flex(&self) -> u8 {
        self.data[300]
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

    pub fn is_ext4(&self) -> bool {
        self.has_extents() || self.has_64bit() || self.has_huge_file()
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

pub const FEATURE_COMPAT_DIR_PREALLOC: u32 = 0x0001;
pub const FEATURE_COMPAT_IMAGIC_INODES: u32 = 0x0002;
pub const FEATURE_COMPAT_HAS_JOURNAL: u32 = 0x0004;
pub const FEATURE_COMPAT_EXT_ATTR: u32 = 0x0008;
pub const FEATURE_COMPAT_RESIZE_INO: u32 = 0x0010;
pub const FEATURE_COMPAT_DIR_INDEX: u32 = 0x0020;
pub const FEATURE_COMPAT_SPARSE_SUPER2: u32 = 0x0200;

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

    fn read_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
    }

    fn read_u32(&self, offset: usize) -> u32 {
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

    pub fn set_size_full(&mut self, size: u64) {
        self.set_size(size as u32);
        if self.on_disk_size >= 112 {
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
        self.write_u32(8, t);
    }
    pub fn set_ctime(&mut self, t: u32) {
        self.write_u32(12, t);
    }
    pub fn set_mtime(&mut self, t: u32) {
        self.write_u32(16, t);
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

    pub fn init_file(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.data = [0; 256];
        self.set_mode(S_IFREG | mode);
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(1);
    }

    pub fn init_dir(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.data = [0; 256];
        self.set_mode(S_IFDIR | mode);
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_atime(now);
        self.set_ctime(now);
        self.set_mtime(now);
        self.set_links_count(2);
    }

    pub fn init_symlink(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.data = [0; 256];
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

    pub fn init_file_ext4(&mut self, mode: u16, uid: u16, gid: u16, now: u32) {
        self.data = [0; 256];
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
        self.data = [0; 256];
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

impl ExtentHeader {
    pub fn valid(&self) -> bool {
        self.magic == EXT4_EXT_MAGIC
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

pub const EXT2_GOOD_OLD_REV: u32 = 0;
pub const EXT2_DYNAMIC_REV: u32 = 1;

pub const EXT2_STATE_VALID: u16 = 0x0001;
pub const EXT2_STATE_ERROR: u16 = 0x0002;
pub const EXT2_STATE_ORPHAN: u16 = 0x0004;

pub const EXT2_ERRORS_CONTINUE: u16 = 1;
pub const EXT2_ERRORS_RO: u16 = 2;
pub const EXT2_ERRORS_PANIC: u16 = 3;
