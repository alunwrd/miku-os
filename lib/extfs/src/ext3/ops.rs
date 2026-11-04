use crate::{FsError, MikuFS};
use crate::host::ExtHost;

impl<H: ExtHost> MikuFS<H> {
    /// Creates a file owned by root. For the `mkfs`/image path and many
    /// callers; other owners use `ext3_create_file_as`
    pub fn ext3_create_file(
        &mut self, parent_ino: u32, name: &str, mode: u16,
    ) -> Result<u32, FsError> {
        self.ext3_create_file_as(parent_ino, name, mode, 0, 0)
    }

    /// Creates a file owned by `uid`:`gid`. `ext2_create_file` and
    /// `ext4_create_file` write `init_file(mode, 0, 0)`, so the owner is set in
    /// the same transaction as the creation
    pub fn ext3_create_file_as(
        &mut self, parent_ino: u32, name: &str, mode: u16, uid: u32, gid: u32,
    ) -> Result<u32, FsError> {
        let use_extents = self.superblock.has_extents();

        if !self.journal_active {
            let new_ino = if use_extents {
                self.ext4_create_file(parent_ino, name, mode)?
            } else {
                self.ext2_create_file(parent_ino, name, mode)?
            };
            if (uid, gid) != (0, 0) {
                self.ext2_chown(new_ino, uid, gid)?;
            }
            return Ok(new_ino);
        }

        self.ext3_begin_txn()?;

        let result = if use_extents {
            self.ext4_create_file(parent_ino, name, mode)
        } else {
            self.ext2_create_file(parent_ino, name, mode)
        }
        .and_then(|new_ino| {
            if (uid, gid) != (0, 0) {
                self.ext2_chown(new_ino, uid, gid)?;
            }
            Ok(new_ino)
        });

        match result {
            Ok(new_ino) => {
                self.ext3_commit_txn()?;
                Ok(new_ino)
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_create_dir(
        &mut self, parent_ino: u32, name: &str, mode: u16,
    ) -> Result<u32, FsError> {
        self.ext3_create_dir_as(parent_ino, name, mode, 0, 0)
    }

    /// Create a directory owned by `uid`:`gid`. See `ext3_create_file_as`.
    pub fn ext3_create_dir_as(
        &mut self, parent_ino: u32, name: &str, mode: u16, uid: u32, gid: u32,
    ) -> Result<u32, FsError> {
        let use_extents = self.superblock.has_extents();

        if !self.journal_active {
            let new_ino = if use_extents {
                self.ext4_create_dir(parent_ino, name, mode)?
            } else {
                self.ext2_create_dir(parent_ino, name, mode)?
            };
            if (uid, gid) != (0, 0) {
                self.ext2_chown(new_ino, uid, gid)?;
            }
            return Ok(new_ino);
        }

        self.ext3_begin_txn()?;

        let result = if use_extents {
            self.ext4_create_dir(parent_ino, name, mode)
        } else {
            self.ext2_create_dir(parent_ino, name, mode)
        }
        .and_then(|new_ino| {
            if (uid, gid) != (0, 0) {
                self.ext2_chown(new_ino, uid, gid)?;
            }
            Ok(new_ino)
        });

        match result {
            Ok(new_ino) => {
                self.ext3_commit_txn()?;
                Ok(new_ino)
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_write_file(
        &mut self, inode_num: u32, data: &[u8], offset: u64,
    ) -> Result<usize, FsError> {
        let use_extents = {
            let inode = self.read_inode(inode_num)?;
            inode.uses_extents() || self.superblock.has_extents()
        };

        if !self.journal_active {
            return if use_extents {
                self.ext4_write_file(inode_num, data, offset)
            } else {
                self.ext2_write_file(inode_num, data, offset)
            };
        }

        self.ext3_begin_txn()?;

        let result = if use_extents {
            self.ext4_write_file(inode_num, data, offset)
        } else {
            self.ext2_write_file(inode_num, data, offset)
        };

        match result {
            Ok(n) => {
                self.ext3_commit_txn()?;
                Ok(n)
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    /// `parts` back to back from `offset`, in one transaction: the page cache's
    /// write-out of a run of pages
    pub fn ext3_write_parts(
        &mut self, inode_num: u32, parts: &[&[u8]], offset: u64,
    ) -> Result<usize, FsError> {
        let use_extents = {
            let inode = self.read_inode(inode_num)?;
            inode.uses_extents() || self.superblock.has_extents()
        };
        let write = |fs: &mut Self| -> Result<usize, FsError> {
            if use_extents {
                return fs.ext4_write_parts(inode_num, parts, offset);
            }
            let mut done = 0usize;
            for data in parts {
                let n = fs.ext2_write_file(inode_num, data, offset + done as u64)?;
                done += n;
                if n < data.len() {
                    break;
                }
            }
            Ok(done)
        };

        if !self.journal_active {
            return write(self);
        }

        self.ext3_begin_txn()?;
        match write(self) {
            Ok(n) => {
                self.ext3_commit_txn()?;
                Ok(n)
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_append_file(
        &mut self, inode_num: u32, data: &[u8],
    ) -> Result<usize, FsError> {
        if !self.journal_active {
            return self.ext2_append_file(inode_num, data);
        }

        self.ext3_begin_txn()?;

        match self.ext2_append_file(inode_num, data) {
            Ok(n) => {
                self.ext3_commit_txn()?;
                Ok(n)
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_delete_file(
        &mut self, parent_ino: u32, name: &str,
    ) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };

        let use_extents = {
            let inode = self.read_inode(target_ino)?;
            inode.uses_extents()
        };

        if !self.journal_active {
            return if use_extents {
                self.ext4_delete_file(parent_ino, name)
            } else {
                self.ext2_delete_file(parent_ino, name)
            };
        }

        self.ext3_begin_txn()?;

        let result = if use_extents {
            self.ext4_delete_file(parent_ino, name)
        } else {
            self.ext2_delete_file(parent_ino, name)
        };

        match result {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_delete_dir(
        &mut self, parent_ino: u32, name: &str,
    ) -> Result<(), FsError> {
        let target_ino = match self.ext2_lookup_in_dir(parent_ino, name)? {
            Some(ino) => ino,
            None => return Err(FsError::NotFound),
        };

        let use_extents = {
            let inode = self.read_inode(target_ino)?;
            inode.uses_extents()
        };

        if !self.journal_active {
            return if use_extents {
                self.ext4_delete_dir(parent_ino, name)
            } else {
                self.ext2_delete_dir(parent_ino, name)
            };
        }

        self.ext3_begin_txn()?;

        let result = if use_extents {
            self.ext4_delete_dir(parent_ino, name)
        } else {
            self.ext2_delete_dir(parent_ino, name)
        };

        match result {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_mknod(
        &mut self,
        parent_ino: u32,
        name: &str,
        kind: u8,
        mode: u16,
        major: u32,
        minor: u32,
    ) -> Result<u32, FsError> {
        self.ext3_mknod_as(parent_ino, name, kind, mode, major, minor, 0, 0)
    }

    /// Creates a device, FIFO, or socket owned by `uid`:`gid`. See
    /// `ext3_create_file_as`
    #[allow(clippy::too_many_arguments)]
    pub fn ext3_mknod_as(
        &mut self,
        parent_ino: u32,
        name: &str,
        kind: u8,
        mode: u16,
        major: u32,
        minor: u32,
        uid: u32,
        gid: u32,
    ) -> Result<u32, FsError> {
        if !self.journal_active {
            let new_ino = self.ext2_mknod(parent_ino, name, kind, mode, major, minor)?;
            if (uid, gid) != (0, 0) {
                self.ext2_chown(new_ino, uid, gid)?;
            }
            return Ok(new_ino);
        }

        self.ext3_begin_txn()?;

        let result = self
            .ext2_mknod(parent_ino, name, kind, mode, major, minor)
            .and_then(|new_ino| {
                if (uid, gid) != (0, 0) {
                    self.ext2_chown(new_ino, uid, gid)?;
                }
                Ok(new_ino)
            });

        match result {
            Ok(new_ino) => {
                self.ext3_commit_txn()?;
                Ok(new_ino)
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    pub fn ext3_create_symlink(
        &mut self,
        parent_ino: u32,
        name: &str,
        target: &str,
    ) -> Result<u32, FsError> {
        self.ext3_create_symlink_as(parent_ino, name, target, 0, 0)
    }

    /// Create a symlink owned by `uid`:`gid`. See `ext3_create_file_as`.
    pub fn ext3_create_symlink_as(
        &mut self,
        parent_ino: u32,
        name: &str,
        target: &str,
        uid: u32,
        gid: u32,
    ) -> Result<u32, FsError> {
        if !self.journal_active {
            let new_ino = self.ext2_create_symlink(parent_ino, name, target)?;
            if (uid, gid) != (0, 0) {
                self.ext2_chown(new_ino, uid, gid)?;
            }
            return Ok(new_ino);
        }

        self.ext3_begin_txn()?;

        let result = self
            .ext2_create_symlink(parent_ino, name, target)
            .and_then(|new_ino| {
                if (uid, gid) != (0, 0) {
                    self.ext2_chown(new_ino, uid, gid)?;
                }
                Ok(new_ino)
            });

        match result {
            Ok(new_ino) => {
                self.ext3_commit_txn()?;
                Ok(new_ino)
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    pub fn ext3_rename(
        &mut self,
        parent_ino: u32,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), FsError> {
        if !self.journal_active {
            return self.ext2_rename(parent_ino, old_name, new_name);
        }

        self.ext3_begin_txn()?;

        // No pre-flight lookup: `ext2_rename` reports a missing name, and the
        // arm below aborts on any error
        let result = self.ext2_rename(parent_ino, old_name, new_name);

        match result {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    /// A rename across directories, journaled. The entry arrives in the
    /// destination, leaves the source, and fixes up `..` in one transaction, so
    /// a crash leaves the name in one directory
    pub fn ext3_move(
        &mut self,
        src_parent: u32,
        src_name: &str,
        dst_parent: u32,
        dst_name: &str,
    ) -> Result<(), FsError> {
        if !self.journal_active {
            return self.ext2_move(src_parent, src_name, dst_parent, dst_name);
        }

        self.ext3_begin_txn()?;

        let result = self.ext2_move(src_parent, src_name, dst_parent, dst_name);

        match result {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    pub fn ext3_hardlink(
        &mut self,
        parent_ino: u32,
        name: &str,
        target_ino: u32,
    ) -> Result<(), FsError> {
        if !self.journal_active {
            return self.ext2_hardlink(parent_ino, name, target_ino);
        }

        self.ext3_begin_txn()?;

        let result = self.ext2_hardlink(parent_ino, name, target_ino);

        match result {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    /// Sets an extended attribute, journaled, so the inode does not refer to an
    /// unwritten attribute block after a crash
    pub fn ext3_set_xattr(
        &mut self,
        inode_num: u32,
        name_index: u8,
        name: &str,
        value: &[u8],
    ) -> Result<(), FsError> {
        if !self.journal_active {
            return self.set_xattr(inode_num, name_index, name, value);
        }
        self.ext3_begin_txn()?;
        match self.set_xattr(inode_num, name_index, name, value) {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    /// Remove an extended attribute, journalled
    pub fn ext3_remove_xattr(
        &mut self,
        inode_num: u32,
        name_index: u8,
        name: &str,
    ) -> Result<(), FsError> {
        if !self.journal_active {
            return self.remove_xattr(inode_num, name_index, name);
        }
        self.ext3_begin_txn()?;
        match self.remove_xattr(inode_num, name_index, name) {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    /// Truncate to zero, journalled
    pub fn ext3_truncate(&mut self, inode_num: u32) -> Result<(), FsError> {
        self.ext3_truncate_to(inode_num, 0)
    }

    /// Truncates to any length, journaled
    pub fn ext3_truncate_to(&mut self, inode_num: u32, new_size: u64) -> Result<(), FsError> {
        let use_extents = {
            let inode = self.read_inode(inode_num)?;
            inode.uses_extents()
        };

        if !self.journal_active {
            return if use_extents {
                self.ext4_truncate_to(inode_num, new_size)
            } else {
                self.ext2_truncate_to(inode_num, new_size)
            };
        }

        self.ext3_begin_txn()?;

        let result = if use_extents {
            self.ext4_truncate_to(inode_num, new_size)
        } else {
            self.ext2_truncate_to(inode_num, new_size)
        };

        match result {
            Ok(()) => {
                self.ext3_commit_txn()?;
                Ok(())
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    pub fn ext3_write_file_create_or_overwrite(
        &mut self, parent_ino: u32, name: &str, mode: u16, data: &[u8],
    ) -> Result<u32, FsError> {
        let use_extents = self.superblock.has_extents();

        match self.ext2_lookup_in_dir(parent_ino, name)? {
            Some(ino) => {
                if !self.journal_active {
                    let inode = self.read_inode(ino)?;
                    self.free_all_blocks(&inode)?;
                    let mut inode = self.read_inode(ino)?;
                    for i in 0..15 { inode.set_block(i, 0); }
                    inode.set_size(0);
                    inode.set_blocks(0);
                    inode.set_mtime(self.get_timestamp());
                    self.write_inode(ino, &inode)?;
                    if use_extents {
                        self.ext4_write_file(ino, data, 0)?;
                    } else {
                        self.ext2_write_file(ino, data, 0)?;
                    }
                    return Ok(ino);
                }

                self.ext3_begin_txn()?;

                let inode = self.read_inode(ino)?;
                self.free_all_blocks(&inode)?;
                let mut inode = self.read_inode(ino)?;
                for i in 0..15 { inode.set_block(i, 0); }
                inode.set_size(0);
                inode.set_blocks(0);
                inode.set_mtime(self.get_timestamp());
                self.write_inode(ino, &inode)?;

                let write_result = if use_extents {
                    self.ext4_write_file(ino, data, 0)
                } else {
                    self.ext2_write_file(ino, data, 0)
                };

                match write_result {
                    Ok(_) => {
                        self.ext3_commit_txn()?;
                        Ok(ino)
                    }
                    Err(e) => { self.ext3_abort_txn(); Err(e) }
                }
            }
            None => {
                if !self.journal_active {
                    let ino = if use_extents {
                        self.ext4_create_file(parent_ino, name, mode)?
                    } else {
                        self.ext2_create_file(parent_ino, name, mode)?
                    };
                    if use_extents {
                        self.ext4_write_file(ino, data, 0)?;
                    } else {
                        self.ext2_write_file(ino, data, 0)?;
                    }
                    return Ok(ino);
                }

                self.ext3_begin_txn()?;

                let create_result = if use_extents {
                    self.ext4_create_file(parent_ino, name, mode)
                } else {
                    self.ext2_create_file(parent_ino, name, mode)
                };

                match create_result {
                    Ok(new_ino) => {
                        let write_result = if use_extents {
                            self.ext4_write_file(new_ino, data, 0)
                        } else {
                            self.ext2_write_file(new_ino, data, 0)
                        };
                        match write_result {
                            Ok(_) => {
                                self.ext3_commit_txn()?;
                                Ok(new_ino)
                            }
                            Err(e) => { self.ext3_abort_txn(); Err(e) }
                        }
                    }
                    Err(e) => { self.ext3_abort_txn(); Err(e) }
                }
            }
        }
    }
}
