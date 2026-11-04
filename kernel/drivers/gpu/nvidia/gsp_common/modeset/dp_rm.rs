// Handles SOR allocation, DisplayPort RM controls, and initial firmware state
// readout

use super::super::disp::H_DISPLAY;
use super::super::rmapi::{RmClient, RmError};
use super::super::rpc::GspRpc;
use crate::drivers::gpu::nvidia::le;

// The modeset modules are re-exported flat from `mod.rs`
use super::*;

// SOR assignment (RM control)

/// 'NV0073_CTRL_CMD_DFP_ASSIGN_SOR'. A display id names a connector; a SOR is
/// the serial output resource driving it, with fewer SORs than connectors. RM
/// owns the assignment
pub const NV0073_CTRL_CMD_DFP_ASSIGN_SOR: u32 = 0x0073_1152;

pub const ASSIGN_SOR_PARAMS_SIZE: usize = 80;
pub const ASSIGN_SOR_MAX: usize = 4;
/// 'sorAssignListWithTag[]', an array of {displayMask, sorType} pairs
pub const OFF_SOR_ASSIGN_LIST_WITH_TAG: usize = 40;

/// 'NV0073_CTRL_CMD_DP_MAIN_LINK_CTRL': power the DisplayPort main link up.
///
/// The main link carries video; AUX is separate and works without it. A
/// trained, unpowered link leaves a monitor awake, holding sync off the clock,
/// showing nothing. RM does not do this as a side effect of training
pub fn dp_main_link_power(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    power_up: bool,
) -> Result<(), RmError> {
    // {subDeviceInstance, displayId, ctrl}, POWER_STATE is bit 0
    let mut p = [0u8; 12];
    le::put_u32(&mut p, 4, display_id);
    le::put_u32(&mut p, 8, power_up as u32);
    client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DP_MAIN_LINK_CTRL, &p)?;
    Ok(())
}

pub const NV0073_CTRL_CMD_DP_MAIN_LINK_CTRL: u32 = 0x0073_1356;

/// DPCD 0x600 `SET_POWER` values
pub const DPCD_POWER_D0: u8 = 1;
pub const DPCD_POWER_D3: u8 = 2;

/// Put a DisplayPort sink to sleep or wake it (`DPCDHALImpl::setPowerState`,
/// `dp_configcaps.cpp:1824`). Waking gets at least 40 tries ("some panels need
/// up to 40 retries") and sleeping one. A DPCD 1.0 sink has no power state:
/// waking is a success and sleeping is not.
///
/// The extended wake-timeout grant DPLib writes before D3, for sinks asking
/// more than the standard millisecond, is not written: DPCD 0x2211 is not read
/// here
pub fn dp_set_sink_power(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    dpcd_rev: u8,
    on: bool,
) -> Result<(), RmError> {
    if dpcd_rev < 0x11 {
        return if on {
            Ok(())
        } else {
            Err(RmError::Status(NV_ERR_NOT_SUPPORTED))
        };
    }
    let state = if on { DPCD_POWER_D0 } else { DPCD_POWER_D3 };
    let tries = if on { 40 } else { 1 };
    let mut last = RmError::Status(0);
    for _ in 0..tries {
        match super::super::disp::dpcd_write(rpc, client, display_id, 0x600, &[state]) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// `NV_ERR_NOT_SUPPORTED`
const NV_ERR_NOT_SUPPORTED: u32 = 0x56;

/// `NV0073_CTRL_CMD_SPECIFIC_SET_MONITOR_POWER`: the monitor on a connector not
/// DisplayPort, on or off, what `nvRmSetDpmsEvo` asks of RM for TMDS
pub const NV0073_CTRL_CMD_SPECIFIC_SET_MONITOR_POWER: u32 = 0x0073_0295;

pub fn set_monitor_power(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    head: u32,
    on: bool,
) -> Result<(), RmError> {
    // {subDeviceInstance, displayId, powerState, headIdx, bForceMonitorState}
    let mut p = [0u8; 20];
    le::put_u32(&mut p, 4, display_id);
    le::put_u32(&mut p, 8, on as u32);
    le::put_u32(&mut p, 12, head);
    client.control(
        rpc,
        H_DISPLAY,
        NV0073_CTRL_CMD_SPECIFIC_SET_MONITOR_POWER,
        &p,
    )?;
    Ok(())
}

/// `NV0073_CTRL_CMD_DP_SET_MANUAL_DISPLAYPORT`: take DisplayPort off RM's
/// automatic path. Issued once, right after `NV04_DISPLAY_COMMON` is allocated,
/// as `r535_disp_oneinit` does.
///
/// It "disables automatic watermark programming": while RM owns the watermark,
/// `DP_CONFIG_STREAM` with `bEnableOverride = TRUE` returns `ILLEGAL_ACTION`.
/// RM's automatic values are computed for the display configuration RM believes
/// in, so with the modeset pushed straight into the core channel they describe
/// another mode, measured as vBlankSym 69275 in hardware against the 6823 this
/// mode needs. Wrong symbol counts give a lit, synced, blank panel
pub fn dp_set_manual(rpc: &mut GspRpc, client: &RmClient) -> Result<(), RmError> {
    // {subDeviceInstance} and nothing else
    let p = [0u8; 4];
    client.control(
        rpc,
        H_DISPLAY,
        NV0073_CTRL_CMD_DP_SET_MANUAL_DISPLAYPORT,
        &p,
    )?;
    Ok(())
}

pub const NV0073_CTRL_CMD_DP_SET_MANUAL_DISPLAYPORT: u32 = 0x0073_1365;

/// 'NV0073_CTRL_CMD_DP_GET_LINK_CONFIG': what the transmitter is set up to
/// drive, in RM's words. The modeset goes straight into the core channel, so if
/// RM thinks the link carries nothing, the panel gets that. Returns
/// '(laneCount, linkBW)'
pub fn dp_get_link_config(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
) -> Result<(u32, u32), RmError> {
    // {subDeviceInstance, displayId, laneCount, linkBW, dp2LinkBW,
    // bFECEnabled}: 24 bytes in 615, which added the FEC flag
    let mut p = [0u8; 24];
    le::put_u32(&mut p, 4, display_id);
    let out = client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DP_GET_LINK_CONFIG, &p)?;
    Ok((le::u32_or0(&out, 8), le::u32_or0(&out, 12)))
}

pub const NV0073_CTRL_CMD_DP_GET_LINK_CONFIG: u32 = 0x0073_1360;

/// 'NV0073_CTRL_CMD_DP_GET_TESTPATTERN': what the main link emits.
///
/// The SOR's own pattern register is undecoded: nouveau's table is Maxwell-era
/// and two neighbours come back as PRI poison. 'NONE' means ordinary video;
/// anything else is a test or training pattern, and no picture reaches the
/// panel
pub fn dp_get_testpattern(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
) -> Result<u32, RmError> {
    // {subDeviceInstance, displayId, testPattern{u32}, cstm}, where
    // NV0073_CTRL_DP_CSTM is nine NvU32: 4+4+4+36 = 48
    let mut p = [0u8; 48];
    le::put_u32(&mut p, 4, display_id);
    let out = client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DP_GET_TESTPATTERN, &p)?;
    Ok(le::u32_or0(&out, 8) & 0x1f)
}

pub const NV0073_CTRL_CMD_DP_GET_TESTPATTERN: u32 = 0x0073_1348;

/// 'NV0073_CTRL_DP_TESTPATTERN_DATA_*'
pub fn testpattern_name(v: u32) -> &'static str {
    match v {
        0x00 => "NONE(video)",
        0x01 => "D10_2",
        0x02 => "SERMP",
        0x03 => "PRBS_7",
        0x04 => "CSTM",
        0x05 => "HBR2COMPLIANCE",
        0x06 => "CP2520PAT3",
        0x07 => "TRAINING1",
        0x08 => "TRAINING2",
        0x09 => "TRAINING3",
        0x0a => "TRAINING4",
        0x0b => "CP2520PAT1",
        _ => "?",
    }
}

/// Ask RM which SOR drives 'display_id', excluding any handed out.
///
/// The answer is in a table: entry 'n' of 'sorAssignListWithTag' holds the
/// display mask SOR 'n' was given, so the index is found by scanning for our
/// bit
pub fn assign_sor(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    exclude_mask: u8,
) -> Result<u32, RmError> {
    let mut p = alloc::vec![0u8; ASSIGN_SOR_PARAMS_SIZE];
    le::put_u32(&mut p, 4, display_id);
    p[8] = exclude_mask;
    let out = client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DFP_ASSIGN_SOR, &p)?;
    if out.len() < ASSIGN_SOR_PARAMS_SIZE {
        return Err(RmError::ShortReply);
    }
    for or in 0..ASSIGN_SOR_MAX {
        let mask = le::u32_or0(&out, OFF_SOR_ASSIGN_LIST_WITH_TAG + or * 8);
        if mask & display_id != 0 {
            return Ok(or as u32);
        }
    }
    // RM accepted the call but gave the display to nothing: the board has no
    // SOR left, reported as a status
    Err(RmError::Status(NV_ERR_INSUFFICIENT_RESOURCES))
}

/// 'NV_ERR_INSUFFICIENT_RESOURCES', the closest RM status to "no SOR free"
pub const NV_ERR_INSUFFICIENT_RESOURCES: u32 = 0x1a;

// Inherited configuration

/// 'NV0073_CTRL_CMD_SYSTEM_GET_ACTIVE': which display id a head drives, or 0
/// for an idle head.
///
/// On a UEFI boot one head is scanning out: the firmware console set it up and
/// GSP-RM inherited it. Another head on the same connector means two heads
/// wanting one output resource, so this is asked before a head is chosen.
///
/// 615 params, 20 B: subDeviceInstance, head, flags, displayId, staleSorIndex
pub const NV0073_CTRL_CMD_SYSTEM_GET_ACTIVE: u32 = 0x0073_010c;

pub fn get_active(rpc: &mut GspRpc, client: &RmClient, head: u32) -> Result<u32, RmError> {
    let mut p = alloc::vec![0u8; 20];
    le::put_u32(&mut p, 4, head);
    let out = client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_SYSTEM_GET_ACTIVE, &p)?;
    Ok(le::u32_or0(&out, 12))
}

/// Find a head the firmware left scanning out, by reading the hardware.
///
/// RM has just booted, knows nothing of the VBIOS lighting a head, and
/// `GET_ACTIVE` reports nothing active while that head draws our console. A
/// live head carries a raster of a plausible size and a non-zero pixel clock; a
/// dead one zeroes or leftovers.
///
/// Returns the head, its raster and its pixel clock
pub fn live_head_from_hw(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    heads: u32,
) -> Option<(u32, u32, u32)> {
    for h in 0..heads {
        let base = CORE_ARM_BASE + M_HEAD_SET_RASTER_SIZE + h * HEAD_STRIDE;
        let raster = bar0.read32(base);
        let clock = bar0.read32(CORE_ARM_BASE + M_HEAD_SET_PIXEL_CLOCK_FREQUENCY + h * HEAD_STRIDE);
        let (w, v) = (raster & 0x7fff, raster >> 16);
        // A raster is total pixels including blanking, always larger than the
        // visible mode; these bounds reject zero and rubbish
        if clock != 0 && (640..=8192).contains(&w) && (480..=4320).contains(&v) {
            return Some((h, raster, clock));
        }
    }
    None
}
