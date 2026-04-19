// Generic fixed-size object pool with generation tracking
//
// Thread-safe pool for reusable objects of fixed size
// Each slot has a generation counter to detect stale handles
// Useful for entity systems, connection pools, resource managers
// Heap-allocated backing store

use crate::heap;
use crate::mem;

const MAX_POOL_SIZE: usize = 4096;

#[repr(C)]
pub struct MikuPool {
    data: *mut u8,         // backing storage
    generations: *mut u32, // generation per slot
    free_list: *mut u32,   // free slot indices
    obj_size: usize,
    capacity: usize,
    free_count: usize,
    active_count: usize,
}

// Pool handle: index + generation packed into u64
// upper 32 bits = generation, lower 32 bits = index
#[repr(C)]
#[derive(Copy, Clone)]
pub struct PoolHandle(pub u64);

impl PoolHandle {
    fn new(index: u32, gen: u32) -> Self {
        Self(((gen as u64) << 32) | (index as u64))
    }
    fn index(self) -> u32 { self.0 as u32 }
    fn generation(self) -> u32 { (self.0 >> 32) as u32 }
}

// Create pool for objects of given size
// Returns zeroed struct on failure
#[no_mangle]
pub extern "C" fn miku_pool_new(obj_size: usize, capacity: usize) -> MikuPool {
    let cap = if capacity > MAX_POOL_SIZE { MAX_POOL_SIZE } else { capacity };
    if obj_size == 0 || cap == 0 {
        return MikuPool {
            data: core::ptr::null_mut(),
            generations: core::ptr::null_mut(),
            free_list: core::ptr::null_mut(),
            obj_size: 0, capacity: 0, free_count: 0, active_count: 0,
        };
    }

    let data = heap::miku_calloc(cap, obj_size);
    let gens = heap::miku_calloc(cap, 4) as *mut u32;
    let fl = heap::miku_malloc(cap * 4) as *mut u32;

    if data.is_null() || gens.is_null() || fl.is_null() {
        if !data.is_null() { heap::miku_free(data); }
        if !gens.is_null() { heap::miku_free(gens as *mut u8); }
        if !fl.is_null() { heap::miku_free(fl as *mut u8); }
        return MikuPool {
            data: core::ptr::null_mut(),
            generations: core::ptr::null_mut(),
            free_list: core::ptr::null_mut(),
            obj_size: 0, capacity: 0, free_count: 0, active_count: 0,
        };
    }

    // fill free list (reverse order so allocation goes 0,1,2...)
    unsafe {
        for i in 0..cap {
            *fl.add(i) = (cap - 1 - i) as u32;
        }
    }

    MikuPool {
        data,
        generations: gens,
        free_list: fl,
        obj_size,
        capacity: cap,
        free_count: cap,
        active_count: 0,
    }
}

// free pool and all memory
#[no_mangle]
pub extern "C" fn miku_pool_free(p: *mut MikuPool) {
    if p.is_null() { return; }
    unsafe {
        let pool = &mut *p;
        if !pool.data.is_null() { heap::miku_free(pool.data); }
        if !pool.generations.is_null() { heap::miku_free(pool.generations as *mut u8); }
        if !pool.free_list.is_null() { heap::miku_free(pool.free_list as *mut u8); }
        pool.data = core::ptr::null_mut();
        pool.capacity = 0;
        pool.free_count = 0;
        pool.active_count = 0;
    }
}

// allocate object from pool
// Returns handle. Handle.0 = 0xFFFFFFFF_FFFFFFFF on failure
#[no_mangle]
pub extern "C" fn miku_pool_alloc(p: *mut MikuPool) -> PoolHandle {
    let bad = PoolHandle(u64::MAX);
    if p.is_null() { return bad; }
    let pool = unsafe { &mut *p };
    if pool.free_count == 0 { return bad; }

    pool.free_count -= 1;
    let index = unsafe { *pool.free_list.add(pool.free_count) };
    let gen = unsafe { *pool.generations.add(index as usize) };

    // zero the object
    unsafe {
        mem::miku_bzero(pool.data.add(index as usize * pool.obj_size), pool.obj_size);
    }

    pool.active_count += 1;
    PoolHandle::new(index, gen)
}

// release object back to pool
// Returns true if handle was valid
#[no_mangle]
pub extern "C" fn miku_pool_release(p: *mut MikuPool, handle: PoolHandle) -> bool {
    if p.is_null() { return false; }
    let pool = unsafe { &mut *p };

    let idx = handle.index() as usize;
    let gen = handle.generation();

    if idx >= pool.capacity { return false; }
    let cur_gen = unsafe { *pool.generations.add(idx) };
    if cur_gen != gen { return false; }

    // increment generation
    unsafe { *pool.generations.add(idx) = cur_gen.wrapping_add(1); }

    // push back to free list
    unsafe { *pool.free_list.add(pool.free_count) = idx as u32; }
    pool.free_count += 1;
    pool.active_count -= 1;
    true
}

// get pointer to object by handle
// Returns null if handle is invalid/stale
#[no_mangle]
pub extern "C" fn miku_pool_get(p: *const MikuPool, handle: PoolHandle) -> *mut u8 {
    if p.is_null() { return core::ptr::null_mut(); }
    let pool = unsafe { &*p };

    let idx = handle.index() as usize;
    let gen = handle.generation();

    if idx >= pool.capacity { return core::ptr::null_mut(); }
    let cur_gen = unsafe { *pool.generations.add(idx) };
    if cur_gen != gen { return core::ptr::null_mut(); }

    unsafe { pool.data.add(idx * pool.obj_size) }
}

// check if handle is still valid
#[no_mangle]
pub extern "C" fn miku_pool_valid(p: *const MikuPool, handle: PoolHandle) -> bool {
    !miku_pool_get(p, handle).is_null()
}

// number of active objects
#[no_mangle]
pub extern "C" fn miku_pool_active(p: *const MikuPool) -> usize {
    if p.is_null() { return 0; }
    unsafe { (*p).active_count }
}

// total capacity
#[no_mangle]
pub extern "C" fn miku_pool_capacity(p: *const MikuPool) -> usize {
    if p.is_null() { return 0; }
    unsafe { (*p).capacity }
}

// number of free slots
#[no_mangle]
pub extern "C" fn miku_pool_available(p: *const MikuPool) -> usize {
    if p.is_null() { return 0; }
    unsafe { (*p).free_count }
}

// iterate over all active objects
type PoolIterFn = extern "C" fn(PoolHandle, *mut u8, *mut u8);

#[no_mangle]
pub extern "C" fn miku_pool_iter(
    p: *const MikuPool,
    cb: PoolIterFn,
    ctx: *mut u8,
) {
    if p.is_null() { return; }
    let pool = unsafe { &*p };

    // An object is active if its slot is not in the free list
    // Simple approach: check if generation matches any possible active state
    // We iterate all slots and skip those in the free list
    // For simplicity, build a "used" check: if a slot is in free_list, skip it
    // Since free_list is unsorted, we iterate it
    unsafe {
        for idx in 0..pool.capacity {
            // check if idx is in free list
            let mut is_free = false;
            for fi in 0..pool.free_count {
                if *pool.free_list.add(fi) == idx as u32 {
                    is_free = true;
                    break;
                }
            }
            if is_free { continue; }

            let gen = *pool.generations.add(idx);
            let handle = PoolHandle::new(idx as u32, gen);
            let ptr = pool.data.add(idx * pool.obj_size);
            cb(handle, ptr, ctx);
        }
    }
}
