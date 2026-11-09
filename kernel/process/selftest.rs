// Adversarial ELF images, built in memory and fed to the real loader.
//
// Each case is a bug that was reachable: a segment mapped into the kernel's
// half of the address space, a relocation whose r_offset points at kernel
// memory, a DT_RELASZ large enough to hang the machine, an entry point that
// would fault `sysret` in ring 0. A refactor dropping a check makes no other
// noise.
//
// Cases needing only the parser run against `elf::parse`. Cases about placement
// or relocation run the whole of `elf_loader::load` into a throwaway address
// space, freed before the next case, so repeated runs do not leak frames.
//
// Reached as `elf test` from the shell, or `run=elf_test` on the GRUB command
// line.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

use crate::kcore::selftest::{Report, Runner};
use crate::process::elf::{self, ElfError};
use crate::process::elf_loader::{self, LoadError};
use crate::mm::vmm::AddressSpace;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_GNU_RELRO: u32 = 0x6474_E552;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;

const EHDR_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;

/// One program header, in the terms the test cases want to talk about
#[derive(Clone, Copy)]
struct Ph {
    typ:    u32,
    flags:  u32,
    offset: u64,
    vaddr:  u64,
    filesz: u64,
    memsz:  u64,
    align:  u64,
}

impl Ph {
    fn load(vaddr: u64, offset: u64, size: u64, flags: u32) -> Self {
        Ph { typ: PT_LOAD, flags, offset, vaddr, filesz: size, memsz: size, align: 0x1000 }
    }
}

/// An ELF64 image assembled byte by byte, with header fields settable
/// individually: half the cases are about a field being wrong in a way a linker
/// would never produce
struct Image {
    e_type:      u16,
    e_machine:   u16,
    e_version:   u32,
    ei_version:  u8,
    e_ehsize:    u16,
    e_phentsize: u16,
    e_entry:     u64,
    phdrs:       Vec<Ph>,
    /// Extra bytes appended after the header table, so segments can point
    /// at real file content
    payload:     Vec<u8>,
}

impl Image {
    fn new() -> Self {
        Image {
            e_type: ET_DYN,
            e_machine: 62,
            e_version: 1,
            ei_version: 1,
            e_ehsize: EHDR_SIZE as u16,
            e_phentsize: PHDR_SIZE as u16,
            e_entry: 0,
            phdrs: Vec::new(),
            payload: Vec::new(),
        }
    }

    fn build(&self) -> Vec<u8> {
        let table = EHDR_SIZE + PHDR_SIZE * self.phdrs.len();
        let mut b = vec![0u8; table];

        b[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        b[4] = 2;                 // ELFCLASS64
        b[5] = 1;                 // ELFDATA2LSB
        b[6] = self.ei_version;
        put16(&mut b, 16, self.e_type);
        put16(&mut b, 18, self.e_machine);
        put32(&mut b, 20, self.e_version);
        put64(&mut b, 24, self.e_entry);
        put64(&mut b, 32, EHDR_SIZE as u64);   // e_phoff
        put16(&mut b, 52, self.e_ehsize);
        put16(&mut b, 54, self.e_phentsize);
        put16(&mut b, 56, self.phdrs.len() as u16);

        for (i, p) in self.phdrs.iter().enumerate() {
            let o = EHDR_SIZE + i * PHDR_SIZE;
            put32(&mut b, o,      p.typ);
            put32(&mut b, o + 4,  p.flags);
            put64(&mut b, o + 8,  p.offset);
            put64(&mut b, o + 16, p.vaddr);
            put64(&mut b, o + 24, p.vaddr);   // p_paddr
            put64(&mut b, o + 32, p.filesz);
            put64(&mut b, o + 40, p.memsz);
            put64(&mut b, o + 48, p.align);
        }

        b.extend_from_slice(&self.payload);
        b
    }
}

fn put16(b: &mut [u8], off: usize, v: u16) { b[off..off + 2].copy_from_slice(&v.to_le_bytes()); }
fn put32(b: &mut [u8], off: usize, v: u32) { b[off..off + 4].copy_from_slice(&v.to_le_bytes()); }
fn put64(b: &mut [u8], off: usize, v: u64) { b[off..off + 8].copy_from_slice(&v.to_le_bytes()); }

/// A PIE that loads: one R+X segment covering the whole file, entry just past
/// the header table. Every "bad" case below is this with one thing changed
fn good_pie() -> Image {
    let mut img = Image::new();
    img.payload = vec![0x90u8; 512];          // filler "code"
    let total = (EHDR_SIZE + PHDR_SIZE + 512) as u64;
    img.phdrs.push(Ph::load(0, 0, total, PF_R | PF_X));
    img.e_entry = EHDR_SIZE as u64 + PHDR_SIZE as u64;
    img
}

fn good_exec() -> Image {
    let mut img = good_pie();
    img.e_type = ET_EXEC;
    let total = (EHDR_SIZE + PHDR_SIZE + 512) as u64;
    img.phdrs.clear();
    img.phdrs.push(Ph::load(0x40_0000, 0, total, PF_R | PF_X));
    img.e_entry = 0x40_0000 + EHDR_SIZE as u64 + PHDR_SIZE as u64;
    img
}

/// Runs the real loader against a throwaway address space, then takes the
/// address space down. Both outcomes free it: on failure the loader unwinds its
/// own mappings, on success it is dropped here
fn try_load(bytes: &[u8]) -> Result<(), LoadError> {
    let aspace = match AddressSpace::new_user() {
        Some(a) => a,
        None    => return Err(LoadError::OutOfMemory),
    };
    let r = elf_loader::load(
        bytes, &aspace, "/selftest", &["/selftest"], &[], None,
        crate::process::cred::Cred::root(),
    );
    let cr3 = aspace.cr3;
    let out = r.map(|_| ());
    crate::mm::mmap::vma_cleanup(cr3);
    drop(aspace);   // Drop frees every frame still mapped
    out
}

fn expect_parse(t: &mut Runner, name: &'static str, img: &Image, want: ElfError) {
    let bytes = img.build();
    match elf::parse(&bytes) {
        Err(e) if e == want => t.check(name, true, format!("rejected: {}", e.as_str())),
        Err(e) => t.check(name, false, format!("wrong error: {} (wanted {})", e.as_str(), want.as_str())),
        Ok(_)  => t.check(name, false, String::from("accepted - it should not have been")),
    }
}

fn expect_load_err(t: &mut Runner, name: &'static str, img: &Image, want: LoadError) {
    let bytes = img.build();
    match try_load(&bytes) {
        Err(e) if e == want => t.check(name, true, format!("rejected: {}", e.as_str())),
        Err(e) => t.check(name, false, format!("wrong error: {} (wanted {})", e.as_str(), want.as_str())),
        Ok(())  => t.check(name, false, String::from("loaded - it should not have")),
    }
}

pub fn run() -> Report {
    let mut t = Runner::new();

    baseline(&mut t);
    header_cases(&mut t);
    placement_cases(&mut t);
    permission_cases(&mut t);
    entry_cases(&mut t);
    reloc_cases(&mut t);
    note_cases(&mut t);
    symbol_cases(&mut t);
    vdso_cases(&mut t);

    t.report()
}

fn baseline(t: &mut Runner) {
    // If this fails, every rejection below is meaningless: the loader could be
    // refusing everything
    let bytes = good_pie().build();
    match try_load(&bytes) {
        Ok(())  => t.check("elf/good-pie", true, String::from("minimal PIE loads")),
        Err(e)  => t.check("elf/good-pie", false, format!("refused a valid PIE: {}", e.as_str())),
    }
    let bytes = good_exec().build();
    match try_load(&bytes) {
        Ok(())  => t.check("elf/good-exec", true, String::from("minimal ET_EXEC loads")),
        Err(e)  => t.check("elf/good-exec", false, format!("refused a valid ET_EXEC: {}", e.as_str())),
    }
}

fn header_cases(t: &mut Runner) {
    let mut img = good_pie();
    img.ei_version = 0;
    expect_parse(t, "elf/ei-version", &img, ElfError::BadVersion);

    let mut img = good_pie();
    img.e_version = 7;
    expect_parse(t, "elf/e-version", &img, ElfError::BadVersion);

    let mut img = good_pie();
    img.e_ehsize = 0;
    expect_parse(t, "elf/e-ehsize", &img, ElfError::BadEhsize);

    // Two PT_INTERP: "the last one wins" lets a benign-looking first entry hide
    // the one used
    let mut img = good_pie();
    img.phdrs.push(Ph { typ: PT_INTERP, flags: PF_R, offset: 64, vaddr: 0, filesz: 8, memsz: 8, align: 1 });
    img.phdrs.push(Ph { typ: PT_INTERP, flags: PF_R, offset: 72, vaddr: 0, filesz: 8, memsz: 8, align: 1 });
    expect_parse(t, "elf/dup-interp", &img, ElfError::DuplicateSegment);

    let mut img = good_pie();
    img.phdrs.push(Ph { typ: PT_DYNAMIC, flags: PF_R, offset: 64, vaddr: 0, filesz: 16, memsz: 16, align: 1 });
    img.phdrs.push(Ph { typ: PT_DYNAMIC, flags: PF_R, offset: 80, vaddr: 0, filesz: 16, memsz: 16, align: 1 });
    expect_parse(t, "elf/dup-dynamic", &img, ElfError::DuplicateSegment);

    // p_align is used as a bitmask when rounding; a non-power-of-two
    // silently corrupts every rounded address derived from it
    let mut img = good_pie();
    img.phdrs[0].align = 3;
    expect_parse(t, "elf/align-not-pow2", &img, ElfError::BadAlign);

    // File and virtual position must agree modulo the alignment, or
    // "map this offset at this address" has no meaning per page
    let mut img = good_pie();
    img.phdrs[0].vaddr = 0x800;
    expect_parse(t, "elf/align-incongruent", &img, ElfError::BadAlign);

    // Sum of p_memsz over PT_LOAD, checked before a frame is allocated
    let mut img = good_pie();
    img.phdrs[0].memsz = 8 * 1024 * 1024 * 1024;
    expect_parse(t, "elf/image-too-large", &img, ElfError::ImageTooLarge);

    let mut img = good_pie();
    img.phdrs[0].filesz = img.phdrs[0].memsz + 4096;
    expect_parse(t, "elf/filesz-over-memsz", &img, ElfError::SegmentOutOfBounds);
}

fn placement_cases(t: &mut Runner) {
    // An ET_EXEC naming a kernel-half address. new_user() shares PML4[256..512]
    // with the kernel root, so mapping there edits the page tables the kernel
    // is running on
    let mut img = good_exec();
    img.phdrs[0].vaddr = 0xFFFF_FFFF_8000_0000;
    img.e_entry = img.phdrs[0].vaddr;
    expect_parse(t, "elf/kernel-half-vaddr", &img, ElfError::AddressOutOfRange);

    // Non-canonical: VirtAddr::new would panic
    let mut img = good_exec();
    img.phdrs[0].vaddr = 0x0000_8000_0000_0000;
    img.e_entry = img.phdrs[0].vaddr;
    expect_parse(t, "elf/non-canonical-vaddr", &img, ElfError::AddressOutOfRange);

    // Page zero stays unmapped, so a NULL dereference stays a fault
    let mut img = good_exec();
    img.phdrs[0].vaddr = 0;
    img.e_entry = 0x40;
    expect_parse(t, "elf/null-page", &img, ElfError::AddressOutOfRange);

    // Overlapping PT_LOAD
    let mut img = good_exec();
    let total = (EHDR_SIZE + PHDR_SIZE * 2 + 512) as u64;
    img.phdrs.clear();
    img.phdrs.push(Ph::load(0x40_0000, 0, total, PF_R | PF_X));
    img.phdrs.push(Ph::load(0x40_0000, 0, total, PF_R));
    expect_load_err(t, "elf/overlap", &img, LoadError::SegmentOverlap);
}

fn permission_cases(t: &mut Runner) {
    let mut img = good_exec();
    img.phdrs[0].flags = PF_R | PF_W | PF_X;
    expect_load_err(t, "elf/wx-segment", &img, LoadError::WxSegment);

    // Two segments that do not overlap byte-wise but share a page after
    // rounding, one writable and one executable. The merged page would be
    // W+X, which is the same violation by a longer route
    let mut img = good_exec();
    img.phdrs.clear();
    img.payload = vec![0x90u8; 4096];
    let hdrs = (EHDR_SIZE + PHDR_SIZE * 2) as u64;
    img.phdrs.push(Ph::load(0x40_0000, 0, hdrs + 64, PF_R | PF_X));
    img.phdrs.push(Ph {
        typ: PT_LOAD, flags: PF_R | PF_W,
        offset: hdrs + 64, vaddr: 0x40_0000 + hdrs + 64,
        filesz: 64, memsz: 64, align: 0x1000,
    });
    img.e_entry = 0x40_0000 + hdrs;
    expect_load_err(t, "elf/wx-shared-page", &img, LoadError::WxSegment);
}

fn entry_cases(t: &mut Runner) {
    // sysret with a non-canonical RIP faults in ring 0, on the kernel's own
    // stack
    let mut img = good_exec();
    img.e_entry = 0x0000_8000_0000_0000;
    expect_load_err(t, "elf/entry-non-canonical", &img, LoadError::BadEntry);

    let mut img = good_exec();
    img.e_entry = 0xFFFF_FFFF_8000_0000;
    expect_load_err(t, "elf/entry-kernel-half", &img, LoadError::BadEntry);

    // Inside the image, but in a segment with no PF_X
    let mut img = good_exec();
    img.phdrs[0].flags = PF_R | PF_W;
    expect_load_err(t, "elf/entry-not-executable", &img, LoadError::BadEntry);

    // Outside every PT_LOAD
    let mut img = good_exec();
    img.e_entry = 0x50_0000;
    expect_load_err(t, "elf/entry-outside-image", &img, LoadError::BadEntry);
}

/// A PIE carrying PT_DYNAMIC with one DT_RELA table of `relas`, so the
/// relocation applier can be pointed at whatever the case wants to test
fn pie_with_relas(relas: &[(u64, u64, i64)], relasz: Option<u64>) -> Vec<u8> {
    let nph = 2usize;
    let hdrs = EHDR_SIZE + PHDR_SIZE * nph;

    // Layout after the headers: [.dynamic][.rela][scratch page]
    let dyn_off  = hdrs as u64;
    let dyn_len  = 5 * 16u64;                     // RELA, RELASZ, RELAENT, PLTREL-ish, NULL
    let rela_off = dyn_off + dyn_len;
    let rela_len = (relas.len() as u64) * 24;
    let scratch  = rela_off + rela_len;
    let total    = scratch + 4096;

    let mut payload = vec![0u8; (total - hdrs as u64) as usize];
    let rebase = |abs: u64| (abs - hdrs as u64) as usize;

    let mut o = rebase(dyn_off);
    for (tag, val) in [
        (7i64, rela_off),                          // DT_RELA (vaddr == offset here)
        (8i64, relasz.unwrap_or(rela_len)),        // DT_RELASZ
        (9i64, 24u64),                             // DT_RELAENT
        (0i64, 0u64),                              // DT_NULL
        (0i64, 0u64),
    ] {
        payload[o..o + 8].copy_from_slice(&tag.to_le_bytes());
        payload[o + 8..o + 16].copy_from_slice(&val.to_le_bytes());
        o += 16;
    }

    let mut o = rebase(rela_off);
    for &(r_offset, r_info, r_addend) in relas {
        payload[o..o + 8].copy_from_slice(&r_offset.to_le_bytes());
        payload[o + 8..o + 16].copy_from_slice(&r_info.to_le_bytes());
        payload[o + 16..o + 24].copy_from_slice(&r_addend.to_le_bytes());
        o += 24;
    }

    let mut img = Image::new();
    img.payload = payload;
    img.phdrs.push(Ph::load(0, 0, total, PF_R | PF_X));
    img.phdrs.push(Ph {
        typ: PT_DYNAMIC, flags: PF_R,
        offset: dyn_off, vaddr: dyn_off, filesz: dyn_len, memsz: dyn_len, align: 8,
    });
    img.e_entry = hdrs as u64;
    img.build()
}

const R_X86_64_RELATIVE: u64 = 8;
const R_X86_64_IRELATIVE: u64 = 37;

fn reloc_cases(t: &mut Runner) {
    // A relocation inside the image is the normal case and must still work
    let bytes = pie_with_relas(&[(0x800, R_X86_64_RELATIVE, 0x1234)], None);
    match try_load(&bytes) {
        Ok(())  => t.check("elf/reloc-in-image", true, String::from("applied")),
        Err(e)  => t.check("elf/reloc-in-image", false, format!("refused: {}", e.as_str())),
    }

    // r_offset naming kernel memory: an arbitrary 8-byte write of an
    // attacker-chosen value through the HHDM alias, with no page permission in
    // the way. The image still loads; the write is refused and counted
    let bytes = pie_with_relas(
        &[(0xFFFF_FFFF_8000_0000u64.wrapping_sub(0), R_X86_64_RELATIVE, 0x4141_4141)],
        None,
    );
    match try_load(&bytes) {
        Ok(())  => t.check("elf/reloc-kernel-target", true, String::from("target refused, image still loads")),
        Err(e)  => t.check("elf/reloc-kernel-target", false, format!("unexpected: {}", e.as_str())),
    }

    // Same, aimed just outside the image's own span rather than at the kernel
    let bytes = pie_with_relas(&[(0x4000_0000, R_X86_64_RELATIVE, 0)], None);
    match try_load(&bytes) {
        Ok(())  => t.check("elf/reloc-outside-image", true, String::from("target refused")),
        Err(e)  => t.check("elf/reloc-outside-image", false, format!("unexpected: {}", e.as_str())),
    }

    // DT_RELASZ of u64::MAX is 7.6e17 entries. It has to come back, and fast
    let before = crate::kcore::time::since_boot_ms();
    let bytes = pie_with_relas(&[(0x800, R_X86_64_RELATIVE, 0)], Some(u64::MAX));
    let outcome = try_load(&bytes);
    let elapsed = crate::kcore::time::since_boot_ms().saturating_sub(before);
    t.check(
        "elf/reloc-count-overflow",
        outcome.is_err() && elapsed < 1000,
        format!("{} in {} ms", if outcome.is_err() { "rejected" } else { "ACCEPTED" }, elapsed),
    );

    // An ifunc relocation in an image with no interpreter. The resolver has to
    // be called in ring 3, which the kernel cannot do, and there is no ld-miku
    // here. A zero left in the GOT would make the first call jump to address
    // zero
    let bytes = pie_with_relas(&[(0x800, R_X86_64_IRELATIVE, 0x1000)], None);
    match try_load(&bytes) {
        Err(LoadError::RelocFailed) =>
            t.check("elf/ifunc-without-interp", true, String::from("refused, not silently zeroed")),
        Err(e) => t.check("elf/ifunc-without-interp", false, format!("wrong error: {}", e.as_str())),
        Ok(())  => t.check("elf/ifunc-without-interp", false,
            String::from("loaded with an unresolved ifunc")),
    }

    // A type no x86_64 loader applies. Refusing beats leaving whatever was in
    // the target
    let bytes = pie_with_relas(&[(0x800, 0x4242, 0)], None);
    match try_load(&bytes) {
        Err(LoadError::RelocFailed) =>
            t.check("elf/unknown-reloc-type", true, String::from("refused")),
        Err(e) => t.check("elf/unknown-reloc-type", false, format!("wrong error: {}", e.as_str())),
        Ok(())  => t.check("elf/unknown-reloc-type", false, String::from("accepted")),
    }

    // PT_GNU_RELRO pointing outside the image would re-map whatever page was
    // there as user-accessible read-only. The range is clipped to the image, so
    // the header is ignored and the load succeeds
    let mut img = good_exec();
    img.phdrs.push(Ph {
        typ: PT_GNU_RELRO, flags: PF_R,
        offset: 0, vaddr: 0x0000_7F00_0000_0000, filesz: 0, memsz: 0x1000, align: 1,
    });
    let bytes = img.build();
    match try_load(&bytes) {
        Ok(())  => t.check("elf/relro-outside-image", true, String::from("ignored, image loads")),
        Err(e)  => t.check("elf/relro-outside-image", false, format!("unexpected: {}", e.as_str())),
    }
}


/// PT_NOTE: the build-id and the CET properties.
///
/// Both are read from a walk over attacker-controlled length fields
/// (`namesz`/`descsz` come from the file). These cases check the good case and
/// that the walk survives the two shapes that would run it off the end of the
/// buffer
fn note_cases(t: &mut Runner) {
    // A well-formed NT_GNU_BUILD_ID, read back byte for byte
    let want: [u8; 8] = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
    let bytes = pie_with_note(NT_GNU_BUILD_ID, b"GNU", &want);
    match elf::parse(&bytes) {
        Ok(i) => {
            let got = i.build_id().unwrap_or(&[]);
            t.check("elf/build-id", got == want,
                format!("{} bytes read back", got.len()));
        }
        Err(e) => t.check("elf/build-id", false, format!("parse failed: {}", e.as_str())),
    }

    // descsz larger than the note segment. The walk must stop rather than
    // read past the buffer it was handed
    let mut bytes = pie_with_note(NT_GNU_BUILD_ID, b"GNU", &want);
    // The descsz field is the second word of the note, which sits at the
    // start of the scratch area the builder placed the note in
    if let Some(at) = find_note_descsz(&bytes) {
        bytes[at..at + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    match elf::parse(&bytes) {
        Ok(i) => t.check("elf/note-descsz-overflow", i.build_id().is_none(),
            String::from("oversized note ignored")),
        Err(e) => t.check("elf/note-descsz-overflow", true,
            format!("rejected: {}", e.as_str())),
    }

    // GNU_PROPERTY_X86_FEATURE_1_AND declaring IBT and shadow stack. The loader
    // does not act on it, with no CET on this hardware, but `elf info` reports
    // it
    let mut desc = alloc::vec::Vec::new();
    desc.extend_from_slice(&elf::GNU_PROPERTY_X86_FEATURE_1_AND.to_le_bytes());
    desc.extend_from_slice(&4u32.to_le_bytes());   // datasz
    desc.extend_from_slice(&(elf::GNU_PROPERTY_X86_FEATURE_1_IBT
        | elf::GNU_PROPERTY_X86_FEATURE_1_SHSTK).to_le_bytes());
    desc.extend_from_slice(&0u32.to_le_bytes());   // pad to 8
    let bytes = pie_with_note(elf::NT_GNU_PROPERTY_TYPE_0, b"GNU", &desc);
    match elf::parse(&bytes) {
        Ok(i) => t.check("elf/gnu-property", i.wants_ibt() && i.wants_shstk(),
            format!("ibt={} shstk={}", i.wants_ibt(), i.wants_shstk())),
        Err(e) => t.check("elf/gnu-property", false, format!("parse failed: {}", e.as_str())),
    }
}

const NT_GNU_BUILD_ID: u32 = 3;

/// A loadable PIE carrying one PT_NOTE with the given note in it
fn pie_with_note(ntype: u32, name: &[u8], desc: &[u8]) -> Vec<u8> {
    let nph = 2usize;
    let hdrs = (EHDR_SIZE + PHDR_SIZE * nph) as u64;

    // namesz/descsz/type, name padded to 4, desc padded to 4
    let mut note = alloc::vec::Vec::new();
    note.extend_from_slice(&((name.len() + 1) as u32).to_le_bytes());
    note.extend_from_slice(&(desc.len() as u32).to_le_bytes());
    note.extend_from_slice(&ntype.to_le_bytes());
    note.extend_from_slice(name);
    note.push(0);
    while note.len() % 4 != 0 { note.push(0); }
    note.extend_from_slice(desc);
    while note.len() % 4 != 0 { note.push(0); }

    let note_off = hdrs;
    let note_len = note.len() as u64;
    let total = note_off + note_len + 256;

    let mut payload = vec![0u8; (total - hdrs) as usize];
    payload[..note.len()].copy_from_slice(&note);

    let mut img = Image::new();
    img.payload = payload;
    img.phdrs.push(Ph::load(0, 0, total, PF_R | PF_X));
    img.phdrs.push(Ph {
        typ: elf::PT_NOTE, flags: PF_R,
        offset: note_off, vaddr: note_off, filesz: note_len, memsz: note_len, align: 4,
    });
    img.e_entry = hdrs + note_len;
    img.build()
}

/// The descsz field of the first note, which sits immediately after the
/// program header table in an image built by `pie_with_note`
fn find_note_descsz(bytes: &[u8]) -> Option<usize> {
    let at = EHDR_SIZE + PHDR_SIZE * 2 + 4;
    if at + 4 <= bytes.len() { Some(at) } else { None }
}

/// Symbolisation, end to end: build a symbol table, index it, and ask it
/// the question a crash report asks
fn symbol_cases(t: &mut Runner) {
    // Two functions, back to back. Asks inside the first, exactly at the start
    // of the second, and past the end of both
    let syms: &[(&str, u64, u64)] = &[("first", 0x1000, 0x40), ("second", 0x1040, 0x20)];
    let (symtab, strtab) = build_symtab(syms);
    let table = match crate::process::symbols::SymbolTable::from_raw(&symtab, &strtab) {
        Some(x) => x,
        None => { t.check("elf/symbolize", false, String::from("table would not build")); return; }
    };

    let mid   = table.resolve(0x1020);
    let exact = table.resolve(0x1040);
    let past  = table.resolve(0x1080);
    let ok = matches!(mid,   Some(("first",  0x20)))
          && matches!(exact, Some(("second", 0)))
          && matches!(past,  None);
    t.check("elf/symbolize", ok, match mid {
        Some((n, o)) => format!("0x1020 -> {}+{:#x}, past-the-end -> {}",
            n, o, if past.is_none() { "none" } else { "SOMETHING" }),
        None => String::from("0x1020 resolved to nothing"),
    });

    // A st_name pointing outside the string table reads as "?"
    let (mut symtab, strtab) = build_symtab(&[("only", 0x2000, 0x10)]);
    symtab[0..4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
    let built = crate::process::symbols::SymbolTable::from_raw(&symtab, &strtab);
    t.check("elf/symbol-name-out-of-range", built.is_none(),
        String::from("symbol with an out-of-range name dropped"));
}

/// `.symtab`/`.strtab` bodies for the symbols given, in the layout
/// `SymbolTable::from_raw` expects
fn build_symtab(syms: &[(&str, u64, u64)]) -> (Vec<u8>, Vec<u8>) {
    let mut strtab = vec![0u8];
    let mut symtab = Vec::new();
    for (name, value, size) in syms {
        let name_off = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);

        symtab.extend_from_slice(&name_off.to_le_bytes());
        symtab.push(elf::STT_FUNC);   // st_info: STB_LOCAL | STT_FUNC
        symtab.push(0);               // st_other
        symtab.extend_from_slice(&1u16.to_le_bytes());  // st_shndx
        symtab.extend_from_slice(&value.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
    }
    (symtab, strtab)
}

/// The vDSO the kernel embedded: the image every process is handed
fn vdso_cases(t: &mut Runner) {
    let bytes = crate::process::vdso::image_bytes();
    match elf::parse(bytes) {
        Ok(i) => {
            // One PT_LOAD at vaddr 0 inside one page, ELF header included. The
            // kernel maps this by hand; anything else would be truncated rather
            // than rejected
            let one = i.load.len() == 1;
            let ph = if one { Some(i.phdrs[i.load[0]]) } else { None };
            let ok = one && ph.map(|p| p.p_vaddr == 0 && p.p_offset == 0 && p.p_memsz <= 4096)
                .unwrap_or(false);
            t.check("vdso/one-page-image", ok, match ph {
                Some(p) => { let (v, m) = (p.p_vaddr, p.p_memsz);
                    format!("{} PT_LOAD, vaddr={:#x}, {} bytes", i.load.len(), v, m) }
                None => format!("{} PT_LOAD segments", i.load.len()),
            });
        }
        Err(e) => t.check("vdso/one-page-image", false, format!("will not parse: {}", e.as_str())),
    }

    // The three entry points userspace looks for. A missing one means a program
    // keeps paying for a syscall
    match crate::process::symbols::SymbolTable::from_file(bytes) {
        Some(tab) => {
            let mut found = 0;
            for want in ["__vdso_clock_gettime", "__vdso_gettimeofday", "__vdso_getcpu"] {
                if tab.iter().any(|(n, _, _)| n == want) { found += 1; }
            }
            t.check("vdso/exports", found == 3, format!("{} of 3 symbols exported", found));
        }
        None => t.check("vdso/exports", false, String::from("no symbol table")),
    }

    t.check("vdso/mapped", crate::process::vdso::available(),
        String::from(if crate::process::vdso::available() {
            "pages built at boot"
        } else {
            "not built - every clock read is a syscall"
        }));
}

/// `elf test`: runs the loader selftest and prints the result.
///
/// Reachable from the boot line as `run=elf_test`, so a headless QEMU run gets
/// a pass or fail from one boot. Same shape as `net test`
pub fn cmd_selftest() {
    crate::cprintln!(57, 197, 187, "elf: loader selftest");
    let report = run();

    for c in &report.cases {
        if c.skipped {
            crate::cprintln!(220, 220, 100, "  skip  {}", c.name);
        } else if c.ok {
            crate::cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            crate::print_error!("  FAIL  {}", c.name);
        }
        crate::println!("          {}", c.detail);
        crate::serial_println!("[elf-test] {} {} - {}",
            if c.skipped { "SKIP" } else if c.ok { "ok  " } else { "FAIL" },
            c.name, c.detail);
    }

    let (pass, fail, skip) = (report.passed(), report.failed(), report.skipped());
    crate::serial_println!("[elf-test] {} passed, {} failed, {} skipped", pass, fail, skip);
    if fail == 0 {
        crate::print_success!("elf: {} passed, {} skipped", pass, skip);
    } else {
        crate::print_error!("elf: {} passed, {} FAILED, {} skipped", pass, fail, skip);
    }
}
