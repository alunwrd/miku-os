// The vDSO, from the kernel's side.
//
// `clock_gettime` is arithmetic: a TSC reading, a frequency and an anchor, with
// no need to enter the kernel. This module publishes those numbers. Two pages
// go into every process at exec:
//
//   vvar  read-only, no-execute   the numbers (`VdsoData`)
//   vdso  read + execute          the code that reads them (lib/runtime/vdso)
//
// Both are the same physical frames in every process, refcounted as `solib`
// shares library text. The code page is filled once at boot from the image
// embedded in the kernel; the data page is rewritten whenever the clock is
// re-anchored.
//
// The address is handed to userspace in AT_SYSINFO_EHDR, and the vvar page sits
// directly below the code page, so the vDSO finds the other by subtracting a
// page.

extern crate alloc;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::paging::PageTableFlags;

use crate::mm::{pmm, vmm::AddressSpace};

// The layout of the data page, shared verbatim with `lib/runtime/vdso`. Included rather
// than re-declared: a mismatch would produce a plausible wrong time
use miku_abi::vvar::*;

/// The built vDSO image, embedded the same way ld-miku is
static VDSO_ELF: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/lib/runtime/vdso/vdso.bin"));

const PAGE_SIZE: u64 = 4096;

/// The two frames, allocated once at boot and shared by every process. Zero
/// means no vDSO, not an error: a system without one is only slower
static CODE_FRAME: AtomicU64 = AtomicU64::new(0);
static VVAR_FRAME: AtomicU64 = AtomicU64::new(0);

/// Builds the shared pages. Called once from `kernel_main`, after the TSC is
/// calibrated: the frequency cannot be filled in later
pub fn init() {
    if CODE_FRAME.load(Ordering::Acquire) != 0 {
        return;
    }

    let Some(info) = image() else {
        crate::serial_println!("[vdso] embedded image is not loadable - disabled");
        return;
    };

    let Some(code) = pmm::alloc_frame() else {
        crate::serial_println!("[vdso] out of memory - disabled");
        return;
    };
    let Some(vvar) = pmm::alloc_frame() else {
        pmm::free_frame(code);
        crate::serial_println!("[vdso] out of memory - disabled");
        return;
    };

    // Copies the image into the code frame through the HHDM. It is one PT_LOAD
    // at vaddr 0 by construction (see lib/runtime/vdso/vdso.ld), so the image and the
    // page are the same span
    let hhdm = crate::arch::x86_64::grub::hhdm();
    unsafe {
        let dst = (code + hhdm) as *mut u8;
        core::ptr::write_bytes(dst, 0, PAGE_SIZE as usize);
        core::ptr::copy_nonoverlapping(VDSO_ELF.as_ptr(), dst, info.len);
        core::ptr::write_bytes((vvar + hhdm) as *mut u8, 0, PAGE_SIZE as usize);
    }

    CODE_FRAME.store(code, Ordering::Release);
    VVAR_FRAME.store(vvar, Ordering::Release);

    // Fills the data page for the first time. Everything after this is an
    // update through the same path
    publish();

    crate::serial_println!(
        "[vdso] {} bytes, {} symbols, tsc={} kHz",
        info.len, info.symbols, crate::kcore::time::tsc_khz(),
    );
}

/// Whether there is a vDSO to map
pub fn available() -> bool {
    CODE_FRAME.load(Ordering::Acquire) != 0
}

// publishing time //

/// What the vDSO may execute on this machine.
///
/// Asked here and not in the vDSO because the vDSO has no writable storage to
/// cache an answer in, and `__vdso_getcpu` is one instruction. The kernel
/// already asks at bring-up (`arch::x86_64::cpu` skips IA32_TSC_AUX on a part
/// without RDTSCP)
fn features() -> u32 {
    let mut f = 0;
    if crate::arch::x86_64::cpu::supports_rdtscp() {
        f |= VDSO_FEATURE_RDTSCP;
    }
    f
}

/// Rewrites the data page.
///
/// Called at init and each time the wall clock is re-anchored: the RTC at boot,
/// NTP once the network is up. The seqlock protocol is the writer's half of
/// what `lib/runtime/vdso` reads: make `seq` odd, write, make it even again, with a
/// fence on each side so a reader cannot see the fields change before the flag.
///
/// One writer, so no lock between writers; this protects against a reader on
/// another CPU catching the page mid-update
pub fn publish() {
    let vvar = VVAR_FRAME.load(Ordering::Acquire);
    if vvar == 0 {
        return;
    }
    let hhdm = crate::arch::x86_64::grub::hhdm();
    let p = (vvar + hhdm) as *mut VdsoData;

    // The anchor has to be one instant: the TSC and the wall clock are read
    // next to each other and stored together
    let anchor_tsc = crate::kcore::time::rdtsc();
    let have_wall = crate::kcore::clock::is_set();
    let (wall_sec, wall_nsec) = if have_wall {
        (crate::kcore::clock::now_unix(), crate::kcore::clock::now_nanos_frac() as u32)
    } else {
        (0, 0)
    };

    unsafe {
        let seq = core::ptr::read_volatile(core::ptr::addr_of!((*p).seq));
        // Odd: readers back off from here until it is even again
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).seq), seq | 1);
        core::sync::atomic::fence(Ordering::Release);

        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).magic), VDSO_MAGIC);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).version), VDSO_VERSION);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).tsc_khz),
            crate::kcore::time::tsc_khz());
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).boot_tsc),
            crate::kcore::time::boot_tsc());
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).anchor_tsc), anchor_tsc);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).wall_sec), wall_sec);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).wall_nsec), wall_nsec);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).have_wall),
            if have_wall { 1 } else { 0 });
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).features), features());

        core::sync::atomic::fence(Ordering::Release);
        // Even, and two higher than it was: a reader that sampled the old
        // value on either side of this sees them differ and reads again
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*p).seq), (seq | 1) + 1);
    }
}

// mapping it into a process //

/// Maps the vDSO into `aspace` and returns the address of its ELF header, for
/// AT_SYSINFO_EHDR.
///
/// The two pages are placed together, vvar first. `kernel_find_free` picks the
/// location, which differs per process under the same layout randomisation as
/// everything else.
///
/// None when there is no vDSO or no room; the caller leaves AT_SYSINFO_EHDR out
/// and userspace falls back to the syscall.
pub fn map_into(aspace: &AddressSpace) -> Option<u64> {
    let code = CODE_FRAME.load(Ordering::Acquire);
    let vvar = VVAR_FRAME.load(Ordering::Acquire);
    if code == 0 || vvar == 0 {
        return None;
    }

    let span = 2 * PAGE_SIZE;
    let base = crate::mm::mmap::kernel_find_free(aspace.cr3, span)?;
    let vvar_va = base;
    let code_va = base + PAGE_SIZE;

    // Both frames gain a reference before either is mapped: an address space
    // torn down between the two would otherwise free a frame the next process
    // uses
    pmm::ref_inc(vvar);
    pmm::ref_inc(code);

    let ro_nx = PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_EXECUTE;
    let rx    = PageTableFlags::USER_ACCESSIBLE;

    if !aspace.map_page(vvar_va, vvar, ro_nx) {
        pmm::free_frame(vvar);
        pmm::free_frame(code);
        return None;
    }
    if !aspace.map_page(code_va, code, rx) {
        aspace.unmap_page_no_free(vvar_va);
        pmm::free_frame(vvar);
        pmm::free_frame(code);
        return None;
    }

    // These two frames must never be swapped out: they are shared by every
    // process, and the kernel writes the data page through the HHDM. `map_page`
    // tracks a user mapping as swappable, so this undoes that
    crate::mm::swap_map::set_pinned(vvar, true);
    crate::mm::swap_map::set_pinned(code, true);

    // Registered so a crash report can name these pages, and so nothing
    // else is ever handed this range by mmap
    crate::mm::mmap::kernel_register_vma(
        aspace.cr3, vvar_va, code_va, crate::mm::mmap::PROT_READ,
    );
    crate::mm::mmap::kernel_register_vma(
        aspace.cr3, code_va, code_va + PAGE_SIZE,
        crate::mm::mmap::PROT_READ | crate::mm::mmap::PROT_EXEC,
    );

    Some(code_va)
}

// what is in the image //

struct ImageInfo {
    len:     usize,
    symbols: usize,
}

/// Sanity-checks the embedded image and counts what it exports. Runs once at
/// boot: a vDSO that does not parse is a build problem
fn image() -> Option<ImageInfo> {
    let info = crate::process::elf::parse(VDSO_ELF).ok()?;
    if !info.is_dyn || info.has_interp() {
        return None;
    }
    // One PT_LOAD, at vaddr 0, inside a page. The kernel maps this by hand, so
    // anything else would be truncated instead of rejected
    if info.load.len() != 1 {
        return None;
    }
    let ph = info.phdrs[info.load[0]];
    let (vaddr, off, filesz, memsz) = (ph.p_vaddr, ph.p_offset, ph.p_filesz, ph.p_memsz);
    if vaddr != 0 || off != 0 || memsz > PAGE_SIZE || filesz != memsz {
        return None;
    }
    if filesz as usize > VDSO_ELF.len() {
        return None;
    }

    let symbols = crate::process::symbols::SymbolTable::from_file(VDSO_ELF)
        .map(|t| t.len())
        .unwrap_or(0);
    Some(ImageInfo { len: filesz as usize, symbols })
}

/// The embedded image, for `elf info` and the selftest
pub fn image_bytes() -> &'static [u8] {
    VDSO_ELF
}
