extern crate alloc;
use alloc::vec::Vec;
use crate::elf::*;
use crate::vmm::AddressSpace;
use crate::pmm;
use crate::grub;
use x86_64::structures::paging::PageTableFlags;

const PAGE_SIZE: u64 = 4096;
const PAGE_MASK: u64 = PAGE_SIZE - 1;
const STACK_PAGES: usize = 256; // 1 mb user stack
const TLS_VIRT: u64 = 0x0000_0000_4100_0000;
const PIE_BASE: u64 = 0x0000_4000_0000;
const INTERP_BASE: u64 = 0x0000_7F00_0000_0000;
const ASLR_BITS: u32 = 20;
const ASLR_STEP: u64 = 0x1000;
const MAX_ARGS: usize = 64;

pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
pub const MAX_ELF_SIZE: usize = 64 * 1024 * 1024;

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

pub fn load(
    data: &[u8],
    aspace: &AddressSpace,
    args: &[&str],
    read_file: Option<ReadFileFn<'_>>,
) -> Result<ElfImage, LoadError> {
    if data.len() > MAX_ELF_SIZE {
        return Err(LoadError::FileTooLarge);
    }

    let info = parse(data).map_err(LoadError::Parse)?;
    check_overlaps(&info)?;

    let (lo, _) = info.memory_bounds();
    let load_bias = if info.is_dyn {
        let offset = crate::random::aslr_offset(ASLR_BITS, ASLR_STEP);
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
        rollback(aspace, &pages);
        return Err(e);
    }

    if info.is_dyn {
        if crate::reloc::apply_rela_from_phys(data, &info, load_bias, aspace).is_err() {
            rollback(aspace, &pages);
            return Err(LoadError::RelocFailed);
        }
    }

    // skip RELRO for dynamically linked binaries - ld-miku applies it after relocations
    if !info.has_interp() {
        apply_relro(&info, load_bias, aspace);
    }

    let mut brk: u64 = 0;
    for i in 0..info.phdr_count {
        let ph = &info.phdrs[i];
        if ph.p_type != PT_LOAD {
            continue;
        }
        let end = ph.p_vaddr + load_bias + ph.p_memsz;
        if end > brk {
            brk = end;
        }
    }
    let brk = (brk + PAGE_MASK) & !PAGE_MASK;

    let tls_base = match setup_tls(data, &info, aspace) {
        Ok(b) => b,
        Err(e) => {
            rollback(aspace, &pages);
            return Err(e);
        }
    };

    let interp = if info.has_interp() {
        match load_interpreter(&info, data, aspace, read_file) {
            Ok(r) => Some(r),
            Err(e) => {
                rollback(aspace, &pages);
                return Err(e);
            }
        }
    } else {
        None
    };

    let interp_base = interp.as_ref().map(|i| i.load_base).unwrap_or(0);
    let exe_entry = info.entry + load_bias;
    let jump_entry = interp.as_ref().map(|i| i.entry).unwrap_or(exe_entry);
    let has_interp = interp.is_some();

    let phdr_vaddr = if info.phdr_vaddr != 0 {
        info.phdr_vaddr + load_bias
    } else {
        // No PT_PHDR - find LOAD segment that covers e_phoff
        let mut fallback = 0u64;
        let phoff = info.ehdr.e_phoff;
        for i in 0..info.phdr_count {
            let ph = &info.phdrs[i];
            if ph.p_type == PT_LOAD && phoff >= ph.p_offset
                && phoff < ph.p_offset + ph.p_filesz
            {
                fallback = ph.p_vaddr + load_bias + (phoff - ph.p_offset);
                break;
            }
        }
        fallback
    };

    let stack_phys = pmm::alloc_frames(STACK_PAGES).ok_or(LoadError::OutOfMemory)?;
    let stack_size = (STACK_PAGES as u64) * PAGE_SIZE;
    let stack_base = USER_STACK_TOP - stack_size;
    let stack_flags = PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    if !aspace.map_range(stack_base, stack_phys, stack_size, stack_flags) {
        pmm::free_frames(stack_phys, STACK_PAGES);
        rollback(aspace, &pages);
        return Err(LoadError::MapFailed);
    }

    let hhdm = grub::hhdm();
    unsafe {
        core::ptr::write_bytes((stack_phys + hhdm) as *mut u8, 0, stack_size as usize);
    }

    let stack_top = setup_stack(
        stack_phys, stack_size, args, &info,
        load_bias, interp_base, exe_entry, phdr_vaddr,
    );

    crate::serial_println!(
        "[elf] ready: jump={:#x} exe={:#x} sp={:#x} brk={:#x} tls={:#x} interp={:#x}",
        jump_entry, exe_entry, stack_top, brk, tls_base, interp_base,
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
    let ibias = if ilo >= INTERP_BASE {
        0
    } else {
        INTERP_BASE.saturating_sub(ilo & !PAGE_MASK)
    };
    iinfo.load_bias = ibias;

    let mut ipages: Vec<(u64, u64)> = Vec::new();

    if let Err(_) = map_all_segments(&idata, &iinfo, ibias, aspace, &mut ipages) {
        rollback(aspace, &ipages);
        return Err(LoadError::InterpLoadFailed);
    }

    if iinfo.is_dyn {
        if crate::reloc::apply_rela_from_phys(&idata, &iinfo, ibias, aspace).is_err() {
            rollback(aspace, &ipages);
            return Err(LoadError::InterpLoadFailed);
        }
    }

    if crate::reloc::apply_rela_from_sections(&idata, ibias, aspace).is_err() {
        rollback(aspace, &ipages);
        return Err(LoadError::InterpLoadFailed);
    }

    apply_relro(&iinfo, ibias, aspace);

    crate::serial_println!(
        "[elf] interp: entry={:#x} base={:#x}",
        iinfo.entry + ibias, ibias,
    );
    let (ilo, _) = iinfo.memory_bounds();
    let actual_base = (ilo & !PAGE_MASK) + ibias;
    Ok(InterpResult { entry: iinfo.entry + ibias, load_base: actual_base })
}

fn check_overlaps(info: &ElfInfo) -> Result<(), LoadError> {
    let mut ranges: [(u64, u64); MAX_PHDRS] = [(0, 0); MAX_PHDRS];
    let mut count = 0usize;

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
        let end = p.p_vaddr + p.p_memsz;
        for j in 0..count {
            let (rs, re) = ranges[j];
            if start < re && end > rs {
                return Err(LoadError::SegmentOverlap);
            }
        }
        ranges[count] = (start, end);
        count += 1;
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

    let vaddr = phdr.p_vaddr + load_bias;
    let filesz = phdr.p_filesz;
    let memsz = phdr.p_memsz;
    let offset = phdr.p_offset;

    let page_start = vaddr & !PAGE_MASK;
    let page_end = (vaddr + memsz + PAGE_MASK) & !PAGE_MASK;
    let num_pages = ((page_end - page_start) / PAGE_SIZE) as usize;
    let seg_file_end = vaddr + filesz;

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

        let start = (ph.p_vaddr + load_bias) & !PAGE_MASK;
        let end = (ph.p_vaddr + load_bias + ph.p_memsz + PAGE_MASK) & !PAGE_MASK;
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

fn setup_tls(data: &[u8], info: &ElfInfo, aspace: &AddressSpace) -> Result<u64, LoadError> {
    let ph = match (0..info.phdr_count)
        .map(|i| &info.phdrs[i])
        .find(|p| p.p_type == PT_TLS)
    {
        Some(p) => p,
        None => return Ok(0),
    };

    let align = ph.p_align.max(8) as usize;
    let filesz = ph.p_filesz as usize;
    let memsz = ph.p_memsz as usize;
    let tcb_off = (memsz + align - 1) & !(align - 1);
    let block_size = tcb_off + 8;
    let pages = (block_size + 4095) / 4096;
    let map_size = pages * 4096;

    let phys = pmm::alloc_frames(pages).ok_or(LoadError::OutOfMemory)?;
    let flags = PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    if !aspace.map_range(TLS_VIRT, phys, map_size as u64, flags) {
        pmm::free_frames(phys, pages);
        return Err(LoadError::MapFailed);
    }

    let hhdm = grub::hhdm();
    unsafe {
        core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, map_size);
        if filesz > 0 && ph.p_offset as usize + filesz <= data.len() {
            core::ptr::copy_nonoverlapping(
                data.as_ptr().add(ph.p_offset as usize),
                (phys + hhdm) as *mut u8,
                filesz,
            );
        }
        let tcb_user_va = TLS_VIRT + tcb_off as u64;
        ((phys + hhdm + tcb_off as u64) as *mut u64).write(tcb_user_va);
    }

    let tls_base = TLS_VIRT + tcb_off as u64;
    crate::serial_println!(
        "[elf] TLS: block={:#x} tcb(FS.base)={:#x} filesz={} memsz={}",
        TLS_VIRT, tls_base, filesz, memsz,
    );
    Ok(tls_base)
}

fn setup_stack(
    stack_phys: u64,
    stack_size: u64,
    args: &[&str],
    info: &ElfInfo,
    load_bias: u64,
    interp_base: u64,
    exe_entry: u64,
    phdr_vaddr: u64,
) -> u64 {
    let hhdm = grub::hhdm();
    let virt_base = USER_STACK_TOP - stack_size;
    let host_base = stack_phys + hhdm;
    let mut sp = USER_STACK_TOP;

    let push_u64 = |sp: &mut u64, val: u64| {
        *sp -= 8;
        let off = *sp - virt_base;
        unsafe {
            ((host_base + off) as *mut u64).write(val);
        }
    };

    let push_bytes = |sp: &mut u64, bytes: &[u8]| -> u64 {
        *sp -= bytes.len() as u64;
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
        *sp -= 1;
        unsafe {
            ((host_base + (*sp - virt_base)) as *mut u8).write(0);
        }
        let b = s.as_bytes();
        *sp -= b.len() as u64;
        let off = *sp - virt_base;
        unsafe {
            core::ptr::copy_nonoverlapping(b.as_ptr(), (host_base + off) as *mut u8, b.len());
        }
        *sp
    };

    let random_va = push_bytes(&mut sp, &crate::random::random_bytes_16());

    let argc = args.len().min(MAX_ARGS).max(1);
    let mut argv_va = [0u64; MAX_ARGS];
    for i in 0..argc {
        argv_va[i] = push_cstr(&mut sp, if i < args.len() { args[i] } else { "" });
    }
    let execfn_va = argv_va[0];

    sp &= !15;

    // Pre-adjust for 16-byte alignment after all pushes
    // Auxv pairs 34 entries (even) NULL's: 2 (even)
    // argc + 1 (argc word) determines parity, Pad if argc is even
    if argc % 2 == 0 {
        sp -= 8;
    }

    let push_auxv = |sp: &mut u64, key: u64, val: u64| {
        push_u64(sp, val);
        push_u64(sp, key);
    };

    push_auxv(&mut sp, AT_NULL, 0);
    push_auxv(&mut sp, AT_CLKTCK, 100);
    push_auxv(&mut sp, AT_HWCAP, 0);
    push_auxv(&mut sp, AT_RANDOM, random_va);
    push_auxv(&mut sp, AT_EXECFN, execfn_va);
    push_auxv(&mut sp, AT_SECURE, 0);
    push_auxv(&mut sp, AT_EGID, 0);
    push_auxv(&mut sp, AT_GID, 0);
    push_auxv(&mut sp, AT_EUID, 0);
    push_auxv(&mut sp, AT_UID, 0);
    push_auxv(&mut sp, AT_FLAGS, 0);
    push_auxv(&mut sp, AT_BASE, interp_base);
    push_auxv(&mut sp, AT_ENTRY, exe_entry);
    push_auxv(&mut sp, AT_PAGESZ, PAGE_SIZE);
    push_auxv(&mut sp, AT_PHNUM, info.phdr_count as u64);
    push_auxv(&mut sp, AT_PHENT, info.ehdr.e_phentsize as u64);
    push_auxv(&mut sp, AT_PHDR, phdr_vaddr);

    push_u64(&mut sp, 0); // envp NULL terminator
    push_u64(&mut sp, 0); // argv NULL terminator

    for i in (0..argc).rev() {
        push_u64(&mut sp, argv_va[i]);
    }
    push_u64(&mut sp, argc as u64);

    sp
}
