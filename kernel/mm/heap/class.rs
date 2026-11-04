// Which cache a layout goes to (`dev/kernel-core.md`, "Small objects: a cache
// for each size")
//
// The same answer on `alloc` and on `dealloc`: Rust hands `dealloc` the layout
// it allocated with, so a free never looks its cache up. Knows nothing of the
// kernel, so the host tests check every size and alignment.

/// A class: its object size, and the pages one of its slabs is. The order is
/// the smallest that leaves the 64-byte header room for at most one object
/// in sixteen (`slab::HDR`).
///
/// 4096 and 8192 are not classes: they are one and two frames exactly, and
/// the page path gives them that with nothing to waste
pub const CLASSES: [(usize, usize); 17] = [
    (8, 1), (16, 1), (32, 1), (48, 1), (64, 1), (96, 1), (128, 1), (192, 1),
    (256, 1), (384, 1), (512, 2), (768, 1), (1024, 4), (1536, 2), (2048, 8),
    (3072, 4), (6144, 8),
];

pub const NCLASSES: usize = CLASSES.len();

/// The largest object a class holds
pub const MAX_SIZE: usize = 6144;

/// What an object of a class is aligned to: objects sit at multiples of
/// their size from a slab aligned to its own size, so the lowest set bit of
/// the size
pub const fn natural_align(size: usize) -> usize {
    1 << size.trailing_zeros()
}

/// The first class for sizes up to 1024, in steps of 8: one look-up for nearly
/// every allocation
const SMALL: [u8; 129] = {
    let mut t = [0u8; 129];
    let mut i = 0;
    while i < 129 {
        let size = if i == 0 { 1 } else { i * 8 };
        let mut c = 0;
        while CLASSES[c].0 < size {
            c += 1;
        }
        t[i] = c as u8;
        i += 1;
    }
    t
};

/// The class of an object `size` bytes long aligned to `align`, or `None`
/// for one the page path takes: bigger than `MAX_SIZE`, or aligned to more
/// than any class's objects are
#[inline]
pub fn class_of(size: usize, align: usize) -> Option<usize> {
    if size > MAX_SIZE {
        return None;
    }
    let mut c = if size <= 1024 {
        SMALL[(size + 7) / 8] as usize
    } else {
        let mut c = SMALL[128] as usize;
        while CLASSES[c].0 < size {
            c += 1;
        }
        c
    };
    while natural_align(CLASSES[c].0) < align {
        c += 1;
        if c == NCLASSES {
            return None;
        }
    }
    Some(c)
}
