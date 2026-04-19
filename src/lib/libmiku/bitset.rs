// dynamic bitset backed by heap allocation
// Compact storage for boolean flags indexed by integer
// Each bit is individually addressable, the bitset grows
// automatically when setting a bit beyond current capacity

use crate::heap;
use crate::mem;

const BITS_PER_WORD: usize = 64;

// dynamic bitset
#[repr(C)]
pub struct MikuBitset {
    words: *mut u64,
    num_words: usize,
}

fn words_for_bits(n: usize) -> usize {
    (n + BITS_PER_WORD - 1) / BITS_PER_WORD
}

// create bitset with initial capacity for n bits
#[no_mangle]
pub extern "C" fn miku_bitset_new(nbits: usize) -> MikuBitset {
    let nw = if nbits == 0 { 1 } else { words_for_bits(nbits) };
    let ptr = heap::miku_calloc(nw, 8);
    MikuBitset {
        words: ptr as *mut u64,
        num_words: if ptr.is_null() { 0 } else { nw },
    }
}

// free bitset
#[no_mangle]
pub extern "C" fn miku_bitset_free(bs: *mut MikuBitset) {
    if bs.is_null() {
        return;
    }
    unsafe {
        if !(*bs).words.is_null() {
            heap::miku_free((*bs).words as *mut u8);
            (*bs).words = core::ptr::null_mut();
        }
        (*bs).num_words = 0;
    }
}

// grow bitset to hold at least nbits
unsafe fn ensure_capacity(bs: *mut MikuBitset, bit: usize) -> bool {
    let needed = words_for_bits(bit + 1);
    if needed <= (*bs).num_words {
        return true;
    }
    let new_words = needed * 2; // grow 2x to avoid frequent realloc
    let new_ptr = heap::miku_realloc(
        (*bs).words as *mut u8,
        new_words * 8,
    );
    if new_ptr.is_null() {
        return false;
    }
    // zero the new words
    let old_bytes = (*bs).num_words * 8;
    let new_bytes = new_words * 8;
    mem::miku_memset(new_ptr.add(old_bytes), 0, new_bytes - old_bytes);
    (*bs).words = new_ptr as *mut u64;
    (*bs).num_words = new_words;
    true
}

// set bit at position
#[no_mangle]
pub extern "C" fn miku_bitset_set(bs: *mut MikuBitset, bit: usize) -> bool {
    if bs.is_null() {
        return false;
    }
    unsafe {
        if !ensure_capacity(bs, bit) {
            return false;
        }
        let word = bit / BITS_PER_WORD;
        let offset = bit % BITS_PER_WORD;
        *(*bs).words.add(word) |= 1u64 << offset;
        true
    }
}

// clear bit at position
#[no_mangle]
pub extern "C" fn miku_bitset_clear(bs: *mut MikuBitset, bit: usize) {
    if bs.is_null() {
        return;
    }
    unsafe {
        let word = bit / BITS_PER_WORD;
        if word >= (*bs).num_words {
            return;
        }
        let offset = bit % BITS_PER_WORD;
        *(*bs).words.add(word) &= !(1u64 << offset);
    }
}

// test bit at position
#[no_mangle]
pub extern "C" fn miku_bitset_test(bs: *const MikuBitset, bit: usize) -> bool {
    if bs.is_null() {
        return false;
    }
    unsafe {
        let word = bit / BITS_PER_WORD;
        if word >= (*bs).num_words {
            return false;
        }
        let offset = bit % BITS_PER_WORD;
        (*(*bs).words.add(word) & (1u64 << offset)) != 0
    }
}

// toggle bit at position
#[no_mangle]
pub extern "C" fn miku_bitset_toggle(bs: *mut MikuBitset, bit: usize) -> bool {
    if bs.is_null() {
        return false;
    }
    unsafe {
        if !ensure_capacity(bs, bit) {
            return false;
        }
        let word = bit / BITS_PER_WORD;
        let offset = bit % BITS_PER_WORD;
        *(*bs).words.add(word) ^= 1u64 << offset;
        true
    }
}

// count number of set bits (popcount)
#[no_mangle]
pub extern "C" fn miku_bitset_count(bs: *const MikuBitset) -> usize {
    if bs.is_null() {
        return 0;
    }
    unsafe {
        let mut total = 0usize;
        for i in 0..(*bs).num_words {
            let w = *(*bs).words.add(i);
            // Kernighan's bit counting
            let mut v = w;
            while v != 0 {
                v &= v - 1;
                total += 1;
            }
        }
        total
    }
}

// clear all bits
#[no_mangle]
pub extern "C" fn miku_bitset_clear_all(bs: *mut MikuBitset) {
    if bs.is_null() {
        return;
    }
    unsafe {
        if !(*bs).words.is_null() {
            mem::miku_memset((*bs).words as *mut u8, 0, (*bs).num_words * 8);
        }
    }
}

// set all bits in range [0..nbits)
#[no_mangle]
pub extern "C" fn miku_bitset_set_all(bs: *mut MikuBitset, nbits: usize) {
    if bs.is_null() || nbits == 0 {
        return;
    }
    unsafe {
        if !ensure_capacity(bs, nbits - 1) {
            return;
        }
        let full_words = nbits / BITS_PER_WORD;
        for i in 0..full_words {
            *(*bs).words.add(i) = u64::MAX;
        }
        let rem = nbits % BITS_PER_WORD;
        if rem > 0 {
            *(*bs).words.add(full_words) = (1u64 << rem) - 1;
        }
    }
}

#[no_mangle]
pub extern "C" fn miku_bitset_or(dst: *mut MikuBitset, src: *const MikuBitset) {
    if dst.is_null() || src.is_null() {
        return;
    }
    unsafe {
        if (*src).num_words > (*dst).num_words {
            let max_bit = (*src).num_words * 64 - 1;
            if !ensure_capacity(dst, max_bit) {
                return; // OOM - do best-effort on existing capacity
            }
        }
        let n = (*src).num_words;
        for i in 0..n {
            *(*dst).words.add(i) |= *(*src).words.add(i);
        }
    }
}

#[no_mangle]
pub extern "C" fn miku_bitset_and(dst: *mut MikuBitset, src: *const MikuBitset) {
    if dst.is_null() || src.is_null() {
        return;
    }
    unsafe {
        let n = if (*dst).num_words < (*src).num_words {
            (*dst).num_words
        } else {
            (*src).num_words
        };
        for i in 0..n {
            *(*dst).words.add(i) &= *(*src).words.add(i);
        }
        // clear words beyond src
        for i in n..(*dst).num_words {
            *(*dst).words.add(i) = 0;
        }
    }
}

// bitwise XOR
#[no_mangle]
pub extern "C" fn miku_bitset_xor(dst: *mut MikuBitset, src: *const MikuBitset) {
    if dst.is_null() || src.is_null() {
        return;
    }
    unsafe {
        if (*src).num_words > (*dst).num_words {
            let max_bit = (*src).num_words * 64 - 1;
            if !ensure_capacity(dst, max_bit) {
                return;
            }
        }
        let n = (*src).num_words;
        for i in 0..n {
            *(*dst).words.add(i) ^= *(*src).words.add(i);
        }
    }
}

// find first set bit, returns -1 if none
#[no_mangle]
pub extern "C" fn miku_bitset_ffs(bs: *const MikuBitset) -> i64 {
    if bs.is_null() {
        return -1;
    }
    unsafe {
        for i in 0..(*bs).num_words {
            let w = *(*bs).words.add(i);
            if w != 0 {
                // count trailing zeros
                let mut bit = 0u32;
                let mut v = w;
                while v & 1 == 0 {
                    v >>= 1;
                    bit += 1;
                }
                return (i * BITS_PER_WORD + bit as usize) as i64;
            }
        }
        -1
    }
}

// check if bitset is all zeros
#[no_mangle]
pub extern "C" fn miku_bitset_is_empty(bs: *const MikuBitset) -> bool {
    if bs.is_null() {
        return true;
    }
    unsafe {
        for i in 0..(*bs).num_words {
            if *(*bs).words.add(i) != 0 {
                return false;
            }
        }
        true
    }
}

// capacity in bits
#[no_mangle]
pub extern "C" fn miku_bitset_capacity(bs: *const MikuBitset) -> usize {
    if bs.is_null() {
        return 0;
    }
    unsafe { (*bs).num_words * BITS_PER_WORD }
}
