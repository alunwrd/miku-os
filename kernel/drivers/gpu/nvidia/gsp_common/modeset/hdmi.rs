// TMDS, and HDMI on top of it
//
// A TMDS output needs no link training, symbol budget or stream configuration:
// the clock is the pixel clock, the encoding is fixed, and nothing is
// negotiated with the sink. The near side (head, raster, viewport, window) is
// shared; here are which SOR protocol the link speaks, and telling RM that the
// sink is an HDMI receiver.
//
// What "HDMI" means to RM
//
// RM does not read the EDID; `SET_HDMI_ENABLE` tells it, and the header says it
// "should be called prior to every modeset in which the displayId is capable of
// hdmi". It switches on the HDMI half of the transmitter: audio packets,
// islands, the data-island period in the blanking interval. HDMI's video is
// DVI's video, so a picture appears without it.
//
// Whether a connector is HDMI-capable comes from `DFP_GET_INFO`, RM's answer
// about the board's wiring, not the EDID's HDMI vendor block: a DVI monitor on
// an HDMI connector is still on an HDMI connector.

use crate::drivers::gpu::nvidia::le;

use super::super::disp::H_DISPLAY;
use super::super::rmapi::{RmClient, RmError};
use super::super::rpc::GspRpc;

// SOR protocols that are not DP

/// `NVCA7D_SOR_SET_CONTROL_PROTOCOL_SINGLE_TMDS_A` / `_B`, the two TMDS
/// sublinks, and `_DUAL_TMDS`, driving a mode too fast for one across both.
///
/// The same numbers `OR_GET_INFO` reports in its `protocol` field:
/// `NV0073_CTRL_SPECIFIC_OR_PROTOCOL_SOR_*` and the class enum agree on 1, 2,
/// 5, 8 and 9, so a connector's reported protocol goes to `sor_set_control`
/// unchanged
pub const SOR_PROTOCOL_SINGLE_TMDS_A: u32 = 1;
pub const SOR_PROTOCOL_SINGLE_TMDS_B: u32 = 2;
pub const SOR_PROTOCOL_DUAL_TMDS: u32 = 5;

/// The single-link TMDS ceiling, in kHz. Above this a mode needs both sublinks
/// (dual-link DVI) or HDMI 2.0's scrambling
pub const TMDS_SINGLE_LINK_MAX_KHZ: u32 = 165_000;

/// Above this an HDMI sink has to support scrambling and SCDC, and the source
/// has to be told. Below it, any HDMI or DVI receiver copes
pub const TMDS_SCRAMBLING_THRESHOLD_KHZ: u32 = 340_000;

/// Which SOR protocol to drive this mode on this connector with.
///
/// The connector's protocol says how the board is wired: a single-link
/// connector cannot become dual-link. A dual-link connector runs single below
/// the single-link ceiling, as the shipping driver does
pub fn tmds_protocol(or_protocol: u32, clock_khz: u32) -> u32 {
    match or_protocol {
        SOR_PROTOCOL_DUAL_TMDS if clock_khz <= TMDS_SINGLE_LINK_MAX_KHZ => {
            SOR_PROTOCOL_SINGLE_TMDS_A
        }
        SOR_PROTOCOL_DUAL_TMDS => SOR_PROTOCOL_DUAL_TMDS,
        SOR_PROTOCOL_SINGLE_TMDS_B => SOR_PROTOCOL_SINGLE_TMDS_B,
        // Includes a protocol this does not recognise: every SOR has sublink A
        _ => SOR_PROTOCOL_SINGLE_TMDS_A,
    }
}

/// Whether this driver drives `clock_khz` over TMDS at this sink.
///
/// The ceiling is the monitor's, when it named one. `caps` comes from the
/// vendor blocks of a CEA extension, so with no extension the answer falls back
/// to what any receiver copes with.
///
/// Past 340 MHz the link is scrambled and the sink says so: `SCDC_Present` in
/// its HDMI Forum block. A sink that cannot descramble stays dark, so refusing
/// brings a 4K60 panel up at a slower timing its EDID offers
pub fn mode_fits_tmds(clock_khz: u32, caps: &crate::drivers::gpu::edid::HdmiCaps) -> bool {
    // What the sink said it takes, if it said anything
    if caps.max_tmds_khz != 0 && clock_khz > caps.max_tmds_khz {
        return false;
    }
    if clock_khz > TMDS_SCRAMBLING_THRESHOLD_KHZ {
        return caps.scdc;
    }
    true
}

/// `NV0073_CTRL_CMD_SPECIFIC_SET_HDMI_SINK_CAPS`: tell RM what the sink can
/// descramble, so it programs a link above 340 MHz. Like `SET_HDMI_ENABLE`, the
/// only way RM learns what the monitor said
pub const NV0073_CTRL_CMD_SPECIFIC_SET_HDMI_SINK_CAPS: u32 = 0x0073_0293;

/// `{subDeviceInstance, displayId, caps}`, 12 bytes
pub const SET_HDMI_SINK_CAPS_PARAMS_SIZE: usize = 12;

/// `..._GT_340MHZ_CLOCK_SUPPORTED`, bit 0, and
/// `..._LTE_340MHZ_SCRAMBLING_SUPPORTED`, bit 1
pub const SINK_CAPS_GT_340MHZ: u32 = 1 << 0;
pub const SINK_CAPS_LTE_340MHZ_SCRAMBLING: u32 = 1 << 1;

pub fn set_hdmi_sink_caps(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    caps: &crate::drivers::gpu::edid::HdmiCaps,
) -> Result<u32, RmError> {
    let mut bits = 0u32;
    if caps.scdc {
        bits |= SINK_CAPS_GT_340MHZ;
    }
    if caps.scramble_below_340 {
        bits |= SINK_CAPS_LTE_340MHZ_SCRAMBLING;
    }
    let mut p = [0u8; SET_HDMI_SINK_CAPS_PARAMS_SIZE];
    le::put_u32(&mut p, 4, display_id);
    le::put_u32(&mut p, 8, bits);
    client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_SPECIFIC_SET_HDMI_SINK_CAPS, &p)?;
    Ok(bits)
}

// What RM says about a DFP

/// `NV0073_CTRL_CMD_DFP_GET_INFO`. A DFP is a digital flat panel: every output
/// but the analog DAC
pub const NV0073_CTRL_CMD_DFP_GET_INFO: u32 = 0x0073_1140;

/// r570 params: `{subDeviceInstance, displayId, flags, UHBRSupportedByDfp}`, 16
/// bytes, confirmed by compiling the C
pub const DFP_GET_INFO_PARAMS_SIZE: usize = 16;

/// What RM knows about one digital output, as a decoded `flags` word
#[derive(Copy, Clone, Debug, Default)]
pub struct DfpInfo {
    pub flags: u32,
}

impl DfpInfo {
    /// `NV0073_CTRL_DFP_FLAGS_SIGNAL`, bits 2:0
    pub fn signal(&self) -> u32 {
        self.flags & 0x7
    }

    /// TMDS is signal 0, also the value of a field never filled in, so this is
    /// trusted only with the connector's protocol from `OR_GET_INFO`
    pub fn is_tmds(&self) -> bool {
        self.signal() == SIGNAL_TMDS
    }

    /// `NV0073_CTRL_DFP_FLAGS_HDMI_CAPABLE`, bit 8: the board can speak HDMI
    /// on this connector
    pub fn hdmi_capable(&self) -> bool {
        self.flags & (1 << 8) != 0
    }

    /// `NV0073_CTRL_DFP_FLAGS_HDMI_ALLOWED`, bit 14. A board can be wired for
    /// HDMI and have it disabled in the VBIOS, and RM answers both
    pub fn hdmi_allowed(&self) -> bool {
        self.flags & (1 << 14) != 0
    }

    /// `NV0073_CTRL_DFP_FLAGS_LANE`, bits 5:3: NONE/SINGLE/DUAL/QUAD/OCT
    pub fn lane(&self) -> u32 {
        (self.flags >> 3) & 0x7
    }

    pub fn signal_name(&self) -> &'static str {
        match self.signal() {
            SIGNAL_TMDS => "TMDS",
            SIGNAL_LVDS => "LVDS",
            SIGNAL_SDI => "SDI",
            SIGNAL_DISPLAYPORT => "DisplayPort",
            SIGNAL_DSI => "DSI",
            SIGNAL_WRBK => "writeback",
            _ => "?",
        }
    }
}

pub const SIGNAL_TMDS: u32 = 0;
pub const SIGNAL_LVDS: u32 = 1;
pub const SIGNAL_SDI: u32 = 2;
pub const SIGNAL_DISPLAYPORT: u32 = 3;
pub const SIGNAL_DSI: u32 = 4;
pub const SIGNAL_WRBK: u32 = 5;

/// Ask RM what kind of digital output this display id is on
pub fn dfp_get_info(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
) -> Result<DfpInfo, RmError> {
    let mut p = [0u8; DFP_GET_INFO_PARAMS_SIZE];
    le::put_u32(&mut p, 4, display_id);
    // Not `control_query`, which sends a zeroed payload: this control needs the
    // display id in the buffer it answers into
    let out = client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DFP_GET_INFO, &p)?;
    Ok(DfpInfo { flags: le::u32_or0(&out, 8) })
}

// Telling RM it is HDMI

/// `NV0073_CTRL_CMD_SPECIFIC_SET_HDMI_ENABLE`
pub const NV0073_CTRL_CMD_SPECIFIC_SET_HDMI_ENABLE: u32 = 0x0073_0273;

/// `{NvU8 subDeviceInstance; NvU32 displayId; NvU8 enable;}`: 12 bytes with
/// padding after the first byte, confirmed by compiling the C. The display id
/// is at 4: one of the few controls in the family whose first field is not a
/// word
pub const SET_HDMI_ENABLE_PARAMS_SIZE: usize = 12;

/// Turn the HDMI half of the transmitter on or off for one output.
///
/// Before the modeset, as the header asks. Off for a DVI sink on an HDMI
/// connector: data islands would go into a receiver not expecting them
pub fn set_hdmi_enable(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    enable: bool,
) -> Result<(), RmError> {
    let mut p = [0u8; SET_HDMI_ENABLE_PARAMS_SIZE];
    le::put_u32(&mut p, 4, display_id);
    p[8] = enable as u8;
    client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_SPECIFIC_SET_HDMI_ENABLE, &p)?;
    Ok(())
}
