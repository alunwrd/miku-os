// Which page of the cache holds which page of a tmpfs file
//
// Two levels of index, both cache pages themselves: the root holds the numbers
// of up to 1024 index pages, each of those the numbers of up to 1024 data
// pages, 4 GiB of file in 4 KiB pages. A hole is a zero entry that reads back
// as zeroes, and an index page exists only once something in its 4 MiB has been
// written.
//
// The struct is `Copy` and two words wide: a vnode is copied by value.

use alloc::vec::Vec;

use crate::fs::vfs::pages::PageCache;
use crate::fs::vfs::types::*;

/// Entries in one index page
const FANOUT: usize = PAGE_SIZE / core::mem::size_of::<PageId>();

#[derive(Debug, Clone, Copy)]
pub struct AddressSpace {
    /// The root index page, or `INVALID_PAGE` for a file with no data
    pub root: PageId,
    pub nr_pages: u32,
}

/// An index page seen as its entries. An entry of `INVALID_PAGE` is a hole
fn entries(pc: &mut PageCache, pid: PageId) -> Option<&mut [PageId; FANOUT]> {
    let data = pc.get_page_data_mut(pid)?;
    Some(unsafe { &mut *(data.as_mut_ptr() as *mut [PageId; FANOUT]) })
}

/// `entries` for a reader: no counter moved (`PageCache::page_data`)
fn entries_ro(pc: &PageCache, pid: PageId) -> Option<&[PageId; FANOUT]> {
    let data = pc.page_data(pid)?;
    Some(unsafe { &*(data.as_ptr() as *const [PageId; FANOUT]) })
}

/// A fresh index page, every entry a hole
fn new_index(pc: &mut PageCache) -> VfsResult<PageId> {
    let pid = pc.alloc_page()?;
    if let Some(e) = entries(pc, pid) {
        e.fill(INVALID_PAGE);
    }
    Ok(pid)
}

impl AddressSpace {
    pub const fn new() -> Self {
        Self { root: INVALID_PAGE, nr_pages: 0 }
    }

    fn split(page_num: usize) -> Option<(usize, usize)> {
        if page_num >= Self::max_pages() {
            return None;
        }
        Some((page_num / FANOUT, page_num % FANOUT))
    }

    pub fn get_page(&self, pc: &mut PageCache, page_num: usize) -> Option<PageId> {
        let (hi, lo) = Self::split(page_num)?;
        if self.root == INVALID_PAGE {
            return None;
        }
        let leaf = entries(pc, self.root)?[hi];
        if leaf == INVALID_PAGE {
            return None;
        }
        let pid = entries(pc, leaf)?[lo];
        (pid != INVALID_PAGE).then_some(pid)
    }

    /// `get_page` from `&PageCache`, for a read under the VFS lock held to
    /// read (`MikuVFS::read_as`)
    pub fn get_page_ro(&self, pc: &PageCache, page_num: usize) -> Option<PageId> {
        let (hi, lo) = Self::split(page_num)?;
        if self.root == INVALID_PAGE {
            return None;
        }
        let leaf = entries_ro(pc, self.root)?[hi];
        if leaf == INVALID_PAGE {
            return None;
        }
        let pid = entries_ro(pc, leaf)?[lo];
        (pid != INVALID_PAGE).then_some(pid)
    }

    /// Record `page_id` as page `page_num`, making the index pages on the way
    pub fn set_page(&mut self, pc: &mut PageCache, page_num: usize, page_id: PageId) -> VfsResult<()> {
        let (hi, lo) = Self::split(page_num).ok_or(VfsError::FileTooLarge)?;
        if self.root == INVALID_PAGE {
            self.root = new_index(pc)?;
        }
        let mut leaf = entries(pc, self.root).ok_or(VfsError::IoError)?[hi];
        if leaf == INVALID_PAGE {
            leaf = new_index(pc)?;
            entries(pc, self.root).ok_or(VfsError::IoError)?[hi] = leaf;
        }
        let slot = &mut entries(pc, leaf).ok_or(VfsError::IoError)?[lo];
        if *slot == INVALID_PAGE {
            self.nr_pages += 1;
        }
        *slot = page_id;
        Ok(())
    }

    /// Take every data page at `keep` and after it out of the file, and hand
    /// them back for the caller to free. Index pages left empty are freed here;
    /// the root goes when nothing is kept
    pub fn truncate(&mut self, pc: &mut PageCache, keep: usize) -> Vec<PageId> {
        let mut gone = Vec::new();
        if self.root == INVALID_PAGE {
            return gone;
        }
        for hi in 0..FANOUT {
            let Some(leaf) = entries(pc, self.root).map(|e| e[hi]) else { break };
            if leaf == INVALID_PAGE {
                continue;
            }
            let first = hi * FANOUT;
            if first + FANOUT <= keep {
                continue;
            }
            let from = keep.saturating_sub(first);
            let mut still_used = false;
            if let Some(e) = entries(pc, leaf) {
                for (lo, slot) in e.iter_mut().enumerate() {
                    if *slot == INVALID_PAGE {
                        continue;
                    }
                    if lo >= from {
                        gone.push(*slot);
                        *slot = INVALID_PAGE;
                    } else {
                        still_used = true;
                    }
                }
            }
            if !still_used {
                pc.free_page(leaf);
                if let Some(e) = entries(pc, self.root) {
                    e[hi] = INVALID_PAGE;
                }
            }
        }
        self.nr_pages = self.nr_pages.saturating_sub(gone.len() as u32);
        if keep == 0 {
            pc.free_page(self.root);
            self.root = INVALID_PAGE;
            self.nr_pages = 0;
        }
        gone
    }

    /// One past the last page held, 0 for none
    pub fn end(&self, pc: &PageCache) -> usize {
        let Some(root) = entries_ro(pc, self.root) else { return 0 };
        for hi in (0..FANOUT).rev() {
            if root[hi] == INVALID_PAGE {
                continue;
            }
            if let Some(leaf) = entries_ro(pc, root[hi]) {
                if let Some(lo) = (0..FANOUT).rev().find(|&lo| leaf[lo] != INVALID_PAGE) {
                    return hi * FANOUT + lo + 1;
                }
            }
        }
        0
    }

    pub const fn max_pages() -> usize {
        FANOUT * FANOUT
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
}

/// Pages given back a part at a time (`Detached::free_part`)
pub const FREE_PART: usize = 1024;

/// A file's pages taken off it whole under the lock guarding their cache, a
/// move of two words, and given back `FREE_PART` at a time after that lock is
/// released. A 64 MiB file's 16384 pages freed in place held the VFS lock 3.7
/// ms
pub struct Detached {
    space: AddressSpace,
    end: usize,
}

impl Detached {
    pub fn new(space: AddressSpace, pc: &PageCache) -> Self {
        Self { space, end: space.end(pc) }
    }

    /// Up to `n` of its pages, and the index pages that leaves empty, back
    /// to `pc`. True when none is left
    pub fn free_part(&mut self, pc: &mut PageCache, n: usize) -> bool {
        let keep = self.end.saturating_sub(n);
        for pid in self.space.truncate(pc, keep) {
            pc.free_page(pid);
        }
        self.end = keep;
        keep == 0
    }
}
