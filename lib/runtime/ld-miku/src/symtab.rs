// Global symbol table: FNV-1a, open addressing with linear probing, `mmap`ed,
// doubling before half full

use crate::{syscall, util};

/// Initial slot count
const FIRST_SLOTS: usize = 8192;

#[derive(Clone, Copy)]
struct Sym {
    name:  *const u8,
    value: u64,
    weak:  bool,
}

const EMPTY: Sym = Sym { name: core::ptr::null(), value: 0, weak: false };

struct SymTab {
    slots: *mut Sym,
    /// A power of two, or zero before the first export
    size:  usize,
    count: usize,
}

unsafe impl Send for SymTab {}
unsafe impl Sync for SymTab {}

// FNV-1a hash for null-terminated C strings
fn fnv1a(name: *const u8) -> usize {
    use miku_base::fnv::{step_64, OFFSET_64};
    let mut h = OFFSET_64;
    let mut i = 0;
    unsafe {
        loop {
            let c = *name.add(i);
            if c == 0 { break; }
            h = step_64(h, c);
            i += 1;
        }
    }
    h as usize
}

/// Memory for `size` slots, every one empty, or null
fn slots_for(size: usize) -> *mut Sym {
    let bytes = size * core::mem::size_of::<Sym>();
    let p = syscall::mmap(0, bytes, syscall::PROT_READ | syscall::PROT_WRITE) as *mut Sym;
    if !p.is_null() {
        for i in 0..size {
            unsafe { p.add(i).write(EMPTY) };
        }
    }
    p
}

impl SymTab {
    const fn new() -> Self {
        Self { slots: core::ptr::null_mut(), size: 0, count: 0 }
    }

    fn slot(&self, i: usize) -> &mut Sym {
        // SAFETY: `i` is masked to below `size`
        unsafe { &mut *self.slots.add(i) }
    }

    /// Makes room for `more` symbols under half full. False when out of memory,
    /// leaving the table as is
    fn reserve(&mut self, more: usize) -> bool {
        let want = (self.count + more) * 2;
        if want <= self.size { return true; }
        let mut size = self.size.max(FIRST_SLOTS);
        while size < want { size *= 2; }
        let slots = slots_for(size);
        if slots.is_null() { return false; }

        let (old, old_size) = (self.slots, self.size);
        self.slots = slots;
        self.size = size;
        self.count = 0;
        for i in 0..old_size {
            let s = unsafe { *old.add(i) };
            if !s.name.is_null() {
                self.insert(s.name, s.value, s.weak);
            }
        }
        if !old.is_null() {
            syscall::munmap(old as *mut u8, old_size * core::mem::size_of::<Sym>());
        }
        true
    }

    /// Into a table known to have room
    fn insert(&mut self, name: *const u8, value: u64, weak: bool) {
        let mask = self.size - 1;
        let mut idx = fnv1a(name) & mask;
        loop {
            let slot = self.slot(idx);
            if slot.name.is_null() {
                *slot = Sym { name, value, weak };
                self.count += 1;
                return;
            }
            if util::streq(slot.name, name) {
                // Duplicate: updates only a weak one
                if slot.weak {
                    slot.value = value;
                    slot.weak  = weak;
                }
                return;
            }
            idx = (idx + 1) & mask;
        }
    }

    fn export(&mut self, name: *const u8, value: u64, weak: bool) {
        if name.is_null() { return; }
        if !self.reserve(1) {
            // Reported, since the program will fail to resolve this name
            util::note(b"[ld-miku] error: no memory to grow the symbol table; dropped ");
            util::noteln(util::cstr_to_bytes(name));
            return;
        }
        self.insert(name, value, weak);
    }

    fn lookup(&self, name: *const u8) -> u64 {
        if name.is_null() || self.size == 0 { return 0; }

        let mask = self.size - 1;
        let mut idx = fnv1a(name) & mask;
        // Never full, so an empty slot ends every probe
        loop {
            let slot = self.slot(idx);
            if slot.name.is_null() {
                return 0; // empty slot: not found
            }
            if util::streq(slot.name, name) {
                return slot.value;
            }
            idx = (idx + 1) & mask;
        }
    }
}

static mut SYMTAB: SymTab = SymTab::new();

/// Reserves for an object's `n` symbols, growing once
pub fn reserve(n: usize) {
    unsafe { core::ptr::addr_of_mut!(SYMTAB).as_mut().unwrap().reserve(n); }
}

pub fn export(name: *const u8, value: u64, weak: bool) {
    unsafe { core::ptr::addr_of_mut!(SYMTAB).as_mut().unwrap().export(name, value, weak); }
}

pub fn lookup(name: *const u8) -> u64 {
    unsafe { core::ptr::addr_of!(SYMTAB).as_ref().unwrap().lookup(name) }
}

pub fn count() -> usize {
    unsafe { core::ptr::addr_of!(SYMTAB).as_ref().unwrap().count }
}
