// vDSO: kernel code run in ring 3
//
// Computes `clock_gettime` from a TSC anchor and frequency in a page mapped
// read-only into every process.
//
// * The mapping is shared, so there is no .data or .bss; the linker script
//   discards them, failing the build.
// * Its address changes per exec, so references are PC-relative and the vvar
//   page is one page below the code.
// * The kernel can re-anchor during a read, so reads use a seqlock.
// * An uncalibrated TSC or unset wall clock falls back to the syscall.

#![no_std]
#![no_main]

mod abi;
use abi::*;

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo) -> ! {
    // Unreachable: no allocation, slicing, or unwrap
    loop {
        unsafe { core::arch::asm!("ud2", options(nomem, nostack)) };
    }
}

/// Image base from `__ehdr_start` at offset 0, the header given in
/// AT_SYSINFO_EHDR. A `lea`, since an `extern static` address would need a GOT
/// nobody relocates
#[inline(always)]
fn image_base() -> u64 {
    let base: u64;
    unsafe {
        core::arch::asm!(
            "lea {}, [rip + __ehdr_start]",
            out(reg) base, options(nomem, nostack, preserves_flags),
        );
    }
    base
}

/// The data page, one page below the code
#[inline(always)]
fn vvar() -> *const VdsoData {
    (image_base() - VVAR_OFFSET) as *const VdsoData
}

#[inline(always)]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc", out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// One consistent read of the data page: read `seq`, refuse odd, read the
/// fields, read `seq` again, retry if it moved. Bounded, so a kernel dying
/// mid-update does not hang a process
fn read_data() -> Option<Snapshot> {
    let p = vvar();
    for _ in 0..64 {
        let seq1 = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*p).seq)) };
        if seq1 & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Acquire);

        let s = unsafe {
            Snapshot {
                magic:     core::ptr::read_volatile(core::ptr::addr_of!((*p).magic)),
                version:   core::ptr::read_volatile(core::ptr::addr_of!((*p).version)),
                have_wall: core::ptr::read_volatile(core::ptr::addr_of!((*p).have_wall)),
                tsc_khz:   core::ptr::read_volatile(core::ptr::addr_of!((*p).tsc_khz)),
                anchor:    core::ptr::read_volatile(core::ptr::addr_of!((*p).anchor_tsc)),
                wall_sec:  core::ptr::read_volatile(core::ptr::addr_of!((*p).wall_sec)),
                wall_nsec: core::ptr::read_volatile(core::ptr::addr_of!((*p).wall_nsec)),
                boot_tsc:  core::ptr::read_volatile(core::ptr::addr_of!((*p).boot_tsc)),
            }
        };

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Acquire);
        let seq2 = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*p).seq)) };
        if seq1 != seq2 {
            continue;
        }
        if s.magic != VDSO_MAGIC || s.version != VDSO_VERSION || s.tsc_khz == 0 {
            return None;
        }
        return Some(s);
    }
    None
}

struct Snapshot {
    magic:     u32,
    version:   u32,
    have_wall: u32,
    tsc_khz:   u64,
    anchor:    u64,
    wall_sec:  u64,
    wall_nsec: u32,
    boot_tsc:  u64,
}

/// TSC cycles to (seconds, nanoseconds). Divides first, since
/// `cycles * 1_000_000 / khz` overflows a u64 in about 1.5 hours at 3.7 GHz
#[inline(always)]
fn cycles_to_time(cycles: u64, khz: u64) -> (u64, u64) {
    let per_sec = khz.saturating_mul(1000);
    if per_sec == 0 {
        return (0, 0);
    }
    let secs = cycles / per_sec;
    let rem  = cycles % per_sec;
    // rem < khz*1000, so rem * 1_000_000 < khz * 1e9, about 3.7e15 at 3.7 GHz
    let nsec = rem.saturating_mul(1_000_000) / khz;
    (secs, nsec)
}

#[repr(C)]
pub struct Timespec {
    pub tv_sec:  i64,
    pub tv_nsec: i64,
}

#[repr(C)]
pub struct Timeval {
    pub tv_sec:  i64,
    pub tv_usec: i64,
}

/// `clock_gettime` without entering the kernel. Returns 0 or a negative errno,
/// like the syscall, which is also the fallback
#[no_mangle]
pub unsafe extern "C" fn __vdso_clock_gettime(clock: i32, ts: *mut Timespec) -> i32 {
    if ts.is_null() {
        return -14; // EFAULT
    }
    let Some(d) = read_data() else {
        return syscall_clock_gettime(clock, ts);
    };

    let now = rdtsc();
    let (sec, nsec) = match clock {
        VDSO_CLOCK_MONOTONIC => {
            let delta = now.wrapping_sub(d.boot_tsc);
            cycles_to_time(delta, d.tsc_khz)
        }
        VDSO_CLOCK_REALTIME => {
            if d.have_wall == 0 {
                return syscall_clock_gettime(clock, ts);
            }
            let delta = now.wrapping_sub(d.anchor);
            let (ds, dn) = cycles_to_time(delta, d.tsc_khz);
            let mut sec = d.wall_sec + ds;
            let mut nsec = d.wall_nsec as u64 + dn;
            if nsec >= 1_000_000_000 {
                nsec -= 1_000_000_000;
                sec += 1;
            }
            (sec, nsec)
        }
        // Unknown clocks go to the kernel
        _ => return syscall_clock_gettime(clock, ts),
    };

    (*ts).tv_sec = sec as i64;
    (*ts).tv_nsec = nsec as i64;
    0
}

/// Older interface over the newer one, in microseconds. The timezone argument
/// is ignored
#[no_mangle]
pub unsafe extern "C" fn __vdso_gettimeofday(tv: *mut Timeval, _tz: *mut u8) -> i32 {
    if tv.is_null() {
        return -14;
    }
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    let r = __vdso_clock_gettime(VDSO_CLOCK_REALTIME, &mut ts);
    if r != 0 {
        return r;
    }
    (*tv).tv_sec = ts.tv_sec;
    (*tv).tv_usec = ts.tv_nsec / 1000;
    0
}

/// Current CPU from RDTSCP's auxiliary register; the kernel writes IA32_TSC_AUX
/// on every core (`arch/x86_64/cpu.rs`). A hint, since the thread can migrate.
/// Returns -1 without RDTSCP, per the data page's feature word, which is
/// written once per boot and needs no seqlock
#[no_mangle]
pub unsafe extern "C" fn __vdso_getcpu(cpu: *mut u32, node: *mut u32, _cache: *mut u8) -> i32 {
    let d = vvar();
    if core::ptr::read_volatile(core::ptr::addr_of!((*d).magic)) != VDSO_MAGIC
        || core::ptr::read_volatile(core::ptr::addr_of!((*d).version)) != VDSO_VERSION
        || core::ptr::read_volatile(core::ptr::addr_of!((*d).features)) & VDSO_FEATURE_RDTSCP == 0
    {
        return -1;
    }

    let aux: u32;
    core::arch::asm!(
        "rdtscp",
        out("eax") _, out("edx") _, out("ecx") aux,
        options(nomem, nostack),
    );
    if !cpu.is_null() {
        *cpu = aux & 0xFFF;
    }
    if !node.is_null() {
        // No NUMA
        *node = 0;
    }
    0
}

/// Fallback when the data page is unusable
#[inline(never)]
unsafe fn syscall_clock_gettime(clock: i32, ts: *mut Timespec) -> i32 {
    const SYS_CLOCK_GETTIME: u64 = 70;
    let ret: i64;
    core::arch::asm!(
        "syscall",
        in("rax") SYS_CLOCK_GETTIME,
        in("rdi") clock as i64 as u64,
        in("rsi") ts as u64,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret as i32
}
