#![allow(dead_code)]

pub const ET_DYN:        u16 = 3;
pub const PT_LOAD:       u32 = 1;
pub const PT_DYNAMIC:    u32 = 2;
pub const PT_TLS:        u32 = 7;
pub const PT_GNU_RELRO:  u32 = 0x6474_E552;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const DT_NULL:        i64 = 0;
pub const DT_NEEDED:      i64 = 1;
pub const DT_PLTRELSZ:    i64 = 2;
pub const DT_PLTGOT:      i64 = 3;
pub const DT_HASH:        i64 = 4;
pub const DT_STRTAB:      i64 = 5;
pub const DT_SYMTAB:      i64 = 6;
pub const DT_RELA:        i64 = 7;
pub const DT_RELASZ:      i64 = 8;
pub const DT_STRSZ:       i64 = 10;
pub const DT_SYMENT:      i64 = 11;
pub const DT_INIT:         i64 = 12;
pub const DT_FINI:         i64 = 13;
pub const DT_SONAME:       i64 = 14;
pub const DT_JMPREL:       i64 = 23;
pub const DT_INIT_ARRAY:   i64 = 25;
pub const DT_FINI_ARRAY:   i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
pub const DT_FLAGS:        i64 = 30;
pub const DT_FLAGS_1:      i64 = 0x6FFFFFFB;
pub const DT_GNU_HASH:     i64 = 0x6FFFFEF5;

pub const R_X86_64_NONE:      u32 = 0;
pub const R_X86_64_64:        u32 = 1;
pub const R_X86_64_PC32:      u32 = 2;
pub const R_X86_64_COPY:      u32 = 5;
pub const R_X86_64_GLOB_DAT:  u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE:  u32 = 8;
pub const R_X86_64_TPOFF64:   u32 = 18;
pub const R_X86_64_DTPMOD64:  u32 = 16;
pub const R_X86_64_DTPOFF64:  u32 = 17;
pub const R_X86_64_IRELATIVE: u32 = 37;

pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK:   u8 = 2;
pub const SHN_UNDEF:  u16 = 0;

pub const AT_PHDR:   u64 = 3;
pub const AT_PHENT:  u64 = 4;
pub const AT_PHNUM:  u64 = 5;
pub const AT_ENTRY:  u64 = 9;
pub const AT_NULL:   u64 = 0;

pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

#[derive(Clone, Copy)]
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

#[derive(Clone, Copy)]
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

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Dyn {
    pub d_tag: i64,
    pub d_val: u64,
}

#[derive(Clone, Copy)]
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
    pub fn bind(&self) -> u8 { self.st_info >> 4 }
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Rela {
    pub r_offset: u64,
    pub r_info:   u64,
    pub r_addend: i64,
}

impl Elf64Rela {
    pub fn sym(&self)   -> u32 { (self.r_info >> 32) as u32 }
    pub fn rtype(&self) -> u32 { self.r_info as u32 }
}
