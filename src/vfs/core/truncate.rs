// File truncation and page eviction

use super::MikuVFS;
use crate::vfs::address_space::AddressSpace;
use crate::vfs::types::*;

impl MikuVFS {
    pub fn free_file_pages(&mut self, id: usize) {
        let mut to_free = [INVALID_ID; DIRECT_BLOCKS];
        let mut free_count = 0;
        for (_, pid) in self.nodes[id].addr_space.iter_pages() {
            if free_count < DIRECT_BLOCKS {
                to_free[free_count] = pid;
                free_count += 1;
            }
        }
        for i in 0..free_count {
            if to_free[i] != INVALID_ID {
                self.page_cache.free_page(to_free[i]);
            }
        }
    }

    pub(super) fn truncate_file(&mut self, id: usize) {
        if self.nodes[id].is_ext_backed() {
            let ino = self.nodes[id].ext2_ino;
            let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.ext3_truncate(ino));
        }
        self.free_file_pages(id);
        self.nodes[id].size = 0;
        self.nodes[id].addr_space = AddressSpace::new();
        self.nodes[id].touch_mtime(self.now());
    }

    pub fn truncate_to(&mut self, id: usize, new_size: u64) {
        let old_size = self.nodes[id].size;
        if new_size >= old_size {
            self.nodes[id].size = new_size;
            return;
        }

        if self.nodes[id].is_ext_backed() && new_size == 0 {
            let ino = self.nodes[id].ext2_ino;
            let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.ext3_truncate(ino));
        }

        let keep_pages = if new_size == 0 {
            0
        } else {
            ((new_size as usize - 1) / PAGE_SIZE) + 1
        };

        let mut to_free = [INVALID_ID; DIRECT_BLOCKS];
        let mut free_count = 0;
        for page_num in keep_pages..DIRECT_BLOCKS {
            if let Some(pid) = self.nodes[id].addr_space.get_page(page_num) {
                to_free[free_count] = pid;
                free_count += 1;
                self.nodes[id].addr_space.clear_page(page_num);
            }
        }
        for i in 0..free_count {
            self.page_cache.free_page(to_free[i]);
        }

        if new_size > 0 {
            let last_page = (new_size as usize - 1) / PAGE_SIZE;
            let zero_from = new_size as usize % PAGE_SIZE;
            if zero_from > 0 {
                if let Some(pid) = self.nodes[id].addr_space.get_page(last_page) {
                    if let Some(data) = self.page_cache.get_page_data_mut(pid) {
                        for i in zero_from..PAGE_SIZE {
                            data[i] = 0;
                        }
                        self.page_cache.mark_dirty(pid);
                    }
                }
            }
        }

        self.nodes[id].size = new_size;
        let ts = self.now();
        self.nodes[id].touch_mtime(ts);
        self.nodes[id].touch_ctime(ts);
    }
}
