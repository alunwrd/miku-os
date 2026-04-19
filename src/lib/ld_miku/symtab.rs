// Global symbol table with hash-based lookup
//
// Uses FNV-1a hash for O(1) average lookup instead of O(n) linear scan
// Open addressing with linear probing - simple and cache-friendly

const HASH_BITS: usize = 13;              // 8192 slots
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: usize = HASH_SIZE - 1;

struct Sym {
    name:  *const u8,
    value: u64,
    weak:  bool,
}

struct SymTab {
    slots: [Sym; HASH_SIZE],
    count: usize,
}

unsafe impl Send for SymTab {}
unsafe impl Sync for SymTab {}

// FNV-1a hash for null-terminated C strings
fn fnv1a(name: *const u8) -> usize {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME:  u64 = 0x100000001b3;
    let mut h = FNV_OFFSET;
    let mut i = 0;
    unsafe {
        loop {
            let c = *name.add(i);
            if c == 0 { break; }
            h ^= c as u64;
            h = h.wrapping_mul(FNV_PRIME);
            i += 1;
        }
    }
    h as usize
}

impl SymTab {
    const fn new() -> Self {
        const EMPTY: Sym = Sym { name: core::ptr::null(), value: 0, weak: false };
        Self { slots: [EMPTY; HASH_SIZE], count: 0 }
    }

    fn export(&mut self, name: *const u8, value: u64, weak: bool) {
        if name.is_null() { return; }

        let hash = fnv1a(name);
        let mut idx = hash & HASH_MASK;

        // probe for existing entry or empty slot
        let mut probes = 0usize;
        while probes < HASH_SIZE {
            let slot = &self.slots[idx];
            if slot.name.is_null() {
                // empty slot - insert here
                self.slots[idx] = Sym { name, value, weak };
                self.count += 1;
                return;
            }
            if crate::util::streq(slot.name, name) {
                // duplicate - update only if existing is weak
                if self.slots[idx].weak {
                    self.slots[idx].value = value;
                    self.slots[idx].weak  = weak;
                }
                return;
            }
            idx = (idx + 1) & HASH_MASK;
            probes += 1;
        }
        // table full - should not happen with 8192 slots
    }

    fn lookup(&self, name: *const u8) -> u64 {
        if name.is_null() { return 0; }

        let hash = fnv1a(name);
        let mut idx = hash & HASH_MASK;

        let mut probes = 0usize;
        while probes < HASH_SIZE {
            let slot = &self.slots[idx];
            if slot.name.is_null() {
                return 0; // empty slot - not found
            }
            if crate::util::streq(slot.name, name) {
                return slot.value;
            }
            idx = (idx + 1) & HASH_MASK;
            probes += 1;
        }
        0
    }
}

static mut SYMTAB: SymTab = SymTab::new();

pub fn export(name: *const u8, value: u64, weak: bool) {
    unsafe { core::ptr::addr_of_mut!(SYMTAB).as_mut().unwrap().export(name, value, weak); }
}

pub fn lookup(name: *const u8) -> u64 {
    unsafe { core::ptr::addr_of!(SYMTAB).as_ref().unwrap().lookup(name) }
}

pub fn count() -> usize {
    unsafe { core::ptr::addr_of!(SYMTAB).as_ref().unwrap().count }
}
