use crate::fs::ext::structs::*;
use crate::fs::ext::{FsError, MikuFS};


pub struct Ext4UpgradeReport {
    pub already_ext4: bool,
    pub had_journal: bool,
    pub inode_size: u32,
    pub inode_size_warning: bool,
    pub set_extents:     bool,
    pub set_filetype:    bool,
    pub set_sparse_super:bool,
    pub set_large_file:  bool,
    pub set_dir_nlink:   bool,
    pub set_extra_isize: bool,
    pub set_dir_index:   bool,
    pub set_rev_level:   bool,
}

impl Ext4UpgradeReport {
    fn new() -> Self {
        Self {
            already_ext4: false, had_journal: false,
            inode_size: 0, inode_size_warning: false,
            set_extents: false, set_filetype: false,
            set_sparse_super: false, set_large_file: false,
            set_dir_nlink: false, set_extra_isize: false,
            set_dir_index: false, set_rev_level: false,
        }
    }

    pub fn any_new(&self) -> bool {
        self.set_extents || self.set_filetype || self.set_sparse_super ||
        self.set_large_file || self.set_dir_nlink || self.set_extra_isize ||
        self.set_dir_index || self.set_rev_level
    }
}


impl MikuFS {
    pub fn ext4_upgrade(&mut self) -> Result<Ext4UpgradeReport, FsError> {
        let mut rep = Ext4UpgradeReport::new();

        rep.had_journal        = self.superblock.has_journal();
        rep.inode_size         = self.inode_size();
        rep.already_ext4       = self.superblock.is_ext4();
        rep.inode_size_warning = rep.inode_size < 256;

        let mut incompat  = self.superblock.feature_incompat();
        let mut ro_compat = self.superblock.feature_ro_compat();
        let mut compat    = self.superblock.feature_compat();

        if incompat & FEATURE_INCOMPAT_EXTENTS == 0 {
            incompat |= FEATURE_INCOMPAT_EXTENTS;
            rep.set_extents = true;
        }

        if incompat & FEATURE_INCOMPAT_FILETYPE == 0 {
            incompat |= FEATURE_INCOMPAT_FILETYPE;
            rep.set_filetype = true;
        }

        if ro_compat & FEATURE_RO_COMPAT_SPARSE_SUPER == 0 {
            ro_compat |= FEATURE_RO_COMPAT_SPARSE_SUPER;
            rep.set_sparse_super = true;
        }

        if ro_compat & FEATURE_RO_COMPAT_LARGE_FILE == 0 {
            ro_compat |= FEATURE_RO_COMPAT_LARGE_FILE;
            rep.set_large_file = true;
        }

        if ro_compat & FEATURE_RO_COMPAT_DIR_NLINK == 0 {
            ro_compat |= FEATURE_RO_COMPAT_DIR_NLINK;
            rep.set_dir_nlink = true;
        }

        if ro_compat & FEATURE_RO_COMPAT_EXTRA_ISIZE == 0 && rep.inode_size >= 256 {
            ro_compat |= FEATURE_RO_COMPAT_EXTRA_ISIZE;
            rep.set_extra_isize = true;
            let extra = ((rep.inode_size - 128) as u16).min(28);
            self.superblock.write_u16(276, extra); 
            self.superblock.write_u16(278, extra); 
        }

        if compat & FEATURE_COMPAT_DIR_INDEX == 0 {
            compat |= FEATURE_COMPAT_DIR_INDEX;
            rep.set_dir_index = true;
        }

        if self.superblock.rev_level() < 1 {
            self.superblock.write_u32(76, 1);
            self.superblock.write_u32(84, 11);
            self.superblock.write_u16(88, rep.inode_size as u16);
            rep.set_rev_level = true;
        }

        self.superblock.write_u32(92, compat);
        self.superblock.write_u32(96, incompat);
        self.superblock.write_u32(100, ro_compat);
        
        let now = self.get_timestamp();
        self.superblock.write_u32(48, now);

        self.flush_superblock()?;
        Ok(rep)
    }

    pub fn ext4_features_complete(&self) -> bool {
        let required_i = FEATURE_INCOMPAT_EXTENTS | FEATURE_INCOMPAT_FILETYPE;
        let required_r = FEATURE_RO_COMPAT_SPARSE_SUPER | FEATURE_RO_COMPAT_LARGE_FILE
                       | FEATURE_RO_COMPAT_DIR_NLINK;
        (self.superblock.feature_incompat() & required_i) == required_i
            && (self.superblock.feature_ro_compat() & required_r) == required_r
            && self.superblock.rev_level() >= 1
    }

    pub fn ext4_missing_features(&self) -> (u32, u32) {
        let want_i = FEATURE_INCOMPAT_EXTENTS | FEATURE_INCOMPAT_FILETYPE;
        let want_r = FEATURE_RO_COMPAT_SPARSE_SUPER | FEATURE_RO_COMPAT_LARGE_FILE
                   | FEATURE_RO_COMPAT_DIR_NLINK;
        let mi = want_i & !self.superblock.feature_incompat();
        let mr = want_r & !self.superblock.feature_ro_compat();
        (mi, mr)
    }
}
