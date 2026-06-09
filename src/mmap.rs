extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::PageTableFlags;
use crate::vmm::AddressSpace;
use crate::pmm;

const PAGE_SIZE:  u64 = 4096;
const MMAP_BASE:  u64 = 0x0000_0001_0000_0000;
const MMAP_LIMIT: u64 = 0x0000_7F00_0000_0000;
const BRK_BASE:   u64 = 0x0000_0060_0000_0000;

// Upper bound (exclusive) of the canonical user half. Any user-supplied
// address+length range must satisfy end <= USER_END or the syscall is
// rejected; otherwise userspace could ask the kernel to manipulate
// kernel-half mappings (munmap on 0xFFFF_8000_... would unmap kernel)
const USER_END: u64 = 0x0000_8000_0000_0000;

pub const PROT_READ:  u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC:  u32 = 4;

const MAP_FIXED: u32 = 0x10;

// Reject any combination of prot bits we do not implement so a future
// flag we honour by accident never sneaks through
const PROT_VALID_MASK: u32 = PROT_READ | PROT_WRITE | PROT_EXEC;

// Returns true if [start, end) lies entirely within the user half and
// the arithmetic did not wrap
#[inline]
fn user_range_ok(start: u64, end: u64) -> bool {
    start < end && end <= USER_END
}

// Compute end = start + size with overflow check, then enforce user-half
#[inline]
fn user_end_for(start: u64, size: u64) -> Option<u64> {
    let end = start.checked_add(size)?;
    if !user_range_ok(start, end) { return None; }
    Some(end)
}

// Returns Some(aligned_size) iff length rounded up to a page doesn't wrap
#[inline]
fn checked_align_up(length: u64) -> Option<u64> {
    length.checked_add(PAGE_SIZE - 1).map(|v| v & !(PAGE_SIZE - 1))
}

// VMA //

#[derive(Copy, Clone, Debug)]
pub struct Vma {
    pub start: u64,
    pub end:   u64, // exclusive
    pub prot:  u32,
}

// VmaMap //

pub struct VmaMap {
    /// Keyed by start address - iteration yields VMAs in address order
    vmas:        BTreeMap<u64, Vma>,
    pub brk:     u64,
    /// Lower bound for brk: shrinking below this would unmap program
    /// data segments. Pinned by the ELF loader to `image.brk`
    pub brk_floor: u64,
}

impl VmaMap {
    pub fn new() -> Self {
        Self { vmas: BTreeMap::new(), brk: BRK_BASE, brk_floor: BRK_BASE }
    }

    pub fn set_brk_base(&mut self, addr: u64) {
        let aligned = page_align_up(addr);
        self.brk = aligned;
        self.brk_floor = aligned;
    }

    // insertion / removal //

    fn insert(&mut self, vma: Vma) {
        self.vmas.insert(vma.start, vma);
    }

    /// Remove all VMAs overlapping [s, e)
    /// VMA's only partially covered are split/trimmed so portions
    /// outside [s, e) are preserved
    fn remove_range(&mut self, s: u64, e: u64) {
        let keys: Vec<u64> = self.vmas
            .range(..e)
            .filter(|(_, v)| v.end > s)
            .map(|(k, _)| *k)
            .collect();

        for k in keys {
            let v = match self.vmas.remove(&k) {
                Some(v) => v,
                None    => continue,
            };
            if v.start < s {
                self.vmas.insert(v.start, Vma { start: v.start, end: s, prot: v.prot });
            }
            if v.end > e {
                self.vmas.insert(e, Vma { start: e, end: v.end, prot: v.prot });
            }
        }
    }

    // address-space allocation //
    fn find_free(&self, size: u64) -> Option<u64> {
        if size == 0 { return None; }
        // Advance cursor past any VMA whose tail overlaps MMAP_BASE
        let mut cursor = MMAP_BASE;
        if let Some((_, v)) = self.vmas.range(..=MMAP_BASE).next_back() {
            if v.end > MMAP_BASE {
                cursor = v.end;
            }
        }

        for (_, v) in self.vmas.range(cursor..) {
            if v.start >= MMAP_LIMIT { break; }
            // checked_add - cursor approaches MMAP_LIMIT; size may be
            // user-supplied and arbitrarily large. Overflow once means
            // no later (higher) cursor can possibly fit either; bail
            match cursor.checked_add(size) {
                Some(end) if end <= v.start => return Some(cursor),
                None => return None,
                _ => {}
            }
            if v.end > cursor {
                cursor = v.end;
            }
        }

        match cursor.checked_add(size) {
            Some(end) if end <= MMAP_LIMIT => Some(cursor),
            _ => None,
        }
    }

    // merging //
    fn insert_merged(&mut self, mut vma: Vma) {
        // Absorb preceding neighbour
        if let Some(prev) = self.vmas.range(..vma.start).next_back().map(|(_, v)| *v) {
            if prev.end == vma.start && prev.prot == vma.prot {
                self.vmas.remove(&prev.start);
                vma.start = prev.start;
            }
        }
        // Absorb following neighbour
        if let Some(next) = self.vmas.get(&vma.end).copied() {
            if next.prot == vma.prot {
                self.vmas.remove(&next.start);
                vma.end = next.end;
            }
        }
        self.vmas.insert(vma.start, vma);
    }

    fn find_and_insert(&mut self, size: u64, prot: u32) -> Option<u64> {
        let base = self.find_free(size)?;
        let end  = base.checked_add(size)?;
        self.insert_merged(Vma { start: base, end, prot });
        Some(base)
    }

    pub fn find(&self, addr: u64) -> Option<&Vma> {
        self.vmas
            .range(..=addr)
            .next_back()
            .map(|(_, v)| v)
            .filter(|v| v.end > addr)
    }
}

// global table //

static VMA_MAP: Mutex<BTreeMap<u64, VmaMap>> = Mutex::new(BTreeMap::new());

#[inline]
fn with_vma<F: FnOnce(&mut VmaMap) -> R, R>(cr3: u64, f: F) -> R {
    let mut map = VMA_MAP.lock();
    f(map.entry(cr3).or_insert_with(VmaMap::new))
}

// public helpers //

pub fn vma_set_brk(cr3: u64, brk_base: u64) {
    with_vma(cr3, |m| m.set_brk_base(brk_base));
}

pub fn kernel_find_free(cr3: u64, size: u64) -> Option<u64> {
    with_vma(cr3, |m| m.find_free(size))
}

pub fn kernel_register_vma(cr3: u64, start: u64, end: u64, prot: u32) {
    // Defensive: kernel callers should already have validated, but the
    // VmaMap relies on start < end for correctness of range queries
    if start >= end { return; }
    with_vma(cr3, |m| m.insert(Vma { start, end, prot }));
}

pub fn vma_cleanup(cr3: u64) {
    VMA_MAP.lock().remove(&cr3);
}

pub fn vma_clone(src_cr3: u64, dst_cr3: u64) {
    let mut map = VMA_MAP.lock();
    if let Some(src) = map.get(&src_cr3) {
        let dst = VmaMap {
            vmas:      src.vmas.clone(),
            brk:       src.brk,
            brk_floor: src.brk_floor,
        };
        map.insert(dst_cr3, dst);
    }
}

// page-table helpers //

#[inline]
fn page_align_up(addr: u64) -> u64 {
    (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Convert POSIX prot bits to x86-64 PTE flags. Enforces W^X: if the
/// user asks for WRITE+EXEC together we drop EXEC and force NO_EXECUTE.
/// RWX mappings are the most directly useful exploit primitive in a
/// ring-3 attack model, so the kernel never hands them out.
/// PRESENT is mandatory: without it every other flag is ignored by the CPU
fn prot_to_flags(prot: u32) -> PageTableFlags {
    let prot = prot & PROT_VALID_MASK;
    let want_exec = (prot & PROT_EXEC) != 0 && (prot & PROT_WRITE) == 0;
    let mut f = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if prot & PROT_WRITE != 0 { f |= PageTableFlags::WRITABLE; }
    if !want_exec { f |= PageTableFlags::NO_EXECUTE; }
    f
}

#[inline]
fn prot_rejects_wx(prot: u32) -> bool {
    (prot & PROT_WRITE) != 0 && (prot & PROT_EXEC) != 0
}

// physical-page helpers //

/// Allocate and zero-fill "pages" frames, mapping them at [base, base+pages*PAGE_SIZE)
fn map_fresh_pages(cr3: u64, base: u64, pages: usize, flags: PageTableFlags) -> Result<(), ()> {
    let hhdm   = crate::grub::hhdm();
    let aspace = AddressSpace::from_raw(cr3);
    let mut mapped = 0usize;

    for i in 0..pages {
        match pmm::alloc_frame() {
            Some(phys) => {
                unsafe {
                    core::ptr::write_bytes(
                        (phys + hhdm) as *mut u8, 0, PAGE_SIZE as usize,
                    );
                }
                if aspace.map_page(base + i as u64 * PAGE_SIZE, phys, flags) {
                    mapped += 1;
                } else {
                    pmm::free_frame(phys);
                    for j in 0..mapped {
                        aspace.unmap_page(base + j as u64 * PAGE_SIZE);
                    }
                    let _ = aspace.into_raw();
                    return Err(());
                }
            }
            None => {
                for j in 0..mapped {
                    aspace.unmap_page(base + j as u64 * PAGE_SIZE);
                }
                let _ = aspace.into_raw();
                return Err(());
            }
        }
    }
    let _ = aspace.into_raw();
    Ok(())
}

fn unmap_pages(cr3: u64, base: u64, pages: usize) {
    let aspace = AddressSpace::from_raw(cr3);
    for i in 0..pages {
        aspace.unmap_page(base + i as u64 * PAGE_SIZE);
    }
    let _ = aspace.into_raw();
}

// syscall implementations //

pub fn sys_mmap(
    cr3:    u64,
    addr:   u64,
    length: u64,
    prot:   u32,
    flags:  u32,
    _fd:    i64,
    _off:   u64,
) -> i64 {
    if length == 0 { return -22; }
    // Refuse explicit W+X at the syscall boundary, not silently downgrade.
    // prot_to_flags would have dropped EXEC anyway, but failing loudly is
    // better than a userspace program proceeding as if it had RWX
    if prot_rejects_wx(prot) || prot & !PROT_VALID_MASK != 0 { return -22; }

    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    let pages = (size / PAGE_SIZE) as usize;
    let fixed = flags & MAP_FIXED != 0;

    let base = if fixed {
        if addr == 0 || addr & 0xFFF != 0 { return -22; }
        if user_end_for(addr, size).is_none() { return -22; }
        unmap_pages(cr3, addr, pages);
        with_vma(cr3, |m| m.remove_range(addr, addr + size));
        addr
    } else {
        // find_and_insert: probe + register under one lock
        match with_vma(cr3, |m| m.find_and_insert(size, prot)) {
            Some(a) => a,
            None    => return -12,
        }
    };

    if map_fresh_pages(cr3, base, pages, prot_to_flags(prot)).is_err() {
        // clean up partially mapped pages
        unmap_pages(cr3, base, pages);
        // saturating_add - base+size validated above to fit in user half
        let end = base.saturating_add(size);
        if !fixed {
            with_vma(cr3, |m| m.remove_range(base, end));
        }
        return -12;
    }

    if fixed {
        let end = base.saturating_add(size);
        with_vma(cr3, |m| m.insert_merged(Vma { start: base, end, prot }));
    }

    crate::serial_println!("[mmap] {:#x}+{:#x} prot={:#x}", base, size, prot);
    base as i64
}

pub fn sys_munmap(cr3: u64, addr: u64, length: u64) -> i64 {
    if addr & 0xFFF != 0 { return -22; }
    if length == 0 { return 0; }
    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    // Reject kernel-half addresses; otherwise a malicious caller could
    // hand the kernel any VA and force unmap_page on it
    let end = match user_end_for(addr, size) {
        Some(e) => e,
        None    => return -22,
    };
    let pages = (size / PAGE_SIZE) as usize;
    unmap_pages(cr3, addr, pages);
    with_vma(cr3, |m| m.remove_range(addr, end));
    0
}

pub fn sys_mprotect(cr3: u64, addr: u64, length: u64, prot: u32) -> i64 {
    if addr & 0xFFF != 0 { return -22; }
    if length == 0 { return 0; }
    if prot_rejects_wx(prot) || prot & !PROT_VALID_MASK != 0 { return -22; }
    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    let end = match user_end_for(addr, size) {
        Some(e) => e,
        None    => return -22,
    };
    let flags  = prot_to_flags(prot);
    let aspace = AddressSpace::from_raw(cr3);
    let mut p  = addr;
    while p < end {
        if let Some(phys) = aspace.virt_to_phys(p) {
            aspace.unmap_page_no_free(p);
            aspace.map_page(p, phys, flags);
        }
        p += PAGE_SIZE;
    }
    let _ = aspace.into_raw();
    // update VMA table to match the new prot so subsequent lookups see the right flags
    with_vma(cr3, |m| {
        m.remove_range(addr, end);
        m.insert_merged(Vma { start: addr, end, prot });
    });
    0
}

pub fn sys_brk(cr3: u64, new_brk: u64) -> u64 {
    let (cur, floor) = with_vma(cr3, |m| (m.brk, m.brk_floor));
    if new_brk == 0 { return cur; }

    let new = match checked_align_up(new_brk) {
        Some(v) if v < USER_END => v,
        _ => return cur,
    };

    // Refuse to shrink below the floor pinned by exec; otherwise we'd
    // unmap pages owned by program data / heap arenas the process can't
    // recreate
    if new < floor { return cur; }

    if new <= cur {
        let pages = ((cur - new) / PAGE_SIZE) as usize;
        unmap_pages(cr3, new, pages);
        with_vma(cr3, |m| { m.remove_range(new, cur); m.brk = new; });
        return new;
    }

    let flags = PageTableFlags::PRESENT
              | PageTableFlags::WRITABLE
              | PageTableFlags::USER_ACCESSIBLE
              | PageTableFlags::NO_EXECUTE;
    let pages = ((new - cur) / PAGE_SIZE) as usize;

    if map_fresh_pages(cr3, cur, pages, flags).is_ok() {
        with_vma(cr3, |m| {
            m.insert_merged(Vma { start: cur, end: new, prot: PROT_READ | PROT_WRITE });
            m.brk = new;
        });
        new
    } else {
        cur
    }
}
