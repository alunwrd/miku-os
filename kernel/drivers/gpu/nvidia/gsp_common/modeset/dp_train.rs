// DisplayPort link training and SST stream configuration.

use super::super::disp::{DpcdCaps, H_DISPLAY};
use super::super::rmapi::{RmClient, RmError};
use super::super::rpc::GspRpc;
use crate::drivers::gpu::nvidia::le;

// The modeset modules are re-exported flat from `mod.rs`
use super::*;

// DisplayPort link training (RM control)

/// 'NV0073_CTRL_CMD_DP_CTRL': the call running link training.
///
/// The DP training sequence (pattern 1, clock recovery, pattern 2/3, channel
/// equalisation, voltage swing and pre-emphasis adjustment across retries)
/// happens inside GSP-RM. The host states a target lane count and bandwidth and
/// learns whether it held
pub const NV0073_CTRL_CMD_DP_CTRL: u32 = 0x0073_1343;

pub const DP_CTRL_PARAMS_SIZE: usize = 28;
pub const OFF_DP_CTRL_ERR: usize = 16;
pub const OFF_DP_CTRL_RETRY_MS: usize = 20;

// 'cmd' bits. SET_LANE_COUNT and SET_LINK_BW say the matching 'data' fields are
// meaningful; without them RM keeps what the link had
pub const DP_CMD_SET_LANE_COUNT: u32 = 1 << 0;
pub const DP_CMD_SET_LINK_BW: u32 = 1 << 1;
pub const DP_CMD_SET_ENHANCED_FRAMING: u32 = 1 << 7;
pub const DP_CMD_POST_LT_ADJ_REQ_GRANTED: u32 = 1 << 10;

// 'data' fields
pub const DP_DATA_LANE_COUNT_SHIFT: u32 = 0;
pub const DP_DATA_LINK_BW_SHIFT: u32 = 8;
pub const DP_DATA_TARGET_SHIFT: u32 = 19;

// 'err' fields (`ctrl0073dp.h`): which phase broke, and how far it got
pub const DP_ERR_CLOCK_RECOVERY: u32 = 1 << 4;
pub const DP_ERR_CHANNEL_EQUALIZATION: u32 = 1 << 5;
pub const DP_ERR_LINK_STATUS_DISCONNECTED: u32 = 1 << 29;
fn dp_err_cr_done_lanes(err: u32) -> u32 {
    (err >> 8) & 0xf
}

/// The DPCD link-rate codes below `bw`, one step (`LinkRates::getLowerRate`
/// over RBR, HBR, HBR2, HBR3). 0 below RBR
pub fn dp_lower_rate(bw: u32) -> u32 {
    match bw {
        0x1e => 0x14,
        0x14 => 0x0a,
        0x0a => 0x06,
        _ => 0,
    }
}

/// `IS_VALID_LANECOUNT`
fn dp_valid_lanes(n: u32) -> bool {
    matches!(n, 0 | 1 | 2 | 4 | 8)
}

/// `LinkConfiguration::lowerConfig`: the next configuration after a failure: a
/// lower rate, or when there is none (or `reduce_lanes`), fewer lanes. One lane
/// failing at the lowest rate leaves nothing.
///
/// Unlike the reference, as its own TODO asks: after one lane it goes back to
/// the sink's lane count at the lower rate, not always to four
pub fn dp_lower_config(lanes: &mut u32, bw: &mut u32, reduce_lanes: bool, max_lanes: u32) -> bool {
    let lower = dp_lower_rate(*bw);
    if reduce_lanes {
        if *lanes == 1 {
            if lower != 0 {
                *lanes = max_lanes.clamp(1, 4);
                *bw = lower;
            } else {
                *lanes = 0;
            }
        } else {
            *lanes /= 2;
        }
    } else if lower != 0 {
        *bw = lower;
    } else {
        *lanes /= 2;
    }
    *lanes != 0
}

/// `NV_DP_RBR_FALLBACK_MAX_TRIES`
const RBR_FALLBACK_MAX_TRIES: u32 = 3;

/// DPCD 0x200-0x205: SINK_COUNT, DEVICE_SERVICE_IRQ_VECTOR, LANE0_1_STATUS,
/// LANE2_3_STATUS, LANE_ALIGN_STATUS_UPDATED, SINK_STATUS
pub const DPCD_LINK_STATUS: u32 = 0x200;
pub const DPCD_LINK_STATUS_LEN: usize = 6;
/// DPCD 0x600, `SET_POWER`
pub const DPCD_SET_POWER: u32 = 0x600;

/// `ConnectorImpl::isLinkLost` for a single stream: RM says no lanes are up, or
/// the lanes lost interlane alignment, or any lane in use lost clock recovery,
/// equalisation or symbol lock. `status` is DPCD 0x200-0x205; anything short of
/// that reads as lost
pub fn dp_link_lost(lanes: u32, status: &[u8]) -> bool {
    if lanes == 0 {
        return true;
    }
    let Some(&align) = status.get(4) else {
        return true;
    };
    // INTERLANE_ALIGN_DONE, bit 0 of 0x204
    if align & 1 == 0 {
        return true;
    }
    (0..lanes.min(4)).any(|lane| {
        // Two lanes a byte, low nibble first: CR_DONE, CHANNEL_EQ_DONE,
        // SYMBOL_LOCKED in bits 0-2
        let byte = status.get(2 + (lane / 2) as usize).copied().unwrap_or(0);
        (byte >> ((lane % 2) * 4)) & 0x7 != 0x7
    })
}

/// `isLinkInD3` (`DPCDHALImpl::getPowerState`): the sink is powered down:
/// `SET_POWER` 2:0 is D3, or anything but D0 and D3-with-AUX-on. A sink that
/// asked to sleep is not a lost link
pub fn dp_in_d3(set_power: Option<u8>) -> bool {
    !matches!(set_power.map(|v| v & 0x7), Some(1) | Some(5))
}

/// Train the link behind 'display_id', starting at 'lanes' lanes and 'bw' (the
/// DPCD link rate byte, 270 Mbps per unit) and falling back until one holds:
/// `EvoMainLink::train` (`dp_evoadapter.cpp:1000-1275`). Returns the
/// configuration that trained.
///
/// RM's status decides, not the 'err' field: "'err' doesn't represent failure
/// in LT" (`:1267`). 'err' says which phase broke, choosing the fallback: a
/// failed clock recovery drops the rate, and at RBR keeps the lanes that
/// recovered; a failed equalisation drops lanes first. A sink gone away is not
/// trained down to nothing.
///
/// The sink only: LTTPR repeaters are not counted, so none are trained, and
/// GSP-RM runs the link transparently. 'lanes = 0' takes the link down
pub fn dp_train_target(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    target: u32,
    lanes: u32,
    bw: u32,
    caps: &DpcdCaps,
) -> Result<(u32, u32), RmError> {
    let mut cmd = DP_CMD_SET_LANE_COUNT | DP_CMD_SET_LINK_BW;
    if caps.enhanced_frame_cap {
        cmd |= DP_CMD_SET_ENHANCED_FRAMING;
    }
    // Post-link-training adjustment is a DP 1.2 feature the sink advertises in
    // DPCD 0x02 bit 5. TPS4-capable sinks use another mechanism and must not be
    // granted it, and it applies only to the sink
    if target == 0 && caps.post_lt_adj_cap && !caps.tps4_supported {
        cmd |= DP_CMD_POST_LT_ADJ_REQ_GRANTED;
    }
    let max_lanes = (caps.max_lane_count as u32).max(1);
    let (mut req_lanes, mut req_bw) = (lanes, bw);
    let mut cr_high_rate_fallbacks = 0;
    let mut last = RmError::Status(0);
    loop {
        let tried = (req_lanes, req_bw);
        let data = (req_lanes << DP_DATA_LANE_COUNT_SHIFT)
            | (req_bw << DP_DATA_LINK_BW_SHIFT)
            | (target << DP_DATA_TARGET_SHIFT);

        // Up to three attempts at one configuration, only while GSP-RM asks for
        // a delay: a panel not yet awake says how long to wait, and a failure
        // with no delay is not timing
        let mut status;
        let mut err;
        let mut retries = 0;
        loop {
            retries += 1;
            let mut p = alloc::vec![0u8; DP_CTRL_PARAMS_SIZE];
            le::put_u32(&mut p, 4, display_id);
            le::put_u32(&mut p, 8, cmd);
            le::put_u32(&mut p, 12, data);
            let (st, out) = client.control_status(rpc, H_DISPLAY, NV0073_CTRL_CMD_DP_CTRL, &p)?;
            status = st;
            err = le::u32_or0(&out, OFF_DP_CTRL_ERR);
            // Clamped: a device asked for this delay, and an absurd one would
            // hang the caller
            let wait_ms = le::u32_or0(&out, OFF_DP_CTRL_RETRY_MS).min(1000);
            if wait_ms > 0 {
                crate::kcore::time::mdelay(wait_ms as u64);
            }
            if status == 0 || wait_ms == 0 || retries >= 3 {
                break;
            }
        }
        if status == 0 {
            return Ok((req_lanes, req_bw));
        }
        last = RmError::Status(status);
        if req_lanes == 0 || err & DP_ERR_LINK_STATUS_DISCONNECTED != 0 {
            break;
        }

        let mut fallback = false;
        if err & DP_ERR_CLOCK_RECOVERY != 0 {
            if req_bw != 0x06 {
                if !dp_lower_config(&mut req_lanes, &mut req_bw, false, max_lanes) {
                    break;
                }
            } else {
                // Already RBR: keep the lanes that did recover, at the rate
                // first asked for
                req_lanes = dp_err_cr_done_lanes(err);
                while !dp_valid_lanes(req_lanes) {
                    req_lanes -= 1;
                }
                if req_lanes == 0 {
                    // "some system that doesn't set CR_DONE or EQ_DONE at all"
                    req_lanes = ((data >> DP_DATA_LANE_COUNT_SHIFT) & 0x1f) / 2;
                    if req_lanes == 0 {
                        break;
                    }
                }
                req_bw = bw;
                cr_high_rate_fallbacks += 1;
            }
            fallback = true;
        }
        if err & DP_ERR_CHANNEL_EQUALIZATION != 0 {
            if dp_err_cr_done_lanes(err) == 0 {
                if req_bw == 0x06 || !dp_lower_config(&mut req_lanes, &mut req_bw, false, max_lanes)
                {
                    break;
                }
            } else if !dp_lower_config(&mut req_lanes, &mut req_bw, true, max_lanes) {
                break;
            }
            fallback = true;
        }
        if !fallback || cr_high_rate_fallbacks >= RBR_FALLBACK_MAX_TRIES {
            break;
        }
        crate::knotice!(
            "[nvidia/dp] {:#010x}: {} lanes x {:#x} did not train ({}) - trying {} lanes x {:#x}",
            display_id,
            tried.0,
            tried.1,
            dp_err_str(err),
            req_lanes,
            req_bw
        );
    }
    crate::kwarn!("[nvidia/dp] {:#010x}: link training gave up after {} lanes x {:#x} (asked for {} x {:#x}): {:?}",
        display_id, req_lanes, req_bw, lanes, bw, last);
    Err(last)
}

/// What the `err` word of a failed `DP_CTRL` says, for the log
fn dp_err_str(err: u32) -> alloc::string::String {
    let mut parts = alloc::vec::Vec::new();
    if err & DP_ERR_CLOCK_RECOVERY != 0 {
        parts.push(alloc::format!(
            "clock recovery, {} lane(s) recovered",
            dp_err_cr_done_lanes(err)
        ));
    }
    if err & DP_ERR_CHANNEL_EQUALIZATION != 0 {
        parts.push(alloc::string::String::from("channel equalisation"));
    }
    if err & DP_ERR_LINK_STATUS_DISCONNECTED != 0 {
        parts.push(alloc::string::String::from("sink disconnected"));
    }
    if parts.is_empty() {
        alloc::format!("err {:#x}", err)
    } else {
        parts.join(", ")
    }
}

// DP stream configuration

/// 'NV0073_CTRL_CMD_DP_CONFIG_STREAM': how the head's pixels are packed into
/// the trained link.
///
/// A DP link runs at a fixed symbol rate unrelated to the pixel clock, so
/// pixels go in transfer units with a watermark controlling how full each gets,
/// and blanking is measured in link symbols. Wrong values leave the link up
/// while the picture tears or never locks
pub const NV0073_CTRL_CMD_DP_CONFIG_STREAM: u32 = 0x0073_1362;

/// The struct has a tail of deprecated fields on the r535 line and not on r570,
/// so the command has two sizes. Every field written here sits at the same
/// offset in both.
///
/// nouveau's r570 header, from that release, says 84, while its code reaches
/// this control through the r535 path and sends 116. 'dp_config_stream_sst'
/// tries both and reports which was accepted
pub const CONFIG_STREAM_SIZE_R570: usize = 84;
pub const CONFIG_STREAM_SIZE_R535: usize = 116;

/// The transfer unit size RM's own calculation assumes
pub const TU_SIZE: u32 = 64;

/// Program the single-stream (non-MST) configuration for 'head' on 'sor'.
/// Returns the parameter size that worked
#[allow(clippy::too_many_arguments)]
pub fn dp_config_stream_sst(
    rpc: &mut GspRpc,
    client: &RmClient,
    head: u32,
    sor_index: u32,
    dp_link_b: bool,
    enhanced_framing: bool,
    watermark: u32,
    hblank_sym: u32,
    vblank_sym: u32,
) -> Result<usize, [(usize, RmError); 2]> {
    let mut errors = [
        (CONFIG_STREAM_SIZE_R570, RmError::Status(0)),
        (CONFIG_STREAM_SIZE_R535, RmError::Status(0)),
    ];
    for slot in errors.iter_mut() {
        match config_stream_once(
            rpc,
            client,
            slot.0,
            head,
            sor_index,
            dp_link_b,
            enhanced_framing,
            watermark,
            hblank_sym,
            vblank_sym,
        ) {
            Ok(()) => return Ok(slot.0),
            // Both are reported: two sizes giving two errors says more than one
            // error
            Err(e) => slot.1 = e,
        }
    }
    Err(errors)
}

#[allow(clippy::too_many_arguments)]
pub fn config_stream_once(
    rpc: &mut GspRpc,
    client: &RmClient,
    size: usize,
    head: u32,
    sor_index: u32,
    dp_link_b: bool,
    enhanced_framing: bool,
    watermark: u32,
    hblank_sym: u32,
    vblank_sym: u32,
) -> Result<(), RmError> {
    let mut p = alloc::vec![0u8; size];
    le::put_u32(&mut p, 4, head);
    le::put_u32(&mut p, 8, sor_index);
    le::put_u32(&mut p, 12, dp_link_b as u32);
    p[16] = 1; // bEnableOverride: use the numbers below, do not re-derive them
    p[17] = 0; // bMST
    le::put_u32(&mut p, 24, hblank_sym);
    le::put_u32(&mut p, 28, vblank_sym);
    // SST substruct at +68: {bEnhancedFraming u8, tuSize u32, waterMark u32}
    p[68] = enhanced_framing as u8;
    le::put_u32(&mut p, 72, TU_SIZE);
    le::put_u32(&mut p, 76, watermark);
    client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DP_CONFIG_STREAM, &p)?;
    Ok(())
}

/// The three numbers 'dp_config_stream_sst' needs
#[derive(Debug, Clone, Copy)]
pub struct StreamParams {
    pub watermark: u32,
    pub hblank_sym: u32,
    pub vblank_sym: u32,
}

/// Watermark and blanking-symbol counts for a single-stream link.
///
/// The calculation of the NVIDIA driver, copied by nouveau with its comments;
/// the formulas came from hardware simulation, and the constants are not
/// explicable from the DP specification alone.
///
/// 'None' when the mode cannot be carried: the first check is the bandwidth
/// test, and the later ones catch modes whose blanking is too short for the
/// mandatory per-line overhead
pub fn dp_sst_stream_params(
    mode: &Mode,
    lanes: u32,
    bw: u32,
    bpc: u32,
    enhanced_framing: bool,
) -> Option<StreamParams> {
    const PRECISION: u64 = 100_000;
    const WATERMARK_ADJUST: u64 = 2;
    const WATERMARK_LIMIT: u64 = 20;

    if lanes == 0 || bw == 0 {
        return None;
    }
    let raster = mode.raster();
    // Symbol rate of the link in Hz: the DPCD byte counts 270 Mbps steps, and
    // 8b/10b makes that 27 million symbols per second per step
    let min_rate = bw as u64 * 27_000 * 1000;
    let lanes64 = lanes as u64;
    let depth = bpc as u64 * 3;
    let pclk = mode.clock_khz as u64 * 1000;
    // Visible width, recovered the same way RM does it
    let surface_width = (raster.h_blanks - raster.h_blanke) as u64;
    let raster_width = raster.h_active as u64;

    // The bandwidth test. Everything below assumes the payload fits
    if pclk * depth >= 8 * min_rate * lanes64 {
        return None;
    }

    let ratio = (pclk * depth * PRECISION) / (8 * min_rate * lanes64);
    if ratio > PRECISION {
        return None;
    }
    let watermark_f = (ratio * TU_SIZE as u64 * (PRECISION - ratio)) / PRECISION;
    let mut watermark =
        WATERMARK_ADJUST + (2 * (depth * PRECISION / (8 * lanes64)) + watermark_f) / PRECISION;

    let symbols_per_line = surface_width * depth / (8 * lanes64);
    if watermark > 39 || watermark > symbols_per_line {
        return None;
    }
    if watermark < WATERMARK_LIMIT {
        watermark = WATERMARK_LIMIT;
    }

    // Bits going out during blanking regardless of pixels: the BS/BE framing
    // symbols on every lane, doubled with enhanced framing, plus VBID, MVID and
    // MAUD sent four times over
    let mut blanking_bits = 3 * 8 * lanes64 + if enhanced_framing { 3 * 8 * lanes64 } else { 0 };
    blanking_bits += 3 * 8 * 4;
    // A line width not a multiple of the lane count is padded to a whole
    // pixel-steering group
    let remain = surface_width % lanes64;
    if remain != 0 {
        blanking_bits += (lanes64 - remain) * depth;
    }
    let blanking_link_clocks = (blanking_bits * PRECISION) / (8 * lanes64);
    let min_hblank = (blanking_link_clocks * pclk / min_rate) / PRECISION + 12;
    if min_hblank > raster_width.saturating_sub(surface_width) {
        return None;
    }
    // Bug 702290 in RM's numbering: narrower than this the hardware misbehaves
    // in ways the formulas do not model
    if surface_width <= 60 {
        return None;
    }

    let mut hblank_symbols =
        (((raster_width - surface_width - min_hblank) * min_rate) / pclk) as i64;
    hblank_symbols -= 1; // stuffer latency to send BS
    hblank_symbols -= 3; // secondary-data-packet latency into the stuffer
    hblank_symbols -= match lanes {
        1 => 9,
        2 => 6,
        _ => 3,
    };

    let mut vblank_symbols: i64 = if surface_width < 40 {
        0
    } else {
        let v = (((surface_width - 40) * min_rate) / pclk) as i64 - 1;
        v - match lanes {
            1 => 39,
            2 => 21,
            _ => 12,
        }
    };
    if vblank_symbols < 0 {
        vblank_symbols = 0;
    }

    Some(StreamParams {
        watermark: watermark as u32,
        hblank_sym: hblank_symbols.max(0) as u32,
        vblank_sym: vblank_symbols as u32,
    })
}
