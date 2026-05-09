use crate::grub;
use crate::pmm;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{page_table::PageTableEntry, PageTable, PageTableFlags};

// page table index helpers //

#[inline(always)]
fn pt_index(virt: u64, level: u8) -> usize {
    ((virt >> (12 + 9 * level as u64)) & 0x1FF) as usize
}

// Walk page tables from P4 down to the P1 entry pointer for `virt`.
// Returns a mutable pointer to the raw PTE u64 in the P1 table, or None
// if any intermediate table is not present.
#[inline]
unsafe fn walk_to_pte(cr3: u64, virt: u64, hhdm: u64) -> Option<*mut u64> {
    let p4 = cr3.saturating_add(hhdm) as *const PageTable;
    let e4 = &(&*p4)[pt_index(virt, 3)];
    if !e4.flags().contains(PageTableFlags::PRESENT) { return None; }

    let p3 = e4.addr().as_u64().saturating_add(hhdm) as *const PageTable;
    let e3 = &(&*p3)[pt_index(virt, 2)];
    if !e3.flags().contains(PageTableFlags::PRESENT) { return None; }

    let p2 = e3.addr().as_u64().saturating_add(hhdm) as *const PageTable;
    let e2 = &(&*p2)[pt_index(virt, 1)];
    if !e2.flags().contains(PageTableFlags::PRESENT) { return None; }

    let p1_addr = e2.addr().as_u64().saturating_add(hhdm);
    if p1_addr == hhdm { return None; }

    let p1 = p1_addr as *mut PageTable;
    Some(&mut (&mut *p1)[pt_index(virt, 0)] as *mut _ as *mut u64)
}

pub struct AddressSpace {
    pub cr3: u64,
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        if self.cr3 == 0 || self.cr3 == kernel_cr3() { return; }
        self.free_address_space();
    }
}

impl AddressSpace {
    pub fn new_user() -> Option<Self> {
        let cr3 = pmm::alloc_frame()?;
        let hhdm = grub::hhdm();
        unsafe {
            let p4 = cr3.saturating_add(hhdm) as *mut PageTable;
            (*p4) = PageTable::new();
            let (kf, _) = Cr3::read();
            let kp4 = kf.start_address().as_u64().saturating_add(hhdm) as *const PageTable;
            for i in 256..512 {
                (&mut *p4)[i] = (&*kp4)[i].clone();
            }
        }
        Some(Self { cr3 })
    }

    pub fn into_raw(mut self) -> u64 {
        let cr3 = self.cr3;
        self.cr3 = 0;
        cr3
    }

    pub fn from_raw(cr3: u64) -> Self {
        Self { cr3 }
    }

    pub fn free_address_space_manual(&mut self) {
        if self.cr3 == 0 { return; }
        self.free_address_space();
        self.cr3 = 0;
    }

    pub fn free_address_space(&mut self) {
        if self.cr3 == 0 { return; }
        let hhdm = grub::hhdm();
        unsafe {
            let p4 = self.cr3.saturating_add(hhdm) as *mut PageTable;
            for i in 0..256 {
                if !(&*p4)[i].flags().contains(PageTableFlags::PRESENT) { continue; }
                let p3 = (&*p4)[i].addr().as_u64().saturating_add(hhdm) as *mut PageTable;
                for j in 0..512 {
                    if !(&*p3)[j].flags().contains(PageTableFlags::PRESENT) { continue; }
                    let p2 = (&*p3)[j].addr().as_u64().saturating_add(hhdm) as *mut PageTable;
                    for k in 0..512 {
                        if !(&*p2)[k].flags().contains(PageTableFlags::PRESENT) { continue; }
                        let p1 = (&*p2)[k].addr().as_u64().saturating_add(hhdm) as *mut PageTable;
                        for m in 0..512 {
                            let raw = &mut (&mut *p1)[m] as *mut _ as *mut u64;
                            let pte = *raw;
                            if crate::swap_map::is_swap_pte(pte) {
                                crate::swap::free_swap_slot(crate::swap_map::slot_from_pte(pte));
                            } else if (&*p1)[m].flags().contains(PageTableFlags::PRESENT) {
                                let phys = (&*p1)[m].addr().as_u64();
                                crate::swap_map::untrack(phys);
                                pmm::free_frame_cow(phys);
                            }
                        }
                        pmm::free_frame((&*p2)[k].addr().as_u64());
                    }
                    pmm::free_frame((&*p3)[j].addr().as_u64());
                }
                pmm::free_frame((&*p4)[i].addr().as_u64());
            }
        }
        pmm::free_frame(self.cr3);
        self.cr3 = 0;
    }

    pub fn map_page(&self, virt: u64, phys: u64, flags: PageTableFlags) -> bool {
        let hhdm = grub::hhdm();
        unsafe {
            let p4 = self.cr3.saturating_add(hhdm) as *mut PageTable;
            let Some(p3) = get_or_create(&mut (&mut *p4)[pt_index(virt, 3)], hhdm) else { return false; };
            let Some(p2) = get_or_create(&mut (&mut *p3)[pt_index(virt, 2)], hhdm) else { return false; };
            let Some(p1) = get_or_create(&mut (&mut *p2)[pt_index(virt, 1)], hhdm) else { return false; };
            (&mut *p1)[pt_index(virt, 0)].set_addr(
                x86_64::PhysAddr::new(phys),
                flags | PageTableFlags::PRESENT,
            );
            let pinned = virt >= 0xFFFF_8000_0000_0000 || phys < 0x40_0000;
            crate::swap_map::track(phys, self.cr3, virt, pinned);
        }
        true
    }

    pub fn map_range(&self, virt: u64, phys: u64, size: u64, flags: PageTableFlags) -> bool {
        let mut cv = virt & !0xFFF;
        let mut cp = phys & !0xFFF;
        let end = virt.saturating_add(size).saturating_add(0xFFF) & !0xFFF;
        while cv < end {
            if !self.map_page(cv, cp, flags) { return false; }
            cv += 4096;
            cp += 4096;
        }
        x86_64::instructions::tlb::flush_all();
        true
    }

    pub fn unmap_page(&self, virt: u64) {
        let hhdm = grub::hhdm();
        unsafe {
            let Some(pte_ptr) = walk_to_pte(self.cr3, virt, hhdm) else { return; };
            let pte = *pte_ptr;
            if crate::swap_map::is_swap_pte(pte) {
                crate::swap::free_swap_slot(crate::swap_map::slot_from_pte(pte));
            } else if pte & PTE_PRESENT != 0 {
                let phys = pte & PTE_ADDR_MASK;
                crate::swap_map::untrack(phys);
                pmm::free_frame(phys);
            }
            *pte_ptr = 0;
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(virt));
        }
    }

    pub fn unmap_page_no_free(&self, virt: u64) -> bool {
        let hhdm = grub::hhdm();
        unsafe {
            let Some(pte_ptr) = walk_to_pte(self.cr3, virt, hhdm) else { return false; };
            let pte = *pte_ptr;
            if pte & PTE_PRESENT == 0 { return false; }
            let phys = pte & PTE_ADDR_MASK;
            crate::swap_map::untrack(phys);
            *pte_ptr = 0;
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(virt));
            true
        }
    }

    pub fn read_pte_raw(&self, virt: u64) -> Option<u64> {
        let hhdm = grub::hhdm();
        unsafe {
            let pte_ptr = walk_to_pte(self.cr3, virt, hhdm)?;
            Some(*pte_ptr)
        }
    }

    pub fn get_page_flags(&self, virt: u64) -> Option<PageTableFlags> {
        let raw = self.read_pte_raw(virt)?;
        if raw & PTE_PRESENT == 0 { return None; }
        Some(PageTableFlags::from_bits_truncate(raw))
    }

    pub fn virt_to_phys(&self, virt: u64) -> Option<u64> {
        let raw = self.read_pte_raw(virt)?;
        if raw & PTE_PRESENT == 0 { return None; }
        Some((raw & PTE_ADDR_MASK) | (virt & 0xFFF))
    }

    pub unsafe fn mark_swapped(&self, virt: u64, slot: u32) {
        let hhdm = grub::hhdm();
        let pte_val = crate::swap_map::make_swap_pte(slot);
        unsafe {
            let Some(pte_ptr) = walk_to_pte(self.cr3, virt, hhdm) else { return; };
            *pte_ptr = pte_val;
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(virt));
        }
    }

    pub unsafe fn write_pte_raw(&self, virt: u64, pte_val: u64) {
        let hhdm = grub::hhdm();
        let Some(pte_ptr) = walk_to_pte(self.cr3, virt, hhdm) else { return; };
        *pte_ptr = pte_val;
    }

    /// Clone user address space with COW semantics
    pub fn clone_cow(&self) -> Option<Self> {
        let hhdm = grub::hhdm();
        let new_cr3 = pmm::alloc_frame()?;

        // Wrap new_cr3 in an AddressSpace so free_address_space is called on failure
        let mut dst = Self { cr3: new_cr3 };

        let ok = unsafe { self.clone_cow_inner(hhdm, &dst) };

        if ok {
            x86_64::instructions::tlb::flush_all();
            // Prevent Drop from freeing dst - transfer ownership
            let cr3 = dst.cr3;
            dst.cr3 = 0;
            Some(Self { cr3 })
        } else {
            // dst drops here, calling free_address_space to clean up partial clone
            None
        }
    }

    unsafe fn clone_cow_inner(&self, hhdm: u64, dst: &Self) -> bool {
        let new_cr3 = dst.cr3;
        let src_p4 = self.cr3.saturating_add(hhdm) as *mut PageTable;
        let dst_p4 = new_cr3.saturating_add(hhdm) as *mut PageTable;
        (*dst_p4) = PageTable::new();

        for i in 256..512 {
            (&mut *dst_p4)[i] = (&*src_p4)[i].clone();
        }

        for i in 0..256 {
            if !(&*src_p4)[i].flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            let src_p3_phys = (&*src_p4)[i].addr().as_u64();
            let src_p3 = src_p3_phys.saturating_add(hhdm) as *mut PageTable;

            let dst_p3_phys = match pmm::alloc_frame() {
                Some(f) => f,
                None => return false,
            };
            let dst_p3 = dst_p3_phys.saturating_add(hhdm) as *mut PageTable;
            (*dst_p3) = PageTable::new();
            (&mut *dst_p4)[i]
                .set_addr(x86_64::PhysAddr::new(dst_p3_phys), (&*src_p4)[i].flags());

            for j in 0..512 {
                if !(&*src_p3)[j].flags().contains(PageTableFlags::PRESENT) {
                    continue;
                }
                let src_p2_phys = (&*src_p3)[j].addr().as_u64();
                let src_p2 = src_p2_phys.saturating_add(hhdm) as *mut PageTable;

                let dst_p2_phys = match pmm::alloc_frame() {
                    Some(f) => f,
                    None => return false,
                };
                let dst_p2 = dst_p2_phys.saturating_add(hhdm) as *mut PageTable;
                (*dst_p2) = PageTable::new();
                (&mut *dst_p3)[j]
                    .set_addr(x86_64::PhysAddr::new(dst_p2_phys), (&*src_p3)[j].flags());

                for k in 0..512 {
                    if !(&*src_p2)[k].flags().contains(PageTableFlags::PRESENT) {
                        continue;
                    }
                    let src_p1_phys = (&*src_p2)[k].addr().as_u64();
                    let src_p1 = src_p1_phys.saturating_add(hhdm) as *mut PageTable;

                    let dst_p1_phys = match pmm::alloc_frame() {
                        Some(f) => f,
                        None => return false,
                    };
                    let dst_p1 = dst_p1_phys.saturating_add(hhdm) as *mut PageTable;
                    (*dst_p1) = PageTable::new();
                    (&mut *dst_p2)[k]
                        .set_addr(x86_64::PhysAddr::new(dst_p1_phys), (&*src_p2)[k].flags());

                    for m in 0..512 {
                        let src_raw = &mut (&mut *src_p1)[m] as *mut _ as *mut u64;
                        let pte_val = *src_raw;

                        if pte_val & PTE_PRESENT == 0 {
                            continue;
                        }
                        if crate::swap_map::is_swap_pte(pte_val) {
                            continue;
                        }

                        let phys = pte_val & PTE_ADDR_MASK;
                        let cow_pte = (pte_val & !PTE_WRITABLE) | PTE_COW;
                        *src_raw = cow_pte;

                        let dst_raw = &mut (&mut *dst_p1)[m] as *mut _ as *mut u64;
                        *dst_raw = cow_pte;

                        pmm::ref_inc(phys);

                        // Pin the frame in swap_map so it is never evicted
                        // while COW-shared (swap_map has one entry per frame,
                        // so we cannot track both parent and child mappings).
                        crate::swap_map::set_pinned(phys, true);
                    }
                }
            }
        }
        true
    }
}

unsafe fn get_or_create(entry: &mut PageTableEntry, hhdm: u64) -> Option<*mut PageTable> {
    if !entry.flags().contains(PageTableFlags::PRESENT) {
        let frame = pmm::alloc_frame()?;
        let table = frame.saturating_add(hhdm) as *mut PageTable;
        (*table) = PageTable::new();
        entry.set_addr(
            x86_64::PhysAddr::new(frame),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
        Some(table)
    } else {
        Some(entry.addr().as_u64().saturating_add(hhdm) as *mut PageTable)
    }
}

pub const PTE_COW: u64 = 1 << 9;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_PRESENT: u64 = 1;
pub const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

pub fn kernel_cr3() -> u64 {
    let (frame, _) = Cr3::read();
    frame.start_address().as_u64()
}

pub unsafe fn mark_swapped(cr3: u64, virt: u64, slot: u32) {
    let aspace = AddressSpace::from_raw(cr3);
    unsafe {
        aspace.mark_swapped(virt, slot);
    }
    let _ = aspace.into_raw();
}

pub unsafe fn mark_swapped_with_flags(cr3: u64, virt: u64, slot: u32, pte_flags: u64) {
    let hhdm = grub::hhdm();
    let aspace = AddressSpace::from_raw(cr3);
    let pte_val = crate::swap_map::make_swap_pte_with_flags(slot, pte_flags);
    unsafe {
        if let Some(pte_ptr) = walk_to_pte(aspace.cr3, virt, hhdm) {
            *pte_ptr = pte_val;
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(virt));
        }
    }
    let _ = aspace.into_raw();
}

pub fn read_pte_raw(cr3: u64, virt: u64) -> Option<u64> {
    let aspace = AddressSpace::from_raw(cr3);
    let result = aspace.read_pte_raw(virt);
    let _ = aspace.into_raw();
    result
}

// MMIO mapping at the HHDM window with uncacheable attributes. Needed for
// device registers where writes must hit silicon on the next write, not on
// an eventual cacheline eviction (LAPIC EOI, IOAPIC RTEs, PCIe BARs). On
// real hardware without an MTRR covering the MMIO range this is the only
// way to avoid losing writes into the L1/L2 cache.
fn alloc_pt_frame() -> u64 {
    let phys = pmm::alloc_frame()
        .expect("map_mmio_uc: out of physical memory for page table");
    let hhdm = grub::hhdm();
    unsafe {
        let ptr = (phys + hhdm) as *mut u8;
        core::ptr::write_bytes(ptr, 0, 4096);
    }
    phys
}

unsafe fn split_huge_p3(p3: &mut PageTable, p3_idx: usize, hhdm: u64) {
    let huge_phys = p3[p3_idx].addr().as_u64();
    let huge_flags = p3[p3_idx].flags();

    let new_p2_phys = alloc_pt_frame();
    let new_p2 = &mut *((new_p2_phys + hhdm) as *mut PageTable);

    for j in 0..512usize {
        let page_phys = huge_phys + (j as u64) * 0x20_0000;
        let mut flags = huge_flags;
        flags.remove(PageTableFlags::HUGE_PAGE);
        flags.insert(PageTableFlags::PRESENT | PageTableFlags::WRITABLE);

        let new_p1_phys = alloc_pt_frame();
        let new_p1 = &mut *((new_p1_phys + hhdm) as *mut PageTable);
        for k in 0..512usize {
            let phys_4k = page_phys + (k as u64) * 0x1000;
            new_p1[k].set_addr(
                x86_64::PhysAddr::new(phys_4k),
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
            );
        }
        new_p2[j].set_addr(x86_64::PhysAddr::new(new_p1_phys), flags);
    }

    p3[p3_idx].set_addr(
        x86_64::PhysAddr::new(new_p2_phys),
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    );
}

unsafe fn split_huge_p2(p2: &mut PageTable, p2_idx: usize, hhdm: u64) {
    let huge_phys = p2[p2_idx].addr().as_u64();
    let huge_flags = p2[p2_idx].flags();

    let new_p1_phys = alloc_pt_frame();
    let new_p1 = &mut *((new_p1_phys + hhdm) as *mut PageTable);

    for k in 0..512usize {
        let phys_4k = huge_phys + (k as u64) * 0x1000;
        let mut flags = huge_flags;
        flags.remove(PageTableFlags::HUGE_PAGE);
        flags.insert(PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
        new_p1[k].set_addr(x86_64::PhysAddr::new(phys_4k), flags);
    }

    p2[p2_idx].set_addr(
        x86_64::PhysAddr::new(new_p1_phys),
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    );
}

pub fn map_mmio_uc(phys_addr: u64, size: u64) {
    let hhdm = grub::hhdm();
    let start_page = phys_addr & !0xFFF;
    let end_page = (phys_addr + size + 0xFFF) & !0xFFF;

    unsafe {
        let (p4_frame, _) = Cr3::read();
        let p4 = &mut *((p4_frame.start_address().as_u64() + hhdm) as *mut PageTable);

        for page in (start_page..end_page).step_by(0x1000) {
            let virt = page + hhdm;
            let p4_idx = ((virt >> 39) & 0x1FF) as usize;
            let p3_idx = ((virt >> 30) & 0x1FF) as usize;
            let p2_idx = ((virt >> 21) & 0x1FF) as usize;
            let p1_idx = ((virt >> 12) & 0x1FF) as usize;

            if !p4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
                p4[p4_idx].set_addr(
                    x86_64::PhysAddr::new(alloc_pt_frame()),
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                );
            }
            let p3 = &mut *((p4[p4_idx].addr().as_u64() + hhdm) as *mut PageTable);

            if !p3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
                p3[p3_idx].set_addr(
                    x86_64::PhysAddr::new(alloc_pt_frame()),
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                );
            } else if p3[p3_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
                split_huge_p3(p3, p3_idx, hhdm);
                x86_64::instructions::tlb::flush_all();
            }
            let p2 = &mut *((p3[p3_idx].addr().as_u64() + hhdm) as *mut PageTable);

            if !p2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
                p2[p2_idx].set_addr(
                    x86_64::PhysAddr::new(alloc_pt_frame()),
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                );
            } else if p2[p2_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
                split_huge_p2(p2, p2_idx, hhdm);
                x86_64::instructions::tlb::flush_all();
            }
            let p1 = &mut *((p2[p2_idx].addr().as_u64() + hhdm) as *mut PageTable);

            // Strict UC (PCD=1, PWT=1 → default PAT index 3). Using only PCD
            // (NO_CACHE) yields UC- which MTRR=WB can downgrade back to WB on
            // real hardware, defeating the whole purpose for MMIO.
            p1[p1_idx].set_addr(
                x86_64::PhysAddr::new(page),
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::NO_CACHE
                    | PageTableFlags::WRITE_THROUGH,
            );
        }

        core::arch::asm!("mfence", options(nostack, nomem));
        x86_64::instructions::tlb::flush_all();
    }
}
