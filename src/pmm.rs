extern crate alloc;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const MAX_FRAMES: usize = 4 * 1024 * 1024;
const FRAME_SIZE: usize = 4096;

static TOTAL_RAM_BYTES: AtomicU64  = AtomicU64::new(0);
static FRAME_CAP:       AtomicUsize = AtomicUsize::new(MAX_FRAMES);

pub fn register_total_ram(bytes: u64) {
    TOTAL_RAM_BYTES.fetch_add(bytes, Ordering::Relaxed);
    let frames = (bytes as usize / FRAME_SIZE).min(MAX_FRAMES);
    FRAME_CAP.fetch_max(frames, Ordering::Relaxed);
}

pub fn total_ram_kb() -> u64 {
    TOTAL_RAM_BYTES.load(Ordering::Relaxed) / 1024
}

fn frame_cap() -> usize {
    FRAME_CAP.load(Ordering::Relaxed)
}

struct FrameAllocator {
    bitmap:            [u64; MAX_FRAMES / 64],
    total:             usize,
    used:              usize,
    free_hint:         usize,
    contiguous_hint:   usize,
}

impl FrameAllocator {
    const fn new() -> Self {
        Self {
            bitmap:          [u64::MAX; MAX_FRAMES / 64],
            total:           0,
            used:            0,
            free_hint:       0,
            contiguous_hint: 0,
        }
    }

    #[inline(always)]
    fn mark_used(&mut self, frame: usize) {
        self.bitmap[frame / 64] |= 1u64 << (frame % 64);
    }

    #[inline(always)]
    fn mark_free(&mut self, frame: usize) {
        self.bitmap[frame / 64] &= !(1u64 << (frame % 64));
        if frame < self.free_hint {
            self.free_hint = frame;
        }
        if frame < self.contiguous_hint {
            self.contiguous_hint = frame;
        }
    }

    #[inline(always)]
    fn is_used(&self, frame: usize) -> bool {
        self.bitmap[frame / 64] & (1u64 << (frame % 64)) != 0
    }

    // Mark a contiguous range as free using whole u64 words wherever
    // possible. The bit loops only run on the partial words at each end
    fn mark_range_free(&mut self, start: usize, end: usize) {
        if start >= end { return; }
        let mut i = start;
        let head_end = ((start + 63) & !63).min(end);
        while i < head_end {
            self.bitmap[i / 64] &= !(1u64 << (i % 64));
            i += 1;
        }
        while i + 64 <= end {
            self.bitmap[i / 64] = 0;
            i += 64;
        }
        while i < end {
            self.bitmap[i / 64] &= !(1u64 << (i % 64));
            i += 1;
        }
        if start < self.free_hint       { self.free_hint = start; }
        if start < self.contiguous_hint { self.contiguous_hint = start; }
    }

    fn add_region(&mut self, base: u64, size: u64) {
        let cap         = frame_cap();
        let start_frame = (base as usize + FRAME_SIZE - 1) / FRAME_SIZE;
        let end_frame   = ((base + size) as usize / FRAME_SIZE).min(cap);
        let added       = end_frame.saturating_sub(start_frame);

        self.mark_range_free(start_frame, end_frame);
        self.total += added;

        crate::serial_println!(
            "[pmm] added region: base={:#x} size={}MB frames={}",
            base,
            size / 1024 / 1024,
            added
        );
    }

    fn alloc_frames(&mut self, count: usize) -> Option<u64> {
        let cap          = frame_cap();
        let search_start = if count == 1 {
            self.free_hint
        } else {
            self.contiguous_hint
        };

        let result = self.find_contiguous(search_start, cap, count);
        let result = match result {
            Some(r) => Some(r),
            None if search_start > 0 => self.find_contiguous(0, search_start, count),
            None => None,
        };

        if let Some(start_idx) = result {
            // Batched mark-used by 64-bit word
            let end = start_idx + count;
            let mut i = start_idx;
            let head_end = ((start_idx + 63) & !63).min(end);
            while i < head_end {
                self.bitmap[i / 64] |= 1u64 << (i % 64);
                i += 1;
            }
            while i + 64 <= end {
                self.bitmap[i / 64] = u64::MAX;
                i += 64;
            }
            while i < end {
                self.bitmap[i / 64] |= 1u64 << (i % 64);
                i += 1;
            }

            self.used += count;
            if count == 1 {
                self.free_hint = start_idx + 1;
            } else {
                self.contiguous_hint = start_idx + count;
            }
            return Some((start_idx * FRAME_SIZE) as u64);
        }

        None
    }

    /// Word-parallel free-run scan. For count==1 we scan u64 words and pick
    /// the first zero bit via 'trailing_ones' - ~64x fewer ops than per-bit
    /// For count>1 we walk bits but fast-skip all-ones words and gulp
    /// all-zeros words 64 at a time
    fn find_contiguous(&self, from: usize, to: usize, count: usize) -> Option<usize> {
        if from >= to || count == 0 { return None; }
        let bitmap = &self.bitmap;

        if count == 1 {
            let mut i = from;
            let head_end = ((from + 63) & !63).min(to);
            while i < head_end {
                if bitmap[i / 64] & (1u64 << (i % 64)) == 0 {
                    return Some(i);
                }
                i += 1;
            }
            while i + 64 <= to {
                let w = bitmap[i / 64];
                if w != u64::MAX {
                    return Some(i + w.trailing_ones() as usize);
                }
                i += 64;
            }
            while i < to {
                if bitmap[i / 64] & (1u64 << (i % 64)) == 0 {
                    return Some(i);
                }
                i += 1;
            }
            return None;
        }

        let mut consecutive = 0usize;
        let mut start_idx   = 0usize;
        let mut i = from;
        while i < to {
            if consecutive == 0 && i % 64 == 0 && i + 64 <= to {
                let w = bitmap[i / 64];
                if w == u64::MAX { i += 64; continue; }
                if w == 0 {
                    start_idx = i;
                    consecutive = 64;
                    i += 64;
                    if consecutive >= count { return Some(start_idx); }
                    continue;
                }
            }
            if bitmap[i / 64] & (1u64 << (i % 64)) == 0 {
                if consecutive == 0 { start_idx = i; }
                consecutive += 1;
                if consecutive >= count { return Some(start_idx); }
            } else {
                consecutive = 0;
            }
            i += 1;
        }
        None
    }

    fn free_frames(&mut self, phys: u64, count: usize) {
        let cap         = frame_cap();
        let start_frame = phys as usize / FRAME_SIZE;
        let end_frame   = (start_frame + count).min(cap);
        for i in start_frame..end_frame {
            if self.is_used(i) {
                self.bitmap[i / 64] &= !(1u64 << (i % 64));
                self.used -= 1;
            }
        }
        if start_frame < self.free_hint       { self.free_hint = start_frame; }
        if start_frame < self.contiguous_hint { self.contiguous_hint = start_frame; }
    }
}

static PMM: Mutex<FrameAllocator> = Mutex::new(FrameAllocator::new());

/// Guard that holds the PMM lock for the duration of a batched free
/// operation - e.g. tearing down an address space. Avoids re-acquiring
/// the global PMM lock per freed frame
pub struct PmmGuard<'a> {
    inner: spin::MutexGuard<'a, FrameAllocator>,
}

impl<'a> PmmGuard<'a> {
    #[inline]
    pub fn free_frame(&mut self, phys: u64) {
        let cap         = frame_cap();
        let frame       = phys as usize / FRAME_SIZE;
        if frame >= cap { return; }
        if self.inner.is_used(frame) {
            self.inner.bitmap[frame / 64] &= !(1u64 << (frame % 64));
            self.inner.used -= 1;
        }
        if frame < self.inner.free_hint       { self.inner.free_hint = frame; }
        if frame < self.inner.contiguous_hint { self.inner.contiguous_hint = frame; }
    }

    #[inline]
    pub fn free_frames(&mut self, phys: u64, count: usize) {
        self.inner.free_frames(phys, count);
    }

    #[inline]
    pub fn alloc_frame(&mut self) -> Option<u64> {
        self.inner.alloc_frames(1)
    }

    #[inline]
    pub fn alloc_frames(&mut self, count: usize) -> Option<u64> {
        self.inner.alloc_frames(count)
    }
}

pub fn lock_for_batch() -> PmmGuard<'static> {
    PmmGuard { inner: PMM.lock() }
}

/// Refcount-aware free using a pre-acquired PMM guard. Returns true if
/// the frame was actually returned to the allocator (refcount hit 0).
/// Kept for non-CoW callers that have no SwapMapGuard handy
pub fn free_frame_cow_batched(pmm: &mut PmmGuard<'_>, phys: u64) -> bool {
    let frame = phys & !0xFFF;
    let should_free = {
        let mut rc = REFCOUNTS.lock();
        if let Some(count) = rc.get_mut(&frame) {
            *count = count.saturating_sub(1);
            let val = *count;
            if val <= 1 { rc.remove(&frame); }
            val == 0
        } else {
            true
        }
    };
    if should_free {
        pmm.free_frame(phys);
    }
    should_free
}

/// Refcount-aware free that also keeps swap_map state consistent across
/// the CoW lifecycle:
///     refcount  0: frame is truly going away. untrack from swap_map
///                  and return to allocator
///     refcount  1: a CoW sibling died; the survivor now owns the page
///                  outright, so clear the pin we set at fork time
///     refcount >1: still shared, leave swap_map and pin alone
/// Returns true iff the frame was returned to the allocator
pub fn free_frame_cow_swap(
    pmm:  &mut PmmGuard<'_>,
    swap: &mut crate::swap_map::SwapMapGuard<'_>,
    phys: u64,
) -> bool {
    let frame = phys & !0xFFF;
    enum Outcome { Free, NowUnique, StillShared }
    let outcome = {
        let mut rc = REFCOUNTS.lock();
        match rc.get_mut(&frame) {
            Some(count) => {
                *count = count.saturating_sub(1);
                let val = *count;
                if val == 0      { rc.remove(&frame); Outcome::Free }
                else if val == 1 { rc.remove(&frame); Outcome::NowUnique }
                else             { Outcome::StillShared }
            }
            None => Outcome::Free,
        }
    };
    match outcome {
        Outcome::Free => {
            swap.untrack(phys);
            pmm.free_frame(phys);
            true
        }
        Outcome::NowUnique => {
            // Last sibling gone; the surviving mapping is unique now,
            // so the frame is swappable again. swap_map entry still
            // points at whichever cr3/virt registered it at map_range
            swap.set_pinned(phys, false);
            false
        }
        Outcome::StillShared => false,
    }
}

const EMERGENCY_POOL_SIZE: usize = 64;
static EMERGENCY_POOL: Mutex<EmergencyPool> = Mutex::new(EmergencyPool::new());

struct EmergencyPool {
    frames: [u64; EMERGENCY_POOL_SIZE],
    count:  usize,
}

impl EmergencyPool {
    const fn new() -> Self {
        Self { frames: [0u64; EMERGENCY_POOL_SIZE], count: 0 }
    }
    fn push(&mut self, phys: u64) -> bool {
        if self.count >= EMERGENCY_POOL_SIZE { return false; }
        self.frames[self.count] = phys;
        self.count += 1;
        true
    }
    fn pop(&mut self) -> Option<u64> {
        if self.count == 0 { return None; }
        self.count -= 1;
        Some(self.frames[self.count])
    }
    fn len(&self) -> usize { self.count }
}

pub fn alloc_frame_emergency() -> Option<u64> {
    EMERGENCY_POOL.lock().pop()
}

pub fn push_emergency_frame(phys: u64) -> bool {
    EMERGENCY_POOL.lock().push(phys)
}

pub fn emergency_frames_available() -> usize {
    EMERGENCY_POOL.lock().len()
}

pub fn refill_emergency_pool() {
    let current = EMERGENCY_POOL.lock().len();
    if current >= EMERGENCY_POOL_SIZE {
        return;
    }
    let needed = EMERGENCY_POOL_SIZE - current;

    let mut frames: [u64; EMERGENCY_POOL_SIZE] = [0; EMERGENCY_POOL_SIZE];
    let mut collected = 0;

    {
        let mut pmm = PMM.lock();
        while collected < needed {
            match pmm.alloc_frames(1) {
                Some(f) => { frames[collected] = f; collected += 1; }
                None    => break,
            }
        }
    }

    if collected > 0 {
        let mut pool = EMERGENCY_POOL.lock();
        for i in 0..collected {
            if !pool.push(frames[i]) {
                drop(pool);
                PMM.lock().free_frames(frames[i], 1);
                for j in (i + 1)..collected {
                    PMM.lock().free_frames(frames[j], 1);
                }
                return;
            }
        }
    }
}

pub fn add_region(base: u64, size: u64) {
    PMM.lock().add_region(base, size);
}

pub fn reserve_region(base: u64, size: u64) {
    let mut pmm = PMM.lock();
    let start_frame = base as usize / 4096;
    let end_frame   = ((base + size + 4095) as usize / 4096).min(frame_cap());
    if start_frame >= end_frame { return; }

    // Word-level scan: count newly-flipped (was-zero) bits, then OR-in the
    // word in one go. Far faster than bit-by-bit for multi-MB regions like
    // the kernel image or framebuffer
    let mut i = start_frame;
    let head_end = ((start_frame + 63) & !63).min(end_frame);
    while i < head_end {
        let bit = 1u64 << (i % 64);
        let w = &mut pmm.bitmap[i / 64];
        if *w & bit == 0 {
            *w |= bit;
            if pmm.used < pmm.total { pmm.used += 1; }
        }
        i += 1;
    }
    while i + 64 <= end_frame {
        let w = &mut pmm.bitmap[i / 64];
        let newly = (!*w).count_ones() as usize;
        *w = u64::MAX;
        // saturating_sub - if the bitmap was ever corrupted and used >
        // total, the subtraction would underflow and overstate room
        let room = pmm.total.saturating_sub(pmm.used);
        pmm.used += newly.min(room);
        i += 64;
    }
    while i < end_frame {
        let bit = 1u64 << (i % 64);
        let w = &mut pmm.bitmap[i / 64];
        if *w & bit == 0 {
            *w |= bit;
            if pmm.used < pmm.total { pmm.used += 1; }
        }
        i += 1;
    }
}

pub fn alloc_frame() -> Option<u64> {
    PMM.lock().alloc_frames(1)
}

pub fn alloc_frames(count: usize) -> Option<u64> {
    PMM.lock().alloc_frames(count)
}

pub fn free_frame(phys: u64) {
    PMM.lock().free_frames(phys, 1);
}

pub fn free_frames(phys: u64, count: usize) {
    PMM.lock().free_frames(phys, count);
}

pub fn stats() -> (usize, usize) {
    let p = PMM.lock();
    (p.used, p.total)
}

//  refcount for COW pages //
// Only pages with refcount >= 2 are tracked. Absent = refcount 1

static REFCOUNTS: Mutex<BTreeMap<u64, u16>> = Mutex::new(BTreeMap::new());

pub fn ref_inc(phys: u64) {
    let frame = phys & !0xFFF;
    let mut rc = REFCOUNTS.lock();
    let entry = rc.entry(frame).or_insert(1);
    *entry = entry.saturating_add(1);
}

pub fn ref_dec(phys: u64) -> u16 {
    let frame = phys & !0xFFF;
    let mut rc = REFCOUNTS.lock();
    if let Some(count) = rc.get_mut(&frame) {
        *count = count.saturating_sub(1);
        let val = *count;
        if val <= 1 {
            rc.remove(&frame);
        }
        val
    } else {
        0
    }
}

pub fn ref_get(phys: u64) -> u16 {
    let frame = phys & !0xFFF;
    let rc = REFCOUNTS.lock();
    rc.get(&frame).copied().unwrap_or(1)
}

pub fn free_frame_cow(phys: u64) {
    let frame = phys & !0xFFF;
    let should_free = {
        let mut rc = REFCOUNTS.lock();
        if let Some(count) = rc.get_mut(&frame) {
            *count = count.saturating_sub(1);
            let val = *count;
            if val <= 1 {
                rc.remove(&frame);
            }
            val == 0
        } else {
            true
        }
    };
    if should_free {
        free_frame(phys);
    }
}
