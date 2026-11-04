///////////////////////////////////////////////////////////////////
//              ELF shared library loader                        //
//															     //
// Loads ELF DYN objects, resolves symbols, applies relocations  //
// Supports both SysV hash and GNU hash for symbol counting      //
///////////////////////////////////////////////////////////////////

use crate::elf::*;
use crate::syscall;
use crate::util;
use crate::symtab;

const MAX_LIBS:   usize = 32;
const MAX_NEEDED: usize = 32;
const MAX_PATH:   usize = 256;
const MAX_PHDRS:  usize = 128;
/// ELF64 program header size; `phent` must match
const PHDR_SIZE:  usize = 56;
/// PT_DYNAMIC entry limit, against a hostile `p_filesz`
const MAX_DYN_ENTRIES: usize = 4096;

// loaded library tracking

struct LoadedLib {
    soname: [u8; 64],
    base:   u64,
    active: bool,
}

static mut LIBS:       [LoadedLib; MAX_LIBS] = {
    const E: LoadedLib = LoadedLib { soname: [0u8; 64], base: 0, active: false };
    [E; MAX_LIBS]
};
static mut LIBS_COUNT: usize = 0;

// statistics
static mut UNRESOLVED_COUNT: usize = 0;
/// Distance from this object's TLS block to the thread pointer, set by
/// record_tls_layout() before relocations
static mut TLS_TCB_OFF: usize = 0;

fn is_loaded(name: &[u8]) -> bool {
    unsafe {
        for i in 0..LIBS_COUNT {
            let lib = &LIBS[i];
            if !lib.active { continue; }
            let slen = lib.soname.iter().position(|&b| b == 0).unwrap_or(64);
            if util::bytes_eq(&lib.soname[..slen], name) { return true; }
        }
    }
    false
}

fn register_lib(name: &[u8], base: u64) {
    unsafe {
        if LIBS_COUNT >= MAX_LIBS {
            util::warn(b"too many loaded libraries (limit 32)");
            return;
        }
        let lib = &mut LIBS[LIBS_COUNT];
        let copy_len = name.len().min(63);
        lib.soname[..copy_len].copy_from_slice(&name[..copy_len]);
        lib.soname[copy_len] = 0;
        lib.base = base;
        lib.active = true;
        LIBS_COUNT += 1;
    }
}

// dynamic info parsed from PT_DYNAMIC

pub struct DynInfo {
    pub rela_va:         u64,
    pub rela_sz:         u64,
    pub jmprel_va:       u64,
    pub jmprel_sz:       u64,
    pub strtab_va:       u64,
    pub symtab_va:       u64,
    pub syment:          u64,
    pub hash_va:         u64,
    pub gnu_hash_va:     u64,
    pub init_fn:         u64,
    pub init_array_va:   u64,
    pub init_array_sz:   u64,
    pub fini_fn:         u64,
    pub fini_array_va:   u64,
    pub fini_array_sz:   u64,
    /// DT_STRSZ, bounding DT_NEEDED offsets
    pub strsz:           u64,
    pub needed:          [[u8; 64]; MAX_NEEDED],
    pub needed_count:    usize,
}

impl DynInfo {
    fn zero() -> Self {
        Self {
            rela_va: 0, rela_sz: 0, jmprel_va: 0, jmprel_sz: 0,
            strtab_va: 0, symtab_va: 0, syment: 24,
            hash_va: 0, gnu_hash_va: 0,
            init_fn: 0, init_array_va: 0, init_array_sz: 0,
            fini_fn: 0, fini_array_va: 0, fini_array_sz: 0,
            strsz: 0,
            needed: [[0u8; 64]; MAX_NEEDED], needed_count: 0,
        }
    }
}

// unaligned reads

/// Bounds-checked pointer into the mapped file, for offsets taken from the file
fn at(buf: *const u8, len: usize, off: usize, need: usize) -> Option<*const u8> {
    let end = off.checked_add(need)?;
    if end > len { return None; }
    Some(unsafe { buf.add(off) })
}

unsafe fn read_u16(p: *const u8) -> u16 { core::ptr::read_unaligned(p as *const u16) }
unsafe fn read_u32(p: *const u8) -> u32 { core::ptr::read_unaligned(p as *const u32) }
unsafe fn read_u64(p: *const u8) -> u64 { core::ptr::read_unaligned(p as *const u64) }
unsafe fn read_i64(p: *const u8) -> i64 { core::ptr::read_unaligned(p as *const i64) }

// symbol count from hash tables

fn sym_count_from_hash(hash_va: u64) -> usize {
    if hash_va == 0 { return 0; }
    let ptr = hash_va as *const u8;
    unsafe { read_u32(ptr.add(4)) as usize }
}

/// Limit on GNU hash buckets and bloom words, which are u32s from the file
/// multiplied into an offset
const MAX_HASH_WORDS: usize = 1 << 20;

fn sym_count_from_gnu_hash(gnu_hash_va: u64) -> usize {
    if gnu_hash_va == 0 { return 0; }
    let ptr = gnu_hash_va as *const u8;
    let nbuckets  = unsafe { read_u32(ptr) } as usize;
    let symoffset = unsafe { read_u32(ptr.add(4)) } as usize;
    let bloom_sz  = unsafe { read_u32(ptr.add(8)) } as usize;
    if nbuckets == 0 || nbuckets > MAX_HASH_WORDS || bloom_sz > MAX_HASH_WORDS {
        return 0;
    }

    let buckets_off = 16 + bloom_sz * 8;
    let mut max_bucket = 0usize;
    for i in 0..nbuckets {
        let b = unsafe { read_u32(ptr.add(buckets_off + i * 4)) } as usize;
        if b > max_bucket { max_bucket = b; }
    }
    if max_bucket == 0 { return symoffset; }

    let chains_off = buckets_off + nbuckets * 4;
    let mut idx = max_bucket;
    loop {
        let ci = idx.checked_sub(symoffset).unwrap_or(0);
        let val = unsafe { read_u32(ptr.add(chains_off + ci * 4)) };
        if val & 1 != 0 { return idx + 1; }
        idx += 1;
        // Two limits: relative to the start and absolute, so a `max_bucket`
        // near usize::MAX cannot wrap the first
        if idx > max_bucket + 0x10000 || idx > MAX_HASH_WORDS { return idx; }
    }
}

// parse PT_DYNAMIC section

pub fn parse_dynamic(base: u64, phdrs_va: u64, phnum: u16, phent: u16) -> DynInfo {
    let mut di = DynInfo::zero();

    // `phent` comes from a file, so it is checked
    if phent as usize != PHDR_SIZE { return di; }
    let phnum = (phnum as usize).min(MAX_PHDRS);

    for i in 0..phnum as u64 {
        let ph_ptr = (phdrs_va + i * phent as u64) as *const u8;
        let p_type = unsafe { read_u32(ph_ptr) };
        if p_type != PT_DYNAMIC { continue; }

        let p_vaddr  = unsafe { read_u64(ph_ptr.add(16)) };
        let p_filesz = unsafe { read_u64(ph_ptr.add(32)) };
        let dyn_base = base + p_vaddr;
        // Limits the count, since `p_filesz` is a u64 from the file
        let count    = (p_filesz / 16).min(MAX_DYN_ENTRIES as u64);

        // First pass: addresses
        for j in 0..count {
            let d   = (dyn_base + j * 16) as *const u8;
            let tag = unsafe { read_i64(d) };
            let val = unsafe { read_u64(d.add(8)) };
            match tag {
                DT_STRTAB       => di.strtab_va      = base + val,
                DT_STRSZ        => di.strsz          = val,
                DT_SYMTAB       => di.symtab_va      = base + val,
                DT_SYMENT       => di.syment         = val,
                DT_HASH         => di.hash_va        = base + val,
                DT_GNU_HASH     => di.gnu_hash_va    = base + val,
                DT_RELA         => di.rela_va        = base + val,
                DT_RELASZ       => di.rela_sz        = val,
                DT_JMPREL       => di.jmprel_va      = base + val,
                DT_PLTRELSZ     => di.jmprel_sz      = val,
                DT_INIT         => di.init_fn        = base + val,
                DT_INIT_ARRAY   => di.init_array_va  = base + val,
                DT_INIT_ARRAYSZ => di.init_array_sz  = val,
                DT_FINI         => di.fini_fn        = base + val,
                DT_FINI_ARRAY   => di.fini_array_va  = base + val,
                DT_FINI_ARRAYSZ => di.fini_array_sz  = val,
                DT_NULL         => break,
                _               => {}
            }
        }

        // Second pass: NEEDED entries, requiring strtab
        if di.strtab_va != 0 {
            for j in 0..count {
                let d   = (dyn_base + j * 16) as *const u8;
                let tag = unsafe { read_i64(d) };
                let val = unsafe { read_u64(d.add(8)) };
                match tag {
                    DT_NEEDED => {
                        // `val` must lie inside the string table (DT_STRSZ)
                        if di.needed_count < MAX_NEEDED && val < di.strsz {
                            let name_ptr   = (di.strtab_va + val) as *const u8;
                            let room       = (di.strsz - val) as usize;
                            let name_bytes = util::cstr_to_bytes_max(name_ptr, room);
                            let copy_len   = name_bytes.len().min(63);
                            di.needed[di.needed_count][..copy_len]
                                .copy_from_slice(&name_bytes[..copy_len]);
                            di.needed_count += 1;
                        }
                    }
                    DT_NULL => break,
                    _ => {}
                }
            }
        }
        break;
    }
    di
}

// export symbols from a loaded ELF

pub fn export_symbols(base: u64, di: &DynInfo) {
    if di.symtab_va == 0 || di.strtab_va == 0 { return; }

    let nsyms = if di.hash_va != 0 {
        sym_count_from_hash(di.hash_va)
    } else if di.gnu_hash_va != 0 {
        sym_count_from_gnu_hash(di.gnu_hash_va)
    } else {
        0
    };
    if nsyms == 0 { return; }
    symtab::reserve(nsyms);

    for idx in 1..nsyms {
        let s = (di.symtab_va + idx as u64 * di.syment) as *const u8;
        let st_name  = unsafe { read_u32(s) };
        let st_info  = unsafe { *s.add(4) };
        let st_shndx = unsafe { read_u16(s.add(6)) };
        let st_value = unsafe { read_u64(s.add(8)) };
        let bind = st_info >> 4;

        if st_shndx != SHN_UNDEF && (bind == STB_GLOBAL || bind == STB_WEAK) && st_value != 0 {
            let name_ptr = (di.strtab_va + st_name as u64) as *const u8;
            symtab::export(name_ptr, base + st_value, bind == STB_WEAK);
        }
    }
}

// apply relocations

pub fn apply_relocations(base: u64, rela_va: u64, rela_sz: u64, di: &DynInfo) {
    if rela_va == 0 || rela_sz == 0 { return; }

    let count = rela_sz / 24;
    for i in 0..count {
        let r = (rela_va + i * 24) as *const u8;
        let r_offset = unsafe { read_u64(r) };
        let r_info   = unsafe { read_u64(r.add(8)) };
        let r_addend = unsafe { read_i64(r.add(16)) };
        let rtype  = r_info as u32;
        let rsym   = (r_info >> 32) as u32;
        let target = (base + r_offset) as *mut u64;

        // RELATIVE is the most common
        if rtype == R_X86_64_RELATIVE {
            unsafe { target.write_unaligned((base as i64 + r_addend) as u64); }
            continue;
        }

        // IRELATIVE: calls the resolver and writes the result
        if rtype == R_X86_64_IRELATIVE {
            let resolver_addr = (base as i64 + r_addend) as u64;
            if resolver_addr != 0 {
                let resolver: fn() -> u64 = unsafe { core::mem::transmute(resolver_addr) };
                unsafe { target.write_unaligned(resolver()); }
            }
            continue;
        }

        // resolve symbol if needed
        let mut sym_val = 0u64;
        let mut sym_off = 0u64;   // unbiased st_value, used for TLS
        let mut sym_here = false; // defined by this object
        if rsym != 0 && di.symtab_va != 0 {
            let s = (di.symtab_va + rsym as u64 * di.syment) as *const u8;
            let st_name  = unsafe { read_u32(s) };
            let st_info  = unsafe { *s.add(4) };
            let st_shndx = unsafe { read_u16(s.add(6)) };
            let st_value = unsafe { read_u64(s.add(8)) };

            if st_shndx != SHN_UNDEF {
                sym_val  = base + st_value;
                sym_off  = st_value;
                sym_here = true;
            } else {
                let name_ptr = (di.strtab_va + st_name as u64) as *const u8;
                sym_val = symtab::lookup(name_ptr);
                // A weak undefined symbol resolving to zero is valid. A strong
                // one is counted and refused in require_all_resolved(), so a
                // run names every missing symbol
                if sym_val == 0 && (st_info >> 4) != STB_WEAK {
                    unsafe { UNRESOLVED_COUNT += 1; }
                    util::note(b"[ld-miku] unresolved: ");
                    util::noteln(util::cstr_to_bytes(name_ptr));
                }
            }
        }

        unsafe {
            match rtype {
                R_X86_64_64 => {
                    target.write_unaligned(sym_val.wrapping_add(r_addend as u64));
                }
                R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                    // Logs the first 5 JUMP_SLOT resolutions
                    if rtype == R_X86_64_JUMP_SLOT && i < 5 {
                        util::print(b"[ld-miku] JMPREL[");
                        util::print_usize(i as usize);
                        util::print(b"] @");
                        util::print_hex(base + r_offset);
                        util::print(b" -> ");
                        util::print_hex(sym_val);
                        util::println(b"");
                    }
                    target.write_unaligned(sym_val);
                }
                R_X86_64_COPY => {
                    let s = (di.symtab_va + rsym as u64 * di.syment) as *const u8;
                    let st_size = read_u64(s.add(16));
                    if sym_val != 0 && st_size > 0 {
                        util::memcpy(target as *mut u8, sym_val as *const u8, st_size as usize);
                    }
                }
                // TLS variant II: %fs points at the TCB and the block sits
                // below it, so a TPOFF64 is a small negative offset
                R_X86_64_TPOFF64 => {
                    if !(sym_here || rsym == 0) {
                        util::noteln(b"[ld-miku] TPOFF64 against a symbol from                                        another object: only this object's TLS                                        block is allocated");
                        util::panic(b"cross-module TLS is not supported");
                    }
                    // Offset in the block, minus the block's distance from the
                    // thread pointer
                    let off = (sym_off as i64)
                        .wrapping_add(r_addend)
                        .wrapping_sub(tls_tcb_off() as i64);
                    target.write_unaligned(off as u64);
                }
                R_X86_64_DTPMOD64 => {
                    // Dynamic-TLS module index. Only the executable's PT_TLS is
                    // allocated (`setup_tls`), so 1 for this object and none
                    // otherwise
                    if !(sym_here || rsym == 0) {
                        util::panic(b"DTPMOD64 for a module with no TLS block");
                    }
                    target.write_unaligned(1);
                }
                R_X86_64_DTPOFF64 => {
                    // Offset from the block, not the thread pointer
                    if !(sym_here || rsym == 0) {
                        util::panic(b"DTPOFF64 for a module with no TLS block");
                    }
                    target.write_unaligned(sym_off.wrapping_add(r_addend as u64));
                }
                R_X86_64_PC32 => {
                    // PC-relative 32-bit
                    let val = sym_val.wrapping_add(r_addend as u64)
                        .wrapping_sub(base + r_offset);
                    (target as *mut u32).write_unaligned(val as u32);
                }
                R_X86_64_NONE => {}
                _ => {
                    // Refused, like the kernel's RelocError::UnknownType in
                    // `reloc.rs`
                    util::note(b"[ld-miku] unhandled reloc type=");
                    util::note_usize(rtype as usize);
                    util::noteln(b"");
                    util::panic(b"unhandled relocation type");
                }
            }
        }
    }
}

// RELRO protection

pub fn apply_relro(base: u64, phdrs_va: u64, phnum: u16, phent: u16) {
    for i in 0..phnum as u64 {
        let ph = (phdrs_va + i * phent as u64) as *const u8;
        let p_type  = unsafe { read_u32(ph) };
        let p_vaddr = unsafe { read_u64(ph.add(16)) };
        let p_memsz = unsafe { read_u64(ph.add(40)) };
        if p_type != PT_GNU_RELRO || p_memsz == 0 { continue; }
        let start = util::page_align_down(base + p_vaddr);
        let end   = util::page_align_up(base + p_vaddr + p_memsz);
        syscall::mprotect(start, (end - start) as usize, syscall::PROT_READ);
    }
}

// library file lookup

fn open_lib(soname: &[u8]) -> i64 {
    let prefixes: [&[u8]; 3] = [b"/lib/", b"/usr/lib/", b"/usr/local/lib/"];
    for prefix in &prefixes {
        let mut path = [0u8; MAX_PATH];
        let plen = prefix.len();
        let nlen = soname.len().min(MAX_PATH - plen - 1);
        path[..plen].copy_from_slice(prefix);
        path[plen..plen + nlen].copy_from_slice(&soname[..nlen]);
        let fd = syscall::open(&path[..plen + nlen]);
        if fd >= 0 { return fd; }
    }
    -1
}

// load a shared library

pub fn load_library(soname: &[u8]) {
    let name_trimmed = {
        let end = soname.iter().position(|&b| b == 0).unwrap_or(soname.len());
        &soname[..end]
    };
    if name_trimmed.is_empty() || is_loaded(name_trimmed) { return; }

    // try kernel solib cache first
    let base = syscall::map_lib(name_trimmed);
    if base > 0 && (base as i64) > 0 {
        register_lib(name_trimmed, base as u64);
        let eh_ptr      = base as *const u8;
        let e_phoff     = unsafe { read_u64(eh_ptr.add(32)) };
        let e_phnum     = unsafe { read_u16(eh_ptr.add(56)) };
        let e_phentsize = unsafe { read_u16(eh_ptr.add(54)) };
        let phdrs_va    = base as u64 + e_phoff;

        let di = parse_dynamic(base as u64, phdrs_va, e_phnum, e_phentsize);
        for i in 0..di.needed_count { load_library(&di.needed[i]); }
        apply_relocations(base as u64, di.rela_va, di.rela_sz, &di);
        export_symbols(base as u64, &di);
        apply_relocations(base as u64, di.jmprel_va, di.jmprel_sz, &di);
        apply_relro(base as u64, phdrs_va, e_phnum, e_phentsize);

        util::print(b"[ld-miku] shared: ");
        util::print(name_trimmed);
        util::print(b" @ ");
        util::print_hex(base as u64);
        util::println(b"");
        return;
    }

    // Loads from the filesystem
    let fd = open_lib(name_trimmed);
    if fd < 0 {
        util::note(b"[ld-miku] not found: ");
        util::noteln(name_trimmed);
        return;
    }

    let fsize = syscall::fsize(fd);
    if fsize <= 0 || fsize > 64 * 1024 * 1024 {
        syscall::close(fd);
        util::warn(b"invalid library file size");
        return;
    }

    let buf = syscall::mmap(0, fsize as usize, syscall::PROT_READ | syscall::PROT_WRITE);
    if buf.is_null() {
        syscall::close(fd);
        util::warn(b"mmap failed for library file");
        return;
    }

    // read entire file
    let mut done = 0usize;
    while done < fsize as usize {
        let n = syscall::read(fd as u64, unsafe { buf.add(done) }, fsize as usize - done);
        if n <= 0 { break; }
        done += n as usize;
    }
    syscall::close(fd);

    // validate ELF header
    if done < 64 {
        syscall::munmap(buf, fsize as usize);
        util::warn(b"library file too small");
        return;
    }
    let magic = unsafe { core::slice::from_raw_parts(buf, 4) };
    if magic != ELF_MAGIC {
        syscall::munmap(buf, fsize as usize);
        util::warn(b"not an ELF file");
        return;
    }

    let e_phoff     = unsafe { read_u64(buf.add(32)) } as usize;
    let e_phnum     = unsafe { read_u16(buf.add(56)) };
    let e_phentsize = unsafe { read_u16(buf.add(54)) };

    // copy program headers (they will be lost after we unmap the file buffer)
    let phdr_total = (e_phnum as usize) * (e_phentsize as usize);
    let phdr_cap = MAX_PHDRS * 56; // 56 = sizeof(Elf64Phdr)
    let mut phdr_copy = [0u8; MAX_PHDRS * 56];
    let copy_len = phdr_total.min(phdr_cap);
    if e_phoff + copy_len <= fsize as usize {
        util::memcpy(phdr_copy.as_mut_ptr(), unsafe { buf.add(e_phoff) }, copy_len);
    }

    // map segments into memory
    let base = map_elf_segments(buf, fsize as usize);
    syscall::munmap(buf, fsize as usize);

    if base == 0 {
        util::print(b"[ld-miku] failed to map: ");
        util::println(name_trimmed);
        return;
    }

    register_lib(name_trimmed, base);
    let phdrs_va = phdr_copy.as_ptr() as u64;
    let di = parse_dynamic(base, phdrs_va, e_phnum, e_phentsize);
    for i in 0..di.needed_count { load_library(&di.needed[i]); }
    apply_relocations(base, di.rela_va, di.rela_sz, &di);
    export_symbols(base, &di);
    apply_relocations(base, di.jmprel_va, di.jmprel_sz, &di);
    apply_relro(base, phdrs_va, e_phnum, e_phentsize);

    util::print(b"[ld-miku] loaded: ");
    util::print(name_trimmed);
    util::print(b" @ ");
    util::print_hex(base);
    util::println(b"");
}

// map ELF PT_LOAD segments

/// i-th program header, or None past the table
fn phdr_at(buf: *const u8, len: usize, phoff: usize, phent: usize, i: usize) -> Option<*const u8> {
    let off = i.checked_mul(phent)?.checked_add(phoff)?;
    at(buf, len, off, PHDR_MIN_SIZE)
}

const PHDR_MIN_SIZE: usize = 56;

fn map_elf_segments(buf: *const u8, buf_len: usize) -> u64 {
    if buf_len < 64 { return 0; }

    let magic = unsafe { core::slice::from_raw_parts(buf, 4) };
    if magic != ELF_MAGIC { return 0; }

    let e_type      = unsafe { read_u16(buf.add(16)) };
    let e_phoff     = unsafe { read_u64(buf.add(32)) } as usize;
    let e_phnum     = unsafe { read_u16(buf.add(56)) } as usize;
    let e_phentsize = unsafe { read_u16(buf.add(54)) } as usize;
    if e_type != ET_DYN { return 0; }
    if e_phentsize < PHDR_MIN_SIZE { return 0; }

    // calculate total memory footprint
    let mut lo = u64::MAX;
    let mut hi = 0u64;
    for i in 0..e_phnum {
        let ph = match phdr_at(buf, buf_len, e_phoff, e_phentsize, i) {
            Some(p) => p,
            None    => return 0,
        };
        let p_type  = unsafe { read_u32(ph) };
        let p_vaddr = unsafe { read_u64(ph.add(16)) };
        let p_memsz = unsafe { read_u64(ph.add(40)) };
        if p_type != PT_LOAD || p_memsz == 0 { continue; }
        if p_vaddr < lo { lo = p_vaddr; }
        let end = match p_vaddr.checked_add(p_memsz) { Some(v) => v, None => return 0 };
        if end > hi { hi = end; }
    }
    if lo == u64::MAX || hi == 0 { return 0; }

    lo &= !0xFFF;
    hi = util::page_align_up(hi);
    let map_size = hi - lo;

    let base_ptr = syscall::mmap(0, map_size as usize, syscall::PROT_READ | syscall::PROT_WRITE);
    if base_ptr.is_null() { return 0; }
    let base = base_ptr as u64 - lo;

    // copy segments and set protection
    for i in 0..e_phnum {
        let ph = match phdr_at(buf, buf_len, e_phoff, e_phentsize, i) {
            Some(p) => p,
            None    => break,
        };
        let p_type   = unsafe { read_u32(ph) };
        let p_flags  = unsafe { read_u32(ph.add(4)) };
        let p_offset = unsafe { read_u64(ph.add(8)) } as usize;
        let p_vaddr  = unsafe { read_u64(ph.add(16)) };
        let p_filesz = unsafe { read_u64(ph.add(32)) } as usize;
        let p_memsz  = unsafe { read_u64(ph.add(40)) } as usize;
        if p_type != PT_LOAD || p_memsz == 0 { continue; }
        if p_filesz > p_memsz { continue; }

        let dst = (base + p_vaddr) as *mut u8;
        // `at`, since `p_offset + p_filesz` can wrap
        let copied = match at(buf, buf_len, p_offset, p_filesz) {
            Some(src) if p_filesz > 0 => { util::memcpy(dst, src, p_filesz); p_filesz }
            _ => 0,
        };
        // Zeroes what was not copied: BSS and any file shortfall
        if p_memsz > copied {
            util::memset(unsafe { dst.add(copied) }, 0, p_memsz - copied);
        }

        let mut prot = 0u64;
        if p_flags & PF_R != 0 { prot |= syscall::PROT_READ; }
        if p_flags & PF_W != 0 { prot |= syscall::PROT_WRITE; }
        if p_flags & PF_X != 0 { prot |= syscall::PROT_EXEC; }
        let pstart = util::page_align_down(base + p_vaddr);
        let pend   = util::page_align_up(base + p_vaddr + p_memsz as u64);
        syscall::mprotect(pstart, (pend - pstart) as usize, prot);
    }

    base
}

// call init functions

pub fn call_init(di: &DynInfo) {
    // DT_INIT runs first
    if di.init_fn != 0 {
        let f: fn() = unsafe { core::mem::transmute(di.init_fn) };
        f();
    }
    // DT_INIT_ARRAY runs second
    if di.init_array_va != 0 && di.init_array_sz != 0 {
        let count = di.init_array_sz / 8;
        for i in 0..count {
            let fn_ptr_va = (di.init_array_va + i * 8) as *const u64;
            let fn_addr = unsafe { fn_ptr_va.read_unaligned() };
            if fn_addr != 0 && fn_addr != u64::MAX {
                let f: fn() = unsafe { core::mem::transmute(fn_addr) };
                f();
            }
        }
    }
}

// setup TLS (thread-local storage)

/// PT_TLS location and distance below the thread pointer, shared by TPOFF64
/// relocation and block allocation
struct TlsLayout {
    vaddr:   u64,
    filesz:  usize,
    memsz:   usize,
    align:   usize,
    tcb_off: usize,
}

fn tls_layout(phdrs_va: u64, phnum: u16, phent: u16) -> Option<TlsLayout> {
    for i in 0..phnum as u64 {
        let ph = (phdrs_va + i * phent as u64) as *const u8;
        let p_type = unsafe { read_u32(ph) };
        let p_memsz = unsafe { read_u64(ph.add(40)) };
        if p_type != PT_TLS || p_memsz == 0 { continue; }

        let p_align = (unsafe { read_u64(ph.add(48)) }).max(1) as usize;
        let align = p_align.max(8);
        let memsz = p_memsz as usize;
        let vaddr = unsafe { read_u64(ph.add(16)) };
        return Some(TlsLayout {
            vaddr,
            filesz:  unsafe { read_u64(ph.add(32)) } as usize,
            memsz,
            align,
            tcb_off: tcb_offset(vaddr, memsz, p_align),
        });
    }
    None
}

/// Distance below the thread pointer where the TLS segment starts, by the
/// linker's formula (lld's `getTlsTpOffset` for x86-64): `memsz`, plus what
/// keeps the start congruent to `p_vaddr` modulo `p_align`
fn tcb_offset(vaddr: u64, memsz: usize, p_align: usize) -> usize {
    memsz + ((0usize.wrapping_sub(vaddr as usize).wrapping_sub(memsz)) & (p_align - 1))
}

/// Records the TLS layout before relocations. Zero with no PT_TLS
pub fn record_tls_layout(phdrs_va: u64, phnum: u16, phent: u16) {
    let off = tls_layout(phdrs_va, phnum, phent).map(|l| l.tcb_off).unwrap_or(0);
    unsafe { TLS_TCB_OFF = off; }
}

fn tls_tcb_off() -> usize {
    unsafe { TLS_TCB_OFF }
}

/// Refuses to start with undefined strong symbols. Names were printed as found
pub fn require_all_resolved() {
    let n = unsafe { UNRESOLVED_COUNT };
    if n == 0 { return; }
    util::note(b"[ld-miku] ");
    util::note_usize(n);
    util::noteln(b" symbol(s) above are defined by no loaded library");
    util::panic(b"unresolved symbols");
}

/// MikuOS thread control block at the thread pointer. The ABI fixes only the
/// first word, the pointer's own value.
///
/// ```text
/// tp +  0   self        this address
///   tp +  8   tls_size    bytes of thread-local data below tp
/// tp + 16   tls_align   alignment of tp
/// tp + 24   tls_image   address of the initial data in the exe image
/// tp + 32   tls_filesz  how much of tls_size the image covers; the rest is `.tbss`
/// tp + 40   errno, tp + 48 pthread block, tp + 56 heap cache: zero here, filled by libmiku (`docs/MikuOS_ABI.md` 3.2d)
/// ```
///
/// `lib/libmiku/thread.rs` reads these at every `thread_spawn`. 64 bytes.
pub const MIKU_TCB_SIZE: usize = 64;

pub fn setup_tls(base: u64, phdrs_va: u64, phnum: u16, phent: u16) -> u64 {
    if let Some(l) = tls_layout(phdrs_va, phnum, phent) {
        let TlsLayout { vaddr: p_vaddr, filesz, memsz: _, align, tcb_off } = l;
        // tp is aligned; the data starts `tcb_off` below it, which may not be
        let lead = (tcb_off + align - 1) & !(align - 1);
        let total = util::page_align_up((lead + MIKU_TCB_SIZE) as u64) as usize;

        let tls_mem = syscall::mmap(0, total, syscall::PROT_READ | syscall::PROT_WRITE);
        if tls_mem.is_null() { return 0; }
        util::memset(tls_mem, 0, total);
        if filesz > 0 {
            util::memcpy(unsafe { tls_mem.add(lead - tcb_off) }, (base + p_vaddr) as *const u8, filesz);
        }

        let tcb = unsafe { tls_mem.add(lead) } as u64;
        unsafe {
            // tp+0: the ABI's requirement
            (tcb as *mut u64).write(tcb);
            // tp+8..tp+32: for building a thread's block. The image pointer is
            // into the executable's `.tdata`, which threads only copy from
            ((tcb + 8)  as *mut u64).write(tcb_off as u64);
            ((tcb + 16) as *mut u64).write(align as u64);
            ((tcb + 24) as *mut u64).write(base + p_vaddr);
            ((tcb + 32) as *mut u64).write(filesz as u64);
        }
        syscall::set_tls(tcb);
        return tcb;
    }

    // No PT_TLS: a one-page zeroed TCB, since `thread_spawn` reads `tp + 8`
    let page = syscall::mmap(0, 4096, syscall::PROT_READ | syscall::PROT_WRITE);
    if page.is_null() { return 0; }
    util::memset(page, 0, 4096);
    let tcb = page as u64;
    unsafe { (tcb as *mut u64).write(tcb); }
    syscall::set_tls(tcb);
    tcb
}

// diagnostics

pub fn print_stats() {
    util::print(b"[ld-miku] symbols: ");
    util::print_usize(symtab::count());
    util::print(b", libs: ");
    unsafe { util::print_usize(LIBS_COUNT); }
    let unresolved = unsafe { UNRESOLVED_COUNT };
    if unresolved > 0 {
        util::print(b", unresolved: ");
        util::print_usize(unresolved);
    }
    util::println(b"");
}
