// What a check remembers while running
//
// Which blocks are claimed, which inodes live, and how many names point at
// each. Arrays by number would not fit: a terabyte at 4 KiB is 268 million
// blocks.
//
// As in e2fsck: a bitmap, a byte per eight blocks, laid out like the
// filesystem's so a group can be compared by memcmp. The link counter keeps one
// bit for "seen once" and one for "seen twice", covering files and childless
// directories, with a map for the rest.
//
// Each allocation uses `try_reserve`, so a check too large for memory fails
// with an answer and does not take the kernel down.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::error::FsError;

/// A bit per object in the filesystem's bitmap layout: bit `i` is
/// `1 << (i % 8)` of byte `i / 8`
pub struct BitMap {
    bytes: Vec<u8>,
    bits: u64,
}

impl BitMap {
    pub fn new(bits: u64) -> Result<Self, FsError> {
        let len = (bits as usize).div_ceil(8);
        let mut bytes: Vec<u8> = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| FsError::OutOfMemory)?;
        bytes.resize(len, 0);
        Ok(Self { bytes, bits })
    }

    pub fn bits(&self) -> u64 {
        self.bits
    }

    pub fn get(&self, i: u64) -> bool {
        if i >= self.bits {
            return false;
        }
        self.bytes[(i / 8) as usize] & (1 << (i % 8)) != 0
    }

    /// Sets a bit and says if it was set, which notices a second claim on a
    /// block as it happens
    pub fn set(&mut self, i: u64) -> bool {
        if i >= self.bits {
            return false;
        }
        let byte = &mut self.bytes[(i / 8) as usize];
        let mask = 1 << (i % 8);
        let was = *byte & mask != 0;
        *byte |= mask;
        was
    }

    pub fn clear(&mut self, i: u64) {
        if i >= self.bits {
            return;
        }
        self.bytes[(i / 8) as usize] &= !(1 << (i % 8));
    }

    /// How many bits are set in `[from, from + len)`
    pub fn count_range(&self, from: u64, len: u64) -> u64 {
        let mut n = 0u64;
        for i in from..(from + len).min(self.bits) {
            if self.get(i) {
                n += 1;
            }
        }
        n
    }

    /// Merges a run of bits from a buffer of the same layout, folding each
    /// group's metadata map into the filesystem-wide one
    pub fn or_bytes(&mut self, byte_offset: usize, src: &[u8]) {
        let end = (byte_offset + src.len()).min(self.bytes.len());
        for (i, b) in (byte_offset..end).enumerate() {
            self.bytes[b] |= src[i];
        }
    }

    /// Bytes covering `[from, from + len)` bits, to compare with a bitmap
    /// block. `from` and `len` are multiples of eight: a block group is
    /// `block_size * 8` blocks and an inode group a whole number of bytes wide
    pub fn byte_range(&self, from: u64, len: u64) -> &[u8] {
        let start = (from / 8) as usize;
        let end = (start + (len as usize).div_ceil(8)).min(self.bytes.len());
        &self.bytes[start.min(end)..end]
    }
}

/// Names pointing at each inode. Most inodes have one (a file) or two (a
/// directory, from its parent and its own `.`), at a bit each. Others go in the
/// map
pub struct LinkCounts {
    once: BitMap,
    twice: BitMap,
    many: BTreeMap<u32, u32>,
}

impl LinkCounts {
    pub fn new(inodes: u32) -> Result<Self, FsError> {
        Ok(Self {
            once: BitMap::new(inodes as u64 + 1)?,
            twice: BitMap::new(inodes as u64 + 1)?,
            many: BTreeMap::new(),
        })
    }

    pub fn inc(&mut self, ino: u32) {
        let i = ino as u64;
        if let Some(n) = self.many.get_mut(&ino) {
            *n += 1;
            return;
        }
        if self.twice.get(i) {
            self.twice.clear(i);
            self.many.insert(ino, 3);
            return;
        }
        if self.once.get(i) {
            self.once.clear(i);
            self.twice.set(i);
            return;
        }
        self.once.set(i);
    }

    pub fn dec(&mut self, ino: u32) {
        let i = ino as u64;
        if let Some(n) = self.many.get_mut(&ino) {
            *n -= 1;
            if *n == 2 {
                self.many.remove(&ino);
                self.twice.set(i);
            }
            return;
        }
        if self.twice.get(i) {
            self.twice.clear(i);
            self.once.set(i);
            return;
        }
        self.once.clear(i);
    }

    pub fn get(&self, ino: u32) -> u32 {
        let i = ino as u64;
        if self.once.get(i) {
            return 1;
        }
        if self.twice.get(i) {
            return 2;
        }
        *self.many.get(&ino).unwrap_or(&0)
    }
}

/// A directory's parent as it declares itself and as the tree says. Kept for
/// every directory, since reachability concerns them all at once
pub struct DirParents {
    /// Directory inode to the parent holding its name. Sorted by child, for a
    /// binary search during reachability
    entries: Vec<(u32, u32)>,
    /// The same, as `..` inside the directory claims it
    declared: Vec<(u32, u32)>,
    /// `entries` reversed, so the walk down asks for a directory's children
    by_parent: Vec<(u32, u32)>,
}

impl DirParents {
    pub fn new() -> Self {
        Self { entries: Vec::new(), declared: Vec::new(), by_parent: Vec::new() }
    }

    pub fn note_child(&mut self, child: u32, parent: u32) {
        self.entries.push((child, parent));
    }

    pub fn note_dotdot(&mut self, dir: u32, parent: u32) {
        self.declared.push((dir, parent));
    }

    /// Sorts once at the walk's end
    pub fn finish(&mut self) {
        self.entries.sort_unstable();
        self.declared.sort_unstable();
        self.by_parent.clear();
        self.by_parent.extend(self.entries.iter().map(|&(c, p)| (p, c)));
        self.by_parent.sort_unstable();
    }

    /// The directories `parent` holds the names of
    pub fn children_of(&self, parent: u32) -> &[(u32, u32)] {
        slice_of(&self.by_parent, parent)
    }

    /// Move a directory to a new parent, after it has been reconnected
    pub fn reparent(&mut self, child: u32, parent: u32) {
        self.entries.push((child, parent));
        self.entries.sort_unstable();
    }

    /// Every parent naming this directory. More than one is a hard-linked
    /// directory, which no operation can produce and which makes the tree a
    /// graph
    pub fn parents_of(&self, child: u32) -> &[(u32, u32)] {
        slice_of(&self.entries, child)
    }

    pub fn dotdot_of(&self, dir: u32) -> Option<u32> {
        slice_of(&self.declared, dir).first().map(|e| e.1)
    }

}

fn slice_of(v: &[(u32, u32)], key: u32) -> &[(u32, u32)] {
    let start = v.partition_point(|e| e.0 < key);
    let end = v.partition_point(|e| e.0 <= key);
    &v[start..end]
}

/// A fixed-size list that keeps counting when full. A badly damaged filesystem
/// can report a million of one fault: the number stays exact, and the examples
/// stop
pub struct Bounded<T> {
    items: Vec<T>,
    limit: usize,
    total: u32,
}

impl<T> Bounded<T> {
    pub fn new(limit: usize) -> Self {
        Self { items: Vec::new(), limit, total: 0 }
    }

    pub fn push(&mut self, item: T) {
        self.total += 1;
        if self.items.len() < self.limit {
            self.items.push(item);
        }
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn truncated(&self) -> u32 {
        self.total - self.items.len() as u32
    }
}

/// A block-sized scratch buffer on the heap, since a check holds several at
/// once on a kernel thread's stack
pub fn block_buf(block_size: u32) -> Vec<u8> {
    vec![0u8; block_size as usize]
}
