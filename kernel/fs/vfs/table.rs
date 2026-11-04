// The vnode table: as many as are cached, each in an object of its own
// (`dev/vfs-dentries-inodes.md`, "Stage 4: the table grows and shrinks")
//
// An id is an index into chunks of pointers, a chunk is added when every id in
// the others is taken, and a freed id is handed out again first. The object
// behind an id comes from a typed cache (`mm::heap::kcache`) and goes back to
// it when the id is freed, so the table costs what it caches.
//
// `Index` reads a free or never-made id as one static empty object, so a check
// of `nodes[id].active`, the shape of every lookup in the VFS, is false for it.
// Writing through a free id is a bug, reported as one

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::NonNull;

use crate::mm::heap::kcache::KCache;

/// Ids a chunk holds: a chunk is 4 KiB of pointers, one frame
const CHUNK: usize = 512;

pub struct Table<T: 'static> {
    chunks: Vec<Box<[Option<NonNull<T>>; CHUNK]>>,
    /// Ids freed, handed out again before a new one
    free: Vec<u32>,
    live: usize,
    /// Ids below this are never handed out: the VFS keeps id 0 unused
    first: u32,
    cache: &'static KCache<T>,
    empty: &'static T,
}

// SAFETY: the table owns its objects, and the VFS's lock is what every access
// goes through
unsafe impl<T: Send> Send for Table<T> {}

impl<T: 'static> Table<T> {
    pub const fn new(cache: &'static KCache<T>, empty: &'static T, first: u32) -> Self {
        Self { chunks: Vec::new(), free: Vec::new(), live: 0, first, cache, empty }
    }

    /// Ids with an object behind them
    pub fn len(&self) -> usize {
        self.live
    }

    #[inline]
    fn slot(&self, id: usize) -> Option<NonNull<T>> {
        self.chunks.get(id / CHUNK).and_then(|c| c[id % CHUNK])
    }

    /// Whether `id` has an object behind it
    #[inline]
    pub fn contains(&self, id: usize) -> bool {
        self.slot(id).is_some()
    }

    /// A new id with `value` behind it, or `None` when there is no memory
    pub fn alloc(&mut self, value: T) -> Option<usize> {
        if self.free.is_empty() {
            // A chunk more. Its ids go on the free stack highest first, so the
            // lowest is the next one out
            self.chunks.try_reserve(1).ok()?;
            self.free.try_reserve(CHUNK).ok()?;
            let base = self.chunks.len() * CHUNK;
            self.chunks.push(Box::new([None; CHUNK]));
            for id in (base.max(self.first as usize)..base + CHUNK).rev() {
                self.free.push(id as u32);
            }
        }
        let id = self.free.pop()? as usize;
        let Some(p) = self.cache.alloc(value) else {
            self.free.push(id as u32);
            return None;
        };
        self.chunks[id / CHUNK][id % CHUNK] = Some(p);
        self.live += 1;
        Some(id)
    }

    /// The object behind `id` back to its cache, and `id` free. Nothing for
    /// an id that is free already
    pub fn free(&mut self, id: usize) {
        let Some(chunk) = self.chunks.get_mut(id / CHUNK) else { return };
        let Some(p) = chunk[id % CHUNK].take() else { return };
        // SAFETY: the object came from this table's cache and the slot that
        // named it is gone
        unsafe { self.cache.free(p) };
        self.free.push(id as u32);
        self.live -= 1;
    }

    /// Every id with an object behind it, lowest first
    pub fn ids(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.live);
        for (c, chunk) in self.chunks.iter().enumerate() {
            for (i, s) in chunk.iter().enumerate() {
                if s.is_some() {
                    out.push(c * CHUNK + i);
                }
            }
        }
        out
    }

    /// Every object, lowest id first
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        // SAFETY: a slot's pointer is a live object the table owns
        self.chunks.iter().flat_map(|c| c.iter().flatten().map(|p| unsafe { p.as_ref() }))
    }
}

impl<T: 'static> core::ops::Index<usize> for Table<T> {
    type Output = T;
    #[inline]
    fn index(&self, id: usize) -> &T {
        match self.slot(id) {
            // SAFETY: a live object the table owns, borrowed with the table
            Some(p) => unsafe { p.as_ref() },
            None => self.empty,
        }
    }
}

impl<T: 'static> core::ops::IndexMut<usize> for Table<T> {
    #[inline]
    fn index_mut(&mut self, id: usize) -> &mut T {
        match self.slot(id) {
            // SAFETY: as `index`, and `&mut self` is the only way in
            Some(mut p) => unsafe { p.as_mut() },
            None => panic!("vfs: a write through vnode id {} that has no vnode", id),
        }
    }
}
