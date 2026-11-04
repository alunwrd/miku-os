#![no_std]
#![no_main]
#![allow(dead_code, unused, static_mut_refs)]

mod syscall;
mod elf;
mod util;
mod symtab;
mod loader;

use elf::*;
use loader::*;

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo) -> ! {
    util::panic(b"rust panic");
}

/// Whether a NUL-terminated string starts with `pre`. Local, since this runs
/// before relocation
unsafe fn has_prefix(s: *const u8, pre: &[u8]) -> bool {
    for (i, b) in pre.iter().enumerate() {
        let c = *s.add(i);
        if c == 0 || c != *b { return false; }
    }
    true
}

unsafe fn read_u16_u(p: *const u8) -> u16 {
    core::ptr::read_unaligned(p as *const u16)
}
unsafe fn read_u64_u(p: *const u8) -> u64 {
    core::ptr::read_unaligned(p as *const u64)
}

#[no_mangle]
pub extern "C" fn dynlinker_main(sp: *const u64) -> ! {
    let argc = unsafe { *sp } as usize;

    // scan past argv[] to find NULL terminator
    let mut p = unsafe { sp.add(1) };
    unsafe {
        while *p != 0 { p = p.add(1); } // skip argv entries
        p = p.add(1);                    // skip argv NULL

        // envp, reading `LD_DEBUG`, glibc's switch for loader output. Off by
        // default, since output costs serial port time
        while *p != 0 {
            let var = *p as *const u8;
            if has_prefix(var, b"LD_DEBUG=") {
                // Any non-empty value
                if *var.add(9) != 0 {
                    util::set_debug(true);
                }
            }
            p = p.add(1);
        }
        p = p.add(1);                    // skip envp NULL
    }

    let mut phdr_va:   u64 = 0;
    let mut phnum:     u16 = 0;
    let mut phent:     u16 = 0;
    let mut exe_entry: u64 = 0;

    let auxv = p;
    let mut av = p;
    loop {
        let key = unsafe { *av };
        let val = unsafe { *av.add(1) };
        match key {
            AT_PHDR  => phdr_va    = val,
            AT_PHNUM => phnum      = val as u16,
            AT_PHENT => phent      = val as u16,
            AT_ENTRY => exe_entry  = val,
            AT_NULL  => break,
            _        => {}
        }
        av = unsafe { av.add(2) };
    }

    if phdr_va == 0 || phnum == 0 || exe_entry == 0 {
        util::panic(b"bad auxv: missing AT_PHDR or AT_ENTRY");
    }

    let exe_base = find_exe_base(phdr_va, phnum, phent);

    let di = parse_dynamic(exe_base, phdr_va, phnum, phent);

    // Before relocations: TPOFF64 is measured against the TLS block, allocated
    // later by setup_tls()
    record_tls_layout(phdr_va, phnum, phent);

    for i in 0..di.needed_count {
        load_library(&di.needed[i]);
    }

    apply_relocations(exe_base, di.rela_va,   di.rela_sz,   &di);
    export_symbols(exe_base, &di);

    util::print(b"[ld-miku] jmprel: ");
    util::print_usize((di.jmprel_sz / 24) as usize);
    util::println(b" entries");

    apply_relocations(exe_base, di.jmprel_va, di.jmprel_sz, &di);

    util::println(b"[ld-miku] jmprel done");

    // Reports missing symbols once everything is bound, before any constructor
    // runs
    require_all_resolved();

    apply_relro(exe_base, phdr_va, phnum, phent);

    setup_tls(exe_base, phdr_va, phnum, phent);

    // Passes auxv to libmiku before constructors can read the clock
    // (`lib/libmiku/auxv.rs`), as glibc's `_dl_auxv`
    let set_auxv = symtab::lookup(b"miku_set_auxv\0".as_ptr());
    if set_auxv != 0 {
        let f: extern "C" fn(*const u64) = unsafe { core::mem::transmute(set_auxv) };
        f(auxv);
    }

    call_init(&di);
    print_stats();

    util::print(b"[ld-miku] -> ");
    util::print_hex(exe_entry);
    util::println(b"");

    unsafe {
        core::arch::asm!(
            "mov rsp, {sp}",
            "jmp {entry}",
            sp    = in(reg) sp,
            entry = in(reg) exe_entry,
            options(noreturn, nostack)
        );
    }
}

fn find_exe_base(phdr_va: u64, phnum: u16, phent: u16) -> u64 {
    // Load bias: the phdr table's address minus its link-time vaddr from
    // PT_PHDR. Works for ET_EXEC and PIE
    for i in 0..phnum as u64 {
        let ph = (phdr_va + i * phent as u64) as *const u8;
        let p_type  = unsafe { read_u32_u(ph) };
        let p_vaddr = unsafe { read_u64_u(ph.add(16)) };
        if p_type == PT_PHDR {
            return phdr_va.wrapping_sub(p_vaddr);
        }
    }
    // No PT_PHDR: with the usual layout, the table follows the 64-byte ehdr
    for i in 0..phnum as u64 {
        let ph = (phdr_va + i * phent as u64) as *const u8;
        let p_type   = unsafe { read_u32_u(ph) };
        let p_vaddr  = unsafe { read_u64_u(ph.add(16)) };
        let p_offset = unsafe { read_u64_u(ph.add(8)) };
        if p_type == PT_LOAD && p_vaddr == 0 && p_offset == 0 {
            return phdr_va.wrapping_sub(64);
        }
    }
    0
}

unsafe fn read_u32_u(p: *const u8) -> u32 {
    core::ptr::read_unaligned(p as *const u32)
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _ldstart() {
    core::arch::naked_asm!(
        "mov  rdi, rsp",
        "and  rsp, -16",
        "call dynlinker_main",
        "mov  rax, 0",
        "xor  rdi, rdi",
        "syscall",
    );
}
