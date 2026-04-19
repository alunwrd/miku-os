use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

impl MikuFS {
    pub fn ext2_hardlink(
        &mut self,
        parent_ino: u32,
        name: &str,
        target_ino: u32,
    ) -> Result<(), FsError> {
        let parent = self.read_inode(parent_ino)?;
        if !parent.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let target = self.read_inode(target_ino)?;
        if target.is_directory() {
            return Err(FsError::IsDirectory);
        }

        match self.lookup(&parent, name) {
            Ok(_) => return Err(FsError::AlreadyExists),
            Err(FsError::NotFound) => {}
            Err(e) => return Err(e),
        }

        let ft = match target.file_type() {
            InodeType::Symlink => FT_SYMLINK,
            _ => FT_REG_FILE,
        };

        self.add_dir_entry(parent_ino, name, target_ino, ft)?;

        let mut target = self.read_inode(target_ino)?;
        target.set_links_count(target.links_count() + 1);
        let now = self.get_timestamp();
        target.set_ctime(now);
        self.write_inode(target_ino, &target)?;

        // update parent dir timestamps
        let mut parent = self.read_inode(parent_ino)?;
        parent.set_mtime(now);
        parent.set_ctime(now);
        self.write_inode(parent_ino, &parent)?;

        Ok(())
    }

    pub fn ext2_unlink_hardlink(&mut self, parent_ino: u32, name: &str) -> Result<(), FsError> {
        let parent_inode = self.read_inode(parent_ino)?;
        let target_ino = match self.lookup(&parent_inode, name) {
            Ok(ino) => ino,
            Err(FsError::NotFound) => return Err(FsError::NotFound),
            Err(e) => return Err(e),
        };

        let mut inode = self.read_inode(target_ino)?;
        if inode.is_directory() {
            return Err(FsError::IsDirectory);
        }

        self.remove_dir_entry(parent_ino, name)?;

        // update parent dir timestamps
        let now = self.get_timestamp();
        let mut parent_inode = self.read_inode(parent_ino)?;
        parent_inode.set_mtime(now);
        parent_inode.set_ctime(now);
        self.write_inode(parent_ino, &parent_inode)?;

        let links = inode.links_count();
        if links > 1 {
            inode.set_links_count(links - 1);
            let now = self.get_timestamp();
            inode.set_ctime(now);
            self.write_inode(target_ino, &inode)?;
        } else {
            if inode.uses_extents() {
                self.ext4_free_extent_blocks(&inode)?;
            } else if !inode.is_symlink() || !inode.is_fast_symlink() {
                self.free_all_blocks(&inode)?;
            }

            let now = self.get_timestamp();
            inode.set_dtime(now);
            inode.set_links_count(0);

            self.write_inode(target_ino, &inode)?;
            self.free_inode(target_ino)?;
        }
        Ok(())
    }
}
