// Variable refresh on a DisplayPort Adaptive-Sync sink
//
// `dev/display-nvkms.md`, decision 6. NVKMS's `nvkms-vrr.c` for class CA,
// reduced to one head with one client: whether the sink and the mode allow it
// (`nvAdjustHwModeTimingsForVrrEvo`), the four RM controls with their parameter
// blocks, and the two head methods putting the head in one-shot mode
// (`EvoSetStallLockC9`, `EvoSetDisplayRateC9`). The order is
// `blackwell/control.rs::set_vrr`, which has the locks.
//
// Every block is laid out from `ctrl0073system.h`: sizes 12, 24, 20 and 8,
// offsets checked with gcc and by the selftest (`vrr_params`). None has an
// array or a pointer, so the transport translates nothing.

use core::sync::atomic::{AtomicU32, Ordering};

use super::super::disp::{DpcdCaps, H_DISPLAY};
use super::super::pushbuf::{PushBuf, PushError};
use super::super::rmapi::{RmClient, RmError};
use super::super::rpc::GspRpc;
use super::{Mode, HEAD_STRIDE};
use crate::drivers::gpu::nvidia::le;

/// `NV0073_CTRL_CMD_SYSTEM_VRR_DISPLAY_INFO`: {subDeviceInstance, displayId,
/// bAddition} - which displays can stretch their vblank, so RM may switch the
/// memory clock inside one
pub const CMD_VRR_DISPLAY_INFO: u32 = 0x0073_012c;
pub const VRR_DISPLAY_INFO_SIZE: usize = 12;
/// `NV0073_CTRL_CMD_SYSTEM_CLEAR_ELV_BLOCK`: {subDeviceInstance, displayId}
pub const CMD_CLEAR_ELV_BLOCK: u32 = 0x0073_012e;
pub const CLEAR_ELV_BLOCK_SIZE: usize = 8;
/// `NV0073_CTRL_CMD_SYSTEM_ARM_LIGHTWEIGHT_SUPERVISOR`: the next update only
/// changes the stall lock, so RM's supervisor may take its short path
pub const CMD_ARM_LIGHTWEIGHT_SUPERVISOR: u32 = 0x0073_012f;
pub const ARM_LWSV_SIZE: usize = 20;
pub const LWSV_ARM: usize = 8;
pub const LWSV_VRR_STATE: usize = 9;
pub const LWSV_V_ACTIVE: usize = 12;
pub const LWSV_VFP: usize = 16;
/// `NV0073_CTRL_CMD_SYSTEM_CONFIG_VRR_PSTATE_SWITCH`
pub const CMD_CONFIG_VRR_PSTATE_SWITCH: u32 = 0x0073_0134;
pub const PSTATE_SWITCH_SIZE: usize = 24;
pub const PS_VRR_STATE: usize = 8;
pub const PS_VRR_DIRTY: usize = 9;
pub const PS_VRR_ENABLED: usize = 10;
pub const PS_MAX_VBLANK_EXTENSION: usize = 12;

/// `NVCA7D_HEAD_SET_STALL_LOCK(head)`: ENABLE 0:0, MODE 2:2 (ONE_SHOT 1),
/// LOCK_PIN 8:4 (NONE 0), UNSTALL_MODE 12:12 (LINE_LOCK 1)
pub const M_HEAD_SET_STALL_LOCK: u32 = 0x2034;
/// `NVCA7D_HEAD_SET_DISPLAY_RATE(head)`: RUN_MODE 1:0 (CONTINUOUS 0,
/// ONE_SHOT 1), MIN_REFRESH 2:2, MIN_REFRESH_INTERVAL 25:4 in microseconds
pub const M_HEAD_SET_DISPLAY_RATE: u32 = 0x21a8;
const MIN_REFRESH_INTERVAL_MASK: u32 = (1 << 22) - 1;

/// DPCD 0x007 `DOWN_STREAM_PORT_COUNT`, bit 6: the sink can ignore the MSA
/// timing, what makes a DP sink Adaptive-Sync
pub const DPCD_MSA_TIMING_PAR_IGNORED: u8 = 1 << 6;
/// DPCD 0x107 `DOWNSPREAD_CTRL`, bit 7: told to ignore it
pub const DPCD_DOWNSPREAD_CTRL: u32 = 0x107;
pub const DPCD_MSA_TIMING_PAR_IGNORE_EN: u8 = 1 << 7;

/// The sink's range, from the EDID's range-limits descriptor (0xFD), the last
/// place `NvTiming_GetVrrFmin` (`nvt_util.c:372`) looks and the only one this
/// panel fills; DisplayID ranges and adaptive-sync blocks are not read. Rates
/// over 255 Hz need the descriptor's offset flags, which the EDID parser does
/// not keep: such a panel reads as having no range
pub fn edid_range(v_min_hz: u8, v_max_hz: u8) -> Option<(u32, u32)> {
    (v_min_hz != 0 && v_max_hz > v_min_hz).then_some((v_min_hz as u32, v_max_hz as u32))
}

/// Whether this output may run variable refresh in `mode`, and at what timeout:
/// `nvDpyIsAdaptiveSync`, `GetEdidTimeoutMicroseconds` and the 110% rule of
/// `nvAdjustHwModeTimingsForVrrEvo`. The error says which said no, for
/// `nvidia disp`
pub fn allowed(
    is_dp: bool,
    dpcd: Option<&DpcdCaps>,
    range: Option<(u32, u32)>,
    mode: Option<&Mode>,
) -> Result<u32, &'static str> {
    if !is_dp {
        return Err("not DisplayPort (HDMI VRR is not written)");
    }
    let Some(d) = dpcd else { return Err("no DPCD read") };
    if !d.msa_timing_par_ignored {
        return Err("the sink does not ignore MSA timing (DPCD 0x007 bit 6)");
    }
    let Some((fmin, _)) = range else { return Err("the EDID gives no refresh range") };
    let timeout = timeout_us(fmin);
    if let Some(m) = mode {
        if !mode_allows(m, timeout) {
            return Err("the mode is under 110% of the panel's lowest rate");
        }
    }
    Ok(timeout)
}

/// The longest a frame may last: 10^6 / fmin microseconds
pub fn timeout_us(fmin_hz: u32) -> u32 {
    1_000_000 / fmin_hz.max(1)
}

/// The mode's refresh in 1/10000 Hz, `nvGetRefreshRate10kHz`
fn refresh_10khz(m: &Mode) -> u64 {
    let total = m.h_total as u64 * m.v_total as u64;
    if total == 0 {
        return 0;
    }
    m.clock_khz as u64 * 10_000_000 / total
}

/// NVKMS refuses VRR when the mode is slower than 110% of fmin
pub fn mode_allows(m: &Mode, timeout_us: u32) -> bool {
    timeout_us != 0 && refresh_10khz(m) >= 1_000_000u64 * 11_000 / timeout_us as u64
}

/// One frame of `m`, in microseconds (`axb_div_c(v_total * 1000, h_total,
/// pclk_khz)` in `ConfigVrrPstateSwitch`)
pub fn frame_time_us(m: &Mode) -> u64 {
    if m.clock_khz == 0 {
        return 0;
    }
    m.v_total as u64 * 1000 * m.h_total as u64 / m.clock_khz as u64
}

/// How many lines the vertical blank may stretch before the timeout repeats the
/// frame: what RM is told to fit a memory-clock switch in
pub fn max_vblank_extension(m: &Mode, timeout_us: u32) -> u32 {
    let frame = frame_time_us(m);
    if frame == 0 || m.v_total == 0 {
        return 0;
    }
    let line_ns = frame * 1000 / m.v_total as u64;
    if line_ns == 0 {
        return 0;
    }
    ((timeout_us as u64).saturating_sub(frame) * 1000 / line_ns) as u32
}

/// `HEAD_SET_STALL_LOCK`'s word. Unstall by line lock either way, as
/// `EvoSetStallLockC9` writes it; enabled is one-shot with no lock pin
pub fn stall_lock_word(enable: bool) -> u32 {
    const UNSTALL_LINE_LOCK: u32 = 1 << 12;
    if enable { UNSTALL_LINE_LOCK | 1 | 1 << 2 } else { UNSTALL_LINE_LOCK }
}

/// `HEAD_SET_DISPLAY_RATE`'s word: one-shot with a minimum refresh of
/// `timeout_us` (none when zero), or continuous (`EvoSetDisplayRateC9`)
pub fn display_rate_word(enable: bool, timeout_us: u32) -> u32 {
    if !enable {
        return 0;
    }
    let t = timeout_us.min(MIN_REFRESH_INTERVAL_MASK);
    1 | t << 4 | if t == 0 { 0 } else { 1 << 2 }
}

/// The two methods, in the order `SetStallLockOneDisp` stages them. The
/// caller commits with `disp::core_update`, which releases ELV
pub fn head_methods(push: &mut PushBuf, head: u32, enable: bool, timeout_us: u32) -> Result<(), PushError> {
    let h = head * HEAD_STRIDE;
    push.mthd1(M_HEAD_SET_STALL_LOCK + h, stall_lock_word(enable))?;
    push.mthd1(M_HEAD_SET_DISPLAY_RATE + h, display_rate_word(enable, timeout_us))
}

pub fn display_info_params(display_id: u32, add: bool) -> [u8; VRR_DISPLAY_INFO_SIZE] {
    let mut p = [0u8; VRR_DISPLAY_INFO_SIZE];
    le::put_u32(&mut p, 4, display_id);
    p[8] = add as u8;
    p
}

pub fn clear_elv_params(display_id: u32) -> [u8; CLEAR_ELV_BLOCK_SIZE] {
    let mut p = [0u8; CLEAR_ELV_BLOCK_SIZE];
    le::put_u32(&mut p, 4, display_id);
    p
}

/// `nvEvoArmLightweightSupervisor`: vActive is the visible height, vfp the
/// front porch (`rasterSize.y - rasterBlankStart.y`)
pub fn lwsv_params(display_id: u32, arm: bool, vrr: bool, m: &Mode) -> [u8; ARM_LWSV_SIZE] {
    let mut p = [0u8; ARM_LWSV_SIZE];
    le::put_u32(&mut p, 4, display_id);
    p[LWSV_ARM] = arm as u8;
    p[LWSV_VRR_STATE] = vrr as u8;
    le::put_u32(&mut p, LWSV_V_ACTIVE, m.v_display);
    le::put_u32(&mut p, LWSV_VFP, m.v_sync_start.saturating_sub(m.v_display));
    p
}

/// `ConfigVrrPstateSwitch`: the extension is computed only when dirty
pub fn pstate_params(
    display_id: u32,
    enabled: bool,
    state: bool,
    dirty: bool,
    m: &Mode,
    timeout_us: u32,
) -> [u8; PSTATE_SWITCH_SIZE] {
    let mut p = [0u8; PSTATE_SWITCH_SIZE];
    le::put_u32(&mut p, 4, display_id);
    p[PS_VRR_STATE] = state as u8;
    p[PS_VRR_DIRTY] = dirty as u8;
    p[PS_VRR_ENABLED] = enabled as u8;
    if dirty {
        le::put_u32(&mut p, PS_MAX_VBLANK_EXTENSION, max_vblank_extension(m, timeout_us));
    }
    p
}

/// One of the four controls on the display object
pub fn control(rpc: &mut GspRpc, client: &RmClient, cmd: u32, params: &[u8]) -> Result<(), RmError> {
    client.control(rpc, H_DISPLAY, cmd, params).map(|_| ())
}

/// Heads whose stall lock is on, one bit each. The cursor reads it on every
/// move, from a path that may not take the output table's lock
static ACTIVE_HEADS: AtomicU32 = AtomicU32::new(0);

pub fn active_on(head: u32) -> bool {
    head < 32 && ACTIVE_HEADS.load(Ordering::Acquire) & (1 << head) != 0
}

pub fn set_active(head: u32, on: bool) {
    if head >= 32 {
        return;
    }
    if on {
        ACTIVE_HEADS.fetch_or(1 << head, Ordering::AcqRel);
    } else {
        ACTIVE_HEADS.fetch_and(!(1 << head), Ordering::AcqRel);
    }
}

/// The display went down: no head is stalled any more
pub fn forget() {
    ACTIVE_HEADS.store(0, Ordering::Release);
}
