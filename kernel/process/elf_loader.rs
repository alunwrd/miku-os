extern crate alloc;
use alloc::vec::Vec;
use crate::process::elf::*;
use crate::mm::vmm::AddressSpace;
use crate::mm::pmm;
use crate::arch::x86_64::grub;
use x86_64::structures::paging::PageTableFlags;

const PAGE_SIZE: u64 = 4096;
const PAGE_MASK: u64 = PAGE_SIZE - 1;

// Process address-space layout (bases are randomized per exec):
//
//   ET_EXEC image   wherever it links (typically 0x40_0000)
//   heap (brk)      image end + random gap (up to 32 MiB)
//   mmap region     0x0000_0001_0000_0000 .. 0x0000_7F00_0000_0000  (mmap.rs)
//   PIE image       0x0000_5555_0000_0000 + ASLR (up to 1 TiB)
//   ld-miku         0x0000_7F00_0000_0000 + ASLR (up to 2 GiB)
//   TLS block       16 MiB below the stack's low end
//   stack           8 MiB VMA ending at 0x0000_7FFF_FFFF_0000 - ASLR (up to 16 MiB)
//
// The PIE image lives inside the mmap region's address range, but every
// image/TLS/stack range is registered in the VMA table, so mmap's find_free
// can never hand out an overlapping window

pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
pub const MAX_ELF_SIZE: usize = 64 * 1024 * 1024;

const PIE_BASE: u64 = 0x0000_5555_0000_0000;
const PIE_ASLR_BITS: u32 = 28; // Linux x86_64 mmap_rnd_bits parity
const INTERP_BASE: u64 = 0x0000_7F00_0000_0000;
const INTERP_ASLR_BITS: u32 = 19; // 2 GiB span
const STACK_ASLR_BITS: u32 = 12; // up to 16 MiB below USER_STACK_TOP
const BRK_ASLR_BITS: u32 = 13; // up to 32 MiB gap between image and heap
const ASLR_STEP: u64 = 0x1000;

/// Total stack VMA (Linux RLIMIT_STACK default). Only the top
/// STACK_EAGER_PAGES are mapped at exec; the rest faults in demand-zero
const STACK_SIZE: u64 = 8 * 1024 * 1024;
const STACK_EAGER_PAGES: usize = 64; // 256 KiB up front for argv/envp/auxv
const TLS_GAP_BELOW_STACK: u64 = 16 * 1024 * 1024;

// exec argument budget, Linux-style: bounded by bytes, not fixed slot counts
const ARG_MAX_STRING_BYTES: u64 = 128 * 1024;
const ARG_MAX_VECTOR_ENTRIES: usize = 8192;

pub type ReadFileFn<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadError {
    Parse(ElfError),
    OutOfMemory,
    MapFailed,
    FileTooLarge,
    SegmentOverlap,
    WxSegment,
    InterpReadFailed,
    InterpLoadFailed,
    RelocFailed,
    ArgvTooLong,
}

impl LoadError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parse(e) => e.as_str(),
            Self::OutOfMemory => "out of memory",
            Self::MapFailed => "page mapping failed",
            Self::FileTooLarge => "ELF file too large",
            Self::SegmentOverlap => "overlapping PT_LOAD segments",
            Self::WxSegment => "W+X segment rejected (W^X policy)",
            Self::InterpReadFailed => "cannot read ELF interpreter",
            Self::InterpLoadFailed => "cannot load ELF interpreter",
            Self::RelocFailed => "relocation failed",
            Self::ArgvTooLong => "argv/envp/auxv too large for user stack",
        }
    }
}

pub struct ElfImage {
    pub entry: u64,
    pub exe_entry: u64,
    pub stack_top: u64,
    pub brk: u64,
    pub load_bias: u64,
    pub tls_base: u64,
    pub interp_base: u64,
    pub has_interp: bool,
}

/// Undo every mapping made so far and drop the half-built VMA table.
/// All allocation paths in load() funnel their (virt, phys) pairs into one
/// shared list, so a failure at any stage releases everything at once -
/// segments, relocated interp pages, the TLS block and the eager stack
fn fail(aspace: &AddressSpace, pages: &[(u64, u64)], e: LoadError) -> LoadError {
    rollback(aspace, pages);
    crate::mm::mmap::vma_cleanup(aspace.cr3);
    e
}

pub fn load(
    data: &[u8],
    aspace: &AddressSpace,
    path: &str,
    args: &[&str],
    envs: &[&str],
    read_file: Option<ReadFileFn<'_>>,
) -> Result<ElfImage, LoadError> {
    if data.len() > MAX_ELF_SIZE {
        return Err(LoadError::FileTooLarge);
    }

    let info = parse(data).map_err(LoadError::Parse)?;
    check_overlaps(&info)?;

    // PT_GNU_STACK asking for an executable stack is refused by policy:
    // the stack is always NX on MikuOS. Log it so a failing binary is
    // diagnosable rather than mysteriously crashing on its first trampoline
    if let Some(fl) = info.gnu_stack_flags() {
        if fl & PF_X != 0 {
            crate::serial_println!("[elf] PT_GNU_STACK requests exec stack - forcing NX");
        }
    }

    let (lo, _) = info.memory_bounds();
    let load_bias = if info.is_dyn {
        let offset = crate::kcore::random::aslr_offset(PIE_ASLR_BITS, ASLR_STEP);
        let base = PIE_BASE + offset;
        base.saturating_sub(lo & !PAGE_MASK)
    } else {
        0
    };

    crate::serial_println!(
        "[elf] {} entry={:#x} bias={:#x}",
        if info.is_dyn { "PIE" } else { "EXEC" },
        info.entry + load_bias,
        load_bias,
    );

    let mut pages: Vec<(u64, u64)> = Vec::new();

    if let Err(e) = map_all_segments(data, &info, load_bias, aspace, &mut pages) {
        return Err(fail(aspace, &pages, e));
    }

    if info.is_dyn {
        if crate::process::reloc::apply_rela_from_phys(data, &info, load_bias, aspace).is_err() {
            return Err(fail(aspace, &pages, LoadError::RelocFailed));
        }
    }

    // skip RELRO for dynamically linked binaries - ld-miku applies it after relocations
    if !info.has_interp() {
        apply_relro(&info, load_bias, aspace);
    }

    register_load_vmas(&info, load_bias, aspace.cr3);

    let mut brk: u64 = 0;
    for i in 0..info.phdr_count {
        let ph = &info.phdrs[i];
        if ph.p_type != PT_LOAD {
            continue;
        }
        let end = ph.p_vaddr
            .saturating_add(load_bias)
            .saturating_add(ph.p_memsz);
        if end > brk {
            brk = end;
        }
    }
    // Random gap between the image and the heap so a heap-relative leak
    // does not reveal image addresses (mirrors Linux brk randomization)
    let brk = (brk.saturating_add(PAGE_MASK) & !PAGE_MASK)
        .saturating_add(crate::kcore::random::aslr_offset(BRK_ASLR_BITS, ASLR_STEP));

    // Stack placement first: the TLS block hangs a fixed gap below it, so
    // both inherit the same per-exec randomization
    let stack_top_va = USER_STACK_TOP
        - crate::kcore::random::aslr_offset(STACK_ASLR_BITS, ASLR_STEP);
    let stack_limit = stack_top_va - STACK_SIZE;

    let tls_base = match setup_tls(data, &info, aspace, stack_limit, &mut pages) {
        Ok(b) => b,
        Err(e) => return Err(fail(aspace, &pages, e)),
    };

    let interp = if info.has_interp() {
        match load_interpreter(&info, data, aspace, read_file, &mut pages) {
            Ok(r) => Some(r),
            Err(e) => return Err(fail(aspace, &pages, e)),
        }
    } else {
        None
    };

    let interp_base = interp.as_ref().map(|i| i.load_base).unwrap_or(0);
    let exe_entry = info.entry + load_bias;
    let jump_entry = interp.as_ref().map(|i| i.entry).unwrap_or(exe_entry);
    let has_interp = interp.is_some();

    let phdr_vaddr = if info.phdr_vaddr != 0 {
        info.phdr_vaddr.saturating_add(load_bias)
    } else {
        // No PT_PHDR - find LOAD segment that covers e_phoff
        let mut fallback = 0u64;
        let phoff = info.ehdr.e_phoff;
        for i in 0..info.phdr_count {
            let ph = &info.phdrs[i];
            // parse() already checked p_offset+p_filesz <= data.len(),
            // but use checked_add for defense in depth
            let seg_file_end = match ph.p_offset.checked_add(ph.p_filesz) {
                Some(v) => v,
                None    => continue,
            };
            if ph.p_type == PT_LOAD && phoff >= ph.p_offset && phoff < seg_file_end {
                fallback = ph.p_vaddr
                    .saturating_add(load_bias)
                    .saturating_add(phoff - ph.p_offset);
                break;
            }
        }
        fallback
    };

    // Eager stack head: only the pages argv/envp/auxv land on are mapped
    // now; everything below faults in demand-zero through the stack VMA.
    // The unregistered gap under stack_limit is the guard - touching it
    // kills the process instead of silently growing into the TLS block
    let eager_size = (STACK_EAGER_PAGES as u64) * PAGE_SIZE;
    let eager_base = stack_top_va - eager_size;
    let stack_phys = pmm::alloc_frames(STACK_EAGER_PAGES).ok_or_else(|| {
        fail(aspace, &pages, LoadError::OutOfMemory)
    })?;
    let stack_flags = PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    if !aspace.map_range(eager_base, stack_phys, eager_size, stack_flags) {
        pmm::free_frames(stack_phys, STACK_EAGER_PAGES);
        return Err(fail(aspace, &pages, LoadError::MapFailed));
    }
    for i in 0..STACK_EAGER_PAGES as u64 {
        pages.push((eager_base + i * PAGE_SIZE, stack_phys + i * PAGE_SIZE));
    }
    crate::mm::mmap::kernel_register_vma(
        aspace.cr3, stack_limit, stack_top_va,
        crate::mm::mmap::PROT_READ | crate::mm::mmap::PROT_WRITE,
    );

    let hhdm = grub::hhdm();
    unsafe {
        core::ptr::write_bytes((stack_phys + hhdm) as *mut u8, 0, eager_size as usize);
    }

    let stack_top = setup_stack(
        stack_phys, eager_base, stack_top_va, path, args, envs, &info,
        interp_base, exe_entry, phdr_vaddr,
    ).ok_or_else(|| fail(aspace, &pages, LoadError::ArgvTooLong))?;

    crate::serial_println!(
        "[elf] ready: jump={:#x} exe={:#x} sp={:#x} brk={:#x} tls={:#x} interp={:#x} stack={:#x}..{:#x}",
        jump_entry, exe_entry, stack_top, brk, tls_base, interp_base,
        stack_limit, stack_top_va,
    );

    Ok(ElfImage {
        entry: jump_entry, exe_entry, stack_top, brk, load_bias,
        tls_base, interp_base, has_interp,
    })
}

struct InterpResult {
    entry: u64,
    load_base: u64,
}

fn load_interpreter(
    info: &ElfInfo,
    data: &[u8],
    aspace: &AddressSpace,
    read_file: Option<ReadFileFn<'_>>,
    pages: &mut Vec<(u64, u64)>,
) -> Result<InterpResult, LoadError> {
    let path = info.interp_path(data).unwrap_or("/lib/ld-miku.so");
    let read_fn = read_file.ok_or(LoadError::InterpReadFailed)?;
    let idata = read_fn(path).ok_or(LoadError::InterpReadFailed)?;

    crate::serial_println!("[elf] interp: {} ({} bytes)", path, idata.len());

    if idata.len() > MAX_ELF_SIZE {
        return Err(LoadError::InterpLoadFailed);
    }

    let mut iinfo = parse(&idata).map_err(|_| LoadError::InterpLoadFailed)?;
    check_overlaps(&iinfo).map_err(|_| LoadError::InterpLoadFailed)?;

    let (ilo, _) = iinfo.memory_bounds();
    let interp_target = INTERP_BASE
        + crate::kcore::random::aslr_offset(INTERP_ASLR_BITS, ASLR_STEP);
    let ibias = if ilo >= interp_target {
        0
    } else {
        interp_target.saturating_sub(ilo & !PAGE_MASK)
    };
    iinfo.load_bias = ibias;

    // Mapped pages go into the caller's shared list: a failure below (or in
    // any later load() stage) is unwound in one place by fail()
    map_all_segments(&idata, &iinfo, ibias, aspace, pages)
        .map_err(|_| LoadError::InterpLoadFailed)?;

    if iinfo.is_dyn {
        if crate::process::reloc::apply_rela_from_phys(&idata, &iinfo, ibias, aspace).is_err() {
            return Err(LoadError::InterpLoadFailed);
        }
    }

    if crate::process::reloc::apply_rela_from_sections(&idata, ibias, aspace).is_err() {
        return Err(LoadError::InterpLoadFailed);
    }

    apply_relro(&iinfo, ibias, aspace);
    register_load_vmas(&iinfo, ibias, aspace.cr3);

    crate::serial_println!(
        "[elf] interp: entry={:#x} base={:#x}",
        iinfo.entry + ibias, ibias,
    );
    let (ilo, _) = iinfo.memory_bounds();
    let actual_base = (ilo & !PAGE_MASK) + ibias;
    Ok(InterpResult { entry: iinfo.entry + ibias, load_base: actual_base })
}

fn check_overlaps(info: &ElfInfo) -> Result<(), LoadError> {
    let mut ranges: Vec<(u64, u64)> = Vec::with_capacity(info.phdr_count);

    for i in 0..info.phdr_count {
        let p = &info.phdrs[i];
        if p.p_type != PT_LOAD || p.p_memsz == 0 {
            continue;
        }

        if p.p_flags & PF_W != 0 && p.p_flags & PF_X != 0 {
            let vaddr = p.p_vaddr;
            crate::serial_println!("[elf] W+X segment rejected at {:#x}", vaddr);
            return Err(LoadError::WxSegment);
        }

        let start = p.p_vaddr;
        // parse() already rejects vaddr+memsz wrap, but be defensive
        let end = match p.p_vaddr.checked_add(p.p_memsz) {
            Some(v) => v,
            None    => return Err(LoadError::SegmentOverlap),
        };
        for &(rs, re) in ranges.iter() {
            if start < re && end > rs {
                return Err(LoadError::SegmentOverlap);
            }
        }
        ranges.push((start, end));
    }
    Ok(())
}

fn map_all_segments(
    data: &[u8],
    info: &ElfInfo,
    load_bias: u64,
    aspace: &AddressSpace,
    pages: &mut Vec<(u64, u64)>,
) -> Result<(), LoadError> {
    for i in 0..info.phdr_count {
        let ph = &info.phdrs[i];
        if ph.p_type != PT_LOAD {
            continue;
        }
        map_load_segment(data, ph, load_bias, aspace, pages)?;
    }
    Ok(())
}

/// Register every PT_LOAD range in the VMA table so mmap's find_free never
/// hands out an overlapping window and /proc-style tooling can see the
/// image. Page-rounded neighbours may touch, so ranges are merged first
fn register_load_vmas(info: &ElfInfo, load_bias: u64, cr3: u64) {
    let mut ranges: Vec<(u64, u64, u32)> = Vec::new();
    for i in 0..info.phdr_count {
        let ph = &info.phdrs[i];
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        let va  = match ph.p_vaddr.checked_add(load_bias) { Some(v) => v, None => continue };
        let end = match va.checked_add(ph.p_memsz)        { Some(v) => v, None => continue };
        let start = va & !PAGE_MASK;
        let end   = end.saturating_add(PAGE_MASK) & !PAGE_MASK;
        let mut prot = 0u32;
        if ph.p_flags & PF_R != 0 { prot |= crate::mm::mmap::PROT_READ; }
        if ph.p_flags & PF_W != 0 { prot |= crate::mm::mmap::PROT_WRITE; }
        if ph.p_flags & PF_X != 0 { prot |= crate::mm::mmap::PROT_EXEC; }
        ranges.push((start, end, prot));
    }
    ranges.sort_unstable_by_key(|r| r.0);

    let mut merged: Vec<(u64, u64, u32)> = Vec::new();
    for r in ranges {
        if let Some(last) = merged.last_mut() {
            if r.0 <= last.1 {
                if r.1 > last.1 { last.1 = r.1; }
                last.2 |= r.2;
                continue;
            }
        }
        merged.push(r);
    }
    for (s, e, p) in merged {
        crate::mm::mmap::kernel_register_vma(cr3, s, e, p);
    }
}

fn map_load_segment(
    data: &[u8],
    phdr: &Elf64Phdr,
    load_bias: u64,
    aspace: &AddressSpace,
    pages: &mut Vec<(u64, u64)>,
) -> Result<(), LoadError> {
    if phdr.p_memsz == 0 {
        return Ok(());
    }

    // All arithmetic on the attacker-controlled phdr fields goes through
    // checked_add so a crafted ELF cannot wrap past USER_MAX or produce
    // a negative page count
    let vaddr = phdr.p_vaddr.checked_add(load_bias).ok_or(LoadError::MapFailed)?;
    let filesz = phdr.p_filesz;
    let memsz = phdr.p_memsz;
    let offset = phdr.p_offset;

    let page_start = vaddr & !PAGE_MASK;
    let vaddr_end  = vaddr.checked_add(memsz).ok_or(LoadError::MapFailed)?;
    let page_end   = vaddr_end
        .checked_add(PAGE_MASK).ok_or(LoadError::MapFailed)? & !PAGE_MASK;
    if page_end <= page_start { return Err(LoadError::MapFailed); }
    let num_pages = ((page_end - page_start) / PAGE_SIZE) as usize;
    let seg_file_end = vaddr.checked_add(filesz).ok_or(LoadError::MapFailed)?;

    let mut flags = PageTableFlags::USER_ACCESSIBLE;
    if phdr.p_flags & PF_W != 0 {
        flags |= PageTableFlags::WRITABLE;
    }
    if phdr.p_flags & PF_X == 0 {
        flags |= PageTableFlags::NO_EXECUTE;
    }

    let hhdm = grub::hhdm();

    for i in 0..num_pages {
        let pv = page_start + (i as u64) * PAGE_SIZE;
        let frame = pmm::alloc_frame().ok_or(LoadError::OutOfMemory)?;

        let copy_vstart = pv.max(vaddr);
        let copy_vend = (pv + PAGE_SIZE).min(seg_file_end);
        let copy_len = copy_vend.saturating_sub(copy_vstart);

        // Skip the 4 KiB memset when the file copy will overwrite the whole
        // page anyway - common for .text / .rodata pages that come straight
        // from the ELF file. Partial pages (BSS tail, page-spanning gaps)
        // still need the zero fill so we never leak previous frame contents
        // back to userspace
        let full_page = copy_vstart == pv && copy_len == PAGE_SIZE;
        if !full_page {
            unsafe {
                core::ptr::write_bytes((frame + hhdm) as *mut u8, 0, PAGE_SIZE as usize);
            }
        }

        if copy_len > 0 {
            copy_segment_data(data, hhdm, frame, pv, copy_vstart, copy_vend, offset, vaddr);
        }

        if let Some(existing_phys) = aspace.virt_to_phys(pv) {
            pmm::free_frame(frame);
            if copy_vend > copy_vstart {
                copy_segment_data(
                    data, hhdm, existing_phys, pv,
                    copy_vstart, copy_vend, offset, vaddr,
                );
            }
            let merged = merge_page_flags(aspace, pv, flags)?;
            aspace.unmap_page_no_free(pv);
            if !aspace.map_page(pv, existing_phys, merged) {
                return Err(LoadError::MapFailed);
            }
        } else {
            if !aspace.map_page(pv, frame, flags) {
                pmm::free_frame(frame);
                return Err(LoadError::MapFailed);
            }
            pages.push((pv, frame));
        }
    }

    crate::serial_println!(
        "[elf]   LOAD va={:#x} pages={} {}{}{}",
        page_start, num_pages,
        if phdr.p_flags & PF_R != 0 { "R" } else { "-" },
        if phdr.p_flags & PF_W != 0 { "W" } else { "-" },
        if phdr.p_flags & PF_X != 0 { "X" } else { "-" },
    );
    Ok(())
}

fn copy_segment_data(
    data: &[u8],
    hhdm: u64,
    frame: u64,
    pv: u64,
    copy_vstart: u64,
    copy_vend: u64,
    offset: u64,
    vaddr: u64,
) {
    let dst_off = (copy_vstart - pv) as usize;
    let src_off = (offset + (copy_vstart - vaddr)) as usize;
    let copy_len = (copy_vend - copy_vstart) as usize;
    if src_off < data.len() {
        let clamped = copy_len.min(data.len() - src_off);
        if clamped > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr().add(src_off),
                    (frame + hhdm + dst_off as u64) as *mut u8,
                    clamped,
                );
            }
        }
    }
}

fn merge_page_flags(
    aspace: &AddressSpace,
    pv: u64,
    new_flags: PageTableFlags,
) -> Result<PageTableFlags, LoadError> {
    let existing_flags = aspace.get_page_flags(pv).unwrap_or(new_flags);
    let old_w = existing_flags.contains(PageTableFlags::WRITABLE);
    let old_x = !existing_flags.contains(PageTableFlags::NO_EXECUTE);
    let new_w = new_flags.contains(PageTableFlags::WRITABLE);
    let new_x = !new_flags.contains(PageTableFlags::NO_EXECUTE);

    if (old_w || new_w) && (old_x || new_x) {
        crate::serial_println!(
            "[elf] W^X: refusing shared page {:#x} (W={} X={})",
            pv, old_w || new_w, old_x || new_x,
        );
        return Err(LoadError::WxSegment);
    }

    let mut merged = existing_flags | new_flags;
    if existing_flags.contains(PageTableFlags::NO_EXECUTE)
        && new_flags.contains(PageTableFlags::NO_EXECUTE)
    {
        merged |= PageTableFlags::NO_EXECUTE;
    } else {
        merged.remove(PageTableFlags::NO_EXECUTE);
    }
    Ok(merged)
}

fn rollback(aspace: &AddressSpace, pages: &[(u64, u64)]) {
    for &(vaddr, phys) in pages {
        aspace.unmap_page_no_free(vaddr);
        pmm::free_frame(phys);
    }
}

fn apply_relro(info: &ElfInfo, load_bias: u64, aspace: &AddressSpace) {
    for i in 0..info.phdr_count {
        let ph = &info.phdrs[i];
        if ph.p_type != PT_GNU_RELRO || ph.p_memsz == 0 {
            continue;
        }

        // saturating - apply_relro is non-fatal; if a malicious phdr
        // wraps we'd rather skip RELRO than panic the kernel
        let raw_start = ph.p_vaddr.saturating_add(load_bias);
        let raw_end   = raw_start
            .saturating_add(ph.p_memsz)
            .saturating_add(PAGE_MASK);
        let start = raw_start & !PAGE_MASK;
        let end   = raw_end   & !PAGE_MASK;
        let ro_flags = PageTableFlags::PRESENT
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;

        let mut pv = start;
        while pv < end {
            if let Some(phys) = aspace.virt_to_phys(pv) {
                aspace.unmap_page_no_free(pv);
                aspace.map_page(pv, phys, ro_flags);
            }
            pv += PAGE_SIZE;
        }
        crate::serial_println!("[elf] RELRO {:#x}..{:#x} -> RO", start, end);
        break;
    }
}

/// Map the PT_TLS initialization image + TCB. The block sits a fixed gap
/// below the (randomized) stack low end, so its address changes each exec
fn setup_tls(
    data: &[u8],
    info: &ElfInfo,
    aspace: &AddressSpace,
    stack_limit: u64,
    pages: &mut Vec<(u64, u64)>,
) -> Result<u64, LoadError> {
    let ph = match (0..info.phdr_count)
        .map(|i| &info.phdrs[i])
        .find(|p| p.p_type == PT_TLS)
    {
        Some(p) => p,
        None => return Ok(0),
    };

    // Cap TLS to a sane size. 64 MB is the largest we ever
    // want to map and prevents overflow on the rounding math below
    const TLS_MAX_SIZE: usize = 64 * 1024 * 1024;
    let raw_align = ph.p_align.max(8) as usize;
    // Align must be a power of two for the bitmask rounding to work,
    // and must not be absurdly large. Fall back to 8 if either fails
    let align = if raw_align.is_power_of_two() && raw_align <= 4096 {
        raw_align
    } else {
        8
    };
    let filesz = ph.p_filesz as usize;
    let memsz  = ph.p_memsz  as usize;
    if memsz > TLS_MAX_SIZE || filesz > memsz {
        return Err(LoadError::MapFailed);
    }
    let tcb_off = match memsz.checked_add(align - 1) {
        Some(v) => v & !(align - 1),
        None    => return Err(LoadError::MapFailed),
    };
    let block_size = tcb_off.checked_add(8).ok_or(LoadError::MapFailed)?;
    let num_pages = block_size.checked_add(4095).ok_or(LoadError::MapFailed)? / 4096;
    let map_size = num_pages.checked_mul(4096).ok_or(LoadError::MapFailed)?;

    // stack_limit is page aligned and map_size is a page multiple, so the
    // block base stays aligned. The gap keeps a stack overrun that somehow
    // clears the guard from landing in TLS
    let tls_virt = stack_limit
        .checked_sub(TLS_GAP_BELOW_STACK)
        .and_then(|v| v.checked_sub(map_size as u64))
        .ok_or(LoadError::MapFailed)?;

    let phys = pmm::alloc_frames(num_pages).ok_or(LoadError::OutOfMemory)?;
    let flags = PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    if !aspace.map_range(tls_virt, phys, map_size as u64, flags) {
        pmm::free_frames(phys, num_pages);
        return Err(LoadError::MapFailed);
    }
    for i in 0..num_pages as u64 {
        pages.push((tls_virt + i * PAGE_SIZE, phys + i * PAGE_SIZE));
    }
    crate::mm::mmap::kernel_register_vma(
        aspace.cr3, tls_virt, tls_virt + map_size as u64,
        crate::mm::mmap::PROT_READ | crate::mm::mmap::PROT_WRITE,
    );

    let hhdm = grub::hhdm();
    let offset_us = ph.p_offset as usize;
    let in_bounds = filesz == 0
        || offset_us
            .checked_add(filesz)
            .map(|end| end <= data.len())
            .unwrap_or(false);
    unsafe {
        core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, map_size);
        if filesz > 0 && in_bounds {
            core::ptr::copy_nonoverlapping(
                data.as_ptr().add(offset_us),
                (phys + hhdm) as *mut u8,
                filesz,
            );
        }
        let tcb_user_va = tls_virt + tcb_off as u64;
        ((phys + hhdm + tcb_off as u64) as *mut u64).write(tcb_user_va);
    }

    let tls_base = tls_virt + tcb_off as u64;
    crate::serial_println!(
        "[elf] TLS: block={:#x} tcb(FS.base)={:#x} filesz={} memsz={}",
        tls_virt, tls_base, filesz, memsz,
    );
    Ok(tls_base)
}

/// CPUID.1:EDX - the feature word Linux hands out as AT_HWCAP on x86_64
fn hwcap() -> u64 {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("edx") edx,
            out("eax") _,
            out("ecx") _,
        );
    }
    edx as u64
}

fn setup_stack(
    eager_phys: u64,
    eager_base: u64,
    stack_top_va: u64,
    path: &str,
    args: &[&str],
    envs: &[&str],
    info: &ElfInfo,
    interp_base: u64,
    exe_entry: u64,
    phdr_vaddr: u64,
) -> Option<u64> {
    // Byte budget up front, Linux-style: no fixed argv/envp slot counts.
    // The eager stack region must hold all strings plus the pointer
    // vectors, so both are checked against explicit limits here and the
    // sticky overflow flag below backstops any miscount
    if args.len().saturating_add(envs.len()) > ARG_MAX_VECTOR_ENTRIES {
        return None;
    }
    let mut str_bytes: u64 = (path.len() as u64) + 1 + 7 + 16; // execfn + "x86_64\0" + AT_RANDOM
    for a in args { str_bytes = str_bytes.saturating_add(a.len() as u64 + 1); }
    for e in envs { str_bytes = str_bytes.saturating_add(e.len() as u64 + 1); }
    if str_bytes > ARG_MAX_STRING_BYTES {
        return None;
    }

    let hhdm = grub::hhdm();
    let virt_base = eager_base;
    let host_base = eager_phys + hhdm;
    let mut sp = stack_top_va;

    // Closures share this sticky overflow flag. Any push that wouldn't
    // fit refuses to write and sets overflow so the caller can reject
    // the whole exec with ArgvTooLong; silently truncating yields
    // attacker-controlled argv pointers, never a safe behaviour
    let overflow = core::cell::Cell::new(false);

    let push_u64 = |sp: &mut u64, val: u64| {
        if overflow.get() { return; }
        if *sp < virt_base + 8 { overflow.set(true); return; }
        *sp -= 8;
        let off = *sp - virt_base;
        unsafe {
            ((host_base + off) as *mut u64).write_unaligned(val);
        }
    };

    let push_bytes = |sp: &mut u64, bytes: &[u8]| -> u64 {
        if overflow.get() { return *sp; }
        let need = bytes.len() as u64;
        if *sp < virt_base + need { overflow.set(true); return *sp; }
        *sp -= need;
        let off = *sp - virt_base;
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (host_base + off) as *mut u8,
                bytes.len(),
            );
        }
        *sp
    };

    let push_cstr = |sp: &mut u64, s: &str| -> u64 {
        if overflow.get() { return *sp; }
        let b = s.as_bytes();
        let need = (b.len() as u64).saturating_add(1);
        if *sp < virt_base + need { overflow.set(true); return *sp; }
        *sp -= 1;
        unsafe {
            ((host_base + (*sp - virt_base)) as *mut u8).write(0);
        }
        *sp -= b.len() as u64;
        let off = *sp - virt_base;
        unsafe {
            core::ptr::copy_nonoverlapping(b.as_ptr(), (host_base + off) as *mut u8, b.len());
        }
        *sp
    };

    let random_va   = push_bytes(&mut sp, &crate::kcore::random::random_bytes_16());
    let execfn_va   = push_cstr(&mut sp, path);
    let platform_va = push_cstr(&mut sp, "x86_64");

    let argc = args.len().max(1);
    let mut argv_va: Vec<u64> = Vec::with_capacity(argc);
    for i in 0..argc {
        argv_va.push(push_cstr(&mut sp, if i < args.len() { args[i] } else { path }));
    }
    let envc = envs.len();
    let mut envp_va: Vec<u64> = Vec::with_capacity(envc);
    for e in envs {
        envp_va.push(push_cstr(&mut sp, *e));
    }

    sp &= !15;

    // In final memory order (low to high): AT_PHDR first, AT_NULL last.
    // Pushed in reverse below since the stack grows down
    let auxv: &[(u64, u64)] = &[
        (AT_PHDR,        phdr_vaddr),
        (AT_PHENT,       info.ehdr.e_phentsize as u64),
        (AT_PHNUM,       info.phdr_count as u64),
        (AT_PAGESZ,      PAGE_SIZE),
        (AT_BASE,        interp_base),
        (AT_FLAGS,       0),
        (AT_ENTRY,       exe_entry),
        (AT_UID,         0),
        (AT_EUID,        0),
        (AT_GID,         0),
        (AT_EGID,        0),
        (AT_SECURE,      0),
        (AT_RANDOM,      random_va),
        (AT_HWCAP,       hwcap()),
        (AT_HWCAP2,      0),
        // AT_CLKTCK is "clock ticks per second"; must match the LAPIC timer
        // frequency announced in MikuOS_ABI.md §3.3 (250 Hz)
        (AT_CLKTCK,      crate::arch::x86_64::apic::TIMER_HZ_DEFAULT as u64),
        (AT_PLATFORM,    platform_va),
        (AT_EXECFN,      execfn_va),
        (AT_MINSIGSTKSZ, 2048),
        (AT_NULL,        0),
    ];

    // Pre-adjust so the final rsp lands 16-byte aligned (SysV: rsp % 16 == 0
    // at process entry, [rsp] = argc). Everything below is 8-byte words:
    // auxv pairs, envp entries + NULL, argv entries + NULL, argc
    let words = (auxv.len() as u64) * 2 + (envc as u64 + 1) + (argc as u64 + 1) + 1;
    if words % 2 == 1 {
        sp -= 8;
    }

    for &(key, val) in auxv.iter().rev() {
        push_u64(&mut sp, val);
        push_u64(&mut sp, key);
    }

    push_u64(&mut sp, 0); // envp NULL terminator
    for i in (0..envc).rev() {
        push_u64(&mut sp, envp_va[i]);
    }
    push_u64(&mut sp, 0); // argv NULL terminator

    for i in (0..argc).rev() {
        push_u64(&mut sp, argv_va[i]);
    }
    push_u64(&mut sp, argc as u64);

    if overflow.get() { None } else { Some(sp) }
}
