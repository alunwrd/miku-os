// Who owns which bytes of video memory
//
// A wrong video-memory address does not fault: it hands somebody else's bytes
// to an engine, and the first symptom is on the screen (`dev/gb206-display.md`,
// section 6.1). So: one place knowing the layout, a name on every block, and a
// free that can be called. `nvidia vram` prints it all.
//
// Why this is not `mm::pmm`
//
// `pmm` is a bitmap over millions of 4 KiB frames, taken from interrupt context
// through an `IrqMutex`. Here there are a few dozen blocks, one page to
// sixty-four mebibytes, over sixteen gibibytes, and nothing asks from an
// interrupt. A sorted list of free ranges under an ordinary lock is enough.

extern crate alloc;

use alloc::vec::Vec;
use spin::Mutex;

use super::staticinfo::StaticInfo;

/// Which end of memory a block should come from.
///
/// * `Low` is where a scanout surface lives. The top of video memory is
///   GSP-RM's write-protected region and its heap, and a dump of this card
///   under the shipping driver has its surface at `0x04e00000`. A window whose
///   fetch is refused reads back black.
/// * `High` is where the display's instance memory comes from.
/// * `Below(limit)` is the top of what lies under `limit`, coming down: a
///   program's video memory (`nvidia/user_vram.rs`), which the CPU reaches
///   through BAR1, so inside its aperture. From the top of that window, where
///   `Low` starts from the bottom: the two meet only when the window is full
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Placement {
    Low,
    High,
    Below(u64),
}

/// A span of video memory. `base` is a GPU physical address
#[derive(Copy, Clone, Debug)]
pub struct Range {
    pub base: u64,
    pub size: u64,
}

impl Range {
    fn end(&self) -> u64 {
        self.base + self.size
    }
}

/// A span handed out, and to whom. A leak shows up as a name in `nvidia vram`
#[derive(Copy, Clone, Debug)]
pub struct Block {
    pub base: u64,
    pub size: u64,
    pub owner: &'static str,
}

struct Vram {
    /// Free spans, sorted by base, never overlapping, always coalesced
    free: Vec<Range>,
    /// Handed out, sorted by base
    used: Vec<Block>,
    /// Every region GSP-RM reported, usable or not, so the map can show what
    /// was excluded and why
    regions: Vec<(Range, bool)>,
    /// Set once `init` has run. An allocation earlier runs before GSP-RM has
    /// said anything about the framebuffer, a mistake to report
    seeded: bool,
}

static VRAM: Mutex<Vram> = Mutex::new(Vram {
    free: Vec::new(),
    used: Vec::new(),
    regions: Vec::new(),
    seeded: false,
});

/// Seed the allocator from what GSP-RM says the board has.
///
/// Only regions `FbRegion::usable()` accepts become free space, nouveau's rule
/// from `r535_gsp_get_static_info_fb`: nothing reserved by RM, not protected,
/// and both compression and isochronous access supported. Scanout needs ISO.
///
/// The firmware console needs no reservation here: `sysinfo::uefi_console_size`
/// reports it to GSP-RM as `consoleMemSize`, and GSP-RM marks that memory
/// reserved, taking it out of `usable()`. The map prints the unusable regions
/// too
pub fn init(si: &StaticInfo) {
    let mut v = VRAM.lock();
    v.free.clear();
    v.used.clear();
    v.regions.clear();
    for r in si.regions.iter() {
        let span = Range { base: r.base, size: r.size() };
        v.regions.push((span, r.usable()));
        if r.usable() {
            v.free.push(span);
        }
    }
    v.free.sort_by_key(|r| r.base);
    v.regions.sort_by_key(|(r, _)| r.base);
    v.seeded = true;
}

/// GSP-RM was unloaded: the layout this was seeded from belongs to it, and the
/// next boot seeds again from its own (`init`)
pub fn forget() {
    let mut v = VRAM.lock();
    v.free.clear();
    v.used.clear();
    v.regions.clear();
    v.seeded = false;
}

pub fn seeded() -> bool {
    VRAM.lock().seeded
}

/// The GPU page a block of `bytes` is mapped with: `memmgrDeterminePageSize`
/// for video memory, by default (`dev/gpu-memory-compute.md`, 3.8):
///
///   2 MiB or more         2 MiB  (`bHugePageSupported` on every GB20x)
///   64 KiB or more        64 KiB (`defaultBigPageSize`)
///   smaller               4 KiB
///
/// A program's allocation is a client's (`hClient`), which OpenRM gives the big
/// page whatever its size; `user_vram` asks for at least 64 KiB for that
pub fn page_for(bytes: u64) -> u64 {
    const HUGE: u64 = 2 << 20;
    const BIG: u64 = 64 << 10;
    if bytes >= HUGE {
        HUGE
    } else if bytes >= BIG {
        BIG
    } else {
        0x1000
    }
}

/// What a block of `bytes` takes: the size padded to its page. OpenRM pads
/// every allocation so (`memmgrAllocDetermineAlignment_GM107`), and mapping a
/// block whole with its own page size needs the end aligned as well as the
/// start
pub fn padded(bytes: u64) -> u64 {
    bytes.next_multiple_of(page_for(bytes))
}

/// Hand out `bytes` aligned to `align`, from whichever end `place` asks for.
///
/// First fit going up for `Low`, last fit coming down for `High`.
///
/// The block is placed and sized for its GPU page (`page_for`): the base on
/// that page's boundary as well as `align`'s, the size padded to it. A caller
/// mapping the block whole asks `block_size`
pub fn alloc(bytes: u64, align: u64, place: Placement, owner: &'static str) -> Option<u64> {
    if bytes == 0 {
        return None;
    }
    let align = align.max(page_for(bytes));
    let bytes = padded(bytes);
    let mut v = VRAM.lock();
    if !v.seeded {
        return None;
    }
    let found = find(&v.free, bytes, align, place)?;
    v.take(Range { base: found, size: bytes }, owner).then_some(found)
}

/// How much the block at `base` holds, padding included. `None` for an address
/// not the start of a block handed out here: the firmware's console surface,
/// say, GSP-RM's memory, mapped no further than the caller knows it goes
pub fn block_size(base: u64) -> Option<u64> {
    VRAM.lock().used.iter().find(|b| b.base == base).map(|b| b.size)
}

/// The block handed out here holding `addr`, as `(base, size)`: a client's
/// window buffer can start part way into its pool
/// (`dev/gpu-window-buffers.md`), and the effects pass maps the whole block
/// once for every buffer in it
pub fn block_containing(addr: u64) -> Option<(u64, u64)> {
    containing(&VRAM.lock().used, addr)
}

/// The decision alone, over a list of blocks (sorted by base or not), for
/// `nvidia selftest`
pub fn containing(used: &[Block], addr: u64) -> Option<(u64, u64)> {
    used.iter()
        .find(|b| addr >= b.base && addr - b.base < b.size)
        .map(|b| (b.base, b.size))
}

/// Where in `free` (sorted by base) a block of `bytes` aligned to `align` goes
/// for `place`. The decision alone, for `nvidia selftest`
pub fn find(free: &[Range], bytes: u64, align: u64, place: Placement) -> Option<u64> {
    let align = align.max(1);
    match place {
        Placement::Low => free.iter().find_map(|r| {
            let at = (r.base + align - 1) & !(align - 1);
            (at + bytes <= r.end()).then_some(at)
        }),
        Placement::High => free.iter().rev().find_map(|r| {
            // Align the end down, so the block sits as high as it can
            let at = r.end().checked_sub(bytes)? & !(align - 1);
            (at >= r.base && at + bytes <= r.end()).then_some(at)
        }),
        // The same, with each span's end cut to the limit
        Placement::Below(limit) => free.iter().rev().find_map(|r| {
            let end = r.end().min(limit);
            let at = end.checked_sub(bytes)? & !(align - 1);
            (at >= r.base && at + bytes <= end).then_some(at)
        }),
    }
}

/// Give a block back. `base` is exactly what `alloc` returned.
///
/// False means no such block: a double free or an address not from here. Both
/// are worth seeing: video memory returned twice is handed to two owners next
/// time
pub fn free(base: u64) -> bool {
    let mut v = VRAM.lock();
    let Some(i) = v.used.iter().position(|b| b.base == base) else {
        return false;
    };
    let b = v.used.remove(i);
    v.give_back(Range { base: b.base, size: b.size });
    true
}

/// Change who a block belongs to, without moving it.
///
/// The two surfaces a compositor flips between are interchangeable, and after
/// an odd number of flips the one on screen was allocated as the second. This
/// keeps the map's label true
pub fn rename(base: u64, owner: &'static str) -> bool {
    let mut v = VRAM.lock();
    match v.used.iter_mut().find(|b| b.base == base) {
        Some(b) => { b.owner = owner; true }
        None => false,
    }
}

/// Everything the shell prints, under one lock so the three halves are from one
/// moment
pub struct Map {
    pub regions: Vec<(Range, bool)>,
    pub free: Vec<Range>,
    pub used: Vec<Block>,
}

pub fn map() -> Map {
    let v = VRAM.lock();
    Map {
        regions: v.regions.clone(),
        free: v.free.clone(),
        used: v.used.clone(),
    }
}

/// (free bytes, used bytes)
pub fn totals() -> (u64, u64) {
    let v = VRAM.lock();
    (
        v.free.iter().map(|r| r.size).sum(),
        v.used.iter().map(|b| b.size).sum(),
    )
}

impl Vram {
    /// Cut `span` out of the free list and record it as `owner`'s.
    ///
    /// The span is inside one free range: adjacent free ranges coalesce, so a
    /// span crossing a boundary crosses into something handed out
    fn take(&mut self, span: Range, owner: &'static str) -> bool {
        let Some(i) = self
            .free
            .iter()
            .position(|r| span.base >= r.base && span.end() <= r.end())
        else {
            return false;
        };
        let r = self.free[i];
        // What is left of the range on either side of the hole. Either may be
        // empty, and if both are the range disappears
        let left = Range { base: r.base, size: span.base - r.base };
        let right = Range { base: span.end(), size: r.end() - span.end() };
        self.free.remove(i);
        if right.size > 0 {
            self.free.insert(i, right);
        }
        if left.size > 0 {
            self.free.insert(i, left);
        }
        let at = self.used.partition_point(|b| b.base < span.base);
        self.used.insert(at, Block { base: span.base, size: span.size, owner });
        true
    }

    /// Put a span back and merge it with whatever it touches.
    ///
    /// Without coalescing a surface freed and asked for again comes back as two
    /// ranges neither big enough
    fn give_back(&mut self, span: Range) {
        let at = self.free.partition_point(|r| r.base < span.base);
        self.free.insert(at, span);
        let mut i = 0;
        while i + 1 < self.free.len() {
            if self.free[i].end() == self.free[i + 1].base {
                self.free[i].size += self.free[i + 1].size;
                self.free.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }
}
