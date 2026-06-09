use crate::elf::*;
use crate::vmm::AddressSpace;
use crate::grub;

#[derive(Debug, Clone, Copy)]
pub enum RelocError {
    BadDynamic,
    WriteError,
    UnknownType(u32),
}

fn hhdm_write64(aspace: &AddressSpace, uva: u64, val: u64) -> bool {
    let hhdm = grub::hhdm();
    let page = uva & !0xFFF;
    let off = (uva & 0xFFF) as usize;
    if off + 8 > 4096 {
        return false;
    }
    match aspace.virt_to_phys(page) {
        Some(phys) => {
            unsafe { ((phys + hhdm + off as u64) as *mut u64).write_unaligned(val); }
            true
        }
        None => false,
    }
}

fn hhdm_read64(aspace: &AddressSpace, uva: u64) -> Option<u64> {
    let hhdm = grub::hhdm();
    let page = uva & !0xFFF;
    let off = uva & 0xFFF;
    if off + 8 > 4096 {
        return None;
    }
    let phys = aspace.virt_to_phys(page)?;
    Some(unsafe { ((phys + hhdm + off) as *const u64).read_unaligned() })
}

pub fn apply_rela_from_phys(
    data:      &[u8],
    info:      &ElfInfo,
    load_bias: u64,
    aspace:    &AddressSpace,
) -> Result<usize, RelocError> {
    let (dyn_off, dyn_sz) = match info.dynamic_offset {
        Some(p) => (p.0 as usize, p.1 as usize),
        None => return Ok(0),
    };
    let dyn_end = dyn_off.checked_add(dyn_sz).ok_or(RelocError::BadDynamic)?;
    if dyn_end > data.len() {
        return Err(RelocError::BadDynamic);
    }

    let mut rela_va: u64 = 0;
    let mut rela_size: u64 = 0;
    let mut jmprel_va: u64 = 0;
    let mut jmprel_sz: u64 = 0;
    let mut symtab_va: u64 = 0;
    let mut syment: u64 = 24;
    let mut cur = dyn_off;

    while cur + 16 <= dyn_end {
        let tag = read_i64_le(data, cur).ok_or(RelocError::BadDynamic)?;
        let val = read_u64_le(data, cur + 8).ok_or(RelocError::BadDynamic)?;
        match tag {
            DT_RELA => rela_va = val,
            DT_RELASZ => rela_size = val,
            DT_JMPREL => jmprel_va = val,
            DT_PLTRELSZ => jmprel_sz = val,
            DT_SYMTAB => symtab_va = val,
            DT_SYMENT => syment = val,
            DT_NULL => break,
            _ => {}
        }
        cur += 16;
    }

    let mut total = 0usize;

    if rela_size > 0 && rela_va != 0 {
        total += apply_rela_entries_phys(
            data, info, load_bias, aspace,
            rela_va, rela_size, symtab_va, syment,
        )?;
    }

    if jmprel_sz > 0 && jmprel_va != 0 {
        total += apply_rela_entries_phys(
            data, info, load_bias, aspace,
            jmprel_va, jmprel_sz, symtab_va, syment,
        )?;
    }

    Ok(total)
}

fn apply_rela_entries_phys(
    data:      &[u8],
    info:      &ElfInfo,
    load_bias: u64,
    aspace:    &AddressSpace,
    rela_va:   u64,
    rela_size: u64,
    symtab_va: u64,
    syment:    u64,
) -> Result<usize, RelocError> {
    let count = (rela_size as usize) / 24;
    let mut applied = 0usize;

    for i in 0..count {
        let entry_va = match (i as u64).checked_mul(24).and_then(|x| rela_va.checked_add(x)) {
            Some(v) => v,
            None => continue,
        };
        let foff = match va_to_file_offset(info, entry_va) {
            Some(o) => o,
            None => continue,
        };

        let r_offset = match read_u64_le(data, foff) { Some(v) => v, None => continue };
        let off_info = match foff.checked_add(8)   { Some(v) => v, None => continue };
        let r_info = match read_u64_le(data, off_info) { Some(v) => v, None => continue };
        let off_addend = match foff.checked_add(16) { Some(v) => v, None => continue };
        let r_addend = match read_i64_le(data, off_addend) { Some(v) => v, None => continue };

        let rtype = r_info as u32;
        let rsym = (r_info >> 32) as u32;
        let target_va = match r_offset.checked_add(load_bias) {
            Some(v) => v,
            None    => continue,
        };

        match rtype {
            R_X86_64_RELATIVE => {
                let value = (load_bias as i64).wrapping_add(r_addend) as u64;
                if !hhdm_write64(aspace, target_va, value) {
                    continue;
                }
                applied += 1;
            }
            R_X86_64_JUMP_SLOT | R_X86_64_GLOB_DAT => {
                if symtab_va == 0 || rsym == 0 {
                    continue;
                }
                if let Some(value) = resolve_sym_from_file(data, info, symtab_va, syment, rsym, load_bias) {
                    if hhdm_write64(aspace, target_va, value) {
                        applied += 1;
                    }
                }
            }
            R_X86_64_64 => {
                let sym_val = if symtab_va != 0 && rsym != 0 {
                    resolve_sym_from_file(data, info, symtab_va, syment, rsym, load_bias)
                        .unwrap_or(0)
                } else {
                    0
                };
                let value = (sym_val as i64).wrapping_add(r_addend) as u64;
                if hhdm_write64(aspace, target_va, value) {
                    applied += 1;
                }
            }
            R_X86_64_IRELATIVE | R_X86_64_COPY | R_X86_64_NONE => {}
            _ => {}
        }
    }

    crate::serial_println!("[reloc] phys: entries={} applied={}", count, applied);
    Ok(applied)
}

fn resolve_sym_from_file(
    data:      &[u8],
    info:      &ElfInfo,
    symtab_va: u64,
    syment:    u64,
    rsym:      u32,
    load_bias: u64,
) -> Option<u64> {
    let sym_file_va = (rsym as u64)
        .checked_mul(syment)
        .and_then(|x| symtab_va.checked_add(x))?;
    let soff = va_to_file_offset(info, sym_file_va)?;
    let soff_end = soff.checked_add(syment as usize)?;
    if soff_end > data.len() {
        return None;
    }
    let off_shndx = soff.checked_add(6)?;
    let off_value = soff.checked_add(8)?;
    let st_shndx = read_u16_le(data, off_shndx)?;
    let st_value = read_u64_le(data, off_value)?;
    if st_shndx == 0 || st_value == 0 {
        return None;
    }
    st_value.checked_add(load_bias)
}

pub fn apply_rela_from_sections(
    data:      &[u8],
    load_bias: u64,
    aspace:    &AddressSpace,
) -> Result<usize, RelocError> {
    if data.len() < 64 {
        return Ok(0);
    }

    let e_shoff = match read_u64_le(data, 40) { Some(v) => v, None => return Ok(0) };
    let e_shentsize = match read_u16_le(data, 58) { Some(v) => v as usize, None => return Ok(0) };
    let e_shnum = match read_u16_le(data, 60) { Some(v) => v as usize, None => return Ok(0) };

    if e_shoff == 0 || e_shnum == 0 || e_shentsize < 64 {
        return Ok(0);
    }

    let mut total_applied = 0usize;

    for i in 0..e_shnum {
        let sh = match i.checked_mul(e_shentsize)
            .and_then(|x| (e_shoff as usize).checked_add(x))
        {
            Some(v) => v,
            None => break,
        };
        let sh_end = match sh.checked_add(e_shentsize) {
            Some(v) => v,
            None => break,
        };
        if sh_end > data.len() {
            break;
        }

        let sh_type_off = sh.checked_add(4).unwrap_or(usize::MAX);
        let sh_type = match read_u32_le(data, sh_type_off) { Some(v) => v, None => continue };
        if sh_type != SHT_RELA {
            continue;
        }

        let sh_off_off  = sh.checked_add(24).unwrap_or(usize::MAX);
        let sh_size_off = sh.checked_add(32).unwrap_or(usize::MAX);
        let sh_offset = match read_u64_le(data, sh_off_off)  { Some(v) => v as usize, None => continue };
        let sh_size   = match read_u64_le(data, sh_size_off) { Some(v) => v as usize, None => continue };

        let count = sh_size / 24;
        for j in 0..count {
            let off = match j.checked_mul(24).and_then(|x| sh_offset.checked_add(x)) {
                Some(v) => v,
                None => break,
            };
            let off_end = match off.checked_add(24) {
                Some(v) => v,
                None => break,
            };
            if off_end > data.len() {
                break;
            }

            let off_info   = off.checked_add(8).unwrap_or(usize::MAX);
            let off_addend = off.checked_add(16).unwrap_or(usize::MAX);
            let r_offset = match read_u64_le(data, off)        { Some(v) => v, None => continue };
            let r_info   = match read_u64_le(data, off_info)   { Some(v) => v, None => continue };
            let r_addend = match read_i64_le(data, off_addend) { Some(v) => v, None => continue };

            let rtype = r_info as u32;
            if rtype != R_X86_64_RELATIVE {
                continue;
            }

            let target_va = match r_offset.checked_add(load_bias) {
                Some(v) => v,
                None => continue,
            };
            let value = (load_bias as i64).wrapping_add(r_addend) as u64;
            if hhdm_write64(aspace, target_va, value) {
                total_applied += 1;
            }
        }
    }

    Ok(total_applied)
}

pub fn apply_rela_mapped(
    data:      &[u8],
    rela_off:  usize,
    rela_size: u64,
    load_bias: u64,
    aspace:    &AddressSpace,
    dyn_info:  &crate::dynlink::DynInfo,
) -> Result<usize, RelocError> {
    let entry_size = core::mem::size_of::<Elf64Rela>();
    let count = rela_size as usize / entry_size;
    let mut applied = 0usize;

    for i in 0..count {
        let off = match i.checked_mul(entry_size).and_then(|x| rela_off.checked_add(x)) {
            Some(v) => v,
            None => break,
        };
        let off_end = match off.checked_add(entry_size) {
            Some(v) => v,
            None => break,
        };
        if off_end > data.len() {
            break;
        }

        let rela = unsafe {
            core::ptr::read_unaligned(data.as_ptr().add(off) as *const Elf64Rela)
        };

        let target_uva = match rela.r_offset.checked_add(load_bias) {
            Some(v) => v,
            None    => continue,
        };
        let rtype = rela.rtype();
        let sym_idx = rela.sym();
        let addend = rela.r_addend;

        match rtype {
            R_X86_64_NONE => {}
            R_X86_64_RELATIVE => {
                let val = (load_bias as i64 + addend) as u64;
                if !hhdm_write64(aspace, target_uva, val) {
                    return Err(RelocError::WriteError);
                }
                applied += 1;
            }
            R_X86_64_64 => {
                let base = sym_value_mapped(data, sym_idx, load_bias, dyn_info)
                    .unwrap_or(0);
                let val = (base as i64 + addend) as u64;
                if !hhdm_write64(aspace, target_uva, val) {
                    return Err(RelocError::WriteError);
                }
                applied += 1;
            }
            R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                if let Some(sv) = sym_value_mapped(data, sym_idx, load_bias, dyn_info) {
                    if !hhdm_write64(aspace, target_uva, sv) {
                        return Err(RelocError::WriteError);
                    }
                    applied += 1;
                }
            }
            R_X86_64_IRELATIVE | R_X86_64_COPY => {}
            _ => {}
        }
    }

    Ok(applied)
}

pub fn apply_rel_mapped(
    data:      &[u8],
    rel_off:   usize,
    rel_size:  u64,
    load_bias: u64,
    aspace:    &AddressSpace,
    dyn_info:  &crate::dynlink::DynInfo,
) -> Result<usize, RelocError> {
    let entry_size = core::mem::size_of::<Elf64Rel>();
    let count = rel_size as usize / entry_size;
    let mut applied = 0usize;

    for i in 0..count {
        let off = match i.checked_mul(entry_size).and_then(|x| rel_off.checked_add(x)) {
            Some(v) => v,
            None => break,
        };
        let off_end = match off.checked_add(entry_size) {
            Some(v) => v,
            None => break,
        };
        if off_end > data.len() {
            break;
        }

        let rel = unsafe {
            core::ptr::read_unaligned(data.as_ptr().add(off) as *const Elf64Rel)
        };

        let target_uva = match rel.r_offset.checked_add(load_bias) {
            Some(v) => v,
            None    => continue,
        };
        let rtype = rel.rtype();
        let sym_idx = rel.sym();

        match rtype {
            R_X86_64_NONE => {}
            R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                if let Some(sv) = sym_value_mapped(data, sym_idx, load_bias, dyn_info) {
                    if hhdm_write64(aspace, target_uva, sv) {
                        applied += 1;
                    }
                }
            }
            R_X86_64_RELATIVE => {
                let addend = hhdm_read64(aspace, target_uva).unwrap_or(0);
                let val = load_bias.wrapping_add(addend);
                if hhdm_write64(aspace, target_uva, val) {
                    applied += 1;
                }
            }
            _ => {}
        }
    }

    Ok(applied)
}

fn sym_value_mapped(
    data:      &[u8],
    sym_idx:   u32,
    load_bias: u64,
    dyn_info:  &crate::dynlink::DynInfo,
) -> Option<u64> {
    let sym_size = core::mem::size_of::<Elf64Sym>();
    if dyn_info.symtab_vaddr == 0 {
        return None;
    }

    let sym_off = dyn_info.symtab_vaddr as usize + sym_idx as usize * sym_size;
    if sym_off + sym_size > data.len() {
        return None;
    }

    let sym = unsafe {
        core::ptr::read_unaligned(data.as_ptr().add(sym_off) as *const Elf64Sym)
    };

    if sym.st_value == 0 {
        return None;
    }
    Some(sym.st_value + load_bias)
}

fn va_to_file_offset(info: &ElfInfo, va: u64) -> Option<usize> {
    for i in 0..info.phdr_count {
        let ph = &info.phdrs[i];
        if ph.p_type != PT_LOAD {
            continue;
        }
        let seg_end = ph.p_vaddr.checked_add(ph.p_filesz)?;
        if va >= ph.p_vaddr && va < seg_end {
            let file_off = ph.p_offset.checked_add(va - ph.p_vaddr)?;
            return Some(file_off as usize);
        }
    }
    None
}

fn read_u64_le(data: &[u8], off: usize) -> Option<u64> {
    let end = off.checked_add(8)?;
    if end > data.len() { return None; }
    Some(u64::from_le_bytes(data[off..end].try_into().ok()?))
}

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    if end > data.len() { return None; }
    Some(u32::from_le_bytes(data[off..end].try_into().ok()?))
}

fn read_u16_le(data: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    if end > data.len() { return None; }
    Some(u16::from_le_bytes(data[off..end].try_into().ok()?))
}

fn read_i64_le(data: &[u8], off: usize) -> Option<i64> {
    read_u64_le(data, off).map(|v| v as i64)
}
