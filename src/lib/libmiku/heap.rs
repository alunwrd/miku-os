use crate::sync::SpinLock;

const ALLOC_ALIGN: usize = 16;
const BLOCK_HDR: usize = 32;
const SLAB_SIZE: usize = 131072;
const LARGE_THRESHOLD: usize = 32768;
const SPLIT_MIN: usize = BLOCK_HDR + ALLOC_ALIGN;
const PROT_RW: u64 = 1 | 2;

#[repr(C)]
struct Block {
    size: usize,
    flags: usize,
    prev_free: *mut Block,
    next_free: *mut Block,
}

const FLAG_USED: usize = 1;
const FLAG_MMAP: usize = 2;

const MAX_ALIGNED: usize = 16;

#[derive(Clone, Copy)]
struct AlignEntry {
    aligned: usize,
    original: usize,
}

struct AlignTable {
    entries: [AlignEntry; MAX_ALIGNED],
    count: usize,
}

impl AlignTable {
    const fn new() -> Self {
        const EMPTY: AlignEntry = AlignEntry { aligned: 0, original: 0 };
        Self { entries: [EMPTY; MAX_ALIGNED], count: 0 }
    }

    fn insert(&mut self, aligned: usize, original: usize) {
        if self.count < MAX_ALIGNED {
            self.entries[self.count] = AlignEntry { aligned, original };
            self.count += 1;
        }
    }

    fn take(&mut self, aligned: usize) -> Option<usize> {
        for i in 0..self.count {
            if self.entries[i].aligned == aligned {
                let orig = self.entries[i].original;
                self.entries[i] = self.entries[self.count - 1];
                self.entries[self.count - 1] = AlignEntry { aligned: 0, original: 0 };
                self.count -= 1;
                return Some(orig);
            }
        }
        None
    }
}

static ALIGN_TABLE: SpinLock<AlignTable> = SpinLock::new(AlignTable::new());

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

struct HeapState {
    free_head: *mut Block,
    slab_ptr: *mut u8,
    slab_end: *mut u8,
}

unsafe impl Send for HeapState {}

impl HeapState {
    const fn new() -> Self {
        Self {
            free_head: core::ptr::null_mut(),
            slab_ptr: core::ptr::null_mut(),
            slab_end: core::ptr::null_mut(),
        }
    }

    unsafe fn free_list_remove(&mut self, block: *mut Block) {
        let prev = (*block).prev_free;
        let next = (*block).next_free;
        if !prev.is_null() {
            (*prev).next_free = next;
        } else {
            self.free_head = next;
        }
        if !next.is_null() {
            (*next).prev_free = prev;
        }
        (*block).prev_free = core::ptr::null_mut();
        (*block).next_free = core::ptr::null_mut();
    }

    unsafe fn free_list_insert(&mut self, block: *mut Block) {
        (*block).flags &= !FLAG_USED;
        (*block).prev_free = core::ptr::null_mut();
        (*block).next_free = self.free_head;
        if !self.free_head.is_null() {
            (*self.free_head).prev_free = block;
        }
        self.free_head = block;
    }

    unsafe fn find_free(&mut self, needed: usize) -> *mut Block {
        let mut best: *mut Block = core::ptr::null_mut();
        let mut best_size = usize::MAX;
        let mut cur = self.free_head;

        while !cur.is_null() {
            let sz = (*cur).size;
            if sz >= needed && sz < best_size {
                best = cur;
                best_size = sz;
                if sz == needed { break; }
            }
            cur = (*cur).next_free;
        }

        if !best.is_null() {
            self.free_list_remove(best);
        }
        best
    }

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
        self.free_list_insert(new_block);
    }

    unsafe fn try_coalesce(&mut self, block: *mut Block) {
        let next = (*block).next_adjacent();
        let next_addr = next as usize;
        let slab_end_addr = self.slab_end as usize;

        if next_addr >= slab_end_addr { return; }
        if (*next).is_free() {
            self.free_list_remove(next);
            (*block).size += (*next).size;
        }
    }

    unsafe fn alloc_slab(&mut self, total: usize) -> *mut Block {
        let slab_left = if self.slab_ptr.is_null() {
            0
        } else {
            (self.slab_end as usize).saturating_sub(self.slab_ptr as usize)
        };

        if slab_left < total {
            let map_size = if SLAB_SIZE > total { SLAB_SIZE } else { align_up(total, 4096) };
            let p = crate::proc::miku_mmap(0, map_size, PROT_RW);
            if p.is_null() { return core::ptr::null_mut(); }
            self.slab_ptr = p;
            self.slab_end = p.add(map_size);
        }

        let block = self.slab_ptr as *mut Block;
        (*block).size = total;
        (*block).flags = FLAG_USED;
        (*block).prev_free = core::ptr::null_mut();
        (*block).next_free = core::ptr::null_mut();
        self.slab_ptr = self.slab_ptr.add(total);
        block
    }

    unsafe fn alloc_large(&mut self, total: usize) -> *mut Block {
        let map_size = align_up(total, 4096);
        let p = crate::proc::miku_mmap(0, map_size, PROT_RW);
        if p.is_null() { return core::ptr::null_mut(); }
        let block = p as *mut Block;
        (*block).size = map_size;
        (*block).flags = FLAG_USED | FLAG_MMAP;
        (*block).prev_free = core::ptr::null_mut();
        (*block).next_free = core::ptr::null_mut();
        block
    }
}

static HEAP: SpinLock<HeapState> = SpinLock::new(HeapState::new());

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_malloc(size: usize) -> *mut u8 {
    if size == 0 { return core::ptr::null_mut(); }
    let total = align_up(size + BLOCK_HDR, ALLOC_ALIGN);
    let mut heap = HEAP.lock();

    unsafe {
        let block = heap.find_free(total);
        if !block.is_null() {
            (*block).flags |= FLAG_USED;
            heap.try_split(block, total);
            return (*block).data_ptr();
        }

        let block = if size >= LARGE_THRESHOLD {
            heap.alloc_large(total)
        } else {
            heap.alloc_slab(total)
        };
        if block.is_null() { return core::ptr::null_mut(); }
        (*block).data_ptr()
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_free(ptr: *mut u8) {
    if ptr.is_null() { return; }

    {
        let mut table = ALIGN_TABLE.lock();
        if let Some(original) = table.take(ptr as usize) {
            drop(table);
            miku_free(original as *mut u8);
            return;
        }
    }

    let mut heap = HEAP.lock();

    unsafe {
        let block = ptr.sub(BLOCK_HDR) as *mut Block;
        if (*block).is_free() { return; }

        if (*block).is_mmap() {
            let sz = (*block).size;
            drop(heap);
            crate::proc::miku_munmap(block as *mut u8, sz);
            return;
        }

        heap.free_list_insert(block);
        heap.try_coalesce(block);
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_realloc(ptr: *mut u8, new_size: usize) -> *mut u8 {
    if ptr.is_null() { return miku_malloc(new_size); }
    if new_size == 0 { miku_free(ptr); return core::ptr::null_mut(); }

    let old_data = unsafe {
        let block = ptr.sub(BLOCK_HDR) as *mut Block;
        (*block).data_size()
    };

    if old_data >= new_size { return ptr; }

    let new_ptr = miku_malloc(new_size);
    if new_ptr.is_null() { return core::ptr::null_mut(); }
    let copy_len = if old_data < new_size { old_data } else { new_size };
    crate::mem::miku_memcpy(new_ptr, ptr, copy_len);
    miku_free(ptr);
    new_ptr
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_calloc(count: usize, size: usize) -> *mut u8 {
    let total = match count.checked_mul(size) {
        Some(t) if t > 0 => t,
        _ => return core::ptr::null_mut(),
    };
    let p = miku_malloc(total);
    if !p.is_null() {
        crate::mem::miku_memset(p, 0, total);
    }
    p
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memalign(align: usize, size: usize) -> *mut u8 {
    if size == 0 || align == 0 || align & (align - 1) != 0 {
        return core::ptr::null_mut();
    }
    if align <= ALLOC_ALIGN {
        return miku_malloc(size);
    }

    let raw = miku_malloc(size + align);
    if raw.is_null() { return core::ptr::null_mut(); }

    let aligned = align_up(raw as usize, align);
    if aligned == raw as usize {
        return raw;
    }

    ALIGN_TABLE.lock().insert(aligned, raw as usize);
    aligned as *mut u8
}
