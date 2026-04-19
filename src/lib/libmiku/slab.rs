/////////////////////////////////////////////////////////////////////////
//          Fixed-size object pool allocator                           //
//                                                                     //
// Pre-allocates a block of memory divided into equal-sized slots      //
// Allocation and deallocation are O(1) using a free-list              //
// Perfect for allocating many objects of the same size                //
// (e.g., network buffers, task structs, inode cache entries)          //
/////////////////////////////////////////////////////////////////////////

use crate::heap;
use crate::mem;

// slab allocator for fixed-size objects
#[repr(C)]
pub struct MikuSlab {
    pool: *mut u8,
    free_head: *mut *mut u8, // pointer to first free slot's "next" pointer
    slot_size: usize,
    capacity: usize,
    in_use: usize,
}

// Each free slot stores a pointer to the next free slot at its star
// This requires slot_size >= size_of::<*mut u8>() (8 bytes on 64-bit)

// create slab allocator for objects of given size
#[no_mangle]
pub extern "C" fn miku_slab_new(obj_size: usize, capacity: usize) -> MikuSlab {
    // slot size must fit at least a pointer for free-list threading
    let min_slot = core::mem::size_of::<*mut u8>();
    let slot = if obj_size < min_slot { min_slot } else { obj_size };
    // align slot size to 16 bytes
    let slot_aligned = (slot + 15) & !15;

    let cap = if capacity == 0 { 16 } else { capacity };
    let pool = heap::miku_malloc(slot_aligned * cap);
    if pool.is_null() {
        return MikuSlab {
            pool: core::ptr::null_mut(),
            free_head: core::ptr::null_mut(),
            slot_size: slot_aligned,
            capacity: 0,
            in_use: 0,
        };
    }

    // build free list
    unsafe {
        for i in 0..cap - 1 {
            let slot_ptr = pool.add(i * slot_aligned);
            let next_ptr = pool.add((i + 1) * slot_aligned);
            *(slot_ptr as *mut *mut u8) = next_ptr;
        }
        // last slot points to null
        let last = pool.add((cap - 1) * slot_aligned);
        *(last as *mut *mut u8) = core::ptr::null_mut();
    }

    MikuSlab {
        pool,
        free_head: pool as *mut *mut u8,
        slot_size: slot_aligned,
        capacity: cap,
        in_use: 0,
    }
}

// free slab allocator
#[no_mangle]
pub extern "C" fn miku_slab_free(s: *mut MikuSlab) {
    if s.is_null() {
        return;
    }
    unsafe {
        if !(*s).pool.is_null() {
            heap::miku_free((*s).pool);
            (*s).pool = core::ptr::null_mut();
        }
        (*s).free_head = core::ptr::null_mut();
        (*s).capacity = 0;
        (*s).in_use = 0;
    }
}

// allocate one object from slab
#[no_mangle]
pub extern "C" fn miku_slab_alloc(s: *mut MikuSlab) -> *mut u8 {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let head = (*s).free_head;
        if head.is_null() {
            return core::ptr::null_mut(); // slab exhausted
        }
        // head points to the free slot
        let slot = head as *mut u8;
        // next free slot is stored at the start of this slot
        let next = *(slot as *const *mut u8);
        (*s).free_head = next as *mut *mut u8;
        (*s).in_use += 1;
        // zero out the slot before returning
        mem::miku_memset(slot, 0, (*s).slot_size);
        slot
    }
}

// return object to slab
#[no_mangle]
pub extern "C" fn miku_slab_dealloc(s: *mut MikuSlab, ptr: *mut u8) {
    if s.is_null() || ptr.is_null() {
        return;
    }
    unsafe {
        // check bounds
        let pool_start = (*s).pool as usize;
        let pool_end = pool_start + (*s).capacity * (*s).slot_size;
        let p = ptr as usize;
        if p < pool_start || p >= pool_end {
            return; // not from this slab
        }

        // prepend to free list
        *(ptr as *mut *mut u8) = (*s).free_head as *mut u8;
        (*s).free_head = ptr as *mut *mut u8;
        if (*s).in_use > 0 {
            (*s).in_use -= 1;
        }
    }
}

// number of allocated objects
#[no_mangle]
pub extern "C" fn miku_slab_in_use(s: *const MikuSlab) -> usize {
    if s.is_null() {
        return 0;
    }
    unsafe { (*s).in_use }
}

// number of free slots
#[no_mangle]
pub extern "C" fn miku_slab_available(s: *const MikuSlab) -> usize {
    if s.is_null() {
        return 0;
    }
    unsafe { (*s).capacity - (*s).in_use }
}

// total capacity
#[no_mangle]
pub extern "C" fn miku_slab_capacity(s: *const MikuSlab) -> usize {
    if s.is_null() {
        return 0;
    }
    unsafe { (*s).capacity }
}

// slot size (may be larger than requested obj_size due to alignment)
#[no_mangle]
pub extern "C" fn miku_slab_slot_size(s: *const MikuSlab) -> usize {
    if s.is_null() {
        return 0;
    }
    unsafe { (*s).slot_size }
}

// check if slab is full
#[no_mangle]
pub extern "C" fn miku_slab_is_full(s: *const MikuSlab) -> bool {
    if s.is_null() {
        return true;
    }
    unsafe { (*s).free_head.is_null() }
}

// check if slab is empty (no allocations)
#[no_mangle]
pub extern "C" fn miku_slab_is_empty(s: *const MikuSlab) -> bool {
    if s.is_null() {
        return true;
    }
    unsafe { (*s).in_use == 0 }
}
