use spin::Mutex;
use x86_64::structures::paging::PageTableFlags;

const MAX_TRACKED: usize = 64 * 1024;

#[derive(Copy, Clone)]
struct ReverseEntry {
    cr3:       u64,
    virt_addr: u64,
    pte_flags: u64,
    age:       u8,
    pinned:    bool,
}

impl ReverseEntry {
    const fn empty() -> Self { Self { cr3: 0, virt_addr: 0, pte_flags: 0, age: 0, pinned: false } }
    #[inline] fn is_used(&self) -> bool { self.cr3 != 0 }
}

struct SwapMap {
    entries:    [ReverseEntry; MAX_TRACKED],
    clock_hand: usize,
    tracked:    usize,
}

impl SwapMap {
    const fn new() -> Self {
        Self { entries: [ReverseEntry::empty(); MAX_TRACKED], clock_hand: 0, tracked: 0 }
    }

    #[inline] fn frame_idx(phys: u64) -> usize { (phys / 4096) as usize }

    pub fn track(&mut self, phys: u64, cr3: u64, virt: u64, pinned: bool) {
        let idx = Self::frame_idx(phys);
        if idx >= MAX_TRACKED { return; }
        if !self.entries[idx].is_used() { self.tracked += 1; }
        // Read current PTE flags for restoring on swap-in
        let pte_flags = crate::vmm::read_pte_raw(cr3, virt).unwrap_or(0) & 0xFFF;
        self.entries[idx] = ReverseEntry { cr3, virt_addr: virt, pte_flags, age: 1, pinned };
    }

    pub fn untrack(&mut self, phys: u64) {
        let idx = Self::frame_idx(phys);
        if idx >= MAX_TRACKED { return; }
        if self.entries[idx].is_used() { self.tracked = self.tracked.saturating_sub(1); }
        self.entries[idx] = ReverseEntry::empty();
    }

    pub fn touch(&mut self, phys: u64) {
        let idx = Self::frame_idx(phys);
        if idx < MAX_TRACKED && self.entries[idx].is_used() {
            self.entries[idx].age = 1;
        }
    }

    pub fn set_pinned(&mut self, phys: u64, pinned: bool) {
        let idx = Self::frame_idx(phys);
        if idx < MAX_TRACKED && self.entries[idx].is_used() {
            self.entries[idx].pinned = pinned;
        }
    }

    pub fn age_all(&mut self) {
        for e in self.entries.iter_mut() {
            if e.is_used() && !e.pinned {
                e.age = e.age.saturating_add(1);
            }
        }
    }

    pub fn pick_victim(&mut self) -> Option<(u64, u64, u64, u64)> {
        if self.tracked == 0 { return None; }
        let n = MAX_TRACKED;

        for _ in 0..n {
            let idx = self.clock_hand;
            self.clock_hand = (self.clock_hand + 1) % n;
            let e = &self.entries[idx];
            if e.is_used() && !e.pinned && e.age >= 3 {
                return Some((idx as u64 * 4096, e.cr3, e.virt_addr, e.pte_flags));
            }
        }

        for idx in 0..n {
            let e = &self.entries[idx];
            if e.is_used() && !e.pinned {
                self.clock_hand = (idx + 1) % n;
                return Some((idx as u64 * 4096, e.cr3, e.virt_addr, e.pte_flags));
            }
        }
        None
    }

    pub fn pick_victim_and_pin(&mut self) -> Option<(u64, u64, u64, u64)> {
        let victim = self.pick_victim()?;
        let (phys, _, _, _) = victim;
        self.set_pinned(phys, true);
        Some(victim)
    }
}

static SWAP_MAP: Mutex<SwapMap> = Mutex::new(SwapMap::new());

pub fn track(phys: u64, cr3: u64, virt: u64, pinned: bool) {
    SWAP_MAP.lock().track(phys, cr3, virt, pinned);
}

pub fn untrack(phys: u64) {
    SWAP_MAP.lock().untrack(phys);
}

pub fn touch(phys: u64) {
    SWAP_MAP.lock().touch(phys);
}

pub fn set_pinned(phys: u64, pinned: bool) {
    SWAP_MAP.lock().set_pinned(phys, pinned);
}

pub fn age_all() {
    SWAP_MAP.lock().age_all();
}

const SWAP_PTE_MARKER: u64     = 0b10;
const SWAP_PTE_SLOT_SHIFT: u64 = 12;

pub fn make_swap_pte(slot: u32) -> u64 {
    SWAP_PTE_MARKER | ((slot as u64) << SWAP_PTE_SLOT_SHIFT)
}

pub fn make_swap_pte_with_flags(slot: u32, pte_flags: u64) -> u64 {
    // bits 0: not present, bit 1: swap marker, bits 2-11: saved flags, bits 12+: slot
    SWAP_PTE_MARKER | (pte_flags & 0xFFC) | ((slot as u64) << SWAP_PTE_SLOT_SHIFT)
}

pub fn flags_from_swap_pte(raw: u64) -> u64 {
    raw & 0xFFC
}

pub fn is_swap_pte(raw: u64) -> bool {
    (raw & 1) == 0 && (raw & SWAP_PTE_MARKER) != 0 && raw != 0
     	&& (raw >> SWAP_PTE_SLOT_SHIFT) != 0
}

pub fn slot_from_pte(raw: u64) -> u32 {
    ((raw >> SWAP_PTE_SLOT_SHIFT) & 0xF_FFFF) as u32
}

pub fn evict_one() -> Option<u64> {
    use crate::swap;
    if !swap::swap_is_active() { return None; }
    if swap::swap_free_pages() == 0 {
        crate::serial_println!("[swap_map] swap full - cannot evict");
        return None;
    }

    let (phys, cr3, virt, pte_flags) = SWAP_MAP.lock().pick_victim_and_pin()?;

    let mut drive = crate::ata::AtaDrive::from_idx(swap::swap_drive_idx());
    let slot = match swap::swap_out_internal(phys, &mut drive) {
        Ok(s) => s,
        Err(e) => {
            SWAP_MAP.lock().set_pinned(phys, false);
            crate::serial_println!("[swap_map] swap_out failed: {:?}", e);
            return None;
        }
    };

    // Encode original PTE flags into swap PTE so they can be restored on swap-in
    unsafe { crate::vmm::mark_swapped_with_flags(cr3, virt, slot, pte_flags); }
    SWAP_MAP.lock().untrack(phys);
    crate::pmm::free_frame(phys);

    crate::serial_println!("[swap_map] evicted virt={:#x} slot={} phys={:#x}", virt, slot, phys);
    Some(phys)
}

pub fn try_swapin(cr3: u64, page_addr: u64, slot: u32, raw_swap_pte: u64) -> bool {
    use crate::swap;

    let phys = match alloc_for_swapin()
        .or_else(|| { evict_one()?; crate::pmm::alloc_frame() })
    {
        Some(f) => f,
        None => {
            crate::serial_println!("[swap_map] OOM: no frame for swap-in virt={:#x}", page_addr);
            return false;
        }
    };

    let mut drive = crate::ata::AtaDrive::from_idx(swap::swap_drive_idx());
    match swap::swap_in_internal(slot, phys, &mut drive) {
        Ok(()) => {}
        Err(e) => {
            crate::pmm::free_frame(phys);
            crate::serial_println!("[swap_map] swap_in failed: {:?}", e);
            return false;
        }
    }

    // Restore original page flags from swap PTE, default to RW if not stored
    let saved = flags_from_swap_pte(raw_swap_pte);
    let flags = if saved != 0 {
        PageTableFlags::from_bits_truncate(saved | PageTableFlags::PRESENT.bits())
    } else {
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
    };

    let aspace = crate::vmm::AddressSpace { cr3 };
    aspace.map_page(page_addr, phys, flags);
    core::mem::forget(aspace);

    track(phys, cr3, page_addr, false);
    crate::serial_println!("[swap_map] swap-in ok: virt={:#x} slot={} -> phys={:#x}", page_addr, slot, phys);
    true
}

pub fn alloc_or_evict() -> Option<u64> {
    if let Some(f) = crate::pmm::alloc_frame() { return Some(f); }
    evict_one()?;
    if let Some(f) = crate::pmm::alloc_frame() { return Some(f); }
    crate::pmm::alloc_frame_emergency()
}

pub fn alloc_for_swapin() -> Option<u64> {
    crate::pmm::alloc_frame_emergency()
}

pub fn refill_emergency_pool_tick() {
    if crate::pmm::emergency_frames_available() >= 32 {
        return;
    }
    while crate::pmm::emergency_frames_available() < 64 {
        if evict_one().is_none() { break; }
    }
}
