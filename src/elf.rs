pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const EV_CURRENT: u8 = 1;
pub const ELFOSABI_NONE: u8 = 0;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

pub const EM_X86_64: u16 = 62;

pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_EH_FRAME: u32 = 0x6474_E550;
pub const PT_GNU_STACK: u32 = 0x6474_E551;
pub const PT_GNU_RELRO: u32 = 0x6474_E552;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_HASH: u32 = 5;
pub const SHT_DYNAMIC: u32 = 6;
pub const SHT_NOTE: u32 = 7;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHT_DYNSYM: u32 = 11;

pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_PLTGOT: i64 = 3;
pub const DT_HASH: i64 = 4;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_STRSZ: i64 = 10;
pub const DT_SYMENT: i64 = 11;
pub const DT_INIT: i64 = 12;
pub const DT_FINI: i64 = 13;
pub const DT_SONAME: i64 = 14;
pub const DT_RPATH: i64 = 15;
pub const DT_SYMBOLIC: i64 = 16;
pub const DT_REL: i64 = 17;
pub const DT_RELSZ: i64 = 18;
pub const DT_RELENT: i64 = 19;
pub const DT_PLTREL: i64 = 20;
pub const DT_DEBUG: i64 = 21;
pub const DT_TEXTREL: i64 = 22;
pub const DT_JMPREL: i64 = 23;
pub const DT_BIND_NOW: i64 = 24;
pub const DT_INIT_ARRAY: i64 = 25;
pub const DT_FINI_ARRAY: i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
pub const DT_FLAGS: i64 = 30;
pub const DT_FLAGS_1: i64 = 0x6FFFFFFB;
pub const DT_GNU_HASH: i64 = 0x6FFFFEF5;
pub const DT_RELACOUNT: i64 = 0x6FFFFFF9;

pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_PC32: u32 = 2;
pub const R_X86_64_GOT32: u32 = 3;
pub const R_X86_64_PLT32: u32 = 4;
pub const R_X86_64_COPY: u32 = 5;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE: u32 = 8;
pub const R_X86_64_GOTPCREL: u32 = 9;
pub const R_X86_64_32: u32 = 10;
pub const R_X86_64_32S: u32 = 11;
pub const R_X86_64_IRELATIVE: u32 = 37;

pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;

pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_FILE: u8 = 4;

pub const AT_NULL:    u64 = 0;
pub const AT_PHDR:    u64 = 3;
pub const AT_PHENT:   u64 = 4;
pub const AT_PHNUM:   u64 = 5;
pub const AT_PAGESZ:  u64 = 6;
pub const AT_BASE:    u64 = 7;
pub const AT_FLAGS:   u64 = 8;
pub const AT_ENTRY:   u64 = 9;
pub const AT_UID:     u64 = 11;
pub const AT_EUID:    u64 = 12;
pub const AT_GID:     u64 = 13;
pub const AT_EGID:    u64 = 14;
pub const AT_HWCAP:   u64 = 16;
pub const AT_CLKTCK:  u64 = 17;
pub const AT_SECURE:  u64 = 23;
pub const AT_RANDOM:  u64 = 25;
pub const AT_HWCAP2:  u64 = 26;
pub const AT_EXECFN:  u64 = 31;
pub const AT_SYSINFO_EHDR: u64 = 33;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Ehdr {
    pub e_ident:     [u8; 16],
    pub e_type:      u16,
    pub e_machine:   u16,
    pub e_version:   u32,
    pub e_entry:     u64,
    pub e_phoff:     u64,
    pub e_shoff:     u64,
    pub e_flags:     u32,
    pub e_ehsize:    u16,
    pub e_phentsize: u16,
    pub e_phnum:     u16,
    pub e_shentsize: u16,
    pub e_shnum:     u16,
    pub e_shstrndx:  u16,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Phdr {
    pub p_type:   u32,
    pub p_flags:  u32,
    pub p_offset: u64,
    pub p_vaddr:  u64,
    pub p_paddr:  u64,
    pub p_filesz: u64,
    pub p_memsz:  u64,
    pub p_align:  u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Shdr {
    pub sh_name:      u32,
    pub sh_type:      u32,
    pub sh_flags:     u64,
    pub sh_addr:      u64,
    pub sh_offset:    u64,
    pub sh_size:      u64,
    pub sh_link:      u32,
    pub sh_info:      u32,
    pub sh_addralign: u64,
    pub sh_entsize:   u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Dyn {
    pub d_tag: i64,
    pub d_val: u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Sym {
    pub st_name:  u32,
    pub st_info:  u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size:  u64,
}

impl Elf64Sym {
    pub fn binding(&self) -> u8 { self.st_info >> 4 }
    pub fn sym_type(&self) -> u8 { self.st_info & 0xF }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Rela {
    pub r_offset: u64,
    pub r_info:   u64,
    pub r_addend: i64,
}

impl Elf64Rela {
    pub fn sym(&self) -> u32 { (self.r_info >> 32) as u32 }
    pub fn rtype(&self) -> u32 { self.r_info as u32 }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Rel {
    pub r_offset: u64,
    pub r_info:   u64,
}

impl Elf64Rel {
    pub fn sym(&self) -> u32 { (self.r_info >> 32) as u32 }
    pub fn rtype(&self) -> u32 { self.r_info as u32 }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    NotX86_64,
    UnsupportedType,
    BadPhdr,
    NoLoadSegments,
    SegmentOutOfBounds,
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
            Self::NoLoadSegments     => "no PT_LOAD segments",
            Self::SegmentOutOfBounds => "segment data beyond file end",
        }
    }
}

pub const MAX_PHDRS: usize = 32;

pub struct ElfInfo {
    pub ehdr:           Elf64Ehdr,
    pub phdrs:          [Elf64Phdr; MAX_PHDRS],
    pub phdr_count:     usize,
    pub entry:          u64,
    pub is_dyn:         bool,
    pub interp_offset:  Option<(u64, u64)>,
    pub dynamic_offset: Option<(u64, u64)>,
    pub load_bias:      u64,
    pub phdr_vaddr:     u64,
}

impl ElfInfo {
    pub fn load_segments(&self) -> LoadSegmentIter<'_> {
        LoadSegmentIter { info: self, idx: 0 }
    }

    pub fn memory_bounds(&self) -> (u64, u64) {
        let mut lo = u64::MAX;
        let mut hi = 0u64;
        for i in 0..self.phdr_count {
            let p = &self.phdrs[i];
            if p.p_type != PT_LOAD { continue; }
            let vaddr = p.p_vaddr;
            // saturating - the parser already rejects segments whose
            // file-backed range overflows; p_memsz can legitimately
            // exceed p_filesz (BSS) so cap rather than wrap
            let end = vaddr.saturating_add(p.p_memsz);
            if vaddr < lo { lo = vaddr; }
            if end   > hi { hi = end;   }
        }
        (lo, hi)
    }

    pub fn interp_path<'a>(&self, data: &'a [u8]) -> Option<&'a str> {
        let (off, sz) = self.interp_offset?;
        let start = off as usize;
        let end   = start.checked_add(sz as usize)?;
        if end > data.len() { return None; }
        let slice = &data[start..end];
        let nul   = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        core::str::from_utf8(&slice[..nul]).ok()
    }

    pub fn has_interp(&self) -> bool {
        self.interp_offset.is_some()
    }

    pub fn gnu_stack_flags(&self) -> Option<u32> {
        for i in 0..self.phdr_count {
            if self.phdrs[i].p_type == PT_GNU_STACK {
                return Some(self.phdrs[i].p_flags);
            }
        }
        None
    }
}

pub struct LoadSegmentIter<'a> {
    info: &'a ElfInfo,
    idx:  usize,
}

impl<'a> Iterator for LoadSegmentIter<'a> {
    type Item = &'a Elf64Phdr;
    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.info.phdr_count {
            let p = &self.info.phdrs[self.idx];
            self.idx += 1;
            if p.p_type == PT_LOAD { return Some(p); }
        }
        None
    }
}

pub fn parse(data: &[u8]) -> Result<ElfInfo, ElfError> {
    if data.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::TooSmall);
    }

    let ehdr = unsafe { core::ptr::read_unaligned(data.as_ptr() as *const Elf64Ehdr) };

    if ehdr.e_ident[0..4] != ELF_MAGIC       { return Err(ElfError::BadMagic);         }
    if ehdr.e_ident[4]    != ELFCLASS64       { return Err(ElfError::Not64Bit);         }
    if ehdr.e_ident[5]    != ELFDATA2LSB      { return Err(ElfError::NotLittleEndian);  }
    if ehdr.e_machine      != EM_X86_64       { return Err(ElfError::NotX86_64);        }
    if ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }

    let phoff = ehdr.e_phoff as usize;
    let phent = ehdr.e_phentsize as usize;
    let phnum = (ehdr.e_phnum as usize).min(MAX_PHDRS);

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
    if ph_end > data.len() {
        return Err(ElfError::BadPhdr);
    }

    let mut phdrs          = [unsafe { core::mem::zeroed::<Elf64Phdr>() }; MAX_PHDRS];
    let mut has_load       = false;
    let mut interp_offset  = None;
    let mut dynamic_offset = None;
    let mut phdr_vaddr     = 0u64;

    for i in 0..phnum {
        // phoff + i*phent is bounded by ph_end above, but compute via
        // checked_add anyway for defense in depth
        let off = match phent.checked_mul(i).and_then(|x| phoff.checked_add(x)) {
            Some(v) => v,
            None    => return Err(ElfError::BadPhdr),
        };
        let p   = unsafe {
            core::ptr::read_unaligned(data.as_ptr().add(off) as *const Elf64Phdr)
        };

        // Any segment with on-disk bytes must reference an in-file range
        // - applies to PT_INTERP / PT_DYNAMIC / etc. too, not just LOAD,
        // since those offsets are later used to read strings/tables
        if p.p_filesz > 0 {
            let off_us = p.p_offset as usize;
            let sz_us  = p.p_filesz as usize;
            let end = match off_us.checked_add(sz_us) {
                Some(v) => v,
                None    => return Err(ElfError::SegmentOutOfBounds),
            };
            if end > data.len() {
                return Err(ElfError::SegmentOutOfBounds);
            }
        }
        // p_memsz can never be smaller than p_filesz; that would leave
        // file bytes with no in-memory backing
        if p.p_memsz < p.p_filesz {
            return Err(ElfError::SegmentOutOfBounds);
        }
        // The virtual range must not wrap. p_vaddr+p_memsz is used in
        // many downstream computations; rejecting wrap here keeps the
        // loader simple
        if p.p_vaddr.checked_add(p.p_memsz).is_none() {
            return Err(ElfError::SegmentOutOfBounds);
        }

        if p.p_type == PT_LOAD { has_load = true; }
        if p.p_type == PT_INTERP  { interp_offset  = Some((p.p_offset, p.p_filesz)); }
        if p.p_type == PT_DYNAMIC { dynamic_offset  = Some((p.p_offset, p.p_filesz)); }
        if p.p_type == PT_PHDR    { phdr_vaddr      = p.p_vaddr; }

        phdrs[i] = p;
    }

    if !has_load { return Err(ElfError::NoLoadSegments); }

    Ok(ElfInfo {
        ehdr,
        phdrs,
        phdr_count: phnum,
        entry: ehdr.e_entry,
        is_dyn: ehdr.e_type == ET_DYN,
        interp_offset,
        dynamic_offset,
        load_bias: 0,
        phdr_vaddr,
    })
}
