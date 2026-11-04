// The `elf` command: looking inside a binary without running it.
//
// Four subcommands, each a thin shell over machinery used elsewhere (the
// parser, `symbols.rs`, `crash.rs`):
//
//   elf info <path>            what the loader sees
//   elf syms <path> [pattern]  the symbol table
//   elf addr <path> <hex>      one address -> function+offset
//   elf test                   the loader's adversarial selftest
//
// `elf addr` checks the symbolisation path without crashing anything: take an
// address from a crash report, subtract the bias the report printed, and ask.

extern crate alloc;
use alloc::vec::Vec;

use crate::println;
use crate::process::elf;
use crate::process::symbols::SymbolTable;

pub fn cmd_elf(rest: &str) {
    let rest = rest.trim();
    let (sub, args) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None    => (rest, ""),
    };

    match sub {
        "test" => crate::process::selftest::cmd_selftest(),
        "info" => info(args),
        "syms" => syms(args),
        "addr" => addr(args),
        "core" => cores(),
        "cache" => crate::process::imgcache::report(),
        _ => {
            println!("Usage:");
            println!("  elf info <path>             segments, dynamic info, build-id");
            println!("  elf syms <path> [pattern]   symbol table");
            println!("  elf addr <path> <hex>       address -> function+offset");
            println!("  elf core                    core dumps in /tmp");
            println!("  elf cache                   shared image pages");
            println!("  elf test                    loader selftest");
        }
    }
}

fn read(path: &str) -> Option<Vec<u8>> {
    match crate::fs::vfs_read::read_file_or_solib(path) {
        Some(d) => Some(d),
        None => { crate::print_error!("  cannot read {}", path); None }
    }
}

fn info(path: &str) {
    if path.is_empty() {
        println!("Usage: elf info <path>");
        return;
    }
    let Some(data) = read(path) else { return };
    let info = match elf::parse(&data) {
        Ok(i) => i,
        Err(e) => { crate::print_error!("  not loadable: {}", e.as_str()); return; }
    };

    let kind = if info.is_dyn { "ET_DYN (PIE/shared)" } else { "ET_EXEC (fixed)" };
    println!("  {} - {} bytes", path, data.len());
    println!("  type      {}", kind);
    println!("  entry     {:#x}", info.entry);
    let (lo, hi) = info.memory_bounds();
    println!("  image     {:#x}-{:#x}  ({} KiB resident)", lo, hi, info.total_mem / 1024);

    match info.build_id() {
        Some(id) => {
            let mut s = alloc::string::String::new();
            for b in id { push_hex2(&mut s, *b); }
            println!("  build-id  {}", s);
        }
        None => println!("  build-id  (none)"),
    }

    if info.wants_ibt() || info.wants_shstk() {
        println!(
            "  cet      {}{}",
            if info.wants_ibt() { " ibt" } else { "" },
            if info.wants_shstk() { " shstk" } else { "" },
        );
    }

    match info.interp_path(&data) {
        Some(p) => println!("  interp    {}", p),
        None    => println!("  interp    (none - static)"),
    }
    match info.gnu_stack_flags() {
        Some(f) if f & elf::PF_X != 0 => println!("  stack     EXECUTABLE (refused at load)"),
        Some(_) => println!("  stack     non-executable"),
        None    => println!("  stack     (no PT_GNU_STACK)"),
    }

    // Fields are copied out one at a time: Elf64Phdr is #[repr(packed)],
    // so a reference to a field of it is not a reference the ABI allows
    println!("  segments:");
    for ph in info.load_segments() {
        let (va, fsz, msz, al, fl) =
            (ph.p_vaddr, ph.p_filesz, ph.p_memsz, ph.p_align, ph.p_flags);
        println!(
            "    LOAD  {:#012x}  file {:>8}  mem {:>8}  {}{}{}  align {:#x}",
            va, fsz, msz,
            if fl & elf::PF_R != 0 { "r" } else { "-" },
            if fl & elf::PF_W != 0 { "w" } else { "-" },
            if fl & elf::PF_X != 0 { "x" } else { "-" },
            al,
        );
    }
    if let Some(i) = info.tls {
        let ph = info.phdrs[i];
        let (va, fsz, msz) = (ph.p_vaddr, ph.p_filesz, ph.p_memsz);
        println!("    TLS   {:#012x}  file {:>8}  mem {:>8}", va, fsz, msz);
    }
    for &i in &info.relro {
        let ph = info.phdrs[i];
        let (va, msz) = (ph.p_vaddr, ph.p_memsz);
        println!("    RELRO {:#012x}  {:>8} bytes made read-only after relocation", va, msz);
    }

    dynamic(&data, &info);
}

/// DT_NEEDED and the relocation counts, from PT_DYNAMIC: the table `reloc.rs`
/// drives from, read here without applying anything
fn dynamic(data: &[u8], info: &elf::ElfInfo) {
    let Some((off, sz)) = info.dynamic_offset else {
        return;
    };
    let (start, end) = (off as usize, (off + sz) as usize);
    if end > data.len() {
        return;
    }

    let mut strtab = 0u64;
    let mut strsz = 0u64;
    let mut needed: Vec<u64> = Vec::new();
    let mut soname: Option<u64> = None;
    let mut relasz = 0u64;
    let mut pltrelsz = 0u64;

    let mut cur = start;
    while cur + 16 <= end {
        let tag = i64::from_le_bytes(data[cur..cur + 8].try_into().unwrap_or([0; 8]));
        let val = u64::from_le_bytes(data[cur + 8..cur + 16].try_into().unwrap_or([0; 8]));
        match tag {
            elf::DT_NEEDED    => needed.push(val),
            elf::DT_SONAME    => soname = Some(val),
            elf::DT_STRTAB    => strtab = val,
            elf::DT_STRSZ     => strsz = val,
            elf::DT_RELASZ    => relasz = val,
            elf::DT_PLTRELSZ  => pltrelsz = val,
            elf::DT_NULL      => break,
            _ => {}
        }
        cur += 16;
    }

    // DT_STRTAB is a virtual address; the file offset it corresponds to is
    // found the same way the relocation applier finds one
    let str_base = vaddr_to_off(info, strtab);
    let name_at = |o: u64| -> &str {
        let Some(base) = str_base else { return "?" };
        let at = (base + o) as usize;
        if at >= data.len() || o >= strsz { return "?"; }
        let rest = &data[at..];
        let nul = rest.iter().position(|&b| b == 0).unwrap_or(0);
        core::str::from_utf8(&rest[..nul]).unwrap_or("?")
    };

    if let Some(o) = soname {
        println!("  soname    {}", name_at(o));
    }
    if !needed.is_empty() {
        println!("  needed:");
        for o in &needed {
            println!("    {}", name_at(*o));
        }
    }
    if relasz + pltrelsz > 0 {
        println!(
            "  relocs    {} in .rela.dyn, {} in .rela.plt",
            relasz / 24, pltrelsz / 24,
        );
    }
}

fn vaddr_to_off(info: &elf::ElfInfo, va: u64) -> Option<u64> {
    if va == 0 {
        return None;
    }
    let ph = info.load_covering_vaddr(va)?;
    let delta = va.checked_sub(ph.p_vaddr)?;
    if delta >= ph.p_filesz {
        return None;
    }
    ph.p_offset.checked_add(delta)
}

fn syms(args: &str) {
    let (path, pattern) = match args.find(char::is_whitespace) {
        Some(i) => (&args[..i], args[i..].trim()),
        None    => (args, ""),
    };
    if path.is_empty() {
        println!("Usage: elf syms <path> [pattern]");
        return;
    }
    let Some(data) = read(path) else { return };
    let Some(table) = SymbolTable::from_file(&data) else {
        crate::print_error!("  no symbol table in {}", path);
        return;
    };

    let mut shown = 0usize;
    for (name, value, size) in table.iter() {
        if !pattern.is_empty() && !name.contains(pattern) {
            continue;
        }
        println!("  {:#012x}  {:>7}  {}", value, size, name);
        shown += 1;
        // A full table is thousands of lines; a pattern asks for more than this
        if shown >= 200 {
            println!("  ... stopped at 200 of {} - narrow it with a pattern", table.len());
            return;
        }
    }
    if shown == 0 {
        println!("  no match among {} symbols", table.len());
    } else {
        println!("  {} of {} symbols", shown, table.len());
    }
}

fn addr(args: &str) {
    let (path, hex) = match args.find(char::is_whitespace) {
        Some(i) => (&args[..i], args[i..].trim()),
        None    => (args, ""),
    };
    if path.is_empty() || hex.is_empty() {
        println!("Usage: elf addr <path> <hex>");
        println!("  the address is link-time: subtract the bias a crash report printed");
        return;
    }
    let hex = hex.trim_start_matches("0x");
    let Ok(a) = u64::from_str_radix(hex, 16) else {
        crate::print_error!("  '{}' is not hex", hex);
        return;
    };
    match crate::process::crash::symbolize_file(path, a) {
        Some((name, off)) => println!("  {:#x}  {}+{:#x}", a, name, off),
        None => println!("  {:#x}  no symbol covers this address", a),
    }
}

fn cores() {
    let list = crate::process::coredump::list();
    if list.is_empty() {
        println!("  no core dumps in /tmp");
        return;
    }
    for (name, size) in &list {
        println!("  /tmp/{}  {} bytes", name, size);
    }
}

fn push_hex2(s: &mut alloc::string::String, b: u8) {
    const D: &[u8; 16] = b"0123456789abcdef";
    s.push(D[(b >> 4) as usize] as char);
    s.push(D[(b & 0xF) as usize] as char);
}
