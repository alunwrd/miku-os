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

pub const PROT_READ:  u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC:  u32 = 4;

const MAP_FIXED: u32 = 0x10;

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
    vmas:    BTreeMap<u64, Vma>,
    pub brk: u64,
}

impl VmaMap {
    pub fn new() -> Self {
        Self { vmas: BTreeMap::new(), brk: BRK_BASE }
    }

    pub fn set_brk_base(&mut self, addr: u64) {
        self.brk = page_align_up(addr);
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
        // Advance cursor past any VMA whose tail overlaps MMAP_BASE
        let mut cursor = MMAP_BASE;
        if let Some((_, v)) = self.vmas.range(..=MMAP_BASE).next_back() {
            if v.end > MMAP_BASE {
                cursor = v.end;
            }
        }

        for (_, v) in self.vmas.range(cursor..) {
            if v.start >= MMAP_LIMIT { break; }
            if cursor + size <= v.start {
                return Some(cursor);
            }
            if v.end > cursor {
                cursor = v.end;
            }
        }

        if cursor + size <= MMAP_LIMIT { Some(cursor) } else { None }
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
        self.insert_merged(Vma { start: base, end: base + size, prot });
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
    with_vma(cr3, |m| m.insert(Vma { start, end, prot }));
}

pub fn vma_cleanup(cr3: u64) {
    VMA_MAP.lock().remove(&cr3);
}

pub fn vma_clone(src_cr3: u64, dst_cr3: u64) {
    let mut map = VMA_MAP.lock();
    if let Some(src) = map.get(&src_cr3) {
        let dst = VmaMap {
            vmas: src.vmas.clone(),
            brk:  src.brk,
        };
        map.insert(dst_cr3, dst);
    }
}

// page-table helpers //

#[inline]
fn page_align_up(addr: u64) -> u64 {
    (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Convert POSIX prot bits to x86-64 PTE flags
/// PRESENT is mandatory: without it every other flag is ignored by the CPU
fn prot_to_flags(prot: u32) -> PageTableFlags {
    let mut f = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if prot & PROT_WRITE != 0 { f |= PageTableFlags::WRITABLE; }
    if prot & PROT_EXEC  == 0 { f |= PageTableFlags::NO_EXECUTE; }
    f
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

    let size  = page_align_up(length);
    let pages = (size / PAGE_SIZE) as usize;
    let fixed = flags & MAP_FIXED != 0;

    let base = if fixed {
        if addr == 0 || addr & 0xFFF != 0 { return -22; }
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
        if !fixed {
            with_vma(cr3, |m| m.remove_range(base, base + size));
        }
        return -12;
    }

    if fixed {
        with_vma(cr3, |m| m.insert_merged(Vma { start: base, end: base + size, prot }));
    }

    crate::serial_println!("[mmap] {:#x}+{:#x} prot={:#x}", base, size, prot);
    base as i64
}

pub fn sys_munmap(cr3: u64, addr: u64, length: u64) -> i64 {
    if addr & 0xFFF != 0 { return -22; }
    let size  = page_align_up(length);
    let pages = (size / PAGE_SIZE) as usize;
    unmap_pages(cr3, addr, pages);
    with_vma(cr3, |m| m.remove_range(addr, addr + size));
    0
}

pub fn sys_mprotect(cr3: u64, addr: u64, length: u64, prot: u32) -> i64 {
    if addr & 0xFFF != 0 { return -22; }
    let size   = page_align_up(length);
    let flags  = prot_to_flags(prot);
    let aspace = AddressSpace::from_raw(cr3);
    let mut p  = addr;
    while p < addr + size {
        if let Some(phys) = aspace.virt_to_phys(p) {
            aspace.unmap_page_no_free(p);
            aspace.map_page(p, phys, flags);
        }
        p += PAGE_SIZE;
    }
    let _ = aspace.into_raw();
    // Keep VMA metadata in sync with the page table.
    with_vma(cr3, |m| {
        m.remove_range(addr, addr + size);
        m.insert_merged(Vma { start: addr, end: addr + size, prot });
    });
    0
}

pub fn sys_brk(cr3: u64, new_brk: u64) -> u64 {
    let cur = with_vma(cr3, |m| m.brk);
    if new_brk == 0 { return cur; }

    let new = page_align_up(new_brk);

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
