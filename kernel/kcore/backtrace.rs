// Walking a stack back to the calls that led here.
//
// The tree is built with `-C force-frame-pointers=yes`, so every frame begins
// with the caller's RBP followed by the return address: unwinding is a
// linked-list walk, read [rbp], read [rbp+8], repeat. `.eh_frame` is not
// decoded.
//
// The walk has to survive two things. Frames it cannot see: assembly entry
// points and `#[naked]` functions push nothing, so a chain can end early and
// hide the caller. And frames that are not frames: a corrupted RBP could send
// the walk through arbitrary memory. So the chain is checked at every step
// (strictly increasing, inside the stack it started in, aligned), and when it
// ends before the stack does, the remainder is scanned for anything resembling
// a return address, reported separately and marked as a guess.

extern crate alloc;
use alloc::vec::Vec;

/// Deepest chain followed. A real stack is tens of frames
pub const MAX_FRAMES: usize = 32;
/// How many words of stack the fallback scan will look at
const SCAN_WORDS: usize = 256;

pub struct Frame {
    pub pc: u64,
    /// True when this came from the scan, not the frame chain, and may not be a
    /// real caller. Reported with a `?`
    pub guessed: bool,
}

/// Walk the frame chain starting at `rbp`, inside [`stack_lo`, `stack_hi`).
/// `read` fetches one u64 and says whether it could, so the same walk serves
/// kernel memory and, via a checked read, another process's stack.
fn walk(
    mut rbp: u64,
    stack_lo: u64,
    stack_hi: u64,
    mut read: impl FnMut(u64) -> Option<u64>,
) -> Vec<Frame> {
    let mut out = Vec::new();
    let mut last = 0u64;

    while out.len() < MAX_FRAMES {
        // A frame pointer is 8-aligned, inside the stack, and above the one
        // before it. Any of those failing ends the chain, normally or through
        // damage
        if rbp & 7 != 0 || rbp <= last || rbp < stack_lo {
            break;
        }
        let ret_at = match rbp.checked_add(8) {
            Some(v) if v + 8 <= stack_hi => v,
            _ => break,
        };

        let next = match read(rbp) { Some(v) => v, None => break };
        let pc = match read(ret_at) { Some(v) => v, None => break };
        if pc == 0 {
            break;
        }
        out.push(Frame { pc, guessed: false });
        last = rbp;
        rbp = next;
    }
    out
}

/// The chain for the currently running kernel thread.
pub fn kernel_frames() -> Vec<Frame> {
    let rbp: u64;
    unsafe { core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack)) };
    kernel_frames_from(rbp)
}

/// The chain from a given RBP: what a fault handler has, as the faulting
/// frame's RBP is in the saved register set.
pub fn kernel_frames_from(rbp: u64) -> Vec<Frame> {
    // The bounds of the stack this thread is on. Anything outside it is
    // not a frame, whatever it looks like
    let (lo, hi) = match crate::sched::current_kernel_stack_bounds() {
        Some(b) => b,
        // No scheduler yet (early boot): the walk is bounded to the page the
        // frame pointer is in
        None => (rbp & !0xFFF, (rbp & !0xFFF) + 0x4000),
    };
    walk(rbp, lo, hi, |addr| {
        if addr < lo || addr + 8 > hi {
            return None;
        }
        Some(unsafe { (addr as *const u64).read_volatile() })
    })
}

/// The chain for a user process, read out of its address space.
///
/// `cr3` is the process's, possibly not the one loaded now: a fault handler
/// runs on the faulting process's CR3, a report written from elsewhere does
/// not. Reads go through the page tables, and no user pointer is dereferenced
/// directly (SMAP would fault).
pub fn user_frames(cr3: u64, rbp: u64, rsp: u64, stack_lo: u64, stack_hi: u64) -> Vec<Frame> {
    let mut frames = walk(rbp, stack_lo, stack_hi, |addr| read_user_u64(cr3, addr));

    // The chain ended early: usually a leaf reached through assembly, or a
    // smashed RBP. Sweep the stack for values landing inside a mapped
    // executable page and offer them, marked.
    //
    // From RSP up, where a leaf's return address is. A leaf using RBP as a
    // scratch register (`memcpy`) leaves any value there
    if frames.len() < 2 {
        let start = rsp.clamp(stack_lo, stack_hi);
        let mut addr = start & !7;
        let mut seen = 0usize;
        while seen < SCAN_WORDS && addr + 8 <= stack_hi && frames.len() < MAX_FRAMES {
            if let Some(v) = read_user_u64(cr3, addr) {
                if looks_executable(cr3, v) {
                    frames.push(Frame { pc: v, guessed: true });
                }
            }
            addr += 8;
            seen += 1;
        }
    }
    frames
}

/// One u64 out of another address space. `vmm::copy_from_user` walks the page
/// tables and goes in through the HHDM, so SMAP stays armed and any CR3 works.
/// A word straddling a page boundary is refused: a frame pointer that unaligned
/// is not one
fn read_user_u64(cr3: u64, uva: u64) -> Option<u64> {
    if uva & 0xFFF > 4096 - 8 {
        return None;
    }
    let aspace = crate::mm::vmm::AddressSpace::from_raw(cr3);
    let mut b = [0u8; 8];
    let n = aspace.copy_from_user(uva, &mut b);
    let _ = aspace.into_raw();
    if n != 8 {
        return None;
    }
    Some(u64::from_le_bytes(b))
}

/// Whether `addr` points into a mapped, executable user page, so the fallback
/// scan does not report data as call sites
fn looks_executable(cr3: u64, addr: u64) -> bool {
    use x86_64::structures::paging::PageTableFlags as F;
    if !crate::mm::vmm::is_user_va(addr) {
        return false;
    }
    let aspace = crate::mm::vmm::AddressSpace::from_raw(cr3);
    let flags = aspace.get_page_flags(addr & !0xFFF);
    let _ = aspace.into_raw();
    match flags {
        Some(f) => {
            f.contains(F::PRESENT)
                && f.contains(F::USER_ACCESSIBLE)
                && !f.contains(F::NO_EXECUTE)
        }
        None => false,
    }
}
