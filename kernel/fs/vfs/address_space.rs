use crate::fs::vfs::types::*;

#[derive(Debug, Clone, Copy)]
pub struct AddressSpace {
    pub direct: [PageId; DIRECT_BLOCKS],
    pub indirect: PageId,
    pub nr_pages: u32,
}

impl AddressSpace {
    pub const fn new() -> Self {
        Self {
            direct: [INVALID_ID; DIRECT_BLOCKS],
            indirect: INVALID_ID,
            nr_pages: 0,
        }
    }

    pub fn get_page(&self, page_num: usize) -> Option<PageId> {
        if page_num < DIRECT_BLOCKS {
            let pid = self.direct[page_num];
            if pid != INVALID_ID {
                return Some(pid);
            }
        }
        None
    }

    pub fn set_page(&mut self, page_num: usize, page_id: PageId) -> VfsResult<()> {
        if page_num < DIRECT_BLOCKS {
            if self.direct[page_num] == INVALID_ID {
                self.nr_pages += 1;
            }
            self.direct[page_num] = page_id;
            Ok(())
        } else {
            Err(VfsError::FileTooLarge)
        }
    }

    pub fn clear_page(&mut self, page_num: usize) {
        if page_num < DIRECT_BLOCKS && self.direct[page_num] != INVALID_ID {
            self.direct[page_num] = INVALID_ID;
            if self.nr_pages > 0 {
                self.nr_pages -= 1;
            }
        }
    }

    pub fn truncate_to(&mut self, new_page_count: usize) -> TruncateIter<'_> {
        TruncateIter {
            space: self,
            pos: new_page_count,
        }
    }

    pub const fn max_pages() -> usize {
        DIRECT_BLOCKS
    }
    pub const fn max_size() -> u64 {
        (Self::max_pages() * PAGE_SIZE) as u64
    }

    pub fn pages_for_size(size: u64) -> usize {
        if size == 0 {
            return 0;
        }
        ((size as usize) + PAGE_SIZE - 1) / PAGE_SIZE
    }

    pub fn iter_pages(&self) -> PageIter<'_> {
        PageIter {
            space: self,
            pos: 0,
        }
    }

    pub fn used_bytes(&self) -> u64 {
        self.nr_pages as u64 * PAGE_SIZE as u64
    }
}

pub struct PageIter<'a> {
    space: &'a AddressSpace,
    pos: usize,
}

impl<'a> Iterator for PageIter<'a> {
    type Item = (usize, PageId);

    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < DIRECT_BLOCKS {
            let idx = self.pos;
            self.pos += 1;
            let pid = self.space.direct[idx];
            if pid != INVALID_ID {
                return Some((idx, pid));
            }
        }
        None
    }
}

pub struct TruncateIter<'a> {
    space: &'a mut AddressSpace,
    pos: usize,
}

impl<'a> Iterator for TruncateIter<'a> {
    type Item = PageId;

    fn next(&mut self) -> Option<PageId> {
        while self.pos < DIRECT_BLOCKS {
            let idx = self.pos;
            self.pos += 1;
            let pid = self.space.direct[idx];
            if pid != INVALID_ID {
                self.space.direct[idx] = INVALID_ID;
                if self.space.nr_pages > 0 {
                    self.space.nr_pages -= 1;
                }
                return Some(pid);
            }
        }
        None
    }
}
