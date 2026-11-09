extern crate alloc;
use alloc::vec::Vec;
use crate::process::elf::*;
use crate::process::elf_source::ElfSource;
use crate::mm::vmm::{AddressSpace, USER_MIN, USER_END, is_user_va};
use crate::mm::pmm;
use crate::arch::x86_64::grub;
use x86_64::structures::paging::PageTableFlags;

const PAGE_SIZE: u64 = 4096;
const PAGE_MASK: u64 = PAGE_SIZE - 1;

// Process address-space layout (bases are randomized per exec):
//
//   ET_EXEC image   wherever it links (typically 0x40_0000)
//   heap (brk)      image end + random gap (up to 32 MiB)
//   mmap region     0x0000_0001_0000_0000 .. 0x0000_7F00_0000_0000  (mmap.rs)
//   PIE image       0x0000_5555_0000_0000 + ASLR (up to 1 TiB)
//   ld-miku         0x0000_7F00_0000_0000 + ASLR (up to 2 GiB)
//   TLS block       16 MiB below the stack's low end
//   stack           8 MiB VMA ending at 0x0000_7FFF_FFFF_0000 - ASLR (up to 16 MiB)
//
// The PIE image lives inside the mmap region's address range, but every
// image/TLS/stack range is registered in the VMA table, so mmap's find_free
// can never hand out an overlapping window

pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
pub const MAX_ELF_SIZE: usize = 64 * 1024 * 1024;

const PIE_BASE: u64 = 0x0000_5555_0000_0000;
const PIE_ASLR_BITS: u32 = 28; // Linux x86_64 mmap_rnd_bits parity
const INTERP_BASE: u64 = 0x0000_7F00_0000_0000;
const INTERP_ASLR_BITS: u32 = 19; // 2 GiB span
const STACK_ASLR_BITS: u32 = 12; // up to 16 MiB below USER_STACK_TOP
const BRK_ASLR_BITS: u32 = 13; // up to 32 MiB gap between image and heap
const ASLR_STEP: u64 = 0x1000;

/// Total stack VMA (Linux RLIMIT_STACK default). Only the top
/// STACK_EAGER_PAGES are mapped at exec; the rest faults in demand-zero
const STACK_SIZE: u64 = 8 * 1024 * 1024;
const STACK_EAGER_PAGES: usize = 64; // 256 KiB up front for argv/envp/auxv
const TLS_GAP_BELOW_STACK: u64 = 16 * 1024 * 1024;

/// Largest PT_TLS image mapped (also bounds the rounding maths in setup_tls).
///
/// The thread control block at the thread pointer: one ABI-mandated word and
/// the template a thread copies to make another thread's block. The layout is
/// in `lib/runtime/ld-miku/src/loader.rs`, and this is the other implementation of it,
/// since a statically-linked program never meets the loader
use miku_abi::tcb::{TCB_SIZE, TCB_TLS_ALIGN, TCB_TLS_FILESZ, TCB_TLS_IMAGE, TCB_TLS_SIZE};
const TLS_MAX_SIZE: u64 = 64 * 1024 * 1024;

/// Highest address a loaded image may reach.
///
/// The TLS block and the stack are placed after the segments are mapped, at
/// addresses derived from USER_STACK_TOP. An ET_EXEC may name any p_vaddr, so
/// without a ceiling an image linked on top of the stack would have its PTEs
/// overwritten by the stack's map_range, orphaning those frames. Everything the
/// loader reserves for itself is subtracted here
const IMAGE_CEILING: u64 = USER_STACK_TOP
    - (1u64 << STACK_ASLR_BITS) * ASLR_STEP   // stack ASLR slack
    - STACK_SIZE
    - TLS_GAP_BELOW_STACK
    - TLS_MAX_SIZE;

/// Longest PT_INTERP string we will look up
const MAX_INTERP_PATH: usize = 128;
const DEFAULT_INTERP: &str = "/lib/ld-miku.so";

// exec argument budget, Linux-style: bounded by bytes, not fixed slot counts.
// ARG_MAX_VECTOR_ENTRIES matches usercopy::MAX_ARGV_ENTRIES, which is what
// the syscall path will already have enforced; the shell path does not go
// through usercopy, so the same ceiling is applied here
const ARG_MAX_STRING_BYTES: u64 = 128 * 1024;
const ARG_MAX_VECTOR_ENTRIES: usize = 256;
/// Number of (key, value) pairs in the auxv table built by setup_stack,
/// AT_NULL included. Used by the stack budget check
const AUXV_ENTRIES: usize = 21;

/// How the loader fetches an ELF interpreter. A Cow lets the built-in ld-miku,
/// `include_bytes!`d into the kernel image, be handed over borrowed rather than
/// copied on every exec of a dynamic binary
pub type ReadFileFn<'a> = &'a dyn Fn(&str) -> Option<alloc::borrow::Cow<'static, [u8]>>;

/// Longest physically contiguous run tried at once (2 MiB). Bigger runs mean
/// fewer page-table walks; too big and the allocator fails on a fragmented
/// machine
const MAX_MAP_RUN_PAGES: usize = 512;

/// A contiguous run of pages mapped while building the image: `count` pages at
/// virtual `va`, backed by frames starting at `phys`. Runs keep the rollback
/// list at a handful of entries for a multi-megabyte image
pub struct MappedRun {
    pub va:    u64,
    pub phys:  u64,
    pub count: usize,
    /// True when these frames came from `imgcache` and belong to every process
    /// running this binary. Rollback drops a reference to them and never frees
    /// them
    pub shared: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadError {
    Parse(ElfError),
    OutOfMemory,
    MapFailed,
    FileTooLarge,
    SegmentOverlap,
    WxSegment,
    InterpReadFailed,
    InterpLoadFailed,
    RelocFailed,
    ArgvTooLong,
    AddressOutOfRange,
    BadEntry,
    InterpNested,
    InterpNotPic,
    BadInterpPath,
}

impl LoadError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parse(e) => e.as_str(),
            Self::OutOfMemory => "out of memory",
            Self::MapFailed => "page mapping failed",
            Self::FileTooLarge => "ELF file too large",
            Self::SegmentOverlap => "overlapping PT_LOAD segments",
            Self::WxSegment => "W+X segment rejected (W^X policy)",
            Self::InterpReadFailed => "cannot read ELF interpreter",
            Self::InterpLoadFailed => "cannot load ELF interpreter",
            Self::RelocFailed => "relocation failed",
            Self::ArgvTooLong => "argv/envp/auxv too large for user stack",
            Self::AddressOutOfRange => "segment lands outside the user address range",
            Self::BadEntry => "entry point is not inside an executable segment",
            Self::InterpNested => "ELF interpreter has an interpreter of its own",
            Self::InterpNotPic => "ELF interpreter is not position-independent",
            Self::BadInterpPath => "bad PT_INTERP path",
        }
    }
}

pub struct ElfImage {
    pub entry: u64,
    pub exe_entry: u64,
    pub stack_top: u64,
    pub brk: u64,
    pub load_bias: u64,
    pub tls_base: u64,
    pub interp_base: u64,
    pub has_interp: bool,
    /// Biased bounds of the executable's PT_LOAD span, and what its notes
    /// said about it. The caller hands these to `process::image` so a later
    /// crash report can name the function an address belongs to
    pub exe_lo: u64,
    pub exe_hi: u64,
    pub build_id: [u8; crate::process::elf::BUILD_ID_MAX],
    pub build_id_len: usize,
    pub x86_features: u32,
}

/// Undoes every mapping made so far and drops the half-built VMA table. All
/// allocation paths in load() put their (virt, phys) pairs into one shared
/// list, so a failure at any stage releases everything at once: segments,
/// relocated interp pages, the TLS block and the eager stack
fn fail(aspace: &AddressSpace, pages: &[MappedRun], e: LoadError) -> LoadError {
    rollback(aspace, pages);
    crate::mm::mmap::vma_cleanup(aspace.cr3);
    e
}

pub fn load<S: ElfSource + ?Sized>(
    src: &S,
    aspace: &AddressSpace,
    path: &str,
    args: &[&str],
    envs: &[&str],
    read_file: Option<ReadFileFn<'_>>,
    // Who the program will be, for the four id entries in the auxiliary
    // vector and for AT_SECURE. Passed in rather than read from the current
    // process: at this point the caller is still the one running, and for
    // `exec_as` the credential the new image gets is not the caller's
    cred: crate::process::cred::Cred,
) -> Result<ElfImage, LoadError> {
    // No ceiling on the file itself: it is read by range, never held whole,
    // and what it may map is `MAX_IMAGE_MEM`, which `parse` enforces. A
    // binary with its debug info left in is six times its loaded size
    let info = parse(src).map_err(LoadError::Parse)?;
    check_overlaps(&info)?;

    // PT_GNU_STACK asking for an executable stack is refused by policy:
    // the stack is always NX on MikuOS. Log it so a failing binary is
    // diagnosable rather than mysteriously crashing on its first trampoline
    if let Some(fl) = info.gnu_stack_flags() {
        if fl & PF_X != 0 {
            crate::serial_println!("[elf] PT_GNU_STACK requests exec stack - forcing NX");
        }
    }

    let (lo, _) = info.memory_bounds();
    let load_bias = if info.is_dyn {
        let offset = crate::kcore::random::aslr_offset(PIE_ASLR_BITS, ASLR_STEP);
        let base = PIE_BASE + offset;
        base.saturating_sub(lo & !PAGE_MASK)
    } else {
        0
    };

    // Nothing has been mapped yet, so both of these are free rejections, and a
    // hostile image consumes no memory on its way to being refused
    validate_placement(&info, load_bias)?;
    let exe_entry = info.entry.checked_add(load_bias).ok_or(LoadError::BadEntry)?;
    validate_entry(&info, exe_entry, load_bias, "exe")?;

    crate::trace_println!(
        "[elf] {} entry={:#x} bias={:#x}{}{}",
        if info.is_dyn { "PIE" } else { "EXEC" },
        exe_entry,
        load_bias,
        if info.wants_ibt() { " ibt" } else { "" },
        if info.wants_shstk() { " shstk" } else { "" },
    );

    let mut pages: Vec<MappedRun> = Vec::new();

    let t0 = crate::kcore::time::since_boot_ms();
    if let Err(e) = map_all_segments(src, &info, load_bias, aspace, &mut pages, Some(path)) {
        return Err(fail(aspace, &pages, e));
    }
    let map_ms = crate::kcore::time::since_boot_ms().saturating_sub(t0);

    if info.is_dyn {
        if let Err(e) = crate::process::reloc::apply_rela_from_phys(src, &info, load_bias, aspace) {
            crate::serial_println!("[elf] relocation failed: {}", e.as_str());
            return Err(fail(aspace, &pages, LoadError::RelocFailed));
        }
    }

    // skip RELRO for dynamically linked binaries - ld-miku applies it after
    // relocations
    if !info.has_interp() {
        apply_relro(&info, load_bias, aspace);
    }

    register_load_vmas(&info, load_bias, aspace.cr3);

    // info.hi is the highest p_vaddr + p_memsz over PT_LOAD, computed in
    // parse()'s single pass
    let brk = info.hi.saturating_add(load_bias);
    // Random gap between the image and the heap so a heap-relative leak
    // does not reveal image addresses (mirrors Linux brk randomization)
    let brk = (brk.saturating_add(PAGE_MASK) & !PAGE_MASK)
        .saturating_add(crate::kcore::random::aslr_offset(BRK_ASLR_BITS, ASLR_STEP));

    // Stack placement first: the TLS block hangs a fixed gap below it, so
    // both inherit the same per-exec randomization
    let stack_top_va = USER_STACK_TOP
        - crate::kcore::random::aslr_offset(STACK_ASLR_BITS, ASLR_STEP);
    let stack_limit = stack_top_va - STACK_SIZE;

    let tls_base = match setup_tls(src, &info, load_bias, aspace, stack_limit, &mut pages) {
        Ok(b) => b,
        Err(e) => return Err(fail(aspace, &pages, e)),
    };

    let interp = if info.has_interp() {
        match load_interpreter(&info, load_bias, src, aspace, read_file, &mut pages) {
            Ok(r) => Some(r),
            Err(e) => return Err(fail(aspace, &pages, e)),
        }
    } else {
        None
    };

    let interp_base = interp.as_ref().map(|i| i.load_base).unwrap_or(0);
    let jump_entry = interp.as_ref().map(|i| i.entry).unwrap_or(exe_entry);
    let has_interp = interp.is_some();

    let phdr_vaddr = if info.phdr_vaddr != 0 {
        info.phdr_vaddr.saturating_add(load_bias)
    } else {
        // No PT_PHDR: find the LOAD segment covering e_phoff
        let mut fallback = 0u64;
        let phoff = info.ehdr.e_phoff;
        for ph in info.load_segments() {
            // parse() already checked p_offset+p_filesz <= src.len(), but
            // checked_add is used anyway
            let seg_file_end = match ph.p_offset.checked_add(ph.p_filesz) {
                Some(v) => v,
                None    => continue,
            };
            if phoff >= ph.p_offset && phoff < seg_file_end {
                fallback = ph.p_vaddr
                    .saturating_add(load_bias)
                    .saturating_add(phoff - ph.p_offset);
                break;
            }
        }
        fallback
    };

    // Eager stack head: only the pages argv/envp/auxv land on are mapped now;
    // everything below faults in demand-zero through the stack VMA. The
    // unregistered gap under stack_limit is the guard: touching it kills the
    // process instead of growing into the TLS block
    let eager_size = (STACK_EAGER_PAGES as u64) * PAGE_SIZE;
    let eager_base = stack_top_va - eager_size;
    let stack_phys = pmm::alloc_frames(STACK_EAGER_PAGES).ok_or_else(|| {
        fail(aspace, &pages, LoadError::OutOfMemory)
    })?;
    let stack_flags = PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    if !aspace.map_range(eager_base, stack_phys, eager_size, stack_flags) {
        pmm::free_frames(stack_phys, STACK_EAGER_PAGES);
        return Err(fail(aspace, &pages, LoadError::MapFailed));
    }
    pages.push(MappedRun { va: eager_base, phys: stack_phys, count: STACK_EAGER_PAGES, shared: false });
    crate::mm::mmap::kernel_register_vma(
        aspace.cr3, stack_limit, stack_top_va,
        crate::mm::mmap::PROT_READ | crate::mm::mmap::PROT_WRITE,
    );

    let hhdm = grub::hhdm();
    unsafe {
        core::ptr::write_bytes((stack_phys + hhdm) as *mut u8, 0, eager_size as usize);
    }

    // The vDSO goes in before the stack because its address is one of the auxv
    // entries on the stack. It is not part of `pages`: its frames are shared
    // with every other process and released by refcount when the address space
    // is torn down, not by this load's rollback
    let vdso_base = crate::process::vdso::map_into(aspace).unwrap_or(0);

    let stack_top = setup_stack(
        stack_phys, eager_base, stack_top_va, path, args, envs, &info,
        interp_base, exe_entry, phdr_vaddr, vdso_base, cred,
    ).ok_or_else(|| fail(aspace, &pages, LoadError::ArgvTooLong))?;

    // `runs` is the number of contiguous physical runs the image needed: one
    // map_range and one page-table walk each
    crate::trace_println!(
        "[elf] ready: jump={:#x} exe={:#x} sp={:#x} brk={:#x} tls={:#x} interp={:#x} stack={:#x}..{:#x} map={}ms runs={}",
        jump_entry, exe_entry, stack_top, brk, tls_base, interp_base,
        stack_limit, stack_top_va, map_ms, pages.len(),
    );

    Ok(ElfImage {
        entry: jump_entry, exe_entry, stack_top, brk, load_bias,
        tls_base, interp_base, has_interp,
        exe_lo: (info.lo & !PAGE_MASK).saturating_add(load_bias),
        exe_hi: (info.hi.saturating_add(PAGE_MASK) & !PAGE_MASK).saturating_add(load_bias),
        build_id: info.build_id,
        build_id_len: info.build_id_len,
        x86_features: info.x86_features,
    })
}

struct InterpResult {
    entry: u64,
    load_base: u64,
}

/// An interpreter path good enough to hand to the VFS. It comes from the image
/// being loaded, so it is attacker-chosen: requires an absolute path, no "..",
/// no NUL or control bytes, and a sane length. It keeps a crafted PT_INTERP
/// from steering the lookup somewhere the caller never meant
fn interp_path_ok(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_INTERP_PATH || !path.starts_with('/') {
        return false;
    }
    if path.split('/').any(|c| c == "..") {
        return false;
    }
    path.bytes().all(|b| (0x20..0x7F).contains(&b))
}

fn load_interpreter<S: ElfSource + ?Sized>(
    info: &ElfInfo,
    exe_bias: u64,
    src: &S,
    aspace: &AddressSpace,
    read_file: Option<ReadFileFn<'_>>,
    pages: &mut Vec<MappedRun>,
) -> Result<InterpResult, LoadError> {
    let named = info.interp_path(src);
    let path = named.as_deref().unwrap_or(DEFAULT_INTERP);
    if !interp_path_ok(path) {
        crate::serial_println!("[elf] rejecting PT_INTERP path");
        return Err(LoadError::BadInterpPath);
    }
    let read_fn = read_file.ok_or(LoadError::InterpReadFailed)?;
    let idata = read_fn(path).ok_or(LoadError::InterpReadFailed)?;

    crate::trace_println!("[elf] interp: {} ({} bytes)", path, idata.len());

    if idata.len() > MAX_ELF_SIZE {
        return Err(LoadError::InterpLoadFailed);
    }

    let mut iinfo = parse(&*idata).map_err(|_| LoadError::InterpLoadFailed)?;
    check_overlaps(&iinfo).map_err(|_| LoadError::InterpLoadFailed)?;

    // One level only: following a chain of interpreters would recurse on
    // attacker-supplied depth
    if iinfo.has_interp() {
        return Err(LoadError::InterpNested);
    }
    // The interpreter is placed at an address picked here, which only works if
    // it is position-independent
    if !iinfo.is_dyn {
        return Err(LoadError::InterpNotPic);
    }

    let (ilo, _) = iinfo.memory_bounds();
    let interp_target = INTERP_BASE
        + crate::kcore::random::aslr_offset(INTERP_ASLR_BITS, ASLR_STEP);
    // Always relocated to the address chosen: a crafted interpreter must not
    // pick where it lands
    let ibias = interp_target.saturating_sub(ilo & !PAGE_MASK);
    iinfo.load_bias = ibias;

    validate_placement(&iinfo, ibias).map_err(|_| LoadError::InterpLoadFailed)?;

    // And it must not land on the executable. The two are placed from different
    // bases with independent randomization and should never meet, but the
    // segment mapper would overwrite a PTE the image owns if they did
    let (ilo2, ihi2) = iinfo.memory_bounds();
    let exe_lo = (info.lo & !PAGE_MASK).saturating_add(exe_bias);
    let exe_hi = (info.hi.saturating_add(PAGE_MASK) & !PAGE_MASK).saturating_add(exe_bias);
    let int_lo = (ilo2 & !PAGE_MASK).saturating_add(ibias);
    let int_hi = (ihi2.saturating_add(PAGE_MASK) & !PAGE_MASK).saturating_add(ibias);
    if int_lo < exe_hi && int_hi > exe_lo {
        crate::serial_println!(
            "[elf] interp {:#x}..{:#x} overlaps the image {:#x}..{:#x}",
            int_lo, int_hi, exe_lo, exe_hi,
        );
        return Err(LoadError::InterpLoadFailed);
    }
    let ientry = iinfo.entry.checked_add(ibias).ok_or(LoadError::BadEntry)?;
    validate_entry(&iinfo, ientry, ibias, "interp")?;

    // Mapped pages go into the caller's shared list: a failure below or in any
    // later load() stage is unwound in one place by fail()
    map_all_segments(&*idata, &iinfo, ibias, aspace, pages, Some(path))
        .map_err(|_| LoadError::InterpLoadFailed)?;

    // The interpreter's own relocations come from PT_DYNAMIC
    if let Err(e) = crate::process::reloc::apply_rela_from_phys(&*idata, &iinfo, ibias, aspace) {
        crate::serial_println!("[elf] interp relocation failed: {}", e.as_str());
        return Err(LoadError::InterpLoadFailed);
    }

    apply_relro(&iinfo, ibias, aspace);
    register_load_vmas(&iinfo, ibias, aspace.cr3);

    let actual_base = (ilo & !PAGE_MASK) + ibias;
    crate::trace_println!("[elf] interp: entry={:#x} base={:#x}", ientry, actual_base);
    Ok(InterpResult { entry: ientry, load_base: actual_base })
}

/// W^X and overlap, in one linear pass over the PT_LOAD list parse() sorted by
/// p_vaddr: a segment can only overlap its immediate predecessor
fn check_overlaps(info: &ElfInfo) -> Result<(), LoadError> {
    let mut prev_end = 0u64;
    let mut first = true;

    for ph in info.load_segments() {
        if ph.p_memsz == 0 {
            continue;
        }

        if ph.p_flags & PF_W != 0 && ph.p_flags & PF_X != 0 {
            let vaddr = ph.p_vaddr;
            crate::serial_println!("[elf] W+X segment rejected at {:#x}", vaddr);
            return Err(LoadError::WxSegment);
        }

        let start = ph.p_vaddr;
        // parse() already rejects vaddr+memsz wrap, but be defensive
        let end = ph.p_vaddr.checked_add(ph.p_memsz).ok_or(LoadError::SegmentOverlap)?;
        if !first && start < prev_end {
            return Err(LoadError::SegmentOverlap);
        }
        prev_end = end;
        first = false;
    }
    Ok(())
}

/// Every address this image will occupy, after the ASLR bias, has to be inside
/// the user half. A kernel-half address does not become a process-private page:
/// it edits the page tables the kernel is running on, because new_user() shares
/// PML4[256..512] with the kernel root. Checked for PT_LOAD, PT_TLS and
/// PT_GNU_RELRO alike, since each ends up in a mapping call
fn validate_placement(info: &ElfInfo, load_bias: u64) -> Result<(), LoadError> {
    let check = |vaddr: u64, memsz: u64| -> Result<(), LoadError> {
        if memsz == 0 {
            return Ok(());
        }
        let start = vaddr.checked_add(load_bias).ok_or(LoadError::AddressOutOfRange)?;
        let end   = start.checked_add(memsz).ok_or(LoadError::AddressOutOfRange)?;
        // The page-rounded range is what gets mapped, so that is what has to
        // fit
        let page_start = start & !PAGE_MASK;
        let page_end   = end.checked_add(PAGE_MASK).ok_or(LoadError::AddressOutOfRange)?
            & !PAGE_MASK;
        if page_start < USER_MIN || page_end > IMAGE_CEILING || page_end <= page_start {
            crate::serial_println!(
                "[elf] segment {:#x}..{:#x} is outside the loadable range (max {:#x})",
                page_start, page_end, IMAGE_CEILING,
            );
            return Err(LoadError::AddressOutOfRange);
        }
        Ok(())
    };

    for ph in info.load_segments() {
        check(ph.p_vaddr, ph.p_memsz)?;
    }
    if let Some(i) = info.tls {
        let ph = &info.phdrs[i];
        check(ph.p_vaddr, ph.p_memsz)?;
    }
    for &i in info.relro.iter() {
        let ph = &info.phdrs[i];
        check(ph.p_vaddr, ph.p_memsz)?;
    }
    Ok(())
}

/// The address to be put in RIP. `sysret` faults in ring 0 if RIP is
/// non-canonical, so an unvalidated entry from a file is a #GP on the kernel's
/// own stack. It also has to point at something executable, or the process dies
/// on its first instruction
fn validate_entry(info: &ElfInfo, entry: u64, bias: u64, what: &str) -> Result<(), LoadError> {
    if !is_user_va(entry) || !info.is_executable_vaddr(entry, bias) {
        crate::serial_println!("[elf] {} entry {:#x} is not in an executable segment", what, entry);
        return Err(LoadError::BadEntry);
    }
    Ok(())
}

/// `key` identifies the file these segments came from, and indexes the
/// shared-page cache. None disables sharing for this load, for images with no
/// stable name to key on
fn map_all_segments<S: ElfSource + ?Sized>(
    src: &S,
    info: &ElfInfo,
    load_bias: u64,
    aspace: &AddressSpace,
    pages: &mut Vec<MappedRun>,
    key: Option<&str>,
) -> Result<(), LoadError> {
    for ph in info.load_segments() {
        map_load_segment(src, ph, load_bias, aspace, pages, key)?;
    }
    Ok(())
}

/// Register every PT_LOAD range in the VMA table so mmap's find_free never
/// hands out an overlapping window and /proc-style tooling can see the
/// image. Page-rounded neighbours may touch, so ranges are merged first
fn register_load_vmas(info: &ElfInfo, load_bias: u64, cr3: u64) {
    let mut ranges: Vec<(u64, u64, u32)> = Vec::with_capacity(info.load.len());
    for ph in info.load_segments() {
        if ph.p_memsz == 0 {
            continue;
        }
        let va  = match ph.p_vaddr.checked_add(load_bias) { Some(v) => v, None => continue };
        let end = match va.checked_add(ph.p_memsz)        { Some(v) => v, None => continue };
        let start = va & !PAGE_MASK;
        let end   = end.saturating_add(PAGE_MASK) & !PAGE_MASK;
        let mut prot = 0u32;
        if ph.p_flags & PF_R != 0 { prot |= crate::mm::mmap::PROT_READ; }
        if ph.p_flags & PF_W != 0 { prot |= crate::mm::mmap::PROT_WRITE; }
        if ph.p_flags & PF_X != 0 { prot |= crate::mm::mmap::PROT_EXEC; }
        ranges.push((start, end, prot));
    }
    // parse() sorted PT_LOAD by p_vaddr and page-rounding preserves that order,
    // so `ranges` is already sorted
    let mut merged: Vec<(u64, u64, u32)> = Vec::new();
    for r in ranges {
        if let Some(last) = merged.last_mut() {
            if r.0 <= last.1 {
                if r.1 > last.1 { last.1 = r.1; }
                last.2 |= r.2;
                continue;
            }
        }
        merged.push(r);
    }
    for (s, e, p) in merged {
        crate::mm::mmap::kernel_register_vma(cr3, s, e, p);
    }
}

/// Fills one physically contiguous run of frames with a segment's contents.
///
/// The run covers [run_vstart, run_vstart + pages*PAGE_SIZE). The part the file
/// backs is copied in one memcpy; every byte outside it (the head before
/// p_vaddr, the BSS tail, and any shortfall if the file is shorter than
/// p_filesz claimed) is zeroed: a frame comes off the allocator holding what
/// the previous owner left, and any byte neither copied nor zeroed is kernel
/// memory handed to ring 3
fn fill_run<S: ElfSource + ?Sized>(
    src: &S,
    hhdm: u64,
    phys: u64,
    run_vstart: u64,
    run_pages: usize,
    vaddr: u64,
    seg_file_end: u64,
    offset: u64,
) {
    let run_bytes = (run_pages as u64) * PAGE_SIZE;
    let run_vend  = run_vstart + run_bytes;
    let dst       = (phys + hhdm) as *mut u8;

    let copy_vstart = run_vstart.max(vaddr);
    let copy_vend   = run_vend.min(seg_file_end);

    if copy_vend <= copy_vstart {
        unsafe { core::ptr::write_bytes(dst, 0, run_bytes as usize) };
        return;
    }

    // Bytes ahead of the file-backed part (only possible on the first run
    // of a segment whose p_vaddr is not page aligned)
    let head = (copy_vstart - run_vstart) as usize;
    if head > 0 {
        unsafe { core::ptr::write_bytes(dst, 0, head) };
    }

    // parse() guarantees p_offset + p_filesz <= src.len() whenever p_filesz >
    // 0, so this cannot come up short for a well-formed file. The source says
    // how many bytes it gave anyway, since the zero-fill below depends on it: a
    // file can shrink under an exec, and a read can fail
    let want = (copy_vend - copy_vstart) as usize;
    let src_off = offset.saturating_add(copy_vstart - vaddr) as usize;
    let got = src.read_at(src_off, unsafe { core::slice::from_raw_parts_mut(dst.add(head), want) });

    // Everything after what was copied: BSS, plus any shortfall
    let filled = head + got;
    if (filled as u64) < run_bytes {
        unsafe {
            core::ptr::write_bytes(dst.add(filled), 0, run_bytes as usize - filled);
        }
    }
}

/// One page that is already mapped, because two PT_LOAD segments share it
/// after page rounding. The frame stays where it is; the new segment's
/// bytes are copied into it and the two segments' permissions are merged
/// under the W^X rule
fn map_shared_page<S: ElfSource + ?Sized>(
    src: &S,
    aspace: &AddressSpace,
    hhdm: u64,
    pv: u64,
    existing_phys: u64,
    flags: PageTableFlags,
    vaddr: u64,
    seg_file_end: u64,
    offset: u64,
) -> Result<(), LoadError> {
    fill_shared_page(src, hhdm, existing_phys, pv, vaddr, seg_file_end, offset);
    let merged = merge_page_flags(aspace, pv, flags)?;
    aspace.unmap_page_no_free(pv);
    if !aspace.map_page(pv, existing_phys & !PAGE_MASK, merged) {
        return Err(LoadError::MapFailed);
    }
    Ok(())
}

/// Copies this segment's share of an already-populated page. Unlike `fill_run`
/// this must not zero what it does not write: the rest of the page belongs to
/// the neighbouring segment, loaded first
fn fill_shared_page<S: ElfSource + ?Sized>(
    src: &S,
    hhdm: u64,
    phys: u64,
    pv: u64,
    vaddr: u64,
    seg_file_end: u64,
    offset: u64,
) {
    let copy_vstart = pv.max(vaddr);
    let copy_vend   = (pv + PAGE_SIZE).min(seg_file_end);
    if copy_vend <= copy_vstart {
        return;
    }
    let dst_off = (copy_vstart - pv) as usize;
    let want    = (copy_vend - copy_vstart) as usize;
    let src_off = offset.saturating_add(copy_vstart - vaddr) as usize;
    let dst = ((phys & !PAGE_MASK) + hhdm + dst_off as u64) as *mut u8;
    src.read_at(src_off, unsafe { core::slice::from_raw_parts_mut(dst, want) });
}

fn map_load_segment<S: ElfSource + ?Sized>(
    src: &S,
    phdr: &Elf64Phdr,
    load_bias: u64,
    aspace: &AddressSpace,
    pages: &mut Vec<MappedRun>,
    key: Option<&str>,
) -> Result<(), LoadError> {
    if phdr.p_memsz == 0 {
        return Ok(());
    }

    // All arithmetic on the attacker-controlled phdr fields goes through
    // checked_add so a crafted ELF cannot wrap past USER_END or produce a
    // negative page count. validate_placement() has confirmed the whole
    // page-rounded range is in the user half
    let vaddr = phdr.p_vaddr.checked_add(load_bias).ok_or(LoadError::MapFailed)?;
    let filesz = phdr.p_filesz;
    let memsz = phdr.p_memsz;
    let offset = phdr.p_offset;

    let page_start = vaddr & !PAGE_MASK;
    let vaddr_end  = vaddr.checked_add(memsz).ok_or(LoadError::MapFailed)?;
    let page_end   = vaddr_end
        .checked_add(PAGE_MASK).ok_or(LoadError::MapFailed)? & !PAGE_MASK;
    if page_end <= page_start { return Err(LoadError::MapFailed); }
    let num_pages = ((page_end - page_start) / PAGE_SIZE) as usize;
    let seg_file_end = vaddr.checked_add(filesz).ok_or(LoadError::MapFailed)?;

    let mut flags = PageTableFlags::USER_ACCESSIBLE;
    if phdr.p_flags & PF_W != 0 {
        flags |= PageTableFlags::WRITABLE;
    }
    if phdr.p_flags & PF_X == 0 {
        flags |= PageTableFlags::NO_EXECUTE;
    }

    let hhdm = grub::hhdm();

    // PT_LOAD segments are sorted by p_vaddr and checked not to overlap
    // byte-wise, so the only page another segment can already own is one of
    // this segment's two edges, where page rounding made neighbours touch.
    // Probing those two costs two page-table walks per segment
    let mut first = 0usize;
    let mut last  = num_pages;

    if let Some(existing) = aspace.virt_to_phys(page_start) {
        map_shared_page(src, aspace, hhdm, page_start, existing, flags,
                        vaddr, seg_file_end, offset)?;
        first = 1;
    }
    if last > first {
        let tail_pv = page_end - PAGE_SIZE;
        if let Some(existing) = aspace.virt_to_phys(tail_pv) {
            map_shared_page(src, aspace, hhdm, tail_pv, existing, flags,
                            vaddr, seg_file_end, offset)?;
            last -= 1;
        }
    }

    // A segment without PF_W is the same bytes in every process running this
    // binary, so a second load can map the first load's frames. Writable
    // segments never qualify: they are relocated per process, and RELRO makes
    // part of them read-only afterwards, so a shared frame would hand one
    // process's relocations to the others
    let share = phdr.p_flags & PF_W == 0;
    let cache_key = if share { key } else { None };

    let mut cached_hit = false;
    if let Some(k) = cache_key {
        if let Some(runs) = crate::process::imgcache::take(k, phdr.p_vaddr, first, last) {
            let mut ok = true;
            for r in &runs {
                let run_vstart = page_start + (r.page_idx as u64) * PAGE_SIZE;
                if !aspace.map_range(run_vstart, r.phys, (r.count as u64) * PAGE_SIZE, flags) {
                    ok = false;
                    break;
                }
                pages.push(MappedRun {
                    va: run_vstart, phys: r.phys, count: r.count, shared: true,
                });
            }
            if !ok {
                // Whatever mapped is in `pages` and will be unwound by fail();
                // the rest is released here so the references taken above do
                // not outlive the attempt
                crate::process::imgcache::release(&runs);
                return Err(LoadError::MapFailed);
            }
            cached_hit = true;
        }
    }

    // Everything between the edges in physically contiguous runs: one
    // alloc_frames, one fill, one map_range per run
    let mut fresh: Vec<crate::process::imgcache::Run> = Vec::new();
    if !cached_hit {
        let mut idx = first;
        while idx < last {
            let want = (last - idx).min(MAX_MAP_RUN_PAGES);
            let (phys, got) = alloc_run(want).ok_or(LoadError::OutOfMemory)?;
            let run_vstart = page_start + (idx as u64) * PAGE_SIZE;

            fill_run(src, hhdm, phys, run_vstart, got, vaddr, seg_file_end, offset);

            if !aspace.map_range(run_vstart, phys, (got as u64) * PAGE_SIZE, flags) {
                pmm::free_frames(phys, got);
                return Err(LoadError::MapFailed);
            }
            pages.push(MappedRun { va: run_vstart, phys, count: got, shared: false });
            fresh.push(crate::process::imgcache::Run { page_idx: idx, phys, count: got });
            idx += got;
        }

        // Offered to the cache only once mapped and correct. The cache takes
        // its own reference, so these frames outlive this process and the next
        // exec of the same binary maps them directly
        if let Some(k) = cache_key {
            crate::process::imgcache::store(k, phdr.p_vaddr, first, last, &fresh);
        }
    }

    crate::trace_println!(
        "[elf]   LOAD va={:#x} pages={} {}{}{}{}",
        page_start, num_pages,
        if phdr.p_flags & PF_R != 0 { "R" } else { "-" },
        if phdr.p_flags & PF_W != 0 { "W" } else { "-" },
        if phdr.p_flags & PF_X != 0 { "X" } else { "-" },
        if cached_hit { " (shared)" } else { "" },
    );
    Ok(())
}

/// Asks for `want` contiguous frames, halving until the allocator can satisfy
/// it. Physical memory fragments over a machine's uptime, and one big run would
/// turn a loadable image into an OOM
fn alloc_run(want: usize) -> Option<(u64, usize)> {
    let mut n = want;
    while n > 1 {
        if let Some(phys) = pmm::alloc_frames(n) {
            return Some((phys, n));
        }
        n /= 2;
    }
    pmm::alloc_frame().map(|f| (f, 1))
}

fn merge_page_flags(
    aspace: &AddressSpace,
    pv: u64,
    new_flags: PageTableFlags,
) -> Result<PageTableFlags, LoadError> {
    let existing_flags = aspace.get_page_flags(pv).unwrap_or(new_flags);
    let old_w = existing_flags.contains(PageTableFlags::WRITABLE);
    let old_x = !existing_flags.contains(PageTableFlags::NO_EXECUTE);
    let new_w = new_flags.contains(PageTableFlags::WRITABLE);
    let new_x = !new_flags.contains(PageTableFlags::NO_EXECUTE);

    if (old_w || new_w) && (old_x || new_x) {
        crate::serial_println!(
            "[elf] W^X: refusing shared page {:#x} (W={} X={})",
            pv, old_w || new_w, old_x || new_x,
        );
        return Err(LoadError::WxSegment);
    }

    let mut merged = existing_flags | new_flags;
    if existing_flags.contains(PageTableFlags::NO_EXECUTE)
        && new_flags.contains(PageTableFlags::NO_EXECUTE)
    {
        merged |= PageTableFlags::NO_EXECUTE;
    } else {
        merged.remove(PageTableFlags::NO_EXECUTE);
    }
    Ok(merged)
}

fn rollback(aspace: &AddressSpace, pages: &[MappedRun]) {
    for run in pages {
        for i in 0..run.count as u64 {
            aspace.unmap_page_no_free(run.va + i * PAGE_SIZE);
        }
        if run.shared {
            // Shared with every other process running this image. Dropping this
            // load's reference is the whole undo: freeing the frames would pull
            // the text out from under a running program
            crate::process::imgcache::release(&[crate::process::imgcache::Run {
                page_idx: 0, phys: run.phys, count: run.count,
            }]);
        } else {
            pmm::free_frames(run.phys, run.count);
        }
    }
}

/// Re-protects PT_GNU_RELRO ranges read-only after relocations are applied.
///
/// The range is clipped to the image's own PT_LOAD span, since a RELRO header
/// can point anywhere, and the existing PTE flags are masked rather than
/// rebuilt, so bits the mapping carried (COW, PAT) survive. A file may carry
/// several
fn apply_relro(info: &ElfInfo, load_bias: u64, aspace: &AddressSpace) {
    if info.relro.is_empty() || info.lo == u64::MAX {
        return;
    }
    // The image's own page-rounded extent: RELRO outside this does not describe
    // this image
    let img_lo = (info.lo & !PAGE_MASK).saturating_add(load_bias);
    let img_hi = (info.hi.saturating_add(PAGE_MASK) & !PAGE_MASK).saturating_add(load_bias);

    for &i in info.relro.iter() {
        let ph = &info.phdrs[i];
        if ph.p_memsz == 0 {
            continue;
        }

        let raw_start = ph.p_vaddr.saturating_add(load_bias);
        let raw_end   = raw_start
            .saturating_add(ph.p_memsz)
            .saturating_add(PAGE_MASK);
        let start = (raw_start & !PAGE_MASK).max(img_lo);
        let end   = (raw_end   & !PAGE_MASK).min(img_hi);
        if end <= start {
            crate::serial_println!(
                "[elf] RELRO {:#x}..{:#x} is outside the image - ignored",
                raw_start & !PAGE_MASK, raw_end & !PAGE_MASK,
            );
            continue;
        }

        let mut pv = start;
        while pv < end {
            if let Some(phys) = aspace.virt_to_phys(pv) {
                let old = aspace.get_page_flags(pv).unwrap_or(
                    PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE,
                );
                let ro = (old | PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE)
                    & !PageTableFlags::WRITABLE;
                aspace.unmap_page_no_free(pv);
                aspace.map_page(pv & !PAGE_MASK, phys & !PAGE_MASK, ro);
            }
            pv += PAGE_SIZE;
        }
        crate::trace_println!("[elf] RELRO {:#x}..{:#x} -> RO", start, end);
    }
}

/// Map the PT_TLS initialization image + TCB. The block sits a fixed gap
/// below the (randomized) stack low end, so its address changes each exec
fn setup_tls<S: ElfSource + ?Sized>(
    src: &S,
    info: &ElfInfo,
    load_bias: u64,
    aspace: &AddressSpace,
    stack_limit: u64,
    pages: &mut Vec<MappedRun>,
) -> Result<u64, LoadError> {
    let ph = match info.tls {
        Some(i) => &info.phdrs[i],
        None    => return Ok(0),
    };

    let raw_align = ph.p_align.max(8) as usize;
    // Align must be a power of two for the bitmask rounding to work. A page is
    // the ceiling, because the block is page-mapped; asking for more is refused
    // by name, since a `movaps` against a misaligned thread-local would fault
    // far from here
    if !raw_align.is_power_of_two() {
        crate::serial_println!("[elf] PT_TLS p_align={} is not a power of two", raw_align);
        return Err(LoadError::MapFailed);
    }
    if raw_align > 4096 {
        crate::serial_println!(
            "[elf] PT_TLS p_align={} exceeds the page this block is mapped in", raw_align);
        return Err(LoadError::MapFailed);
    }
    let align = raw_align;
    let filesz = ph.p_filesz as usize;
    let memsz  = ph.p_memsz  as usize;
    if memsz as u64 > TLS_MAX_SIZE || filesz > memsz {
        return Err(LoadError::MapFailed);
    }
    // How far below the thread pointer the segment starts: the linker's number,
    // `memsz` plus what keeps the start congruent to `p_vaddr` modulo `p_align`
    // (lld's `getTlsTpOffset`). Rounding `memsz` up to the alignment is the
    // same only for an aligned `p_vaddr`. `ld-miku` has the same formula for a
    // dynamic program
    let p_align = (ph.p_align as usize).max(1);
    let pad = 0usize.wrapping_sub(ph.p_vaddr as usize).wrapping_sub(memsz) & (p_align - 1);
    let tcb_off = memsz.checked_add(pad).ok_or(LoadError::MapFailed)?;
    // The thread pointer itself is aligned; the data sits `tcb_off` below it
    let lead = match tcb_off.checked_add(align - 1) {
        Some(v) => v & !(align - 1),
        None    => return Err(LoadError::MapFailed),
    };
    // The TCB, which is 64 bytes. The x86-64 ABI fixes only the first word (the
    // thread pointer's own value); MikuOS puts there the four numbers a thread
    // needs to build another thread's block. `ld-miku` builds the same
    // sixty-four for a dynamic program, and `lib/runtime/libmiku/core/thread.rs` reads them
    // in both cases
    let block_size = lead.checked_add(TCB_SIZE).ok_or(LoadError::MapFailed)?;
    let num_pages = block_size.checked_add(4095).ok_or(LoadError::MapFailed)? / 4096;
    let map_size = num_pages.checked_mul(4096).ok_or(LoadError::MapFailed)?;

    // stack_limit is page aligned and map_size is a page multiple, so the block
    // base stays aligned. The gap keeps a stack overrun clearing the guard from
    // landing in TLS
    let tls_virt = stack_limit
        .checked_sub(TLS_GAP_BELOW_STACK)
        .and_then(|v| v.checked_sub(map_size as u64))
        .ok_or(LoadError::MapFailed)?;

    let phys = pmm::alloc_frames(num_pages).ok_or(LoadError::OutOfMemory)?;
    let flags = PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    if !aspace.map_range(tls_virt, phys, map_size as u64, flags) {
        pmm::free_frames(phys, num_pages);
        return Err(LoadError::MapFailed);
    }
    pages.push(MappedRun { va: tls_virt, phys, count: num_pages, shared: false });
    crate::mm::mmap::kernel_register_vma(
        aspace.cr3, tls_virt, tls_virt + map_size as u64,
        crate::mm::mmap::PROT_READ | crate::mm::mmap::PROT_WRITE,
    );

    let hhdm = grub::hhdm();
    let offset_us = ph.p_offset as usize;
    let in_bounds = filesz == 0
        || offset_us
            .checked_add(filesz)
            .map(|end| end <= src.len())
            .unwrap_or(false);
    unsafe {
        core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, map_size);
        if filesz > 0 && in_bounds {
            let data = (phys + hhdm + (lead - tcb_off) as u64) as *mut u8;
            src.read_at(offset_us, core::slice::from_raw_parts_mut(data, filesz));
        }
        let tcb_user_va = tls_virt + lead as u64;
        let tcb = (phys + hhdm + lead as u64) as *mut u64;
        // tp+0 is the ABI's; tp+8..tp+32 is the template a thread copies from.
        // The image pointer is the program's own `.tdata` in its mapping,
        // read-only in effect: threads copy out of it, never back
        tcb.write(tcb_user_va);
        tcb.byte_add(TCB_TLS_SIZE).write(tcb_off as u64);
        tcb.byte_add(TCB_TLS_ALIGN).write(align as u64);
        tcb.byte_add(TCB_TLS_IMAGE).write(load_bias.wrapping_add(ph.p_vaddr));
        tcb.byte_add(TCB_TLS_FILESZ).write(filesz as u64);
    }

    let tls_base = tls_virt + lead as u64;
    crate::trace_println!(
        "[elf] TLS: block={:#x} tcb(FS.base)={:#x} filesz={} memsz={}",
        tls_virt, tls_base, filesz, memsz,
    );
    Ok(tls_base)
}

/// CPUID.1:EDX: the feature word Linux hands out as AT_HWCAP on x86_64
fn hwcap() -> u64 {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("edx") edx,
            out("eax") _,
            out("ecx") _,
        );
    }
    edx as u64
}

fn setup_stack(
    eager_phys: u64,
    eager_base: u64,
    stack_top_va: u64,
    path: &str,
    args: &[&str],
    envs: &[&str],
    info: &ElfInfo,
    interp_base: u64,
    exe_entry: u64,
    phdr_vaddr: u64,
    // Address of the vDSO's ELF header, or 0 when there is none, which
    // userspace already handles (a kernel with no vDSO leaves the entry out)
    vdso_base: u64,
    cred: crate::process::cred::Cred,
) -> Option<u64> {
    // Byte budget up front, Linux-style, with no fixed argv/envp slot counts.
    // The eager stack region must hold all strings plus the pointer vectors, so
    // both are checked against explicit limits here, and the sticky overflow
    // flag below backstops any miscount
    if args.len().saturating_add(envs.len()) > ARG_MAX_VECTOR_ENTRIES {
        return None;
    }
    let mut str_bytes: u64 = (path.len() as u64) + 1 + 7 + 16; // execfn + "x86_64\0" + AT_RANDOM
    for a in args { str_bytes = str_bytes.saturating_add(a.len() as u64 + 1); }
    for e in envs { str_bytes = str_bytes.saturating_add(e.len() as u64 + 1); }
    if str_bytes > ARG_MAX_STRING_BYTES {
        return None;
    }

    // The two caps above bound each half independently, and their sum can still
    // exceed the eagerly mapped stack head. The pointer vectors are the other
    // half: argv and envp entries plus their NULLs, the auxv pairs, argc, and
    // slack for the 16-byte alignment steps. Refusing up front gives the caller
    // ArgvTooLong instead of a half-built stack
    let words = (args.len().max(1) as u64 + 1)
        .saturating_add(envs.len() as u64 + 1)
        .saturating_add(AUXV_ENTRIES as u64 * 2)
        .saturating_add(1);
    let need = str_bytes
        .saturating_add(words.saturating_mul(8))
        .saturating_add(32); // alignment slack
    let have = (STACK_EAGER_PAGES as u64) * PAGE_SIZE;
    if need > have {
        crate::serial_println!(
            "[elf] argv/envp/auxv needs {} bytes, eager stack is {}", need, have,
        );
        return None;
    }

    let hhdm = grub::hhdm();
    let virt_base = eager_base;
    let host_base = eager_phys + hhdm;
    let mut sp = stack_top_va;

    // Closures share this sticky overflow flag. A push that would not fit
    // refuses to write and sets it, so the caller rejects the whole exec with
    // ArgvTooLong; truncating would yield attacker-controlled argv pointers
    let overflow = core::cell::Cell::new(false);

    let push_u64 = |sp: &mut u64, val: u64| {
        if overflow.get() { return; }
        if *sp < virt_base + 8 { overflow.set(true); return; }
        *sp -= 8;
        let off = *sp - virt_base;
        unsafe {
            ((host_base + off) as *mut u64).write_unaligned(val);
        }
    };

    let push_bytes = |sp: &mut u64, bytes: &[u8]| -> u64 {
        if overflow.get() { return *sp; }
        let need = bytes.len() as u64;
        if *sp < virt_base + need { overflow.set(true); return *sp; }
        *sp -= need;
        let off = *sp - virt_base;
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (host_base + off) as *mut u8,
                bytes.len(),
            );
        }
        *sp
    };

    let push_cstr = |sp: &mut u64, s: &str| -> u64 {
        if overflow.get() { return *sp; }
        let b = s.as_bytes();
        let need = (b.len() as u64).saturating_add(1);
        if *sp < virt_base + need { overflow.set(true); return *sp; }
        *sp -= 1;
        unsafe {
            ((host_base + (*sp - virt_base)) as *mut u8).write(0);
        }
        *sp -= b.len() as u64;
        let off = *sp - virt_base;
        unsafe {
            core::ptr::copy_nonoverlapping(b.as_ptr(), (host_base + off) as *mut u8, b.len());
        }
        *sp
    };

    let random_va   = push_bytes(&mut sp, &crate::kcore::random::random_bytes_16());
    let execfn_va   = push_cstr(&mut sp, path);
    let platform_va = push_cstr(&mut sp, "x86_64");

    let argc = args.len().max(1);
    let mut argv_va: Vec<u64> = Vec::with_capacity(argc);
    for i in 0..argc {
        argv_va.push(push_cstr(&mut sp, if i < args.len() { args[i] } else { path }));
    }
    let envc = envs.len();
    let mut envp_va: Vec<u64> = Vec::with_capacity(envc);
    for e in envs {
        envp_va.push(push_cstr(&mut sp, *e));
    }

    sp &= !15;

    // In final memory order (low to high): AT_PHDR first, AT_NULL last.
    // Pushed in reverse below since the stack grows down
    let auxv: &[(u64, u64)] = &[
        (AT_PHDR,        phdr_vaddr),
        (AT_PHENT,       info.ehdr.e_phentsize as u64),
        (AT_PHNUM,       info.phdr_count as u64),
        (AT_PAGESZ,      PAGE_SIZE),
        (AT_BASE,        interp_base),
        (AT_FLAGS,       0),
        (AT_ENTRY,       exe_entry),
        // The four ids, from the credential this image runs with: a libc
        // caching these at startup makes permission decisions on them
        (AT_UID,         cred.uid  as u64),
        (AT_EUID,        cred.euid as u64),
        (AT_GID,         cred.gid  as u64),
        (AT_EGID,        cred.egid as u64),
        // This image is running with an identity its starter did not have. With
        // no setuid bit, that can only come from a credential the kernel handed
        // over: mikuD starting a service as a user, or mikudo brokering. A libc
        // reads this flag to decide to ignore LD_* and friends
        (AT_SECURE,      ((cred.euid != cred.uid) || (cred.egid != cred.gid)) as u64),
        (AT_RANDOM,      random_va),
        (AT_HWCAP,       hwcap()),
        (AT_HWCAP2,      0),
        // AT_CLKTCK is clock ticks per second; must match the LAPIC timer
        // frequency announced in MikuOS_ABI.md §3.3 (250 Hz)
        (AT_CLKTCK,      crate::arch::x86_64::apic::TIMER_HZ_DEFAULT as u64),
        (AT_PLATFORM,    platform_va),
        (AT_EXECFN,      execfn_va),
        (AT_MINSIGSTKSZ, 2048),
        // Zero means no vDSO, use the syscall, which is what a reader ignoring
        // this entry does
        (AT_SYSINFO_EHDR, vdso_base),
        (AT_NULL,        0),
    ];
    debug_assert_eq!(auxv.len(), AUXV_ENTRIES);

    // Pre-adjust so the final rsp lands 16-byte aligned (SysV: rsp % 16 == 0
    // at process entry, [rsp] = argc). Everything below is 8-byte words:
    // auxv pairs, envp entries + NULL, argv entries + NULL, argc
    let words = (auxv.len() as u64) * 2 + (envc as u64 + 1) + (argc as u64 + 1) + 1;
    if words % 2 == 1 {
        sp -= 8;
    }

    for &(key, val) in auxv.iter().rev() {
        push_u64(&mut sp, val);
        push_u64(&mut sp, key);
    }

    push_u64(&mut sp, 0); // envp NULL terminator
    for i in (0..envc).rev() {
        push_u64(&mut sp, envp_va[i]);
    }
    push_u64(&mut sp, 0); // argv NULL terminator

    for i in (0..argc).rev() {
        push_u64(&mut sp, argv_va[i]);
    }
    push_u64(&mut sp, argc as u64);

    if overflow.get() { None } else { Some(sp) }
}
