// The CPU-level guards, and the primitive that makes the strictest usable.
//
//   CR0.WP    - the kernel obeys the read-only bit in its own page tables.
//               Without it a write through a user pointer the kernel validated
//               as "mapped" goes through a read-only or copy-on-write page.
//   CR4.SMEP  - ring 0 cannot execute a page marked user-accessible.
//   CR4.SMAP  - ring 0 cannot read or write a user page either, unless
//               RFLAGS.AC is set. `stac`/`clac` do that, wrapped by
//               `usercopy::UserAccess`: a stray dereference of a user pointer
//               elsewhere in the kernel faults.
//
// SMEP and SMAP are enabled when the CPU reports them, and either can be turned
// off from the GRUB command line (`nosmep`, `nosmap`).

use core::sync::atomic::{AtomicBool, Ordering};

static SMEP_ON: AtomicBool = AtomicBool::new(false);
static SMAP_ON: AtomicBool = AtomicBool::new(false);
static WP_ON:   AtomicBool = AtomicBool::new(false);
/// What the CPU says about CET, latched at the first `init_protections`.
/// Separate from whether it is on: "the CPU cannot" differs from "we chose not
/// to"
static IBT_SUP:   AtomicBool = AtomicBool::new(false);
static SHSTK_SUP: AtomicBool = AtomicBool::new(false);
static CET_ON:    AtomicBool = AtomicBool::new(false);

const CR0_WP:   u64 = 1 << 16;
const CR4_SMEP: u64 = 1 << 20;
const CR4_SMAP: u64 = 1 << 21;

/// Whether SMAP is live. Read by `stac`/`clac`, which are #UD on a CPU
/// that does not have it
#[inline(always)]
pub fn smap_enabled() -> bool {
    SMAP_ON.load(Ordering::Relaxed)
}

pub fn smep_enabled() -> bool { SMEP_ON.load(Ordering::Relaxed) }
pub fn wp_enabled()   -> bool { WP_ON.load(Ordering::Relaxed) }

/// Open the SMAP window. Only ever called through `usercopy::UserAccess`,
/// which closes it again on the way out of scope
#[inline(always)]
pub fn stac() {
    if smap_enabled() {
        unsafe { core::arch::asm!("stac", options(nomem, nostack)) };
    }
}

/// Close it. Idempotent, and safe to call when SMAP is off
#[inline(always)]
pub fn clac() {
    if smap_enabled() {
        unsafe { core::arch::asm!("clac", options(nomem, nostack)) };
    }
}

fn cpu_supports() -> (bool, bool) {
    let max_leaf = core::arch::x86_64::__cpuid(0).eax;
    if max_leaf < 7 {
        return (false, false);
    }
    let r = core::arch::x86_64::__cpuid_count(7, 0);
    (r.ebx & (1 << 7) != 0, r.ebx & (1 << 20) != 0)
}

/// Write this core's number into IA32_TSC_AUX, which RDTSCP returns in ECX
/// alongside the timestamp.
///
/// `__vdso_getcpu` reads it in a single instruction, for ring 3; nothing in the
/// kernel reads it.
///
/// A core without RDTSCP never has the MSR written, and the vDSO must not
/// execute the instruction there: it is #UD, so `process::vdso` publishes
/// `VDSO_FEATURE_RDTSCP`. Every physical x86_64 part has RDTSCP since Nehalem,
/// but QEMU's default `qemu64` model does not, and the builder launches that.
pub fn init_tsc_aux(cpu_id: u32) {
    const IA32_TSC_AUX: u32 = 0xC000_0103;
    if !supports_rdtscp() {
        return;
    }
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_TSC_AUX,
            in("eax") cpu_id,
            in("edx") 0u32,
            options(nostack, preserves_flags),
        );
    }
}

/// CPUID.80000001H:EDX[27]. Public because the vDSO's feature word is built
/// from it
pub fn supports_rdtscp() -> bool {
    let r = core::arch::x86_64::__cpuid(0x8000_0000);
    if r.eax < 0x8000_0001 {
        return false;
    }
    let r = core::arch::x86_64::__cpuid(0x8000_0001);
    r.edx & (1 << 27) != 0
}

/// Control-flow Enforcement, as the CPU reports it.
///
///   IBT   CPUID.(EAX=7,ECX=0):EDX[20] - indirect branches must land on an `endbr64`
///   SHSTK CPUID.(EAX=7,ECX=0):ECX[7]  - a hardware-maintained copy of every return address
fn cet_supports() -> (bool, bool) {
    let max_leaf = core::arch::x86_64::__cpuid(0).eax;
    if max_leaf < 7 {
        return (false, false);
    }
    let r = core::arch::x86_64::__cpuid_count(7, 0);
    (r.edx & (1 << 20) != 0, r.ecx & (1 << 7) != 0)
}

/// Whether CET is enabled on this machine, and what the CPU can do. The loader
/// reads it and does not honour a binary's CET properties on a CPU that cannot
/// enforce them
pub fn cet_state() -> (bool, bool, bool) {
    (
        CET_ON.load(Ordering::Relaxed),
        IBT_SUP.load(Ordering::Relaxed),
        SHSTK_SUP.load(Ordering::Relaxed),
    )
}

/// Called once per CPU, on the BSP and on every AP, after the GDT and IDT are
/// in place. The flags are per-core registers
pub fn init_protections() {
    let (smep_sup, smap_sup) = cpu_supports();
    let want_smep = smep_sup && !crate::arch::x86_64::grub::cmdline_flag("nosmep");
    let want_smap = smap_sup && !crate::arch::x86_64::grub::cmdline_flag("nosmap");

    unsafe {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        core::arch::asm!("mov cr0, {}", in(reg) cr0 | CR0_WP, options(nostack));

        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        let mut new = cr4;
        if want_smep { new |= CR4_SMEP; }
        if want_smap { new |= CR4_SMAP; }
        if new != cr4 {
            core::arch::asm!("mov cr4, {}", in(reg) new, options(nostack));
        }
    }

    // Published only after the write, so nothing calls stac() on a CPU
    // where SMAP is not yet on
    WP_ON.store(true, Ordering::Relaxed);
    SMEP_ON.store(want_smep, Ordering::Relaxed);
    SMAP_ON.store(want_smap, Ordering::Relaxed);

    init_cet();
}

/// CET: detection only.
///
///   IBT requires every indirect branch target in ring 3 to be an `endbr64`. Our programs are not built with `-Z cf-protection`, so turning it on faults the first call through a function pointer.
///
///   Shadow stacks require pages with the "dirty, not writable" encoding, one per thread, plus IA32_PL3_SSP maintenance across every context switch and signal delivery. `mm/vmm.rs` cannot express that page type.
///
/// The state is latched, reported, and shown by `elf info` for any binary
/// declaring the properties. QEMU has no CET at all. `nocet` on the GRUB line
/// forces this path
fn init_cet() {
    let (ibt, shstk) = cet_supports();
    IBT_SUP.store(ibt, Ordering::Relaxed);
    SHSTK_SUP.store(shstk, Ordering::Relaxed);
    CET_ON.store(false, Ordering::Relaxed);
}

/// One line, printed by the BSP, naming what is on. "SMAP is enabled" differs
/// from "the CPU has no SMAP"
pub fn report() {
    let (smep_sup, smap_sup) = cpu_supports();
    crate::serial_println!(
        "[cpu] CR0.WP=on SMEP={} SMAP={}",
        if smep_enabled() { "on" } else if smep_sup { "off (cmdline)" } else { "unsupported" },
        if smap_enabled() { "on" } else if smap_sup { "off (cmdline)" } else { "unsupported" },
    );

    let (_on, ibt, shstk) = cet_state();
    let why = if crate::arch::x86_64::grub::cmdline_flag("nocet") {
        "off (cmdline)"
    } else if !ibt && !shstk {
        "unsupported"
    } else {
        // On hardware with CET, this line is what is left to build
        "available, not enabled (needs endbr64 binaries and shadow-stack pages)"
    };
    crate::serial_println!(
        "[cpu] CET: IBT={} SHSTK={} - {}",
        if ibt { "yes" } else { "no" },
        if shstk { "yes" } else { "no" },
        why,
    );
}
