extern crate alloc;
use crate::process::elf_source::{read_vec, ElfSource};

pub use miku_abi::auxv::*;
pub use miku_abi::elf::*;

/// Longest build-id kept. It ties a crash report or a core file to the exact
/// binary that produced it
pub const BUILD_ID_MAX: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    NotX86_64,
    UnsupportedType,
    BadPhdr,
    TooManyPhdrs,
    NoLoadSegments,
    SegmentOutOfBounds,
    BadVersion,
    BadEhsize,
    DuplicateSegment,
    BadAlign,
    AddressOutOfRange,
    ImageTooLarge,
}

impl ElfError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TooSmall           => "file too small for ELF header",
            Self::BadMagic           => "invalid ELF magic",
            Self::Not64Bit           => "not ELF64",
            Self::NotLittleEndian    => "not little-endian",
            Self::NotX86_64          => "not x86_64",
            Self::UnsupportedType    => "unsupported ELF type (need ET_EXEC or ET_DYN)",
            Self::BadPhdr            => "program headers out of bounds",
            Self::TooManyPhdrs       => "too many program headers",
            Self::NoLoadSegments     => "no PT_LOAD segments",
            Self::SegmentOutOfBounds => "segment data beyond file end",
            Self::BadVersion         => "unsupported ELF version",
            Self::BadEhsize          => "wrong e_ehsize for ELF64",
            Self::DuplicateSegment   => "duplicate PT_INTERP/PT_DYNAMIC/PT_PHDR/PT_TLS",
            Self::BadAlign           => "bad p_align, or p_vaddr not congruent to p_offset",
            Self::AddressOutOfRange  => "segment outside the user address range",
            Self::ImageTooLarge      => "image asks for more memory than a process may have",
        }
    }
}

pub const MAX_PHDRS: usize = 128;

/// Ceiling on the sum of p_memsz across PT_LOAD. Without it a handful of
/// segments claiming a terabyte each would loop on frame allocation until
/// physical memory runs out, and then unwind. Checking the total before the
/// first alloc_frame makes that a cheap rejection
pub const MAX_IMAGE_MEM: u64 = 1024 * 1024 * 1024;

/// Largest note segment `parse` reads. A build id and a GNU property note
/// are well under a hundred bytes together
const MAX_NOTE_SEGMENT: u64 = 64 * 1024;

/// Largest `PT_INTERP` read, far past any path the loader would accept
const MAX_INTERP_SEGMENT: usize = 4096;

pub struct ElfInfo {
    pub ehdr:           Elf64Ehdr,
    pub phdrs:          alloc::vec::Vec<Elf64Phdr>,
    pub phdr_count:     usize,
    pub entry:          u64,
    pub is_dyn:         bool,
    pub interp_offset:  Option<(u64, u64)>,
    pub dynamic_offset: Option<(u64, u64)>,
    pub load_bias:      u64,
    pub phdr_vaddr:     u64,

    // Everything below is collected during the single pass in parse().
    /// Indices into `phdrs` of the PT_LOAD segments, sorted by p_vaddr.
    /// Sorted order is what makes the overlap check and the vaddr->file
    /// lookup linear instead of quadratic
    pub load:       alloc::vec::Vec<usize>,
    pub tls:        Option<usize>,
    /// PT_GNU_RELRO indices. More than one is legal and all of them matter
    pub relro:      alloc::vec::Vec<usize>,
    pub gnu_stack:  Option<u32>,
    /// Unbiased virtual bounds of the PT_LOAD set, and the sum of their
    /// p_memsz. `lo` is u64::MAX when there are no PT_LOAD segments
    pub lo:         u64,
    pub hi:         u64,
    pub total_mem:  u64,

    /// NT_GNU_BUILD_ID from PT_NOTE, and its real length. Fixed-size so
    /// ElfInfo stays copyable into an ImageInfo without another allocation
    pub build_id:     [u8; BUILD_ID_MAX],
    pub build_id_len: usize,
    /// GNU_PROPERTY_X86_FEATURE_1_AND from PT_GNU_PROPERTY
    pub x86_features: u32,
}

impl ElfInfo {
    pub fn load_segments(&self) -> LoadSegmentIter<'_> {
        LoadSegmentIter { info: self, idx: 0 }
    }

    /// Unbiased [lowest p_vaddr, highest p_vaddr + p_memsz) over PT_LOAD.
    /// Precomputed in parse(); kept as a method because callers read better
    pub fn memory_bounds(&self) -> (u64, u64) {
        (self.lo, self.hi)
    }

    /// `PT_INTERP`'s path, read from the image. The loader bounds its
    /// length before it trusts it (`interp_path_ok`); this bounds the read
    pub fn interp_path<S: ElfSource + ?Sized>(&self, src: &S) -> Option<alloc::string::String> {
        let (off, sz) = self.interp_offset?;
        if sz as usize > MAX_INTERP_SEGMENT { return None; }
        let bytes = read_vec(src, off as usize, sz as usize)?;
        let nul   = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        core::str::from_utf8(&bytes[..nul]).ok().map(alloc::string::String::from)
    }

    pub fn has_interp(&self) -> bool {
        self.interp_offset.is_some()
    }

    pub fn gnu_stack_flags(&self) -> Option<u32> {
        self.gnu_stack
    }

    /// The PT_LOAD segment covering `va` (unbiased), by binary search over the
    /// sorted index. Runs once per relocation
    pub fn load_covering_vaddr(&self, va: u64) -> Option<&Elf64Phdr> {
        let mut lo = 0usize;
        let mut hi = self.load.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let ph = &self.phdrs[self.load[mid]];
            if va < ph.p_vaddr {
                hi = mid;
            } else if va >= ph.p_vaddr.saturating_add(ph.p_memsz) {
                lo = mid + 1;
            } else {
                return Some(ph);
            }
        }
        None
    }

    pub fn build_id(&self) -> Option<&[u8]> {
        if self.build_id_len == 0 { None } else { Some(&self.build_id[..self.build_id_len]) }
    }

    pub fn wants_ibt(&self) -> bool {
        self.x86_features & GNU_PROPERTY_X86_FEATURE_1_IBT != 0
    }

    pub fn wants_shstk(&self) -> bool {
        self.x86_features & GNU_PROPERTY_X86_FEATURE_1_SHSTK != 0
    }

    /// True when `va` (already biased) falls inside a PT_LOAD marked PF_X.
    /// `bias` is what was added to every p_vaddr at load time
    pub fn is_executable_vaddr(&self, va: u64, bias: u64) -> bool {
        let unbiased = match va.checked_sub(bias) { Some(v) => v, None => return false };
        match self.load_covering_vaddr(unbiased) {
            Some(ph) => ph.p_flags & PF_X != 0,
            None     => false,
        }
    }
}

pub struct LoadSegmentIter<'a> {
    info: &'a ElfInfo,
    idx:  usize,
}

impl<'a> Iterator for LoadSegmentIter<'a> {
    type Item = &'a Elf64Phdr;
    fn next(&mut self) -> Option<Self::Item> {
        let i = *self.info.load.get(self.idx)?;
        self.idx += 1;
        Some(&self.info.phdrs[i])
    }
}

/// Where a symbol table lives in a file: (symbols, strings) as byte ranges.
pub struct SymTabRef {
    pub sym_off:  usize,
    pub sym_size: usize,
    pub str_off:  usize,
    pub str_size: usize,
}

/// Locates a symbol table by walking the section header table.
///
/// Sections are not needed to run a program, so this is separate from `parse()`
/// and called only when somebody wants names for addresses. `.symtab` is
/// preferred over `.dynsym`: it carries local functions too, most of what a
/// backtrace is made of.
pub fn find_symtab<S: ElfSource + ?Sized>(src: &S) -> Option<SymTabRef> {
    // By range, like `parse`: the section headers are at the far end of a file
    // that can be bigger than the kernel would read whole
    let ehdr_bytes = read_vec(src, 0, core::mem::size_of::<Elf64Ehdr>())?;
    let ehdr = unsafe { core::ptr::read_unaligned(ehdr_bytes.as_ptr() as *const Elf64Ehdr) };
    if ehdr.e_ident[0..4] != ELF_MAGIC {
        return None;
    }

    let shoff = ehdr.e_shoff as usize;
    let shent = ehdr.e_shentsize as usize;
    let shnum = ehdr.e_shnum as usize;
    if shoff == 0 || shnum == 0 || shent < core::mem::size_of::<Elf64Shdr>() {
        return None;
    }
    let table = read_vec(src, shoff, shent.checked_mul(shnum)?)?;

    let shdr_at = |i: usize| -> Option<Elf64Shdr> {
        let off = shent.checked_mul(i)?;
        Some(unsafe { core::ptr::read_unaligned(table.as_ptr().add(off) as *const Elf64Shdr) })
    };

    let len = src.len();
    let mut best: Option<SymTabRef> = None;
    for i in 0..shnum {
        let sh = shdr_at(i)?;
        if sh.sh_type != SHT_SYMTAB && sh.sh_type != SHT_DYNSYM {
            continue;
        }
        // sh_link names the string table for this symbol table
        let link = sh.sh_link as usize;
        if link >= shnum {
            continue;
        }
        let strsh = shdr_at(link)?;
        if strsh.sh_type != SHT_STRTAB {
            continue;
        }

        let sym_off = sh.sh_offset as usize;
        let sym_size = sh.sh_size as usize;
        let str_off = strsh.sh_offset as usize;
        let str_size = strsh.sh_size as usize;
        let sym_end = sym_off.checked_add(sym_size)?;
        let str_end = str_off.checked_add(str_size)?;
        if sym_end > len || str_end > len || sym_size == 0 {
            continue;
        }

        let found = SymTabRef { sym_off, sym_size, str_off, str_size };
        // SHT_SYMTAB wins outright; .dynsym is only the fallback
        if sh.sh_type == SHT_SYMTAB {
            return Some(found);
        }
        if best.is_none() {
            best = Some(found);
        }
    }
    best
}

/// Walks one PT_NOTE segment.
///
/// A note is `namesz, descsz, type`, then the name padded to 4 bytes, then the
/// descriptor padded to 4. Every length comes from the file, so each step is
/// bounds-checked against the segment
fn walk_notes(data: &[u8], off: u64, size: u64, mut visit: impl FnMut(&str, u32, &[u8])) {
    let start = off as usize;
    let end = match (off as usize).checked_add(size as usize) {
        Some(v) if v <= data.len() => v,
        _ => return,
    };

    let mut cur = start;
    // A note is 12 bytes of header at minimum; stop as soon as one cannot fit
    while cur + 12 <= end {
        let namesz = u32::from_le_bytes(match data[cur..cur + 4].try_into() {
            Ok(v) => v, Err(_) => return,
        }) as usize;
        let descsz = u32::from_le_bytes(match data[cur + 4..cur + 8].try_into() {
            Ok(v) => v, Err(_) => return,
        }) as usize;
        let ntype = u32::from_le_bytes(match data[cur + 8..cur + 12].try_into() {
            Ok(v) => v, Err(_) => return,
        });

        let name_at = cur + 12;
        let name_end = match name_at.checked_add(namesz) {
            Some(v) if v <= end => v,
            _ => return,
        };
        let desc_at = match name_end.checked_add(3) {
            Some(v) => v & !3,
            None    => return,
        };
        let desc_end = match desc_at.checked_add(descsz) {
            Some(v) if v <= end => v,
            _ => return,
        };

        // The name carries its own NUL; trim it before comparing
        let raw = &data[name_at..name_end];
        let nul = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        if let Ok(name) = core::str::from_utf8(&raw[..nul]) {
            visit(name, ntype, &data[desc_at..desc_end]);
        }

        cur = match desc_end.checked_add(3) {
            Some(v) => v & !3,
            None    => return,
        };
    }
}

/// GNU_PROPERTY_X86_FEATURE_1_AND out of a NT_GNU_PROPERTY_TYPE_0 note.
/// Properties are `type, datasz, data`, each padded to 8 bytes on ELF64
fn parse_gnu_property(desc: &[u8]) -> u32 {
    let mut cur = 0usize;
    let mut features = 0u32;
    while cur + 8 <= desc.len() {
        let ptype = u32::from_le_bytes(match desc[cur..cur + 4].try_into() {
            Ok(v) => v, Err(_) => break,
        });
        let datasz = u32::from_le_bytes(match desc[cur + 4..cur + 8].try_into() {
            Ok(v) => v, Err(_) => break,
        }) as usize;
        let data_at = cur + 8;
        let data_end = match data_at.checked_add(datasz) {
            Some(v) if v <= desc.len() => v,
            _ => break,
        };
        if ptype == GNU_PROPERTY_X86_FEATURE_1_AND && datasz >= 4 {
            features = u32::from_le_bytes(match desc[data_at..data_at + 4].try_into() {
                Ok(v) => v, Err(_) => break,
            });
        }
        cur = match data_end.checked_add(7) {
            Some(v) => v & !7,
            None    => break,
        };
    }
    features
}

/// Every field below comes from a file a user picked: bounds are checked with
/// checked_* against data.len(), addresses against the user half, and anything
/// the ELF spec says "must" about is rejected rather than repaired. A loader
/// tolerating a malformed image has to guess what the missing part meant.
///
/// The header, the program headers and the notes of an image, read from `src`
/// by range (`elf_source`): nothing here needs the whole file
pub fn parse<S: ElfSource + ?Sized>(src: &S) -> Result<ElfInfo, ElfError> {
    let ehdr_bytes = read_vec(src, 0, core::mem::size_of::<Elf64Ehdr>()).ok_or(ElfError::TooSmall)?;
    let ehdr = unsafe { core::ptr::read_unaligned(ehdr_bytes.as_ptr() as *const Elf64Ehdr) };

    if ehdr.e_ident[0..4] != ELF_MAGIC       { return Err(ElfError::BadMagic);         }
    if ehdr.e_ident[4]    != ELFCLASS64      { return Err(ElfError::Not64Bit);         }
    if ehdr.e_ident[5]    != ELFDATA2LSB     { return Err(ElfError::NotLittleEndian);  }
    // EI_VERSION and e_version both name the same "1", and a file disagreeing
    // with itself about its format version is rejected
    if ehdr.e_ident[6]    != EV_CURRENT      { return Err(ElfError::BadVersion);       }
    if ehdr.e_version     != EV_CURRENT as u32 { return Err(ElfError::BadVersion);     }
    if ehdr.e_ehsize as usize != core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::BadEhsize);
    }
    if ehdr.e_machine     != EM_X86_64       { return Err(ElfError::NotX86_64);        }
    if ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }

    let phoff = ehdr.e_phoff as usize;
    let phent = ehdr.e_phentsize as usize;
    let phnum = ehdr.e_phnum as usize;

    // Reject rather than silently truncate: dropping phdrs past a cap could
    // skip PT_LOAD/PT_DYNAMIC entries and load a half-image
    if phnum > MAX_PHDRS {
        return Err(ElfError::TooManyPhdrs);
    }

    // All multiplications/additions on attacker-controlled u64 fields
    // go through checked arithmetic so a crafted ELF cannot wrap and
    // bypass the data.len() bound below
    if phent < core::mem::size_of::<Elf64Phdr>() {
        return Err(ElfError::BadPhdr);
    }
    let ph_table_size = match phent.checked_mul(phnum) {
        Some(v) => v,
        None    => return Err(ElfError::BadPhdr),
    };
    let ph_end = match phoff.checked_add(ph_table_size) {
        Some(v) => v,
        None    => return Err(ElfError::BadPhdr),
    };
    if ph_end > src.len() {
        return Err(ElfError::BadPhdr);
    }
    let ph_table = read_vec(src, phoff, ph_table_size).ok_or(ElfError::BadPhdr)?;

    let is_dyn = ehdr.e_type == ET_DYN;

    let mut phdrs          = alloc::vec::Vec::with_capacity(phnum);
    let mut load           = alloc::vec::Vec::new();
    let mut relro          = alloc::vec::Vec::new();
    let mut tls            = None;
    let mut gnu_stack      = None;
    let mut interp_offset  = None;
    let mut dynamic_offset = None;
    let mut phdr_vaddr     = 0u64;
    let mut saw_phdr       = false;
    let mut lo             = u64::MAX;
    let mut hi             = 0u64;
    let mut total_mem      = 0u64;
    let mut notes: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();

    for i in 0..phnum {
        // phoff + i*phent is bounded by ph_end above, but computed via
        // checked_add anyway. Within the table just read: i < phnum, so i *
        // phent < ph_table_size
        let off = match phent.checked_mul(i) {
            Some(v) => v,
            None    => return Err(ElfError::BadPhdr),
        };
        let p = unsafe {
            core::ptr::read_unaligned(ph_table.as_ptr().add(off) as *const Elf64Phdr)
        };

        // Any segment with on-disk bytes must reference an in-file range,
        // PT_INTERP, PT_DYNAMIC and others as well as LOAD, since those offsets
        // are later used to read strings and tables
        if p.p_filesz > 0 {
            let off_us = p.p_offset as usize;
            let sz_us  = p.p_filesz as usize;
            let end = match off_us.checked_add(sz_us) {
                Some(v) => v,
                None    => return Err(ElfError::SegmentOutOfBounds),
            };
            if end > src.len() {
                return Err(ElfError::SegmentOutOfBounds);
            }
        }
        // p_memsz can never be smaller than p_filesz; that would leave
        // file bytes with no in-memory backing
        if p.p_memsz < p.p_filesz {
            return Err(ElfError::SegmentOutOfBounds);
        }
        // The virtual range must not wrap. p_vaddr+p_memsz is used in many
        // downstream computations
        let vend = match p.p_vaddr.checked_add(p.p_memsz) {
            Some(v) => v,
            None    => return Err(ElfError::SegmentOutOfBounds),
        };

        // Exactly one of each of these is meaningful. With two, "the last one
        // wins" is a choice an attacker can aim, by hiding a benign PT_INTERP
        // in front of the real one
        match p.p_type {
            PT_INTERP => {
                if interp_offset.is_some() { return Err(ElfError::DuplicateSegment); }
                interp_offset = Some((p.p_offset, p.p_filesz));
            }
            PT_DYNAMIC => {
                if dynamic_offset.is_some() { return Err(ElfError::DuplicateSegment); }
                dynamic_offset = Some((p.p_offset, p.p_filesz));
            }
            PT_PHDR => {
                if saw_phdr { return Err(ElfError::DuplicateSegment); }
                saw_phdr = true;
                phdr_vaddr = p.p_vaddr;
            }
            PT_TLS => {
                if tls.is_some() { return Err(ElfError::DuplicateSegment); }
                tls = Some(i);
            }
            PT_GNU_STACK => { gnu_stack = Some(p.p_flags); }
            // Both carry notes; PT_GNU_PROPERTY is a NOTE segment that the
            // linker split out so the loader can find it without scanning
            PT_NOTE | PT_GNU_PROPERTY => {
                if p.p_filesz > 0 { notes.push((p.p_offset, p.p_filesz)); }
            }
            PT_GNU_RELRO => { relro.push(i); }
            PT_LOAD => {
                // p_align is used as a bitmask when rounding to pages, so a
                // non-power-of-two corrupts the rounding. The spec requires the
                // segment's file and virtual positions to agree modulo the
                // alignment; if not, "map this file offset at this address" has
                // no consistent meaning at page granularity
                if p.p_align > 1 {
                    if !p.p_align.is_power_of_two() {
                        return Err(ElfError::BadAlign);
                    }
                    if p.p_vaddr % p.p_align != p.p_offset % p.p_align {
                        return Err(ElfError::BadAlign);
                    }
                }

                // For ET_EXEC there is no bias to come: p_vaddr is the
                // final address, so it can be judged now. ET_DYN is judged
                // by the loader once the ASLR bias is picked
                if !is_dyn && p.p_memsz > 0 {
                    if p.p_vaddr < crate::mm::vmm::USER_MIN
                        || vend > crate::mm::vmm::USER_END
                    {
                        return Err(ElfError::AddressOutOfRange);
                    }
                }

                total_mem = match total_mem.checked_add(p.p_memsz) {
                    Some(v) if v <= MAX_IMAGE_MEM => v,
                    _ => return Err(ElfError::ImageTooLarge),
                };
                if p.p_memsz > 0 {
                    if p.p_vaddr < lo { lo = p.p_vaddr; }
                    if vend > hi     { hi = vend; }
                }
                load.push(i);
            }
            _ => {}
        }

        phdrs.push(p);
    }

    if load.is_empty() { return Err(ElfError::NoLoadSegments); }

    // Notes are read after the phdr loop so the segment bounds above are
    // checked. A missing or malformed note is not fatal: notes describe the
    // binary, they do not load it
    let mut build_id = [0u8; BUILD_ID_MAX];
    let mut build_id_len = 0usize;
    let mut x86_features = 0u32;
    for &(off, sz) in notes.iter() {
        // A note segment is a few dozen bytes; one claiming more than this is
        // not read
        if sz > MAX_NOTE_SEGMENT {
            continue;
        }
        let Some(bytes) = read_vec(src, off as usize, sz as usize) else { continue };
        walk_notes(&bytes, 0, sz, |name, ntype, desc| {
            if name == "GNU" && ntype == NT_GNU_BUILD_ID && build_id_len == 0 {
                let n = desc.len().min(BUILD_ID_MAX);
                build_id[..n].copy_from_slice(&desc[..n]);
                build_id_len = n;
            } else if name == "GNU" && ntype == NT_GNU_PROPERTY_TYPE_0 {
                x86_features |= parse_gnu_property(desc);
            }
        });
    }

    // Sorted by p_vaddr once, here, so every later stage (overlap check,
    // vaddr->file lookup, VMA registration) can rely on the order
    load.sort_unstable_by_key(|&i| phdrs[i].p_vaddr);

    Ok(ElfInfo {
        ehdr,
        phdrs,
        phdr_count: phnum,
        entry: ehdr.e_entry,
        is_dyn,
        interp_offset,
        dynamic_offset,
        load_bias: 0,
        phdr_vaddr,
        load,
        tls,
        relro,
        gnu_stack,
        lo,
        hi,
        total_mem,
        build_id,
        build_id_len,
        x86_features,
    })
}
