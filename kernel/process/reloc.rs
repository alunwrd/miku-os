// Applying an ELF's relocations means writing 8-byte words at addresses the
// file itself names, the most dangerous thing this kernel does with untrusted
// input. The rules are stated once, and every writer in this file goes through
// the same two functions:
//
//   * the target must be a canonical user address (mm::vmm::is_user_range),
//   * it must fall inside the image being relocated,
//   * the page must already be mapped PRESENT and USER_ACCESSIBLE, so the
//     write lands on a page belonging to userspace,
//   * and it must not straddle a page boundary.
//
// The writes go through the HHDM alias of the frame rather than the user
// mapping, which makes them possible before the process runs, and makes the
// checks above all that stands between a crafted r_offset and the kernel's own
// memory.

use crate::process::elf::*;
use crate::process::elf_source::{ElfSource, Window};
use crate::mm::vmm::AddressSpace;
use crate::arch::x86_64::grub;

/// Ceiling on how many relocation entries one image may ask for. DT_RELASZ is a
/// u64 from the file: at 24 bytes an entry, an unbounded count is 7.6e17
/// iterations
pub const MAX_RELOCS: usize = 1 << 20;

#[derive(Debug, Clone, Copy)]
pub enum RelocError {
    BadDynamic,
    WriteError,
    TooMany,
    /// A relocation only a dynamic linker can resolve, in an image with no
    /// PT_INTERP. Apart from `UnknownType` because the entry is understood, and
    /// nobody is left to run it
    NeedsInterp(u32),
    UnknownType(u32),
}

impl RelocError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BadDynamic     => "malformed PT_DYNAMIC",
            Self::WriteError     => "relocation target rejected",
            Self::TooMany        => "too many relocation entries",
            Self::NeedsInterp(_) => "relocation needs a dynamic linker, image has none",
            Self::UnknownType(_) => "unsupported relocation type",
        }
    }
}

/// The image a relocation pass may write into: the page-rounded span of its
/// PT_LOAD segments, after the load bias. Carried as a value so the target
/// check is a comparison
#[derive(Clone, Copy)]
pub struct RelocWindow {
    lo: u64,
    hi: u64,
}

impl RelocWindow {
    pub fn for_image(info: &ElfInfo, load_bias: u64) -> Option<Self> {
        if info.lo == u64::MAX {
            return None;
        }
        let lo = (info.lo & !0xFFF).checked_add(load_bias)?;
        let hi = (info.hi.checked_add(0xFFF)? & !0xFFF).checked_add(load_bias)?;
        if hi <= lo || !crate::mm::vmm::is_user_range(lo, hi - lo) {
            return None;
        }
        Some(Self { lo, hi })
    }

    #[inline]
    fn contains_word(&self, uva: u64) -> bool {
        match uva.checked_add(8) {
            Some(end) => uva >= self.lo && end <= self.hi,
            None      => false,
        }
    }
}

/// One resolved page, remembered between writes.
///
/// Relocation entries are in address order, so consecutive writes almost always
/// land on the same page: a library with 20k RELATIVE entries pays about 40
/// walks
struct PageCache {
    va:   u64,
    phys: u64,
    ok:   bool,
}

impl PageCache {
    fn new() -> Self { Self { va: u64::MAX, phys: 0, ok: false } }

    /// Physical address of the page holding `uva`, or None if writing there is
    /// not allowed. Every rule from the file header is applied here
    fn resolve(
        &mut self,
        aspace: &AddressSpace,
        win: &RelocWindow,
        uva: u64,
    ) -> Option<u64> {
        if !win.contains_word(uva) {
            return None;
        }
        let page = uva & !0xFFF;
        if page == self.va {
            return if self.ok { Some(self.phys) } else { None };
        }

        self.va = page;
        self.ok = false;

        // PRESENT | USER_ACCESSIBLE: the page has to be one this process owns.
        // Otherwise a target resolving through the kernel's shared upper half
        // would be written
        let flags = aspace.get_page_flags(page)?;
        use x86_64::structures::paging::PageTableFlags as F;
        if !flags.contains(F::PRESENT) || !flags.contains(F::USER_ACCESSIBLE) {
            return None;
        }
        let phys = aspace.virt_to_phys(page)? & !0xFFF;

        self.phys = phys;
        self.ok = true;
        Some(phys)
    }
}

fn write_u64(
    aspace: &AddressSpace,
    cache: &mut PageCache,
    win: &RelocWindow,
    uva: u64,
    val: u64,
) -> bool {
    let off = uva & 0xFFF;
    // No 8-byte word may span two pages: the second page is a separate mapping
    // with its own permissions
    if off + 8 > 4096 {
        return false;
    }
    match cache.resolve(aspace, win, uva) {
        Some(phys) => {
            let hhdm = grub::hhdm();
            unsafe { ((phys + hhdm + off) as *mut u64).write_unaligned(val); }
            true
        }
        None => false,
    }
}

fn read_u64(
    aspace: &AddressSpace,
    cache: &mut PageCache,
    win: &RelocWindow,
    uva: u64,
) -> Option<u64> {
    let off = uva & 0xFFF;
    if off + 8 > 4096 {
        return None;
    }
    let phys = cache.resolve(aspace, win, uva)?;
    let hhdm = grub::hhdm();
    Some(unsafe { ((phys + hhdm + off) as *const u64).read_unaligned() })
}

/// Applies an image's relocations, driven by PT_DYNAMIC. The only relocation
/// entry point: section headers are not required in a loaded image
pub fn apply_rela_from_phys<S: ElfSource + ?Sized>(
    src:       &S,
    info:      &ElfInfo,
    load_bias: u64,
    aspace:    &AddressSpace,
) -> Result<usize, RelocError> {
    let (dyn_off, dyn_sz) = match info.dynamic_offset {
        Some(p) => (p.0 as usize, p.1 as usize),
        None => return Ok(0),
    };
    let dyn_end = dyn_off.checked_add(dyn_sz).ok_or(RelocError::BadDynamic)?;
    if dyn_end > src.len() {
        return Err(RelocError::BadDynamic);
    }
    // The dynamic table, the relocation tables and the symbols they name are
    // read from the image in place, through one buffer (`elf_source`)
    let mut data = Window::new(src);

    // No PT_LOAD span means nothing may be written, so nothing is
    let win = match RelocWindow::for_image(info, load_bias) {
        Some(w) => w,
        None    => return Err(RelocError::BadDynamic),
    };

    let mut rela_va: u64 = 0;
    let mut rela_size: u64 = 0;
    let mut jmprel_va: u64 = 0;
    let mut jmprel_sz: u64 = 0;
    let mut symtab_va: u64 = 0;
    let mut syment: u64 = 24;
    let mut cur = dyn_off;

    while cur + 16 <= dyn_end {
        let tag = data.i64_at(cur).ok_or(RelocError::BadDynamic)?;
        let val = data.u64_at(cur + 8).ok_or(RelocError::BadDynamic)?;
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

    // A symbol entry smaller than the fields read from it cannot be indexed, so
    // this is treated as no symbol table
    if syment < core::mem::size_of::<Elf64Sym>() as u64 {
        symtab_va = 0;
    }

    let mut cache = PageCache::new();
    let mut budget = MAX_RELOCS;
    let mut total = 0usize;

    if rela_size > 0 && rela_va != 0 {
        total += apply_rela_entries_phys(
            &mut data, info, load_bias, aspace, &win, &mut cache, &mut budget,
            rela_va, rela_size, symtab_va, syment,
        )?;
    }

    if jmprel_sz > 0 && jmprel_va != 0 {
        total += apply_rela_entries_phys(
            &mut data, info, load_bias, aspace, &win, &mut cache, &mut budget,
            jmprel_va, jmprel_sz, symtab_va, syment,
        )?;
    }

    Ok(total)
}

#[allow(clippy::too_many_arguments)]
fn apply_rela_entries_phys<S: ElfSource + ?Sized>(
    data:      &mut Window<'_, S>,
    info:      &ElfInfo,
    load_bias: u64,
    aspace:    &AddressSpace,
    win:       &RelocWindow,
    cache:     &mut PageCache,
    budget:    &mut usize,
    rela_va:   u64,
    rela_size: u64,
    symtab_va: u64,
    syment:    u64,
) -> Result<usize, RelocError> {
    // rela_size is a u64 from the file. The entry count is capped before it
    // becomes a loop bound, and charged against a budget shared with the other
    // tables
    let claimed = (rela_size / 24) as usize;
    if claimed > *budget {
        crate::serial_println!(
            "[reloc] {} entries claimed, budget {} - refusing", claimed, *budget,
        );
        return Err(RelocError::TooMany);
    }
    let count = claimed;
    *budget -= count;

    let mut applied = 0usize;
    let mut rejected = 0usize;
    // Entries handed to ld-miku rather than applied here
    let mut deferred = 0usize;

    for i in 0..count {
        let entry_va = match (i as u64).checked_mul(24).and_then(|x| rela_va.checked_add(x)) {
            Some(v) => v,
            None => break,
        };
        // Once the table runs off the end of any PT_LOAD, later entries cannot
        // be inside one either: stop
        let foff = match va_to_file_offset(info, entry_va) {
            Some(o) => o,
            None => break,
        };

        let r_offset = match data.u64_at(foff) { Some(v) => v, None => break };
        let off_info = match foff.checked_add(8)   { Some(v) => v, None => break };
        let r_info = match data.u64_at(off_info) { Some(v) => v, None => break };
        let off_addend = match foff.checked_add(16) { Some(v) => v, None => break };
        let r_addend = match data.i64_at(off_addend) { Some(v) => v, None => break };

        let rtype = r_info as u32;
        let rsym = (r_info >> 32) as u32;
        let target_va = match r_offset.checked_add(load_bias) {
            Some(v) => v,
            None    => { rejected += 1; continue; }
        };

        match rtype {
            R_X86_64_RELATIVE => {
                let value = (load_bias as i64).wrapping_add(r_addend) as u64;
                if write_u64(aspace, cache, win, target_va, value) {
                    applied += 1;
                } else {
                    rejected += 1;
                }
            }
            R_X86_64_JUMP_SLOT | R_X86_64_GLOB_DAT => {
                // A symbol this image does not define is bound by ld-miku
                // against whatever library provides it. Counted as deferred, so
                // the applied/deferred tally adds up to `count`
                let value = if symtab_va != 0 && rsym != 0 {
                    resolve_sym_from_file(data, info, symtab_va, syment, rsym, load_bias)
                } else {
                    None
                };
                match value {
                    Some(v) if write_u64(aspace, cache, win, target_va, v) => applied += 1,
                    Some(_) => rejected += 1,
                    None    => deferred += 1,
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
                if write_u64(aspace, cache, win, target_va, value) {
                    applied += 1;
                } else {
                    rejected += 1;
                }
            }
            R_X86_64_NONE => {}

            // Relocations whose value cannot be computed here.
            //
            // IRELATIVE names a resolver function that has to be called, in
            // ring 3 with the process's own CPU state. COPY moves an object out
            // of a shared library into the executable's .bss, which means
            // knowing which library won the symbol. TLS entries need a module
            // index and a thread-block offset that exist only once a TLS block
            // is laid out per thread.
            //
            // All three are ld-miku's work (lib/ld-miku/src/loader.rs), so with
            // a PT_INTERP they are skipped here and resolved there. Without one
            // (a static-pie, or a binary built against the wrong linker) nobody
            // can do it, and this is an error: a zero left in the GOT would
            // turn the first ifunc call into a jump to address zero
            R_X86_64_IRELATIVE
            | R_X86_64_COPY
            | R_X86_64_TPOFF64
            | R_X86_64_DTPMOD64
            | R_X86_64_DTPOFF64
            | R_X86_64_TLSDESC => {
                if !info.has_interp() {
                    crate::serial_println!(
                        "[reloc] {} at {:#x} needs a dynamic linker, image has no PT_INTERP",
                        reloc_name(rtype), r_offset,
                    );
                    return Err(RelocError::NeedsInterp(rtype));
                }
                deferred += 1;
            }

            // Anything else is a relocation this loader does not know. Refusing
            // beats writing whatever the previous branch left in the target
            other => {
                crate::serial_println!(
                    "[reloc] unsupported relocation type {} at {:#x}", other, r_offset,
                );
                return Err(RelocError::UnknownType(other));
            }
        }
    }

    // A rejected target is a relocation aimed outside the image, and is
    // reported: a nonzero count on a normal program means something upstream is
    // wrong
    if rejected > 0 {
        crate::serial_println!(
            "[reloc] {} of {} targets rejected (outside the image)", rejected, count,
        );
    }
    if deferred > 0 {
        crate::trace_println!(
            "[reloc] {} entries left to the dynamic linker (ifunc/copy/TLS)", deferred,
        );
    }
    crate::trace_println!("[reloc] entries={} applied={}", count, applied);
    Ok(applied)
}

/// A relocation type as text, for the one place that explains itself to a
/// person
fn reloc_name(rtype: u32) -> &'static str {
    match rtype {
        R_X86_64_IRELATIVE => "R_X86_64_IRELATIVE",
        R_X86_64_COPY      => "R_X86_64_COPY",
        R_X86_64_TPOFF64   => "R_X86_64_TPOFF64",
        R_X86_64_DTPMOD64  => "R_X86_64_DTPMOD64",
        R_X86_64_DTPOFF64  => "R_X86_64_DTPOFF64",
        R_X86_64_TLSDESC   => "R_X86_64_TLSDESC",
        _                  => "relocation",
    }
}

fn resolve_sym_from_file<S: ElfSource + ?Sized>(
    data:      &mut Window<'_, S>,
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
    // The window refuses a read past the image's end itself
    soff.checked_add(syment as usize)?;
    let off_shndx = soff.checked_add(6)?;
    let off_value = soff.checked_add(8)?;
    let st_shndx = data.u16_at(off_shndx)?;
    let st_value = data.u64_at(off_value)?;
    if st_shndx == 0 || st_value == 0 {
        return None;
    }
    st_value.checked_add(load_bias)
}

/// Maps an unbiased virtual address to its offset in the file, by binary search
/// over the sorted PT_LOAD list parse() builds. Runs once per relocation
fn va_to_file_offset(info: &ElfInfo, va: u64) -> Option<usize> {
    let ph = info.load_covering_vaddr(va)?;
    let seg_end = ph.p_vaddr.checked_add(ph.p_filesz)?;
    if va >= seg_end {
        return None; // inside the segment, but in its BSS tail
    }
    let file_off = ph.p_offset.checked_add(va - ph.p_vaddr)?;
    let off = file_off as usize;
    if off as u64 != file_off { return None; }
    Some(off)
}

