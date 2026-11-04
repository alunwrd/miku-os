// The frame allocator (`dev/kernel-core.md`, "Frames")
//
// A bit a frame and a copy-on-write count a frame, both sized at boot by the
// highest frame the memory map calls usable. Until `layout` they do not exist:
// the boot names its regions and reservations first (`add_region`,
// `reserve_region`), and those are only written down, because where the two
// arrays can go is known once everything that must not be written over has been
// named.

use crate::kcore::irq_lock::{IrqMutex, IrqMutexGuard};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, AtomicU64, AtomicUsize, Ordering};

const FRAME_SIZE: usize = 4096;

/// Usable regions and reservations the boot names before `layout`. The
/// memory map of a PC is a few dozen entries; a module a reservation
const MAX_REGIONS: usize = 128;
const MAX_RESERVED: usize = 64;

static TOTAL_RAM_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn register_total_ram(bytes: u64) {
    TOTAL_RAM_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

pub fn total_ram_kb() -> u64 {
    TOTAL_RAM_BYTES.load(Ordering::Relaxed) / 1024
}

/// What the boot said before there was a bitmap to say it to
struct Boot {
    regions: [(u64, u64); MAX_REGIONS],
    n_regions: usize,
    reserved: [(u64, u64); MAX_RESERVED],
    n_reserved: usize,
}

static BOOT: IrqMutex<Boot> = IrqMutex::new(Boot {
    regions: [(0, 0); MAX_REGIONS],
    n_regions: 0,
    reserved: [(0, 0); MAX_RESERVED],
    n_reserved: 0,
});

/// `layout` has run: the bitmap and the counts exist
static READY: AtomicBool = AtomicBool::new(false);

pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

struct FrameAllocator {
    /// A bit a frame, set when it is not free. Null until `layout`
    bitmap:          *mut u64,
    /// Frames the bitmap covers: every frame below the top of usable memory
    frames:          usize,
    total:           usize,
    used:            usize,
    free_hint:       usize,
    contiguous_hint: usize,
}

unsafe impl Send for FrameAllocator {}

impl FrameAllocator {
    const fn new() -> Self {
        Self {
            bitmap:          core::ptr::null_mut(),
            frames:          0,
            total:           0,
            used:            0,
            free_hint:       0,
            contiguous_hint: 0,
        }
    }

    #[inline(always)]
    fn word(&self, i: usize) -> u64 {
        // SAFETY: every caller indexes below `frames`, which the bitmap covers
        unsafe { *self.bitmap.add(i) }
    }

    #[inline(always)]
    fn word_mut(&mut self, i: usize) -> &mut u64 {
        // SAFETY: as `word`
        unsafe { &mut *self.bitmap.add(i) }
    }

    #[inline(always)]
    fn is_used(&self, frame: usize) -> bool {
        self.word(frame / 64) & (1u64 << (frame % 64)) != 0
    }

    // Mark a contiguous range as free using whole u64 words wherever
    // possible. The bit loops only run on the partial words at each end
    fn mark_range_free(&mut self, start: usize, end: usize) {
        if start >= end { return; }
        let mut i = start;
        let head_end = ((start + 63) & !63).min(end);
        while i < head_end {
            *self.word_mut(i / 64) &= !(1u64 << (i % 64));
            i += 1;
        }
        while i + 64 <= end {
            *self.word_mut(i / 64) = 0;
            i += 64;
        }
        while i < end {
            *self.word_mut(i / 64) &= !(1u64 << (i % 64));
            i += 1;
        }
        if start < self.free_hint       { self.free_hint = start; }
        if start < self.contiguous_hint { self.contiguous_hint = start; }
    }

    fn add_region(&mut self, base: u64, size: u64) {
        let start_frame = (base as usize + FRAME_SIZE - 1) / FRAME_SIZE;
        let end_frame   = ((base + size) as usize / FRAME_SIZE).min(self.frames);
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

    fn reserve(&mut self, base: u64, size: u64) {
        let start_frame = base as usize / FRAME_SIZE;
        let end_frame   = ((base + size + 4095) as usize / FRAME_SIZE).min(self.frames);
        if start_frame >= end_frame { return; }

        // Word-level scan: count newly-flipped (was-zero) bits, then OR-in the
        // word in one go. Far faster than bit-by-bit for multi-MB regions like
        // the kernel image or framebuffer
        let mut i = start_frame;
        let head_end = ((start_frame + 63) & !63).min(end_frame);
        while i < head_end {
            let bit = 1u64 << (i % 64);
            let w = self.word_mut(i / 64);
            if *w & bit == 0 {
                *w |= bit;
                if self.used < self.total { self.used += 1; }
            }
            i += 1;
        }
        while i + 64 <= end_frame {
            let w = self.word_mut(i / 64);
            let newly = (!*w).count_ones() as usize;
            *w = u64::MAX;
            // saturating_sub: if the bitmap were corrupted and used > total,
            // the subtraction would underflow and overstate room
            let room = self.total.saturating_sub(self.used);
            self.used += newly.min(room);
            i += 64;
        }
        while i < end_frame {
            let bit = 1u64 << (i % 64);
            let w = self.word_mut(i / 64);
            if *w & bit == 0 {
                *w |= bit;
                if self.used < self.total { self.used += 1; }
            }
            i += 1;
        }
    }

    /// `count` contiguous frames below frame `limit`
    fn alloc_frames(&mut self, count: usize, limit: usize) -> Option<u64> {
        self.alloc_frames_aligned(count, 1, limit)
    }

    /// `count` frames in one run that starts on a multiple of `align` frames
    /// (a power of two), all below frame `limit`
    fn alloc_frames_aligned(&mut self, count: usize, align: usize, limit: usize) -> Option<u64> {
        let cap          = self.frames.min(limit);
        let search_start = if count == 1 {
            self.free_hint
        } else {
            self.contiguous_hint
        }.min(cap);

        let result = self.find_contiguous(search_start, cap, count, align);
        let result = match result {
            Some(r) => Some(r),
            None if search_start > 0 => self.find_contiguous(0, search_start.min(cap), count, align),
            None => None,
        };

        if let Some(start_idx) = result {
            // Batched mark-used by 64-bit word
            let end = start_idx + count;
            let mut i = start_idx;
            let head_end = ((start_idx + 63) & !63).min(end);
            while i < head_end {
                *self.word_mut(i / 64) |= 1u64 << (i % 64);
                i += 1;
            }
            while i + 64 <= end {
                *self.word_mut(i / 64) = u64::MAX;
                i += 64;
            }
            while i < end {
                *self.word_mut(i / 64) |= 1u64 << (i % 64);
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

    /// Word-parallel free-run scan. For count==1 it scans u64 words and picks
    /// the first zero bit via 'trailing_ones'. For count>1 it walks bits but
    /// skips all-ones words and takes all-zeros words 64 at a time. A run
    /// starts only on a multiple of `align`
    fn find_contiguous(&self, from: usize, to: usize, count: usize, align: usize) -> Option<usize> {
        if from >= to || count == 0 { return None; }

        if count == 1 && align == 1 {
            let mut i = from;
            let head_end = ((from + 63) & !63).min(to);
            while i < head_end {
                if self.word(i / 64) & (1u64 << (i % 64)) == 0 {
                    return Some(i);
                }
                i += 1;
            }
            while i + 64 <= to {
                let w = self.word(i / 64);
                if w != u64::MAX {
                    return Some(i + w.trailing_ones() as usize);
                }
                i += 64;
            }
            while i < to {
                if self.word(i / 64) & (1u64 << (i % 64)) == 0 {
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
            // A run may start only where it is aligned; in between, nothing
            // free counts towards one
            if consecutive == 0 && i % align != 0 {
                i = (i | (align - 1)) + 1;
                continue;
            }
            if consecutive == 0 && i % 64 == 0 && i + 64 <= to {
                let w = self.word(i / 64);
                if w == u64::MAX { i += 64; continue; }
                if w == 0 {
                    start_idx = i;
                    consecutive = 64;
                    i += 64;
                    if consecutive >= count { return Some(start_idx); }
                    continue;
                }
            }
            if self.word(i / 64) & (1u64 << (i % 64)) == 0 {
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
        let start_frame = phys as usize / FRAME_SIZE;
        let end_frame   = (start_frame + count).min(self.frames);
        for i in start_frame..end_frame {
            // In a CPU's cache: free already, and handed out from there next
            if refs((i * FRAME_SIZE) as u64).is_some_and(|r| r.load(Ordering::Relaxed) == CACHED) {
                crate::kwarn!("[pmm] frame {:#x} freed twice; the second free is ignored", i * FRAME_SIZE);
                continue;
            }
            if self.is_used(i) {
                *self.word_mut(i / 64) &= !(1u64 << (i % 64));
                self.used -= 1;
            }
        }
        if start_frame < self.free_hint       { self.free_hint = start_frame; }
        if start_frame < self.contiguous_hint { self.contiguous_hint = start_frame; }
    }
}

// Masks interrupts while held: a holder interrupted on its own CPU must not
// wait on itself
static PMM: IrqMutex<FrameAllocator> = IrqMutex::new(FrameAllocator::new());

/// Lays the allocator out over the memory the boot has named: the bitmap and
/// the counts sized by the top of usable memory, placed in the first usable run
/// below 4 GiB that nothing reserved touches, then the regions freed and the
/// reservations taken out. Once, after the last reservation the boot knows of
/// (`pstore` included)
pub fn layout() {
    let boot = BOOT.lock();
    let regions = &boot.regions[..boot.n_regions];
    let reserved = &boot.reserved[..boot.n_reserved];

    let top = regions.iter().map(|&(b, l)| b + l).max().unwrap_or(0);
    let frames = (top as usize / FRAME_SIZE + 63) & !63;
    let bitmap_bytes = (frames / 8 + FRAME_SIZE - 1) & !(FRAME_SIZE - 1);
    let refs_bytes = (frames * 2 + FRAME_SIZE - 1) & !(FRAME_SIZE - 1);
    let need = (bitmap_bytes + refs_bytes) as u64;

    // The first place a region has that much room clear of every reservation,
    // above the first megabyte. Candidates are where a region starts and
    // where a reservation inside it ends
    let clear = |at: u64| reserved.iter().all(|&(b, l)| at + need <= b || at >= b + l);
    let mut place = None;
    'regions: for &(base, len) in regions {
        let end = base + len;
        let mut candidates = [0u64; MAX_RESERVED + 1];
        candidates[0] = base;
        for (i, &(b, l)) in reserved.iter().enumerate() {
            candidates[i + 1] = b + l;
        }
        for &c in &candidates[..reserved.len() + 1] {
            let at = (c.max(base).max(1 << 20) + FRAME_SIZE as u64 - 1) & !(FRAME_SIZE as u64 - 1);
            if at + need <= end && at + need <= 1 << 32 && clear(at) {
                place = Some(at);
                break 'regions;
            }
        }
    }
    let Some(at) = place else {
        panic!("pmm: no room for the frame bitmap ({} KiB) in {} regions", need / 1024, regions.len());
    };

    let virt = crate::arch::x86_64::grub::phys_to_virt(at) as *mut u8;
    // SAFETY: usable RAM that no reservation covers, reached through the
    // direct map, and nobody else's until it is reserved below
    unsafe {
        core::ptr::write_bytes(virt, 0xFF, bitmap_bytes);
        core::ptr::write_bytes(virt.add(bitmap_bytes), 0, refs_bytes);
    }
    REFS.store(unsafe { virt.add(bitmap_bytes) } as *mut AtomicU16, Ordering::Release);
    REFS_LEN.store(frames, Ordering::Release);

    let mut pmm = PMM.lock();
    pmm.bitmap = virt as *mut u64;
    pmm.frames = frames;
    for &(base, len) in regions {
        pmm.add_region(base, len);
    }
    for &(base, len) in reserved {
        pmm.reserve(base, len);
    }
    pmm.reserve(at, need);
    crate::serial_println!(
        "[pmm] {} frames up to {:#x}: bitmap and counts ({} KiB) at {:#x}, {} MiB usable",
        frames, top, need / 1024, at, pmm.total * FRAME_SIZE >> 20
    );
    drop(pmm);
    READY.store(true, Ordering::Release);
}

/// Guard holding the PMM lock for a batched free (tearing down an address
/// space), so the global lock is not re-acquired per freed frame
pub struct PmmGuard<'a> {
    inner: IrqMutexGuard<'a, FrameAllocator>,
}

impl<'a> PmmGuard<'a> {
    #[inline]
    pub fn free_frame(&mut self, phys: u64) {
        self.inner.free_frames(phys, 1);
    }

    #[inline]
    pub fn free_frames(&mut self, phys: u64, count: usize) {
        self.inner.free_frames(phys, count);
    }

    #[inline]
    pub fn alloc_frame(&mut self) -> Option<u64> {
        let f = self.inner.alloc_frames(1, usize::MAX);
        if let Some(p) = f { fresh(p, 1); }
        f
    }

    #[inline]
    pub fn alloc_frames(&mut self, count: usize) -> Option<u64> {
        let f = self.inner.alloc_frames(count, usize::MAX);
        if let Some(p) = f { fresh(p, count); }
        f
    }
}

pub fn lock_for_batch() -> PmmGuard<'static> {
    PmmGuard { inner: PMM.lock() }
}

/// Refcount-aware free using a pre-acquired PMM guard. Returns true if
/// the frame was actually returned to the allocator (refcount hit 0).
/// Kept for non-CoW callers that have no SwapMapGuard handy
pub fn free_frame_cow_batched(pmm: &mut PmmGuard<'_>, phys: u64) -> bool {
    let should_free = !drop_shared(phys);
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
    swap: &mut crate::mm::swap_map::SwapMapGuard<'_>,
    phys: u64,
) -> bool {
    match put_cow_ref(phys) {
        CowPut::Last => {
            swap.untrack(phys);
            pmm.free_frame(phys);
            true
        }
        CowPut::Unique => {
            // Last sibling gone; the surviving mapping is unique now,
            // so the frame is swappable again. swap_map entry still
            // points at whichever cr3/virt registered it at map_range
            swap.set_pinned(phys, false);
            false
        }
        CowPut::Shared => false,
    }
}

const EMERGENCY_POOL_SIZE: usize = 64;
static EMERGENCY_POOL: IrqMutex<EmergencyPool> = IrqMutex::new(EmergencyPool::new());

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
    let f = EMERGENCY_POOL.lock().pop();
    if let Some(p) = f { fresh(p, 1); }
    f
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
            match pmm.alloc_frames(1, usize::MAX) {
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

/// A usable region of the memory map. Written down until `layout`
pub fn add_region(base: u64, size: u64) {
    if ready() {
        PMM.lock().add_region(base, size);
        return;
    }
    let mut b = BOOT.lock();
    if b.n_regions == MAX_REGIONS {
        crate::serial_println!("[pmm] warn: more than {} usable regions; {:#x}+{:#x} left out", MAX_REGIONS, base, size);
        return;
    }
    let n = b.n_regions;
    b.regions[n] = (base, size);
    b.n_regions += 1;
}

/// Memory that is not to be handed out. Written down until `layout`, which
/// keeps the bitmap clear of it too
pub fn reserve_region(base: u64, size: u64) {
    if ready() {
        PMM.lock().reserve(base, size);
        return;
    }
    let mut b = BOOT.lock();
    if b.n_reserved == MAX_RESERVED {
        panic!("pmm: more than {} reservations before layout", MAX_RESERVED);
    }
    let n = b.n_reserved;
    b.reserved[n] = (base, size);
    b.n_reserved += 1;
}

pub fn alloc_frame() -> Option<u64> {
    if let Some(f) = cache_pop() {
        return Some(f);
    }
    let f = PMM.lock().alloc_frames(1, usize::MAX);
    if let Some(p) = f { fresh(p, 1); }
    f
}

pub fn alloc_frames(count: usize) -> Option<u64> {
    if count == 1 {
        return alloc_frame();
    }
    let f = PMM.lock().alloc_frames(count, usize::MAX);
    if let Some(p) = f { fresh(p, count); }
    f
}

/// `count` contiguous frames that all lie below `limit` (a physical
/// address): what a device that addresses 32 bits can be given
pub fn alloc_frames_below(count: usize, limit: u64) -> Option<u64> {
    alloc_frames_aligned(count, 1, limit)
}

/// `count` contiguous frames starting on a multiple of `align` frames (a
/// power of two), all below `limit` (a physical address, `u64::MAX` for
/// anywhere): a slab aligned to its own size, a buffer a device must not see
/// cross a boundary
pub fn alloc_frames_aligned(count: usize, align: usize, limit: u64) -> Option<u64> {
    debug_assert!(align.is_power_of_two());
    let limit = if limit == u64::MAX { usize::MAX } else { limit as usize / FRAME_SIZE };
    let f = PMM.lock().alloc_frames_aligned(count, align.max(1), limit);
    if let Some(p) = f { fresh(p, count); }
    f
}

pub fn free_frame(phys: u64) {
    if cache_push(phys) {
        return;
    }
    PMM.lock().free_frames(phys, 1);
}

pub fn free_frames(phys: u64, count: usize) {
    if count == 1 {
        free_frame(phys);
        return;
    }
    PMM.lock().free_frames(phys, count);
}

pub fn stats() -> (usize, usize) {
    let p = PMM.lock();
    (p.used, p.total)
}

/// Free frames below a sixteenth of the machine: where kswapd starts giving
/// back what is only kept to be quick (cached file pages, spare kernel stacks)
pub fn running_low() -> bool {
    let (used, total) = stats();
    total.saturating_sub(used) < total / 16
}

// Copy-on-write counts
//
// One `AtomicU16` a frame, holding how many holders the frame has besides the
// first: zero is one holder, which every frame has unless `fork`, a shared
// library or a shared mapping made it more. So the array is zeroed at `layout`
// and a frame needs nothing written to start out right. `CACHED` is the one
// value that is not a count: a frame in a CPU's cache

static REFS: AtomicPtr<AtomicU16> = AtomicPtr::new(core::ptr::null_mut());
static REFS_LEN: AtomicUsize = AtomicUsize::new(0);

/// A frame that is in a CPU's cache, free though its bit says used
const CACHED: u16 = u16::MAX;
/// The most holders past the first a count records
const MOST: u16 = u16::MAX - 1;

fn refs(phys: u64) -> Option<&'static AtomicU16> {
    let frame = phys as usize / FRAME_SIZE;
    let base = REFS.load(Ordering::Acquire);
    if base.is_null() || frame >= REFS_LEN.load(Ordering::Relaxed) {
        return None;
    }
    // SAFETY: inside the array `layout` made, which lives for ever
    Some(unsafe { &*base.add(frame) })
}

/// Frames just allocated start with one holder, whatever a careless free
/// left in their counts
fn fresh(phys: u64, count: usize) {
    for i in 0..count {
        if let Some(r) = refs(phys + (i * FRAME_SIZE) as u64) {
            r.store(0, Ordering::Relaxed);
        }
    }
}

/// One holder fewer, if there were more than one: true when somebody else
/// still holds the frame (so it must not be freed), false when the caller was
/// the only one
fn drop_shared(phys: u64) -> bool {
    let Some(r) = refs(phys) else { return false };
    r.fetch_update(Ordering::AcqRel, Ordering::Acquire, |e| match e {
        0 | CACHED => None,
        e => Some(e - 1),
    }).is_ok()
}

pub fn ref_inc(phys: u64) {
    if let Some(r) = refs(phys) {
        let _ = r.fetch_update(Ordering::AcqRel, Ordering::Acquire, |e| (e < MOST).then(|| e + 1));
    }
}

/// One holder fewer. The count left, or 0 when there was only one holder
/// and nothing was changed
pub fn ref_dec(phys: u64) -> u16 {
    let Some(r) = refs(phys) else { return 0 };
    match r.fetch_update(Ordering::AcqRel, Ordering::Acquire, |e| match e {
        0 | CACHED => None,
        e => Some(e - 1),
    }) {
        Ok(was) => was,
        Err(_) => 0,
    }
}

/// One more holder of `phys`, for a reader that copies from a frame it does
/// not own once the lock that found it is let go (`FileCache::pin`). False
/// when the frame keeps no count, and `free_frame_cow` would free it from
/// under its owner: such a frame is not to be pinned
pub fn try_ref(phys: u64) -> bool {
    refs(phys).is_some_and(|r| {
        r.fetch_update(Ordering::AcqRel, Ordering::Acquire, |e| (e < MOST).then(|| e + 1)).is_ok()
    })
}

pub fn ref_get(phys: u64) -> u16 {
    match refs(phys).map(|r| r.load(Ordering::Acquire)) {
        Some(CACHED) | None => 1,
        Some(e) => e + 1,
    }
}

/// What letting go of one copy-on-write reference left behind
pub enum CowPut {
    /// Somebody else still shares it
    Shared,
    /// One holder left, who owns it outright now
    Unique,
    /// That was the last reference: the caller frees the frame
    Last,
}

/// Lets go of one reference to `phys`, deciding in one step what that means.
///
/// For a copy-on-write break, which reads the count, copies, and only then lets
/// go. The other holder can let go in between (a child `exec`ing drops its
/// whole address space), so the frame has to be freed by whichever goes last
pub fn put_cow_ref(phys: u64) -> CowPut {
    let Some(r) = refs(phys) else { return CowPut::Last };
    match r.fetch_update(Ordering::AcqRel, Ordering::Acquire, |e| match e {
        0 | CACHED => None,
        e => Some(e - 1),
    }) {
        Ok(1) => CowPut::Unique,
        Ok(_) => CowPut::Shared,
        Err(_) => CowPut::Last,
    }
}

pub fn free_frame_cow(phys: u64) {
    if !drop_shared(phys) {
        free_frame(phys);
    }
}

// Per-CPU frame caches
//
// A page fault and a copy-on-write break each want one frame. A CPU keeps a few
// free frames of its own, touched only with interrupts off on that CPU, and
// goes to the shared bitmap for them `BATCH` at a time. A cached frame's count
// reads `CACHED`, so a second free of it is recognised and ignored

const CACHE: usize = 32;
const BATCH: usize = 16;

struct FrameCache {
    frames: [u64; CACHE],
    count: usize,
}

static CACHES: [IrqMutex<FrameCache>; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { IrqMutex::new(FrameCache { frames: [0; CACHE], count: 0 }) }; crate::arch::x86_64::percpu::MAX_CPUS];

fn cache_pop() -> Option<u64> {
    if !ready() { return None; }
    let mut c = CACHES[crate::arch::x86_64::percpu::current_index()].lock();
    if c.count == 0 {
        let mut pmm = PMM.lock();
        while c.count < BATCH {
            match pmm.alloc_frames(1, usize::MAX) {
                Some(f) => {
                    let n = c.count;
                    c.frames[n] = f;
                    c.count += 1;
                    if let Some(r) = refs(f) { r.store(CACHED, Ordering::Relaxed); }
                }
                None => break,
            }
        }
    }
    if c.count == 0 { return None; }
    c.count -= 1;
    let f = c.frames[c.count];
    if let Some(r) = refs(f) { r.store(0, Ordering::Relaxed); }
    Some(f)
}

/// True when the frame went into this CPU's cache, or was already in one
fn cache_push(phys: u64) -> bool {
    if !ready() { return false; }
    let phys = phys & !(FRAME_SIZE as u64 - 1);
    let Some(r) = refs(phys) else { return false };
    if r.load(Ordering::Acquire) == CACHED {
        crate::kwarn!("[pmm] frame {:#x} freed twice; the second free is ignored", phys);
        return true;
    }
    let mut c = CACHES[crate::arch::x86_64::percpu::current_index()].lock();
    if c.count == CACHE {
        let mut pmm = PMM.lock();
        for _ in 0..BATCH {
            c.count -= 1;
            let f = c.frames[c.count];
            if let Some(r) = refs(f) { r.store(0, Ordering::Relaxed); }
            pmm.free_frames(f, 1);
        }
    }
    r.store(CACHED, Ordering::Relaxed);
    let n = c.count;
    c.frames[n] = phys;
    c.count += 1;
    true
}
