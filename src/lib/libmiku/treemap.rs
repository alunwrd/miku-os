// Ordered key-value map backed by AVL tree
//
// Keys are i64 for simplicity, Values are u64
// Self-balancing: O(log n) insert, lookup, remove
// In-order iteration gives sorted keys
// All nodes are heap-allocated

use crate::heap;
use crate::mem;

// AVL tree node
#[repr(C)]
struct AvlNode {
    key: i64,
    val: u64,
    left: *mut AvlNode,
    right: *mut AvlNode,
    height: i32,
}

// ordered map handle
#[repr(C)]
pub struct MikuTreeMap {
    root: *mut AvlNode,
    count: usize,
}

unsafe fn node_height(n: *const AvlNode) -> i32 {
    if n.is_null() { 0 } else { (*n).height }
}

unsafe fn update_height(n: *mut AvlNode) {
    let lh = node_height((*n).left);
    let rh = node_height((*n).right);
    (*n).height = 1 + if lh > rh { lh } else { rh };
}

unsafe fn balance_factor(n: *const AvlNode) -> i32 {
    if n.is_null() { 0 } else { node_height((*n).left) - node_height((*n).right) }
}

unsafe fn rotate_right(y: *mut AvlNode) -> *mut AvlNode {
    let x = (*y).left;
    let t2 = (*x).right;
    (*x).right = y;
    (*y).left = t2;
    update_height(y);
    update_height(x);
    x
}

unsafe fn rotate_left(x: *mut AvlNode) -> *mut AvlNode {
    let y = (*x).right;
    let t2 = (*y).left;
    (*y).left = x;
    (*x).right = t2;
    update_height(x);
    update_height(y);
    y
}

unsafe fn rebalance(n: *mut AvlNode) -> *mut AvlNode {
    update_height(n);
    let bf = balance_factor(n);

    // left-heavy
    if bf > 1 {
        if balance_factor((*n).left) < 0 {
            (*n).left = rotate_left((*n).left);
        }
        return rotate_right(n);
    }
    // right-heavy
    if bf < -1 {
        if balance_factor((*n).right) > 0 {
            (*n).right = rotate_right((*n).right);
        }
        return rotate_left(n);
    }
    n
}

unsafe fn new_node(key: i64, val: u64) -> *mut AvlNode {
    let p = heap::miku_malloc(core::mem::size_of::<AvlNode>());
    if p.is_null() {
        return core::ptr::null_mut();
    }
    let node = p as *mut AvlNode;
    (*node).key = key;
    (*node).val = val;
    (*node).left = core::ptr::null_mut();
    (*node).right = core::ptr::null_mut();
    (*node).height = 1;
    node
}

unsafe fn insert_node(
    node: *mut AvlNode,
    key: i64,
    val: u64,
    inserted: *mut bool,
) -> *mut AvlNode {
    if node.is_null() {
        let n = new_node(key, val);
        if !n.is_null() { *inserted = true; }
        return n;
    }
    if key < (*node).key {
        (*node).left = insert_node((*node).left, key, val, inserted);
    } else if key > (*node).key {
        (*node).right = insert_node((*node).right, key, val, inserted);
    } else {
        // duplicate key - update value
        (*node).val = val;
        return node;
    }
    rebalance(node)
}

unsafe fn find_min(mut n: *mut AvlNode) -> *mut AvlNode {
    while !(*n).left.is_null() {
        n = (*n).left;
    }
    n
}

unsafe fn remove_node(
    node: *mut AvlNode,
    key: i64,
    removed: *mut bool,
) -> *mut AvlNode {
    if node.is_null() {
        return core::ptr::null_mut();
    }
    if key < (*node).key {
        (*node).left = remove_node((*node).left, key, removed);
    } else if key > (*node).key {
        (*node).right = remove_node((*node).right, key, removed);
    } else {
        *removed = true;
        if (*node).left.is_null() || (*node).right.is_null() {
            let child = if !(*node).left.is_null() {
                (*node).left
            } else {
                (*node).right
            };
            heap::miku_free(node as *mut u8);
            return child;
        }
        // two children: replace with in-order successor
        let succ = find_min((*node).right);
        (*node).key = (*succ).key;
        (*node).val = (*succ).val;
        (*node).right = remove_node((*node).right, (*succ).key, &mut false);
    }
    rebalance(node)
}

unsafe fn free_tree(node: *mut AvlNode) {
    if node.is_null() {
        return;
    }
    free_tree((*node).left);
    free_tree((*node).right);
    heap::miku_free(node as *mut u8);
}

unsafe fn find_node(node: *const AvlNode, key: i64) -> *const AvlNode {
    if node.is_null() {
        return core::ptr::null();
    }
    if key < (*node).key {
        find_node((*node).left, key)
    } else if key > (*node).key {
        find_node((*node).right, key)
    } else {
        node
    }
}

type TreeCallback = extern "C" fn(i64, u64, *mut u8);

unsafe fn inorder(node: *const AvlNode, cb: TreeCallback, ctx: *mut u8) {
    if node.is_null() {
        return;
    }
    inorder((*node).left, cb, ctx);
    cb((*node).key, (*node).val, ctx);
    inorder((*node).right, cb, ctx);
}

// public API //

// create a new tree map
#[no_mangle]
pub extern "C" fn miku_tree_new() -> MikuTreeMap {
    MikuTreeMap {
        root: core::ptr::null_mut(),
        count: 0,
    }
}

// free tree map
#[no_mangle]
pub extern "C" fn miku_tree_free(t: *mut MikuTreeMap) {
    if t.is_null() {
        return;
    }
    unsafe {
        free_tree((*t).root);
        (*t).root = core::ptr::null_mut();
        (*t).count = 0;
    }
}

// insert or update key-value pair
#[no_mangle]
pub extern "C" fn miku_tree_insert(t: *mut MikuTreeMap, key: i64, val: u64) -> bool {
    if t.is_null() {
        return false;
    }
    unsafe {
        let mut inserted = false;
        (*t).root = insert_node((*t).root, key, val, &mut inserted);
        if inserted { (*t).count += 1; }
        true
    }
}

// look up value by key, returns pointer to value or null
#[no_mangle]
pub extern "C" fn miku_tree_get(t: *const MikuTreeMap, key: i64) -> *const u64 {
    if t.is_null() {
        return core::ptr::null();
    }
    unsafe {
        let node = find_node((*t).root, key);
        if node.is_null() {
            core::ptr::null()
        } else {
            &(*node).val as *const u64
        }
    }
}

// check if key exists
#[no_mangle]
pub extern "C" fn miku_tree_contains(t: *const MikuTreeMap, key: i64) -> bool {
    !miku_tree_get(t, key).is_null()
}

// remove key, returns true if found
#[no_mangle]
pub extern "C" fn miku_tree_remove(t: *mut MikuTreeMap, key: i64) -> bool {
    if t.is_null() {
        return false;
    }
    unsafe {
        let mut removed = false;
        (*t).root = remove_node((*t).root, key, &mut removed);
        if removed && (*t).count > 0 {
            (*t).count -= 1;
        }
        removed
    }
}

// number of entries
#[no_mangle]
pub extern "C" fn miku_tree_len(t: *const MikuTreeMap) -> usize {
    if t.is_null() {
        return 0;
    }
    unsafe { (*t).count }
}

// check if tree is empty
#[no_mangle]
pub extern "C" fn miku_tree_is_empty(t: *const MikuTreeMap) -> bool {
    if t.is_null() {
        return true;
    }
    unsafe { (*t).count == 0 }
}

// iterate in-order (sorted by key)
// callback: fn(key: i64, val: u64, ctx: *mut u8)
#[no_mangle]
pub extern "C" fn miku_tree_iter(
    t: *const MikuTreeMap,
    cb: TreeCallback,
    ctx: *mut u8,
) {
    if t.is_null() {
        return;
    }
    unsafe { inorder((*t).root, cb, ctx); }
}

// get minimum key, writes to *out_key and *out_val
#[no_mangle]
pub extern "C" fn miku_tree_min(
    t: *const MikuTreeMap,
    out_key: *mut i64,
    out_val: *mut u64,
) -> bool {
    if t.is_null() {
        return false;
    }
    unsafe {
        let root = (*t).root;
        if root.is_null() {
            return false;
        }
        let min = find_min(root as *mut AvlNode);
        if !out_key.is_null() { *out_key = (*min).key; }
        if !out_val.is_null() { *out_val = (*min).val; }
        true
    }
}

// get maximum key
#[no_mangle]
pub extern "C" fn miku_tree_max(
    t: *const MikuTreeMap,
    out_key: *mut i64,
    out_val: *mut u64,
) -> bool {
    if t.is_null() {
        return false;
    }
    unsafe {
        let mut n = (*t).root;
        if n.is_null() {
            return false;
        }
        while !(*n).right.is_null() {
            n = (*n).right;
        }
        if !out_key.is_null() { *out_key = (*n).key; }
        if !out_val.is_null() { *out_val = (*n).val; }
        true
    }
}

// clear all entries
#[no_mangle]
pub extern "C" fn miku_tree_clear(t: *mut MikuTreeMap) {
    if t.is_null() {
        return;
    }
    unsafe {
        free_tree((*t).root);
        (*t).root = core::ptr::null_mut();
        (*t).count = 0;
    }
}
