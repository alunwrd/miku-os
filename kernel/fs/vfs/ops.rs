use crate::fs::vfs::types::*;

pub trait FsOps {
    fn fs_type(&self) -> FsType;
    fn sync(&mut self) -> VfsResult<()>;
    fn statfs(&self) -> VfsResult<StatFs>;
}

pub trait VNodeOps {
    fn lookup(&self, name: &str) -> VfsResult<InodeId>;
    fn create(&mut self, name: &str, kind: VNodeKind, mode: FileMode) -> VfsResult<InodeId>;
    fn remove(&mut self, name: &str) -> VfsResult<()>;
    fn readdir(&self, offset: usize) -> VfsResult<Option<DirEntry>>;
    fn read(&mut self, buf: &mut [u8], offset: u64) -> VfsResult<usize>;
    fn write(&mut self, buf: &[u8], offset: u64) -> VfsResult<usize>;
}

pub struct TmpFsOps;

impl FsOps for TmpFsOps {
    fn fs_type(&self) -> FsType {
        FsType::TmpFS
    }

    fn sync(&mut self) -> VfsResult<()> {
        Ok(())
    }

    fn statfs(&self) -> VfsResult<StatFs> {
        Ok(StatFs {
            fs_type: FsType::TmpFS,
            block_size: PAGE_SIZE as u32,
            total_blocks: MAX_DATA_PAGES as u64,
            free_blocks: 0,
            total_inodes: MAX_VNODES as u64,
            free_inodes: 0,
            max_name_len: NAME_LEN as u32,
            flags: 0,
        })
    }
}

/// Ops vtable for the whole ext family. The concrete flavour is decided per
/// mount from the superblock feature bits (`active_fs_type()`), never
/// assumed - reporting ext4 as "ext2" was making `df`, `statfs()` and
/// `/proc/mounts` lie about every journalled or extent-based volume
pub struct ExtFsOps;

/// Kept so existing call sites keep compiling; the ops are flavour-agnostic
pub type Ext2FsOps = ExtFsOps;

impl FsOps for ExtFsOps {
    fn fs_type(&self) -> FsType {
        crate::fs::ext::mount::active_fs_type()
    }

    fn sync(&mut self) -> VfsResult<()> {
        let result = crate::fs::ext::mount::with_ext2_pub(|fs| {
            fs.sync().map_err(|_| VfsError::IoError)
        });
        match result {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(e),
            None => Err(VfsError::NotMounted),
        }
    }

    fn statfs(&self) -> VfsResult<StatFs> {
        let info = crate::fs::ext::mount::with_ext2_pub(|fs| fs.fs_info());
        match info {
            Some(i) => Ok(StatFs {
                fs_type: crate::fs::ext::mount::active_fs_type(),
                block_size: i.block_size,
                total_blocks: i.total_blocks as u64,
                free_blocks: i.free_blocks as u64,
                total_inodes: i.total_inodes as u64,
                free_inodes: i.free_inodes as u64,
                max_name_len: 255,
                flags: 0,
            }),
            None => Err(VfsError::NotMounted),
        }
    }
}
