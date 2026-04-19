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

    fn mark_used(&mut self, frame: usize) {
        self.bitmap[frame / 64] |= 1 << (frame % 64);
    }

    fn mark_free(&mut self, frame: usize) {
        self.bitmap[frame / 64] &= !(1 << (frame % 64));
        if frame < self.free_hint {
            self.free_hint = frame;
        }
        if frame < self.contiguous_hint {
            self.contiguous_hint = frame;
        }
    }

    fn is_used(&self, frame: usize) -> bool {
        self.bitmap[frame / 64] & (1 << (frame % 64)) != 0
    }

    fn add_region(&mut self, base: u64, size: u64) {
        let cap         = frame_cap();
        let start_frame = (base as usize + FRAME_SIZE - 1) / FRAME_SIZE;
        let end_frame   = ((base + size) as usize / FRAME_SIZE).min(cap);

        for i in start_frame..end_frame {
            self.mark_free(i);
            self.total += 1;
        }
        crate::serial_println!(
            "[pmm] added region: base={:#x} size={}MB frames={}",
            base,
            size / 1024 / 1024,
            end_frame.saturating_sub(start_frame)
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
            for j in start_idx..(start_idx + count) {
                self.mark_used(j);
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

    fn find_contiguous(&self, from: usize, to: usize, count: usize) -> Option<usize> {
        let mut consecutive = 0;
        let mut start_idx   = 0;

        for i in from..to {
            if !self.is_used(i) {
                if consecutive == 0 {
                    start_idx = i;
                }
                consecutive += 1;
                if consecutive == count {
                    return Some(start_idx);
                }
            } else {
                consecutive = 0;
            }
        }
        None
    }

    fn free_frames(&mut self, phys: u64, count: usize) {
        let cap         = frame_cap();
        let start_frame = phys as usize / FRAME_SIZE;
        for i in start_frame..(start_frame + count) {
            if i < cap && self.is_used(i) {
                self.mark_free(i);
                self.used -= 1;
            }
        }
    }
}

static PMM: Mutex<FrameAllocator> = Mutex::new(FrameAllocator::new());

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
    for i in start_frame..end_frame {
        if !pmm.is_used(i) {
            pmm.mark_used(i);
            if pmm.used < pmm.total { pmm.used += 1; }
        }
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
