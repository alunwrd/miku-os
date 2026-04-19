// Hash table with open addressing (Robin Hood hashing)
// Keys and values are fixed-size byte blobs. Default key/value = u64.
// Uses FNV-1a for hashing

use crate::heap;
use crate::mem;

const MAP_INITIAL_CAP: usize = 16;
const MAP_LOAD_FACTOR_NUM: usize = 3; // grow when load > 75%
const MAP_LOAD_FACTOR_DEN: usize = 4;

const SLOT_EMPTY: u8 = 0;
const SLOT_OCCUPIED: u8 = 1;
const SLOT_TOMBSTONE: u8 = 2;

// FNV-1a hash for arbitrary bytes

fn fnv1a(data: *const u8, len: usize) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for i in 0..len {
        h ^= unsafe { *data.add(i) } as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// Each slot layout in memory: [state: 1 byte][key: key_size bytes][value: val_size bytes]

#[repr(C)]
pub struct MikuMap {
    slots: *mut u8,
    cap: usize,   // number of slots (always power of 2)
    count: usize, // number of occupied entries
    key_size: usize,
    val_size: usize,
    slot_size: usize, // 1 + key_size + val_size
}

impl MikuMap {
    fn slot_ptr(&self, index: usize) -> *mut u8 {
        unsafe { self.slots.add(index * self.slot_size) }
    }

    fn slot_state(&self, index: usize) -> u8 {
        unsafe { *self.slot_ptr(index) }
    }

    fn slot_key(&self, index: usize) -> *const u8 {
        unsafe { self.slot_ptr(index).add(1) }
    }

    fn slot_val(&self, index: usize) -> *mut u8 {
        unsafe { self.slot_ptr(index).add(1 + self.key_size) }
    }

    fn set_slot_state(&self, index: usize, state: u8) {
        unsafe {
            *self.slot_ptr(index) = state;
        }
    }
}

// lifecycle //

#[no_mangle]
pub extern "C" fn miku_map_new(key_size: usize, val_size: usize) -> MikuMap {
    let ks = if key_size == 0 { 1 } else { key_size };
    let vs = if val_size == 0 { 1 } else { val_size };
    MikuMap {
        slots: core::ptr::null_mut(),
        cap: 0,
        count: 0,
        key_size: ks,
        val_size: vs,
        slot_size: 1 + ks + vs,
    }
}

#[no_mangle]
pub extern "C" fn miku_map_free(m: *mut MikuMap) {
    if m.is_null() {
        return;
    }
    unsafe {
        if !(*m).slots.is_null() {
            heap::miku_free((*m).slots);
        }
        (*m).slots = core::ptr::null_mut();
        (*m).cap = 0;
        (*m).count = 0;
    }
}

// internal: allocate slots zeroed

unsafe fn alloc_slots(cap: usize, slot_size: usize) -> *mut u8 {
    let total = cap * slot_size;
    let p = heap::miku_malloc(total);
    if !p.is_null() {
        mem::miku_memset(p, 0, total);
    }
    p
}

// internal: grow and rehash

unsafe fn map_grow(m: *mut MikuMap) -> bool {
    let new_cap = if (*m).cap == 0 {
        MAP_INITIAL_CAP
    } else {
        (*m).cap * 2
    };
    let new_slots = alloc_slots(new_cap, (*m).slot_size);
    if new_slots.is_null() {
        return false;
    }

    let old_slots = (*m).slots;
    let old_cap = (*m).cap;

    (*m).slots = new_slots;
    (*m).cap = new_cap;
    (*m).count = 0;

    // rehash all occupied entries from old table
    if !old_slots.is_null() {
        for i in 0..old_cap {
            let state = *old_slots.add(i * (*m).slot_size);
            if state == SLOT_OCCUPIED {
                let key = old_slots.add(i * (*m).slot_size + 1);
                let val = old_slots.add(i * (*m).slot_size + 1 + (*m).key_size);
                map_insert_inner(m, key, val);
            }
        }
        heap::miku_free(old_slots);
    }
    true
}

// internal: insert without checking load factor

unsafe fn map_insert_inner(m: *mut MikuMap, key: *const u8, val: *const u8) {
    let mask = (*m).cap - 1;
    let hash = fnv1a(key, (*m).key_size);
    let mut index = (hash as usize) & mask;
    let mut tombstone: usize = usize::MAX; // index of first tombstone, or sentinel

    loop {
        let state = (*m).slot_state(index);
        if state == SLOT_EMPTY {
            // Key does not exist; insert at tombstone if we saw one, else here
            let target = if tombstone != usize::MAX { tombstone } else { index };
            (*m).set_slot_state(target, SLOT_OCCUPIED);
            mem::miku_memcpy((*m).slot_ptr(target).add(1), key, (*m).key_size);
            mem::miku_memcpy((*m).slot_val(target), val, (*m).val_size);
            (*m).count += 1;
            return;
        }
        if state == SLOT_TOMBSTONE {
            // Record the first tombstone but keep probing for an existing key
            if tombstone == usize::MAX { tombstone = index; }
        } else {
            // SLOT_OCCUPIED: check for key match
            if mem::miku_memcmp((*m).slot_key(index), key, (*m).key_size) == 0 {
                mem::miku_memcpy((*m).slot_val(index), val, (*m).val_size);
                return;
            }
        }
        index = (index + 1) & mask;
    }
}

// find slot index for given key, or usize::MAX if not found

unsafe fn map_find(m: *const MikuMap, key: *const u8) -> usize {
    if (*m).cap == 0 {
        return usize::MAX;
    }
    let mask = (*m).cap - 1;
    let hash = fnv1a(key, (*m).key_size);
    let mut index = (hash as usize) & mask;
    let start = index;

    loop {
        let state = *(*m).slots.add(index * (*m).slot_size);
        if state == SLOT_EMPTY {
            return usize::MAX;
        }
        if state == SLOT_OCCUPIED {
            let k = (*m).slot_key(index);
            if mem::miku_memcmp(k, key, (*m).key_size) == 0 {
                return index;
            }
        }
        index = (index + 1) & mask;
        if index == start {
            return usize::MAX;
        }
    }
}

// public API //

#[no_mangle]
pub extern "C" fn miku_map_insert(m: *mut MikuMap, key: *const u8, val: *const u8) -> bool {
    if m.is_null() || key.is_null() || val.is_null() {
        return false;
    }
    unsafe {
        // check load factor
        let threshold = (*m).cap * MAP_LOAD_FACTOR_NUM / MAP_LOAD_FACTOR_DEN;
        if (*m).count >= threshold {
            if !map_grow(m) {
                return false;
            }
        }
        map_insert_inner(m, key, val);
        true
    }
}

#[no_mangle]
pub extern "C" fn miku_map_get(m: *const MikuMap, key: *const u8) -> *const u8 {
    if m.is_null() || key.is_null() {
        return core::ptr::null();
    }
    unsafe {
        let idx = map_find(m, key);
        if idx == usize::MAX {
            return core::ptr::null();
        }
        (*m).slot_val(idx)
    }
}

#[no_mangle]
pub extern "C" fn miku_map_contains(m: *const MikuMap, key: *const u8) -> bool {
    if m.is_null() || key.is_null() {
        return false;
    }
    unsafe { map_find(m, key) != usize::MAX }
}

#[no_mangle]
pub extern "C" fn miku_map_remove(m: *mut MikuMap, key: *const u8) -> bool {
    if m.is_null() || key.is_null() {
        return false;
    }
    unsafe {
        let idx = map_find(m, key);
        if idx == usize::MAX {
            return false;
        }
        (*m).set_slot_state(idx, SLOT_TOMBSTONE);
        (*m).count -= 1;
        true
    }
}

#[no_mangle]
pub extern "C" fn miku_map_len(m: *const MikuMap) -> usize {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).count }
}

#[no_mangle]
pub extern "C" fn miku_map_clear(m: *mut MikuMap) {
    if m.is_null() {
        return;
    }
    unsafe {
        if !(*m).slots.is_null() {
            mem::miku_memset((*m).slots, 0, (*m).cap * (*m).slot_size);
        }
        (*m).count = 0;
    }
}

// iteration: calls callback(key, val, user_data) for each entry

#[no_mangle]
pub extern "C" fn miku_map_iter(
    m: *const MikuMap,
    cb: extern "C" fn(*const u8, *const u8, *mut u8),
    user_data: *mut u8,
) {
    if m.is_null() {
        return;
    }
    unsafe {
        for i in 0..(*m).cap {
            if (*m).slot_state(i) == SLOT_OCCUPIED {
                cb((*m).slot_key(i), (*m).slot_val(i) as *const u8, user_data);
            }
        }
    }
}

// convenience: u64 -> u64 map helpers

#[no_mangle]
pub extern "C" fn miku_map_new_u64() -> MikuMap {
    miku_map_new(8, 8)
}

#[no_mangle]
pub extern "C" fn miku_map_insert_u64(m: *mut MikuMap, key: u64, val: u64) -> bool {
    miku_map_insert(
        m,
        &key as *const u64 as *const u8,
        &val as *const u64 as *const u8,
    )
}

#[no_mangle]
pub extern "C" fn miku_map_get_u64(m: *const MikuMap, key: u64) -> u64 {
    let p = miku_map_get(m, &key as *const u64 as *const u8);
    if p.is_null() {
        return 0;
    }
    unsafe { *(p as *const u64) }
}
