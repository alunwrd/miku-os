// The allocator behind ring 3's heap (`dev/kernel-core.md`)
//
// Used by `heap.rs` in core_miku.so: size bins, boundary tags, and slabs, over
// memory from a `Source`. Takes no lock and makes no system call, so `mmap`
// feeds it in a program and the host tests feed it in `lib/runtime/libmiku/hosttest`.

pub const ALLOC_ALIGN: usize = 16;
pub const BLOCK_HDR: usize = 32;
const SPLIT_MIN: usize = BLOCK_HDR + ALLOC_ALIGN;
const PAGE: usize = 4096;

#[repr(C)]
struct Block {
    size: usize,
    flags: usize,
    prev_free: *mut Block,
    next_free: *mut Block,
}

const FLAG_USED: usize = 1;
/// A block with its own memory from the `Source`, returned whole when freed
const FLAG_MMAP: usize = 2;

/// An over-aligned block is recorded in front of itself.
///
/// `memalign` leaves room for a whole `Block` header before the returned
/// pointer, writes one with `FLAG_ALIGNED`, and stores the distance back to the
/// real block in `size`. `free` reads the header and takes the other branch. No
/// ceiling, lock, or search.
///
/// `FLAG_ALIGNED` cannot appear on an ordinary block; only this allocator
/// writes `flags`.
const FLAG_ALIGNED: usize = 4;

/// The block before this one in its slab is free, and its size is the last word
/// before this header: the boundary tag of every free block. Lets `free` merge
/// backward (`dev/std-architecture.md`, "The heap's free list")
const FLAG_PREV_FREE: usize = 8;

/// Below this, one bin per size, 16 bytes apart
const SMALL_LIMIT: usize = 1024;
const SMALL_BINS: usize = SMALL_LIMIT / ALLOC_ALIGN;
/// Above it, `1 << SUB_BITS` bins to every power of two
const SUB_BITS: u32 = 2;
/// Enough for the biggest slab; a bigger block shares the last bin
const NBINS: usize = 128;

/// Bin of a free block of `size` bytes, header included. Larger sizes never
/// land in a lower bin
fn bin_of(size: usize) -> usize {
    if size < SMALL_LIMIT {
        return size / ALLOC_ALIGN;
    }
    let log = usize::BITS - 1 - size.leading_zeros();
    let sub = (size >> (log - SUB_BITS)) & ((1 << SUB_BITS) - 1);
    (SMALL_BINS + (((log - SMALL_LIMIT.trailing_zeros()) as usize) << SUB_BITS) + sub).min(NBINS - 1)
}

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

impl Block {
    fn data_size(&self) -> usize { self.size.saturating_sub(BLOCK_HDR) }
    fn data_ptr(&self) -> *mut u8 { (self as *const Block as *mut u8).wrapping_add(BLOCK_HDR) }
    fn is_free(&self) -> bool { self.flags & FLAG_USED == 0 }
    fn is_mmap(&self) -> bool { self.flags & FLAG_MMAP != 0 }

    fn next_adjacent(&self) -> *mut Block {
        let addr = self as *const Block as usize;
        (addr + self.size) as *mut Block
    }
}

/// Where a heap's memory comes from
pub trait Source {
    /// `bytes` of unused memory, a whole number of pages aligned to one, or
    /// null.
    ///
    /// # Safety
    /// What is returned must stay readable and writable until returned
    /// (`Large`), or forever for a slab
    unsafe fn grab(&mut self, bytes: usize) -> *mut u8;
}

/// A freed block with its own memory: the caller returns it to its `Source`
/// after the lock is down
pub struct Large {
    pub ptr: *mut u8,
    pub bytes: usize,
}

/// Each block's neighbor is in its own slab.
///
/// A freed block merges with the block after it (`next_adjacent`, the header
/// `size` bytes on). So a slab is one free block when made, ending in a header
/// marked used that is never freed: merging stops there, and every header a
/// merge reads was written here.
///
/// Free blocks are in bins by size (`bin_of`), with a bit per bin saying if it
/// has any. No two free blocks are adjacent, since `free` merges with both
/// neighbors
pub struct Heap<S> {
    bins: [*mut Block; NBINS],
    /// Which bins have a block in them
    full: [u64; NBINS / 64],
    source: S,
    /// How much a slab is at least
    slab: usize,
    /// From how many bytes a request gets memory of its own
    large: usize,
    /// Bytes in blocks somebody holds, headers included
    used: usize,
    /// Bytes the heap has from its `Source`, slabs and large blocks
    held: usize,
}

unsafe impl<S: Send> Send for Heap<S> {}

impl<S: Source> Heap<S> {
    pub const fn new(source: S, slab: usize, large: usize) -> Self {
        Self {
            bins: [core::ptr::null_mut(); NBINS],
            full: [0; NBINS / 64],
            source,
            slab,
            large,
            used: 0,
            held: 0,
        }
    }

    pub fn source(&mut self) -> &mut S {
        &mut self.source
    }

    /// Bytes in use, headers included
    pub fn used(&self) -> usize {
        self.used
    }

    /// Bytes taken from the `Source` and not returned
    pub fn held(&self) -> usize {
        self.held
    }

    unsafe fn bin_insert(&mut self, block: *mut Block) {
        let i = bin_of((*block).size);
        let head = self.bins[i];
        (*block).prev_free = core::ptr::null_mut();
        (*block).next_free = head;
        if !head.is_null() {
            (*head).prev_free = block;
        }
        self.bins[i] = block;
        self.full[i / 64] |= 1 << (i % 64);
    }

    /// Removes a block from its bin, which its size names, so the size must not
    /// have changed
    unsafe fn bin_remove(&mut self, block: *mut Block) {
        let i = bin_of((*block).size);
        let prev = (*block).prev_free;
        let next = (*block).next_free;
        if !prev.is_null() {
            (*prev).next_free = next;
        } else {
            self.bins[i] = next;
            if next.is_null() {
                self.full[i / 64] &= !(1 << (i % 64));
            }
        }
        if !next.is_null() {
            (*next).prev_free = prev;
        }
        (*block).prev_free = core::ptr::null_mut();
        (*block).next_free = core::ptr::null_mut();
    }

    /// The first bin from `from` up with a block in it
    fn next_full(&self, from: usize) -> Option<usize> {
        let mut i = from;
        while i < NBINS {
            let word = self.full[i / 64] >> (i % 64);
            if word != 0 {
                return Some(i + word.trailing_zeros() as usize);
            }
            i = (i / 64 + 1) * 64;
        }
        None
    }

    /// A free block of at least `needed`, out of its bin: the first big enough
    /// in the request's bin (every one, below `SMALL_LIMIT`), then the head of
    /// the next non-empty bin
    unsafe fn find_free(&mut self, needed: usize) -> *mut Block {
        let first = bin_of(needed);
        let mut cur = self.bins[first];
        while !cur.is_null() {
            if (*cur).size >= needed {
                self.bin_remove(cur);
                return cur;
            }
            cur = (*cur).next_free;
        }
        match self.next_full(first + 1) {
            Some(i) => {
                let block = self.bins[i];
                self.bin_remove(block);
                block
            }
            None => core::ptr::null_mut(),
        }
    }

    /// `block` is now free: its size is written at its end, the next block is
    /// told, and it enters its bin
    unsafe fn release(&mut self, block: *mut Block) {
        (*block).flags &= !FLAG_USED;
        let end = (block as *mut u8).add((*block).size);
        *(end.sub(core::mem::size_of::<usize>()) as *mut usize) = (*block).size;
        (*(end as *mut Block)).flags |= FLAG_PREV_FREE;
        self.bin_insert(block);
    }

    /// Returns what `block`, just taken and not yet marked used, has over
    /// `needed`. What is returned has `block` before it, soon used, so it
    /// starts with no `FLAG_PREV_FREE`
    unsafe fn try_split(&mut self, block: *mut Block, needed: usize) {
        let total = (*block).size;
        let remainder = total - needed;
        if remainder < SPLIT_MIN { return; }

        (*block).size = needed;

        let new_block = (block as *mut u8).add(needed) as *mut Block;
        (*new_block).size = remainder;
        (*new_block).flags = 0;
        (*new_block).prev_free = core::ptr::null_mut();
        (*new_block).next_free = core::ptr::null_mut();
        self.release(new_block);
    }

    /// `block` is now used: the next block is told
    unsafe fn claim(&mut self, block: *mut Block) {
        (*block).flags |= FLAG_USED;
        (*(*block).next_adjacent()).flags &= !FLAG_PREV_FREE;
    }

    /// A slab with room for `total`, in a bin as one block with the sentinel
    /// after it. `slab` bytes when the `Source` has that much in one piece,
    /// else only what this request needs. False when there is no memory
    unsafe fn grow(&mut self, total: usize) -> bool {
        let need = align_up(total + BLOCK_HDR, PAGE);
        let mut map_size = need.max(self.slab);
        let mut p = self.source.grab(map_size);
        if p.is_null() && map_size > need {
            map_size = need;
            p = self.source.grab(map_size);
        }
        if p.is_null() { return false; }
        self.held += map_size;

        let sentinel = p.add(map_size - BLOCK_HDR) as *mut Block;
        (*sentinel).size = BLOCK_HDR;
        (*sentinel).flags = FLAG_USED;
        (*sentinel).prev_free = core::ptr::null_mut();
        (*sentinel).next_free = core::ptr::null_mut();

        let block = p as *mut Block;
        (*block).size = map_size - BLOCK_HDR;
        (*block).flags = 0;
        self.release(block);
        true
    }

    unsafe fn alloc_large(&mut self, total: usize) -> *mut Block {
        let map_size = align_up(total, PAGE);
        let p = self.source.grab(map_size);
        if p.is_null() { return core::ptr::null_mut(); }
        self.held += map_size;
        let block = p as *mut Block;
        (*block).size = map_size;
        (*block).flags = FLAG_USED | FLAG_MMAP;
        (*block).prev_free = core::ptr::null_mut();
        (*block).next_free = core::ptr::null_mut();
        block
    }

    /// `size` bytes aligned to `ALLOC_ALIGN`, or null.
    ///
    /// # Safety
    /// The `Source`'s rules hold
    pub unsafe fn alloc(&mut self, size: usize) -> *mut u8 {
        if size == 0 { return core::ptr::null_mut(); }
        let Some(total) = size.checked_add(BLOCK_HDR + ALLOC_ALIGN - 1).map(|n| n & !(ALLOC_ALIGN - 1)) else {
            return core::ptr::null_mut();
        };

        let mut block = self.find_free(total);
        if block.is_null() {
            if size >= self.large {
                let block = self.alloc_large(total);
                if block.is_null() { return core::ptr::null_mut(); }
                self.used += (*block).size;
                return (*block).data_ptr();
            }
            if !self.grow(total) { return core::ptr::null_mut(); }
            block = self.find_free(total);
        }
        self.try_split(block, total);
        self.claim(block);
        self.used += (*block).size;
        (*block).data_ptr()
    }

    /// Frees what `alloc` or `memalign` returned. A block with its own memory
    /// comes back for the caller to return.
    ///
    /// # Safety
    /// `ptr` is null or came from this heap
    pub unsafe fn free(&mut self, ptr: *mut u8) -> Option<Large> {
        if ptr.is_null() { return None; }

        // Every allocation has this header. Over-aligned, `size` holds the
        // distance back to the real block's data pointer
        let hdr = ptr.sub(BLOCK_HDR) as *mut Block;
        let ptr = if (*hdr).flags & FLAG_ALIGNED != 0 { ptr.sub((*hdr).size) } else { ptr };

        let block = ptr.sub(BLOCK_HDR) as *mut Block;
        if (*block).is_free() { return None; }

        if (*block).is_mmap() {
            let bytes = (*block).size;
            self.used -= bytes;
            self.held -= bytes;
            return Some(Large { ptr: block as *mut u8, bytes });
        }
        self.used -= (*block).size;

        // Marked free before any merge, so a second free finds a free block
        // where it looks
        (*block).flags &= !FLAG_USED;
        let mut block = block;
        let next = (*block).next_adjacent();
        if (*next).is_free() {
            self.bin_remove(next);
            (*block).size += (*next).size;
        }
        if (*block).flags & FLAG_PREV_FREE != 0 {
            let before = *((block as *mut u8).sub(core::mem::size_of::<usize>()) as *const usize);
            let prev = (block as *mut u8).sub(before) as *mut Block;
            self.bin_remove(prev);
            (*prev).size += (*block).size;
            block = prev;
        }
        self.release(block);
        None
    }

    /// Bytes an ordinary block holds, one from `alloc` and neither over-aligned
    /// nor separately mapped, or None.
    ///
    /// Needs no heap: the header belongs to the block's owner while allocated.
    /// A neighbor's free may set `FLAG_PREV_FREE` under the lock, so `flags` is
    /// read whole, and only for bits unchanging on a live block. For the thread
    /// caches in `heap.rs`.
    ///
    /// # Safety
    /// `ptr` is live and came from this heap
    pub unsafe fn plain_usable(ptr: *mut u8) -> Option<usize> {
        let hdr = ptr.sub(BLOCK_HDR) as *const Block;
        let flags = core::ptr::read_volatile(core::ptr::addr_of!((*hdr).flags));
        if flags & (FLAG_ALIGNED | FLAG_MMAP) != 0 || flags & FLAG_USED == 0 {
            return None;
        }
        Some(core::ptr::read_volatile(core::ptr::addr_of!((*hdr).size)).saturating_sub(BLOCK_HDR))
    }

    /// Bytes from `ptr` on that are the caller's: what `realloc` can grow into
    /// without moving. An over-aligned pointer has no block header before it,
    /// pointing into the middle of a block.
    ///
    /// # Safety
    /// `ptr` is live and came from this heap
    pub unsafe fn usable(&self, ptr: *mut u8) -> usize {
        let hdr = ptr.sub(BLOCK_HDR) as *mut Block;
        if (*hdr).flags & FLAG_ALIGNED != 0 {
            let orig = (ptr as usize) - (*hdr).size;
            let block = (orig as *mut u8).sub(BLOCK_HDR) as *mut Block;
            (*block).data_size().saturating_sub(ptr as usize - orig)
        } else {
            (*hdr).data_size()
        }
    }

    /// `size` bytes at a multiple of `align`, a power of two, or null.
    ///
    /// # Safety
    /// The `Source`'s rules hold
    pub unsafe fn memalign(&mut self, align: usize, size: usize) -> *mut u8 {
        if size == 0 || align == 0 || align & (align - 1) != 0 {
            return core::ptr::null_mut();
        }
        if align <= ALLOC_ALIGN {
            return self.alloc(size);
        }

        // Room for the alignment slack and a header before the returned
        // pointer. `checked_add`, since a size near the top of the address
        // space would wrap
        let raw_size = match size.checked_add(align).and_then(|n| n.checked_add(BLOCK_HDR)) {
            Some(n) => n,
            None    => return core::ptr::null_mut(),
        };
        let raw = self.alloc(raw_size);
        if raw.is_null() { return core::ptr::null_mut(); }

        // At least a header's gap for the marker below. Aligning up from
        // `raw + BLOCK_HDR` guarantees it, including when `raw` is already
        // aligned
        let aligned = align_up(raw as usize + BLOCK_HDR, align);

        let hdr = (aligned - BLOCK_HDR) as *mut Block;
        (*hdr).size = aligned - raw as usize;  // the way back, not a length
        (*hdr).flags = FLAG_USED | FLAG_ALIGNED;
        (*hdr).prev_free = core::ptr::null_mut();
        (*hdr).next_free = core::ptr::null_mut();
        aligned as *mut u8
    }

    /// The heap's invariants, for the host tests (`lib/runtime/libmiku/hosttest`), over
    /// what the `Source` gave, `(start, length)`, skipping a large block's own
    /// memory: every slab tiled to its sentinel, no adjacent free blocks,
    /// `FLAG_PREV_FREE` and the boundary tag correct, every free block in its
    /// size's bin only, and the bitmap matching. Returns the counts of free
    /// blocks and slabs
    #[cfg(test)]
    pub fn check(&self, slabs: &[(usize, usize)]) -> Result<(usize, usize), String> {
        let mut free_in_slabs = 0usize;
        let mut walked = 0usize;
        unsafe {
            for &(start, len) in slabs {
                if (*(start as *const Block)).is_mmap() {
                    continue;
                }
                walked += 1;
                let end = start + len - BLOCK_HDR;
                let mut at = start;
                let mut prev_free = false;
                while at < end {
                    let b = at as *const Block;
                    let size = (*b).size;
                    if size < SPLIT_MIN || size % ALLOC_ALIGN != 0 || at + size > end {
                        return Err(format!("block at {at:#x} has size {size}"));
                    }
                    if ((*b).flags & FLAG_PREV_FREE != 0) != prev_free {
                        return Err(format!("block at {at:#x}: FLAG_PREV_FREE says {}, the block before is {}",
                            (*b).flags & FLAG_PREV_FREE != 0, if prev_free { "free" } else { "used" }));
                    }
                    let free = (*b).is_free();
                    if free {
                        if prev_free {
                            return Err(format!("two free blocks touch at {at:#x}"));
                        }
                        let tag = *((at + size - core::mem::size_of::<usize>()) as *const usize);
                        if tag != size {
                            return Err(format!("free block at {at:#x} of {size} has the tag {tag}"));
                        }
                        let i = bin_of(size);
                        let mut cur = self.bins[i];
                        let mut found = false;
                        while !cur.is_null() {
                            found |= cur as usize == at;
                            cur = (*cur).next_free;
                        }
                        if !found {
                            return Err(format!("free block at {at:#x} of {size} is not in bin {i}"));
                        }
                        free_in_slabs += 1;
                    }
                    prev_free = free;
                    at += size;
                }
                if at != end {
                    return Err(format!("the slab at {start:#x} ends at {at:#x}, not its sentinel at {end:#x}"));
                }
                let s = end as *const Block;
                if (*s).size != BLOCK_HDR || (*s).is_free() || ((*s).flags & FLAG_PREV_FREE != 0) != prev_free {
                    return Err(format!("the sentinel at {end:#x} is wrong"));
                }
            }
            let mut in_bins = 0usize;
            for i in 0..NBINS {
                let mut cur = self.bins[i];
                let mut prev: *mut Block = core::ptr::null_mut();
                if cur.is_null() == (self.full[i / 64] & (1 << (i % 64)) != 0) {
                    return Err(format!("bin {i}'s bit is wrong"));
                }
                while !cur.is_null() {
                    if bin_of((*cur).size) != i || !(*cur).is_free() || (*cur).prev_free != prev {
                        return Err(format!("bin {i} holds {:#x} wrongly", cur as usize));
                    }
                    in_bins += 1;
                    prev = cur;
                    cur = (*cur).next_free;
                }
            }
            if in_bins != free_in_slabs {
                return Err(format!("{in_bins} blocks in the bins, {free_in_slabs} free in the slabs"));
            }
        }
        Ok((free_in_slabs, walked))
    }
}
