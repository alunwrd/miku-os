// A program's video memory
//
// 3.7 of `dev/gpu-memory-compute.md`. A block of video memory a process can
// write with ordinary stores and the GPU can read at its own speed: the block
// lies under BAR1's aperture, and BAR1 is in physical mode, so its CPU address
// is the aperture's base plus its offset in video memory (`mapping_cpu.c`;
// `blackwell/bar1.rs`). A process holds a pool (`mayland::server::shm`,
// `Backing::Video`), so ownership, the per-process limit and teardown at exit
// are the pool's; this is the memory under it.
//
// Where
//
// From the top of the aperture down (`Placement::Below`). The kernel's own
// blocks (surfaces, LUTs, the cursor) come from the bottom (`Low`), so the two
// meet only when the window is full. 128 MiB for all programs together, half
// the window.
//
// Given and taken back
//
// Zeroed before anybody sees it. Taken back only once nothing can reach it: out
// of every GPU address space, then both engines waited out, then the allocator.
// A block the engines would not let go of is kept and renamed, never handed to
// the next caller.

use core::sync::atomic::{AtomicU64, Ordering};

use super::gmmu::Aperture;
use super::gsp_common::vram::{self, Placement};

/// What all programs together may hold
pub const LIMIT: u64 = 128 << 20;

/// The least a program is given: one big page (3.8)
const BIG_PAGE: u64 = 64 << 10;

/// What they hold now, in bytes
static HELD: AtomicU64 = AtomicU64::new(0);

/// Why a program was not given video memory
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
    /// No GB20x in this machine, or its GSP-RM is not up
    NoCard,
    /// BAR1 is not in physical mode (`nobar1`, or GSP-RM came up without it):
    /// the block has no CPU address
    NoWindow,
    /// All programs together would hold more than `LIMIT`
    Quota,
    /// No room under the aperture
    NoMemory,
}

impl Refused {
    pub fn as_str(self) -> &'static str {
        match self {
            Refused::NoCard => "there is no GPU with video memory to give",
            Refused::NoWindow => "BAR1 is not in physical mode, so video memory has no CPU address",
            Refused::Quota => "programs already hold all the video memory they may",
            Refused::NoMemory => "no room left in the part of video memory BAR1 reaches",
        }
    }
}

/// A block of at least `bytes`, zeroed: `(vram, cpu_phys, bytes)`, the real
/// size, padded to its GPU page so all of it can be mapped with that page
pub fn alloc(bytes: u64) -> Result<(u64, u64, u64), Refused> {
    if super::device().is_none() || !super::blackwell::boot::is_booted() || !vram::seeded() {
        return Err(Refused::NoCard);
    }
    let ap = super::blackwell::bar1::physical().ok_or(Refused::NoWindow)?;
    // A program's allocation is a client's, and OpenRM gives a client the big
    // page whatever it asked for (`memmgrDeterminePageSize`, `hClient`): 64 KiB
    // at the least, 2 MiB pages from 2 MiB up (`vram::page_for`)
    let bytes = vram::padded(bytes.max(BIG_PAGE));
    // Counted before the allocation, and given back if it fails: two callers
    // checking the total and then allocating would together pass it
    if HELD.fetch_add(bytes, Ordering::AcqRel) + bytes > LIMIT {
        HELD.fetch_sub(bytes, Ordering::AcqRel);
        return Err(Refused::Quota);
    }
    let Some(base) = vram::alloc(bytes, 0x1000, Placement::Below(ap.size), "program video memory") else {
        HELD.fetch_sub(bytes, Ordering::AcqRel);
        return Err(Refused::NoMemory);
    };
    // Through a write-combining window of the kernel's own, the whole block
    let Some(p) = super::blackwell::bar1::map(base, bytes, true) else {
        vram::free(base);
        HELD.fetch_sub(bytes, Ordering::AcqRel);
        return Err(Refused::NoWindow);
    };
    unsafe { core::ptr::write_bytes(p, 0, bytes as usize) };
    // Write-combined stores are not ordered with what follows until fenced
    core::sync::atomic::fence(Ordering::SeqCst);
    Ok((base, ap.phys + base, bytes))
}

/// Give a block back, once nothing can reach it.
///
/// Out of every GPU address space now, so nothing new is sent at it; the wait
/// for what the engines have, and the free, on a kernel task. A pool goes away
/// on the exit path too, where the reaper should not sit for two seconds on a
/// fence
pub fn retire(base: u64, bytes: u64) {
    if let Some(d) = super::device() {
        super::vaspace::unmap_phys(&d.bar0, base, bytes, Aperture::Vram);
    }
    crate::sched::submit_task(move || {
        let idle = super::blackwell::boot::ce_wait_last(2_000_000)
            && super::blackwell::boot::compute_wait_last(2_000_000);
        if !idle {
            vram::rename(base, "program video memory the engines held");
            crate::kwarn!(
                "[nvidia] {} KiB of program video memory at {:#x} kept: the engines did not finish with it",
                bytes >> 10, base);
            return;
        }
        vram::free(base);
        HELD.fetch_sub(bytes, Ordering::AcqRel);
    });
}

/// GSP-RM is going away with programs' video memory revoked
/// (`shm::revoke_video`): the allocator is reset after this, and the blocks
/// with it, so nothing is owed
pub fn forget() {
    HELD.store(0, Ordering::Release);
}

/// `(held, limit)` in bytes, for `gpu_info`
pub fn held() -> (u64, u64) {
    (HELD.load(Ordering::Acquire), LIMIT)
}
