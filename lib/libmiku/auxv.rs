// The auxiliary vector and the vDSO found through it (`dev/kernel-core.md`,
// "Bottlenecks round")
//
// The kernel puts the auxiliary vector above `envp` on a new program's stack.
// ld-miku walks it for the program's headers and hands it here
// (`miku_set_auxv`) before any constructor runs, as glibc's `ld.so` does
// through `_dl_auxv`.
//
// libmiku wants `AT_SYSINFO_EHDR`: the vDSO, whose `__vdso_clock_gettime`
// answers from a TSC reading and a published anchor without entering the
// kernel. `std`'s `Instant::now` is `miku_clock_gettime`.
//
// `lib/miku/src/time.rs` asks this file.

use core::sync::atomic::{AtomicUsize, Ordering};

const AT_NULL: u64 = 0;
pub const AT_SYSINFO_EHDR: u64 = 33;

/// The auxiliary vector's first pair, from ld-miku; 0 until given
static AUXV: AtomicUsize = AtomicUsize::new(0);

/// From ld-miku, once, before the program's constructors: where the auxiliary
/// vector is
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_set_auxv(auxv: *const u64) {
    AUXV.store(auxv as usize, Ordering::Release);
}

/// Value of `key` in the auxiliary vector, or 0: `getauxval`'s answer
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getauxval(key: u64) -> u64 {
    let mut p = AUXV.load(Ordering::Acquire) as *const u64;
    if p.is_null() {
        return 0;
    }
    // Upper bound on a vector the kernel wrote
    for _ in 0..64 {
        let (k, v) = unsafe { (*p, *p.add(1)) };
        if k == AT_NULL {
            return 0;
        }
        if k == key {
            return v;
        }
        p = unsafe { p.add(2) };
    }
    0
}

#[repr(C)]
pub struct VdsoTimespec {
    pub sec: i64,
    pub nsec: i64,
}

pub type ClockFn = unsafe extern "C" fn(i32, *mut VdsoTimespec) -> i32;
pub type GetCpuFn = unsafe extern "C" fn(*mut u32, *mut u32, *mut u8) -> i32;

/// Resolved function pointers, as usize in an atomic. 0 = not looked up, 1 =
/// looked up and absent, else the address. The middle state keeps a process
/// with no vDSO from parsing on every call
static CLOCK_GETTIME: AtomicUsize = AtomicUsize::new(0);
static GETCPU: AtomicUsize = AtomicUsize::new(0);
const ABSENT: usize = 1;

/// `__vdso_clock_gettime`, or None: no vDSO, or not handed the vector
pub fn clock_gettime() -> Option<ClockFn> {
    // SAFETY: a non-sentinel value is the address `resolve` found
    lookup(&CLOCK_GETTIME).map(|f| unsafe { core::mem::transmute::<usize, ClockFn>(f) })
}

/// `__vdso_getcpu`, or None
pub fn getcpu() -> Option<GetCpuFn> {
    // SAFETY: as above
    lookup(&GETCPU).map(|f| unsafe { core::mem::transmute::<usize, GetCpuFn>(f) })
}

fn lookup(slot: &AtomicUsize) -> Option<usize> {
    let mut v = slot.load(Ordering::Acquire);
    if v == 0 {
        init();
        v = slot.load(Ordering::Acquire);
    }
    (v > ABSENT).then_some(v)
}

/// Both symbols, once. Racing threads do the same work and store the same
/// answers. Nothing is stored before ld-miku hands over the vector, so an early
/// clock read asks again later
fn init() {
    if AUXV.load(Ordering::Acquire) == 0 {
        return;
    }
    let base = miku_getauxval(AT_SYSINFO_EHDR);
    let (clock, cpu) = if base == 0 {
        (0, 0)
    } else {
        // SAFETY: the kernel maps the image at `base` for the process's life
        unsafe { (resolve(base, b"__vdso_clock_gettime"), resolve(base, b"__vdso_getcpu")) }
    };
    CLOCK_GETTIME.store(if clock == 0 { ABSENT } else { clock }, Ordering::Release);
    GETCPU.store(if cpu == 0 { ABSENT } else { cpu }, Ordering::Release);
}

// A minimal ELF reader, just enough for one symbol

const PT_DYNAMIC: u32 = 2;
const DT_NULL: i64 = 0;
const DT_HASH: i64 = 4;
const DT_STRTAB: i64 = 5;
const DT_SYMTAB: i64 = 6;

/// Resolves `name` in the vDSO mapped at `base`.
///
/// The vDSO is not relocated, so every address in its dynamic section is an
/// offset from `base`. The symbol count comes from DT_HASH's `nchain`, the only
/// place a dynamic section records it, so the vDSO is linked with
/// `--hash-style=sysv`.
///
/// An image lacking these tables never matches and returns 0.
unsafe fn resolve(base: u64, name: &[u8]) -> usize {
    unsafe {
        // ELF header: checks the magic before reading any offset
        let ident = base as *const u8;
        if *ident != 0x7F || *ident.add(1) != b'E' || *ident.add(2) != b'L' || *ident.add(3) != b'F' {
            return 0;
        }
        let e_phoff = core::ptr::read_unaligned((base + 32) as *const u64);
        let e_phentsize = core::ptr::read_unaligned((base + 54) as *const u16) as u64;
        let e_phnum = core::ptr::read_unaligned((base + 56) as *const u16) as u64;
        if e_phentsize < 56 || e_phnum == 0 || e_phnum > 64 {
            return 0;
        }

        let mut dyn_va = 0u64;
        for i in 0..e_phnum {
            let ph = base + e_phoff + i * e_phentsize;
            if core::ptr::read_unaligned(ph as *const u32) == PT_DYNAMIC {
                dyn_va = core::ptr::read_unaligned((ph + 16) as *const u64);
                break;
            }
        }
        if dyn_va == 0 {
            return 0;
        }

        let (mut symtab, mut strtab, mut hash) = (0u64, 0u64, 0u64);
        let mut d = base + dyn_va;
        for _ in 0..64 {
            let tag = core::ptr::read_unaligned(d as *const i64);
            let val = core::ptr::read_unaligned((d + 8) as *const u64);
            match tag {
                DT_NULL => break,
                DT_SYMTAB => symtab = val,
                DT_STRTAB => strtab = val,
                DT_HASH => hash = val,
                _ => {}
            }
            d += 16;
        }
        if symtab == 0 || strtab == 0 || hash == 0 {
            return 0;
        }

        // DT_HASH: [nbucket, nchain, ...]. nchain is the symbol count, as each
        // symbol has one chain slot
        let nchain = core::ptr::read_unaligned((base + hash + 4) as *const u32) as u64;
        if nchain == 0 || nchain > 4096 {
            return 0;
        }

        // Elf64_Sym is 24 bytes: st_name(4) st_info(1) st_other(1) st_shndx(2)
        // st_value(8) st_size(8)
        for i in 0..nchain {
            let sym = base + symtab + i * 24;
            let st_name = core::ptr::read_unaligned(sym as *const u32) as u64;
            let st_value = core::ptr::read_unaligned((sym + 8) as *const u64);
            if st_name != 0 && st_value != 0 && streq(base + strtab + st_name, name) {
                return (base + st_value) as usize;
            }
        }
        0
    }
}

/// A NUL-terminated string at `p` against a byte slice, with no allocation or
/// length from the image
unsafe fn streq(p: u64, want: &[u8]) -> bool {
    let s = p as *const u8;
    unsafe {
        for (i, w) in want.iter().enumerate() {
            if *s.add(i) != *w {
                return false;
            }
        }
        *s.add(want.len()) == 0
    }
}
