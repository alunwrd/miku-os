// The page GSP-RM writes its numbers into: RM user shared data
// (`dev/gpu-perf.md`)
//
// OpenRM's `gpu_user_shared_data.c`, the part a kernel that is the page's only
// reader keeps:
//
//   link       `gpuCreateRusdMemory_IMPL` + `gpuRusdInitPhysical_KERNEL`: a
//              zeroed page of system memory, its address handed to GSP-RM with
//              `INTERNAL_INIT_USER_SHARED_DATA` on the internal client, then
//              `_gpushareddataInitPollingFrequency`: nothing polled, 500 ms
//   unlink     `_gpushareddataDestroyGsp`: the same control with bInit = 0
//   polling    `_handlePollMaskHelper`: GSP-RM polls only while somebody reads.
//              A reader takes a lease (`touch`) and the GPU event thread, where
//              RPCs are allowed, turns polling on and off (`service`), so
//              nothing on a syscall path waits on GSP-RM
//   reading    `RusdReadData` (`cl00de.h`): copy a section between two reads
//              of its timestamp, kept only when they agree
//
// Every offset is `cl00de.h` of 615 measured with gcc.

use alloc::vec::Vec;
use core::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};

use crate::drivers::gpu::nvidia::gsp_common::rmapi::{RmClient, RmError};
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::le;
use crate::mm::dma::DmaBuffer;

/// `NV2080_CTRL_CMD_INTERNAL_INIT_USER_SHARED_DATA`
pub const NV2080_CTRL_CMD_INTERNAL_INIT_USER_SHARED_DATA: u32 = 0x2080_0afe;
/// `NV2080_CTRL_CMD_INTERNAL_USER_SHARED_DATA_SET_DATA_POLL`
pub const NV2080_CTRL_CMD_INTERNAL_USER_SHARED_DATA_SET_DATA_POLL: u32 = 0x2080_0aff;

/// `sizeof(NV00DE_SHARED_DATA)`
pub const SHARED_DATA_SIZE: usize = 3184;
const _: () = assert!(SHARED_DATA_SIZE <= 4096);

/// One section of the page: where it starts and how much is copied: its
/// timestamp and the fields after it that are read. A prefix is enough: the
/// timestamp guards the whole section
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sec {
    pub off: usize,
    pub len: usize,
}

/// `RUSD_CLK_PUBLIC_DOMAIN_INFOS`, 40 bytes: `{targetClkMHz, actualClkKHz}`
/// for graphics, memory, video and SM
pub const CLOCKS: Sec = Sec { off: 72, len: 40 };
/// `RUSD_CLK_THROTTLE_INFO` (1880 bytes): the timestamp and `reasonMask`
pub const THROTTLE: Sec = Sec { off: 112, len: 12 };
const THROTTLE_SIZE: usize = 1880;
/// `RUSD_PERF_DEVICE_UTILIZATION` (48 bytes): `gpuPercentBusy`,
/// `memoryPercentBusy`
pub const UTIL: Sec = Sec { off: 1992, len: 16 };
const UTIL_SIZE: usize = 48;
/// `RUSD_PERF_CURRENT_PSTATE`: `currentPstate`, one bit as
/// `NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE` gives it
pub const PSTATE: Sec = Sec { off: 2280, len: 12 };
/// `RUSD_POWER_LIMITS`: `requestedmW`, `enforcedmW`
pub const POWER_LIMIT: Sec = Sec { off: 2296, len: 16 };
/// `RUSD_TEMPERATURE temperatures[RUSD_TEMPERATURE_TYPE_MAX]`, 16 bytes each
const TEMPS: usize = 2312;
const TEMP_STRIDE: usize = 16;
/// `RUSD_TEMPERATURE_TYPE_*`: GPU, memory, board, power supply, HBM
pub const TEMP_TYPES: usize = 5;
/// `RUSD_AVG_POWER_USAGE` (24 bytes): `averageGpuPower`, mW
pub const AVG_POWER: Sec = Sec { off: 2472, len: 12 };
/// `RUSD_INST_POWER_USAGE` (24 bytes): `instGpuPower`, mW
pub const INST_POWER: Sec = Sec { off: 2496, len: 12 };
/// `RUSD_POWER_POLICY_STATUS`: `tgpmW`
pub const POWER_POLICY: Sec = Sec { off: 2520, len: 12 };
/// `RUSD_FAN_COOLER_STATUS`, 72 bytes: `rpmCurr[RUSD_FAN_COOLER_MAX_COOLERS]`
pub const FANS: Sec = Sec { off: 2584, len: 72 };
pub const FAN_COOLERS: usize = 16;
/// Where `pciBusData` and `procUtil` start, the neighbours pinning the sections
/// above to their sizes
const PCI_BUS_DATA: usize = 2536;
const PROC_UTIL: usize = 2656;

/// The temperature section of one `RUSD_TEMPERATURE_TYPE`: timestamp, `NvTemp`
pub const fn temp(kind: usize) -> Sec {
    Sec { off: TEMPS + kind * TEMP_STRIDE, len: 12 }
}

// The sections against their neighbours, as gcc laid them out
const _: () = assert!(CLOCKS.off + CLOCKS.len == THROTTLE.off);
const _: () = assert!(THROTTLE.off + THROTTLE_SIZE == UTIL.off);
const _: () = assert!(PSTATE.off + 16 == POWER_LIMIT.off);
const _: () = assert!(POWER_LIMIT.off + POWER_LIMIT.len == TEMPS);
const _: () = assert!(AVG_POWER.off + 24 == INST_POWER.off);
const _: () = assert!(INST_POWER.off + 24 == POWER_POLICY.off);
const _: () = assert!(POWER_POLICY.off + 16 == PCI_BUS_DATA);
const _: () = assert!(FANS.off + FANS.len == PROC_UTIL);
const _: () = assert!(FANS.len == 8 + FAN_COOLERS * 4);
const _: () = assert!(UTIL.off + UTIL_SIZE == 2040);

/// `RUSD_TIMESTAMP_INVALID`: never written
pub const TS_INVALID: u64 = 0;
/// `RUSD_TIMESTAMP_WRITE_IN_PROGRESS`
pub const TS_WRITING: u64 = u64::MAX;
/// `RUSD_SEQ_START`: at or above it the word is a sequence number of data not
/// polled, odd while being written
pub const SEQ_START: u64 = 0xFF00_0000_0000_0000;
/// `RUSD_READ_DATA`'s attempts
const ATTEMPTS: usize = 10;

/// `NV00DE_RUSD_POLL_*`
pub const POLL_CLOCK: u64 = 0x1;
pub const POLL_PERF: u64 = 0x2;
pub const POLL_POWER: u64 = 0x8;
pub const POLL_THERMAL: u64 = 0x10;
pub const POLL_FAN: u64 = 0x40;
/// Everything this driver reads
pub const POLL_MASK: u64 = POLL_CLOCK | POLL_PERF | POLL_POWER | POLL_THERMAL | POLL_FAN;
const _: () = assert!(POLL_MASK == 0x5b);
/// `NV_REG_STR_RM_RUSD_POLLING_INTERVAL_DEFAULT`
pub const INTERVAL_MS: u32 = 500;
/// How long one read keeps GSP-RM polling
pub const LEASE_MS: u64 = 10_000;
/// Older than this, a section is not what the card is doing now
pub const FRESH_MS: u64 = 3 * INTERVAL_MS as u64;
/// A timestamp this close to PTIMER is taken to be in PTIMER's count
const SAME_CLOCK_NS: u64 = 60_000_000_000;

/// `NV2080_CTRL_INTERNAL_INIT_USER_SHARED_DATA_PARAMS`, 16 bytes: `NvBool
/// bInit` at 0 and the page's address at 8. `None` is the destroy
pub fn init_params(phys: Option<u64>) -> [u8; 16] {
    let mut p = [0u8; 16];
    if let Some(a) = phys {
        p[0] = 1;
        le::put_u64(&mut p, 8, a);
    }
    p
}

/// `NV2080_CTRL_INTERNAL_USER_SHARED_DATA_SET_DATA_POLL_PARAMS`, 16 bytes:
/// `polledDataMask` at 0, `pollIntervalMs` at 8
pub fn poll_params(mask: u64, interval_ms: u32) -> [u8; 16] {
    let mut p = [0u8; 16];
    le::put_u64(&mut p, 0, mask);
    le::put_u32(&mut p, 8, interval_ms);
    p
}

/// Where a section is read from: the page GSP-RM writes, or a test's bytes
pub trait Page {
    /// The section's timestamp, read afresh
    fn timestamp(&self, off: usize) -> u64;
    /// `out.len()` bytes from `off`
    fn copy(&self, off: usize, out: &mut [u8]);
}

/// What reading one section gave
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    /// A copy taken whole, and the timestamp it was taken under
    Whole { ts: u64, bytes: Vec<u8> },
    /// GSP-RM never wrote it
    Never,
    /// Still being written, or written over, every time it was copied
    Busy,
}

impl Section {
    /// The copy and its timestamp, if there is one
    pub fn whole(&self) -> Option<(u64, &[u8])> {
        match self {
            Section::Whole { ts, bytes } => Some((*ts, bytes)),
            _ => None,
        }
    }

    /// The 32-bit field `at` bytes into the section
    pub fn u32_at(&self, at: usize) -> Option<u32> {
        self.whole().and_then(|(_, b)| le::try_u32(b, at))
    }
}

/// `RusdReadData`: the section copied between two reads of its timestamp
pub fn read_section(page: &dyn Page, s: Sec) -> Section {
    let mut second = page.timestamp(s.off);
    for _ in 0..ATTEMPTS {
        let ts = second;
        if ts < SEQ_START {
            if ts == TS_INVALID {
                return Section::Never;
            }
            if ts == TS_WRITING {
                second = page.timestamp(s.off);
                continue;
            }
        } else if ts & 1 == 1 {
            second = page.timestamp(s.off);
            continue;
        }
        fence(Ordering::Acquire);
        let mut bytes = alloc::vec![0u8; s.len];
        page.copy(s.off, &mut bytes);
        fence(Ordering::Acquire);
        second = page.timestamp(s.off);
        if second == ts {
            return Section::Whole { ts, bytes };
        }
    }
    Section::Busy
}

/// Every section this driver reads, read once
#[derive(Clone, Debug)]
pub struct Raw {
    pub clocks: Section,
    pub throttle: Section,
    pub util: Section,
    pub pstate: Section,
    pub power_limit: Section,
    pub temps: [Section; TEMP_TYPES],
    pub avg_power: Section,
    pub inst_power: Section,
    pub power_policy: Section,
    pub fans: Section,
}

/// Read every section of `page`
pub fn read_raw(page: &dyn Page) -> Raw {
    Raw {
        clocks: read_section(page, CLOCKS),
        throttle: read_section(page, THROTTLE),
        util: read_section(page, UTIL),
        pstate: read_section(page, PSTATE),
        power_limit: read_section(page, POWER_LIMIT),
        temps: core::array::from_fn(|k| read_section(page, temp(k))),
        avg_power: read_section(page, AVG_POWER),
        inst_power: read_section(page, INST_POWER),
        power_policy: read_section(page, POWER_POLICY),
        fans: read_section(page, FANS),
    }
}

/// How old a polled section is, and by which clock that was decided
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// Its timestamp is PTIMER nanoseconds, and this far behind PTIMER now
    Ptimer(u64),
    /// Its timestamp is in some other count; this long since it last moved
    SinceMoved(u64),
}

impl Age {
    pub fn ms(self) -> u64 {
        match self {
            Age::Ptimer(ms) | Age::SinceMoved(ms) => ms,
        }
    }
}

/// The last timestamp seen on the clock section and when, for `Age::SinceMoved`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub ts: u64,
    pub at_ms: u64,
}

/// `Moved::at_ms` of a timestamp already there when polling began: older than
/// the polling, so never fresh
const NOT_SINCE_POLLING: u64 = u64::MAX;

impl Moved {
    /// `ts` is on the page as polling begins; only a timestamp after it is news
    pub fn unmoved(ts: u64) -> Self {
        Moved { ts, at_ms: NOT_SINCE_POLLING }
    }
}

/// How old a polled timestamp `ts` is. PTIMER is the GPU's clock, set by the
/// boot to wall-clock nanoseconds (`hal_gb20x`), what GSP-RM stamps its records
/// with; a timestamp within a minute of it is taken to be in the same count.
/// Otherwise the age is the time since `ts` last changed, as `last` remembers,
/// and one unchanged since polling began (`Moved::unmoved`) is as old as can
/// be. `None` for a word that is not a polled timestamp
pub fn age(ts: u64, ptimer_ns: Option<u64>, now_ms: u64, last: &mut Moved) -> Option<Age> {
    if ts == TS_INVALID || ts == TS_WRITING || ts >= SEQ_START {
        return None;
    }
    if last.ts != ts {
        *last = Moved { ts, at_ms: now_ms };
    }
    if let Some(pt) = ptimer_ns {
        if pt >= ts && pt - ts < SAME_CLOCK_NS {
            return Some(Age::Ptimer((pt - ts) / 1_000_000));
        }
    }
    if last.at_ms == NOT_SINCE_POLLING {
        return Some(Age::SinceMoved(u64::MAX));
    }
    Some(Age::SinceMoved(now_ms.saturating_sub(last.at_ms)))
}

/// How old `s`'s own stamp is by PTIMER, ms. Meaningful once the page's stamps
/// are known to be PTIMER's (the clock section's `Age::Ptimer`): each section
/// is stamped when written, and they need not move together. `None` for a
/// section with no polled stamp
pub fn stamp_age_ms(s: &Section, ptimer_ns: u64) -> Option<u64> {
    let (ts, _) = s.whole()?;
    (ts != TS_INVALID && ts < SEQ_START).then(|| ptimer_ns.saturating_sub(ts) / 1_000_000)
}

/// What the event thread tells GSP-RM: `Some(mask)` to change what it polls,
/// `None` to leave it
pub fn decide(now_ms: u64, wanted_until_ms: u64, polling: bool) -> Option<u64> {
    match (now_ms < wanted_until_ms, polling) {
        (true, false) => Some(POLL_MASK),
        (false, true) => Some(0),
        _ => None,
    }
}

/// The page, while GSP-RM may write it
struct Shared {
    page: DmaBuffer,
    /// GSP-RM was handed it and not told to drop it
    linked: bool,
}

static SHARED: spin::Mutex<Option<Shared>> = spin::Mutex::new(None);
/// What GSP-RM was last told to poll: something, or nothing
static POLLING: AtomicBool = AtomicBool::new(false);
/// GSP-RM refused a `SET_DATA_POLL`; not asked again until the next link
static REFUSED: AtomicBool = AtomicBool::new(false);
/// Somebody read until this, in our milliseconds since boot
static WANTED_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static MOVED: spin::Mutex<Moved> = spin::Mutex::new(Moved { ts: TS_INVALID, at_ms: NOT_SINCE_POLLING });

/// The live page, read through the cache: each line is flushed before it is
/// read. OpenRM maps the page cached and relies on GSP-RM's writes snooping;
/// the flush is right either way, for 50 lines
struct Live<'a>(&'a DmaBuffer);

impl Page for Live<'_> {
    fn timestamp(&self, off: usize) -> u64 {
        let p = unsafe { self.0.virt_base().add(off) };
        crate::mm::dma::flush_lines(p, 8);
        unsafe { core::ptr::read_volatile(p as *const u64) }
    }
    fn copy(&self, off: usize, out: &mut [u8]) {
        let p = unsafe { self.0.virt_base().add(off) };
        crate::mm::dma::flush_lines(p, out.len());
        for (i, b) in out.iter_mut().enumerate() {
            *b = unsafe { core::ptr::read_volatile(p.add(i)) };
        }
    }
}

/// Why GSP-RM has no page to write into
#[derive(Debug)]
pub enum LinkError {
    /// No page of system memory to give it
    NoPage,
    /// `INIT_USER_SHARED_DATA` was refused or never answered
    Rm(RmError),
}

/// Hand GSP-RM a page to write into (`gpuCreateRusdMemory_IMPL`), with nothing
/// polled at the default interval. Returns the page's address
pub fn link(rpc: &mut GspRpc, internal: &RmClient) -> Result<u64, LinkError> {
    let Ok(mut page) = DmaBuffer::alloc(1) else {
        return Err(LinkError::NoPage);
    };
    page.zero();
    // Out of the cache first: a dirty line of zeroes written back later would
    // land on what GSP-RM wrote, if it does not snoop
    page.flush_to_memory();
    let phys = page.phys();
    // Refused: GSP-RM never took it, and the page is ours to drop
    internal.control(rpc, internal.h_subdevice, NV2080_CTRL_CMD_INTERNAL_INIT_USER_SHARED_DATA,
        &init_params(Some(phys))).map_err(LinkError::Rm)?;
    // A page left by a GSP-RM that never reached its unload is still that
    // GSP-RM's to write, so not handed back (`crashcat::configure`)
    if let Some(old) = SHARED.lock().replace(Shared { page, linked: true }) {
        core::mem::forget(old.page);
    }
    POLLING.store(false, Ordering::Release);
    REFUSED.store(false, Ordering::Release);
    // A zeroed page: the first timestamp GSP-RM writes is the first news
    *MOVED.lock() = Moved::unmoved(TS_INVALID);
    // `_gpushareddataInitPollingFrequency`: the interval, nothing polled yet
    if internal.control(rpc, internal.h_subdevice, NV2080_CTRL_CMD_INTERNAL_USER_SHARED_DATA_SET_DATA_POLL,
        &poll_params(0, INTERVAL_MS)).is_err()
    {
        REFUSED.store(true, Ordering::Release);
    }
    Ok(phys)
}

/// `_gpushareddataDestroyGsp`: tell GSP-RM to stop writing the page. The page
/// stays until `release`
pub fn unlink(rpc: &mut GspRpc, internal: &RmClient) -> Result<(), RmError> {
    let was = SHARED.lock().as_mut().map(|s| core::mem::replace(&mut s.linked, false)).unwrap_or(false);
    POLLING.store(false, Ordering::Release);
    if !was {
        return Ok(());
    }
    internal.control(rpc, internal.h_subdevice, NV2080_CTRL_CMD_INTERNAL_INIT_USER_SHARED_DATA,
        &init_params(None)).map(|_| ())
}

/// GSP-RM is gone. The page goes back to the allocator only when the unload saw
/// the core halted and WPR2 down, the rule of CrashCat's queue and the rings
/// (`dev/gsp-foundation.md`, section 14); otherwise it stays out for good
pub fn release(gpu_done_with_it: bool) {
    if let Some(s) = SHARED.lock().take() {
        if !gpu_done_with_it {
            core::mem::forget(s.page);
        }
    }
    POLLING.store(false, Ordering::Release);
}

/// Is there a page GSP-RM writes, and is it polling into it
pub fn state() -> (bool, bool, bool) {
    let linked = SHARED.lock().as_ref().is_some_and(|s| s.linked);
    (linked, POLLING.load(Ordering::Acquire), REFUSED.load(Ordering::Acquire))
}

/// A reader: keep GSP-RM polling for the next `LEASE_MS`. Atomics and a kick,
/// safe anywhere a syscall runs
pub fn touch() {
    let until = crate::kcore::time::since_boot_ms() + LEASE_MS;
    WANTED_UNTIL_MS.fetch_max(until, Ordering::AcqRel);
    if !POLLING.load(Ordering::Acquire) && !REFUSED.load(Ordering::Acquire) {
        crate::drivers::gpu::nvidia::gsp_common::kgsp::worker::kick();
    }
}

/// What `service` would tell GSP-RM now, if anything. Atomics and one lock
/// never held across an RPC, so the event thread asks before taking RM's lock
pub fn pending() -> Option<u64> {
    if REFUSED.load(Ordering::Acquire) || !SHARED.lock().as_ref().is_some_and(|s| s.linked) {
        return None;
    }
    decide(crate::kcore::time::since_boot_ms(), WANTED_UNTIL_MS.load(Ordering::Acquire),
        POLLING.load(Ordering::Acquire))
}

/// The event thread's half: start or stop GSP-RM's polling as the lease says.
/// On the internal client, as OpenRM sends it
pub fn service(rpc: &mut GspRpc, internal: &RmClient) {
    let Some(mask) = pending() else {
        return;
    };
    match internal.control(rpc, internal.h_subdevice, NV2080_CTRL_CMD_INTERNAL_USER_SHARED_DATA_SET_DATA_POLL,
        &poll_params(mask, INTERVAL_MS))
    {
        Ok(_) => {
            POLLING.store(mask != 0, Ordering::Release);
            // What is on the page was written before this polling; only what
            // comes after says what the card is doing
            if mask != 0 {
                if let Some(s) = SHARED.lock().as_ref() {
                    *MOVED.lock() = Moved::unmoved(Live(&s.page).timestamp(CLOCKS.off));
                }
            }
            crate::kdebug!("[nvidia/perf] GSP-RM {} the shared data",
                if mask != 0 { "polls into" } else { "stopped polling into" });
        }
        Err(e) => {
            REFUSED.store(true, Ordering::Release);
            crate::kwarn!("[nvidia/perf] GSP-RM refused to poll the shared data (mask {:#x}): {:?}", mask, e);
        }
    }
}

/// The live page read once, with how old its clock section is. `None` with no
/// page linked
pub fn read(ptimer_ns: Option<u64>) -> Option<(Raw, Option<Age>)> {
    let guard = SHARED.lock();
    let s = guard.as_ref().filter(|s| s.linked)?;
    let raw = read_raw(&Live(&s.page));
    drop(guard);
    let polling = POLLING.load(Ordering::Acquire);
    let age = raw.clocks.whole().and_then(|(ts, _)| {
        let a = age(ts, ptimer_ns, crate::kcore::time::since_boot_ms(), &mut MOVED.lock())?;
        // Moving or not cannot be told while nothing moves it
        (polling || matches!(a, Age::Ptimer(_))).then_some(a)
    });
    Some((raw, age))
}

/// GSP-RM is gone: no lease outlives it
pub fn forget() {
    WANTED_UNTIL_MS.store(0, Ordering::Release);
    POLLING.store(false, Ordering::Release);
    *MOVED.lock() = Moved::unmoved(TS_INVALID);
}
