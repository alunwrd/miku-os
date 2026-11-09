//! ELF: what the kernel's loader and `ld-miku` both read

// The ident bytes
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const ELFOSABI_NONE: u8 = 0;
pub const EV_CURRENT: u8 = 1;

// e_type
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

// e_machine
pub const EM_X86_64: u16 = 62;

// p_type
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
pub const PT_GNU_PROPERTY: u32 = 0x6474_E553;

// p_flags
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

// sh_type
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

// d_tag
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

// Relocations
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
pub const R_X86_64_DTPMOD64: u32 = 16;
pub const R_X86_64_DTPOFF64: u32 = 17;
pub const R_X86_64_TPOFF64: u32 = 18;
pub const R_X86_64_TLSGD: u32 = 19;
pub const R_X86_64_TLSLD: u32 = 20;
pub const R_X86_64_DTPOFF32: u32 = 21;
pub const R_X86_64_GOTTPOFF: u32 = 22;
pub const R_X86_64_TPOFF32: u32 = 23;
pub const R_X86_64_TLSDESC: u32 = 36;
pub const R_X86_64_IRELATIVE: u32 = 37;

// Symbols
pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;
pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_FILE: u8 = 4;
pub const SHN_UNDEF: u16 = 0;

// Notes
pub const NT_GNU_BUILD_ID: u32 = 3;
pub const NT_GNU_PROPERTY_TYPE_0: u32 = 5;
pub const GNU_PROPERTY_X86_FEATURE_1_AND: u32 = 0xC000_0002;
pub const GNU_PROPERTY_X86_FEATURE_1_IBT:   u32 = 1 << 0;
pub const GNU_PROPERTY_X86_FEATURE_1_SHSTK: u32 = 1 << 1;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Ehdr {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Shdr {
    pub sh_name: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_link: u32,
    pub sh_info: u32,
    pub sh_addralign: u64,
    pub sh_entsize: u64,
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
    pub st_name: u32,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size: u64,
}

impl Elf64Sym {
    pub fn binding(&self) -> u8 {
        self.st_info >> 4
    }
    pub fn sym_type(&self) -> u8 {
        self.st_info & 0xF
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Rela {
    pub r_offset: u64,
    pub r_info: u64,
    pub r_addend: i64,
}

impl Elf64Rela {
    pub fn sym(&self) -> u32 {
        (self.r_info >> 32) as u32
    }
    pub fn rtype(&self) -> u32 {
        self.r_info as u32
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Rel {
    pub r_offset: u64,
    pub r_info: u64,
}

impl Elf64Rel {
    pub fn sym(&self) -> u32 {
        (self.r_info >> 32) as u32
    }
    pub fn rtype(&self) -> u32 {
        self.r_info as u32
    }
}

const _: () = assert!(core::mem::size_of::<Elf64Ehdr>() == 64);
const _: () = assert!(core::mem::size_of::<Elf64Phdr>() == 56);
const _: () = assert!(core::mem::size_of::<Elf64Shdr>() == 64);
const _: () = assert!(core::mem::size_of::<Elf64Sym>() == 24);
const _: () = assert!(core::mem::size_of::<Elf64Rela>() == 24);
