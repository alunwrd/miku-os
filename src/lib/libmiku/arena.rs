//////////////////////////////////////////////////////////////////////////////////////
// All allocations from an arena are freed at once via miku_arena_free              //
// or miku_arena_reset. Individual free is not supported - this is the              //
// whole point of arenas: allocate many small objects, free them all                //
//////////////////////////////////////////////////////////////////////////////////////

use crate::heap;
use crate::mem;

const ARENA_BLOCK_SIZE: usize = 4096;
const ARENA_ALIGN: usize = 16;

// internal block header
#[repr(C)]
struct ArenaBlock {
    data: *mut u8,
    size: usize,
    used: usize,
    next: *mut ArenaBlock,
}

// public arena handle
#[repr(C)]
pub struct MikuArena {
    head: *mut ArenaBlock,
    block_size: usize,
    total_alloc: usize,
    first: *mut ArenaBlock,
}

#[inline(always)]
fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

unsafe fn block_new(size: usize) -> *mut ArenaBlock {
    let header_size = core::mem::size_of::<ArenaBlock>();
    let total = header_size + size;
    let raw = heap::miku_malloc(total);
    if raw.is_null() {
        return core::ptr::null_mut();
    }
    let block = raw as *mut ArenaBlock;
    (*block).data = raw.add(header_size);
    (*block).size = size;
    (*block).used = 0;
    (*block).next = core::ptr::null_mut();
    block
}

// create a new arena with default block size
#[no_mangle]
pub extern "C" fn miku_arena_new() -> MikuArena {
    miku_arena_with_block_size(ARENA_BLOCK_SIZE)
}

// create arena with custom block size
#[no_mangle]
pub extern "C" fn miku_arena_with_block_size(block_size: usize) -> MikuArena {
    let bs = if block_size < 64 { 64 } else { block_size };
    let head = unsafe { block_new(bs) };
    MikuArena {
        head,
        first: head,
        block_size: bs,
        total_alloc: 0,
    }
}

// allocate memory from arena (16-byte aligned)
#[no_mangle]
pub extern "C" fn miku_arena_alloc(arena: *mut MikuArena, size: usize) -> *mut u8 {
    if arena.is_null() || size == 0 {
        return core::ptr::null_mut();
    }
    let aligned = align_up(size, ARENA_ALIGN);

    unsafe {
        // try current head block
        let head = (*arena).head;
        if !head.is_null() {
            let remain = (*head).size - (*head).used;
            if remain >= aligned {
                let ptr = (*head).data.add((*head).used);
                (*head).used += aligned;
                (*arena).total_alloc += aligned;
                return ptr;
            }
        }

        // need a new block - at least big enough for this alloc
        let new_size = if aligned > (*arena).block_size {
            aligned
        } else {
            (*arena).block_size
        };
        let new_block = block_new(new_size);
        if new_block.is_null() {
            return core::ptr::null_mut();
        }

        // prepend to chain
        (*new_block).next = (*arena).head;
        (*arena).head = new_block;

        let ptr = (*new_block).data;
        (*new_block).used = aligned;
        (*arena).total_alloc += aligned;
        ptr
    }
}

// allocate zeroed memory from arena
#[no_mangle]
pub extern "C" fn miku_arena_calloc(arena: *mut MikuArena, size: usize) -> *mut u8 {
    let ptr = miku_arena_alloc(arena, size);
    if !ptr.is_null() {
        // miku_arena_alloc internally rounds up to align_up(size, 16)
        // Zeroing only 'size' bytes left the trailing alignment padding
        // uninitialised. Zero the full aligned block to match calloc semantics.
        let aligned = (size + 15) & !15;
        mem::miku_memset(ptr, 0, aligned);
    }
    ptr
}

// duplicate a string into arena
#[no_mangle]
pub extern "C" fn miku_arena_strdup(arena: *mut MikuArena, s: *const u8) -> *mut u8 {
    if arena.is_null() || s.is_null() {
        return core::ptr::null_mut();
    }
    let len = crate::string::miku_strlen(s);
    let ptr = miku_arena_alloc(arena, len + 1);
    if !ptr.is_null() {
        mem::miku_memcpy(ptr, s, len);
        unsafe { *ptr.add(len) = 0; }
    }
    ptr
}

// reset arena (free all blocks except first, reset used counters)
#[no_mangle]
pub extern "C" fn miku_arena_reset(arena: *mut MikuArena) {
    if arena.is_null() {
        return;
    }
    unsafe {
        // free all blocks except first
        let mut cur = (*arena).head;
        while !cur.is_null() && cur != (*arena).first {
            let next = (*cur).next;
            heap::miku_free(cur as *mut u8);
            cur = next;
        }
        (*arena).head = (*arena).first;
        if !(*arena).first.is_null() {
            (*(*arena).first).next = core::ptr::null_mut();
            (*(*arena).first).used = 0;
        }
        (*arena).total_alloc = 0;
    }
}

// free arena completely
#[no_mangle]
pub extern "C" fn miku_arena_free(arena: *mut MikuArena) {
    if arena.is_null() {
        return;
    }
    unsafe {
        let mut cur = (*arena).head;
        while !cur.is_null() {
            let next = (*cur).next;
            heap::miku_free(cur as *mut u8);
            cur = next;
        }
        (*arena).head = core::ptr::null_mut();
        (*arena).first = core::ptr::null_mut(); // prevent UAF via miku_arena_reset
        (*arena).total_alloc = 0;
    }
}

// get total bytes allocated from arena
#[no_mangle]
pub extern "C" fn miku_arena_used(arena: *const MikuArena) -> usize {
    if arena.is_null() {
        return 0;
    }
    unsafe { (*arena).total_alloc }
}
