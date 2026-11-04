extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::PageTableFlags;
use crate::mm::vmm::{AddressSpace, MapOutcome, PTE_ADDR_MASK, PTE_DIRTY, PTE_PRESENT, PTE_SHARED};
use crate::mm::pmm;

const PAGE_SIZE:  u64 = 4096;
const MMAP_BASE:  u64 = 0x0000_0001_0000_0000;
const MMAP_LIMIT: u64 = 0x0000_7F00_0000_0000;
const BRK_BASE:   u64 = 0x0000_0060_0000_0000;

// Upper bound (exclusive) of the canonical user half. Any user-supplied
// address+length range must satisfy end <= USER_END or the syscall is rejected;
// otherwise userspace could manipulate kernel-half mappings. Defined in
// mm::vmm, next to the page-table guard that enforces it
use crate::mm::vmm::USER_END;

pub const PROT_READ:  u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC:  u32 = 4;

const MAP_FIXED: u32 = 0x10;

// Rejects any combination of prot bits not implemented, so a future flag
// honoured by accident does not slip through
const PROT_VALID_MASK: u32 = PROT_READ | PROT_WRITE | PROT_EXEC;

// Returns true if [start, end) lies entirely within the user half and
// the arithmetic did not wrap
#[inline]
fn user_range_ok(start: u64, end: u64) -> bool {
    start < end && end <= USER_END
}

// Compute end = start + size with overflow check, then enforce user-half
#[inline]
fn user_end_for(start: u64, size: u64) -> Option<u64> {
    let end = start.checked_add(size)?;
    if !user_range_ok(start, end) { return None; }
    Some(end)
}

// Returns Some(aligned_size) iff length rounded up to a page doesn't wrap
#[inline]
fn checked_align_up(length: u64) -> Option<u64> {
    length.checked_add(PAGE_SIZE - 1).map(|v| v & !(PAGE_SIZE - 1))
}

// VMA //

/// File backing for a memory mapping. Pages of a file-backed VMA are filled
/// lazily from 'vnode_id' at the page-fault handler, and dirtied MAP_SHARED
/// pages are written back on unmap/msync. The vnode id is stored rather than
/// the fd, so the mapping outlives the fd being closed
#[derive(Copy, Clone, Debug)]
pub struct FileBacking {
    pub vnode_id: u32,
    /// File byte offset corresponding to 'Vma::start'
    pub file_off: u64,
    /// File size at map time, so reads past EOF zero-fill the tail page
    pub file_size: u64,
    /// MAP_SHARED (write back) vs MAP_PRIVATE (copy-on-write, no writeback)
    pub shared: bool,
}

#[derive(Copy, Clone, Debug)]
pub struct Vma {
    pub start: u64,
    pub end:   u64, // exclusive
    pub prot:  u32,
    /// None = anonymous mapping; Some = file-backed (lazy fill)
    pub backing: Option<FileBacking>,
}

// VmaMap //

pub struct VmaMap {
    /// Keyed by start address: iteration yields VMAs in address order
    vmas:        BTreeMap<u64, Vma>,
    pub brk:     u64,
    /// Lower bound for brk: shrinking below this would unmap program
    /// data segments. Pinned by the ELF loader to 'image.brk'
    pub brk_floor: u64,
}

impl VmaMap {
    pub fn new() -> Self {
        Self { vmas: BTreeMap::new(), brk: BRK_BASE, brk_floor: BRK_BASE }
    }

    pub fn set_brk_base(&mut self, addr: u64) {
        let aligned = page_align_up(addr);
        self.brk = aligned;
        self.brk_floor = aligned;
    }

    // insertion / removal //

    /// A VMA in. The only way one goes in: a file's counts its mapping
    /// (`MAPPINGS`)
    fn put(&mut self, vma: Vma) {
        count(&vma, true);
        if let Some(old) = self.vmas.insert(vma.start, vma) {
            count(&old, false);
        }
    }

    /// The VMA at `start` out. The only way one goes out
    fn take(&mut self, start: u64) -> Option<Vma> {
        let v = self.vmas.remove(&start)?;
        count(&v, false);
        Some(v)
    }

    fn insert(&mut self, vma: Vma) {
        self.put(vma);
    }

    /// The VMAs overlapping [s, e), in address order: the one that begins below
    /// `s` and reaches past it, if any, and those that begin inside. They never
    /// overlap each other.
    ///
    /// A filtered `range(..e)` would read every VMA below `e` on every `munmap`
    /// and `mprotect`; with two thousand thread stacks that is 45 us per unmap
    fn overlapping(&self, s: u64, e: u64) -> impl Iterator<Item = &Vma> {
        let before = self.vmas.range(..s).next_back().map(|(_, v)| v).filter(|v| v.end > s);
        before.into_iter().chain(self.vmas.range(s..e).map(|(_, v)| v))
    }

    /// Removes all VMAs overlapping [s, e). VMAs only partly covered are split
    /// or trimmed so portions outside [s, e) are preserved
    fn remove_range(&mut self, s: u64, e: u64) {
        let keys: Vec<u64> = self.overlapping(s, e).map(|v| v.start).collect();

        for k in keys {
            let v = match self.take(k) {
                Some(v) => v,
                None    => continue,
            };
            if v.start < s {
                // Left fragment keeps the original backing/offset
                self.put(Vma { start: v.start, end: s, prot: v.prot, backing: v.backing });
            }
            if v.end > e {
                // Right fragment's file offset advances by the bytes skipped
                let backing = v.backing.map(|mut b| {
                    b.file_off += e - v.start;
                    b
                });
                self.put(Vma { start: e, end: v.end, prot: v.prot, backing });
            }
        }
    }

    /// Changes the protection of the VMAs inside [s, e), and only of them.
    ///
    /// A hole stays a hole, with no error: not everything the kernel maps is in
    /// this table (an ELF image's segments are page tables the loader wrote),
    /// and `ld-miku`'s RELRO `mprotect` lands on those. The caller changes the
    /// pages themselves either way.
    ///
    /// The pieces keep their backing, a right-hand piece with its file offset
    /// moved on by the bytes before it, as `remove_range` splits
    fn protect(&mut self, s: u64, e: u64, prot: u32) {
        let covering: Vec<Vma> = self.overlapping(s, e).copied().collect();
        for v in covering {
            self.take(v.start);
            let at = |addr: u64| v.backing.map(|mut b| { b.file_off += addr - v.start; b });
            if v.start < s {
                self.put(Vma { start: v.start, end: s, prot: v.prot, backing: v.backing });
            }
            let (ms, me) = (v.start.max(s), v.end.min(e));
            let middle = Vma { start: ms, end: me, prot, backing: at(ms) };
            if middle.backing.is_none() {
                self.insert_merged(middle);
            } else {
                self.put(middle);
            }
            if v.end > e {
                self.put(Vma { start: e, end: v.end, prot: v.prot, backing: at(e) });
            }
        }
    }

    // address-space allocation //
    fn find_free(&self, size: u64) -> Option<u64> {
        if size == 0 { return None; }
        // Advance cursor past any VMA whose tail overlaps MMAP_BASE
        let mut cursor = MMAP_BASE;
        if let Some((_, v)) = self.vmas.range(..=MMAP_BASE).next_back() {
            if v.end > MMAP_BASE {
                cursor = v.end;
            }
        }

        for (_, v) in self.vmas.range(cursor..) {
            if v.start >= MMAP_LIMIT { break; }
            // checked_add: cursor approaches MMAP_LIMIT and size may be
            // user-supplied and large. Overflow once means no later (higher)
            // cursor can fit either
            match cursor.checked_add(size) {
                Some(end) if end <= v.start => return Some(cursor),
                None => return None,
                _ => {}
            }
            if v.end > cursor {
                cursor = v.end;
            }
        }

        match cursor.checked_add(size) {
            Some(end) if end <= MMAP_LIMIT => Some(cursor),
            _ => None,
        }
    }

    // merging //
    fn insert_merged(&mut self, mut vma: Vma) {
        // Only anonymous mappings merge; file-backed VMAs carry per-region
        // offsets that would be lost by coalescing, so they stay separate
        if vma.backing.is_none() {
            // Absorb preceding neighbour
            if let Some(prev) = self.vmas.range(..vma.start).next_back().map(|(_, v)| *v) {
                if prev.end == vma.start && prev.prot == vma.prot && prev.backing.is_none() {
                    self.take(prev.start);
                    vma.start = prev.start;
                }
            }
            // Absorb following neighbour
            if let Some(next) = self.vmas.get(&vma.end).copied() {
                if next.prot == vma.prot && next.backing.is_none() {
                    self.take(next.start);
                    vma.end = next.end;
                }
            }
        }
        self.put(vma);
    }

    fn find_and_insert(&mut self, size: u64, prot: u32) -> Option<u64> {
        let base = self.find_free(size)?;
        let end  = base.checked_add(size)?;
        self.insert_merged(Vma { start: base, end, prot, backing: None });
        Some(base)
    }

    /// Reserve a free region and register it as a file-backed mapping,
    /// without touching page tables (pages fault in lazily). Returns the base
    fn find_and_insert_file(&mut self, size: u64, prot: u32, backing: FileBacking) -> Option<u64> {
        let base = self.find_free(size)?;
        let end  = base.checked_add(size)?;
        self.insert_merged(Vma { start: base, end, prot, backing: Some(backing) });
        Some(base)
    }

    pub fn find(&self, addr: u64) -> Option<&Vma> {
        self.vmas
            .range(..=addr)
            .next_back()
            .map(|(_, v)| v)
            .filter(|v| v.end > addr)
    }
}

// global table //

/// Mappings of each vnode's file, every address space's VMAs counted.
///
/// A file mapping keeps the vnode's number so it outlives the descriptor it was
/// made from, and the VFS has to know: a mapped vnode is not evicted, and an
/// unlinked file is not freed while mapped (`vfs::core::reclaim`). Every VMA
/// goes in and out through `VmaMap::put` and `take`, which keep this
static MAPPINGS: Mutex<BTreeMap<u32, u32>> = Mutex::new(BTreeMap::new());

/// One VMA more or fewer. The last mapping of a file going tells the VFS, which
/// frees an unlinked file then, not here under the VMA lock
fn count(v: &Vma, more: bool) {
    let Some(b) = v.backing else { return };
    let mut m = MAPPINGS.lock();
    if more {
        *m.entry(b.vnode_id).or_insert(0) += 1;
        return;
    }
    let gone = match m.get_mut(&b.vnode_id) {
        Some(n) => {
            *n -= 1;
            *n == 0
        }
        None => false,
    };
    if gone {
        m.remove(&b.vnode_id);
        drop(m);
        crate::fs::vfs::core::mapping_released(b.vnode_id);
    }
}

/// Whether any address space maps `vnode`'s file
pub fn is_mapped(vnode: u32) -> bool {
    MAPPINGS.lock().contains_key(&vnode)
}

static VMA_MAP: Mutex<BTreeMap<u64, VmaMap>> = Mutex::new(BTreeMap::new());

#[inline]
fn with_vma<F: FnOnce(&mut VmaMap) -> R, R>(cr3: u64, f: F) -> R {
    let mut map = VMA_MAP.lock();
    f(map.entry(cr3).or_insert_with(VmaMap::new))
}

// public helpers //

pub fn vma_set_brk(cr3: u64, brk_base: u64) {
    with_vma(cr3, |m| m.set_brk_base(brk_base));
}

pub fn kernel_find_free(cr3: u64, size: u64) -> Option<u64> {
    with_vma(cr3, |m| m.find_free(size))
}

/// Snapshot the VMA covering 'addr', if any (page-fault handler use)
pub fn vma_lookup(cr3: u64, addr: u64) -> Option<Vma> {
    with_vma(cr3, |m| m.find(addr).copied())
}

/// Every registered region of an address space, in address order.
///
/// Copied out rather than visited under the lock: the caller is a crash report,
/// which prints, and printing under the VMA lock can deadlock a fault
pub fn vma_snapshot(cr3: u64) -> alloc::vec::Vec<Vma> {
    with_vma(cr3, |m| m.vmas.values().copied().collect::<alloc::vec::Vec<Vma>>())
}

/// Resolve a not-present fault against the VMA table. Anonymous VMAs get a
/// fresh zero page (demand-zero, lock-free besides pmm/page tables, so this
/// is safe from any CPL - the kernel touching a lazy user stack page during
/// signal delivery or a syscall resolves here too). File-backed VMAs take
/// the VFS lock to read, so they are only serviced for user-mode faults;
/// kernel code reaches them via fault_in_user_range before taking VFS locks
pub fn handle_vma_fault(cr3: u64, fault_addr: u64, from_user: bool) -> bool {
    let page_addr = fault_addr & !(PAGE_SIZE - 1);
    if page_addr >= USER_END { return false; }
    // A swapped-out page is "not present" too, but has content on disk; filling
    // it demand-zero would lose data and leak the slot. Swap-in belongs to the
    // swap path
    if let Some(pte) = crate::mm::vmm::read_pte_raw(cr3, page_addr) {
        if crate::mm::swap_map::is_swap_pte(pte) { return false; }
    }
    let Some(vma) = vma_lookup(cr3, page_addr) else { return false };
    match vma.backing {
        Some(_) => from_user && handle_file_fault(cr3, fault_addr),
        None    => handle_anon_fault(cr3, page_addr, &vma),
    }
}

/// Demand-zero fill for one page of an anonymous VMA (lazy stack, lazy mmap)
fn handle_anon_fault(cr3: u64, page_addr: u64, vma: &Vma) -> bool {
    // PROT_NONE is a guard: nothing is filled, and the access is the fault it
    // was placed to catch.
    //
    // A page already present when made PROT_NONE stays present with its data,
    // as a supervisor page ring 3 cannot reach (see `prot_to_flags`)
    if vma.prot & PROT_VALID_MASK == 0 {
        return false;
    }
    let aspace = AddressSpace::from_raw(cr3);
    // Another thread may have filled it since this one faulted
    if aspace.read_pte_raw(page_addr).map_or(false, |p| p & PTE_PRESENT != 0) {
        let _ = aspace.into_raw();
        return true;
    }
    let Some(phys) = pmm::alloc_frame() else {
        let _ = aspace.into_raw();
        return false;
    };
    let hhdm = crate::arch::x86_64::grub::hhdm();
    unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE as usize); }
    // Or does so while this one zeroes: the check above is only the cheap way
    // out, and the mapping decides (`map_page_if_absent`)
    let flags = prot_to_flags(vma.prot);
    let outcome = aspace.map_page_if_absent(page_addr, phys, flags);
    if matches!(outcome, MapOutcome::Mapped) {
        fault_around(&aspace, page_addr, vma, flags);
    }
    let _ = aspace.into_raw();
    finish_fill(outcome, phys, page_addr)
}

/// Pages an anonymous fault fills past its own when the page before it is
/// there already: 64 KiB at a time for memory written front to back, Linux's
/// multi-size THP for anonymous memory in its smallest useful size
/// (`dev/kernel-core.md`, "Anonymous page faults")
const FAULT_AROUND: u64 = 15;

/// The pages after `page_addr`, up to `FAULT_AROUND` of them inside `vma`,
/// zeroed and mapped now, when the page before it is mapped (a run written in
/// order). A fault costs about as much again as zeroing the page it fills
/// (QEMU: 350 ns and 330), and memory touched in order pays it once in sixteen
/// pages. Stops at the first page already there, and gives up when frames are
/// short
fn fault_around(aspace: &AddressSpace, page_addr: u64, vma: &Vma, flags: PageTableFlags) {
    if page_addr < vma.start + PAGE_SIZE || pmm::running_low() {
        return;
    }
    let present = |va: u64| aspace.read_pte_raw(va).map_or(false, |p| p & PTE_PRESENT != 0);
    if !present(page_addr - PAGE_SIZE) {
        return;
    }
    let hhdm = crate::arch::x86_64::grub::hhdm();
    let end = page_addr.saturating_add((FAULT_AROUND + 1) * PAGE_SIZE).min(vma.end);
    let mut va = page_addr + PAGE_SIZE;
    while va < end {
        // Present, or swapped out: not this fault's
        if aspace.read_pte_raw(va).map_or(false, |p| p != 0) {
            return;
        }
        let Some(phys) = pmm::alloc_frame() else { return };
        unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE as usize); }
        // A not-present entry is never in a TLB, so a page mapped here
        // needs no flush; one another thread mapped first keeps its own
        if !matches!(aspace.map_page_if_absent(va, phys, flags), MapOutcome::Mapped) {
            pmm::free_frame_cow(phys);
            return;
        }
        va += PAGE_SIZE;
    }
}

/// A demand fill's frame after the race for its entry: flushed in when it
/// was mapped, returned when another fill got there first (whose page is
/// the one every thread now sees, so the fault is resolved either way)
fn finish_fill(outcome: MapOutcome, phys: u64, page_addr: u64) -> bool {
    match outcome {
        MapOutcome::Mapped => {
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(page_addr));
            true
        }
        // By reference, for the shared fill: its frame is the file's, with a
        // reference taken for this mapping, which goes back here
        MapOutcome::Present => {
            pmm::free_frame_cow(phys);
            true
        }
        MapOutcome::Failed => {
            pmm::free_frame_cow(phys);
            false
        }
    }
}

/// Populate every lazy (VMA-covered, not-present) page in [ptr, ptr+len).
/// Syscall pointer validation calls this before checking PTEs, so a buffer
/// living on a not-yet-touched lazy page does not spuriously EFAULT. Must
/// be called before the caller takes the VFS lock (file-backed fill reads
/// through the VFS)
pub fn fault_in_user_range(cr3: u64, ptr: u64, len: u64) {
    if len == 0 { return; }
    let Some(end) = ptr.checked_add(len) else { return };
    let mut va = ptr & !(PAGE_SIZE - 1);
    while va < end && va < USER_END {
        match crate::mm::vmm::read_pte_raw(cr3, va) {
            Some(pte) if pte & PTE_PRESENT != 0 => {}
            Some(pte) if crate::mm::swap_map::is_swap_pte(pte) => {
                // Swapped-out page: bring the real content back rather than
                // failing the caller's PRESENT check
                let slot = crate::mm::swap_map::slot_from_pte(pte);
                let _ = crate::mm::swap_map::try_swapin(cr3, va, slot, pte);
            }
            _ => {
                let _ = handle_vma_fault(cr3, va, true);
            }
        }
        va += PAGE_SIZE;
    }
}

/// Fill a faulting page of a file-backed mapping. Returns true if 'addr'
/// fell in a file-backed VMA and the page was mapped (the fault is then
/// resolved); false if it is not our mapping, so the handler continues to
/// its normal error path. Reached only for user-mode, not-present faults
pub fn handle_file_fault(cr3: u64, fault_addr: u64) -> bool {
    let page_addr = fault_addr & !(PAGE_SIZE - 1);
    let Some(vma) = vma_lookup(cr3, page_addr) else { return false };
    let Some(backing) = vma.backing else { return false };

    // File offset of this page
    let file_off = backing.file_off + (page_addr - vma.start);

    // Shared, of a tmpfs file: the file's own page, not a copy, so every
    // mapping, `read` and `write` are one page (`vfs::pages`)
    if backing.shared {
        if let Some(phys) = crate::fs::vfs::core::shared_frame(backing.vnode_id as usize, file_off) {
            let aspace = AddressSpace::from_raw(cr3);
            let outcome = aspace.map_shared_if_absent(page_addr, phys, prot_to_flags(vma.prot));
            let _ = aspace.into_raw();
            return finish_fill(outcome, phys, page_addr);
        }
    }

    let Some(phys) = pmm::alloc_frame() else { return false };
    let hhdm = crate::arch::x86_64::grub::hhdm();
    let dst = (phys + hhdm) as *mut u8;

    // Zero the frame, then fill the part that lies within the file at map
    // time (the tail page past EOF stays zero, like Linux)
    unsafe { core::ptr::write_bytes(dst, 0, PAGE_SIZE as usize); }
    if file_off < backing.file_size {
        let want = (backing.file_size - file_off).min(PAGE_SIZE) as usize;
        let buf = unsafe { core::slice::from_raw_parts_mut(dst, want) };
        // read_at_vnode briefly takes the VFS lock; safe here because file
        // faults only come from user mode, never from kernel code already
        // holding it
        if crate::fs::vfs::core::read_at_vnode(backing.vnode_id as usize, file_off, buf).is_err() {
            pmm::free_frame(phys);
            return false;
        }
    }

    // MAP_SHARED keeps the prot (writes go to the page and later flush back);
    // MAP_PRIVATE also maps writable, but its dirtied pages are never written
    // back: they are this process's private copy
    let flags = prot_to_flags(vma.prot);
    let aspace = AddressSpace::from_raw(cr3);
    let outcome = aspace.map_page_if_absent(page_addr, phys, flags);
    let _ = aspace.into_raw();
    finish_fill(outcome, phys, page_addr)
}

pub fn kernel_register_vma(cr3: u64, start: u64, end: u64, prot: u32) {
    // Defensive: kernel callers should already have validated, but the
    // VmaMap relies on start < end for correctness of range queries
    if start >= end { return; }
    with_vma(cr3, |m| m.insert(Vma { start, end, prot, backing: None }));
}

pub fn vma_cleanup(cr3: u64) {
    let gone = VMA_MAP.lock().remove(&cr3);
    if let Some(m) = gone {
        for v in m.vmas.values() {
            count(v, false);
        }
    }
}

/// The child's copy of the table, less `leave_out`: the ranges `fork` did not
/// give it the pages of (`PTE_LENT`), which it must not find mapped either: its
/// first touch there would fault in zeroes
pub fn vma_clone(src_cr3: u64, dst_cr3: u64, leave_out: &[(u64, u64)]) {
    let mut map = VMA_MAP.lock();
    if let Some(src) = map.get(&src_cr3) {
        let mut dst = VmaMap {
            vmas:      src.vmas.clone(),
            brk:       src.brk,
            brk_floor: src.brk_floor,
        };
        // The child's VMAs are mappings too
        for v in dst.vmas.values() {
            count(v, true);
        }
        for &(start, len) in leave_out {
            if let Some(end) = checked_align_up(len).and_then(|l| start.checked_add(l)) {
                dst.remove_range(start, end);
            }
        }
        map.insert(dst_cr3, dst);
    }
}

// page-table helpers //

#[inline]
fn page_align_up(addr: u64) -> u64 {
    (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Convert POSIX prot bits to x86-64 PTE flags. Enforces W^X: if the
/// user asks for WRITE+EXEC together we drop EXEC and force NO_EXECUTE.
/// RWX mappings are the most directly useful exploit primitive in a
/// ring-3 attack model, so the kernel never hands them out.
/// PRESENT is mandatory: without it every other flag is ignored by the CPU
fn prot_to_flags(prot: u32) -> PageTableFlags {
    let prot = prot & PROT_VALID_MASK;
    // PROT_NONE: present, so the frame and its data stay where they are, and
    // not USER_ACCESSIBLE, so ring 3 cannot touch it: a supervisor page in the
    // user half
    if prot == 0 {
        return PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE;
    }
    let want_exec = (prot & PROT_EXEC) != 0 && (prot & PROT_WRITE) == 0;
    let mut f = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if prot & PROT_WRITE != 0 { f |= PageTableFlags::WRITABLE; }
    if !want_exec { f |= PageTableFlags::NO_EXECUTE; }
    f
}

#[inline]
fn prot_rejects_wx(prot: u32) -> bool {
    (prot & PROT_WRITE) != 0 && (prot & PROT_EXEC) != 0
}

// physical-page helpers //

/// Allocate and zero-fill "pages" frames, mapping them at [base,
/// base+pages*PAGE_SIZE)
fn map_fresh_pages(cr3: u64, base: u64, pages: usize, flags: PageTableFlags) -> Result<(), ()> {
    let hhdm   = crate::arch::x86_64::grub::hhdm();
    let aspace = AddressSpace::from_raw(cr3);
    let mut mapped = 0usize;

    for i in 0..pages {
        match pmm::alloc_frame() {
            Some(phys) => {
                unsafe {
                    core::ptr::write_bytes(
                        (phys + hhdm) as *mut u8, 0, PAGE_SIZE as usize,
                    );
                }
                if aspace.map_page(base + i as u64 * PAGE_SIZE, phys, flags) {
                    mapped += 1;
                } else {
                    pmm::free_frame(phys);
                    for j in 0..mapped {
                        aspace.unmap_page(base + j as u64 * PAGE_SIZE);
                    }
                    let _ = aspace.into_raw();
                    return Err(());
                }
            }
            None => {
                for j in 0..mapped {
                    aspace.unmap_page(base + j as u64 * PAGE_SIZE);
                }
                let _ = aspace.into_raw();
                return Err(());
            }
        }
    }
    let _ = aspace.into_raw();
    Ok(())
}

/// Maps an existing physical range into a process's address space
///
/// Unlike `map_fresh_pages` the frames belong to whoever allocated them, and
/// this hands the process a window onto them. A Mayland shared-memory pool is
/// read by the copy engine at its physical address while the client writes it
/// through this mapping
///
/// Every frame gets a reference taken, because process teardown frees each
/// present page it walks. With the reference, `free_frame_cow_swap` finds a
/// count of two, drops it to one and leaves the frame alone; the owner frees it
/// when the pool goes.
///
/// The entries are `PTE_LENT`: an ordinary page would be copy-on-write after
/// `fork` and swappable, so a client that spawned anything would draw into
/// private copies the compositor never reads, and the swapper could evict a
/// frame the pool owns
pub fn map_phys_into_user(cr3: u64, phys: u64, size: u64, prot: u32) -> Option<u64> {
    map_lent(cr3, phys, size, prot, false)
}

/// `map_phys_into_user` for memory that is not RAM: a device aperture, such as
/// a program's video memory through BAR1 (`nvidia/user_vram.rs`).
/// Write-combining (PAT index 4, which `vmm::init_pat` made WC), because the
/// processor promises nothing for write-back over a device's memory.
///
/// The same lent entries and references as for RAM: a reference on each page
/// keeps teardown from handing it to the frame allocator, and here the address
/// is not a frame at all. The caller takes the window down before the address
/// space goes (`shm::unmap_windows_in`)
pub fn map_mmio_into_user_wc(cr3: u64, phys: u64, size: u64, prot: u32) -> Option<u64> {
    map_lent(cr3, phys, size, prot, true)
}

fn map_lent(cr3: u64, phys: u64, size: u64, prot: u32, write_combining: bool) -> Option<u64> {
    if size == 0 || phys & 0xFFF != 0 {
        return None;
    }
    let size = checked_align_up(size)?;
    let pages = (size / PAGE_SIZE) as usize;

    let base = with_vma(cr3, |m| m.find_and_insert(size, prot))?;
    let mut flags = prot_to_flags(prot);
    if write_combining {
        // Bit 7 of a 4 KiB leaf is PAT, which the crate calls HUGE_PAGE; with
        // PCD and PWT clear it selects index 4 - see `vmm::map_mmio_typed`
        flags |= PageTableFlags::HUGE_PAGE;
    }
    let aspace = AddressSpace::from_raw(cr3);

    for i in 0..pages {
        let p = phys + i as u64 * PAGE_SIZE;
        // The reference before the entry, as the vDSO does: a teardown in
        // between would otherwise find a count of one and free the frame
        pmm::ref_inc(p);
        if aspace.map_lent_if_absent(base + i as u64 * PAGE_SIZE, p, flags) != MapOutcome::Mapped {
            pmm::ref_dec(p);
            // Without freeing, like `unmap_phys_from_user` below
            for j in 0..i {
                aspace.unmap_page_no_free(base + j as u64 * PAGE_SIZE);
                pmm::ref_dec(phys + j as u64 * PAGE_SIZE);
            }
            let _ = aspace.into_raw();
            with_vma(cr3, |m| m.remove_range(base, base + size));
            return None;
        }
    }
    let _ = aspace.into_raw();
    Some(base)
}

/// Undo `map_phys_into_user`: drop the window and the references, leaving the
/// frames themselves to their owner
pub fn unmap_phys_from_user(cr3: u64, base: u64, phys: u64, size: u64) {
    let Some(size) = checked_align_up(size) else { return };
    let pages = (size / PAGE_SIZE) as usize;
    // `unmap_page_no_free`, not `unmap_pages`
    //
    // The frames are not this mapping's to free: they belong to whoever handed
    // them over (a Mayland shared-memory pool owns one contiguous run and frees
    // it when destroyed), and `unmap_page` frees whatever it unmaps. That would
    // give the frames to the allocator while the pool owns them, and the pool's
    // own free would give them back a second time. `ref_dec` below drops the
    // reference this mapping took
    let aspace = AddressSpace::from_raw(cr3);
    let mut cleared = false;
    for i in 0..pages {
        cleared |= aspace.unmap_page_no_free(base + i as u64 * PAGE_SIZE);
    }
    let _ = aspace.into_raw();
    // Out of every processor's TLB as well before the owner may free them, as
    // `unmap_pages` says: a sibling thread's translation outlives the entry
    // (`mm::tlb`)
    if cleared && crate::mm::tlb::cr3_loaded_elsewhere(cr3) {
        crate::mm::tlb::shootdown();
    }
    for i in 0..pages {
        pmm::ref_dec(phys + i as u64 * PAGE_SIZE);
    }
    with_vma(cr3, |m| m.remove_range(base, base + size));
}

/// Clears the entries, then, when a sibling thread may hold translations for
/// them on another processor, waits until none does, and only then frees the
/// frames (`mm::tlb`). A sibling's stale translation must not write into
/// whatever the frame becomes next
fn unmap_pages(cr3: u64, base: u64, pages: usize) {
    let aspace = AddressSpace::from_raw(cr3);
    let mut frames = Vec::new();
    aspace.take_range(base, pages, &mut frames);
    let _ = aspace.into_raw();
    if frames.is_empty() {
        return;
    }
    if crate::mm::tlb::cr3_loaded_elsewhere(cr3) {
        crate::mm::tlb::shootdown();
    }
    // By reference: a copy-on-write page after `fork` and a tmpfs page mapped
    // shared are frames somebody else holds as well, and the frame goes back
    // only with the last of them
    for phys in frames {
        pmm::free_frame_cow(phys);
    }
}

// syscall implementations //

pub fn sys_mmap(
    cr3:    u64,
    addr:   u64,
    length: u64,
    prot:   u32,
    flags:  u32,
    _fd:    i64,
    _off:   u64,
) -> i64 {
    if length == 0 { return -22; }
    // Refuse explicit W+X at the syscall boundary, not silently downgrade.
    // prot_to_flags would have dropped EXEC anyway, but failing loudly is
    // better than a userspace program proceeding as if it had RWX
    if prot_rejects_wx(prot) || prot & !PROT_VALID_MASK != 0 { return -22; }

    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    let pages = (size / PAGE_SIZE) as usize;
    let fixed = flags & MAP_FIXED != 0;

    let base = if fixed {
        if addr == 0 || addr & 0xFFF != 0 { return -22; }
        if user_end_for(addr, size).is_none() { return -22; }
        unmap_pages(cr3, addr, pages);
        with_vma(cr3, |m| m.remove_range(addr, addr + size));
        addr
    } else {
        // find_and_insert: probe + register under one lock
        match with_vma(cr3, |m| m.find_and_insert(size, prot)) {
            Some(a) => a,
            None    => return -12,
        }
    };

    // Nothing is allocated here. A page is zeroed and mapped on its first touch
    // (`handle_anon_fault`), from ring 3 or from a syscall validating a pointer
    // into it (`fault_in_user_range`), as the main stack is. A thread's 2 MiB
    // stack then costs only the pages it uses, and `mmap(PROT_NONE)`, which
    // wasmtime uses to reserve gigabytes for guards, costs nothing. A PROT_NONE
    // page stays unfilled until `mprotect` opens it.
    //
    // Nothing depends on the pages being there: futex keys on the address, shm
    // pools and GPU memory are the kernel's own frames (`map_phys_into_user`),
    // fork clones the VMA table with the page tables, and munmap/mprotect walk
    // only what is present
    if fixed {
        let end = base.saturating_add(size);
        with_vma(cr3, |m| m.insert_merged(Vma { start: base, end, prot, backing: None }));
    }

    crate::trace_println!("[mmap] {:#x}+{:#x} prot={:#x}", base, size, prot);
    base as i64
}


/// File-backed mmap. Maps 'length' bytes of the file behind 'vnode_id'
/// starting at 'file_off', lazily: no pages are mapped now, they fault in
/// from the file on first access. 'shared' selects MAP_SHARED (dirtied
/// pages flush back to the file on unmap/msync) vs MAP_PRIVATE
pub fn sys_mmap_file(
    cr3: u64,
    addr: u64,
    length: u64,
    prot: u32,
    flags: u32,
    vnode_id: u32,
    file_off: u64,
    file_size: u64,
    shared: bool,
) -> i64 {
    if length == 0 { return -22; }
    if prot_rejects_wx(prot) || prot & !PROT_VALID_MASK != 0 { return -22; }
    if file_off & 0xFFF != 0 { return -22; } // file offset must be page aligned

    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    let fixed = flags & MAP_FIXED != 0;
    let backing = FileBacking { vnode_id, file_off, file_size, shared };

    let base = if fixed {
        if addr == 0 || addr & 0xFFF != 0 { return -22; }
        if user_end_for(addr, size).is_none() { return -22; }
        // Drop anything already there (and flush it if it was shared)
        writeback_and_unmap(cr3, addr, addr + size);
        with_vma(cr3, |m| {
            m.remove_range(addr, addr + size);
            m.insert_merged(Vma { start: addr, end: addr + size, prot, backing: Some(backing) });
        });
        addr
    } else {
        match with_vma(cr3, |m| m.find_and_insert_file(size, prot, backing)) {
            Some(a) => a,
            None    => return -12,
        }
    };

    crate::trace_println!(
        "[mmap] file vnode={} off={:#x} -> {:#x}+{:#x} prot={:#x} shared={}",
        vnode_id, file_off, base, size, prot, shared
    );
    base as i64
}

/// Write every dirty, present page of a MAP_SHARED file-backed mapping in
/// [start, end) back to its file. Used by munmap and msync
fn writeback_file_range(cr3: u64, start: u64, end: u64) {
    // The shared file mappings in the range, looked up once rather than per
    // page under the table's lock
    let start = start & !(PAGE_SIZE - 1);
    let shared: Vec<Vma> = with_vma(cr3, |m| {
        m.overlapping(start, end)
            .filter(|v| v.backing.is_some_and(|b| b.shared))
            .copied()
            .collect()
    });
    if shared.is_empty() {
        return;
    }
    let aspace = AddressSpace::from_raw(cr3);
    let hhdm = crate::arch::x86_64::grub::hhdm();
    for vma in shared {
        let Some(b) = vma.backing else { continue };
        let mut p = start.max(vma.start);
        while p < end.min(vma.end) {
            if let Some(pte) = aspace.read_pte_raw(p) {
                // A shared tmpfs page is the file's page: there is nothing to
                // write back
                if pte & PTE_PRESENT != 0 && pte & PTE_DIRTY != 0 && pte & PTE_SHARED == 0 {
                    let phys = pte & PTE_ADDR_MASK;
                    let file_off = b.file_off + (p - vma.start);
                    // Don't extend the file past its mapped size
                    if file_off < b.file_size {
                        let n = (b.file_size - file_off).min(PAGE_SIZE) as usize;
                        let src = unsafe {
                            core::slice::from_raw_parts((phys + hhdm) as *const u8, n)
                        };
                        let _ = crate::fs::vfs::core::write_at_vnode(b.vnode_id as usize, file_off, src);
                    }
                }
            }
            p += PAGE_SIZE;
        }
    }
    let _ = aspace.into_raw();
}

/// Flush dirty shared pages, then unmap every page in [start, end)
fn writeback_and_unmap(cr3: u64, start: u64, end: u64) {
    writeback_file_range(cr3, start, end);
    let pages = ((end - start) / PAGE_SIZE) as usize;
    unmap_pages(cr3, start, pages);
}

/// msync(2): flush a file-backed mapping's dirty pages to disk now
pub fn sys_msync(cr3: u64, addr: u64, length: u64) -> i64 {
    if addr & 0xFFF != 0 { return -22; }
    if length == 0 { return 0; }
    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    let end = match user_end_for(addr, size) {
        Some(e) => e,
        None    => return -22,
    };
    writeback_file_range(cr3, addr, end);
    0
}

pub fn sys_munmap(cr3: u64, addr: u64, length: u64) -> i64 {
    if addr & 0xFFF != 0 { return -22; }
    if length == 0 { return 0; }
    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    // Reject kernel-half addresses; otherwise a malicious caller could
    // hand the kernel any VA and force unmap_page on it
    let end = match user_end_for(addr, size) {
        Some(e) => e,
        None    => return -22,
    };
    // Flush dirty shared file-backed pages before tearing the mapping down,
    // then unmap. Anonymous pages skip the writeback (no backing file)
    writeback_file_range(cr3, addr, end);
    let pages = (size / PAGE_SIZE) as usize;
    unmap_pages(cr3, addr, pages);
    with_vma(cr3, |m| m.remove_range(addr, end));
    0
}

pub fn sys_mprotect(cr3: u64, addr: u64, length: u64, prot: u32) -> i64 {
    if addr & 0xFFF != 0 { return -22; }
    if length == 0 { return 0; }
    if prot_rejects_wx(prot) || prot & !PROT_VALID_MASK != 0 { return -22; }
    let size = match checked_align_up(length) {
        Some(s) => s,
        None    => return -22,
    };
    let end = match user_end_for(addr, size) {
        Some(e) => e,
        None    => return -22,
    };
    let flags  = prot_to_flags(prot);
    let aspace = AddressSpace::from_raw(cr3);
    let mut p  = addr;
    let mut changed = false;
    while p < end {
        changed |= aspace.reprotect(p, flags);
        p += PAGE_SIZE;
    }
    let _ = aspace.into_raw();
    // A sibling on another processor keeps the old permission until its TLB
    // lets go of it: a page made read-only would stay writable there
    if changed && crate::mm::tlb::cr3_loaded_elsewhere(cr3) {
        crate::mm::tlb::shootdown();
    }
    // And the table, so a page not yet faulted in comes in with the new
    // protection (see `VmaMap::protect`)
    with_vma(cr3, |m| m.protect(addr, end, prot));
    0
}

pub fn sys_brk(cr3: u64, new_brk: u64) -> u64 {
    let (cur, floor) = with_vma(cr3, |m| (m.brk, m.brk_floor));
    if new_brk == 0 { return cur; }

    let new = match checked_align_up(new_brk) {
        Some(v) if v < USER_END => v,
        _ => return cur,
    };

    // Refuses to shrink below the floor pinned by exec; otherwise pages owned
    // by program data or heap arenas would be unmapped
    if new < floor { return cur; }

    if new <= cur {
        let pages = ((cur - new) / PAGE_SIZE) as usize;
        unmap_pages(cr3, new, pages);
        with_vma(cr3, |m| { m.remove_range(new, cur); m.brk = new; });
        return new;
    }

    let flags = PageTableFlags::PRESENT
              | PageTableFlags::WRITABLE
              | PageTableFlags::USER_ACCESSIBLE
              | PageTableFlags::NO_EXECUTE;
    let pages = ((new - cur) / PAGE_SIZE) as usize;

    if map_fresh_pages(cr3, cur, pages, flags).is_ok() {
        with_vma(cr3, |m| {
            m.insert_merged(Vma { start: cur, end: new, prot: PROT_READ | PROT_WRITE, backing: None });
            m.brk = new;
        });
        new
    } else {
        cur
    }
}
