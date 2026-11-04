// The last few seconds of clocks, as the PMU sampled them (`dev/gpu-perf.md`)
//
// `NV2080_CTRL_CMD_CLK_PMUMON_CLK_DOMAINS_GET_SAMPLES`, routed to GSP-RM (flags
// 0x40048 in its export entry). Up to 250 samples, as often as every 20 ms:
// about five seconds. The control's `super` is in and out, meant to be handed
// back so the next call returns what is new; no history is kept here, so every
// call starts from zero and gets what the queue holds.

use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::gsp_common::rmapi::{RmClient, RmError};
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::le;

/// `NV2080_CTRL_CMD_CLK_PMUMON_CLK_DOMAINS_GET_SAMPLES`
pub const NV2080_CTRL_CMD_CLK_PMUMON_CLK_DOMAINS_GET_SAMPLES: u32 = 0x2080_1037;
/// `NV2080_CTRL_CLK_PMUMON_CLK_DOMAINS_SAMPLE_COUNT`
pub const SAMPLE_COUNT: usize = 250;
/// `NV2080_CTRL_PMUMON_GET_SAMPLES_SUPER`: `tailIndex`, `sequenceId`,
/// `headIndex`, `numSamples`, `resetCount`, `retrievalMode`
const SUPER: usize = 24;
const NUM_SAMPLES: usize = 12;
/// One sample: a PTIMER timestamp, then GPCCLK, DRAMCLK, NVDCLK and the active
/// GPCCLK, kHz
const SAMPLE: usize = 24;
/// The whole block, measured with gcc
pub const PARAMS: usize = 6024;
const _: () = assert!(SUPER + SAMPLE_COUNT * SAMPLE == PARAMS);
/// `NV2080_CTRL_CLK_PMUMON_CLK_DOMAINS_SAMPLE_INVALID`: not supported
pub const INVALID: u32 = u32::MAX;

/// One point of the history
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockSample {
    /// PTIMER nanoseconds
    pub timestamp: u64,
    pub gpc_khz: u32,
    pub dram_khz: u32,
    pub nvd_khz: u32,
    pub active_gpc_khz: u32,
}

/// The block the control is sent with: `retrievalMode` ALL_SAMPLES (0), and
/// the rest zero
pub fn request() -> Vec<u8> {
    alloc::vec![0u8; PARAMS]
}

/// The samples of an answer, oldest first. `None` for a block too short to be
/// one
pub fn parse(b: &[u8]) -> Option<Vec<ClockSample>> {
    if b.len() < PARAMS {
        return None;
    }
    let n = (le::u32_at(b, NUM_SAMPLES) as usize).min(SAMPLE_COUNT);
    Some((0..n).map(|i| {
        let at = SUPER + i * SAMPLE;
        ClockSample {
            timestamp: le::u64_at(b, at),
            gpc_khz: le::u32_at(b, at + 8),
            dram_khz: le::u32_at(b, at + 12),
            nvd_khz: le::u32_at(b, at + 16),
            active_gpc_khz: le::u32_at(b, at + 20),
        }
    }).collect())
}

/// Ask for what the PMU's clock queue holds
pub fn query(rpc: &mut GspRpc, client: &RmClient) -> Result<Vec<ClockSample>, RmError> {
    let b = client.control(rpc, client.h_subdevice, NV2080_CTRL_CMD_CLK_PMUMON_CLK_DOMAINS_GET_SAMPLES,
        &request())?;
    parse(&b).ok_or(RmError::ShortReply)
}

/// A clock change in the history: when, from the first sample, and the clocks
/// from then on
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub after_ms: u64,
    pub gpc_khz: u32,
    pub dram_khz: u32,
    /// How many samples in a row had these clocks
    pub samples: u32,
}

/// The history folded into changes: a run of samples with the same GPC and
/// DRAM clocks is one line
pub fn changes(samples: &[ClockSample]) -> Vec<Change> {
    let Some(first) = samples.first() else { return Vec::new() };
    let mut out: Vec<Change> = Vec::new();
    for s in samples {
        match out.last_mut() {
            Some(c) if c.gpc_khz == s.gpc_khz && c.dram_khz == s.dram_khz => c.samples += 1,
            _ => out.push(Change {
                after_ms: s.timestamp.saturating_sub(first.timestamp) / 1_000_000,
                gpc_khz: s.gpc_khz,
                dram_khz: s.dram_khz,
                samples: 1,
            }),
        }
    }
    out
}
