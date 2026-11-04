// CrashCat: GSP-RM's own account of how it died
//
// When a Peregrine core crashes, its firmware writes a report into a queue it
// shares with the host, bumps a put pointer in a scratch register and raises
// HALT. `kgspHealthCheck_TU102` (`kernel_gsp_tu102.c:976`) reads every report
// after every drain, logs it, and calls GSP-RM dead when the crash was not
// contained below it. This does that, with the library under it
// (`src/libraries/crashcat`: engine, wayfinder_v1, queue_v1, report_v1,
// impl/libos3; the formats are `nv-crashcat.h` and `libos_v3_crashcat.h`).
//
// The queue is ours: on GB202-207 OpenRM hands GSP-RM a 4 KiB sysmem queue
// through the WPR metadata (`kgspGetCrashcatSysmemBufferSize`, 4096 for those
// five). The wayfinder in the falcon's registers says where the queue is; a
// report is read only from the queue this driver registered.
//
// `dev/gsp-foundation.md`, section 15.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::mm::dma::DmaBuffer;
use crate::serial_println;

/// `kgspGetCrashcatSysmemBufferSize_b845ef` (GB202-GB207). A multiple of the 1
/// KiB the queue is aligned to in size and offset
/// (`kcrashcatEngineConfigure_IMPL`), and one page, so contiguous
pub const QUEUE_SIZE: u32 = 4096;

/// `NV_CRASHCAT_SIGNATURE`: the low 16 bits of the L0 wayfinder and of every
/// packet header
const SIGNATURE: u64 = 0xdead;

// NV_PFALCON_FALCON_* (`turing/tu102/dev_falcon_v4.h`), from the falcon base
const FALCON_MAILBOX0: u32 = 0x40;
const FALCON_MAILBOX1: u32 = 0x44;
/// Where the L0 wayfinder lives (`kcrashcatEngineGetWFL0Offset_TU102`)
const FALCON_DEBUGINFO: u32 = 0x94;
/// `NV_PFALCON_FALCON_COMMON_SCRATCH_GROUP_<g>(i)`
const fn scratch(g: u32, i: u32) -> u32 {
    0x300 + g * 0x10 + i * 4
}

/// `NV_CRASHCAT_DEF_SCRATCH_GROUP_V1_REGMAP_TABLE`: the registers of each
/// scratch group id. GSP defines no IMPL_DEF group, so it is empty like F
fn scratch_group(id: u32) -> &'static [u32] {
    const A: [u32; 2] = [FALCON_MAILBOX0, FALCON_MAILBOX1];
    const B: [u32; 4] = [scratch(0, 0), scratch(0, 1), scratch(0, 2), scratch(0, 3)];
    const C: [u32; 4] = [scratch(1, 0), scratch(1, 1), scratch(1, 2), scratch(1, 3)];
    const D: [u32; 4] = [scratch(2, 0), scratch(2, 1), scratch(2, 2), scratch(2, 3)];
    const E: [u32; 4] = [scratch(3, 0), scratch(3, 1), scratch(3, 2), scratch(3, 3)];
    match id {
        1 => &A,
        2 => &B,
        3 => &C,
        4 => &D,
        5 => &E,
        _ => &[],
    }
}

// NV_CRASHCAT_MEM_APERTURE
const APERTURE_SYSGPA: u64 = 0;

// NV_CRASHCAT_PACKET_TYPE
const PACKET_REPORT: u8 = 0;
const PACKET_CSR: u8 = 1;
const PACKET_GPR: u8 = 2;
const PACKET_TRACE: u8 = 3;
const PACKET_IO32: u8 = 4;

// Fixed packet sizes, header included: `NvCrashCatReport_V1` is 8 u64s,
// `..Riscv64CsrState_V1` 1 + 7, `..Riscv64GprState_V1` 1 + 31
const REPORT_BYTES: usize = 64;
const CSR_BYTES: usize = 64;
const GPR_BYTES: usize = 256;

/// "LIBOS3.1" (`NV_CRASHCAT_REPORT_IMPLEMENTER_SIGNATURE_LIBOS3`)
const IMPLEMENTER_LIBOS3: u64 = 0x4C49_424F_5333_2E31;

// NV_CRASHCAT_CONTAINMENT
const CONTAINMENT_UNSPECIFIED: u8 = 0x0;
const CONTAINMENT_RISCV_MODE_M: u8 = 0x1;
const CONTAINMENT_RISCV_HART: u8 = 0x4;
const CONTAINMENT_UNCONTAINED: u8 = 0xf;

fn bits(v: u64, hi: u32, lo: u32) -> u64 {
    (v >> lo) & ((1u64 << (hi - lo + 1)) - 1)
}

fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

// Packet header (`NV_CRASHCAT_PACKET_HEADER`)

fn format_version(hdr: u64) -> u64 {
    bits(hdr, 19, 16)
}

/// `crashcatPacketHeaderPayloadSize`: (size + 1) units of 8 B, 1, 4 or 64 KiB
fn payload_size(hdr: u64) -> usize {
    let shift = match bits(hdr, 21, 20) {
        0 => 3,
        1 => 10,
        2 => 12,
        _ => 16,
    };
    ((bits(hdr, 31, 22) + 1) << shift) as usize
}

/// `crashcatPacketHeaderValid`. The payload size is never 0: it is stored minus
/// one
fn header_valid(hdr: u64) -> bool {
    bits(hdr, 15, 0) == SIGNATURE && format_version(hdr) == 1
}

/// `NV_CRASHCAT_CAUSE_TYPE_WATCHDOG` (`nv-crashcat.h`, 615): a timeout the
/// libos watchdog saw. The generic implementer has no name for it
const CAUSE_TYPE_WATCHDOG: u64 = 3;

fn packet_type(hdr: u64) -> u8 {
    bits(hdr, 39, 32) as u8
}

/// One crash report and the packets that came with it
/// (`CrashCatReport`, v1 members)
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// `NvCrashCatReport_V1`: header, implementer, reporter id, reporter
    /// data, source id, source cause, source pc, source data
    base: [u64; 8],
    csr: Option<[u64; 8]>,
    gpr: Option<[u64; 32]>,
    stack: Option<(u64, Vec<u64>)>,
    pc_trace: Option<(u64, Vec<u64>)>,
    io32: Option<(u64, Vec<u64>)>,
}

impl Report {
    fn implementer(&self) -> u64 { self.base[1] }
    fn reporter_id(&self) -> u64 { self.base[2] }
    fn reporter_data(&self) -> u64 { self.base[3] }
    fn source_id(&self) -> u64 { self.base[4] }
    fn source_cause(&self) -> u64 { self.base[5] }
    fn source_pc(&self) -> u64 { self.base[6] }
    fn source_data(&self) -> u64 { self.base[7] }

    fn libos3(&self) -> bool {
        self.implementer() == IMPLEMENTER_LIBOS3
    }

    /// `crashcatReportSourceContainment_HAL`: only LIBOS3 defines it
    pub fn containment(&self) -> u8 {
        if self.libos3() { bits(self.source_cause(), 7, 4) as u8 } else { CONTAINMENT_UNSPECIFIED }
    }

    /// `crashcatReportIsWatchdog_V1`: a libos task overran its watchdog (615
    /// on). OpenRM logs and posts it, not counting it an error
    pub fn is_watchdog(&self) -> bool {
        bits(self.source_cause(), 3, 0) == CAUSE_TYPE_WATCHDOG
    }

    /// `kgspCrashCatReportImpactsGspRm`
    pub fn impacts_gsp_rm(&self) -> bool {
        matches!(
            self.containment(),
            CONTAINMENT_RISCV_MODE_M | CONTAINMENT_RISCV_HART | CONTAINMENT_UNCONTAINED
        )
    }

    /// Whether this report makes GSP-RM dead (`kgspHealthCheck_TU102`): it
    /// impacts GSP-RM and is not a watchdog report
    pub fn is_fatal(&self) -> bool {
        self.impacts_gsp_rm() && !self.is_watchdog()
    }

    /// `crashcatReportLog`: the source line first (the one OpenRM raises as
    /// the Xid), then the reporter, then each packet by type, then the end
    pub fn lines(&self) -> Vec<String> {
        const IND: &str = "    ";
        const DATA: &str = "        ";
        let mut out = Vec::new();
        out.push(alloc::format!("GSP {}", self.source_line()));
        out.push(alloc::format!("{}{}", IND, self.reporter_line()));
        if let Some(c) = &self.csr {
            out.push(alloc::format!("{}RISC-V CSR State:", IND));
            let (x, epc_name, tval_name, cause_name) = match bits(c[0], 42, 40) {
                1 => ("m", "mepc", "mbadaddr", "mcause"),
                2 => ("s", "sepc", "stval", "scause"),
                _ => ("x", "xepc", "xtval", "xcause"),
            };
            // xstatus, xie, xip, xepc, xtval, xcause, xscratch
            out.push(alloc::format!(
                "{}{x}status:{:#018x}  {x}scratch:{:#018x}  {x}ie:{:#018x}  {x}ip:{:#018x}",
                DATA, c[1], c[7], c[2], c[3]
            ));
            out.push(alloc::format!(
                "{}{}:{:#018x}  {}:{:#018x}  {}:{:#018x}",
                DATA, epc_name, c[4], tval_name, c[5], cause_name, c[6]
            ));
        }
        if let Some(g) = &self.gpr {
            // ra sp gp tp t0 t1 t2 s0 s1 a0..a7 s2..s11 t3..t6, as the struct
            const NAMES: [&str; 31] = [
                "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3",
                "a4", "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10",
                "s11", "t3", "t4", "t5", "t6",
            ];
            out.push(alloc::format!("{}RISC-V GPR State:", IND));
            // OpenRM's order: ra sp gp tp / a0-a7 / s0-s11 / t0-t6
            let order: [usize; 31] = [
                0, 1, 2, 3, 9, 10, 11, 12, 13, 14, 15, 16, 7, 8, 17, 18, 19, 20, 21, 22, 23, 24,
                25, 26, 4, 5, 6, 27, 28, 29, 30,
            ];
            for row in order.chunks(4) {
                let mut line = String::from(DATA);
                for &i in row {
                    line.push_str(&alloc::format!("{:>3}:{:#018x}  ", NAMES[i], g[i + 1]));
                }
                out.push(String::from(line.trim_end()));
            }
        }
        for (title, trace) in [("Stack Trace:", &self.stack), ("PC Trace:", &self.pc_trace)] {
            if let Some((_, addrs)) = trace {
                out.push(alloc::format!("{}{}", IND, title));
                for row in addrs.chunks(if title == "PC Trace:" { 5 } else { 1 }) {
                    let mut line = String::from(DATA);
                    for a in row {
                        line.push_str(&alloc::format!("{:#018x}  ", a));
                    }
                    out.push(String::from(line.trim_end()));
                }
            }
        }
        if let Some((hdr, regs)) = &self.io32 {
            out.push(alloc::format!("{}{}", IND, match bits(*hdr, 41, 40) {
                1 => "Local I/O Register State:",
                2 => "External I/O Register State:",
                _ => "Additional Register State:",
            }));
            for row in regs.chunks(4) {
                let mut line = String::from(DATA);
                // Each entry is { u32 value; u32 offset; }
                for r in row {
                    line.push_str(&alloc::format!("{:#010x}:{:#010x}   ", *r >> 32, *r as u32));
                }
                out.push(String::from(line.trim_end()));
            }
        }
        out.push(alloc::format!("{}------------[ end crash report ]------------", IND));
        out
    }

    /// `crashcatReportLogSource_V1_LIBOS3`, or `_GENERIC` for any other
    /// implementer
    fn source_line(&self) -> String {
        let id = self.source_id();
        let (partition, ucode, mode) = (bits(id, 7, 0), bits(id, 15, 8), bits(id, 18, 16));
        let cause_type = bits(self.source_cause(), 3, 0);
        if !self.libos3() {
            let cause = match cause_type {
                0 => "exception",
                2 => "panic",
                1 => "timeout",
                _ => "unknown failure",
            };
            return alloc::format!(
                "{} in partition:{} ucode:{} [{}-mode] @ pc:{:#x}, data:{:#x}",
                cause, partition, ucode, mode_char(mode), self.source_pc(), self.source_data()
            );
        }
        let task = bits(id, 31, 24);
        let mode = match mode {
            1 => "monitor",
            2 => "kernel",
            3 => "task",
            _ => "unspecified",
        };
        let what = match cause_type {
            0 => {
                let xcause = self.source_data();
                alloc::format!("{} exception: {} (cause:{:#x}) @ pc:{:#x}",
                    mode, riscv_cause(xcause), xcause, self.source_pc())
            }
            1 => alloc::format!("{} timeout @ pc:{:#x}", mode, self.source_pc()),
            CAUSE_TYPE_WATCHDOG => alloc::format!("{} watchdog timeout @ pc:{:#x}", mode, self.source_pc()),
            2 => {
                let reason = bits(self.source_cause(), 39, 32);
                alloc::format!("{} panic: {} ({}) @ pc:{:#x}, aux:{:#x}",
                    mode, panic_reason(reason), reason, self.source_pc(), self.source_data())
            }
            // No case in OpenRM's switch: it prints nothing
            _ => alloc::format!("{} cause type {}", mode, cause_type),
        };
        if task == 0xff {
            alloc::format!("{}, partition:{}#{}", what, partition, ucode)
        } else {
            alloc::format!("{}, partition:{}#{}, task:{}", what, partition, ucode, task)
        }
    }

    /// `crashcatReportLogReporter_V1_LIBOS3` / `_GENERIC`
    fn reporter_line(&self) -> String {
        let id = self.reporter_id();
        let data = self.reporter_data();
        let (partition, ucode) = (bits(id, 7, 0), bits(id, 15, 8));
        let (version, timestamp) = (bits(data, 31, 0), bits(data, 63, 32));
        if !self.libos3() {
            return alloc::format!(
                "Reported by partition:{} ucode:{} [{}-mode] version:{} @ {}",
                partition, ucode, mode_char(bits(id, 18, 16)), version, timestamp
            );
        }
        let (major, minor, cl) = (bits(version, 31, 28), bits(version, 27, 24), bits(version, 23, 0));
        let task = bits(id, 31, 24);
        if task == 0xff {
            alloc::format!("Reported by libos partition:{}#{} kernel v{}.{} [{}] @ ts:{}",
                partition, ucode, major, minor, cl, timestamp)
        } else {
            alloc::format!("Reported by libos partition:{}#{} task:{} v{}.{} [{}] @ ts:{}",
                partition, ucode, task, major, minor, cl, timestamp)
        }
    }
}

fn mode_char(mode: u64) -> char {
    match mode {
        1 => 'M',
        2 => 'S',
        3 => 'U',
        _ => '?',
    }
}

/// `crashcatReportRiscvCauseToString`
fn riscv_cause(xcause: u64) -> &'static str {
    let code = xcause & 0x1f;
    if xcause >> 63 != 0 {
        match code {
            0 => "user software interrupt",
            1 => "supervisor software interrupt",
            3 => "machine software interrupt",
            4 => "user timer interrupt",
            5 => "supervisor timer interrupt",
            7 => "machine timer interrupt",
            8 => "user external interrupt",
            9 => "supervisor external interrupt",
            0xb => "machine external interrupt",
            _ => "unknown interrupt",
        }
    } else {
        match code {
            0 => "instruction address misaligned",
            1 => "instruction access fault",
            2 => "illegal instruction",
            3 => "breakpoint",
            4 => "load address misaligned",
            5 => "load access fault",
            6 => "store address misaligned",
            7 => "store access fault",
            8 => "environment call from U-mode",
            9 => "environment call from S-mode",
            0xb => "environment call from M-mode",
            0xc => "instruction access page fault",
            0xd => "load access page fault",
            0xf => "store access page fault",
            _ => "unknown exception",
        }
    }
}

/// `crashcatReportPanicReasonToString_LIBOS3`
fn panic_reason(reason: u64) -> &'static str {
    match reason {
        0x01 => "unhandled state",
        0x02 => "invalid configuration",
        0x03 => "fatal hardware error",
        0x04 => "insufficient resources",
        0x05 => "timeout",
        0x06 => "environment call failed",
        0x08 => "asan memory error detected",
        0x0a => "programming error",
        0x0b => "condition failed",
        0x0c => "unhandled trap",
        0x0d => "instruction access fault",
        _ => "unknown error",
    }
}

/// `crashcatReportExtract_V1`: one report from the front of `bytes`, and how
/// many bytes it took: up to the next REPORT packet or the end. `None` when the
/// front is not a report; the caller then skips what it read, as
/// `crashcatQueueConsumeNextReport_V1` does.
///
/// Where OpenRM would read past the end (a trace or I/O packet longer than the
/// bytes left, or a skipped packet that is), this stops at the end
pub fn extract(bytes: &[u8]) -> (Option<Report>, usize) {
    let mut r = Report::default();
    // crashcatReportExtractReport_V1
    let Some(hdr) = u64_at(bytes, 0) else { return (None, 0) };
    if bytes.len() < REPORT_BYTES
        || !header_valid(hdr)
        || payload_size(hdr) != REPORT_BYTES - 8
    {
        crate::kwarn!("[gsp/crashcat] invalid V1 report header {:#x}", hdr);
        return (None, 0);
    }
    for (i, w) in r.base.iter_mut().enumerate() {
        *w = u64_at(bytes, i * 8).unwrap();
    }

    let mut pos = REPORT_BYTES;
    while pos < bytes.len() {
        let remaining = bytes.len() - pos;
        let hdr = match u64_at(bytes, pos) {
            Some(h) if header_valid(h) => h,
            h => {
                crate::kwarn!(
                    "[gsp/crashcat] invalid packet header ({:#x}), skipping {} bytes remaining",
                    h.unwrap_or(0), remaining
                );
                pos = bytes.len();
                continue;
            }
        };
        let kind = packet_type(hdr);
        // The next report, which the next call takes
        if kind == PACKET_REPORT {
            break;
        }
        let words = |n: usize| -> Vec<u64> {
            (0..n).map(|i| u64_at(bytes, pos + i * 8).unwrap_or(0)).collect()
        };
        let taken = match kind {
            PACKET_CSR if remaining >= CSR_BYTES && payload_size(hdr) == CSR_BYTES - 8 => {
                r.csr = Some(words(8).try_into().unwrap());
                CSR_BYTES
            }
            PACKET_GPR if remaining >= GPR_BYTES && payload_size(hdr) == GPR_BYTES - 8 => {
                r.gpr = Some(words(32).try_into().unwrap());
                GPR_BYTES
            }
            PACKET_TRACE if remaining >= 8 + payload_size(hdr) => {
                let n = payload_size(hdr) / 8;
                let addrs: Vec<u64> = (0..n).map(|i| u64_at(bytes, pos + 8 + i * 8).unwrap()).collect();
                // One of each: a stack walk and the NVRISC-V trace buffer
                let slot = if bits(hdr, 43, 43) == 0 { &mut r.stack } else { &mut r.pc_trace };
                if slot.is_some() {
                    crate::knotice!("[gsp/crashcat] only one trace of a kind is kept, discarding the old one");
                }
                *slot = Some((hdr, addrs));
                8 + payload_size(hdr)
            }
            PACKET_IO32 if r.io32.is_none() && remaining >= 8 + payload_size(hdr) => {
                let n = payload_size(hdr) / 8;
                let regs = (0..n).map(|i| u64_at(bytes, pos + 8 + i * 8).unwrap()).collect();
                r.io32 = Some((hdr, regs));
                8 + payload_size(hdr)
            }
            _ => 0,
        };
        if taken == 0 {
            let skip = payload_size(hdr) + 8;
            crate::kwarn!(
                "[gsp/crashcat] unable to extract packet type {:#04x} ({} bytes), skipping",
                kind, skip
            );
            pos = (pos + skip).min(bytes.len());
            continue;
        }
        pos += taken;
    }
    (Some(r), pos.min(bytes.len()))
}

/// Where the queue is and how it is driven, once the wayfinder has said
#[derive(Clone, Copy)]
struct QueueCtl {
    put_reg: u32,
    get_reg: u32,
    size: u32,
}

/// `KernelCrashCatEngine` for the GSP: the registered queue, and what the
/// wayfinder has said so far
struct Engine {
    queue: DmaBuffer,
    /// The L0 wayfinder, 0 until it has been seen valid
    wfl0: u32,
    /// The L1 wayfinder, 0 until decoded. Non-zero with no `ctl` means the
    /// queue it names could not be used, and it is not retried
    wfl1: u64,
    ctl: Option<QueueCtl>,
}

static ENGINE: spin::Mutex<Option<Engine>> = spin::Mutex::new(None);
/// Every report read since the machine booted, and the last one as it was
/// logged, for `nvidia intr`
static REPORTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static LAST: spin::Mutex<Vec<String>> = spin::Mutex::new(Vec::new());

/// Keep a report's lines as the last one read
pub fn remember(lines: Vec<String>) {
    REPORTS.fetch_add(1, Ordering::Relaxed);
    *LAST.lock() = lines;
}

/// How many reports have been read, and the last one's lines
pub fn last() -> (u32, Vec<String>) {
    (REPORTS.load(Ordering::Relaxed), LAST.lock().clone())
}

/// Things said once per engine, not once per drain
static SAID_VERSION: AtomicBool = AtomicBool::new(false);
static SAID_POINTERS: AtomicBool = AtomicBool::new(false);

/// `kcrashcatEngineConfigure` with `allocQueueSize`: allocate and register the
/// queue for a GSP-RM about to boot. Its address and size go into the WPR
/// metadata; `None` if the page could not be had, leaving no CrashCat for this
/// boot
pub fn configure() -> Option<(u64, u32)> {
    let mut queue = match DmaBuffer::alloc(1) {
        Ok(q) => q,
        Err(e) => {
            crate::kwarn!("[gsp/crashcat] no queue page ({:?}) - CrashCat off for this boot", e);
            return None;
        }
    };
    queue.zero();
    DmaBuffer::write_barrier();
    let at = (queue.phys(), QUEUE_SIZE);
    // A queue from a boot that never reached its unload may still be GSP-RM's
    // to write, so it is not handed back
    if let Some(old) = ENGINE.lock().replace(Engine { queue, wfl0: 0, wfl1: 0, ctl: None }) {
        core::mem::forget(old.queue);
    }
    SAID_VERSION.store(false, Ordering::Relaxed);
    SAID_POINTERS.store(false, Ordering::Relaxed);
    Some(at)
}

/// `kcrashcatEngineConfigured`
pub fn configured() -> bool {
    ENGINE.lock().is_some()
}

/// `kcrashcatEngineUnload`: GSP-RM is gone. The queue page is freed only when
/// the unload showed the core halted and WPR2 down, the rule the rings follow
/// (`dev/gsp-foundation.md`, section 14); otherwise it stays out of the
/// allocator
pub fn release(gpu_done_with_it: bool) {
    if let Some(e) = ENGINE.lock().take() {
        if !gpu_done_with_it {
            core::mem::forget(e.queue);
        }
    }
}

/// `crashcatEngineGetNextCrashReport`: the next whole report in the queue, if
/// there is one. `falcon` is the GSP falcon's register base
pub fn next_report(bar0: &MmioRegion, falcon: u32) -> Option<Report> {
    let mut guard = ENGINE.lock();
    let e = guard.as_mut()?;

    // crashcatEngineLoadWayfinder
    if e.wfl0 == 0 {
        let wfl0 = bar0.read32(falcon + FALCON_DEBUGINFO);
        if bits(wfl0 as u64, 15, 0) != SIGNATURE {
            return None;
        }
        // Version 1 is the only wayfinder there is
        // (`NV_CRASHCAT_WAYFINDER_VERSION_LAST`)
        if bits(wfl0 as u64, 19, 16) != 1 {
            if !SAID_VERSION.swap(true, Ordering::Relaxed) {
                crate::kwarn!("[gsp/crashcat] wayfinder version {} is not one this driver reads (wfl0 {:#x})",
                    bits(wfl0 as u64, 19, 16), wfl0);
            }
            return None;
        }
        e.wfl0 = wfl0;
    }

    // crashcatWayfinderGetReportQueue_V1
    if e.ctl.is_none() {
        if e.wfl1 != 0 {
            return None;
        }
        let location = bits(e.wfl0 as u64, 22, 20) as u32;
        let regs = scratch_group(location);
        if regs.len() < 2 {
            if !SAID_VERSION.swap(true, Ordering::Relaxed) {
                crate::kwarn!("[gsp/crashcat] insufficiently-sized L1 wayfinder scratch location {}", location);
            }
            return None;
        }
        e.wfl1 = (bar0.read32(falcon + regs[1]) as u64) << 32 | bar0.read32(falcon + regs[0]) as u64;
        let aperture = bits(e.wfl1, 2, 0);
        let shift = match bits(e.wfl1, 4, 3) {
            1 => 10,
            2 => 12,
            3 => 16,
            _ => 0,
        };
        let size = if shift == 0 { 0 } else { (bits(e.wfl1, 9, 6) + 1) << shift };
        let offset = bits(e.wfl1, 63, 10) << 10;
        let (put_reg, get_reg) = if regs.len() >= 4 {
            (regs[2], regs[3])
        } else {
            // Two registers only: they were the L1 wayfinder and become put
            // and get, and WFL0 saying NONE tells the producer so
            bar0.write32(falcon + regs[1], 0);
            bar0.write32(falcon + regs[0], 0);
            bar0.write32(falcon + FALCON_DEBUGINFO, e.wfl0 & !(0x7 << 20));
            (regs[0], regs[1])
        };
        // crashcatEngineMapCrashBuffer: a sysmem queue must be the registered
        // one. Another aperture would need a mapping (FB) or a port (DMEM/EMEM)
        // this driver does not have; GB20x is given sysmem
        if aperture != APERTURE_SYSGPA
            || offset != e.queue.phys()
            || size == 0
            || size > e.queue.size() as u64
        {
            crate::kerror!(
                "[gsp/crashcat] queue at aperture {} offset {:#x} size {:#x} is not the one registered ({:#x}) - reports will not be read",
                aperture, offset, size, e.queue.phys()
            );
            return None;
        }
        serial_println!("[gsp/crashcat] wayfinder: queue {:#x} + {:#x}, put/get at {:#x}/{:#x}",
            offset, size, put_reg, get_reg);
        e.ctl = Some(QueueCtl { put_reg, get_reg, size: size as u32 });
    }

    // crashcatQueueConsumeNextReport_V1
    let ctl = e.ctl?;
    let put = bar0.read32(falcon + ctl.put_reg);
    let get = bar0.read32(falcon + ctl.get_reg);
    if put >= ctl.size || get >= ctl.size {
        if !SAID_POINTERS.swap(true, Ordering::Relaxed) {
            crate::kerror!("[gsp/crashcat] queue pointers out of range: put {:#x} get {:#x} size {:#x}",
                put, get, ctl.size);
        }
        return None;
    }
    if put == get {
        return None;
    }
    // The report is complete once put has moved; the bytes behind it are read
    // after the pointer
    core::sync::atomic::fence(Ordering::Acquire);
    let q = e.queue.as_slice();
    let (put, get, size) = (put as usize, get as usize, ctl.size as usize);
    let bytes: Vec<u8> = if put > get {
        q[get..put].to_vec()
    } else {
        let mut v = q[get..size].to_vec();
        v.extend_from_slice(&q[..put]);
        v
    };
    let (report, taken) = extract(&bytes);
    // A report that could not be read is skipped whole, so a bad one is said
    // once
    let taken = if report.is_none() || taken == 0 { bytes.len() } else { taken };
    bar0.write32(falcon + ctl.get_reg, ((get + taken) % size) as u32);
    report
}
