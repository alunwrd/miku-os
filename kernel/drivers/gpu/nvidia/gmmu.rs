// The GPU's own page tables
//
// BAR1 is the aperture through which a CPU writes video memory with ordinary
// stores. When GSP-RM boots it switches BAR1 from physical mode, where a BAR1
// offset is a video-memory offset, to virtual mode, where it is an address into
// a GPU page table.
//
// RM hands over the physical address of the page directory it built for BAR1
// (`bar1PdeBase`, in the static config it sends at boot) and expects whoever
// wants a mapping to add it there. nouveau's `r535_bar_bar1_init` adopts RM's
// directory as the root of its own address space, and so does this file.
//
// The format is VER3, introduced on Hopper and used by Blackwell. Six levels
// for a 4 KiB page, from `vmmgh100.c`:
//
//     level 5  1 bit    8-byte PDE      root, the one RM gives us
//     level 4  9 bits   8-byte PDE
//     level 3  9 bits   8-byte PDE
//     level 2  9 bits   8-byte PDE
//     level 1  8 bits   16-byte dual PDE   one entry per 2 MiB
//     level 0  9 bits   8-byte PTE         one entry per 4 KiB
//
// The dual PDE at level 1 carries two independent halves, one for big pages and
// one for small.
//
// Our own tables live in system memory: the aperture field in a PDE allows it,
// and the CPU writes them without PRAMIN.

use super::mmio::MmioRegion;
use super::pramin;
use crate::mm::dma::DmaBuffer;

extern crate alloc;

/// Nothing calls this. `dev/roadmap-desktop.md` stage C records why mapping the
/// scanout surface into BAR1 was dropped. The BAR1 entry points stay as the
/// worked example of writing into a directory RM built; a copy-engine channel's
/// address space uses the same VER3 page tables.
///
/// Write into RM's page directory, or only look at it.
///
/// RM's own entries decode as this file writes them: `APERTURE_VIDEO_MEMORY`
/// with `PCF_VALID_CACHED_ATS_NOT_ALLOWED`, address at 51:12, five directories
/// deep. A VA past the end of BAR1 is never translated, which reads back 0/4,
/// the same as a bad mapping, so the aperture size is printed. `rollback` takes
/// back a mapping that fails its readback
#[allow(dead_code)] // switch for the BAR1 mapping path, parked; see `map_vram_into_bar1`
pub const DIAG_MAP_BAR1: bool = true;

// PTE, from OpenRM's `swref/published/hopper/gh100/dev_mmu.h`, VER3, which
// Blackwell uses unchanged (`kgmmuFmtInitLevels_GB10X`). The same 8 bytes are a
// 4 KiB PTE, a 64 KiB one in a big table, and a 2 MiB one written into a
// level-1 entry
const PTE_VALID: u64 = 1 << 0;
/// APERTURE is 2:1 and VIDEO_MEMORY is 0, so video memory adds nothing here
const PTE_APERTURE_VIDEO: u64 = 0 << 1;
/// PCF is 7:3. `REGULAR_RW_ATOMIC_CACHED_ACD` = 0x10, what nouveau uses for a
/// writable mapping of video memory (`gh100_vmm_valid`)
const PTE_PCF_RW_CACHED: u64 = 0x10 << 3;
/// SYSTEM_COHERENT_MEMORY. `gf100_vmm_aper` returns 2 for
/// `NVKM_MEM_TARGET_HOST`
const PTE_APERTURE_SYSCOH: u64 = 2 << 1;
/// `REGULAR_RW_ATOMIC_UNCACHED_ACD` = 0x11, what `gh100_vmm_valid` picks when
/// the target is host memory (`vol`). An L2 line holding a stale copy of a ring
/// the CPU is writing is a channel that runs old commands
const PTE_PCF_RW_UNCACHED: u64 = 0x11 << 3;
/// `PRIVILEGE_RW_ATOMIC_CACHED_ACD` = 0x12 and `..._UNCACHED_ACD` = 0x13: the
/// same two PCFs with the privilege bit set.
///
/// The context-switch engine walks a graphics context, and
/// `r535_gr_promote_ctx` maps every one of its buffers with `.priv = 1`. The
/// engine may not touch an ordinary mapping of that memory, and the failure is
/// a fault during a context switch
const PTE_PCF_PRIV_RW_CACHED: u64 = 0x12 << 3;
const PTE_PCF_PRIV_RW_UNCACHED: u64 = 0x13 << 3;
/// KIND is 11:8; PITCH is 0. A pitch surface is what the display scans, and the
/// kind has to agree with the KIND bit in the window's ISO address
const PTE_KIND_PITCH: u64 = 0 << 8;

// PDE
const PDE_APERTURE_VIDEO: u64 = 1 << 1;
const PDE_APERTURE_SYSCOH: u64 = 2 << 1;
/// PCF is 5:3. A PDE has no separate valid bit (bit 0 means "this entry is a
/// PTE, not a PDE"), so validity is carried by the PCF encoding
const PDE_PCF_VALID_CACHED: u64 = 2 << 3;
const PDE_PCF_VALID_UNCACHED_ATS: u64 = 1 << 3;

const PAGE_SHIFT: u32 = 12;
const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;
/// One level-1 entry covers this much: OpenRM's PD0, bits 28:21. The walk below
/// goes one entry at a time. Also the size of a huge page, a PTE written into
/// that entry
const PD0_SPAN: u64 = 2 << 20;
/// A big page, and what one entry of a big table covers: bits 20:16, 32 entries
const BIG_SPAN: u64 = 64 << 10;

/// A GPU page size (`dev/gpu-memory-compute.md`, 3.8)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageSize {
    /// 4 KiB, in a small table: the high half of a level-1 entry
    Small,
    /// 64 KiB, in a big table: the low half
    Big,
    /// 2 MiB, a PTE in the level-1 entry itself
    Huge,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            PageSize::Small => PAGE_SIZE,
            PageSize::Big => BIG_SPAN,
            PageSize::Huge => PD0_SPAN,
        }
    }
}

/// How many pages of each size a mapping took, for `nvidia va all`
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Pages {
    pub huge: u32,
    pub big: u32,
    pub small: u32,
}

/// The page for the piece of a mapping at `va` -> `phys` with `left` bytes to
/// go: the largest the virtual address, the physical address and the length all
/// allow.
///
/// OpenRM decides the size per allocation (`memmgrDeterminePageSize`) and
/// aligns the allocation to it; `vram::alloc` does the same, so a block mapped
/// whole from its start gets its own page. Asking the alignment serves memory
/// somebody else allocated too (GSP-RM's context buffers), never making a page
/// larger than what it covers.
///
/// System memory is 4 KiB, as in OpenRM unless a registry key says otherwise
/// (`sysmemPageSize`). `small_only` is `gpu4k`
pub fn page_for(va: u64, phys: u64, left: u64, aper: Aperture, small_only: bool) -> PageSize {
    if aper == Aperture::Sysmem || small_only {
        return PageSize::Small;
    }
    for p in [PageSize::Huge, PageSize::Big] {
        let b = p.bytes();
        if va % b == 0 && phys % b == 0 && left >= b {
            return p;
        }
    }
    PageSize::Small
}

/// The pages a mapping would take, without writing any: `page_for` piece by
/// piece. For `nvidia selftest`, which has no card to map on
pub fn page_plan(va: u64, phys: u64, bytes: u64, aper: Aperture) -> Pages {
    let mut pages = Pages::default();
    let mut done = 0u64;
    let bytes = bytes.next_multiple_of(PAGE_SIZE);
    while done < bytes {
        let p = page_for(va + done, phys + done, bytes - done, aper, false);
        match p {
            PageSize::Huge => pages.huge += 1,
            PageSize::Big => pages.big += 1,
            PageSize::Small => pages.small += 1,
        }
        done += p.bytes();
    }
    pages
}

/// `gpu4k` on the GRUB line: every mapping in 4 KiB pages. `nogpuvm` implies
/// it: that hatch leaves old translations in place, and a larger page over a
/// stale smaller one is refused
fn small_pages_only() -> bool {
    crate::arch::x86_64::grub::cmdline_flag("gpu4k") || keep_tables_forever()
}

/// Where a page table lives, which decides both how the CPU reaches it and what
/// aperture the PDE pointing at it has to declare
#[derive(Clone, Copy)]
enum Table {
    Vram(u64),
    Sys(u64),
}

impl Table {
    fn read64(&self, bar0: &MmioRegion, off: u64) -> u64 {
        match *self {
            Table::Vram(base) => {
                let mut w = pramin::Window::new(bar0);
                let lo = w.read32(base + off) as u64;
                let hi = w.read32(base + off + 4) as u64;
                lo | (hi << 32)
            }
            Table::Sys(base) => unsafe {
                core::ptr::read_volatile(virt(base + off) as *const u64)
            },
        }
    }

    fn write64(&self, bar0: &MmioRegion, off: u64, val: u64) {
        match *self {
            Table::Vram(base) => {
                // Every video-memory table is RM's, since this file builds its
                // own in system memory, so each write here is into RM's
                // structures and is recorded for taking back
                RM_WRITES.lock().push((base, off));
                let mut w = pramin::Window::new(bar0);
                w.write32(base + off, val as u32);
                w.write32(base + off + 4, (val >> 32) as u32);
            }
            Table::Sys(base) => unsafe {
                core::ptr::write_volatile(virt(base + off) as *mut u64, val);
            },
        }
    }

    /// The PDE that points at this table
    fn pde(&self) -> u64 {
        match *self {
            Table::Vram(a) => a | PDE_APERTURE_VIDEO | PDE_PCF_VALID_CACHED,
            Table::Sys(a) => a | PDE_APERTURE_SYSCOH | PDE_PCF_VALID_UNCACHED_ATS,
        }
    }

    /// The table's own physical address, without the PDE bits. Only the
    /// dump path asks for it
    #[allow(dead_code)]
    fn addr(&self) -> u64 {
        match *self {
            Table::Vram(a) | Table::Sys(a) => a,
        }
    }
}

fn virt(phys: u64) -> u64 {
    crate::arch::x86_64::grub::phys_to_virt(phys)
}

/// Every eight bytes this file has written into a table it did not allocate.
///
/// Tables allocated here are in system memory; everything in video memory below
/// `bar1PdeBase` belongs to RM. So the aperture of a write is the distinction,
/// and `write64` records the video-memory ones. `rollback` puts them back to
/// zero, what they were: `map_vram_into_bar1` refuses to touch an entry that is
/// not
static RM_WRITES: spin::Mutex<alloc::vec::Vec<(u64, u64)>> = spin::Mutex::new(alloc::vec::Vec::new());

/// The PDE bits for a table at `addr`, on either half of a dual PDE. The big
/// half keeps its address in 51:8 with a shift of 8 and the small half in 51:12
/// of the high word with a shift of 12, so a 4 KiB-aligned address is the same
/// number in both; aperture and PCF sit at 2:1 and 5:3 in each
/// (`NV_MMU_VER3_DUAL_PDE_*`). Selftest checks the encoding against the header
pub fn pde_bits(addr: u64, sysmem: bool) -> u64 {
    if sysmem {
        Table::Sys(addr).pde()
    } else {
        Table::Vram(addr).pde()
    }
}

/// The bits of a PTE: small, big or huge, the format is one
pub fn pte_bits(phys: u64, aper: Aperture, privileged: bool) -> u64 {
    let bits = match (aper, privileged) {
        (Aperture::Vram, false) => PTE_APERTURE_VIDEO | PTE_PCF_RW_CACHED,
        (Aperture::Sysmem, false) => PTE_APERTURE_SYSCOH | PTE_PCF_RW_UNCACHED,
        (Aperture::Vram, true) => PTE_APERTURE_VIDEO | PTE_PCF_PRIV_RW_CACHED,
        (Aperture::Sysmem, true) => PTE_APERTURE_SYSCOH | PTE_PCF_PRIV_RW_UNCACHED,
    };
    phys | PTE_VALID | bits | PTE_KIND_PITCH
}

/// Decode a dual PDE's big half, the low word, into the big table it names.
///
/// ADDRESS_BIG is 51:8 with a shift of 8: a big table covers its 2 MiB in 32
/// entries of 8 bytes, so it is 256 bytes and 256-byte aligned. `None` for an
/// empty half or a level-1 entry that is a 2 MiB PTE
fn big_pde_table(entry: u64) -> Option<Table> {
    if entry & PTE_VALID != 0 {
        return None;
    }
    let addr = entry & 0x000f_ffff_ffff_ff00;
    match (entry >> 1) & 3 {
        0 => None,
        1 => Some(Table::Vram(addr)),
        _ => Some(Table::Sys(addr)),
    }
}

/// The address of the big table a dual PDE's low word names, for the selftest
/// that checks `pde_bits` decodes back to where it pointed
pub fn big_pde_addr(entry: u64) -> Option<u64> {
    big_pde_table(entry).map(parent_phys)
}

/// Decode a PDE back into the table it points at. `None` when the entry is not
/// a valid directory: untouched, sparse, or a PTE
fn pde_table(entry: u64) -> Option<Table> {
    // Bit 0 set means the entry is a PTE rather than a directory; PCF 0 with
    // aperture 0 is an untouched or invalid entry
    if entry & 1 != 0 {
        return None;
    }
    let addr = entry & 0x000f_ffff_ffff_f000;
    match (entry >> 1) & 3 {
        0 => None,
        1 => Some(Table::Vram(addr)),
        _ => Some(Table::Sys(addr)),
    }
}

/// A window into video memory that the CPU can write with ordinary stores.
/// `cpu` is the pointer a caller writes through, and `va`/`bytes` say which
/// part of video memory that is
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Bar1Window {
    /// Address inside the BAR1 aperture: both the GPU-side virtual address and
    /// the offset from the BAR's physical base
    pub va: u64,
    /// Where the CPU writes. Mapped write-combining; see `mm::vmm::init_pat`
    pub cpu: *mut u8,
    pub bytes: u64,
}

// The pointer is an MMIO aperture that outlives everything and is not aliased
// by any Rust allocation
unsafe impl Send for Bar1Window {}

/// Map `bytes` of video memory at `vram` into BAR1, and hand back a pointer.
///
/// `pdb` is `bar1PdeBase` from the GSP static config, the page directory RM
/// built for BAR1 in video memory. Levels below it that do not exist yet are
/// allocated here, in system memory, and leaked: a display surface is mapped
/// once.
///
/// Refuses if something is already mapped at the chosen address. RM allocates
/// BAR1 from the bottom, so the address is halfway up the aperture.
///
/// A mapping that gets part way and then refuses leaves nothing behind: its
/// entries in RM's directory are taken back out
// Parked with the rest of the BAR1 path; see the module note at the top of this
// file
#[allow(dead_code)]
pub fn map_vram_into_bar1(
    bar0: &MmioRegion,
    bar1_phys: u64,
    bar1_size: u64,
    pdb: u64,
    vram: u64,
    bytes: u64,
    say: &dyn Fn(&str),
) -> Result<Bar1Window, &'static str> {
    RM_WRITES.lock().clear();
    let r = map_locked(bar0, bar1_phys, bar1_size, pdb, vram, bytes, say);
    if r.is_err() {
        rollback(bar0, pdb);
    }
    r
}

/// Undo every write this file made into RM's page directory.
///
/// Also for the caller: a mapping can be built as intended and still be wrong,
/// and only a readback through PRAMIN says so, after this returns
// Parked with the rest of the BAR1 path; see the module note at the top of this
// file
#[allow(dead_code)]
pub fn rollback(bar0: &MmioRegion, pdb: u64) -> usize {
    // Drained first: `write64` records into the same lock, and the writes below
    // go straight out
    let entries: alloc::vec::Vec<(u64, u64)> = core::mem::take(&mut *RM_WRITES.lock());
    if !entries.is_empty() {
        let mut w = pramin::Window::new(bar0);
        for (base, off) in entries.iter() {
            w.write32(base + off, 0);
            w.write32(base + off + 4, 0);
        }
        drop(w);
        invalidate(bar0, pdb, true, SpaceKind::Bar);
    }
    entries.len()
}

/// Which side of the bus a mapping's backing store is on
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aperture {
    Vram,
    Sysmem,
}

/// Map `bytes` of `phys` at virtual address `va`, in the address space whose
/// root page directory is `pdb`, creating every level that does not exist yet.
///
/// The caller says where, and in a directory we built nothing exists below the
/// root until it is written.
///
/// Anywhere in the 57 bits six levels reach, checked by `check_range`. The
/// levels above the page table are indexed from the address itself
/// (`level_steps`)
pub fn map_at(
    bar0: &MmioRegion,
    pdb: u64,
    va: u64,
    phys: u64,
    bytes: u64,
    aper: Aperture,
) -> Result<Pages, &'static str> {
    map_pages(bar0, pdb, va, phys, bytes, aper, false)
}

/// The same mapping, marked privileged. For memory the hardware walks on its
/// own: a graphics context and everything promoted with it
pub fn map_at_priv(
    bar0: &MmioRegion,
    pdb: u64,
    va: u64,
    phys: u64,
    bytes: u64,
    aper: Aperture,
) -> Result<Pages, &'static str> {
    map_pages(bar0, pdb, va, phys, bytes, aper, true)
}

// `vaspace.rs` keeps the record of what is mapped where, per page directory,
// and is the one caller of `map_at`/`unmap_at` for a mapping somebody will look
// for again. This file writes the entries.

/// How far up the address space the six levels reach. 12 bits of page offset,
/// then 9 + 8 + 9 + 9 + 9 + 1 across levels 0 to 5, which is 57 bits
const VA_BITS: u32 = 57;

/// One level-1 table covers 2 MiB x 256 = 512 MiB, so this is what the four
/// levels above it have in common
const L1_COVER_SHIFT: u32 = 29;

/// The index `va` takes at each level above the page table, outermost first:
/// level 5, then 4, 3 and 2. The result of each step is the table the next
/// index is applied to, so these are the parent's indices. Below 512 MiB all
/// are zero. Checked by `nvidia selftest`
pub fn level_steps(va: u64) -> [u64; 4] {
    [
        (va >> 56) & 0x1,
        (va >> 47) & 0x1ff,
        (va >> 38) & 0x1ff,
        (va >> L1_COVER_SHIFT) & 0x1ff,
    ]
}

/// Does a binding of `bytes` at `phys` reach into
/// `[other, other + other_bytes)`? Half-open on both sides: a pool ending where
/// a mapping begins does not overlap it
pub fn ranges_overlap(phys: u64, bytes: u64, other: u64, other_bytes: u64) -> bool {
    let end = phys.saturating_add(bytes);
    let other_end = other.saturating_add(other_bytes);
    phys < other_end && other < end
}

/// The shared argument check for both directions
pub fn check_range(va: u64, bytes: u64) -> Result<(), &'static str> {
    // Both callers normalise `bytes` to whole pages and refuse zero first, but
    // this is public, and `bytes - 1` on zero is a panic in a debug build and a
    // whole-address-space range in a release one
    if bytes == 0 {
        return Err("an empty range");
    }
    let last = va.checked_add(bytes - 1).ok_or("the range wraps the address space")?;
    if last >> VA_BITS != 0 {
        return Err("the range is above what six levels of page table reach");
    }
    Ok(())
}

/// Walk from RM's root down to the level-1 table covering `va`.
///
/// The index at each step is the parent's own level index. `create` says
/// whether a missing level is built or reported: mapping needs the first,
/// unmapping the second
fn walk_to_l1(
    bar0: &MmioRegion,
    pdb: u64,
    va: u64,
    create: bool,
) -> Result<Option<Table>, &'static str> {
    // Every level above the page table has 8-byte entries; only level 1's dual
    // PDE is 16, indexed by the caller
    let mut t = Table::Vram(pdb);
    for index in level_steps(va) {
        t = if create {
            descend(bar0, t, index, 8)?
        } else {
            match walk_one(bar0, t, index, 8)? {
                Some(next) => next,
                None => return Ok(None),
            }
        };
    }
    Ok(Some(t))
}

/// Take a range out of the address space.
///
/// A table's count reaching zero means it describes nothing. Then, in nouveau's
/// order: clear the entry pointing at the table, let the caller's single
/// invalidate make that visible, and only then let the page be handed out.
///
/// A range never mapped is not an error: a teardown has to be retryable
pub fn unmap_at(
    bar0: &MmioRegion,
    pdb: u64,
    va: u64,
    bytes: u64,
) -> Result<(), &'static str> {
    if pdb == 0 {
        return Err("no page directory");
    }
    if va % PAGE_SIZE != 0 {
        return Err("unmapping is not page aligned");
    }
    let bytes = bytes.next_multiple_of(PAGE_SIZE);
    if bytes == 0 {
        return Err("nothing to unmap");
    }
    check_range(va, bytes)?;
    if keep_tables_forever() {
        // The escape hatch: leave every translation in place
        return Ok(());
    }

    let mut done = 0u64;
    while done < bytes {
        let this = va + done;
        let span = (PD0_SPAN - (this & (PD0_SPAN - 1))).min(bytes - done);
        // Walked per span, since a release can unlink the very table a cache
        // would hold. Once a level is gone the walk reports it missing and the
        // rest of the range is skipped
        let Some(l1) = walk_to_l1(bar0, pdb, this, false)? else {
            done += span;
            continue;
        };
        let pdei = (this >> 21) & 0xff;
        let low = l1.read64(bar0, pdei * 16);

        // A 2 MiB page. Written only for a piece wholly inside one mapping, and
        // a mapping is taken out whole, so a range touching it covers it. If
        // one does not, the entry still goes: a fault in the engine beats a
        // translation to memory about to be handed to somebody else
        if low & PTE_VALID != 0 {
            if span != PD0_SPAN {
                crate::kwarn!("[gpu/mmu] unmapping {:#x} + {:#x} cuts a 2 MiB page at {:#x} - taken out whole",
                    this, span, this & !(PD0_SPAN - 1));
            }
            l1.write64(bar0, pdei * 16, 0);
            l1.write64(bar0, pdei * 16 + 8, 0);
            table_release(bar0, parent_phys(l1), 1);
            done += span;
            continue;
        }

        // 64 KiB pages: every entry the range reaches into, for the same reason
        if let Some(bpt) = big_pde_table(low) {
            let mut removed = 0u32;
            let first = this & !(BIG_SPAN - 1);
            let mut at = first;
            while at < this + span {
                let i = (at >> 16) & 0x1f;
                if bpt.read64(bar0, i * 8) & PTE_VALID != 0 {
                    if at < this || at + BIG_SPAN > this + span {
                        crate::kwarn!("[gpu/mmu] unmapping {:#x} + {:#x} cuts a 64 KiB page at {:#x} - taken out whole",
                            this, span, at);
                    }
                    removed += 1;
                    bpt.write64(bar0, i * 8, 0);
                }
                at += BIG_SPAN;
            }
            table_release(bar0, parent_phys(bpt), removed);
        }

        // 4 KiB pages
        if let Some(spt) = pde_table(l1.read64(bar0, pdei * 16 + 8)) {
            let mut removed = 0u32;
            for off in (0..span).step_by(PAGE_SIZE as usize) {
                let ptei = ((this + off) >> PAGE_SHIFT) & 0x1ff;
                if spt.read64(bar0, ptei * 8) & PTE_VALID != 0 {
                    removed += 1;
                    spt.write64(bar0, ptei * 8, 0);
                }
            }
            table_release(bar0, parent_phys(spt), removed);
        }
        done += span;
    }

    // One invalidate for the whole range, and only then may the unlinked pages
    // be handed out: "GPU may have cached the PTs, flush before freeing"
    invalidate(bar0, pdb, true, SpaceKind::Channel);
    settle_released_tables();
    Ok(())
}

fn map_pages(
    bar0: &MmioRegion,
    pdb: u64,
    va: u64,
    phys: u64,
    bytes: u64,
    aper: Aperture,
    privileged: bool,
) -> Result<Pages, &'static str> {
    if pdb == 0 {
        return Err("no page directory");
    }
    if va % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
        return Err("mapping is not page aligned");
    }
    let bytes = bytes.next_multiple_of(PAGE_SIZE);
    if bytes == 0 {
        return Err("nothing to map");
    }
    check_range(va, bytes)?;

    let small_only = small_pages_only();
    let mut pages = Pages::default();

    let mut done = 0u64;
    // The four levels above the page table change only every 512 MiB, so they
    // are walked once per region
    let mut region: Option<(u64, Table)> = None;
    while done < bytes {
        let this = va + done;
        let l1 = match region {
            Some((r, t)) if r == this >> L1_COVER_SHIFT => t,
            _ => {
                let t = walk_to_l1(bar0, pdb, this, true)?
                    .ok_or("the walk down to the page table could not be built")?;
                region = Some((this >> L1_COVER_SHIFT, t));
                t
            }
        };
        let pdei = (this >> 21) & 0xff;
        let (lo_off, hi_off) = (pdei * 16, pdei * 16 + 8);
        let span = (PD0_SPAN - (this & (PD0_SPAN - 1))).min(bytes - done);
        let low = l1.read64(bar0, lo_off);
        let high = l1.read64(bar0, hi_off);
        // A 2 MiB page here is somebody's mapping: `vaspace` takes out whatever
        // a new mapping overlaps before it gets this far
        if low & PTE_VALID != 0 {
            return Err("a 2 MiB page is already mapped there");
        }

        // The whole entry as one 2 MiB page, when the piece allows it and
        // nothing hangs off the entry yet. A table left there (`nogpuvm`, or
        // another mapping in the same 2 MiB) leaves the piece to 64 KiB pages
        if page_for(this, phys + done, bytes - done, aper, small_only) == PageSize::Huge
            && low == 0
            && high == 0
        {
            let pte = pte_bits(phys + done, aper, privileged);
            l1.write64(bar0, lo_off, pte);
            table_add(parent_phys(l1), 1);
            if l1.read64(bar0, lo_off) != pte {
                return Err("the page directory did not take the 2 MiB page");
            }
            pages.huge += 1;
            done += span;
            continue;
        }

        // Otherwise 64 KiB and 4 KiB pages, each in its own table under one of
        // the entry's two halves. Both tables under one entry is an ordinary
        // state, "hybrid" in OpenRM's walker, and an empty big PTE sends the
        // MMU on to the small table. Both sizes must never be valid over one
        // address, so each write checks the other table
        let mut big = match big_pde_table(low) {
            Some(t) => Some(t),
            None if low != 0 => return Err("that address is sparse or reserved"),
            None => None,
        };
        let mut small = match pde_table(high) {
            Some(t) => Some(t),
            None if high != 0 => return Err("that address is sparse or reserved"),
            None => None,
        };
        // Slots going from invalid to valid, so each table knows how many
        // translations it holds. Counted even when the entry is left half done:
        // the caller takes a failed mapping out with `unmap_at`, which releases
        // against this count
        let (mut big_added, mut small_added) = (0u32, 0u32);
        let pieces: Result<(), &'static str> = 'pieces: {
            let mut off = 0u64;
            while off < span {
                let at = this + off;
                let to = phys + done + off;
                let size = match page_for(at, to, bytes - done - off, aper, small_only) {
                    PageSize::Small => PageSize::Small,
                    // A huge piece that could not have the entry is 64 KiB
                    // pages
                    PageSize::Big | PageSize::Huge => PageSize::Big,
                };
                if size == PageSize::Big {
                    let bpt = match big {
                        Some(t) => t,
                        None => {
                            let t = match alloc_table(bar0, Some((l1, lo_off))) {
                                Ok(t) => t,
                                Err(e) => break 'pieces Err(e),
                            };
                            l1.write64(bar0, lo_off, t.pde());
                            table_add(parent_phys(l1), 1);
                            big = Some(t);
                            if l1.read64(bar0, lo_off) != t.pde() {
                                break 'pieces Err("the page directory did not take the big table");
                            }
                            t
                        }
                    };
                    if let Some(spt) = small {
                        let first = (at >> PAGE_SHIFT) & 0x1ff;
                        if (first..first + 16).any(|i| spt.read64(bar0, i * 8) & PTE_VALID != 0) {
                            break 'pieces Err("4 KiB pages are mapped under that 64 KiB page");
                        }
                    }
                    let i = (at >> 16) & 0x1f;
                    if bpt.read64(bar0, i * 8) & PTE_VALID == 0 {
                        big_added += 1;
                    }
                    bpt.write64(bar0, i * 8, pte_bits(to, aper, privileged));
                    pages.big += 1;
                    off += BIG_SPAN;
                } else {
                    let spt = match small {
                        Some(t) => t,
                        None => {
                            let t = match alloc_table(bar0, Some((l1, hi_off))) {
                                Ok(t) => t,
                                Err(e) => break 'pieces Err(e),
                            };
                            l1.write64(bar0, hi_off, t.pde());
                            table_add(parent_phys(l1), 1);
                            small = Some(t);
                            if l1.read64(bar0, hi_off) != t.pde() {
                                break 'pieces Err("the page directory did not take the entry");
                            }
                            t
                        }
                    };
                    if let Some(bpt) = big {
                        if bpt.read64(bar0, ((at >> 16) & 0x1f) * 8) & PTE_VALID != 0 {
                            break 'pieces Err("a 64 KiB page is mapped over that 4 KiB page");
                        }
                    }
                    let ptei = (at >> PAGE_SHIFT) & 0x1ff;
                    if spt.read64(bar0, ptei * 8) & PTE_VALID == 0 {
                        small_added += 1;
                    }
                    spt.write64(bar0, ptei * 8, pte_bits(to, aper, privileged));
                    pages.small += 1;
                    off += PAGE_SIZE;
                }
            }
            Ok(())
        };
        if let Some(t) = big {
            table_add(parent_phys(t), big_added);
        }
        if let Some(t) = small {
            table_add(parent_phys(t), small_added);
        }
        pieces?;
        done += span;
    }

    // Nothing else is walking the directory yet, but the TLB is shared and RM
    // has this page directory, so the invalidate the BAR1 path ends with,
    // reaching the engines' TLBs too
    invalidate(bar0, pdb, true, SpaceKind::Channel);
    Ok(pages)
}

// Parked with the rest of the BAR1 path; see the module note at the top of this
// file
#[allow(dead_code)]
fn map_locked(
    bar0: &MmioRegion,
    bar1_phys: u64,
    bar1_size: u64,
    pdb: u64,
    vram: u64,
    bytes: u64,
    say: &dyn Fn(&str),
) -> Result<Bar1Window, &'static str> {
    if pdb == 0 {
        return Err("GSP-RM reported no BAR1 page directory");
    }
    if bar1_phys == 0 || bar1_size == 0 {
        return Err("BAR1 is not assigned");
    }
    if vram % PAGE_SIZE != 0 {
        return Err("surface is not page aligned");
    }
    let bytes = bytes.next_multiple_of(PAGE_SIZE);
    if bytes == 0 || bytes > bar1_size / 2 {
        return Err("surface does not fit in half of BAR1");
    }

    // As low in the aperture as there is room for. RM has built one 64 KiB page
    // table for the first 2 MiB, with nothing in it, and the field giving the
    // address space's limit reads back as a fill pattern, so how much of the
    // 256 MiB aperture is translated is unknown
    let root = Table::Vram(pdb);
    let l4 = descend(bar0, root, 0, 8)?;
    let l3 = descend(bar0, l4, 0, 8)?;
    let l2 = descend(bar0, l3, 0, 8)?;
    let l1 = descend(bar0, l2, 0, 8)?;

    // From the bottom, skipping every 2 MiB whose dual PDE has either half
    // written, even a big half whose table holds no valid entry.
    //
    // `gh100_vmm_lpt_invalid` fills big-page entries with
    // `PCF_NO_VALID_4KB_PAGE`, and `gh100_vmm_pgt_sparse` with `PCF_SPARSE`:
    // both have VALID clear, and both tell the MMU something other than "keep
    // looking". RM fills a BAR address space with sparse (`kbusInitBar1_GM107`:
    // "sparsify the VAS to prevent faults during CPU access"), and a sparse
    // range swallows writes and reads back nothing.
    //
    // So a non-zero big half disqualifies the span
    let spans = bytes.div_ceil(PD0_SPAN);
    let mut found = None;
    'search: for start in (0..bar1_size.saturating_sub(bytes) + 1).step_by(PD0_SPAN as usize) {
        for s in 0..spans {
            let pdei = ((start + s * PD0_SPAN) >> 21) & 0xff;
            if l1.read64(bar0, pdei * 16) != 0 || l1.read64(bar0, pdei * 16 + 8) != 0 {
                continue 'search;
            }
        }
        found = Some(start);
        break;
    }
    let Some(va) = found else {
        return Err("no run of free 2 MiB entries in BAR1");
    };
    // Everything below stays inside the one level-1 table found above, true for
    // any aperture up to 512 MiB, and checked
    if (va >> 29) != 0 || ((va + bytes - 1) >> 29) != 0 {
        return Err("the mapping crosses a level-1 boundary");
    }

    // Level 1 is the dual PDE: 16 bytes, the low half describing a 64 KiB page
    // table and the high half a 4 KiB one (`NV_MMU_VER3_DUAL_PDE`;
    // `gh100_vmm_pd0_pde` writes them in that order). Only the high half is
    // written here
    let mut mapped = 0u64;
    while mapped < bytes {
        let this_va = va + mapped;
        let pdei = (this_va >> 21) & 0xff;
        // The big half is only read here, and any value in it is RM having said
        // something about this 2 MiB, a 64 KiB mapping or a sparse range. Both
        // make the small half useless
        if l1.read64(bar0, pdei * 16) != 0 {
            return Err("RM has the big half of that dual PDE");
        }
        let entry = l1.read64(bar0, pdei * 16 + 8);
        let spt = match pde_table(entry) {
            Some(_) => return Err("something is already mapped there"),
            None if entry != 0 => return Err("that address is sparse or reserved"),
            None => {
                let t = alloc_table(bar0, Some((l1, pdei * 16 + 8)))?;
                l1.write64(bar0, pdei * 16 + 8, t.pde());
                // Read it back: this entry goes into RM's own directory through
                // PRAMIN, and a directory silently refusing the write looks
                // like a mapping the GPU refuses to walk
                if l1.read64(bar0, pdei * 16 + 8) != t.pde() {
                    return Err("the page directory did not take the entry");
                }
                t
            }
        };

        // One 2 MiB span, or whatever is left of the surface
        let span = (PD0_SPAN - (this_va & (PD0_SPAN - 1))).min(bytes - mapped);
        for off in (0..span).step_by(PAGE_SIZE as usize) {
            let ptei = ((this_va + off) >> PAGE_SHIFT) & 0x1ff;
            if spt.read64(bar0, ptei * 8) & PTE_VALID != 0 {
                return Err("something is already mapped there");
            }
            spt.write64(
                bar0,
                ptei * 8,
                (vram + mapped + off)
                    | PTE_VALID
                    | PTE_APERTURE_VIDEO
                    | PTE_PCF_RW_CACHED
                    | PTE_KIND_PITCH,
            );
        }
        mapped += span;
    }

    // Everything written, read back as numbers. The PTE writes and the PDE
    // write land in two different tables
    {
        let pdei = (va >> 21) & 0xff;
        let spt = pde_table(l1.read64(bar0, pdei * 16 + 8));
        let last = va + bytes - PAGE_SIZE;
        let (first_pte, last_pte) = match (spt, pde_table(l1.read64(bar0, ((last >> 21) & 0xff) * 16 + 8))) {
            (Some(a), Some(b)) => (
                a.read64(bar0, ((va >> PAGE_SHIFT) & 0x1ff) * 8),
                b.read64(bar0, ((last >> PAGE_SHIFT) & 0x1ff) * 8),
            ),
            _ => (0, 0),
        };
        say(&alloc::format!(
            "R bar1 wrote va {:#x} pde big {:#x} small {:#x} pte[first] {:#x} pte[last] {:#x}",
            va,
            l1.read64(bar0, pdei * 16),
            l1.read64(bar0, pdei * 16 + 8),
            first_pte,
            last_pte
        ));
    }

    // Clean L2 down to memory before the invalidate, where RM does it
    // (`kbusVerifyBar2_GH100` writes through one path, evicts L2, then reads
    // through the other). Once, here, not on the console's path
    say(&alloc::format!(
        "R bar1 l2 clean {}",
        if pramin::l2_flush_dirty(bar0) { "ok" } else { "unavailable" }
    ));

    // A TLB that never reported the invalidate finished means a stuck MMU, not
    // a mapping problem. The caller undoes the writes on this error like any
    // other
    if !invalidate(bar0, pdb, true, SpaceKind::Bar) {
        return Err("the MMU never finished the TLB invalidate");
    }
    let (skipped, skipped_all) = *LAST_INVALIDATE.lock();
    say(&alloc::format!(
        "R bar1 tlb invalidate pdb skipped {} all-pdb skipped {}",
        skipped, skipped_all
    ));

    // The aperture is mapped write-combining: four uncached stores per pixel is
    // what PRAMIN costs
    crate::mm::vmm::map_mmio_wc(bar1_phys + va, bytes);
    Ok(Bar1Window {
        va,
        cpu: virt(bar1_phys + va) as *mut u8,
        bytes,
    })
}

/// Read the BAR1 hierarchy without touching it, and say what is there. Breaking
/// RM's aperture shows up as an RPC timeout, so the entries come out as raw
/// numbers before anything is written
// Parked with the rest of the BAR1 path; see the module note at the top of this
// file
#[allow(dead_code)]
pub fn dump_walk(
    bar0: &MmioRegion,
    bar1_phys: u64,
    bar1_size: u64,
    pdb: u64,
    va: u64,
    say: &dyn Fn(&str),
) {
    let root = Table::Vram(pdb);
    // The aperture goes in the same line as its directory, so a log cannot lose
    // one of them
    say(&alloc::format!(
        "R bar1 at {:#x} size {:#x} pdb {:#x} root[0] {:#018x} root[1] {:#018x}",
        bar1_phys,
        bar1_size,
        pdb,
        root.read64(bar0, 0),
        root.read64(bar0, 8)
    ));

    // VA 0 as well as the candidate. RM maps its own things from the bottom of
    // the aperture, so the walk down VA 0 is a hierarchy known to work. A walk
    // dying at the first level on both addresses means the format or the
    // address is wrong; a valid PTE at 0 and nothing at the candidate means the
    // format is right and the address is empty
    for va in [0u64, va] {
        let mut here = Some(root);
        let mut line = alloc::format!("R bar1 va {:#x}", va);
        let mut lpt_line: Option<alloc::string::String> = None;
        for (name, i, sz) in [
            ("l4", (va >> 56) & 0x1, 8u64),
            ("l3", (va >> 47) & 0x1ff, 8),
            ("l2", (va >> 38) & 0x1ff, 8),
            ("l1", (va >> 29) & 0x1ff, 8),
            // The dual PDE: 16 bytes, low half for 64 KiB pages and high half
            // for 4 KiB. Both are printed: a non-zero low half is RM holding
            // this 2 MiB
            ("l0", (va >> 21) & 0xff, 16),
        ] {
            let Some(t) = here else {
                line.push_str(&alloc::format!(" {}:-", name));
                continue;
            };
            if sz == 16 {
                let big = t.read64(bar0, i * 16);
                let small = t.read64(bar0, i * 16 + 8);
                line.push_str(&alloc::format!(" {}[{}]=big {:#x} small {:#x}", name, i, big, small));
                // What is in the 64 KiB table, as numbers. VALID bits alone
                // cannot tell an empty table from one full of `PCF_SPARSE` or
                // `PCF_NO_VALID_4KB_PAGE`, three different things to the MMU.
                // PCF is 7:3
                if let Some(lpt) = big_pde_table(big) {
                    let mut l = alloc::format!("R bar1 lpt {:#x}", lpt.addr());
                    for e in 0..4u64 {
                        let v = lpt.read64(bar0, e * 8);
                        l.push_str(&alloc::format!(" [{}]={:#x}(pcf {})", e, v, (v >> 3) & 0x1f));
                    }
                    lpt_line = Some(l);
                }
                here = pde_table(small);
                continue;
            }
            let e = t.read64(bar0, i * sz);
            line.push_str(&alloc::format!(" {}[{}]={:#x}", name, i, e));
            here = pde_table(e);
        }
        // And one leaf, the entry whose format all this depends on
        if let Some(t) = here {
            let ptei = (va >> 12) & 0x1ff;
            line.push_str(&alloc::format!(" pte[{}]={:#x}", ptei, t.read64(bar0, ptei * 8)));
        }
        say(&line);
        if let Some(l) = lpt_line {
            say(&l);
        }
    }
}

/// What the hardware says BAR1 is bound to.
///
/// `bar1PdeBase` from the GSP static config and a virtual aperture over its
/// whole length are assumptions, and each fails the same way: a mapping that
/// builds and addresses nothing.
///
/// `kbusBar1InstBlkBind_GH100` writes
/// `NV_VIRTUAL_FUNCTION_PRIV_FUNC_BAR1_BLOCK_LOW_ADDR/HIGH_ADDR`, which on
/// Hopper and later live in the VF aperture at 0xb80f60, not in
/// `NV_PBUS_BAR1_BLOCK` as through Turing, and those name the instance block.
/// The instance block carries the page directory the MMU walks, the address
/// limit of the space, and the big page size, at the `NV_RAMIN_*` offsets
/// (dwords 128 through 131)
#[derive(Clone, Copy, Debug)]
/// What BAR1's instance block says about itself. Read by `read_bar1_binding`,
/// parked with the rest of the aperture work
#[allow(dead_code)]
pub struct Bar1Binding {
    pub inst: u64,
    /// False means BAR1 offsets are video-memory offsets and page tables are
    /// not involved
    pub virtual_mode: bool,
    pub target: u32,
    /// The directory the hardware walks. A mapping goes into this one, whatever
    /// GSP-RM said
    pub pdb: u64,
    pub big_page_64k: bool,
    /// Highest address the space translates, inclusive. A VA above it faults
    pub limit: u64,
}

// Parked with the rest of the BAR1 path; see the module note at the top of this
// file
#[allow(dead_code)]
pub fn read_bar1_binding(bar0: &MmioRegion, say: &dyn Fn(&str)) -> Option<Bar1Binding> {
    const BLK_LO: u32 = 0x00b8_0f60;
    const BLK_HI: u32 = 0x00b8_0f64;

    let lo = bar0.read32(BLK_LO);
    let hi = bar0.read32(BLK_HI);
    // A register that is not there answers with the PRI poison pattern
    if MmioRegion::is_pri_error(lo) {
        say(&alloc::format!("R bar1 blk register reads {:#x} - not this address", lo));
        return None;
    }

    let inst = ((hi as u64) << 32) | (lo as u64 & 0xffff_f000);
    let binding_target = (lo >> 10) & 3;
    let virtual_mode = (lo >> 9) & 1 == 1;
    if inst == 0 {
        say("R bar1 blk names no instance block");
        return None;
    }

    // The instance block itself, wherever it was put
    let blk = if binding_target == 0 { Table::Vram(inst) } else { Table::Sys(inst) };
    let base = blk.read64(bar0, 0x200);
    let limit = blk.read64(bar0, 0x208);
    let b = Bar1Binding {
        inst,
        virtual_mode,
        target: binding_target,
        pdb: ((base >> 32) << 32) | (base & 0xffff_f000),
        big_page_64k: (base >> 11) & 1 == 1,
        limit: ((limit >> 32) << 32) | (limit & 0xffff_f000) | 0xfff,
    };
    say(&alloc::format!(
        "R bar1 blk inst {:#x} tgt {} mode {} pdb {:#x} big {} limit {:#x}",
        b.inst,
        b.target,
        if b.virtual_mode { "virtual" } else { "PHYSICAL" },
        b.pdb,
        if b.big_page_64k { "64K" } else { "128K" },
        b.limit
    ));
    // The raw register too, and its two non-address bits: PENDING (0:0) and
    // OUTSTANDING (1:1) are what `kbusBar1InstBlkBind_GH100` polls after a
    // bind. A bind that never settled answers nothing whatever the page tables
    // say
    say(&alloc::format!(
        "R bar1 blk raw lo {:#010x} hi {:#010x} pending {} outstanding {}",
        lo, hi, lo & 1, (lo >> 1) & 1
    ));
    Some(b)
}

/// Ask the MMU why it refused.
///
/// Every refused access writes an entry into a fault buffer with the address
/// and the reason. `NV_VIRTUAL_FUNCTION_PRIV_MMU_FAULT_BUFFER_*` at 0xb83000
/// names the buffer and carries its GET and PUT pointers; the entry layout is
/// `NVC369_BUF_ENTRY_*`, 32 bytes, with the address at dwords 2 and 3 and the
/// reason at dword 7.
///
/// The reasons are `NV_PFAULT_FAULT_TYPE_*` in `dev_fault.h`: 0 is a bad page
/// directory entry, 2 a bad page table entry, 3 the address past the end of the
/// address space, 4 an instance block not bound, 9 an aperture the engine
/// cannot use
pub fn dump_mmu_faults(bar0: &MmioRegion, say: &dyn Fn(&str)) {
    const BUF: u32 = 0x00b8_3000;

    for i in 0..2u32 {
        let base = BUF + i * 32;
        let lo = bar0.read32(base);
        let hi = bar0.read32(base + 4);
        let get = bar0.read32(base + 8);
        let put = bar0.read32(base + 12);
        if MmioRegion::is_pri_error(lo) || (lo == 0 && hi == 0) {
            continue;
        }
        let phys = ((hi as u64) << 32) | (lo & 0xffff_f000) as u64;
        let entries = put & 0x000f_ffff;
        say(&alloc::format!(
            "R mmu buf{} at {:#x} aper {} get {} put {}",
            i, phys, (lo >> 1) & 3, get & 0x000f_ffff, entries
        ));
        // A buffer the GPU addresses virtually is not one we can follow
        if lo & 1 != 0 || entries == 0 {
            continue;
        }

        // The newest entry. Reading it needs the buffer's aperture: LOCAL is
        // video memory through PRAMIN, the two SYS values are ordinary memory
        // through the direct map
        let off = (entries as u64 - 1) * 32;
        let t = if (lo >> 1) & 3 == 0 { Table::Vram(phys) } else { Table::Sys(phys) };
        let addr = t.read64(bar0, off + 8);
        let reason = t.read64(bar0, off + 24);
        let fault_va = ((addr >> 32) << 32) | (addr & 0xffff_f000);
        let w7 = (reason >> 32) as u32;
        say(&alloc::format!(
            "R mmu fault va {:#x} type {} access {} valid {}",
            fault_va,
            w7 & 0x1f,
            (w7 >> 16) & 0xf,
            w7 >> 31
        ));
    }
}

/// Prove the CPU can read video memory through BAR1, writing nothing.
///
/// A mapping can build cleanly and read back nothing for two reasons with
/// opposite fixes: page tables the GPU does not accept, or a CPU-side address
/// that is not the aperture (a VA past the end of BAR1, a wrong BAR, the wrong
/// memory type).
///
/// RM's own mapping of the bottom of the aperture is live, so following it to
/// the video memory behind BAR1 offset 0 and reading the same bytes twice, with
/// an ordinary load and through PRAMIN, tests the CPU side alone. Agreement
/// leaves only our page tables; disagreement means no page table would help
// Parked with the rest of the BAR1 path; see the module note at the top of this
// file
#[allow(dead_code)]
pub fn probe_bar1_readable(
    bar0: &MmioRegion,
    bar1_phys: u64,
    va: u64,
    pdb: u64,
    say: &dyn Fn(&str),
) -> bool {
    if pdb == 0 || bar1_phys == 0 {
        return false;
    }
    // Down to the dual PDE at VA 0: four directory levels, index 0 in each
    let mut t = Table::Vram(pdb);
    for _ in 0..4 {
        match pde_table(t.read64(bar0, 0)) {
            Some(next) => t = next,
            None => {
                say("R bar1 probe: RM's directory stops above the leaf");
                return false;
            }
        }
    }

    // The big half. ADDRESS_BIG is 51:8 with a shift of 8, since a 64 KiB page
    // table covers its 2 MiB in 32 entries of 8 bytes: 256 bytes, 256-byte
    // aligned
    let big = t.read64(bar0, 0);
    if big & 1 != 0 || (big >> 1) & 3 != 1 {
        say("R bar1 probe: RM has no 64 KiB table at the bottom of BAR1");
        return false;
    }
    let lpt = Table::Vram(big & 0x000f_ffff_ffff_ff00);

    // The 64 KiB entries of that table, looking for one worth comparing. A page
    // reading as zero proves the path weakly, so take the first entry with
    // non-zero contents, and a valid one only if none has
    let mut found: Option<(u64, u64, [u32; 4])> = None;
    let mut valid = 0;
    for i in 0..32u64 {
        let pte = lpt.read64(bar0, i * 8);
        // APERTURE is 2:1 and VIDEO_MEMORY is 0 in a PTE, unlike the PDE
        // encoding, where video memory is 1. ADDRESS_VID is 39:12
        if pte & PTE_VALID == 0 {
            continue;
        }
        valid += 1;
        let aper = (pte >> 1) & 3;
        let phys = if aper == 0 {
            pte & 0x0000_00ff_ffff_f000
        } else {
            pte & 0x000f_ffff_ffff_f000
        };
        // System memory is as good a witness, read without PRAMIN: both paths
        // only have to reach the same bytes
        let want = match aper {
            0 => {
                let mut w = [0u32; 4];
                pramin::sample_vram(bar0, phys, &mut w);
                w
            }
            2 | 3 => read4(phys),
            _ => continue,
        };
        let interesting = want != [0; 4];
        if found.is_none() || interesting {
            found = Some((i * 0x1_0000, phys, want));
        }
        if interesting {
            break;
        }
    }
    say(&alloc::format!(
        "R bar1 probe lpt {:#x} valid {} of 32",
        big & 0x000f_ffff_ffff_ff00, valid
    ));

    let Some((off, vram, want)) = found else {
        // Nothing of RM's to compare against, so report what the aperture
        // answers: all-ones is a read nothing claimed
        crate::mm::vmm::map_mmio_uc(bar1_phys, PAGE_SIZE);
        let at0 = read4(bar1_phys);
        crate::mm::vmm::map_mmio_uc(bar1_phys + va, PAGE_SIZE);
        let atva = read4(bar1_phys + va);
        say(&alloc::format!(
            "R bar1 probe nothing mapped by RM - raw at0 {:#x} atva {:#x}",
            at0[0], atva[0]
        ));
        return false;
    };

    // Uncached first, then write-combining, the one the console would use. If
    // PAT slot 4 is not what `init_pat` thinks, this shows it
    crate::mm::vmm::map_mmio_uc(bar1_phys + off, PAGE_SIZE);
    let uc = read4(bar1_phys + off);
    crate::mm::vmm::map_mmio_wc(bar1_phys + off, PAGE_SIZE);
    let wc = read4(bar1_phys + off);

    // A verdict in words, for reading off a screen. The numbers come out only
    // on disagreement, and only the first word of each: different data,
    // all-ones, or zero
    say(&alloc::format!(
        "R bar1 probe off {:#x} vram {:#x} uc {} wc {} {}",
        off,
        vram,
        if uc == want { "ok" } else { "BAD" },
        if wc == want { "ok" } else { "BAD" },
        if want == [0; 4] { "(zero data - weak)" } else { "" }
    ));
    if uc != want || wc != want {
        say(&alloc::format!(
            "R bar1 probe pramin {:#x} uc {:#x} wc {:#x}",
            want[0], uc[0], wc[0]
        ));
    }

    uc == want && wc == want
}

// Parked with the rest of the BAR1 path; see the module note at the top of this
// file
#[allow(dead_code)]
fn read4(phys: u64) -> [u32; 4] {
    let p = virt(phys) as *const u32;
    let mut out = [0u32; 4];
    for (i, w) in out.iter_mut().enumerate() {
        *w = unsafe { core::ptr::read_volatile(p.add(i)) };
    }
    out
}

/// Follow one directory entry, allocating the table under it if it is absent
fn descend(bar0: &MmioRegion, parent: Table, index: u64, entry_size: u64) -> Result<Table, &'static str> {
    match walk_one(bar0, parent, index, entry_size)? {
        Some(t) => Ok(t),
        None => {
            // The parent is remembered on the child, so an empty table can be
            // unlinked without searching every directory
            let off = index * entry_size;
            let t = alloc_table(bar0, Some((parent, off)))?;
            parent.write64(bar0, off, t.pde());
            table_add(parent_phys(parent), 1);
            Ok(t)
        }
    }
}

/// The same step without creating anything: `None` when this level is not
/// there, an answer on the unmap path
fn walk_one(
    bar0: &MmioRegion,
    parent: Table,
    index: u64,
    entry_size: u64,
) -> Result<Option<Table>, &'static str> {
    let entry = parent.read64(bar0, index * entry_size);
    if let Some(t) = pde_table(entry) {
        return Ok(Some(t));
    }
    // Not a directory, but not empty: a PCF of SPARSE or an entry marked as a
    // PTE is RM saying something about this range
    if entry != 0 {
        return Err("a level of the walk is sparse or reserved");
    }
    Ok(None)
}

fn parent_phys(t: Table) -> u64 {
    match t {
        Table::Sys(a) | Table::Vram(a) => a,
    }
}

/// Where new page tables go: system memory.
///
/// The CPU reaches video memory only through PRAMIN, and RM's aperture
/// self-test says a PRAMIN write and a read through another path are not the
/// same memory until L2 is cleaned (`kbusVerifyBar2_GH100`: write through BAR2,
/// `kmemsysSendL2InvalidateEvict`, then read through the BAR0 window, "ideally
/// this should hit the L2 cache"). Page tables written through PRAMIN and read
/// by the GPU's MMU are that pairing.
///
/// In system memory the MMU fetches the directory over PCIe with no L2 in the
/// way, and RM writes the encoding itself: `gh100_vmm_pde` for
/// `NVKM_MEM_TARGET_HOST` is
/// `APERTURE_SYSTEM_COHERENT_MEMORY | PCF_VALID_UNCACHED_ATS_ALLOWED`, what
/// `Table::pde` produces.
///
/// Zeroing a table in video memory is also 1024 PRAMIN dwords at about 21 ns
/// each.

/// One page table this file built, and what points into it
struct TableEntry {
    /// The frames. Released only into `FREE_TABLES`, never back to the page
    /// allocator
    buf: DmaBuffer,
    /// Valid entries below this table: PTEs for a level-0 table, PDEs for a
    /// directory. At zero the table describes nothing and can be unlinked
    live: u32,
    /// The entry that points at this table: the parent table and the byte
    /// offset of the PDE within it.
    ///
    /// `None` for a table directly under RM's root. Those are never freed, so
    /// the unmap path never writes into RM's own directory, at a price of one 4
    /// KiB table per address space
    parent: Option<(Table, u64)>,
}

/// Every page table this file owns, by physical address. A `Vec`: there are
/// dozens of these
static TABLES: spin::Mutex<alloc::vec::Vec<(u64, TableEntry)>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// Page tables unlinked whose pages the GPU may still have cached.
///
/// nouveau states the rule in `nvkm_vmm_unref_pdes`: clear the parent PDE, then
/// flush, then release ("GPU may have cached the PTs, flush before freeing").
/// These wait here for the invalidate after the unmap, then move to
/// `FREE_TABLES`
static PENDING_TABLES: spin::Mutex<alloc::vec::Vec<DmaBuffer>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// Page tables free to be handed out again.
///
/// They never go back to the page allocator, as in nouveau (`nvkm_mmu_ptc_put`
/// returns a table to the MMU's own cache): a page that never leaves the GPU's
/// ownership cannot be handed to something else while a busmaster has a
/// translation to it. The cost is a few reserved kilobytes
static FREE_TABLES: spin::Mutex<alloc::vec::Vec<DmaBuffer>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// GSP-RM was unloaded: every address space these tables described is gone with
/// it, and so are RM's own tables this file wrote into.
///
/// The tables' pages go to `FREE_TABLES`, as in an unmap. Nothing can be
/// walking them, the engines having stopped with GSP-RM. The writes into RM's
/// video-memory tables are dropped, not rolled back
pub fn forget() {
    super::vaspace::forget_all();
    RM_WRITES.lock().clear();
    let tables = core::mem::take(&mut *TABLES.lock());
    let pending = core::mem::take(&mut *PENDING_TABLES.lock());
    let mut free = FREE_TABLES.lock();
    free.extend(tables.into_iter().map(|(_, t)| t.buf));
    free.extend(pending);
}

/// Escape hatch: never unlink or free a page table.
///
/// `unmap_at` then zeroes PTEs and stops. The address space being unmapped is
/// the live one (the blur targets, the backdrop, the console shadow and both
/// channels' rings), and a wrong index on the free path takes the picture away.
/// `nogpuvm` on the GRUB line sets it
fn keep_tables_forever() -> bool {
    crate::arch::x86_64::grub::cmdline_flag("nogpuvm")
}

/// A zeroed 4 KiB page table, registered so it can be found again. `parent` is
/// the PDE that will point at it; `None` means it hangs off RM's root and is
/// never freed
fn alloc_table(bar0: &MmioRegion, parent: Option<(Table, u64)>) -> Result<Table, &'static str> {
    let _ = bar0;
    let buf = match FREE_TABLES.lock().pop() {
        Some(b) => b,
        None => DmaBuffer::alloc(1).map_err(|_| "no memory for a page table")?,
    };
    let phys = buf.phys();
    unsafe {
        core::ptr::write_bytes(virt(phys) as *mut u8, 0, PAGE_SIZE as usize);
    }
    TABLES.lock().push((phys, TableEntry { buf, live: 0, parent }));
    Ok(Table::Sys(phys))
}

/// This table gained `n` valid entries below it
fn table_add(phys: u64, n: u32) {
    if n == 0 {
        return;
    }
    let mut t = TABLES.lock();
    if let Some(e) = t.iter_mut().find(|(p, _)| *p == phys) {
        e.1.live = e.1.live.saturating_add(n);
    }
}

/// This table lost `n` valid entries below it; unlink and release it, its
/// parent and its parent's parent, as long as that leaves them empty.
///
/// The order per level is nouveau's: clear the parent PDE, then let go of the
/// page. The caller's single invalidate makes the unlink visible, so the pages
/// go to `PENDING_TABLES` first
fn table_release(bar0: &MmioRegion, phys: u64, n: u32) {
    if keep_tables_forever() {
        return;
    }
    let mut phys = phys;
    let mut n = n;
    loop {
        let parent = {
            let mut t = TABLES.lock();
            let Some(e) = t.iter_mut().find(|(p, _)| *p == phys) else { return };
            e.1.live = e.1.live.saturating_sub(n);
            if e.1.live != 0 {
                return;
            }
            e.1.parent
        };
        // No parent means this table hangs off RM's root. It stays, so the
        // unmap path never writes into a directory RM built
        let Some((ptab, off)) = parent else { return };
        ptab.write64(bar0, off, 0);

        let buf = {
            let mut t = TABLES.lock();
            t.iter()
                .position(|(p, _)| *p == phys)
                .map(|i| t.swap_remove(i).1.buf)
        };
        if let Some(buf) = buf {
            PENDING_TABLES.lock().push(buf);
        }

        phys = match ptab {
            Table::Sys(a) | Table::Vram(a) => a,
        };
        n = 1;
    }
}

/// The invalidate has run, so the pages unlinked before it are safe to reuse
fn settle_released_tables() {
    let mut pending = PENDING_TABLES.lock();
    if pending.is_empty() {
        return;
    }
    let mut free = FREE_TABLES.lock();
    free.append(&mut pending);
}

/// How many page tables this file holds, and how many are spare. For
/// `nvidia vram` and the teardown test
pub fn table_counts() -> (usize, usize) {
    (TABLES.lock().len(), FREE_TABLES.lock().len())
}

/// Whose address space a directory is, for the invalidate.
///
/// `VASPACE_FLAGS_BAR` in OpenRM. A BAR space is walked by the host's own path
/// alone, so its invalidate stops at the hub; a channel's space is walked by
/// the engines too, the copy engine through the hub and the compute engine
/// through the TLBs in each GPC, and its invalidate reaches them all
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SpaceKind {
    /// BAR1 or BAR2: only RM's BAR1 directory here, on the parked path
    Bar,
    /// A channel's address space: the kernel's copy and compute channels, a
    /// process's own. Every directory `map_at`/`unmap_at` writes
    Channel,
}

/// Throw away what the GPU's TLB thinks this address space looks like.
///
/// Follows `kgmmuInvalidateTlb_GM107` and `kgmmuCommitTlbInvalidate_GB100`, the
/// pair GB20x uses (the membar WAR is empty on it, and the scope comes from
/// `kgmmuSetTlbInvalidationScope_GA100`):
///
///  - one directory named, `ALL_VA`, no range;
///  - `HUBTLB_ONLY` only for a BAR space ("for host VAS (e.g. BAR) we do not
///    have to invalidate GR"). A channel's invalidate has to reach the GPCs'
///    TLBs too, or the compute engine keeps translating an unmapped page, and a
///    dispatch writes into it once that page is somebody else's;
///  - `INVAL_SCOPE` `NON_LINK_TLBS`, as `gvaspaceInvalidateTlb` asks for a
///    space that is not NVLink's.
///
/// nouveau agrees on the first two (`tu102_vmm_flush`: hub-only and `ALL_PDB`
/// together, only for a BAR space), and adds `ALL_PDB`, which its own comment
/// calls a hack: it drops the translations of every address space on the chip,
/// GSP-RM's included. It is still written for a BAR space (see below); a
/// channel's space is invalidated by its directory alone. `tlball` on the GRUB
/// line puts `ALL_PDB` back everywhere.
///
/// The register: `NV_VIRTUAL_FUNCTION_PRIV_MMU_*` at 0xb830a0, with the
/// directory address as `>>12` in field 31:4, the same bits as `>>8`, and the
/// aperture in bit 1. Anything above 40 bits goes in the upper register.
///
/// Returns false if the invalidate never reported itself finished: the MMU is
/// wedged
fn invalidate(bar0: &MmioRegion, pdb: u64, vidmem: bool, kind: SpaceKind) -> bool {
    let sw = crate::kcore::time::Stopwatch::start();
    let ok = invalidate_timed(bar0, pdb, vidmem, kind);
    let us = sw.elapsed_us();
    INVALIDATE_US.fetch_add(us, core::sync::atomic::Ordering::Relaxed);
    INVALIDATE_MAX_US.fetch_max(us, core::sync::atomic::Ordering::Relaxed);
    // An invalidate is a register write and a wait for the MMU; milliseconds of
    // it is a frame the compositor did not draw
    if us >= 2000 {
        crate::kwarn!("[gpu/mmu] TLB invalidate of pdb {:#x} ({:?}) took {} us", pdb, kind, us);
    }
    ok
}

/// Time spent in invalidates, and the longest one, since boot
static INVALIDATE_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static INVALIDATE_MAX_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `(microseconds in all, the longest)` spent in TLB invalidates
pub fn invalidate_time() -> (u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (INVALIDATE_US.load(Relaxed), INVALIDATE_MAX_US.load(Relaxed))
}

fn invalidate_timed(bar0: &MmioRegion, pdb: u64, vidmem: bool, kind: SpaceKind) -> bool {
    const PDB: u32 = 0x00b8_30a0;
    const PDB_HI: u32 = 0x00b8_30a4;
    const INVALIDATE: u32 = 0x00b8_30b0;
    const TRIGGER: u32 = 0x8000_0000;
    const ALL_VA: u32 = 1 << 0;
    const ALL_PDB: u32 = 1 << 1;
    const HUBTLB_ONLY: u32 = 1 << 2;
    /// 16:15 `INVAL_SCOPE`: 0 all TLBs, 1 NVLink's, 2 everything but NVLink's
    const SCOPE_NON_LINK: u32 = 2 << 15;
    /// 30:30, read-only status: the MMU declined to do the invalidate
    const SKIPPED: u32 = 1 << 30;

    // The entries have to land before the GPU reads them, on both paths: a CPU
    // store buffer for tables in system memory, and the video-memory write path
    // for everything through PRAMIN. The driver does the second as
    // `kbusFlush(BUS_FLUSH_VIDEO_MEMORY)` before every rebind
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    pramin::flush_fb(bar0);

    // `kgmmuCheckPendingInvalidates` first: an invalidate in flight has to
    // finish before its registers can be loaded with another
    if !bar0.wait32(INVALIDATE, TRIGGER, 0, 200_000) {
        return false;
    }

    let reach = match kind {
        SpaceKind::Bar => HUBTLB_ONLY,
        SpaceKind::Channel => 0,
    };
    let aperture = if vidmem { 0 } else { 1 << 1 };
    bar0.write32(PDB, ((((pdb >> 12) & 0x0fff_ffff) as u32) << 4) | aperture);
    bar0.write32(PDB_HI, ((pdb >> 12) >> 28) as u32);
    bar0.write32(INVALIDATE, TRIGGER | ALL_VA | reach | SCOPE_NON_LINK);
    if !bar0.wait32(INVALIDATE, TRIGGER, 0, 200_000) {
        return false;
    }
    // SKIPPED is bit 30 and readable: the register says whether the invalidate
    // happened. TRIGGER clearing only means the command was consumed
    let skipped = bar0.read32(INVALIDATE) & SKIPPED != 0;

    // Then nouveau's form, for a BAR space: `ALL_PDB` as well. A PDB-targeted
    // invalidate the MMU skips leaves a stale negative translation.
    //
    // `skipped` above says whether the targeted form was refused, the only
    // thing justifying the second write; the card has shown zero in every run
    // so far. Neither reference writes it for a channel's space, and the
    // targeted form is the whole invalidate unless it was refused. All TLBs
    // then, since `ALL_PDB` overrides the scope in OpenRM too. `tlball` writes
    // it for every space
    let always_all_pdb = crate::arch::x86_64::grub::cmdline_flag("tlball");
    let second = always_all_pdb
        || skipped
        || (kind == SpaceKind::Bar && !targeted_is_enough());
    let (ok, skipped_all) = if second {
        bar0.write32(INVALIDATE, TRIGGER | ALL_VA | ALL_PDB | reach);
        let ok = bar0.wait32(INVALIDATE, TRIGGER, 0, 200_000);
        (ok, bar0.read32(INVALIDATE) & SKIPPED != 0)
    } else {
        (true, false)
    };
    if skipped {
        SKIPPED_TARGETED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    INVALIDATES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    *LAST_INVALIDATE.lock() = (skipped, skipped_all);
    ok
}

/// Is the targeted invalidate alone trusted, for a BAR space?
///
/// This answers false, so a BAR space gets the ALL_PDB write. A channel's space
/// does not ask: both references invalidate it by its directory alone.
/// `nvidia vram` prints the number that would change this
fn targeted_is_enough() -> bool {
    false
}

/// Invalidates issued, and how many times the targeted form was refused.
///
/// The second number decides nouveau's ALL_PDB hack on this chip: zero over a
/// long run of desktop work makes the second write pure cost
static INVALIDATES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SKIPPED_TARGETED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `(invalidates, targeted ones the MMU refused)`
pub fn invalidate_counts() -> (u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (INVALIDATES.load(Relaxed), SKIPPED_TARGETED.load(Relaxed))
}

/// Whether the last invalidate reported SKIPPED, for the targeted form and for
/// nouveau's all-directories form. The caller reads it, so this function only
/// talks to registers
static LAST_INVALIDATE: spin::Mutex<(bool, bool)> = spin::Mutex::new((false, false));

/// Make CPU writes through a write-combining mapping visible to the GPU.
///
/// Two separate things: the CPU's write-combining buffers are pushed out
/// (`sfence`), and the GPU's own path from the aperture to memory is flushed,
/// the same `flush_fb` PRAMIN writes end with
pub fn flush(bar0: &MmioRegion) -> bool {
    unsafe { core::arch::asm!("sfence", options(nostack, nomem)) };
    pramin::flush_fb(bar0)
}
