use super::journal::*;
use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

impl MikuFS {
    pub fn ext3_create_file(
        &mut self, parent_ino: u32, name: &str, mode: u16,
    ) -> Result<u32, FsError> {
        let use_extents = self.superblock.has_extents();

        if !self.journal_active {
            return if use_extents {
                self.ext4_create_file(parent_ino, name, mode)
            } else {
                self.ext2_create_file(parent_ino, name, mode)
            };
        }

        self.ext3_begin_txn()?;
        let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        self.journal_inode_blocks(parent_ino)?;
        self.journal_inode_metadata(parent_ino)?;
        self.journal_group_metadata(group)?;

        let result = if use_extents {
            self.ext4_create_file(parent_ino, name, mode)
        } else {
            self.ext2_create_file(parent_ino, name, mode)
        };

        match result {
            Ok(new_ino) => {
                self.journal_inode_metadata(new_ino)?;
                let new_group = ((new_ino - 1) / self.inodes_per_group) as usize;
                if new_group != group { self.journal_group_metadata(new_group)?; }
                // commit deferred to pdflush
                Ok(new_ino)
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_create_dir(
        &mut self, parent_ino: u32, name: &str, mode: u16,
    ) -> Result<u32, FsError> {
        let use_extents = self.superblock.has_extents();

        if !self.journal_active {
            return if use_extents {
                self.ext4_create_dir(parent_ino, name, mode)
            } else {
                self.ext2_create_dir(parent_ino, name, mode)
            };
        }

        self.ext3_begin_txn()?;
        let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        self.journal_inode_blocks(parent_ino)?;
        self.journal_inode_metadata(parent_ino)?;
        self.journal_group_metadata(group)?;

        let result = if use_extents {
            self.ext4_create_dir(parent_ino, name, mode)
        } else {
            self.ext2_create_dir(parent_ino, name, mode)
        };

        match result {
            Ok(new_ino) => {
                self.journal_inode_blocks(new_ino)?;
                self.journal_inode_metadata(new_ino)?;
                let new_group = ((new_ino - 1) / self.inodes_per_group) as usize;
                if new_group != group { self.journal_group_metadata(new_group)?; }
                // commit deferred to pdflush
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
        self.journal_inode_metadata(inode_num)?;

        let result = if use_extents {
            self.ext4_write_file(inode_num, data, offset)
        } else {
            self.ext2_write_file(inode_num, data, offset)
        };

        match result {
            Ok(n) => {
                let group = ((inode_num - 1) / self.inodes_per_group) as usize;
                self.journal_group_metadata(group)?;
                // commit deferred to pdflush
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
        self.journal_inode_metadata(inode_num)?;

        match self.ext2_append_file(inode_num, data) {
            Ok(n) => {
                let group = ((inode_num - 1) / self.inodes_per_group) as usize;
                self.journal_group_metadata(group)?;
                // commit deferred to pdflush
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
        self.journal_inode_blocks(parent_ino)?;
        self.journal_inode_metadata(target_ino)?;
        self.journal_inode_metadata(parent_ino)?;
        let group = ((target_ino - 1) / self.inodes_per_group) as usize;
        self.journal_group_metadata(group)?;
        let parent_group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        if parent_group != group { self.journal_group_metadata(parent_group)?; }
        self.ext3_journal_revoke_inode_blocks(target_ino)?;

        let result = if use_extents {
            self.ext4_delete_file(parent_ino, name)
        } else {
            self.ext2_delete_file(parent_ino, name)
        };

        match result {
            Ok(()) => {
                // commit deferred to pdflush
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
        self.journal_inode_blocks(parent_ino)?;
        self.journal_inode_metadata(target_ino)?;
        self.journal_inode_metadata(parent_ino)?;
        let group = ((target_ino - 1) / self.inodes_per_group) as usize;
        self.journal_group_metadata(group)?;
        let parent_group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        if parent_group != group { self.journal_group_metadata(parent_group)?; }
        self.ext3_journal_revoke_inode_blocks(target_ino)?;

        let result = if use_extents {
            self.ext4_delete_dir(parent_ino, name)
        } else {
            self.ext2_delete_dir(parent_ino, name)
        };

        match result {
            Ok(()) => {
                // commit deferred to pdflush
                Ok(())
            }
            Err(e) => { self.ext3_abort_txn(); Err(e) }
        }
    }

    pub fn ext3_create_symlink(
        &mut self,
        parent_ino: u32,
        name: &str,
        target: &str,
    ) -> Result<u32, FsError> {
        if !self.journal_active {
            return self.ext2_create_symlink(parent_ino, name, target);
        }

        self.ext3_begin_txn()?;
        let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        self.journal_inode_blocks(parent_ino)?;
        self.journal_inode_metadata(parent_ino)?;
        self.journal_group_metadata(group)?;

        let result = self.ext2_create_symlink(parent_ino, name, target);

        match result {
            Ok(new_ino) => {
                self.journal_inode_metadata(new_ino)?;
                let new_group = ((new_ino - 1) / self.inodes_per_group) as usize;
                if new_group != group {
                    self.journal_group_metadata(new_group)?;
                }
                // commit deferred to pdflush
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
        let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        self.journal_inode_blocks(parent_ino)?;
        self.journal_inode_metadata(parent_ino)?;
        self.journal_group_metadata(group)?;

        let target_ino = match self.ext2_lookup_in_dir(parent_ino, old_name)? {
            Some(ino) => ino,
            None => {
                self.ext3_abort_txn();
                return Err(FsError::NotFound);
            }
        };
        self.journal_inode_metadata(target_ino)?;

        let result = self.ext2_rename(parent_ino, old_name, new_name);

        match result {
            Ok(()) => {
                // commit deferred to pdflush
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
        let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
        self.journal_inode_blocks(parent_ino)?;
        self.journal_inode_metadata(parent_ino)?;
        self.journal_group_metadata(group)?;
        self.journal_inode_metadata(target_ino)?;
        let target_group = ((target_ino - 1) / self.inodes_per_group) as usize;
        if target_group != group {
            self.journal_group_metadata(target_group)?;
        }

        let result = self.ext2_hardlink(parent_ino, name, target_ino);

        match result {
            Ok(()) => {
                // commit deferred to pdflush
                Ok(())
            }
            Err(e) => {
                self.ext3_abort_txn();
                Err(e)
            }
        }
    }

    pub fn ext3_truncate(&mut self, inode_num: u32) -> Result<(), FsError> {
        let use_extents = {
            let inode = self.read_inode(inode_num)?;
            inode.uses_extents()
        };

        if !self.journal_active {
            return if use_extents {
                self.ext4_truncate(inode_num)
            } else {
                self.ext2_truncate(inode_num)
            };
        }

        self.ext3_begin_txn()?;
        self.journal_inode_metadata(inode_num)?;
        let group = ((inode_num - 1) / self.inodes_per_group) as usize;
        self.journal_group_metadata(group)?;
        self.ext3_journal_revoke_inode_blocks(inode_num)?;

        let result = if use_extents {
            self.ext4_truncate(inode_num)
        } else {
            self.ext2_truncate(inode_num)
        };

        match result {
            Ok(()) => {
                // commit deferred to pdflush
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
                self.journal_inode_metadata(ino)?;
                let group = ((ino - 1) / self.inodes_per_group) as usize;
                self.journal_group_metadata(group)?;

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
                        self.journal_inode_metadata(ino)?;
                        self.journal_group_metadata(group)?;
                        // commit deferred to pdflush
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
                let group = ((parent_ino - 1) / self.inodes_per_group) as usize;
                self.journal_inode_blocks(parent_ino)?;
                self.journal_inode_metadata(parent_ino)?;
                self.journal_group_metadata(group)?;

                let create_result = if use_extents {
                    self.ext4_create_file(parent_ino, name, mode)
                } else {
                    self.ext2_create_file(parent_ino, name, mode)
                };

                match create_result {
                    Ok(new_ino) => {
                        let new_group = ((new_ino - 1) / self.inodes_per_group) as usize;
                        if new_group != group { self.journal_group_metadata(new_group)?; }
                        let write_result = if use_extents {
                            self.ext4_write_file(new_ino, data, 0)
                        } else {
                            self.ext2_write_file(new_ino, data, 0)
                        };
                        match write_result {
                            Ok(_) => {
                                self.journal_inode_metadata(new_ino)?;
                                self.journal_group_metadata(new_group)?;
                                // commit deferred to pdflush
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
