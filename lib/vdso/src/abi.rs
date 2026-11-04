// vDSO data page layout, included by the kernel (`kernel/process/vdso.rs`) and
// the vDSO (`lib/vdso/src/main.rs`). Read-only and non-executable in every
// process

/// Handshake at the top of the page. Anything else means falling back to the
/// syscall
pub const VDSO_MAGIC: u32 = 0x4D_49_4B_55; // "MIKU"

/// Layout version, checked for equality
pub const VDSO_VERSION: u32 = 2;

/// Time as of `anchor_tsc`, with what is needed to advance it. A seqlock: `seq`
/// is odd during an update, and a reader retries on an odd or changed value
#[repr(C)]
pub struct VdsoData {
    pub magic:   u32,
    pub version: u32,
    /// Odd means update in progress. Read with acquire ordering around the
    /// fields
    pub seq:     u32,
    /// Non-zero once the wall clock is set
    pub have_wall: u32,

    /// TSC frequency in kHz; zero means uncalibrated, and no clock here is
    /// usable
    pub tsc_khz:    u64,
    /// TSC reading at the wall-clock anchor
    pub anchor_tsc: u64,
    /// Wall clock at `anchor_tsc`, split to avoid dividing a nanosecond count
    /// with an epoch in it
    pub wall_sec:   u64,
    pub wall_nsec:  u32,
    /// CPU features (`VDSO_FEATURE_*`), since the vDSO has no writable storage
    /// to cache CPUID
    pub features:   u32,
    /// TSC at boot, for CLOCK_MONOTONIC
    pub boot_tsc:   u64,
}

/// Clock ids, as in the `clock_gettime` syscall
pub const VDSO_CLOCK_REALTIME:  i32 = 0;
pub const VDSO_CLOCK_MONOTONIC: i32 = 1;

/// RDTSCP, needed by `__vdso_getcpu`: CPUID.80000001H:EDX[27]. Absent on QEMU's
/// `qemu64`, where it is #UD
pub const VDSO_FEATURE_RDTSCP: u32 = 1 << 0;

/// The data page is one page below the code page
pub const VVAR_OFFSET: u64 = 0x1000;
