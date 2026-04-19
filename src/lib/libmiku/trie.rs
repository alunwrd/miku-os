// trie.rs - prefix trie for string lookups
//
// Fixed-capacity trie for fast prefix matching and autocomplete.
// Useful for command completion, dictionary lookup, routing tables.

use crate::heap;
use crate::mem;

const ALPHA_SIZE: usize = 128; // ASCII range
const MAX_TRIE_NODES: usize = 4096;

// single trie node //
#[repr(C)]
struct TrieNode {
    children: [u16; ALPHA_SIZE], // index into node pool, 0 = none
    is_end: bool,
    value: u64, // user-associated value
}

// trie structure //
#[repr(C)]
pub struct MikuTrie {
    nodes: *mut TrieNode,
    count: usize,
    cap: usize,
}

fn empty_node() -> TrieNode {
    TrieNode {
        children: [0u16; ALPHA_SIZE],
        is_end: false,
        value: 0,
    }
}

// create new trie //
#[no_mangle]
pub extern "C" fn miku_trie_new() -> MikuTrie {
    let size = core::mem::size_of::<TrieNode>() * MAX_TRIE_NODES;
    let ptr = heap::miku_malloc(size) as *mut TrieNode;
    if ptr.is_null() {
        return MikuTrie { nodes: core::ptr::null_mut(), count: 0, cap: 0 };
    }
    mem::miku_bzero(ptr as *mut u8, size);
    // node 0 is root
    unsafe { *ptr = empty_node(); }
    MikuTrie { nodes: ptr, count: 1, cap: MAX_TRIE_NODES }
}

// free trie //
#[no_mangle]
pub extern "C" fn miku_trie_free(t: *mut MikuTrie) {
    if t.is_null() { return; }
    let t = unsafe { &mut *t };
    if !t.nodes.is_null() {
        heap::miku_free(t.nodes as *mut u8);
        t.nodes = core::ptr::null_mut();
    }
    t.count = 0;
}

fn alloc_node(t: &mut MikuTrie) -> u16 {
    if t.count >= t.cap { return 0; }
    let idx = t.count;
    unsafe { *t.nodes.add(idx) = empty_node(); }
    t.count += 1;
    idx as u16
}

fn node_ref(t: &MikuTrie, idx: u16) -> &TrieNode {
    unsafe { &*t.nodes.add(idx as usize) }
}

fn node_mut(t: &mut MikuTrie, idx: u16) -> &mut TrieNode {
    unsafe { &mut *t.nodes.add(idx as usize) }
}

// insert key with associated value //
#[no_mangle]
pub extern "C" fn miku_trie_insert(
    t: *mut MikuTrie,
    key: *const u8,
    key_len: usize,
    value: u64,
) -> bool {
    if t.is_null() || key.is_null() { return false; }
    let t = unsafe { &mut *t };
    if t.nodes.is_null() { return false; }

    let mut cur = 0u16;
    for i in 0..key_len {
        let c = unsafe { *key.add(i) } as usize;
        if c >= ALPHA_SIZE { return false; }

        let child = node_ref(t, cur).children[c];
        if child == 0 {
            let new_idx = alloc_node(t);
            if new_idx == 0 { return false; }
            node_mut(t, cur).children[c] = new_idx;
            cur = new_idx;
        } else {
            cur = child;
        }
    }

    node_mut(t, cur).is_end = true;
    node_mut(t, cur).value = value;
    true
}

// search for exact key //
#[no_mangle]
pub extern "C" fn miku_trie_search(
    t: *const MikuTrie,
    key: *const u8,
    key_len: usize,
) -> bool {
    if t.is_null() || key.is_null() { return false; }
    let t = unsafe { &*t };
    if t.nodes.is_null() { return false; }

    let mut cur = 0u16;
    for i in 0..key_len {
        let c = unsafe { *key.add(i) } as usize;
        if c >= ALPHA_SIZE { return false; }
        let child = node_ref(t, cur).children[c];
        if child == 0 { return false; }
        cur = child;
    }
    node_ref(t, cur).is_end
}

// get value for key, returns 0 if not found //
#[no_mangle]
pub extern "C" fn miku_trie_get(
    t: *const MikuTrie,
    key: *const u8,
    key_len: usize,
) -> u64 {
    if t.is_null() || key.is_null() { return 0; }
    let t = unsafe { &*t };
    if t.nodes.is_null() { return 0; }

    let mut cur = 0u16;
    for i in 0..key_len {
        let c = unsafe { *key.add(i) } as usize;
        if c >= ALPHA_SIZE { return 0; }
        let child = node_ref(t, cur).children[c];
        if child == 0 { return 0; }
        cur = child;
    }
    if node_ref(t, cur).is_end { node_ref(t, cur).value } else { 0 }
}

// check if any key starts with prefix
#[no_mangle]
pub extern "C" fn miku_trie_has_prefix(
    t: *const MikuTrie,
    prefix: *const u8,
    prefix_len: usize,
) -> bool {
    if t.is_null() || prefix.is_null() { return false; }
    let t = unsafe { &*t };
    if t.nodes.is_null() { return false; }

    let mut cur = 0u16;
    for i in 0..prefix_len {
        let c = unsafe { *prefix.add(i) } as usize;
        if c >= ALPHA_SIZE { return false; }
        let child = node_ref(t, cur).children[c];
        if child == 0 { return false; }
        cur = child;
    }
    true
}

// remove key from trie (marks as non-terminal)
#[no_mangle]
pub extern "C" fn miku_trie_remove(
    t: *mut MikuTrie,
    key: *const u8,
    key_len: usize,
) -> bool {
    if t.is_null() || key.is_null() { return false; }
    let t = unsafe { &mut *t };
    if t.nodes.is_null() { return false; }

    let mut cur = 0u16;
    for i in 0..key_len {
        let c = unsafe { *key.add(i) } as usize;
        if c >= ALPHA_SIZE { return false; }
        let child = node_ref(t, cur).children[c];
        if child == 0 { return false; }
        cur = child;
    }

    if node_ref(t, cur).is_end {
        node_mut(t, cur).is_end = false;
        true
    } else {
        false
    }
}

// collect keys with prefix into buffer
// Returns number of matches written.
// Each match is written as: key bytes + null terminator.
// buf_ptr/buf_len: output buffer for concatenated results.
// max_results: max number of matches to collect.
#[no_mangle]
pub extern "C" fn miku_trie_prefix_collect(
    t: *const MikuTrie,
    prefix: *const u8,
    prefix_len: usize,
    buf: *mut u8,
    buf_len: usize,
    max_results: usize,
) -> usize {
    if t.is_null() || prefix.is_null() || buf.is_null() { return 0; }
    let t = unsafe { &*t };
    if t.nodes.is_null() { return 0; }

    // navigate to prefix node
    let mut cur = 0u16;
    for i in 0..prefix_len {
        let c = unsafe { *prefix.add(i) } as usize;
        if c >= ALPHA_SIZE { return 0; }
        let child = node_ref(t, cur).children[c];
        if child == 0 { return 0; }
        cur = child;
    }

    // DFS collect
    let mut state = CollectState {
        buf, buf_len, max_results,
        written: 0, buf_pos: 0,
        path: [0u8; 256],
        path_len: 0,
    };

    // copy prefix into path
    let plen = if prefix_len > 255 { 255 } else { prefix_len };
    unsafe { mem::miku_memcpy(state.path.as_mut_ptr(), prefix, plen); }
    state.path_len = plen;

    collect_dfs(t, cur, &mut state);
    state.written
}

struct CollectState {
    buf: *mut u8,
    buf_len: usize,
    max_results: usize,
    written: usize,
    buf_pos: usize,
    path: [u8; 256],
    path_len: usize,
}

fn collect_dfs(t: &MikuTrie, node: u16, st: &mut CollectState) {
    if st.written >= st.max_results { return; }

    if node_ref(t, node).is_end {
        // write path + null to buffer
        let needed = st.path_len + 1;
        if st.buf_pos + needed <= st.buf_len {
            unsafe {
                mem::miku_memcpy(st.buf.add(st.buf_pos), st.path.as_ptr(), st.path_len);
                *st.buf.add(st.buf_pos + st.path_len) = 0;
            }
            st.buf_pos += needed;
            st.written += 1;
        }
    }

    if st.path_len >= 255 { return; }

    for c in 0..ALPHA_SIZE {
        let child = node_ref(t, node).children[c];
        if child != 0 && st.written < st.max_results {
            st.path[st.path_len] = c as u8;
            st.path_len += 1;
            collect_dfs(t, child, st);
            st.path_len -= 1;
        }
    }
}

// number of allocated nodes
#[no_mangle]
pub extern "C" fn miku_trie_node_count(t: *const MikuTrie) -> usize {
    if t.is_null() { return 0; }
    unsafe { (*t).count }
}
