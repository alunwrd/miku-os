// What the driver may ask of the firmware's clock choice (`dev/gpu-perf.md`)
//
//   clock lock     `NV2080_CTRL_CMD_PERF_RATED_TDP_SET_CONTROL` as client OS,
//                  "expected to be used by the operating system to set
//                  FORCE_LOCK for performance profiling" (`ctrl2080perf.h`).
//                  `subdeviceCtrlCmdPerfRatedTdpSetControl_KERNEL`
//                  (`kern_perf_pwr.c`) forwards the block unchanged on a GSP
//                  client; `_GET_CONTROL` is routed
//   power source   `NV2080_CTRL_CMD_PERF_GET_POWERSTATE`, routed. Linux tells
//                  GSP-RM only on a mobile config (`osinit.c`), so a desktop
//                  should always read AC
//
// The boost is in `mod.rs`.

use crate::drivers::gpu::nvidia::gsp_common::rmapi::{RmClient, RmError};
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::le;

/// `NV2080_CTRL_CMD_PERF_RATED_TDP_GET_CONTROL`
pub const NV2080_CTRL_CMD_PERF_RATED_TDP_GET_CONTROL: u32 = 0x2080_206e;
/// `NV2080_CTRL_CMD_PERF_RATED_TDP_SET_CONTROL`
pub const NV2080_CTRL_CMD_PERF_RATED_TDP_SET_CONTROL: u32 = 0x2080_206f;
/// `NV2080_CTRL_CMD_PERF_GET_POWERSTATE`
pub const NV2080_CTRL_CMD_PERF_GET_POWERSTATE: u32 = 0x2080_205a;

/// `NV2080_CTRL_PERF_RATED_TDP_CONTROL_PARAMS`: `client`, `input`,
/// `vPstateType`, 12 bytes
pub const TDP_PARAMS: usize = 12;
/// `NV2080_CTRL_PERF_RATED_TDP_CLIENT_OS`
pub const CLIENT_OS: u32 = 3;

/// `NV2080_CTRL_PERF_RATED_TDP_ACTION`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// No request of this client's
    Default = 0,
    ForceExceed = 1,
    ForceLimit = 2,
    /// Clocks fixed at the vPstate
    ForceLock = 3,
    /// The vPstate as the floor, boosting above it allowed
    ForceFloor = 4,
}

impl Action {
    pub fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Default,
            1 => Self::ForceExceed,
            2 => Self::ForceLimit,
            3 => Self::ForceLock,
            4 => Self::ForceFloor,
            _ => return None,
        })
    }
}

/// `NV2080_CTRL_PERF_RATED_TDP_VPSTATE_TYPE`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VPstate {
    RatedTdp = 0,
    TurboBoost = 1,
}

/// The block for client OS
pub fn tdp_params(action: Action, vpstate: VPstate) -> [u8; TDP_PARAMS] {
    let mut p = [0u8; TDP_PARAMS];
    le::put_u32(&mut p, 0, CLIENT_OS);
    le::put_u32(&mut p, 4, action as u32);
    le::put_u32(&mut p, 8, vpstate as u32);
    p
}

/// Ask for `action` at `vpstate` as client OS. `Action::Default` takes the
/// request back
pub fn set(rpc: &mut GspRpc, client: &RmClient, action: Action, vpstate: VPstate) -> Result<(), RmError> {
    client.control(rpc, client.h_subdevice, NV2080_CTRL_CMD_PERF_RATED_TDP_SET_CONTROL,
        &tdp_params(action, vpstate))?;
    Ok(())
}

/// What client OS has asked for: the action, and the vPstate word as it came
/// back. `None` for an action this driver has no name for
pub fn get(rpc: &mut GspRpc, client: &RmClient) -> Result<(Option<Action>, u32), RmError> {
    let b = client.control(rpc, client.h_subdevice, NV2080_CTRL_CMD_PERF_RATED_TDP_GET_CONTROL,
        &tdp_params(Action::Default, VPstate::RatedTdp))?;
    Ok((Action::from_u32(le::u32_or0(&b, 4)), le::u32_or0(&b, 8)))
}

/// `NV2080_CTRL_PERF_POWER_SOURCE_*`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerSource {
    Ac,
    Battery,
    Other(u32),
}

/// Which power source GSP-RM believes it is on
pub fn power_source(rpc: &mut GspRpc, client: &RmClient) -> Result<PowerSource, RmError> {
    let b = client.control_query(rpc, client.h_subdevice, NV2080_CTRL_CMD_PERF_GET_POWERSTATE, 4)?;
    Ok(match le::u32_or0(&b, 0) {
        0 => PowerSource::Ac,
        1 => PowerSource::Battery,
        v => PowerSource::Other(v),
    })
}
