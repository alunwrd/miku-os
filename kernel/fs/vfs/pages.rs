// The pages behind tmpfs: physical frames, one per page of a file
//
// tmpfs is the vnode table itself. A page is a frame from `pmm`, 4 KiB, taken
// when a file first writes it and returned when the file lets go of it. The
// ceiling is a share of the machine's memory, taken the first time a page is
// asked for, when the allocator knows how much there is.
//
// A page is the same frame everywhere it appears. A file mapped `MAP_SHARED`
// maps this frame (`mm::mmap::handle_file_fault`), so a store through one
// mapping is what `read` and every other mapping see, as SQLite's `-shm` index
// needs. The frame is reference-counted through `pmm`'s CoW counts: the cache
// holds one reference, each mapping another, and the frame goes back to the
// allocator when the last lets go, so a file truncated under a mapping keeps
// the page under it.
//
// There is no eviction: a tmpfs page has nowhere to be written back to.
//
// The page cache for files on a disk is a second one of these, with a limit of
// its own (`filecache`): the same frames and the same index, and pages that can
// always be read again, so it can evict.

use alloc::vec::Vec;

use crate::fs::vfs::types::*;

pub struct PageCache {
    /// How many pages this cache may hold, asked when it allocates
    limit: fn() -> usize,
    /// PageId -> the frame's physical address; 0 for a free slot
    frames: Vec<u64>,
    /// Slots in `frames` that are free, reused before the table grows
    free_ids: Vec<PageId>,
    used: usize,
    pub total_writes: u64,
    pub total_reads: u64,
}

/// The share of physical memory tmpfs may fill: a quarter
const MEMORY_SHARE_DIVISOR: usize = 4;

/// Pages tmpfs may hold at once: its share of the machine
pub fn tmpfs_limit() -> usize {
    let (_, total) = crate::mm::pmm::stats();
    (total / MEMORY_SHARE_DIVISOR).max(256)
}

impl PageCache {
    /// tmpfs's: a quarter of the machine
    pub const fn new() -> Self {
        Self::with_limit(tmpfs_limit)
    }

    pub const fn with_limit(limit: fn() -> usize) -> Self {
        Self {
            limit,
            frames: Vec::new(),
            free_ids: Vec::new(),
            used: 0,
            total_writes: 0,
            total_reads: 0,
        }
    }

    fn limit(&self) -> usize {
        (self.limit)()
    }

    /// A zeroed page, or `NoSpace` when tmpfs has its share or the machine
    /// has no frame to give
    pub fn alloc_page(&mut self) -> VfsResult<PageId> {
        if self.used >= self.limit() {
            return Err(VfsError::NoSpace);
        }
        let phys = crate::mm::pmm::alloc_frame().ok_or(VfsError::NoSpace)?;
        unsafe {
            core::ptr::write_bytes(Self::ptr(phys), 0, PAGE_SIZE);
        }
        self.adopt_frame(phys).inspect_err(|_| crate::mm::pmm::free_frame(phys))
    }

    /// A frame somebody already filled, as a page of this cache: what a read
    /// done outside the VFS lock hands back (`filecache`). The cache owns it
    /// from here; on `Err` the caller still does
    pub fn adopt(&mut self, phys: u64) -> VfsResult<PageId> {
        if self.used >= self.limit() {
            return Err(VfsError::NoSpace);
        }
        self.adopt_frame(phys)
    }

    fn adopt_frame(&mut self, phys: u64) -> VfsResult<PageId> {
        let pid = match self.free_ids.pop() {
            Some(pid) => {
                self.frames[pid as usize] = phys;
                pid
            }
            None => {
                if self.frames.len() >= INVALID_PAGE as usize {
                    return Err(VfsError::NoSpace);
                }
                self.frames.push(phys);
                (self.frames.len() - 1) as PageId
            }
        };
        self.used += 1;
        Ok(pid)
    }

    /// Release a page. The frame goes back when no mapping holds it
    /// (`pmm::free_frame_cow`)
    pub fn free_page(&mut self, page_id: PageId) {
        let Some(phys) = self.phys(page_id) else { return };
        self.frames[page_id as usize] = 0;
        self.free_ids.push(page_id);
        self.used -= 1;
        crate::mm::pmm::free_frame_cow(phys);
    }

    /// The frame behind a page, for a mapping of it
    pub fn phys(&self, page_id: PageId) -> Option<u64> {
        match self.frames.get(page_id as usize) {
            Some(&phys) if phys != 0 => Some(phys),
            _ => None,
        }
    }

    fn ptr(phys: u64) -> *mut u8 {
        (phys + crate::arch::x86_64::grub::hhdm()) as *mut u8
    }

    pub fn get_page_data(&mut self, page_id: PageId) -> Option<&[u8; PAGE_SIZE]> {
        let phys = self.phys(page_id)?;
        self.total_reads += 1;
        Some(unsafe { &*(Self::ptr(phys) as *const [u8; PAGE_SIZE]) })
    }

    /// `get_page_data` without the count, from `&self`: for a read under
    /// the VFS lock held to read
    pub fn page_data(&self, page_id: PageId) -> Option<&[u8; PAGE_SIZE]> {
        let phys = self.phys(page_id)?;
        Some(unsafe { &*(Self::ptr(phys) as *const [u8; PAGE_SIZE]) })
    }

    pub fn get_page_data_mut(&mut self, page_id: PageId) -> Option<&mut [u8; PAGE_SIZE]> {
        let phys = self.phys(page_id)?;
        self.total_writes += 1;
        Some(unsafe { &mut *(Self::ptr(phys) as *mut [u8; PAGE_SIZE]) })
    }

    pub fn used_pages(&self) -> usize {
        self.used
    }

    pub fn free_pages(&self) -> usize {
        self.limit().saturating_sub(self.used)
    }

    pub fn total_capacity(&self) -> usize {
        self.limit()
    }
}
