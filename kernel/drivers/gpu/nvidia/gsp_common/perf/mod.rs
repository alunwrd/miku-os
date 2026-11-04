// P-states and clocks: what the firmware chose, and what may be asked of it
// (`dev/gpu-perf.md`; `dev/gpu-memory-compute.md`, 3.9)
//
// On a GB20x the P-state, the clocks and the voltages are GSP-RM's and its
// PMU's; the kernel half of OpenRM never programs a PLL. What it keeps, and
// this:
//
//   P-state      `NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE`, routed to GSP-RM
//                (`RMCTRL_FLAGS_ROUTE_TO_PHYSICAL` in its export entry)
//   boost        `kperfBoostSet_IMPL` (`kern_perf_boost.c`) turns the public
//                `PERF_BOOST` into `INTERNAL_PERF_BOOST_SET_2X` for GSP-RM
//   the numbers  the page GSP-RM writes them into (`rusd.rs`): clocks, load,
//                power, temperatures, fans, why the clocks are held down
//   history      the PMU's clock samples (`clocks.rs`)
//   requests     a clock lock or floor, the power source (`limits.rs`)
//
// GSP-RM's utilisation ring (`PERF_GET_GPUMON_PERFMON_UTIL_SAMPLES_V2`, 3.9)
// comes back empty on the card; the load figure comes from the page.
//
// `Snapshot` is the page decoded once, for `nvidia perf` and `gpu_info`.

pub mod clocks;
pub mod limits;
pub mod rusd;

use alloc::string::String;

/// What this module's lines are signed with in `nvidia log`: the perf engine's
/// load hands its boot lines through it (`nvidia::speaking`)
pub static SITE: crate::kcore::klog::Site = crate::klog_site!();

use crate::drivers::gpu::nvidia::gsp_common::rmapi::{RmClient, RmError};
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::le;


/// `NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE`
pub const NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE: u32 = 0x2080_2068;
/// `NV2080_CTRL_CMD_INTERNAL_PERF_BOOST_SET_2X`
pub const NV2080_CTRL_CMD_INTERNAL_PERF_BOOST_SET_2X: u32 = 0x2080_0a9a;

/// The P-state the GPU is in, as its number: `currPstate` is one bit,
/// `NV2080_CTRL_PERF_PSTATES_P0` = bit 0 up to P15. `None` for an answer with
/// no bit set (`_UNDEFINED`)
pub fn current_pstate(rpc: &mut GspRpc, client: &RmClient) -> Result<Option<u32>, RmError> {
    let b = client.control_query(rpc, client.h_subdevice, NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE, 4)?;
    let bits = le::u32_or0(&b, 0);
    Ok((bits != 0).then(|| bits.trailing_zeros()))
}

/// What a boost asks for (`NV2080_CTRL_PERF_BOOST_FLAGS_CMD`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boost {
    Clear = 0,
    OneLevel = 1,
    ToMax = 2,
}

/// `NV2080_CTRL_PERF_BOOST_DURATION_INFINITE`
pub const BOOST_UNTIL_CLEARED: u32 = 0xffff_ffff;
/// `NV2080_CTRL_PERF_BOOST_DURATION_MAX`, seconds
pub const BOOST_MAX_SECONDS: u32 = 3600;

/// `kperfBoostSet_IMPL`: the public command's flags and duration, as
/// `NV2080_CTRL_INTERNAL_PERF_BOOST_SET_PARAMS_2X`: an `NvBool flags` (the
/// command fits its low byte, as OpenRM's assignment truncates it) and a 32-bit
/// duration in seconds, 8 bytes
pub fn boost(rpc: &mut GspRpc, client: &RmClient, what: Boost, seconds: u32) -> Result<(), RmError> {
    let mut p = [0u8; 8];
    p[0] = what as u8;
    le::put_u32(&mut p, 4, if what == Boost::Clear { 0 } else { seconds });
    client.control(rpc, client.h_subdevice, NV2080_CTRL_CMD_INTERNAL_PERF_BOOST_SET_2X, &p)?;
    Ok(())
}

/// GSP-RM is gone; what it said is of a GPU that is not running it
pub fn forget() {
    rusd::forget();
}

/// `RUSD_CLK_PUBLIC_DOMAIN_*`, the order of the page's clocks
pub const GRAPHICS: usize = 0;
pub const MEMORY: usize = 1;
pub const VIDEO: usize = 2;
pub const SM: usize = 3;

/// `RUSD_TEMPERATURE_TYPE_*`
pub const TEMP_SENSORS: [&str; rusd::TEMP_TYPES] = ["GPU", "memory", "board", "power supply", "HBM"];

/// `RUSD_CLK_THROTTLE_REASON_*`, by bit
pub const THROTTLE_REASONS: [&str; 11] = [
    "idle", "application clocks", "software power cap", "hardware slowdown", "sync boost",
    "software thermal slowdown", "hardware thermal slowdown", "hardware power brake",
    "display clock", "board limit", "reliability",
];

/// The names of the reasons set in `mask`, and any bit there is no name for
pub fn throttle_names(mask: u32) -> alloc::vec::Vec<String> {
    (0..32)
        .filter(|b| mask & (1 << b) != 0)
        .map(|b| THROTTLE_REASONS.get(b as usize).map_or_else(|| alloc::format!("bit {}", b), |n| String::from(*n)))
        .collect()
}

/// One clock domain: what the firmware aims for and what it runs at
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    pub target_mhz: u32,
    pub actual_khz: u32,
}

/// The shared page decoded: every field is `None` where its section was never
/// written or could not be read whole
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// P0 is 0
    pub pstate: Option<u32>,
    /// `GRAPHICS`, `MEMORY`, `VIDEO`, `SM`
    pub clocks: Option<[Clock; 4]>,
    /// `RUSD_CLK_THROTTLE_REASON_*` bits
    pub throttle: Option<u32>,
    /// Graphics and memory busy, percent
    pub util: Option<(u32, u32)>,
    /// Average, instantaneous and total board power, mW
    pub power_mw: Option<u32>,
    pub inst_power_mw: Option<u32>,
    pub tgp_mw: Option<u32>,
    /// The power limit requested and the one enforced, mW
    pub limit_mw: Option<(u32, u32)>,
    /// `TEMP_SENSORS` order, tenths of a degree Celsius
    pub temps_dc: [Option<i32>; rusd::TEMP_TYPES],
    pub fans_rpm: Option<[u32; rusd::FAN_COOLERS]>,
    /// How old the clocks are, `None` when that cannot be told
    pub age: Option<rusd::Age>,
    /// How old every other section is, by its own stamp
    pub ages: Ages,
}

/// Each section's own age by PTIMER, ms. GSP-RM stamps each section as it
/// writes it, and they do not move together: the P-state section can read P8
/// under clocks that are P0's. All `None` when the stamps are not PTIMER's;
/// then only the clocks' age says anything
#[derive(Clone, Copy, Debug, Default)]
pub struct Ages {
    pub pstate: Option<u64>,
    pub throttle: Option<u64>,
    pub util: Option<u64>,
    pub power: Option<u64>,
    pub limit: Option<u64>,
    pub temps: [Option<u64>; rusd::TEMP_TYPES],
    pub fans: Option<u64>,
}

impl Snapshot {
    /// Decode one read of the page; `ptimer_ns` is PTIMER when it was read
    pub fn from_raw(raw: &rusd::Raw, age: Option<rusd::Age>, ptimer_ns: Option<u64>) -> Self {
        let clocks = raw.clocks.whole().map(|(_, b)| {
            core::array::from_fn(|d| Clock {
                target_mhz: le::u32_or0(b, 8 + d * 8),
                actual_khz: le::u32_or0(b, 12 + d * 8),
            })
        });
        let pstate = raw.pstate.u32_at(8).filter(|&bits| bits != 0).map(|bits| bits.trailing_zeros());
        let util = raw.util.u32_at(8).zip(raw.util.u32_at(12));
        let limit_mw = raw.power_limit.u32_at(8).zip(raw.power_limit.u32_at(12));
        // `NvTemp` is signed 24.8 fixed point
        let temps_dc = core::array::from_fn(|k| raw.temps[k].u32_at(8).map(|v| (v as i32 as i64 * 10 / 256) as i32));
        let fans_rpm = raw.fans.whole().map(|(_, b)| core::array::from_fn(|i| le::u32_or0(b, 8 + i * 4)));
        let ages = match (age, ptimer_ns) {
            (Some(rusd::Age::Ptimer(_)), Some(pt)) => {
                let of = |s: &rusd::Section| rusd::stamp_age_ms(s, pt);
                Ages {
                    pstate: of(&raw.pstate),
                    throttle: of(&raw.throttle),
                    util: of(&raw.util),
                    power: of(&raw.avg_power),
                    limit: of(&raw.power_limit),
                    temps: core::array::from_fn(|k| of(&raw.temps[k])),
                    fans: of(&raw.fans),
                }
            }
            _ => Ages::default(),
        };
        Snapshot {
            pstate,
            clocks,
            throttle: raw.throttle.u32_at(8),
            util,
            power_mw: raw.avg_power.u32_at(8),
            inst_power_mw: raw.inst_power.u32_at(8),
            tgp_mw: raw.power_policy.u32_at(8),
            limit_mw,
            temps_dc,
            fans_rpm,
            age,
            ages,
        }
    }

    /// Is this what the card does now, not what it did when polling last ran
    pub fn fresh(&self) -> bool {
        self.age.is_some_and(|a| a.ms() <= rusd::FRESH_MS)
    }

    /// Is a section of `age` (one of `ages`) current: the page fresh, and the
    /// section not left behind by it
    pub fn current(&self, age: Option<u64>) -> bool {
        self.fresh() && age.is_none_or(|a| a <= rusd::FRESH_MS)
    }

    /// The fans as far as the last one that turns; `[0]` when none does
    pub fn fans(&self) -> Option<&[u32]> {
        let f = self.fans_rpm.as_ref()?;
        let n = f.iter().rposition(|&r| r != 0).map_or(1, |i| i + 1);
        Some(&f[..n])
    }
}

/// Watts with one decimal, from mW
fn watts(mw: u32) -> String {
    alloc::format!("{}.{}", mw / 1000, mw % 1000 / 100)
}

/// Whole degrees, rounded, from tenths
fn degrees(dc: i32) -> i32 {
    if dc >= 0 { (dc + 5) / 10 } else { (dc - 5) / 10 }
}

/// The lines `gpu_info` carries (`docs/MikuOS_ABI.md`), each only where its
/// numbers are known and its section current, and none from a page not fresh.
/// SM is left out where the card does not report it (0: this one does not)
pub fn info_lines(s: &Snapshot) -> String {
    use core::fmt::Write;
    let mut out = String::new();
    if !s.fresh() {
        return out;
    }
    let a = s.ages;
    if let Some(p) = s.pstate.filter(|_| s.current(a.pstate)) {
        let _ = writeln!(out, "perf P{}", p);
    }
    if let Some(c) = s.clocks {
        let mhz = |d: usize| c[d].actual_khz / 1000;
        let sm = if c[SM].actual_khz != 0 { alloc::format!(" sm {}", mhz(SM)) } else { String::new() };
        let _ = writeln!(out, "clock gr {}{} mem {} video {}", mhz(GRAPHICS), sm, mhz(MEMORY), mhz(VIDEO));
    }
    if let Some((gpu, mem)) = s.util.filter(|_| s.current(a.util)) {
        let _ = writeln!(out, "util gpu {} mem {}", gpu, mem);
    }
    if let Some(p) = s.power_mw.filter(|_| s.current(a.power)) {
        let _ = match s.limit_mw.filter(|_| s.current(a.limit)) {
            Some((_, enforced)) => writeln!(out, "power {} limit {}", watts(p), watts(enforced)),
            None => writeln!(out, "power {}", watts(p)),
        };
    }
    let temps: alloc::vec::Vec<String> = [(0usize, "gpu"), (1, "mem"), (2, "board")]
        .iter()
        .filter(|&&(k, _)| s.current(a.temps[k]))
        .filter_map(|&(k, name)| s.temps_dc[k].map(|t| alloc::format!("{} {}", name, degrees(t))))
        .collect();
    if !temps.is_empty() {
        let _ = writeln!(out, "temp {}", temps.join(" "));
    }
    if let Some(f) = s.fans().filter(|_| s.current(a.fans)) {
        let rpm: alloc::vec::Vec<String> = f.iter().map(|r| alloc::format!("{}", r)).collect();
        let _ = writeln!(out, "fan {}", rpm.join(" "));
    }
    out
}

/// Read the page with PTIMER's `now`, keeping GSP-RM polling for the next
/// reader. `None` with no page linked
pub fn snapshot(ptimer_ns: Option<u64>) -> Option<Snapshot> {
    rusd::touch();
    let (raw, age) = rusd::read(ptimer_ns)?;
    Some(Snapshot::from_raw(&raw, age, ptimer_ns))
}
