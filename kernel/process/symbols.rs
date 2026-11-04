// Turning an address back into a name.
//
// A `SymbolTable` is built from an ELF's `.symtab` (or `.dynsym` when that is
// all there is) and answers which function contains an address, and how far
// into it. A user image's table is built when a crash report needs it and
// dropped at once. The kernel's own table is built once from the section
// headers GRUB hands over and kept, because a kernel panic is not a moment to
// read files.

extern crate alloc;
use alloc::vec::Vec;
use crate::process::elf;
use crate::process::elf_source::{read_vec, ElfSource};

/// Ceiling on how many symbols are indexed from one file. A table this size is
/// already 1.5 MiB of entries; beyond it a file is malformed or hostile
const MAX_SYMBOLS: usize = 1 << 16;

/// Ceiling on the string table read with them. A symbol whose name starts
/// past it is left out, as a malformed one is
const MAX_STRINGS: usize = 8 << 20;

/// One indexed symbol. `name` is an offset into `strtab` rather than a
/// `&str` so the table owns exactly two allocations no matter how many
/// symbols it holds
#[derive(Clone, Copy)]
struct Sym {
    value: u64,
    size:  u64,
    name:  u32,
}

pub struct SymbolTable {
    /// Sorted by `value`, which is what makes lookup a binary search
    syms:   Vec<Sym>,
    strtab: Vec<u8>,
}

impl SymbolTable {
    /// Builds from an ELF image: a file held in memory, or an open one read by
    /// range (`elf_source::FileSource`), so a program too big to read whole
    /// still gets names in its crash report. Only the two sections are read, as
    /// much as the table keeps
    pub fn from_file<S: ElfSource + ?Sized>(src: &S) -> Option<Self> {
        let r = elf::find_symtab(src)?;
        let entsize = core::mem::size_of::<elf::Elf64Sym>();
        let symbols = read_vec(src, r.sym_off, r.sym_size.min(MAX_SYMBOLS * entsize))?;
        let strings = read_vec(src, r.str_off, r.str_size.min(MAX_STRINGS))?;
        Self::from_raw(&symbols, &strings)
    }

    /// Builds from the two section bodies directly. The kernel's own tables
    /// arrive this way: GRUB loaded the sections into memory
    pub fn from_raw(symbols: &[u8], strings: &[u8]) -> Option<Self> {
        let entsize = core::mem::size_of::<elf::Elf64Sym>();
        let count = (symbols.len() / entsize).min(MAX_SYMBOLS);
        if count == 0 {
            return None;
        }

        let mut syms = Vec::with_capacity(count);
        for i in 0..count {
            let off = i * entsize;
            let sym = unsafe {
                core::ptr::read_unaligned(symbols.as_ptr().add(off) as *const elf::Elf64Sym)
            };
            // Only things occupying an address range. SECTION/FILE entries have
            // no body, and a zero st_value is an undefined symbol
            let kind = sym.sym_type();
            if kind != elf::STT_FUNC && kind != elf::STT_OBJECT {
                continue;
            }
            if sym.st_value == 0 || sym.st_name as usize >= strings.len() {
                continue;
            }
            syms.push(Sym { value: sym.st_value, size: sym.st_size, name: sym.st_name });
        }
        if syms.is_empty() {
            return None;
        }

        syms.sort_unstable_by_key(|s| s.value);
        Some(SymbolTable { syms, strtab: strings.to_vec() })
    }

    pub fn len(&self) -> usize { self.syms.len() }
    pub fn is_empty(&self) -> bool { self.syms.is_empty() }

    fn name_at(&self, off: u32) -> &str {
        let start = off as usize;
        if start >= self.strtab.len() {
            return "?";
        }
        let rest = &self.strtab[start..];
        let nul = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        core::str::from_utf8(&rest[..nul]).unwrap_or("?")
    }

    /// The symbol containing `addr`, and how far into it that is.
    ///
    /// `addr` must have any load bias subtracted: the table holds link-time
    /// addresses. When several symbols share a start address (aliases) the
    /// search lands on one of them.
    pub fn resolve(&self, addr: u64) -> Option<(&str, u64)> {
        // partition_point gives the first symbol after addr, so the candidate
        // is the one before it
        let idx = self.syms.partition_point(|s| s.value <= addr);
        if idx == 0 {
            return None;
        }
        let s = self.syms[idx - 1];
        let delta = addr - s.value;
        // An address outside a symbol's declared size is not in it; a symbol
        // with no size is the best guess, and "func+0x2e" beats nothing
        if s.size != 0 && delta >= s.size {
            return None;
        }
        Some((self.name_at(s.name), delta))
    }

    /// Every symbol, in address order: (name, value, size). For `elf syms`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, u64, u64)> + '_ {
        self.syms.iter().map(move |s| (self.name_at(s.name), s.value, s.size))
    }
}

// the kernel's own table //

use spin::Mutex;

/// Built once from the section headers GRUB handed over, then kept. A panic is
/// the wrong moment to allocate and parse, and there is no file to read it from
static KERNEL_SYMS: Mutex<Option<SymbolTable>> = Mutex::new(None);
static KERNEL_TRIED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Locates `.symtab` and its `.strtab` among the kernel's sections and indexes
/// them. Safe to call more than once
pub fn init_kernel() {
    use core::sync::atomic::Ordering;
    if KERNEL_TRIED.swap(true, Ordering::SeqCst) {
        return;
    }

    let Some((arr, num, entsize)) = crate::arch::x86_64::grub::elf_sections() else {
        crate::serial_println!("[sym] no multiboot2 ELF sections - kernel symbols unavailable");
        return;
    };

    let shdr = |i: usize| -> elf::Elf64Shdr {
        unsafe {
            core::ptr::read_unaligned((arr + (i * entsize) as u64) as *const elf::Elf64Shdr)
        }
    };

    let hhdm = crate::arch::x86_64::grub::hhdm();
    // A section GRUB loaded but the kernel does not map has sh_addr in
    // physical terms; reach it through the HHDM. One that is part of the
    // loaded image already has a virtual address in the kernel half
    let body = |sh: &elf::Elf64Shdr| -> Option<&'static [u8]> {
        let addr = sh.sh_addr;
        let size = sh.sh_size as usize;
        if addr == 0 || size == 0 {
            return None;
        }
        let va = if addr >= crate::mm::vmm::KERNEL_HALF { addr } else { addr + hhdm };
        Some(unsafe { core::slice::from_raw_parts(va as *const u8, size) })
    };

    for i in 0..num {
        let sh = shdr(i);
        if sh.sh_type != elf::SHT_SYMTAB {
            continue;
        }
        let link = sh.sh_link as usize;
        if link >= num {
            continue;
        }
        let strsh = shdr(link);
        if strsh.sh_type != elf::SHT_STRTAB {
            continue;
        }
        let (Some(syms), Some(strs)) = (body(&sh), body(&strsh)) else { continue };
        if let Some(t) = SymbolTable::from_raw(syms, strs) {
            crate::serial_println!("[sym] kernel: {} symbols indexed", t.len());
            *KERNEL_SYMS.lock() = Some(t);
            return;
        }
    }
    crate::serial_println!("[sym] kernel .symtab not found among {} sections", num);
}

/// Name and offset for a kernel address, or None when the table is missing
pub fn resolve_kernel(addr: u64) -> Option<(alloc::string::String, u64)> {
    let guard = KERNEL_SYMS.lock();
    let t = guard.as_ref()?;
    t.resolve(addr).map(|(n, off)| (alloc::string::String::from(n), off))
}

pub fn kernel_symbol_count() -> usize {
    KERNEL_SYMS.lock().as_ref().map(|t| t.len()).unwrap_or(0)
}
