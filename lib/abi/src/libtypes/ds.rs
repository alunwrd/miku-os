//! ds_miku: containers and allocators

use core::sync::atomic::AtomicUsize;

// public arena handle
#[repr(C)]
pub struct MikuArena {
    pub head: *mut ArenaBlock,
    pub block_size: usize,
    pub total_alloc: usize,
    pub first: *mut ArenaBlock,
}

// internal block header
#[repr(C)]
pub struct ArenaBlock {
    pub data: *mut u8,
    pub size: usize,
    pub used: usize,
    pub next: *mut ArenaBlock,
}

// dynamic bitset
#[repr(C)]
pub struct MikuBitset {
    pub words: *mut u64,
    pub num_words: usize,
}

// bounded SPSC channel
#[repr(C)]
pub struct MikuChannel {
    pub buf: *mut u8,
    pub cap: usize,          // number of slots
    pub elem_size: usize,
    pub head: AtomicUsize,   // write position (producer)
    pub tail: AtomicUsize,   // read position (consumer)
}

#[repr(C)]
pub struct MikuMap {
    pub slots: *mut u8,
    pub cap: usize,   // number of slots (always power of 2)
    pub count: usize, // number of occupied entries
    pub key_size: usize,
    pub val_size: usize,
    pub slot_size: usize, // 1 + key_size + val_size
}

pub type CmpFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;

// priority queue backed by binary heap
#[repr(C)]
pub struct MikuHeapQueue {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
    pub elem_size: usize,
    pub cmp: CmpFn,
}

#[repr(C)]
pub struct MikuList {
    pub head: *mut ListNode,
    pub tail: *mut ListNode,
    pub len: usize,
    pub elem_size: usize,
}

#[repr(C)]
pub struct ListNode {
    pub prev: *mut ListNode,
    pub next: *mut ListNode,
    // data follows immediately after this header
}

#[repr(C)]
pub struct MikuPool {
    pub data: *mut u8,         // backing storage
    pub generations: *mut u32, // generation per slot
    pub free_list: *mut u32,   // free slot indices
    pub obj_size: usize,
    pub capacity: usize,
    pub free_count: usize,
    pub active_count: usize,
}

// Pool handle: index + generation packed into u64
// upper 32 bits = generation, lower 32 bits = index
#[repr(C)]
#[derive(Copy, Clone)]
pub struct PoolHandle(pub u64);

pub type PoolIterFn = extern "C" fn(PoolHandle, *mut u8, *mut u8);

// queue structure
#[repr(C)]
pub struct MikuQueue {
    pub data: *mut u8,
    pub elem_size: usize,
    pub cap: usize,      // max number of elements
    pub head: usize,     // read position
    pub tail: usize,     // write position
    pub count: usize,
}

#[repr(C)]
pub struct MikuRingBuf {
    pub buf: *mut u8,
    pub cap: usize,    // total capacity in bytes
    pub head: usize,   // read position
    pub tail: usize,   // write position
}

// slab allocator for fixed-size objects
#[repr(C)]
pub struct MikuSlab {
    pub pool: *mut u8,
    pub free_head: *mut *mut u8, // pointer to first free slot's "next" pointer
    pub slot_size: usize,
    pub capacity: usize,
    pub in_use: usize,
}

// ordered map handle
#[repr(C)]
pub struct MikuTreeMap {
    pub root: *mut AvlNode,
    pub count: usize,
}

pub type TreeCallback = extern "C" fn(i64, u64, *mut u8);

// AVL tree node
#[repr(C)]
pub struct AvlNode {
    pub key: i64,
    pub val: u64,
    pub left: *mut AvlNode,
    pub right: *mut AvlNode,
    pub height: i32,
}

// trie structure //
#[repr(C)]
pub struct MikuTrie {
    pub nodes: *mut TrieNode,
    pub count: usize,
    pub cap: usize,
}

// single trie node //
#[repr(C)]
pub struct TrieNode {
    pub children: [u16; ALPHA_SIZE], // index into node pool, 0 = none
    pub is_end: bool,
    pub value: u64, // user-associated value
}

pub const ALPHA_SIZE: usize = 128; // ASCII range

#[repr(C)]
pub struct MikuVec {
    pub data: *mut u8,
    pub len: usize,       // number of elements
    pub cap: usize,       // capacity in elements
    pub elem_size: usize,  // size of one element in bytes
}

impl MikuMap {
    pub fn slot_ptr(&self, index: usize) -> *mut u8 {
        unsafe { self.slots.add(index * self.slot_size) }
    }

    pub fn slot_state(&self, index: usize) -> u8 {
        unsafe { *self.slot_ptr(index) }
    }

    pub fn slot_key(&self, index: usize) -> *const u8 {
        unsafe { self.slot_ptr(index).add(1) }
    }

    pub fn slot_val(&self, index: usize) -> *mut u8 {
        unsafe { self.slot_ptr(index).add(1 + self.key_size) }
    }

    pub fn set_slot_state(&self, index: usize, state: u8) {
        unsafe {
            *self.slot_ptr(index) = state;
        }
    }
}

impl ListNode {
    pub fn data_ptr(&self) -> *mut u8 {
        (self as *const ListNode as *mut u8).wrapping_add(core::mem::size_of::<ListNode>())
    }
}

impl PoolHandle {
    pub fn new(index: u32, gen: u32) -> Self {
        Self(((gen as u64) << 32) | (index as u64))
    }
    pub fn index(self) -> u32 { self.0 as u32 }
    pub fn generation(self) -> u32 { (self.0 >> 32) as u32 }
}
