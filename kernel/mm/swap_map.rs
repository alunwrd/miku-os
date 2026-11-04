use core::sync::atomic::{AtomicBool, Ordering};

use crate::kcore::irq_lock::{IrqMutex, IrqMutexGuard};
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
    // Upper bound (exclusive) on used indices. All scans clip to this, so
    // age_all and pick_victim do not walk 64K entries on a small workload
    high_water: usize,
}

impl SwapMap {
    const fn new() -> Self {
        Self {
            entries:    [ReverseEntry::empty(); MAX_TRACKED],
            clock_hand: 0,
            tracked:    0,
            high_water: 0,
        }
    }

    #[inline] fn frame_idx(phys: u64) -> usize { (phys / 4096) as usize }

    #[inline]
    fn bump_hw(&mut self, idx: usize) {
        if idx + 1 > self.high_water { self.high_water = idx + 1; }
    }

    // Tighten high_water if we just freed the top entry. Only walks while
    // it actually shrinks; amortized O(1)
    #[inline]
    fn trim_hw(&mut self) {
        while self.high_water > 0 && !self.entries[self.high_water - 1].is_used() {
            self.high_water -= 1;
        }
    }

    pub fn track(&mut self, phys: u64, cr3: u64, virt: u64, pinned: bool) {
        let idx = Self::frame_idx(phys);
        if idx >= MAX_TRACKED { return; }
        if !self.entries[idx].is_used() { self.tracked += 1; }
        // Read current PTE flags for restoring on swap-in
        let pte_flags = crate::mm::vmm::read_pte_raw(cr3, virt).unwrap_or(0) & 0xFFF;
        self.entries[idx] = ReverseEntry { cr3, virt_addr: virt, pte_flags, age: 1, pinned };
        self.bump_hw(idx);
    }

    pub fn untrack(&mut self, phys: u64) {
        let idx = Self::frame_idx(phys);
        if idx >= MAX_TRACKED { return; }
        if self.entries[idx].is_used() { self.tracked = self.tracked.saturating_sub(1); }
        self.entries[idx] = ReverseEntry::empty();
        if idx + 1 == self.high_water { self.trim_hw(); }
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
        // Bound by high_water
        let hw = self.high_water;
        for e in self.entries[..hw].iter_mut() {
            if e.is_used() && !e.pinned {
                e.age = e.age.saturating_add(1);
            }
        }
    }

    pub fn pick_victim(&mut self) -> Option<(u64, u64, u64, u64)> {
        if self.tracked == 0 { return None; }
        let hw = self.high_water;
        if hw == 0 { return None; }

        if self.clock_hand >= hw { self.clock_hand = 0; }

        // Single-pass clock scan with a fallback: the preferred victim is
        // age>=3, and the first usable any-age entry is remembered so cold
        // caches need no second sweep
        let start = self.clock_hand;
        let mut fallback: Option<usize> = None;
        let mut steps = 0usize;
        let mut idx   = start;

        while steps < hw {
            let e = &self.entries[idx];
            if e.is_used() && !e.pinned {
                if e.age >= 3 {
                    self.clock_hand = if idx + 1 >= hw { 0 } else { idx + 1 };
                    return Some((idx as u64 * 4096, e.cr3, e.virt_addr, e.pte_flags));
                }
                if fallback.is_none() { fallback = Some(idx); }
            }
            idx += 1;
            if idx >= hw { idx = 0; }
            steps += 1;
        }

        if let Some(fi) = fallback {
            let e = &self.entries[fi];
            self.clock_hand = if fi + 1 >= hw { 0 } else { fi + 1 };
            return Some((fi as u64 * 4096, e.cr3, e.virt_addr, e.pte_flags));
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

// An IrqMutex, as `pmm::PMM` is: the two are held together on the fork and
// teardown paths
static SWAP_MAP: IrqMutex<SwapMap> = IrqMutex::new(SwapMap::new());

pub fn track(phys: u64, cr3: u64, virt: u64, pinned: bool) {
    SWAP_MAP.lock().track(phys, cr3, virt, pinned);
}

/// Guard holding SWAP_MAP across a batch of tracked frames, e.g. a multi-MB
/// map_range. The caller supplies pte_flags, so page tables are not re-walked
/// for what was just written
pub struct SwapMapGuard<'a> {
    inner: IrqMutexGuard<'a, SwapMap>,
}

impl<'a> SwapMapGuard<'a> {
    #[inline]
    pub fn track(&mut self, phys: u64, cr3: u64, virt: u64, pinned: bool, pte_flags: u64) {
        let idx = SwapMap::frame_idx(phys);
        if idx >= MAX_TRACKED { return; }
        if !self.inner.entries[idx].is_used() { self.inner.tracked += 1; }
        self.inner.entries[idx] = ReverseEntry {
            cr3,
            virt_addr: virt,
            pte_flags: pte_flags & 0xFFF,
            age: 1,
            pinned,
        };
        self.inner.bump_hw(idx);
    }

    #[inline]
    pub fn untrack(&mut self, phys: u64) {
        let idx = SwapMap::frame_idx(phys);
        if idx >= MAX_TRACKED { return; }
        if self.inner.entries[idx].is_used() {
            self.inner.tracked = self.inner.tracked.saturating_sub(1);
        }
        self.inner.entries[idx] = ReverseEntry::empty();
        if idx + 1 == self.inner.high_water { self.inner.trim_hw(); }
    }

    #[inline]
    pub fn set_pinned(&mut self, phys: u64, pinned: bool) {
        let idx = SwapMap::frame_idx(phys);
        if idx < MAX_TRACKED && self.inner.entries[idx].is_used() {
            self.inner.entries[idx].pinned = pinned;
        }
    }
}

pub fn lock_batch() -> SwapMapGuard<'static> {
    SwapMapGuard { inner: SWAP_MAP.lock() }
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
    // bits 0: not present, bit 1: swap marker, bits 2-11: saved flags, bits
    // 12+: slot
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
    use crate::mm::swap;
    if !swap::swap_is_active() { return None; }
    if swap::swap_free_pages() == 0 {
        crate::serial_println!("[swap_map] swap full - cannot evict");
        return None;
    }

    let (phys, cr3, virt, pte_flags) = SWAP_MAP.lock().pick_victim_and_pin()?;

    let slot = match swap::swap_out_internal(phys) {
        Ok(s) => s,
        Err(e) => {
            SWAP_MAP.lock().set_pinned(phys, false);
            crate::serial_println!("[swap_map] swap_out failed: {:?}", e);
            return None;
        }
    };

    // Encode original PTE flags into swap PTE so they can be restored on
    // swap-in
    unsafe { crate::mm::vmm::mark_swapped_with_flags(cr3, virt, slot, pte_flags); }
    SWAP_MAP.lock().untrack(phys);
    crate::mm::pmm::free_frame(phys);

    crate::serial_println!("[swap_map] evicted virt={:#x} slot={} phys={:#x}", virt, slot, phys);
    Some(phys)
}

pub fn try_swapin(cr3: u64, page_addr: u64, slot: u32, raw_swap_pte: u64) -> bool {
    use crate::mm::swap;

    let phys = match alloc_for_swapin()
        .or_else(|| { evict_one()?; crate::mm::pmm::alloc_frame() })
    {
        Some(f) => f,
        None => {
            crate::serial_println!("[swap_map] OOM: no frame for swap-in virt={:#x}", page_addr);
            return false;
        }
    };

    match swap::swap_in_internal(slot, phys) {
        Ok(()) => {}
        Err(e) => {
            crate::mm::pmm::free_frame(phys);
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

    let aspace = crate::mm::vmm::AddressSpace { cr3 };
    aspace.map_page(page_addr, phys, flags);
    core::mem::forget(aspace);

    track(phys, cr3, page_addr, false);
    crate::serial_println!("[swap_map] swap-in ok: virt={:#x} slot={} -> phys={:#x}", page_addr, slot, phys);
    true
}

pub fn alloc_or_evict() -> Option<u64> {
    if let Some(f) = crate::mm::pmm::alloc_frame() { return Some(f); }
    evict_one()?;
    if let Some(f) = crate::mm::pmm::alloc_frame() { return Some(f); }
    crate::mm::pmm::alloc_frame_emergency()
}

pub fn alloc_for_swapin() -> Option<u64> {
    crate::mm::pmm::alloc_frame_emergency()
}

// Reclaim (kswapd)
//
// The timer ISR only raises a flag, and the work happens on this thread, where
// sleeping and I/O are legal: evict_one() makes a blocking swap write to disk

const EMERGENCY_LOW_WATER:  usize = 32;
const EMERGENCY_HIGH_WATER: usize = 64;

static RECLAIM_WANTED: AtomicBool = AtomicBool::new(false);

/// Called from the timer ISR. Must stay allocation-free, lock-free and
/// I/O-free: it only decides whether kswapd has work
pub fn request_reclaim_from_isr() {
    if crate::mm::pmm::emergency_frames_available() < EMERGENCY_LOW_WATER {
        RECLAIM_WANTED.store(true, Ordering::Relaxed);
    }
}

/// Refills the emergency frame pool by evicting cold pages to swap. Thread
/// context only: this performs disk I/O
pub fn refill_emergency_pool() {
    if crate::mm::pmm::emergency_frames_available() >= EMERGENCY_LOW_WATER {
        return;
    }
    while crate::mm::pmm::emergency_frames_available() < EMERGENCY_HIGH_WATER {
        if evict_one().is_none() { break; }
    }
}

/// How often the heap's depots give back what they did not need: Bonwick's
/// working-set interval, about five seconds at 250 Hz
const REAP_TICKS: u64 = 5 * 250;

/// Page-reclaim kernel thread. Wakes on the flag the tick raises, ages the
/// reverse map, and tops the emergency pool back up
pub fn kswapd_thread() -> ! {
    crate::serial_println!("[kswapd] thread started");
    let mut last_age_tick = 0u64;
    let mut last_reap_tick = 0u64;
    loop {
        let tick = crate::arch::x86_64::interrupts::get_tick();
        // Ages the reverse map about once a second (250 Hz tick). It walks up
        // to high_water entries, which must not run with interrupts off
        if tick.saturating_sub(last_age_tick) >= crate::arch::x86_64::apic::TIMER_HZ_DEFAULT as u64 {
            last_age_tick = tick;
            age_all();
            crate::sched::stats::report_overdue_sleepers();
        }
        if RECLAIM_WANTED.swap(false, Ordering::Relaxed) {
            refill_emergency_pool();
        }
        // What is kept only to be quick goes back when frames are short: cached
        // file pages first, each of which can be read again (`mm::shrink`). The
        // heap's mapped area gets its freed ranges back here, where nothing is
        // held, and its caches give back what sat unused for a whole interval
        crate::mm::shrink::run_if_low();
        crate::mm::vmap::purge_if_wanted();
        if tick.saturating_sub(last_reap_tick) >= REAP_TICKS {
            last_reap_tick = tick;
            crate::mm::heap::reap();
        }
        crate::sched::sleep(25);
    }
}
