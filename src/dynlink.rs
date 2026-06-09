extern crate alloc;

use crate::elf::*;
use crate::vmm::AddressSpace;

#[derive(Debug, Clone, Copy)]
pub enum RelocError {
    BadReloc,
    UndefinedSymbol,
    WriteError,
    UnknownType(u32),
}

pub struct DynInfo {
    pub rela_vaddr: u64,
    pub rela_size: u64,
    pub rela_ent: u64,
    pub rel_vaddr: u64,
    pub rel_size: u64,
    pub rel_ent: u64,
    pub plt_rela_vaddr: u64,
    pub plt_rela_size: u64,
    pub plt_is_rela: bool,
    pub symtab_vaddr: u64,
    pub strtab_vaddr: u64,
    pub strtab_size: u64,
    pub init_vaddr: u64,
    pub fini_vaddr: u64,
    pub gnu_hash_vaddr: u64,
    pub flags_1: u64,
    pub needed: [u32; 32],
    pub needed_count: usize,
}

impl DynInfo {
    pub fn empty() -> Self {
        Self {
            rela_vaddr: 0, rela_size: 0, rela_ent: 24,
            rel_vaddr: 0, rel_size: 0, rel_ent: 16,
            plt_rela_vaddr: 0, plt_rela_size: 0, plt_is_rela: true,
            symtab_vaddr: 0, strtab_vaddr: 0, strtab_size: 0,
            init_vaddr: 0, fini_vaddr: 0, gnu_hash_vaddr: 0,
            flags_1: 0,
            needed: [0u32; 32],
            needed_count: 0,
        }
    }
}

pub fn parse_dynamic(data: &[u8], info: &ElfInfo) -> DynInfo {
    let mut d = DynInfo::empty();
    let bias = info.load_bias;

    for i in 0..info.phdr_count {
        let ph = &info.phdrs[i];
        if ph.p_type != PT_DYNAMIC {
            continue;
        }

        let off = ph.p_offset as usize;
        if off >= data.len() { break; }
        // checked_add to avoid wrap on attacker-controlled p_filesz
        let nominal_end = match off.checked_add(ph.p_filesz as usize) {
            Some(v) => v,
            None    => break,
        };
        let end = nominal_end.min(data.len());
        let bytes = &data[off..end];
        let entry_size = core::mem::size_of::<Elf64Dyn>();
        // PT_DYNAMIC must be a whole number of Elf64Dyn entries. A
        // non-multiple p_filesz is either a truncated or hand-crafted
        // ELF - warn so the user knows downstream relocs may be wrong,
        // but proceed with the count we can safely cover.
        // Copy out of the packed struct before formatting to avoid
        // taking an unaligned reference
        let p_filesz = ph.p_filesz;
        if p_filesz as usize % entry_size != 0 {
            crate::serial_println!(
                "[dynlink] warn: PT_DYNAMIC filesz {} not a multiple of {} - trailing bytes ignored",
                p_filesz, entry_size,
            );
        }
        let count = bytes.len() / entry_size;

        for j in 0..count {
            let dyn_entry = unsafe {
                core::ptr::read_unaligned(
                    bytes.as_ptr().add(j * entry_size) as *const Elf64Dyn
                )
            };
            match dyn_entry.d_tag {
                DT_RELA => d.rela_vaddr = dyn_entry.d_val.wrapping_sub(bias),
                DT_RELASZ => d.rela_size = dyn_entry.d_val,
                DT_RELAENT => d.rela_ent = dyn_entry.d_val,
                DT_REL => d.rel_vaddr = dyn_entry.d_val.wrapping_sub(bias),
                DT_RELSZ => d.rel_size = dyn_entry.d_val,
                DT_RELENT => d.rel_ent = dyn_entry.d_val,
                DT_JMPREL => d.plt_rela_vaddr = dyn_entry.d_val.wrapping_sub(bias),
                DT_PLTRELSZ => d.plt_rela_size = dyn_entry.d_val,
                DT_PLTREL => d.plt_is_rela = dyn_entry.d_val == DT_RELA as u64,
                DT_SYMTAB => d.symtab_vaddr = dyn_entry.d_val.wrapping_sub(bias),
                DT_STRTAB => d.strtab_vaddr = dyn_entry.d_val.wrapping_sub(bias),
                DT_STRSZ => d.strtab_size = dyn_entry.d_val,
                DT_INIT => d.init_vaddr = dyn_entry.d_val,
                DT_FINI => d.fini_vaddr = dyn_entry.d_val,
                DT_GNU_HASH => d.gnu_hash_vaddr = dyn_entry.d_val,
                DT_FLAGS_1 => d.flags_1 = dyn_entry.d_val,
                DT_NEEDED => {
                    if d.needed_count < 32 {
                        d.needed[d.needed_count] = dyn_entry.d_val as u32;
                        d.needed_count += 1;
                    }
                }
                DT_NULL => break,
                _ => {}
            }
        }
        break;
    }

    d
}

pub fn apply_all_relocations(
    data: &[u8],
    load_bias: u64,
    aspace: &AddressSpace,
    dyn_info: &DynInfo,
) -> Result<(), RelocError> {
    if dyn_info.rela_size > 0 && dyn_info.rela_vaddr < data.len() as u64 {
        crate::reloc::apply_rela_mapped(
            data,
            dyn_info.rela_vaddr as usize,
            dyn_info.rela_size,
            load_bias,
            aspace,
            dyn_info,
        ).map_err(|_| RelocError::WriteError)?;
    }

    if dyn_info.rel_size > 0 && dyn_info.rel_vaddr < data.len() as u64 {
        crate::reloc::apply_rel_mapped(
            data,
            dyn_info.rel_vaddr as usize,
            dyn_info.rel_size,
            load_bias,
            aspace,
            dyn_info,
        ).map_err(|_| RelocError::WriteError)?;
    }

    if dyn_info.plt_rela_size > 0 && dyn_info.plt_rela_vaddr < data.len() as u64 {
        if dyn_info.plt_is_rela {
            crate::reloc::apply_rela_mapped(
                data,
                dyn_info.plt_rela_vaddr as usize,
                dyn_info.plt_rela_size,
                load_bias,
                aspace,
                dyn_info,
            ).map_err(|_| RelocError::WriteError)?;
        } else {
            crate::reloc::apply_rel_mapped(
                data,
                dyn_info.plt_rela_vaddr as usize,
                dyn_info.plt_rela_size,
                load_bias,
                aspace,
                dyn_info,
            ).map_err(|_| RelocError::WriteError)?;
        }
    }

    Ok(())
}

pub fn get_needed_names<'a>(data: &'a [u8], dyn_info: &DynInfo) -> [Option<&'a str>; 32] {
    let mut result = [None; 32];
    if dyn_info.strtab_vaddr == 0 || dyn_info.strtab_size == 0 {
        return result;
    }

    let strtab_off = dyn_info.strtab_vaddr as usize;
    if strtab_off >= data.len() {
        return result;
    }
    // checked_add - strtab_size is from PT_DYNAMIC, attacker-controlled
    let nominal_end = match strtab_off.checked_add(dyn_info.strtab_size as usize) {
        Some(v) => v,
        None    => return result,
    };
    let strtab_end = nominal_end.min(data.len());
    let strtab = &data[strtab_off..strtab_end];

    for i in 0..dyn_info.needed_count {
        let str_off = dyn_info.needed[i] as usize;
        if str_off >= strtab.len() {
            continue;
        }
        let nul = strtab[str_off..].iter().position(|&b| b == 0)
            .unwrap_or(strtab.len() - str_off);
        result[i] = core::str::from_utf8(&strtab[str_off..str_off + nul]).ok();
    }

    result
}
