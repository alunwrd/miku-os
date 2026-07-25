use crate::fs::vfs::types::VfsError;

/// Fixed-capacity index allocator.
///
/// The previous version declared itself generic over `N` but stored a
/// `[u16; 64]` free list and a single `u64` of occupancy bits, capping
/// `MAX_ITEMS` at `min(N, 64)`. Nothing said so at the call site, so the
/// page cache - a `Slab<1024>` sitting next to 1024 `CachedPage` entries,
/// half a megabyte of .bss - could only ever hand out 64 pages. The cache
/// ran at a sixteenth of its declared size while paying full price in
/// memory, and no error was ever reported.
pub struct Slab<const N: usize> {
    free_stack: [u16; N],
    free_top: u32,
    total_allocated: u32,
    active: [bool; N],
}

impl<const N: usize> Slab<N> {
    pub const MAX_ITEMS: usize = N;

    pub const fn new() -> Self {
        let mut free_stack = [0u16; N];
        let mut i = 0;
        while i < N {
            free_stack[i] = i as u16;
            i += 1;
        }
        Self {
            free_stack,
            free_top: N as u32,
            total_allocated: 0,
            active: [false; N],
        }
    }

    pub fn alloc(&mut self) -> Result<usize, VfsError> {
        if self.free_top == 0 {
            return Err(VfsError::NoSpace);
        }
        self.free_top -= 1;
        let idx = self.free_stack[self.free_top as usize] as usize;
        self.set_active(idx, true);
        self.total_allocated += 1;
        Ok(idx)
    }

    pub fn free(&mut self, idx: usize) {
        if idx < N && self.is_active(idx) {
            self.set_active(idx, false);
            if (self.free_top as usize) < N {
                self.free_stack[self.free_top as usize] = idx as u16;
                self.free_top += 1;
            }
            if self.total_allocated > 0 {
                self.total_allocated -= 1;
            }
        }
    }

    #[inline]
    pub fn is_active(&self, idx: usize) -> bool {
        idx < N && self.active[idx]
    }

    #[inline]
    fn set_active(&mut self, idx: usize, active: bool) {
        if idx < N {
            self.active[idx] = active;
        }
    }

    pub fn count(&self) -> usize {
        self.total_allocated as usize
    }
    pub fn free_count(&self) -> usize {
        self.free_top as usize
    }
    pub fn capacity(&self) -> usize {
        N
    }

    pub fn iter_active(&self) -> SlabIter<'_, N> {
        SlabIter { slab: self, pos: 0 }
    }
}

pub struct SlabIter<'a, const N: usize> {
    slab: &'a Slab<N>,
    pos: usize,
}

impl<'a, const N: usize> Iterator for SlabIter<'a, N> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        while self.pos < N {
            let idx = self.pos;
            self.pos += 1;
            if self.slab.is_active(idx) {
                return Some(idx);
            }
        }
        None
    }
}
