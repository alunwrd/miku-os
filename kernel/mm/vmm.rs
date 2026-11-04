use crate::arch::x86_64::grub;
use crate::mm::pmm;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{page_table::PageTableEntry, PageTable, PageTableFlags};

// Where userspace ends and the kernel begins. usercopy's USER_MAX and mmap's
// USER_END are re-exports of these

/// The first page is never mapped for a user process. A NULL dereference
/// has to stay a fault; if page zero can be mapped, an ELF that asks for
/// p_vaddr = 0 quietly disarms every null check in userspace
/// (Linux calls the same knob mmap_min_addr)
pub const USER_MIN: u64 = 0x1000;

/// Exclusive top of the lower canonical half. Anything at or above this
/// is either non-canonical or kernel-owned
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// Inclusive last byte a user pointer may name, the form usercopy wants
pub const USER_MAX: u64 = USER_END - 1;

/// Base of the higher canonical half (the kernel's)
pub const KERNEL_HALF: u64 = 0xFFFF_8000_0000_0000;

#[inline]
pub fn is_user_va(va: u64) -> bool {
    va >= USER_MIN && va < USER_END
}

/// True when [va, va+len) lies wholly inside the user half. Rejects a
/// zero length and any range whose end wraps
#[inline]
pub fn is_user_range(va: u64, len: u64) -> bool {
    if len == 0 || va < USER_MIN {
        return false;
    }
    match va.checked_add(len) {
        Some(end) => end <= USER_END,
        None      => false,
    }
}

// page table index helpers //

#[inline(always)]
fn pt_index(virt: u64, level: u8) -> usize {
    ((virt >> (12 + 9 * level as u64)) & 0x1FF) as usize
}

// Walks page tables from P4 down to the P1 entry pointer for 'virt'. Returns a
// mutable pointer to the raw PTE u64 in the P1 table, or None if any
// intermediate table is not present.
//
// A PRESENT entry at the P3 or P2 level with HUGE_PAGE set is a 1 GiB or 2 MiB
// leaf, not a pointer to the next table. The kernel half and the HHDM are
// mapped with 1 GiB pages, and there is no 4 KiB PTE to return for a huge
// mapping, so this returns None for one
#[inline]
unsafe fn walk_to_pte(cr3: u64, virt: u64, hhdm: u64) -> Option<*mut u64> {
    let p4 = cr3.saturating_add(hhdm) as *const PageTable;
    let e4 = &(&*p4)[pt_index(virt, 3)];
    if !e4.flags().contains(PageTableFlags::PRESENT) { return None; }

    let p3 = e4.addr().as_u64().saturating_add(hhdm) as *const PageTable;
    let e3 = &(&*p3)[pt_index(virt, 2)];
    if !e3.flags().contains(PageTableFlags::PRESENT) { return None; }
    if e3.flags().contains(PageTableFlags::HUGE_PAGE) { return None; }

    let p2 = e3.addr().as_u64().saturating_add(hhdm) as *const PageTable;
    let e2 = &(&*p2)[pt_index(virt, 1)];
    if !e2.flags().contains(PageTableFlags::PRESENT) { return None; }
    if e2.flags().contains(PageTableFlags::HUGE_PAGE) { return None; }

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
            zero_table(p4);
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
        // Holds SWAP_MAP and PMM once for the entire teardown: 2 lock
        // acquisitions in place of 3*N
        let mut swap_guard = crate::mm::swap_map::lock_batch();
        let mut pmm_guard  = pmm::lock_for_batch();
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
                            if crate::mm::swap_map::is_swap_pte(pte) {
                                crate::mm::swap::free_swap_slot(crate::mm::swap_map::slot_from_pte(pte));
                            } else if (&*p1)[m].flags().contains(PageTableFlags::PRESENT) {
                                let phys = (&*p1)[m].addr().as_u64();
                                // CoW-aware: only untrack/free when this
                                // is the last reference; if a sibling is
                                // still alive, leave swap_map entry but
                                // clear pin (handled inside the helper)
                                pmm::free_frame_cow_swap(
                                    &mut pmm_guard,
                                    &mut swap_guard,
                                    phys,
                                );
                            }
                        }
                        pmm_guard.free_frame((&*p2)[k].addr().as_u64());
                    }
                    pmm_guard.free_frame((&*p3)[j].addr().as_u64());
                }
                pmm_guard.free_frame((&*p4)[i].addr().as_u64());
            }
        }
        pmm_guard.free_frame(self.cr3);
        drop(pmm_guard);
        drop(swap_guard);
        self.cr3 = 0;
    }

    /// True when this address space belongs to a process rather than the
    /// kernel. A user address space shares PML4[256..512] by value with the
    /// kernel root (see `new_user`), so writing a kernel-half mapping through
    /// one edits the live kernel tables for every CPU and every process, and
    /// `free_address_space` (which only walks 0..256) never takes it back.
    /// Every mapping call has to know which side of the split it may touch
    #[inline]
    fn is_user_space(&self) -> bool {
        self.cr3 != 0 && self.cr3 != kernel_cr3()
    }

    /// The guard behind that: a user address space may only map inside
    /// [USER_MIN, USER_END). A second line of defence: the ELF loader validates
    /// p_vaddr itself, but a bug there or in another caller stops here
    #[inline]
    fn may_map(&self, virt: u64, len: u64) -> bool {
        !self.is_user_space() || is_user_range(virt, len)
    }

    /// Maps `virt` only if nothing is mapped there yet, as one step.
    ///
    /// A demand fill (a lazy anonymous page, a page of a mapped file) reads the
    /// entry, fills a frame, and maps it. Two threads touching the same fresh
    /// page do all of that at once, and a plain `map_page` would replace the
    /// first mapping after its owner wrote to it. So the entry is swapped in
    /// only over the non-present value it still holds, and the loser keeps its
    /// frame to free. `Present` when another fill won; the caller's frame is
    /// then unused
    pub fn map_page_if_absent(&self, virt: u64, phys: u64, flags: PageTableFlags) -> MapOutcome {
        self.install_if_absent(virt, phys, flags, 0, true)
    }

    /// The same for a frame this address space does not own: a tmpfs page
    /// mapped `MAP_SHARED` (`mm::mmap::handle_file_fault`). The entry carries
    /// `PTE_SHARED`, so `fork` keeps it shared rather than copy-on-write, and
    /// the frame is not handed to the swapper, which would write out a page
    /// the file still holds. The caller has taken a reference for it
    pub fn map_shared_if_absent(&self, virt: u64, phys: u64, flags: PageTableFlags) -> MapOutcome {
        self.install_if_absent(virt, phys, flags, PTE_SHARED, false)
    }

    /// The same for a frame lent to this address space alone (`PTE_LENT`).
    /// The caller has taken a reference for it
    pub fn map_lent_if_absent(&self, virt: u64, phys: u64, flags: PageTableFlags) -> MapOutcome {
        self.install_if_absent(virt, phys, flags, PTE_LENT, false)
    }

    fn install_if_absent(
        &self,
        virt: u64,
        phys: u64,
        flags: PageTableFlags,
        software: u64,
        track: bool,
    ) -> MapOutcome {
        use core::sync::atomic::{AtomicU64, Ordering};
        if !self.may_map(virt, 4096) {
            crate::serial_println!(
                "[vmm] refusing user mapping at {:#x} (outside the user half)", virt,
            );
            return MapOutcome::Failed;
        }
        let user_tables = self.is_user_space();
        let hhdm = grub::hhdm();
        unsafe {
            let p4 = self.cr3.saturating_add(hhdm) as *mut PageTable;
            let Some(p3) = get_or_create(&mut (&mut *p4)[pt_index(virt, 3)], hhdm, user_tables) else { return MapOutcome::Failed; };
            let Some(p2) = get_or_create(&mut (&mut *p3)[pt_index(virt, 2)], hhdm, user_tables) else { return MapOutcome::Failed; };
            let Some(p1) = get_or_create(&mut (&mut *p2)[pt_index(virt, 1)], hhdm, user_tables) else { return MapOutcome::Failed; };
            let slot = AtomicU64::from_ptr(&mut (&mut *p1)[pt_index(virt, 0)] as *mut PageTableEntry as *mut u64);
            let seen = slot.load(Ordering::Acquire);
            if seen & PTE_PRESENT != 0 {
                return MapOutcome::Present;
            }
            let mut entry = PageTableEntry::new();
            entry.set_addr(x86_64::PhysAddr::new(phys), flags | PageTableFlags::PRESENT);
            let want = *(&entry as *const PageTableEntry as *const u64) | software;
            if slot.compare_exchange(seen, want, Ordering::AcqRel, Ordering::Acquire).is_err() {
                return MapOutcome::Present;
            }
            if track {
                let pinned = virt >= 0xFFFF_8000_0000_0000 || phys < 0x40_0000;
                crate::mm::swap_map::track(phys, self.cr3, virt, pinned);
            }
        }
        MapOutcome::Mapped
    }

    /// Changes a present page's protection where it stands.
    ///
    /// The frame, the software bits and accessed/dirty stay, so `PTE_COW` and
    /// `PTE_SHARED` survive; a CoW page stays read-only whatever it is asked
    /// for, and its fault makes it writable when copied. False when nothing is
    /// mapped at `virt`
    pub fn reprotect(&self, virt: u64, flags: PageTableFlags) -> bool {
        let hhdm = grub::hhdm();
        // Under `COW_LOCK`, like every rewrite of a present user entry (see
        // `take_page`)
        cow_locked(|| unsafe {
            let Some(pte_ptr) = walk_to_pte(self.cr3, virt, hhdm) else { return false };
            let raw = *pte_ptr;
            if raw & PTE_PRESENT == 0 {
                return false;
            }
            // The memory type too: PWT, PCD and PAT (bits 3, 4 and 7 of a
            // leaf). A write-combining window onto a device
            // (`mmap::map_mmio_into_user_wc`) put back to write-back by
            // `mprotect` would cache a device's memory. On an ordinary page all
            // three are clear
            const KEEP: u64 = PTE_ADDR_MASK | PTE_COW | PTE_SHARED | PTE_LENT | (1 << 5) | PTE_DIRTY
                | (1 << 3) | (1 << 4) | (1 << 7);
            let mut new = (raw & KEEP) | (flags | PageTableFlags::PRESENT).bits();
            if raw & PTE_COW != 0 {
                new &= !PTE_WRITABLE;
            }
            *pte_ptr = new;
            flush_va(virt);
            true
        })
    }

    pub fn map_page(&self, virt: u64, phys: u64, flags: PageTableFlags) -> bool {
        if !self.may_map(virt, 4096) {
            crate::serial_println!(
                "[vmm] refusing user mapping at {:#x} (outside the user half)", virt,
            );
            return false;
        }
        let user_tables = self.is_user_space();
        let hhdm = grub::hhdm();
        unsafe {
            let p4 = self.cr3.saturating_add(hhdm) as *mut PageTable;
            let Some(p3) = get_or_create(&mut (&mut *p4)[pt_index(virt, 3)], hhdm, user_tables) else { return false; };
            let Some(p2) = get_or_create(&mut (&mut *p3)[pt_index(virt, 2)], hhdm, user_tables) else { return false; };
            let Some(p1) = get_or_create(&mut (&mut *p2)[pt_index(virt, 1)], hhdm, user_tables) else { return false; };
            (&mut *p1)[pt_index(virt, 0)].set_addr(
                x86_64::PhysAddr::new(phys),
                flags | PageTableFlags::PRESENT,
            );
            let pinned = virt >= 0xFFFF_8000_0000_0000 || phys < 0x40_0000;
            crate::mm::swap_map::track(phys, self.cr3, virt, pinned);
        }
        true
    }

    pub fn map_range(&self, virt: u64, phys: u64, size: u64, flags: PageTableFlags) -> bool {
        // Two-level batching: one page-table walk per P1 table instead of per
        // page, and one SWAP_MAP acquisition for the whole range. On partial
        // failure (get_or_create OOM) every PTE already written is rolled back
        // via unmap_page_no_free, so the address space stays consistent
        let hhdm = grub::hhdm();
        let virt_start = virt & !0xFFF;
        let mut cv  = virt_start;
        let mut cp  = phys & !0xFFF;
        let end     = virt.saturating_add(size).saturating_add(0xFFF) & !0xFFF;
        if end <= virt_start { return false; }
        if !self.may_map(virt_start, end - virt_start) {
            crate::serial_println!(
                "[vmm] refusing user range {:#x}..{:#x} (outside the user half)",
                virt_start, end,
            );
            return false;
        }
        let user_tables = self.is_user_space();
        // An address space not loaded in CR3 has no TLB entries to shoot down.
        // exec builds the whole image in a fresh address space, so no flush_all
        // is needed there
        let is_current = self.cr3 == Cr3::read().0.start_address().as_u64();
        let flags_full = flags | PageTableFlags::PRESENT;
        let flags_bits = flags_full.bits();

        // Closure undoing everything written so far. swap_guard is dropped
        // before the rollback so unmap_page_no_free can take the global
        // swap_map lock through its normal API
        let rollback = |mapped_end: u64| {
            let mut v = virt_start;
            while v < mapped_end {
                self.unmap_page_no_free(v);
                v += 4096;
            }
            if is_current { x86_64::instructions::tlb::flush_all(); }
        };

        let mut swap_guard = crate::mm::swap_map::lock_batch();

        unsafe {
            let p4 = self.cr3.saturating_add(hhdm) as *mut PageTable;

            while cv < end {
                let Some(p3) = get_or_create(&mut (&mut *p4)[pt_index(cv, 3)], hhdm, user_tables) else {
                    drop(swap_guard); rollback(cv); return false;
                };
                let Some(p2) = get_or_create(&mut (&mut *p3)[pt_index(cv, 2)], hhdm, user_tables) else {
                    drop(swap_guard); rollback(cv); return false;
                };
                let Some(p1) = get_or_create(&mut (&mut *p2)[pt_index(cv, 1)], hhdm, user_tables) else {
                    drop(swap_guard); rollback(cv); return false;
                };

                let mut i = pt_index(cv, 0);
                while cv < end && i < 512 {
                    (&mut *p1)[i].set_addr(x86_64::PhysAddr::new(cp), flags_full);
                    let pinned = cv >= 0xFFFF_8000_0000_0000 || cp < 0x40_0000;
                    swap_guard.track(cp, self.cr3, cv, pinned, flags_bits);
                    cv += 4096;
                    cp += 4096;
                    i  += 1;
                }
            }
        }
        drop(swap_guard);
        if is_current { x86_64::instructions::tlb::flush_all(); }
        true
    }

    pub fn unmap_page(&self, virt: u64) {
        if let Some(phys) = self.take_page(virt) {
            pmm::free_frame(phys);
        }
    }

    /// Clears `virt`'s entry and returns the frame it mapped, not freed.
    ///
    /// For an unmap in an address space another processor may have loaded: the
    /// frame may be freed only once every processor has dropped its translation
    /// (`mm::tlb::shootdown`), and only this processor's is flushed here. A
    /// swapped-out page has no frame and no translation, so its slot goes back
    /// at once
    ///
    /// Under `COW_LOCK`. A present entry is rewritten in three places (here,
    /// `reprotect`, and `fork` marking it copy-on-write), each a read and then
    /// a store. A sibling thread's `munmap` could otherwise clear an entry
    /// `fork` had read, and `fork`'s store would put it back: the frame would
    /// be freed with both processes still mapping it
    pub fn take_page(&self, virt: u64) -> Option<u64> {
        let hhdm = grub::hhdm();
        cow_locked(|| unsafe {
            let pte_ptr = walk_to_pte(self.cr3, virt, hhdm)?;
            let pte = *pte_ptr;
            let mut frame = None;
            if crate::mm::swap_map::is_swap_pte(pte) {
                crate::mm::swap::free_swap_slot(crate::mm::swap_map::slot_from_pte(pte));
            } else if pte & PTE_PRESENT != 0 {
                let phys = pte & PTE_ADDR_MASK;
                crate::mm::swap_map::untrack(phys);
                frame = Some(phys);
            }
            *pte_ptr = 0;
            flush_va(virt);
            frame
        })
    }

    /// `take_page` over `pages` pages from `base`, each frame into `out`.
    ///
    /// The same per-entry work, but `COW_LOCK` is taken once per page table
    /// (512 entries), an entry never filled is left alone (no store, no
    /// `invlpg`), and a missing table skips to the next 2 MiB. A thread's stack
    /// is 2 MiB of which a few pages are touched
    pub fn take_range(&self, base: u64, pages: usize, out: &mut alloc::vec::Vec<u64>) {
        const SPAN: u64 = 512 * 4096;
        let hhdm = grub::hhdm();
        let end = base.saturating_add(pages as u64 * 4096);
        let mut va = base;
        while va < end {
            let chunk_end = ((va / SPAN) + 1).saturating_mul(SPAN).min(end);
            cow_locked(|| unsafe {
                while va < chunk_end {
                    let Some(pte_ptr) = walk_to_pte(self.cr3, va, hhdm) else {
                        va = chunk_end;
                        break;
                    };
                    let pte = *pte_ptr;
                    if pte != 0 {
                        if crate::mm::swap_map::is_swap_pte(pte) {
                            crate::mm::swap::free_swap_slot(crate::mm::swap_map::slot_from_pte(pte));
                        } else if pte & PTE_PRESENT != 0 {
                            let phys = pte & PTE_ADDR_MASK;
                            crate::mm::swap_map::untrack(phys);
                            out.push(phys);
                        }
                        *pte_ptr = 0;
                        flush_va(va);
                    }
                    va += 4096;
                }
            });
        }
    }

    pub fn unmap_page_no_free(&self, virt: u64) -> bool {
        let hhdm = grub::hhdm();
        // Under `COW_LOCK`: see `take_page`
        cow_locked(|| unsafe {
            let Some(pte_ptr) = walk_to_pte(self.cr3, virt, hhdm) else { return false; };
            let pte = *pte_ptr;
            if pte & PTE_PRESENT == 0 { return false; }
            let phys = pte & PTE_ADDR_MASK;
            crate::mm::swap_map::untrack(phys);
            *pte_ptr = 0;
            flush_va(virt);
            true
        })
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

    /// Copies out of a user address space that may not be the one loaded.
    ///
    /// Reads go through the page tables and the HHDM alias rather than the user
    /// mapping: it works for any CR3, and it does not touch a user address from
    /// ring 0, so SMAP stays armed. A page that is missing, not present or not
    /// user-owned ends the copy, and the caller gets a short count.
    ///
    /// Returns the number of bytes copied, always a prefix of `buf`.
    pub fn copy_from_user(&self, uva: u64, buf: &mut [u8]) -> usize {
        if buf.is_empty() || !is_user_range(uva, buf.len() as u64) {
            return 0;
        }
        let hhdm = grub::hhdm();
        let mut done = 0usize;
        while done < buf.len() {
            let cur = uva + done as u64;
            let page = cur & !0xFFF;
            let off  = (cur & 0xFFF) as usize;
            let chunk = (4096 - off).min(buf.len() - done);

            let Some(raw) = self.read_pte_raw(page) else { break };
            let flags = PageTableFlags::from_bits_truncate(raw);
            if !flags.contains(PageTableFlags::PRESENT)
                || !flags.contains(PageTableFlags::USER_ACCESSIBLE)
            {
                break;
            }
            let phys = raw & PTE_ADDR_MASK;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (phys + hhdm + off as u64) as *const u8,
                    buf.as_mut_ptr().add(done),
                    chunk,
                );
            }
            done += chunk;
        }
        done
    }

    pub unsafe fn mark_swapped(&self, virt: u64, slot: u32) {
        let hhdm = grub::hhdm();
        let pte_val = crate::mm::swap_map::make_swap_pte(slot);
        unsafe {
            let Some(pte_ptr) = walk_to_pte(self.cr3, virt, hhdm) else { return; };
            *pte_ptr = pte_val;
            flush_va(virt);
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

        // Wrap new_cr3 in an AddressSpace so free_address_space is called on
        // failure
        let mut dst = Self { cr3: new_cr3 };

        // Marking the parent's pages read-only changes translations its other
        // threads have cached writable on other processors. Their writes would
        // go on into frames the child now shares. So every processor is flushed
        // before the lock is released, and a break waits for the lock
        let ok = cow_locked(|| {
            let ok = unsafe { self.clone_cow_inner(hhdm, &dst) };
            if ok && crate::mm::tlb::cr3_loaded_elsewhere(self.cr3) {
                crate::mm::tlb::shootdown();
            }
            ok
        });

        if ok {
            x86_64::instructions::tlb::flush_all();
            // Prevent Drop from freeing dst: transfer ownership
            let cr3 = dst.cr3;
            dst.cr3 = 0;
            Some(Self { cr3 })
        } else {
            // dst drops here, calling free_address_space to clean up partial
            // clone
            None
        }
    }

    unsafe fn clone_cow_inner(&self, hhdm: u64, dst: &Self) -> bool {
        let new_cr3 = dst.cr3;
        let src_p4 = self.cr3.saturating_add(hhdm) as *mut PageTable;
        let dst_p4 = new_cr3.saturating_add(hhdm) as *mut PageTable;
        zero_table(dst_p4);

        for i in 256..512 {
            (&mut *dst_p4)[i] = (&*src_p4)[i].clone();
        }

        // Hold SWAP_MAP and PMM across the whole clone walk. set_pinned and
        // alloc_frame would otherwise take global locks per page and per PT
        // page
        let mut swap_guard = crate::mm::swap_map::lock_batch();
        let mut pmm_guard  = pmm::lock_for_batch();

        for i in 0..256 {
            if !(&*src_p4)[i].flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            let src_p3_phys = (&*src_p4)[i].addr().as_u64();
            let src_p3 = src_p3_phys.saturating_add(hhdm) as *mut PageTable;

            let dst_p3_phys = match pmm_guard.alloc_frame() {
                Some(f) => f,
                None => return false,
            };
            let dst_p3 = dst_p3_phys.saturating_add(hhdm) as *mut PageTable;
            zero_table(dst_p3);
            (&mut *dst_p4)[i]
                .set_addr(x86_64::PhysAddr::new(dst_p3_phys), (&*src_p4)[i].flags());

            for j in 0..512 {
                if !(&*src_p3)[j].flags().contains(PageTableFlags::PRESENT) {
                    continue;
                }
                let src_p2_phys = (&*src_p3)[j].addr().as_u64();
                let src_p2 = src_p2_phys.saturating_add(hhdm) as *mut PageTable;

                let dst_p2_phys = match pmm_guard.alloc_frame() {
                    Some(f) => f,
                    None => return false,
                };
                let dst_p2 = dst_p2_phys.saturating_add(hhdm) as *mut PageTable;
                zero_table(dst_p2);
                (&mut *dst_p3)[j]
                    .set_addr(x86_64::PhysAddr::new(dst_p2_phys), (&*src_p3)[j].flags());

                for k in 0..512 {
                    if !(&*src_p2)[k].flags().contains(PageTableFlags::PRESENT) {
                        continue;
                    }
                    let src_p1_phys = (&*src_p2)[k].addr().as_u64();
                    let src_p1 = src_p1_phys.saturating_add(hhdm) as *mut PageTable;

                    let dst_p1_phys = match pmm_guard.alloc_frame() {
                        Some(f) => f,
                        None => return false,
                    };
                    let dst_p1 = dst_p1_phys.saturating_add(hhdm) as *mut PageTable;
                    zero_table(dst_p1);
                    (&mut *dst_p2)[k]
                        .set_addr(x86_64::PhysAddr::new(dst_p1_phys), (&*src_p2)[k].flags());

                    for m in 0..512 {
                        let src_raw = &mut (&mut *src_p1)[m] as *mut _ as *mut u64;
                        let pte_val = *src_raw;

                        // A page on swap is an entry with the present bit
                        // clear, so this comes before the present check below.
                        // Both sides name the slot, the slot counts both, and
                        // each reads its own copy back on its first touch
                        if crate::mm::swap_map::is_swap_pte(pte_val) {
                            let slot = crate::mm::swap_map::slot_from_pte(pte_val);
                            if !crate::mm::swap::dup_swap_slot(slot) {
                                return false;
                            }
                            let dst_raw = &mut (&mut *dst_p1)[m] as *mut _ as *mut u64;
                            *dst_raw = pte_val;
                            continue;
                        }
                        if pte_val & PTE_PRESENT == 0 {
                            continue;
                        }

                        let phys = pte_val & PTE_ADDR_MASK;

                        // A lent frame is the parent's window and nobody
                        // else's. Made copy-on-write, the owner and the window
                        // hold two references, so the parent's next write
                        // always copies, and it draws into a private copy while
                        // the compositor reads the pool
                        if pte_val & PTE_LENT != 0 {
                            continue;
                        }

                        // A shared file page is the same frame in both: the
                        // child takes a reference and the entry as it is
                        if pte_val & PTE_SHARED != 0 {
                            let dst_raw = &mut (&mut *dst_p1)[m] as *mut _ as *mut u64;
                            *dst_raw = pte_val;
                            pmm::ref_inc(phys);
                            continue;
                        }

                        let cow_pte = (pte_val & !PTE_WRITABLE) | PTE_COW;
                        *src_raw = cow_pte;

                        let dst_raw = &mut (&mut *dst_p1)[m] as *mut _ as *mut u64;
                        *dst_raw = cow_pte;

                        pmm::ref_inc(phys);

                        // Pin the frame so it is never evicted while
                        // COW-shared (swap_map has one entry per frame,
                        // so it can't track both parent and child)
                        swap_guard.set_pinned(phys, true);
                    }
                }
            }
        }
        drop(pmm_guard);
        drop(swap_guard);
        true
    }
}

/// `user` decides whether a freshly created intermediate table is reachable
/// from ring 3.
///
/// A new table is swapped in over the empty entry rather than stored: two
/// threads faulting in the same fresh 2 MiB each build one, and a second store
/// would orphan the first table with what was mapped through it. The loser
/// frees its table and walks the winner's
unsafe fn get_or_create(
    entry: &mut PageTableEntry,
    hhdm: u64,
    user: bool,
) -> Option<*mut PageTable> {
    use core::sync::atomic::{AtomicU64, Ordering};
    let slot = AtomicU64::from_ptr(entry as *mut PageTableEntry as *mut u64);
    let seen = slot.load(Ordering::Acquire);
    if seen & PTE_PRESENT != 0 {
        return Some((seen & PTE_ADDR_MASK).saturating_add(hhdm) as *mut PageTable);
    }
    let frame = pmm::alloc_frame()?;
    let table = frame.saturating_add(hhdm) as *mut PageTable;
    zero_table(table);
    let mut f = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    if user { f |= PageTableFlags::USER_ACCESSIBLE; }
    let want = frame | f.bits();
    match slot.compare_exchange(seen, want, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Some(table),
        Err(winner) => {
            pmm::free_frame(frame);
            Some((winner & PTE_ADDR_MASK).saturating_add(hhdm) as *mut PageTable)
        }
    }
}

/// What `map_page_if_absent` did
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapOutcome {
    Mapped,
    /// Something was mapped there first; the frame offered was not used
    Present,
    Failed,
}

/// Invalidates one TLB entry. VirtAddr::new panics on a non-canonical address,
/// and addresses on these paths come from ELF files. Nothing is cached for an
/// address the CPU cannot form, so it is skipped
#[inline]
/// Makes the kernel's PML4 entry `slot` (256..512) point at a table of its own,
/// so every address space made after this shares what is mapped under it:
/// `AddressSpace::new_user` copies slots 256..512 by value, and a slot made
/// later would be missing from every existing space. For the heap's mapped area
/// (`mm::vmap`), at boot
pub fn share_kernel_slot(slot: usize) -> bool {
    debug_assert!((256..512).contains(&slot));
    let hhdm = grub::hhdm();
    unsafe {
        let p4 = kernel_cr3().saturating_add(hhdm) as *mut PageTable;
        get_or_create(&mut (&mut *p4)[slot], hhdm, false).is_some()
    }
}

/// Map one page of the kernel half in the kernel's own tables, out of the
/// swapper's sight: the heap's mapped area (`mm::vmap`). The PML4 entry above
/// it must be shared already (`share_kernel_slot`)
pub fn map_kernel_page(virt: u64, phys: u64, flags: PageTableFlags) -> bool {
    let hhdm = grub::hhdm();
    unsafe {
        let p4 = kernel_cr3().saturating_add(hhdm) as *mut PageTable;
        let e4 = &mut (&mut *p4)[pt_index(virt, 3)];
        if !e4.flags().contains(PageTableFlags::PRESENT) {
            return false;
        }
        let Some(p3) = get_or_create(e4, hhdm, false) else { return false; };
        let Some(p2) = get_or_create(&mut (&mut *p3)[pt_index(virt, 2)], hhdm, false) else { return false; };
        let Some(p1) = get_or_create(&mut (&mut *p2)[pt_index(virt, 1)], hhdm, false) else { return false; };
        (&mut *p1)[pt_index(virt, 0)].set_addr(x86_64::PhysAddr::new(phys), flags | PageTableFlags::PRESENT);
    }
    true
}

/// Clear one page `map_kernel_page` mapped and hand back its frame, not
/// freed. Only this processor's translation is flushed: the caller decides
/// when the others' are (`mm::vmap`)
pub fn unmap_kernel_page(virt: u64) -> Option<u64> {
    let hhdm = grub::hhdm();
    unsafe {
        let pte_ptr = walk_to_pte(kernel_cr3(), virt, hhdm)?;
        let pte = *pte_ptr;
        if pte & PTE_PRESENT == 0 {
            return None;
        }
        *pte_ptr = 0;
        flush_va(virt);
        Some(pte & PTE_ADDR_MASK)
    }
}

/// The frame a page `map_kernel_page` mapped is in
pub fn kernel_page_phys(virt: u64) -> Option<u64> {
    let hhdm = grub::hhdm();
    unsafe {
        let pte = *walk_to_pte(kernel_cr3(), virt, hhdm)?;
        (pte & PTE_PRESENT != 0).then_some((pte & PTE_ADDR_MASK) | (virt & 0xFFF))
    }
}

pub fn flush_va(virt: u64) {
    if let Ok(va) = x86_64::VirtAddr::try_new(virt) {
        x86_64::instructions::tlb::flush(va);
    }
}

pub const PTE_COW: u64 = 1 << 9;
/// A frame the address space maps but does not own (a tmpfs page mapped
/// `MAP_SHARED`), shared across `fork` rather than copied on write
pub const PTE_SHARED: u64 = 1 << 10;
/// A frame lent to this one address space by its owner: a Mayland shared-memory
/// pool, whose window the compositor reads at its physical address. `fork`
/// leaves it out of the child, the swapper never sees it, and the owner frees
/// it (`mm::mmap::map_phys_into_user`)
pub const PTE_LENT: u64 = 1 << 11;
/// Set by the processor when the page is written
pub const PTE_DIRTY: u64 = 1 << 6;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_PRESENT: u64 = 1;
pub const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// The kernel's own page-table root, latched once at boot. CR3 at call time
/// would be the current process's root, making every `cr3 == kernel_cr3()`
/// guard trivially true from a user context
static KERNEL_CR3: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Latch the running page-table root as "the kernel's". Must be called
/// during boot, before the first user process exists (kernel_main does)
pub fn latch_kernel_cr3() {
    let (frame, _) = Cr3::read();
    KERNEL_CR3.store(
        frame.start_address().as_u64(),
        core::sync::atomic::Ordering::Relaxed,
    );
}

pub fn kernel_cr3() -> u64 {
    let v = KERNEL_CR3.load(core::sync::atomic::Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    // Not latched yet: only possible during early boot, where the running
    // root IS the kernel's. Latch it now
    latch_kernel_cr3();
    KERNEL_CR3.load(core::sync::atomic::Ordering::Relaxed)
}

/// Breaks copy-on-write for one user page so it becomes privately writable.
/// Shared by the #PF handler (user write fault) and syscalls writing
/// out-parameters into user memory: after fork the parent's stack pages are
/// COW, and a kernel-side write does the same copy-or-unshare. Returns false
/// only on allocation failure
///
/// Under `COW_LOCK`, and shot down. Two threads can fault on the same page at
/// once: both would read a reference count of 2, copy, and drop a reference,
/// freeing the child's page under it, and the second would find a PTE already
/// writable. And the copy replaces the frame under every sibling with the old
/// read-only translation cached
pub fn break_cow_page(cr3: u64, va: u64) -> bool {
    let page_addr = va & !0xFFF;
    // Already private: nothing to break, and no lock to take to find out. Every
    // `read` on the machine checks its buffer here, and the machine-wide lock
    // would serialise them (`dev/vfs-locking.md`). The flush stays, for the
    // reason `break_cow_locked` gives
    if let Some(pte) = read_pte_raw(cr3, page_addr) {
        if pte & PTE_PRESENT != 0 && pte & PTE_COW == 0 {
            flush_va(page_addr);
            return true;
        }
    }
    cow_locked(|| break_cow_locked(cr3, page_addr))
}

/// One copy-on-write change to page tables at a time, machine-wide: a break
/// (`break_cow_page`) and a `fork` marking pages (`clone_cow`). A spin with
/// interrupts off, answering shootdowns while it waits, since the holder may be
/// in one, waiting for this processor
static COW_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// A fresh page-table page, zeroed in place. `*t = PageTable::new()` builds the
/// 4 KiB table on the stack and copies it in, which would give `clone_cow` a 20
/// KiB frame
#[inline(always)]
unsafe fn zero_table(t: *mut PageTable) {
    core::ptr::write_bytes(t as *mut u8, 0, core::mem::size_of::<PageTable>());
}

fn cow_locked<R>(f: impl FnOnce() -> R) -> R {
    use core::sync::atomic::Ordering;
    x86_64::instructions::interrupts::without_interrupts(|| {
        while COW_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            crate::mm::tlb::answer_pending();
            core::hint::spin_loop();
        }
        let r = f();
        COW_LOCK.store(false, Ordering::Release);
        r
    })
}

/// Makes one copy-on-write page privately writable. False only when there is no
/// frame to copy into.
///
/// A copy is not writable until every processor has let go of the old frame.
/// The copy moves the page to a new frame, and a sibling's processor may hold a
/// read-only translation to the old one. With the copy installed writable
/// before the shootdown, a thread on a flushed processor writes the new frame
/// while a thread on an unreached one reads the old. Linux closes the same
/// window by clearing the entry and flushing before the new one goes in.
///
/// So, in this order, all under `COW_LOCK`:
/// 1. copy the frame;
/// 2. point the entry at the copy, still read-only and still `PTE_COW`: a write
///    anywhere now faults and waits for this lock, and a read of either frame
///    reads the same bytes;
/// 3. flush every processor that has this address space loaded
///    (`tlb::cr3_loaded_elsewhere`), so none can reach the old frame;
/// 4. make the entry writable: a stale read-only translation now names the
///    copy, and its write's fault settles the permission;
/// 5. let go of the old frame, which the other holder may be about to free.
fn break_cow_locked(cr3: u64, page_addr: u64) -> bool {
    let pte_raw = match read_pte_raw(cr3, page_addr) {
        Some(p) => p,
        None => return false,
    };
    if pte_raw & PTE_PRESENT == 0 || pte_raw & PTE_COW == 0 {
        // Nothing to break (unmapped, or already private and writable by a
        // sibling that faulted first). A sibling that broke it by dropping the
        // bit sent no shootdown, so this processor may hold the read-only
        // translation: harmless in ring 3, whose fault is retried, and a kernel
        // crash for the syscall about to write through this pointer
        flush_va(page_addr);
        return true;
    }

    let old_phys = pte_raw & PTE_ADDR_MASK;
    let hhdm = grub::hhdm();
    let rc = pmm::ref_get(old_phys);
    let aspace = AddressSpace { cr3 };

    let ok = if rc > 1 {
        match pmm::alloc_frame() {
            Some(new_phys) => {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (old_phys + hhdm) as *const u8,
                        (new_phys + hhdm) as *mut u8,
                        4096,
                    );
                }
                let flags = pte_raw & !PTE_ADDR_MASK;
                let read_only = (new_phys & PTE_ADDR_MASK) | flags;
                unsafe { aspace.write_pte_raw(page_addr, read_only); }
                flush_va(page_addr);
                if crate::mm::tlb::cr3_loaded_elsewhere(cr3) {
                    crate::mm::tlb::shootdown();
                }
                let writable = (new_phys & PTE_ADDR_MASK) | (flags & !PTE_COW) | PTE_WRITABLE;
                unsafe { aspace.write_pte_raw(page_addr, writable); }
                flush_va(page_addr);
                crate::mm::swap_map::track(new_phys, cr3, page_addr, false);
                // Ours goes now, and the frame with it if the other holder let
                // go while this copied (see `pmm::put_cow_ref`)
                match pmm::put_cow_ref(old_phys) {
                    pmm::CowPut::Shared => {}
                    pmm::CowPut::Unique => crate::mm::swap_map::set_pinned(old_phys, false),
                    pmm::CowPut::Last => {
                        crate::mm::swap_map::untrack(old_phys);
                        pmm::free_frame(old_phys);
                    }
                }
                true
            }
            None => false,
        }
    } else {
        // Last reference: drops the COW bit and makes it writable. The frame
        // stays, so a sibling's read-only translation is stale only in its
        // permission, which its next write's fault settles
        let new_pte = (pte_raw & !PTE_COW) | PTE_WRITABLE;
        unsafe { aspace.write_pte_raw(page_addr, new_pte); }
        crate::mm::swap_map::set_pinned(old_phys, false);
        flush_va(page_addr);
        true
    };
    core::mem::forget(aspace);
    ok
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
    let pte_val = crate::mm::swap_map::make_swap_pte_with_flags(slot, pte_flags);
    unsafe {
        if let Some(pte_ptr) = walk_to_pte(aspace.cr3, virt, hhdm) {
            *pte_ptr = pte_val;
            flush_va(virt);
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

/// Puts write-combining into the page-attribute table, on this CPU.
///
/// x86 has no write-combining memory type reachable from a page-table entry
/// until IA32_PAT says so. The power-on table is `WB WT UC- UC WB WT UC- UC`
/// for indices 0..7. Slot 4, selected by the PAT bit alone with PCD and PWT
/// clear, is redefined to WC, as Linux does.
///
/// A display surface written through an aperture with four-byte uncached stores
/// costs one PCIe transaction per pixel, as PRAMIN does. Write-combining
/// gathers a full 64-byte burst before anything crosses the bus, which is the
/// difference between BAR1 and PRAMIN.
///
/// Must run on every CPU: the PAT is per-core state
pub fn init_pat() {
    const IA32_PAT: u32 = 0x277;
    // WB WT UC- UC | WC WT UC- UC: only entry 4 changes, from WB to WC (0x01)
    const PAT: u64 = 0x0007_0406_0007_0406;
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_PAT,
            in("eax") PAT as u32,
            in("edx") (PAT >> 32) as u32,
            options(nostack, preserves_flags),
        );
        // The manual asks for clean caches and TLBs around a PAT change;
        // nothing is mapped WC yet on this core, but entries cached under the
        // old meaning of slot 4 have to go
        core::arch::asm!("wbinvd", options(nostack));
    }
    x86_64::instructions::tlb::flush_all();
}

/// Map an MMIO range write-combining. See `init_pat` for why this is not just
/// `map_mmio_uc` with different flags
pub fn map_mmio_wc(phys_addr: u64, size: u64) {
    map_mmio_typed(phys_addr, size, true)
}

/// MMIO mapping at the HHDM window with uncacheable attributes, for device
/// registers where a write must reach the device at once, not at a cacheline
/// eviction (LAPIC EOI, IOAPIC RTEs, PCIe BARs)
pub fn map_mmio_uc(phys_addr: u64, size: u64) {
    map_mmio_typed(phys_addr, size, false)
}

fn map_mmio_typed(phys_addr: u64, size: u64, write_combining: bool) {
    let _hhdm = HHDM_LOCK.lock();
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

            // UC: PCD=1, PWT=1 -> default PAT index 3. PCD alone (NO_CACHE)
            // gives UC-, which MTRR=WB can downgrade back to WB on real
            // hardware.
            //
            // WC: PAT bit alone -> index 4, which `init_pat` redefined. PCD and
            // PWT stay clear; with them set this would be index 7, uncached.
            // MTRRs can only make a WC mapping stronger (UC wins), so a BAR
            // left UC by firmware MTRRs behaves as UC here, which shows up as
            // BAR1 being no faster than PRAMIN.
            //
            // Bit 7 of a 4 KiB leaf entry is PAT. The crate names it HUGE_PAGE,
            // which is what the same bit means one level up
            const PTE_PAT: PageTableFlags = PageTableFlags::HUGE_PAGE;
            let flags = if write_combining {
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PTE_PAT
            } else {
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::NO_CACHE
                    | PageTableFlags::WRITE_THROUGH
            };
            p1[p1_idx].set_addr(x86_64::PhysAddr::new(page), flags);
        }

        core::arch::asm!("mfence", options(nostack, nomem));
        x86_64::instructions::tlb::flush_all();
    }
}

/// One writer of the kernel's own tables at a time. Every thread's kernel stack
/// unmaps its guard frame here and maps it back when it goes, and a guard
/// inside a 2 MiB page splits that page first. Two processors would each build
/// a table for the same page, and the guard written into the table that lost
/// would never be unmapped or, on the way out, mapped back
static HHDM_LOCK: crate::kcore::irq_lock::IrqMutex<()> = crate::kcore::irq_lock::IrqMutex::new(());

/// Walks the HHDM mapping down to the 4 KiB leaf covering `phys`, splitting any
/// huge page in the way, and returns a pointer to that leaf entry.
///
/// The HHDM lives under PML4 slot 256, which every address space copies
/// verbatim from the kernel tables (`AddressSpace::new_user`). The copy is of
/// the P4 entry itself, so a change further down is visible in every address
/// space
unsafe fn hhdm_leaf_pte(phys: u64) -> Option<*mut x86_64::structures::paging::page_table::PageTableEntry> {
    let hhdm = grub::hhdm();
    let virt = (phys & !0xFFF) + hhdm;

    let (p4_frame, _) = Cr3::read();
    let p4 = &mut *((p4_frame.start_address().as_u64() + hhdm) as *mut PageTable);

    let p4_idx = ((virt >> 39) & 0x1FF) as usize;
    let p3_idx = ((virt >> 30) & 0x1FF) as usize;
    let p2_idx = ((virt >> 21) & 0x1FF) as usize;
    let p1_idx = ((virt >> 12) & 0x1FF) as usize;

    if !p4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
        return None;
    }
    let p3 = &mut *((p4[p4_idx].addr().as_u64() + hhdm) as *mut PageTable);

    if !p3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
        return None;
    }
    if p3[p3_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        split_huge_p3(p3, p3_idx, hhdm);
    }
    let p2 = &mut *((p3[p3_idx].addr().as_u64() + hhdm) as *mut PageTable);

    if !p2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
        return None;
    }
    if p2[p2_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        split_huge_p2(p2, p2_idx, hhdm);
    }
    let p1 = &mut *((p2[p2_idx].addr().as_u64() + hhdm) as *mut PageTable);

    Some(&mut p1[p1_idx] as *mut _)
}

/// Makes one physical frame unreachable through the HHDM.
///
/// Fences off kernel stacks: the frame below a stack is mapped nowhere, so
/// running off the bottom faults at the offending instruction. Returns false if
/// the HHDM has no mapping to clear
pub fn hhdm_unmap_guard_frame(phys: u64) -> bool {
    {
        let _hhdm = HHDM_LOCK.lock();
        unsafe {
            let Some(pte) = hhdm_leaf_pte(phys) else { return false };
            (*pte).set_unused();
            core::arch::asm!("mfence", options(nostack, nomem));
            x86_64::instructions::tlb::flush_all();
        }
    }
    // Every processor, and after the lock: one spinning on it with interrupts
    // off could not answer. A stack overflowing on another processor would
    // otherwise write on through its cached translation
    crate::mm::tlb::shootdown();
    true
}

/// Undoes `hhdm_unmap_guard_frame`, restoring the plain writable HHDM mapping.
/// Must run before the frame goes back to the allocator, or its next owner
/// faults when anything touches it through the HHDM.
///
/// No flush, here or on any other processor. Not-present to present needs no
/// invalidation: "no TLB entry or paging-structure-cache entry is created with
/// information from a paging-structure entry in which the P flag is 0" (Intel
/// SDM vol. 3, 4.10.4.3)
pub fn hhdm_remap_guard_frame(phys: u64) -> bool {
    let _hhdm = HHDM_LOCK.lock();
    unsafe {
        let Some(pte) = hhdm_leaf_pte(phys) else { return false };
        (*pte).set_addr(
            x86_64::PhysAddr::new(phys & !0xFFF),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
        );
        core::arch::asm!("mfence", options(nostack, nomem));
    }
    true
}
