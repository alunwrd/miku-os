// One cache of objects of one size: its slabs, their free lists, and the
// three lists a slab is on (`dev/kernel-core.md`, "Small objects: a cache for
// each size")
//
// This file knows no lock and no kernel. The caller holds the cache's lock,
// and pages come through `Pages`, so `kernel/mm/heap/hosttest` builds this
// file as it is and runs it against a model on the build host.
//
// A slab is 1 to 8 pages aligned to its own size. Objects fill it from the
// start, and its header is the last `HDR` bytes, so the slab of any object
// is its address with the low bits cleared and the header is found from
// that. Every class's slab order is chosen so the header fits in what the
// objects leave over, or costs one object in sixteen at most
// (`heap::CLASSES`).

pub const PAGE: usize = 4096;
/// The header at the end of every slab
pub const HDR: usize = 64;
/// Empty slabs a cache keeps for the next burst; more go back at once
pub const MIN_EMPTY: usize = 2;
/// What a free object is filled with under `slabdebug`, past its link
pub const POISON: u8 = 0x6b;

/// Where a cache's slabs come from
pub trait Pages {
    /// `count` pages (a power of two, at most 8) in one run aligned to its
    /// own size, or null
    ///
    /// # Safety
    /// What is handed out must stay readable and writable until `give`
    unsafe fn grab(&mut self, count: usize) -> *mut u8;
    /// Pages `grab` handed out, back
    ///
    /// # Safety
    /// `ptr` and `count` must be one earlier `grab`'s, and nothing may use
    /// the pages after
    unsafe fn give(&mut self, ptr: *mut u8, count: usize);
}

/// Which list a slab is on
const PARTIAL: u8 = 0;
const FULL: u8 = 1;
const EMPTY: u8 = 2;

/// Every slab header says this, XORed with its cache's object size: a free of
/// an object into the wrong cache (a layout not matching the one it was
/// allocated with) is caught at the header
const MAGIC: u32 = 0x51AB_0000;

#[repr(C)]
struct Slab {
    prev: *mut Slab,
    next: *mut Slab,
    /// The first free object, encoded (`Cache::link`); the end is encoded too
    free: usize,
    /// Objects out of this slab
    inuse: u32,
    magic: u32,
    list: u8,
}

const _: () = assert!(core::mem::size_of::<Slab>() <= HDR);

#[derive(Clone, Copy)]
struct List {
    head: *mut Slab,
    len: usize,
}

impl List {
    const fn new() -> Self {
        Self { head: core::ptr::null_mut(), len: 0 }
    }
}

pub struct Cache {
    pub name: &'static str,
    /// What one object takes in a slab
    pub size: usize,
    /// Pages a slab is
    pub pages: usize,
    /// Objects a slab holds
    pub per_slab: usize,
    /// Free-list links are stored XORed with this and with their own
    /// address, as Linux's `SLAB_FREELIST_HARDENED`: a stray write into a
    /// free object makes a link that fails the check, not one that hands
    /// out somebody else's memory
    secret: usize,
    partial: List,
    full: List,
    empty: List,
    /// Objects out of the slabs: with whoever asked, or in a magazine
    pub out: usize,
    /// `slabdebug`: a free object is poison past its link, and an allocation
    /// checks that it still is
    pub poison: bool,
    /// The last object found written after it was freed, for the caller to
    /// report once its lock is down; 0 for none
    pub breach: usize,
}

// SAFETY: the raw pointers are into slabs this cache owns, and the caller's
// lock is what every access goes through
unsafe impl Send for Cache {}

impl Cache {
    pub const fn new(name: &'static str, size: usize, pages: usize) -> Self {
        Self {
            name,
            size,
            pages,
            per_slab: (pages * PAGE - HDR) / size,
            secret: 0x9E37_79B9_7F4A_7C15,
            partial: List::new(),
            full: List::new(),
            empty: List::new(),
            out: 0,
            poison: false,
            breach: 0,
        }
    }

    pub fn set_secret(&mut self, secret: usize) {
        // Only before the first slab: links already written used the old one
        if self.slabs() == 0 {
            self.secret = secret | 1;
        }
    }

    /// Slabs the cache holds
    pub fn slabs(&self) -> usize {
        self.partial.len + self.full.len + self.empty.len
    }

    /// Slabs with nothing out
    pub fn empty_slabs(&self) -> usize {
        self.empty.len
    }

    fn bytes(&self) -> usize {
        self.pages * PAGE
    }

    fn base_of(&self, p: usize) -> usize {
        p & !(self.bytes() - 1)
    }

    fn header(&self, base: usize) -> *mut Slab {
        (base + self.bytes() - HDR) as *mut Slab
    }

    /// What is stored at `at` to say the next free object is `next` (0 for
    /// none)
    fn link(&self, next: usize, at: usize) -> usize {
        next ^ self.secret ^ at
    }

    /// The next free object after the one at `at`, checked: inside the same
    /// slab, on an object's boundary, or the end
    unsafe fn next_of(&self, at: usize, base: usize) -> usize {
        let next = unsafe { (at as *const usize).read() } ^ self.secret ^ at;
        if next != 0 && (next < base || next >= base + self.per_slab * self.size || (next - base) % self.size != 0) {
            panic!("heap: {}'s free list is broken at {:#x}: it says {:#x} is next", self.name, at, next);
        }
        next
    }

    unsafe fn list_of(&mut self, which: u8) -> &mut List {
        match which {
            PARTIAL => &mut self.partial,
            FULL => &mut self.full,
            _ => &mut self.empty,
        }
    }

    unsafe fn unlink(&mut self, s: *mut Slab) {
        unsafe {
            let l = self.list_of((*s).list);
            if (*s).prev.is_null() {
                l.head = (*s).next;
            } else {
                (*(*s).prev).next = (*s).next;
            }
            if !(*s).next.is_null() {
                (*(*s).next).prev = (*s).prev;
            }
            l.len -= 1;
        }
    }

    unsafe fn push(&mut self, s: *mut Slab, which: u8) {
        unsafe {
            let l = self.list_of(which);
            (*s).prev = core::ptr::null_mut();
            (*s).next = l.head;
            if !l.head.is_null() {
                (*l.head).prev = s;
            }
            l.head = s;
            l.len += 1;
            (*s).list = which;
        }
    }

    unsafe fn move_to(&mut self, s: *mut Slab, which: u8) {
        unsafe {
            if (*s).list != which {
                self.unlink(s);
                self.push(s, which);
            }
        }
    }

    /// A new slab, every object on its free list in address order
    unsafe fn grow(&mut self, src: &mut impl Pages) -> *mut Slab {
        let base = unsafe { src.grab(self.pages) } as usize;
        if base == 0 {
            return core::ptr::null_mut();
        }
        debug_assert!(base % self.bytes() == 0, "pages not aligned to the slab");
        let s = self.header(base);
        for i in 0..self.per_slab {
            let at = base + i * self.size;
            let next = if i + 1 < self.per_slab { at + self.size } else { 0 };
            unsafe {
                if self.poison {
                    self.fill_poison(at);
                }
                (at as *mut usize).write(self.link(next, at));
            }
        }
        unsafe {
            s.write(Slab {
                prev: core::ptr::null_mut(),
                next: core::ptr::null_mut(),
                free: base,
                inuse: 0,
                magic: MAGIC ^ self.size as u32,
                list: EMPTY,
            });
            self.push(s, EMPTY);
        }
        s
    }

    /// One object, or null when there are no pages for a slab
    ///
    /// # Safety
    /// The caller holds the cache's lock, and `src` is the source every slab
    /// of this cache came from
    pub unsafe fn alloc(&mut self, src: &mut impl Pages) -> *mut u8 {
        let mut s = self.partial.head;
        if s.is_null() {
            s = self.empty.head;
        }
        if s.is_null() {
            s = unsafe { self.grow(src) };
            if s.is_null() {
                return core::ptr::null_mut();
            }
        }
        unsafe {
            let at = (*s).free;
            let base = self.base_of(s as usize);
            (*s).free = self.next_of(at, base);
            if self.poison && !self.still_poison(at) {
                self.breach = at;
            }
            (*s).inuse += 1;
            self.out += 1;
            self.move_to(s, if (*s).free == 0 { FULL } else { PARTIAL });
            at as *mut u8
        }
    }

    /// An object back. A slab left empty beyond `MIN_EMPTY` goes back to
    /// `src` at once
    ///
    /// # Safety
    /// `p` came from this cache's `alloc` and is not used after; the caller
    /// holds the cache's lock
    pub unsafe fn free(&mut self, p: *mut u8, src: &mut impl Pages) {
        let at = p as usize;
        let base = self.base_of(at);
        let s = self.header(base);
        unsafe {
            if (*s).magic != MAGIC ^ self.size as u32 || (at - base) % self.size != 0 || (*s).inuse == 0 {
                panic!("heap: {:#x} is not an object of {}", at, self.name);
            }
            if self.poison {
                self.fill_poison(at);
            }
            (at as *mut usize).write(self.link((*s).free, at));
            (*s).free = at;
            (*s).inuse -= 1;
            self.out -= 1;
            if (*s).inuse == 0 {
                self.move_to(s, EMPTY);
                if self.empty.len > MIN_EMPTY {
                    self.release(s, src);
                }
            } else {
                // To the head of the partial list: the object just freed is
                // the next one handed out, while its cache lines are warm
                self.unlink(s);
                self.push(s, PARTIAL);
            }
        }
    }

    /// Poison over an object, past the word its link takes
    unsafe fn fill_poison(&self, at: usize) {
        let w = core::mem::size_of::<usize>();
        unsafe { core::ptr::write_bytes((at + w) as *mut u8, POISON, self.size - w) };
    }

    unsafe fn still_poison(&self, at: usize) -> bool {
        let w = core::mem::size_of::<usize>();
        let bytes = unsafe { core::slice::from_raw_parts((at + w) as *const u8, self.size - w) };
        bytes.iter().all(|&b| b == POISON)
    }

    unsafe fn release(&mut self, s: *mut Slab, src: &mut impl Pages) {
        unsafe {
            self.unlink(s);
            (*s).magic = 0;
            src.give(self.base_of(s as usize) as *mut u8, self.pages);
        }
    }

    /// Every empty slab back to `src`; how many pages went
    ///
    /// # Safety
    /// As `free`
    pub unsafe fn shrink(&mut self, src: &mut impl Pages) -> usize {
        let mut pages = 0;
        while !self.empty.head.is_null() {
            unsafe { self.release(self.empty.head, src) };
            pages += self.pages;
        }
        pages
    }

    /// Whether `p` is on its slab's free list already: a second free of it.
    /// A walk of the list, for `slabdebug`
    ///
    /// # Safety
    /// `p` points into one of this cache's slabs; the caller holds the lock
    pub unsafe fn is_free(&self, p: *mut u8) -> bool {
        let base = self.base_of(p as usize);
        let s = self.header(base);
        let mut at = unsafe { (*s).free };
        let mut steps = 0;
        while at != 0 && steps <= self.per_slab {
            if at == p as usize {
                return true;
            }
            at = unsafe { self.next_of(at, base) };
            steps += 1;
        }
        false
    }
}
