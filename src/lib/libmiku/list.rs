// doubly-linked list
// Heap-allocated nodes with arbitrary element size
// Provides O(1) push/pop at both ends, O(n) access by index

use crate::heap;
use crate::mem;

#[repr(C)]
struct ListNode {
    prev: *mut ListNode,
    next: *mut ListNode,
    // data follows immediately after this header
}

const NODE_HDR: usize = core::mem::size_of::<ListNode>();

impl ListNode {
    fn data_ptr(&self) -> *mut u8 {
        (self as *const ListNode as *mut u8).wrapping_add(NODE_HDR)
    }
}

#[repr(C)]
pub struct MikuList {
    head: *mut ListNode,
    tail: *mut ListNode,
    len: usize,
    elem_size: usize,
}

// internal: allocate a node with given data

unsafe fn alloc_node(elem: *const u8, elem_size: usize) -> *mut ListNode {
    let total = NODE_HDR + elem_size;
    let raw = heap::miku_malloc(total);
    if raw.is_null() { return core::ptr::null_mut(); }
    let node = raw as *mut ListNode;
    (*node).prev = core::ptr::null_mut();
    (*node).next = core::ptr::null_mut();
    if !elem.is_null() {
        mem::miku_memcpy((*node).data_ptr(), elem, elem_size);
    }
    node
}

unsafe fn free_node(node: *mut ListNode) {
    heap::miku_free(node as *mut u8);
}

// internal: get node at index

unsafe fn node_at(list: *const MikuList, index: usize) -> *mut ListNode {
    if index >= (*list).len { return core::ptr::null_mut(); }

    // traverse from closer end
    if index <= (*list).len / 2 {
        let mut node = (*list).head;
        for _ in 0..index { node = (*node).next; }
        node
    } else {
        let mut node = (*list).tail;
        for _ in 0..((*list).len - 1 - index) { node = (*node).prev; }
        node
    }
}

// lifecycle

#[no_mangle]
pub extern "C" fn miku_list_new(elem_size: usize) -> MikuList {
    MikuList {
        head: core::ptr::null_mut(),
        tail: core::ptr::null_mut(),
        len: 0,
        elem_size: if elem_size == 0 { 1 } else { elem_size },
    }
}

#[no_mangle]
pub extern "C" fn miku_list_free(l: *mut MikuList) {
    if l.is_null() { return; }
    unsafe {
        let mut node = (*l).head;
        while !node.is_null() {
            let next = (*node).next;
            free_node(node);
            node = next;
        }
        (*l).head = core::ptr::null_mut();
        (*l).tail = core::ptr::null_mut();
        (*l).len = 0;
    }
}

// accessors

#[no_mangle]
pub extern "C" fn miku_list_len(l: *const MikuList) -> usize {
    if l.is_null() { return 0; }
    unsafe { (*l).len }
}

#[no_mangle]
pub extern "C" fn miku_list_is_empty(l: *const MikuList) -> bool {
    if l.is_null() { return true; }
    unsafe { (*l).len == 0 }
}

// push_front: insert at beginning

#[no_mangle]
pub extern "C" fn miku_list_push_front(l: *mut MikuList, elem: *const u8) -> bool {
    if l.is_null() || elem.is_null() { return false; }
    unsafe {
        let node = alloc_node(elem, (*l).elem_size);
        if node.is_null() { return false; }

        (*node).next = (*l).head;
        if !(*l).head.is_null() {
            (*(*l).head).prev = node;
        } else {
            (*l).tail = node;
        }
        (*l).head = node;
        (*l).len += 1;
        true
    }
}

// push_back: insert at end

#[no_mangle]
pub extern "C" fn miku_list_push_back(l: *mut MikuList, elem: *const u8) -> bool {
    if l.is_null() || elem.is_null() { return false; }
    unsafe {
        let node = alloc_node(elem, (*l).elem_size);
        if node.is_null() { return false; }

        (*node).prev = (*l).tail;
        if !(*l).tail.is_null() {
            (*(*l).tail).next = node;
        } else {
            (*l).head = node;
        }
        (*l).tail = node;
        (*l).len += 1;
        true
    }
}

// pop_front: remove first element

#[no_mangle]
pub extern "C" fn miku_list_pop_front(l: *mut MikuList, out: *mut u8) -> bool {
    if l.is_null() { return false; }
    unsafe {
        if (*l).head.is_null() { return false; }

        let node = (*l).head;
        if !out.is_null() {
            mem::miku_memcpy(out, (*node).data_ptr(), (*l).elem_size);
        }

        (*l).head = (*node).next;
        if !(*l).head.is_null() {
            (*(*l).head).prev = core::ptr::null_mut();
        } else {
            (*l).tail = core::ptr::null_mut();
        }

        (*l).len -= 1;
        free_node(node);
        true
    }
}

// pop_back: remove last element

#[no_mangle]
pub extern "C" fn miku_list_pop_back(l: *mut MikuList, out: *mut u8) -> bool {
    if l.is_null() { return false; }
    unsafe {
        if (*l).tail.is_null() { return false; }

        let node = (*l).tail;
        if !out.is_null() {
            mem::miku_memcpy(out, (*node).data_ptr(), (*l).elem_size);
        }

        (*l).tail = (*node).prev;
        if !(*l).tail.is_null() {
            (*(*l).tail).next = core::ptr::null_mut();
        } else {
            (*l).head = core::ptr::null_mut();
        }

        (*l).len -= 1;
        free_node(node);
        true
    }
}

// get: pointer to element at index

#[no_mangle]
pub extern "C" fn miku_list_get(l: *const MikuList, index: usize) -> *const u8 {
    if l.is_null() { return core::ptr::null(); }
    unsafe {
        let node = node_at(l, index);
        if node.is_null() { return core::ptr::null(); }
        (*node).data_ptr()
    }
}

// set: overwrite element at index

#[no_mangle]
pub extern "C" fn miku_list_set(l: *mut MikuList, index: usize, elem: *const u8) -> bool {
    if l.is_null() || elem.is_null() { return false; }
    unsafe {
        let node = node_at(l, index);
        if node.is_null() { return false; }
        mem::miku_memcpy((*node).data_ptr(), elem, (*l).elem_size);
        true
    }
}

// insert: insert at specific index

#[no_mangle]
pub extern "C" fn miku_list_insert(l: *mut MikuList, index: usize, elem: *const u8) -> bool {
    if l.is_null() || elem.is_null() { return false; }
    unsafe {
        if index > (*l).len { return false; }

        if index == 0 { return miku_list_push_front(l, elem); }
        if index == (*l).len { return miku_list_push_back(l, elem); }

        let next_node = node_at(l, index);
        if next_node.is_null() { return false; }

        let new_node = alloc_node(elem, (*l).elem_size);
        if new_node.is_null() { return false; }

        let prev_node = (*next_node).prev;
        (*new_node).prev = prev_node;
        (*new_node).next = next_node;
        (*next_node).prev = new_node;
        if !prev_node.is_null() {
            (*prev_node).next = new_node;
        }

        (*l).len += 1;
        true
    }
}

// remove: remove element at index

#[no_mangle]
pub extern "C" fn miku_list_remove(l: *mut MikuList, index: usize) -> bool {
    if l.is_null() { return false; }
    unsafe {
        if index >= (*l).len { return false; }
        if index == 0 { return miku_list_pop_front(l, core::ptr::null_mut()); }
        if index == (*l).len - 1 { return miku_list_pop_back(l, core::ptr::null_mut()); }

        let node = node_at(l, index);
        if node.is_null() { return false; }

        let prev = (*node).prev;
        let next = (*node).next;
        if !prev.is_null() { (*prev).next = next; }
        if !next.is_null() { (*next).prev = prev; }

        (*l).len -= 1;
        free_node(node);
        true
    }
}

// clear: remove all elements

#[no_mangle]
pub extern "C" fn miku_list_clear(l: *mut MikuList) {
    if l.is_null() { return; }
    unsafe {
        let mut node = (*l).head;
        while !node.is_null() {
            let next = (*node).next;
            free_node(node);
            node = next;
        }
        (*l).head = core::ptr::null_mut();
        (*l).tail = core::ptr::null_mut();
        (*l).len = 0;
    }
}

// contains: linear search

#[no_mangle]
pub extern "C" fn miku_list_contains(l: *const MikuList, elem: *const u8) -> bool {
    if l.is_null() || elem.is_null() { return false; }
    unsafe {
        let mut node = (*l).head;
        while !node.is_null() {
            if mem::miku_memcmp((*node).data_ptr(), elem, (*l).elem_size) == 0 {
                return true;
            }
            node = (*node).next;
        }
        false
    }
}

// iter: call callback for each element

#[no_mangle]
pub extern "C" fn miku_list_iter(
    l: *const MikuList,
    cb: extern "C" fn(*const u8, usize, *mut u8),
    user_data: *mut u8,
) {
    if l.is_null() { return; }
    unsafe {
        let mut node = (*l).head;
        let mut idx = 0usize;
        while !node.is_null() {
            cb((*node).data_ptr(), idx, user_data);
            node = (*node).next;
            idx += 1;
        }
    }
}

// convenience: push/pop u64

#[no_mangle]
pub extern "C" fn miku_list_push_back_u64(l: *mut MikuList, val: u64) -> bool {
    miku_list_push_back(l, &val as *const u64 as *const u8)
}

#[no_mangle]
pub extern "C" fn miku_list_get_u64(l: *const MikuList, index: usize) -> u64 {
    let ptr = miku_list_get(l, index);
    if ptr.is_null() { return 0; }
    unsafe { *(ptr as *const u64) }
}

// find index of first element matching 'elem', returns -1 if not found
#[no_mangle]
pub extern "C" fn miku_list_index_of(l: *const MikuList, elem: *const u8) -> i32 {
    if l.is_null() || elem.is_null() { return -1; }
    unsafe {
        let mut node = (*l).head;
        let mut idx = 0i32;
        while !node.is_null() {
            if mem::miku_memcmp((*node).data_ptr(), elem, (*l).elem_size) == 0 {
                return idx;
            }
            node = (*node).next;
            idx += 1;
        }
        -1
    }
}

// reverse list in-place
#[no_mangle]
pub extern "C" fn miku_list_reverse(l: *mut MikuList) {
    if l.is_null() { return; }
    unsafe {
        let mut node = (*l).head;
        while !node.is_null() {
            let next = (*node).next;
            (*node).next = (*node).prev;
            (*node).prev = next;
            node = next;
        }
        let tmp = (*l).head;
        (*l).head = (*l).tail;
        (*l).tail = tmp;
    }
}

// remove all elements matching predicate
// Returns number of removed elements
#[no_mangle]
pub extern "C" fn miku_list_remove_if(
    l: *mut MikuList,
    pred: extern "C" fn(*const u8) -> bool,
) -> usize {
    if l.is_null() { return 0; }
    let mut removed = 0usize;
    unsafe {
        let mut node = (*l).head;
        while !node.is_null() {
            let next = (*node).next;
            if pred((*node).data_ptr()) {
                let prev = (*node).prev;
                if !prev.is_null() { (*prev).next = next; } else { (*l).head = next; }
                if !next.is_null() { (*next).prev = prev; } else { (*l).tail = prev; }
                (*l).len -= 1;
                free_node(node);
                removed += 1;
            }
            node = next;
        }
    }
    removed
}

// get front element pointer
#[no_mangle]
pub extern "C" fn miku_list_front(l: *const MikuList) -> *const u8 {
    if l.is_null() { return core::ptr::null(); }
    unsafe {
        if (*l).head.is_null() { return core::ptr::null(); }
        (*(*l).head).data_ptr()
    }
}

// get back element pointer
#[no_mangle]
pub extern "C" fn miku_list_back(l: *const MikuList) -> *const u8 {
    if l.is_null() { return core::ptr::null(); }
    unsafe {
        if (*l).tail.is_null() { return core::ptr::null(); }
        (*(*l).tail).data_ptr()
    }
}
