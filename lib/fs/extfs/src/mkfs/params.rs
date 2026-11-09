/// The smallest journal JBD2 recognizes. Below it e2fsck reports an invalid
/// journal and cannot check
pub const MIN_JOURNAL_BLOCKS: u32 = 1024;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FsType {
    Ext2,
    Ext3,
    Ext4,
}

impl FsType {
    pub fn name(self) -> &'static str {
        match self {
            FsType::Ext2 => "ext2",
            FsType::Ext3 => "ext3",
            FsType::Ext4 => "ext4",
        }
    }
    pub fn needs_journal(self) -> bool {
        matches!(self, FsType::Ext3 | FsType::Ext4)
    }
    pub fn needs_extents(self) -> bool {
        matches!(self, FsType::Ext4)
    }
}

pub struct MkfsParams {
    pub fs_type:        FsType,
    pub drive_index:    usize,
    pub total_sectors:  u32,
    pub start_lba:      u32,
    pub block_size:     u32,
    pub inode_size:     u32,
    pub journal_blocks: u32,
    pub label:          [u8; 16],
}

impl MkfsParams {
    pub fn new(fs_type: FsType, drive_index: usize) -> Self {
        // JBD2 and e2fsck require at least 1024 journal blocks
        let (block_size, inode_size, journal_blocks) = match fs_type {
            FsType::Ext2 => (1024, 128, 0),
            FsType::Ext3 => (1024, 128, MIN_JOURNAL_BLOCKS),
            FsType::Ext4 => (4096, 256, MIN_JOURNAL_BLOCKS),
        };
        Self {
            fs_type,
            drive_index,
            total_sectors:  0,
            start_lba:      0,
            block_size,
            inode_size,
            journal_blocks,
            label: *b"miku\0\0\0\0\0\0\0\0\0\0\0\0",
        }
    }
}
