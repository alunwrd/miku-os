// Writing what a process looked like at the moment it died.
//
// The crash report says where a process died; a core says what it was doing:
// registers, the stack that led to the fault, the writable data the program had
// built up, in a file a debugger can read after the process is gone.
//
// The file is a normal ET_CORE ELF, laid out as any other x86_64 core: PT_NOTE
// first with the register set and the mapping table, then one PT_LOAD per
// region worth keeping.
//
// The constraint is space: cores land in /tmp, which is RAM. A dump is a choice
// of memory. The budget is spent in priority order (notes, the stack around
// RSP, writable anonymous memory), and anything that did not fit is a PT_LOAD
// with p_filesz=0, the standard way to say the region existed and its contents
// are not here, so a truncated core stays valid.
//
// Read-only file-backed regions are skipped: their contents are in the binary
// the NT_FILE note points at.

extern crate alloc;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::vfs::address_space::AddressSpace as VfsSpace;
use crate::fs::vfs::types::{DirEntry, FileMode};
use crate::mm::mmap::{Vma, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::process::crash::Regs;
use crate::process::elf::{
    ELFCLASS64, ELFDATA2LSB, ELFOSABI_NONE, ELF_MAGIC, EM_X86_64, EV_CURRENT,
    Elf64Ehdr, Elf64Phdr, PF_R, PF_W, PF_X, PT_LOAD, PT_NOTE,
};

/// ELF file type for a core image. Not in `elf.rs`: the loader must not accept
/// one
const ET_CORE: u16 = 4;

/// Note types, in the "CORE" namespace. NT_FILE is what ties an address in
/// the dump back to the file it was mapped from
const NT_PRSTATUS: u32 = 1;
const NT_PRPSINFO: u32 = 3;
const NT_FILE:     u32 = 0x4649_4C45;
/// Our own note, in a "MIKU" namespace, saying which registers in
/// NT_PRSTATUS are real. See `push_note_miku`
const NT_MIKU_INFO: u32 = 1;

/// Ceiling on a single core: the smaller of what a tmpfs file can hold and 16
/// MiB. /tmp is RAM, and a crash loop writing larger cores would take the
/// machine's memory
const CORE_MAX_BYTES: usize = {
    let file = VfsSpace::max_size() as usize;
    let planned = 16 * 1024 * 1024;
    if file < planned { file } else { planned }
};
/// Of that, how much the stack may claim. The stack is the part a
/// backtrace is read from, so it gets first call on the budget
const STACK_BUDGET: usize = CORE_MAX_BYTES / 2;
/// How many cores /tmp may hold at once. tmpfs is RAM, and a crash loop would
/// otherwise fill it
const CORE_KEEP: usize = 3;
/// Largest number of regions described. A process with more mappings than
/// this has bigger problems than an incomplete NT_FILE
const MAX_REGIONS: usize = 64;

/// Cores this boot, oldest first. Only files we wrote ourselves are ever
/// reaped - something a user put in /tmp is not ours to delete
static WRITTEN: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Writes a core for a process about to be killed.
///
/// Called from the fault handlers after the crash report, on the faulting
/// process's own CR3 while it is alive: the memory dumped is only reachable
/// through its page tables. Every failure here is non-fatal: the process still
/// gets killed
pub fn on_crash(pid: u64, regs: &Regs) {
    let cr3 = crate::syscall::usercopy::current_cr3();
    if cr3 == 0 || cr3 == crate::mm::vmm::kernel_cr3() {
        return;
    }

    let (exe, _bias) = match crate::process::image::path_and_bias(pid) {
        Some(v) => v,
        // No image record means an address space the kernel did not load:
        // nothing to tie a dump to, and NT_FILE would be empty
        None => return,
    };
    let name = basename(&exe);

    let body = match build(pid, cr3, regs, &exe, name) {
        Some(b) => b,
        None => return,
    };

    let mut path = String::from("/tmp/core.");
    path.push_str(name);
    path.push('.');
    push_dec(&mut path, pid);

    match write_out(&path, &body) {
        Ok(()) => crate::serial_println!(
            "[core] {} ({} bytes)", path, body.len(),
        ),
        Err(e) => crate::serial_println!("[core] {} failed: {}", path, e),
    }
}

// building the image //

/// One region as it will appear in the file: where it lived, and how much of it
/// is kept
struct Region {
    vaddr:  u64,
    memsz:  u64,
    /// Source address of the bytes stored, which is `vaddr` except for the
    /// stack, where only the live end is kept
    from:   u64,
    filesz: usize,
    flags:  u32,
}

fn build(pid: u64, cr3: u64, regs: &Regs, exe: &str, name: &str) -> Option<Vec<u8>> {
    let vmas = crate::mm::mmap::vma_snapshot(cr3);
    if vmas.is_empty() {
        return None;
    }

    let regions = plan(&vmas, regs);
    let notes = build_notes(pid, regs, exe, name, &vmas);

    // Header, then one phdr for the notes and one per region. The sizes
    // have to be known before any of it can be written, because a phdr
    // holds the file offset of its own contents
    let ehsz = core::mem::size_of::<Elf64Ehdr>();
    let phsz = core::mem::size_of::<Elf64Phdr>();
    let phnum = regions.len() + 1;
    let hdr_bytes = ehsz + phnum * phsz;

    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(hdr_bytes + notes.len() + STACK_BUDGET).ok()?;
    out.resize(hdr_bytes, 0);

    // Notes go straight after the phdrs, unaligned: a note segment has no
    // alignment requirement beyond the 4-byte one inside it, which the note
    // builder honours
    let notes_off = out.len();
    out.extend_from_slice(&notes);

    let mut phdrs: Vec<Elf64Phdr> = Vec::new();
    phdrs.push(Elf64Phdr {
        p_type:   PT_NOTE,
        p_flags:  0,
        p_offset: notes_off as u64,
        p_vaddr:  0,
        p_paddr:  0,
        p_filesz: notes.len() as u64,
        p_memsz:  0,
        p_align:  4,
    });

    let aspace = crate::mm::vmm::AddressSpace::from_raw(cr3);
    for r in &regions {
        // A region whose contents were not budgeted for is still described,
        // with p_filesz=0. That is the difference between "this memory was
        // empty" and "this memory is not in this file"
        let mut stored = 0usize;
        let off = out.len();
        if r.filesz > 0 && out.len() + r.filesz <= CORE_MAX_BYTES {
            let base = out.len();
            if out.try_reserve(r.filesz).is_ok() {
                out.resize(base + r.filesz, 0);
                stored = aspace.copy_from_user(r.from, &mut out[base..base + r.filesz]);
                // A short read means the region has holes: keep the prefix that
                // was there rather than a tail of invented zeroes
                out.truncate(base + stored);
            }
        }
        phdrs.push(Elf64Phdr {
            p_type:   PT_LOAD,
            p_flags:  r.flags,
            p_offset: if stored > 0 { off as u64 } else { 0 },
            p_vaddr:  if stored > 0 { r.from } else { r.vaddr },
            p_paddr:  0,
            p_filesz: stored as u64,
            p_memsz:  if stored > 0 { stored as u64 } else { r.memsz },
            p_align:  4096,
        });
    }
    let _ = aspace.into_raw();

    // Back-fill the header now that every offset is known
    let ehdr = Elf64Ehdr {
        e_ident:     ident(),
        e_type:      ET_CORE,
        e_machine:   EM_X86_64,
        e_version:   1,
        e_entry:     0,
        e_phoff:     ehsz as u64,
        e_shoff:     0,
        e_flags:     0,
        e_ehsize:    ehsz as u16,
        e_phentsize: phsz as u16,
        e_phnum:     phnum as u16,
        e_shentsize: 0,
        e_shnum:     0,
        e_shstrndx:  0,
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            &ehdr as *const Elf64Ehdr as *const u8, out.as_mut_ptr(), ehsz,
        );
        for (i, ph) in phdrs.iter().enumerate() {
            core::ptr::copy_nonoverlapping(
                ph as *const Elf64Phdr as *const u8,
                out.as_mut_ptr().add(ehsz + i * phsz),
                phsz,
            );
        }
    }
    Some(out)
}

fn ident() -> [u8; 16] {
    let mut id = [0u8; 16];
    id[..4].copy_from_slice(&ELF_MAGIC);
    id[4] = ELFCLASS64;
    id[5] = ELFDATA2LSB;
    id[6] = EV_CURRENT;
    id[7] = ELFOSABI_NONE;
    id
}

/// Decides what goes in and in what order, inside the byte budget.
///
/// Order is priority, not address. The stack is first because it explains the
/// fault; other writable memory follows; read-only file mappings are described
/// but not stored.
fn plan(vmas: &[Vma], regs: &Regs) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    let mut budget = CORE_MAX_BYTES.saturating_sub(8 * 1024); // headers + notes

    // The stack, from the page RSP is in up to the top of its VMA. Below
    // RSP is dead space the program has already left
    let stack = vmas.iter().find(|v| regs.rsp >= v.start && regs.rsp < v.end);
    if let Some(v) = stack {
        let from = (regs.rsp & !0xFFF).max(v.start);
        let want = (v.end - from) as usize;
        let take = want.min(STACK_BUDGET).min(budget);
        budget -= take;
        out.push(Region {
            vaddr: v.start, memsz: v.end - v.start,
            from, filesz: take, flags: prot_to_pf(v.prot),
        });
    }

    for v in vmas.iter().take(MAX_REGIONS) {
        if let Some(s) = stack {
            if s.start == v.start {
                continue;
            }
        }
        if v.prot & PROT_READ == 0 {
            continue;
        }
        // Read-only file-backed memory is already on disk, byte for byte.
        // Describe it so NT_FILE can point at it; do not copy it
        let store = v.prot & PROT_WRITE != 0 && v.backing.is_none();
        let want = (v.end - v.start) as usize;
        let take = if store { want.min(budget) } else { 0 };
        budget -= take;
        out.push(Region {
            vaddr: v.start, memsz: v.end - v.start,
            from: v.start, filesz: take, flags: prot_to_pf(v.prot),
        });
    }
    out
}

fn prot_to_pf(prot: u32) -> u32 {
    let mut f = 0;
    if prot & PROT_READ  != 0 { f |= PF_R; }
    if prot & PROT_WRITE != 0 { f |= PF_W; }
    if prot & PROT_EXEC  != 0 { f |= PF_X; }
    f
}

// notes //

fn build_notes(pid: u64, regs: &Regs, exe: &str, name: &str, vmas: &[Vma]) -> Vec<u8> {
    let mut n = Vec::new();
    push_prstatus(&mut n, pid, regs);
    push_prpsinfo(&mut n, pid, name);
    push_nt_file(&mut n, exe, vmas);
    push_build_id(&mut n, pid);
    push_note_miku(&mut n);
    n
}

/// A note is namesz/descsz/type, then the NUL-terminated name padded to 4,
/// then the descriptor padded to 4
fn push_note(out: &mut Vec<u8>, name: &str, ntype: u32, desc: &[u8]) {
    let namesz = name.len() + 1;
    out.extend_from_slice(&(namesz as u32).to_le_bytes());
    out.extend_from_slice(&(desc.len() as u32).to_le_bytes());
    out.extend_from_slice(&ntype.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    while out.len() % 4 != 0 { out.push(0); }
    out.extend_from_slice(desc);
    while out.len() % 4 != 0 { out.push(0); }
}

/// `elf_prstatus` as x86_64 Linux lays it out: 112 bytes of signal and timing
/// fields, then 27 general registers, then `pr_fpvalid`.
///
/// Only some of those registers are real. An `x86-interrupt` handler is handed
/// the interrupt frame alone, so RIP, RSP, RFLAGS, CS and SS are exact, RBP
/// comes from the frame-pointer chain, and the rest are zero because nothing
/// saved them. `push_note_miku` says so in the file
fn push_prstatus(out: &mut Vec<u8>, pid: u64, regs: &Regs) {
    let mut d = [0u8; 336];
    let sig = 11u32; // SIGSEGV: the only signal that reaches a dump
    d[0..4].copy_from_slice(&sig.to_le_bytes());       // pr_info.si_signo
    d[12..14].copy_from_slice(&(sig as u16).to_le_bytes()); // pr_cursig
    d[32..36].copy_from_slice(&(pid as u32).to_le_bytes()); // pr_pid

    // pr_reg, in user_regs_struct order
    let reg = |i: usize| 112 + i * 8;
    let put = |d: &mut [u8; 336], i: usize, v: u64| {
        d[reg(i)..reg(i) + 8].copy_from_slice(&v.to_le_bytes());
    };
    put(&mut d, 4,  regs.rbp);
    put(&mut d, 16, regs.rip);
    put(&mut d, 17, regs.cs);
    put(&mut d, 18, regs.rflags);
    put(&mut d, 19, regs.rsp);
    put(&mut d, 20, crate::arch::x86_64::gdt::user_data_selector().0 as u64); // ss
    push_note(out, "CORE", NT_PRSTATUS, &d);
}

/// `elf_prpsinfo`: enough for a debugger to print a process name. The fixed
/// offsets come from the x86_64 layout: state/sname/zomb/nice, flags, uid/gid,
/// the four pids, then fname[16] and psargs[80]
fn push_prpsinfo(out: &mut Vec<u8>, pid: u64, name: &str) {
    let mut d = [0u8; 136];
    d[0] = b'R';       // pr_state: running when it died
    d[1] = b'R';       // pr_sname
    d[32..36].copy_from_slice(&(pid as u32).to_le_bytes()); // pr_pid
    let f = name.as_bytes();
    let n = f.len().min(15);
    d[56..56 + n].copy_from_slice(&f[..n]);   // pr_fname
    d[72..72 + n].copy_from_slice(&f[..n]);   // pr_psargs
    push_note(out, "CORE", NT_PRPSINFO, &d);
}

/// NT_FILE: which file each mapping came from, so the read-only segments not
/// stored are recoverable from the binary named here.
///
/// Layout: count, page size, then `count` triples of (start, end, file offset
/// in pages), then `count` NUL-terminated paths.
fn push_nt_file(out: &mut Vec<u8>, exe: &str, vmas: &[Vma]) {
    let mut ents: Vec<(u64, u64, u64)> = Vec::new();
    let mut names: Vec<u8> = Vec::new();
    for v in vmas.iter().take(MAX_REGIONS) {
        let Some(b) = v.backing else { continue };
        ents.push((v.start, v.end, b.file_off >> 12));
        // The vnode a mapping was made from does not carry its path back, so
        // the executable's own path is the one name known; anything else is
        // identified by vnode number
        if b.vnode_id != u32::MAX && exe_vnode(exe) == Some(b.vnode_id) {
            names.extend_from_slice(exe.as_bytes());
        } else {
            names.extend_from_slice(b"[vnode ");
            let mut s = String::new();
            push_dec(&mut s, b.vnode_id as u64);
            names.extend_from_slice(s.as_bytes());
            names.push(b']');
        }
        names.push(0);
    }
    if ents.is_empty() {
        return;
    }

    let mut d: Vec<u8> = Vec::new();
    d.extend_from_slice(&(ents.len() as u64).to_le_bytes());
    d.extend_from_slice(&4096u64.to_le_bytes());
    for (s, e, o) in &ents {
        d.extend_from_slice(&s.to_le_bytes());
        d.extend_from_slice(&e.to_le_bytes());
        d.extend_from_slice(&o.to_le_bytes());
    }
    d.extend_from_slice(&names);
    push_note(out, "CORE", NT_FILE, &d);
}

/// The build-id of the binary, so a dump can be matched to the exact build
/// that produced it. Same note the binary itself carries, copied across
fn push_build_id(out: &mut Vec<u8>, pid: u64) {
    let mut buf = [0u8; crate::process::elf::BUILD_ID_MAX];
    let len = crate::process::image::with(pid, |i| match i.build_id() {
        Some(id) => { buf[..id.len()].copy_from_slice(id); id.len() }
        None => 0,
    }).unwrap_or(0);
    if len == 0 {
        return;
    }
    push_note(out, "GNU", crate::process::elf::NT_GNU_BUILD_ID, &buf[..len]);
}

/// Which of the NT_PRSTATUS registers to trust, as plain text
fn push_note_miku(out: &mut Vec<u8>) {
    const TEXT: &[u8] =
        b"valid-regs=rip,rsp,rbp,rflags,cs,ss; others zero (interrupt frame only)";
    push_note(out, "MIKU", NT_MIKU_INFO, TEXT);
}

/// The vnode the executable resolves to, for matching mappings against it
fn exe_vnode(exe: &str) -> Option<u32> {
    crate::fs::vfs::core::with_vfs(|vfs| {
        vfs.resolve_path(0, exe).ok().map(|id| id as u32)
    })
}

// writing it out //

fn write_out(path: &str, body: &[u8]) -> Result<(), &'static str> {
    reap();

    let name = basename(path);
    crate::fs::vfs::core::with_vfs(|vfs| {
        let dir = vfs.resolve_path(0, "/tmp").map_err(|_| "no /tmp")?;
        // A core for this pid may already exist if the same pid crashed
        // twice; the newer one replaces it rather than appending to it
        let _ = vfs.unlink(0, path);
        let vid = vfs.create_file(dir, name, FileMode::new(0o600))
            .map_err(|_| "create failed")?;
        let n = vfs.write_at_vnode(vid, 0, body).map_err(|_| "write failed")?;
        if n != body.len() {
            return Err("short write");
        }
        Ok(())
    })?;

    let mut w = WRITTEN.lock();
    w.push_back(String::from(path));
    Ok(())
}

/// Keeps /tmp under the core limit. Only files this module wrote are candidates
fn reap() {
    let mut w = WRITTEN.lock();
    while w.len() >= CORE_KEEP {
        let Some(old) = w.pop_front() else { break };
        crate::fs::vfs::core::with_vfs(|vfs| {
            let _ = vfs.unlink(0, &old);
        });
    }
}

/// Cores present in /tmp right now, for `elf` and for anything that wants
/// to know whether a crash left one behind
pub fn list() -> Vec<(String, u64)> {
    let mut out = Vec::new();
    crate::fs::vfs::core::with_vfs(|vfs| {
        let Ok(dir) = vfs.resolve_path(0, "/tmp") else { return };
        let mut ents = [DirEntry::empty(); 32];
        let Ok(n) = vfs.readdir(dir, &mut ents) else { return };
        for e in ents.iter().take(n) {
            let len = e.name_len as usize;
            let Ok(name) = core::str::from_utf8(&e.name[..len]) else { continue };
            if !name.starts_with("core.") {
                continue;
            }
            let size = vfs.stat(dir, name).map(|s| s.size).unwrap_or(0);
            out.push((String::from(name), size));
        }
    });
    out
}

// small helpers //

fn basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

fn push_dec(s: &mut String, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 { break; }
    }
    s.push_str(core::str::from_utf8(&buf[i..]).unwrap_or("?"));
}
