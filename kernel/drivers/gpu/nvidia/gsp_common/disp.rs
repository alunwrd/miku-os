// Display enumeration through GSP-RM
//
// Which outputs the board has, which have a monitor, what that monitor can do,
// plus the channel machinery a modeset is pushed through; the modeset itself
// lives in `modeset`.
//
// Everything hangs off NV04_DISPLAY_COMMON, allocated under the device
// (displays belong to the board, not to one GPU of a multi-GPU device). It
// takes no allocation parameters.
//
// Display ids are a bitmask, one bit per output. GET_SUPPORTED says which bits
// exist; GET_CONNECT_STATE takes a mask in and returns the subset with
// something plugged in. Both are addressed to the display-common object, and
// each carries its own subDeviceInstance field, so every params block below
// starts with a zero.

use crate::drivers::gpu::nvidia::le;
use alloc::vec::Vec;

use super::pushbuf::{PushBuf, PushError};
use super::rmapi::{RmClient, RmError};
use super::rpc::GspRpc;

/// NV04_DISPLAY_COMMON (cl0073.h). Allocated with no parameters
pub const CLASS_DISPLAY_COMMON: u32 = 0x0073;

/// Handle for the display-common object
pub const H_DISPLAY: u32 = 0xd150_0000;

/// Heads on a GB20x board. `num_heads` and `head_mask` both confirm 4 on this
/// card; the constant spares loops a runtime value
pub const MAX_HEADS: u32 = 4;

pub const NV0073_CTRL_CMD_SYSTEM_GET_NUM_HEADS: u32 = 0x0073_0102;
pub const NV0073_CTRL_CMD_SYSTEM_GET_SUPPORTED: u32 = 0x0073_0107;
pub const NV0073_CTRL_CMD_SYSTEM_GET_CONNECT_STATE: u32 = 0x0073_0108;
pub const NV0073_CTRL_CMD_SPECIFIC_GET_EDID_V2: u32 = 0x0073_0245;

/// NV0073_CTRL_SPECIFIC_GET_EDID_MAX_EDID_BYTES
pub const EDID_MAX_BYTES: usize = 2048;

/// `NV2080_CTRL_CMD_INTERNAL_DISPLAY_WRITE_INST_MEM`.
///
/// The display engine needs a 64 KiB block of video memory for its instance
/// data; the host allocates it and hands GSP-RM the physical address. Without
/// it the display subsystem posts `NV_ERR_INVALID_DMA_SPECIFIER` (0x27), the
/// error for a memory descriptor an engine cannot address.
///
/// This goes to GSP-RM's internal subdevice handle from GspStaticConfigInfo, as
/// the whole NV2080_CTRL_CMD_INTERNAL_* family does.
pub const NV2080_CTRL_CMD_INTERNAL_DISPLAY_WRITE_INST_MEM: u32 = 0x2080_0a49;

/// Address-space selectors shared by the display's internal controls. FBMEM is
/// 2: 0 is unassigned, meaning "unspecified", and a control may accept it and
/// place the memory elsewhere
pub const ADDR_SYSMEM: u32 = 1;
pub const ADDR_FBMEM: u32 = 2;

/// PBTARGETAPERTURE, the enum that says how the GPU should reach a pushbuffer
pub const PHYS_NVM: u32 = 1;
pub const PHYS_PCI: u32 = 2;
pub const PHYS_PCI_COHERENT: u32 = 3;

/// NV_MEMORY_WRITECOMBINED
pub const MEM_WRITECOMBINED: u32 = 2;

/// Size and alignment nouveau uses for the block (`nvkm_gpuobj_new(device,
/// 0x10000, 0x10000, ...)`)
pub const INST_MEM_SIZE: u64 = 0x1_0000;
pub const INST_MEM_ALIGN: u64 = 0x1_0000;

/// Hand GSP-RM the display's instance memory.
///
/// `internal` must be the client built from GspStaticConfigInfo's
/// hInternalClient/Device/Subdevice: an RmControl carries both a client and an
/// object handle, and the internal subdevice belongs to the internal client
pub fn write_inst_mem(
    rpc: &mut GspRpc,
    internal: &RmClient,
    phys: u64,
    size: u64,
) -> Result<(), RmError> {
    // {instMemPhysAddr u64, instMemSize u64, instMemAddrSpace u32,
    //  instMemCpuCacheAttr u32} - both u64s are 8-aligned, so no padding
    let mut params = alloc::vec![0u8; 24];
    le::put_u64(&mut params, 0, phys);
    le::put_u64(&mut params, 8, size);
    le::put_u32(&mut params, 16, ADDR_FBMEM);
    le::put_u32(&mut params, 20, MEM_WRITECOMBINED);
    internal.control(
        rpc, internal.h_subdevice,
        NV2080_CTRL_CMD_INTERNAL_DISPLAY_WRITE_INST_MEM, &params,
    )?;
    Ok(())
}

/// Allocate NV04_DISPLAY_COMMON under the device. Once per boot: a second call
/// fails with NV_ERR_INSUFFICIENT_RESOURCES or similar
pub fn alloc_display(rpc: &mut GspRpc, client: &RmClient) -> Result<(), RmError> {
    client.alloc(rpc, client.h_device, H_DISPLAY, CLASS_DISPLAY_COMMON, &[])
}

/// `NV0073_CTRL_SYSTEM_GET_NUM_HEADS_PARAMS`: how many independent scanout
/// engines this board has, the ceiling on displays lit at once
pub fn num_heads(rpc: &mut GspRpc, client: &RmClient) -> Result<u32, RmError> {
    // {subDeviceInstance, flags, numHeads}
    let out = client.control_query(
        rpc, H_DISPLAY, NV0073_CTRL_CMD_SYSTEM_GET_NUM_HEADS, 12,
    )?;
    Ok(le::u32_or0(&out, 8))
}

/// `NV0073_CTRL_SYSTEM_GET_SUPPORTED_PARAMS`. Returns (displayMask,
/// displayMaskDDC): every output the board has, and the subset that has a DDC
/// line to read an EDID over
pub fn supported(rpc: &mut GspRpc, client: &RmClient) -> Result<(u32, u32), RmError> {
    // {subDeviceInstance, displayMask, displayMaskDDC}
    let out = client.control_query(
        rpc, H_DISPLAY, NV0073_CTRL_CMD_SYSTEM_GET_SUPPORTED, 12,
    )?;
    Ok((le::u32_or0(&out, 4), le::u32_or0(&out, 8)))
}

/// `NV0073_CTRL_SYSTEM_GET_CONNECT_STATE_PARAMS`. `probe` is the set of outputs
/// to test; the returned mask is the subset with a monitor.
///
/// A live probe: RM may drive DDC or an HPD check, so this can take a while
pub fn connect_state(
    rpc: &mut GspRpc,
    client: &RmClient,
    probe: u32,
) -> Result<u32, RmError> {
    // {subDeviceInstance, flags, displayMask, retryTimeMs}
    let mut params = alloc::vec![0u8; 16];
    le::put_u32(&mut params, 8, probe);
    let out = client.control(
        rpc, H_DISPLAY, NV0073_CTRL_CMD_SYSTEM_GET_CONNECT_STATE, &params,
    )?;
    Ok(le::u32_or0(&out, 8))
}

/// `NV0073_CTRL_SPECIFIC_GET_EDID_V2_PARAMS`. `display_id` is a single bit: an
/// EDID belongs to one monitor
pub fn read_edid(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
) -> Result<Vec<u8>, RmError> {
    // {subDeviceInstance, displayId, bufferSize, flags, edidBuffer[2048]}
    let mut params = alloc::vec![0u8; 16 + EDID_MAX_BYTES];
    le::put_u32(&mut params, 4, display_id);
    le::put_u32(&mut params, 8, EDID_MAX_BYTES as u32);
    let out = client.control(
        rpc, H_DISPLAY, NV0073_CTRL_CMD_SPECIFIC_GET_EDID_V2, &params,
    )?;
    if out.len() < 16 {
        return Err(RmError::ShortReply);
    }
    // bufferSize comes back as the number of bytes actually written
    let n = (le::u32_or0(&out, 8) as usize).min(out.len() - 16);
    Ok(out[16..16 + n].to_vec())
}

pub const NV0073_CTRL_CMD_SPECIFIC_GET_ALL_HEAD_MASK: u32 = 0x0073_0287;
pub const NV0073_CTRL_CMD_SPECIFIC_OR_GET_INFO: u32 = 0x0073_028b;

/// Which heads exist, as a bitmask. `num_heads` gives the count; this says
/// which indices are real, and the two can disagree on a cut-down board
pub fn head_mask(rpc: &mut GspRpc, client: &RmClient) -> Result<u32, RmError> {
    // {subDeviceInstance, headMask}
    let out = client.control_query(
        rpc, H_DISPLAY, NV0073_CTRL_CMD_SPECIFIC_GET_ALL_HEAD_MASK, 8,
    )?;
    Ok(le::u32_or0(&out, 4))
}

/// What RM says about one output resource: what kind sits behind a display id
/// and what protocol it speaks, including `location`, which tells an on-die SOR
/// from an external one. A TMDS link and a DisplayPort link have different
/// bring-ups, and DP needs link training first
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default)]
pub struct OrInfo {
    /// Index of the output resource (which SOR, typically)
    pub index: u32,
    pub or_type: u32,
    pub protocol: u32,
    pub location: u32,
}

pub const OR_TYPE_NONE: u32 = 0;
pub const OR_TYPE_DAC: u32 = 1;
pub const OR_TYPE_SOR: u32 = 2;
pub const OR_TYPE_PIOR: u32 = 3;
pub const OR_TYPE_DSI: u32 = 5;

impl OrInfo {
    pub fn type_name(&self) -> &'static str {
        match self.or_type {
            OR_TYPE_NONE => "none",
            OR_TYPE_DAC => "DAC",
            OR_TYPE_SOR => "SOR",
            OR_TYPE_PIOR => "PIOR",
            OR_TYPE_DSI => "DSI",
            _ => "?",
        }
    }

    /// Protocol names are per OR type: the numbers overlap, so read with
    /// `or_type`
    pub fn protocol_name(&self) -> &'static str {
        match (self.or_type, self.protocol) {
            (OR_TYPE_DAC, 0) => "RGB CRT",
            (OR_TYPE_SOR, 0) => "LVDS",
            (OR_TYPE_SOR, 1) => "TMDS-A",
            (OR_TYPE_SOR, 2) => "TMDS-B",
            (OR_TYPE_SOR, 5) => "dual TMDS",
            (OR_TYPE_SOR, 8) => "DP-A",
            (OR_TYPE_SOR, 9) => "DP-B",
            (OR_TYPE_SOR, 0x10) => "DSI",
            (_, 0xffff_ffff) => "unknown",
            _ => "?",
        }
    }

    /// An output resource not assigned yet reports its index as all ones. There
    /// are fewer SORs than connectors, and `DFP_ASSIGN_SOR` binds one at
    /// modeset time
    pub fn or_assigned(&self) -> Option<u32> {
        (self.index != u32::MAX).then_some(self.index)
    }

    /// True when a picture on this output requires DisplayPort link training
    pub fn is_dp(&self) -> bool {
        self.or_type == OR_TYPE_SOR && (self.protocol == 8 || self.protocol == 9)
    }

    /// True when this output speaks TMDS: DVI, and HDMI's video. No training or
    /// negotiation: the link runs at the pixel clock and the sink locks to it
    /// or does not
    pub fn is_tmds(&self) -> bool {
        self.or_type == OR_TYPE_SOR
            && (self.protocol == 1 || self.protocol == 2 || self.protocol == 5)
    }
}

/// `NV0073_CTRL_SPECIFIC_OR_GET_INFO_PARAMS`, 56 bytes. `display_id` is a
/// single bit, at +4
pub fn or_info(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
) -> Result<OrInfo, RmError> {
    let mut params = alloc::vec![0u8; 56];
    le::put_u32(&mut params, 4, display_id);
    let out = client.control(
        rpc, H_DISPLAY, NV0073_CTRL_CMD_SPECIFIC_OR_GET_INFO, &params,
    )?;
    Ok(OrInfo {
        index: le::u32_or0(&out, 8),
        or_type: le::u32_or0(&out, 12),
        protocol: le::u32_or0(&out, 16),
        location: le::u32_or0(&out, 28),
    })
}

pub const NV0073_CTRL_CMD_SPECIFIC_GET_CONNECTOR_DATA: u32 = 0x0073_0250;

/// The physical connector a display id belongs to.
///
/// A DP++ port answers to a DP id and a TMDS one, for passive adapters, both
/// naming one socket. RM says which in `data[].index`, what Linux groups by:
/// one DRM connector per index (`nv_drm_get_connector`,
/// `nvidia-drm-connector.c:586`), the display ids as encoders on it
#[derive(Clone, Copy, Debug, Default)]
pub struct ConnectorData {
    /// `data[0].index`, which every entry repeats (`AllocConnector`,
    /// `nvkms-rm.c`)
    pub physical_index: u32,
    /// `NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_*`, one per entry
    pub types: [u32; 4],
    pub count: u32,
}

/// `NV0073_CTRL_SPECIFIC_GET_CONNECTOR_DATA_PARAMS`, 72 bytes
/// (`ctrl0073specific.h:401`): displayId at +4, count at +16, then four
/// `{index, type, location}` from +20
pub fn connector_data(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
) -> Result<ConnectorData, RmError> {
    let mut params = alloc::vec![0u8; 72];
    le::put_u32(&mut params, 4, display_id);
    let out = client.control(
        rpc, H_DISPLAY, NV0073_CTRL_CMD_SPECIFIC_GET_CONNECTOR_DATA, &params,
    )?;
    let count = le::u32_or0(&out, 16).min(4);
    let mut types = [0u32; 4];
    for (i, t) in types.iter_mut().enumerate().take(count as usize) {
        *t = le::u32_or0(&out, 20 + 12 * i + 4);
    }
    Ok(ConnectorData {
        physical_index: le::u32_or0(&out, 20),
        types,
        count,
    })
}

// Display root and channels

// Blackwell (GB20x) display classes, from nouveau's `rm/gb20x.c`
// cross-referenced with `include/nvif/class.h`. The core channel carries a
// modeset
pub const GB202_DISP: u32 = 0xca70;
#[allow(dead_code)] // GB202 class id, kept beside the GB206 ones this chip uses
pub const GB202_DISP_CAPS: u32 = 0xca73;
pub const GB202_DISP_CORE_CHANNEL_DMA: u32 = 0xca7d;
pub const GB202_DISP_WINDOW_CHANNEL_DMA: u32 = 0xca7e;
pub const GB202_DISP_WINDOW_IMM_CHANNEL_DMA: u32 = 0xca7b;
#[allow(dead_code)] // GB202 class id, kept beside the GB206 ones this chip uses
pub const GB202_DISP_CURSOR: u32 = 0xca7a;

/// Allocate the display root object under the device.
///
/// NV04_DISPLAY_COMMON answers the NV0073_CTRL_* queries; this is the parent
/// display channels hang off. nouveau allocates both. The handle is the class
/// shifted left 16, keeping channel handles (class<<16 | instance) from
/// colliding
pub fn alloc_disp_root(rpc: &mut GspRpc, client: &RmClient) -> Result<u32, RmError> {
    let handle = GB202_DISP << 16;
    // No allocation parameters: nouveau passes size 0
    client.alloc(rpc, client.h_device, handle, GB202_DISP, &[])?;
    Ok(handle)
}

/// `NV2080_CTRL_CMD_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER`: tell GSP-RM where a
/// channel's pushbuffer lives, before allocating the channel.
///
/// So the channel allocation's params carry only an instance number and a put
/// offset. Goes to the internal subdevice like every INTERNAL_* control
pub const NV2080_CTRL_CMD_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER: u32 = 0x2080_0a58;

/// Where a pushbuffer lives, in the terms this control wants. All three
/// apertures the control accepts; the display channels use coherent sysmem
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum PushbufTarget {
    /// In video memory
    Vram,
    /// In system memory the GPU reads without snooping caches
    SysmemNonCoherent,
    /// In system memory with cache snooping
    SysmemCoherent,
}

impl PushbufTarget {
    fn encode(self) -> (u32, u32, u32) {
        // (addressSpace, cacheSnoop, pbTargetAperture)
        match self {
            PushbufTarget::Vram => (ADDR_FBMEM, 0, PHYS_NVM),
            PushbufTarget::SysmemNonCoherent => (ADDR_SYSMEM, 0, PHYS_PCI),
            PushbufTarget::SysmemCoherent => (ADDR_SYSMEM, 1, PHYS_PCI_COHERENT),
        }
    }
}

/// Register a channel's pushbuffer. `phys` and `size` describe the buffer;
/// `oclass` is the channel class and `instance` which of that class's channels
pub fn set_channel_pushbuf(
    rpc: &mut GspRpc,
    internal: &RmClient,
    oclass: u32,
    instance: u32,
    phys: u64,
    size: u64,
    target: PushbufTarget,
) -> Result<(), RmError> {
    let (address_space, cache_snoop, aperture) = target.encode();
    // {addressSpace u32, pad, physicalAddr u64, limit u64, cacheSnoop u32,
    //  hclass u32, channelInstance u32, valid u8, pbTargetAperture u32,
    //  channelPBSize u32, subDeviceId u32}
    let mut p = alloc::vec![0u8; PUSHBUF_PARAMS_SIZE];
    le::put_u32(&mut p, 0, address_space);
    le::put_u64(&mut p, 8, phys);
    // 'limit' is the last valid byte, not the length
    le::put_u64(&mut p, 16, size - 1);
    le::put_u32(&mut p, 24, cache_snoop);
    le::put_u32(&mut p, 28, oclass);
    le::put_u32(&mut p, 32, instance);
    // 'valid' is false only for the cursor class (low byte 0x7a), a PIO channel
    // with no pushbuffer
    p[36] = if oclass & 0xff != 0x7a { 1 } else { 0 };
    le::put_u32(&mut p, 40, aperture);
    // subDeviceId is one-hot; bit 0 is the only GPU on this board
    le::put_u32(&mut p, 48, 1u32);
    internal.control(
        rpc, internal.h_subdevice,
        NV2080_CTRL_CMD_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER, &p,
    )?;
    Ok(())
}

/// Size of NV2080_CTRL_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER_PARAMS on the r570
/// line, with pbTargetAperture/channelPBSize/subDeviceId over r535
const PUSHBUF_PARAMS_SIZE: usize = 56;

/// Allocate a DMA display channel under the display root.
///
/// `NV50VAIO_CHANNELDMA_ALLOCATION_PARAMETERS` has fields for a pushbuffer
/// handle and a control-region pointer, but the GSP path leaves them zero:
/// `set_channel_pushbuf` told GSP-RM about the buffer, and r570 adds only
/// `subDeviceId` to the instance and offset
pub fn alloc_dma_channel(
    rpc: &mut GspRpc,
    client: &RmClient,
    h_disp_root: u32,
    oclass: u32,
    instance: u32,
    put_offset: u32,
) -> Result<u32, RmError> {
    // {channelInstance u32, hObjectBuffer u32, hObjectNotify u32, offset u32,
    //  pControl u64 (8-aligned), flags u32, channelPBSize u32,
    //  subDeviceId u32}
    let mut p = alloc::vec![0u8; 40];
    le::put_u32(&mut p, 0, instance);
    le::put_u32(&mut p, 12, put_offset);
    le::put_u32(&mut p, 32, 1u32); // subDeviceId, one-hot
    let handle = (oclass << 16) | instance;
    client.alloc(rpc, h_disp_root, handle, oclass, &p)?;
    Ok(handle)
}

/// Allocate a PIO display channel: only the cursor-immediate channel is one.
///
/// `NV50VAIO_CHANNELPIO_ALLOCATION_PARAMETERS` is three fields against the DMA
/// form's eight. There is no pushbuffer, so no `set_channel_pushbuf` first. The
/// control region is the method space, at a fixed BAR0 offset
/// (`pushbuf::curs_chan_user`), and `pControl` is RM's own mapping of it
pub fn alloc_pio_channel(
    rpc: &mut GspRpc,
    client: &RmClient,
    h_disp_root: u32,
    oclass: u32,
    instance: u32,
) -> Result<u32, RmError> {
    // {channelInstance u32, hObjectNotify u32, pControl u64 (8-aligned)}
    let mut p = alloc::vec![0u8; 16];
    le::put_u32(&mut p, 0, instance);
    // No notifier, as NVKMS asks too (`nvAllocCursorEvo`, nvkms-cursor.c)
    let handle = (oclass << 16) | instance;
    client.alloc(rpc, h_disp_root, handle, oclass, &p)?;
    Ok(handle)
}

// Core channel method stream (NVCA7D)

// Method addresses from nouveau's `include/nvhw/class/clca7d.h`. Each family
// is an array indexed by window/head with a fixed stride
const M_WINDOW_SET_FORMAT_USAGE_BOUNDS: u32 = 0x1004;
const M_WINDOW_SET_ROTATED_FORMAT_USAGE_BOUNDS: u32 = 0x1008;
const M_WINDOW_SET_USAGE_BOUNDS: u32 = 0x1010;
const M_WINDOW_SET_PHYSICAL: u32 = 0x1014;
const WINDOW_STRIDE: u32 = 0x80;

const M_HEAD_SET_HEAD_USAGE_BOUNDS: u32 = 0x2030;
const M_HEAD_SET_TILE_MASK: u32 = 0x2060;
const HEAD_STRIDE: u32 = 0x800;

const M_TILE_SET_TILE_SIZE: u32 = 0x6000;
const TILE_STRIDE: u32 = 0x200;

pub const M_SET_INTERLOCK_FLAGS: u32 = 0x218;
const M_SET_WINDOW_INTERLOCK_FLAGS: u32 = 0x21c;
pub const M_UPDATE: u32 = 0x200;

/// GB20x geometry, matching what nouveau's `coreca7d_init` walks
pub const CORE_WINDOWS: u32 = 8;
pub const CORE_HEADS: u32 = 4;
/// Heads the class has methods for (`NVCA73_SYS_CAP_HEAD_EXISTS__SIZE_1`),
/// more than this chip drives
const CLASS_HEADS: u32 = 8;

/// `WINDOW_SET_WINDOW_USAGE_BOUNDS` with everything but the capacity field:
/// ILUT_ALLOWED 16:16, INPUT_SCALER_TAPS 22:20 = TAPS_2 (1), UPSCALING_ALLOWED
/// 24:24 = 0, LAYOUT 26:25 = PITCH_BLOCKLINEAR (0), TMO_LUT_ALLOWED 28:28.
/// Reads back as 0x10110000 on every window of a lit desktop
const USAGE_FLAGS: u32 = (1 << 16) | (1 << 20) | (1 << 28);

/// RGB_PACKED1BPP | 2BPP | 4BPP, bits 0..2. Bit 3 is the 8-byte format, which
/// the shipping driver never declares
const FORMAT_USAGE_RGB: u32 = 0b111;
/// What every window of a lit desktop carries except the one scanning out
const FORMAT_USAGE_NONE: u32 = 0;

/// `WINDOW_SET_PHYSICAL_WINDOW_NONE`: the field is a mask of physical windows,
/// so `NONE` is zero and physical window n is `1 << n`
const PHYSICAL_NONE: u32 = 0;

/// `nvGetMaxPixelsFetchedPerLine` from NVKMS `include/nvkms-evo3.h`.
///
/// The `+14` is Ampere's overfetch allowance, so tiled and two-head-one-OR
/// configurations have no artefacts at head boundaries; the `+8` is Turing's
/// fixed margin. `scale` is `SET_MAX_INPUT_SCALE_FACTOR.HORIZONTAL` in 1.10
/// fixed point, so 1024 means no downscaling. At 1920 with no scaling this
/// yields 1942, what the shipping driver has armed on a lit desktop
pub fn max_pixels_fetched_per_line(width: u32, scale: u32) -> u32 {
    ((((width + 14) * scale + 1023) >> 10) + 8) & 0x7fff
}

/// `SET_MAX_INPUT_SCALE_FACTOR.HORIZONTAL` for the unscaled case
pub const SCALE_FACTOR_1X: u32 = 1024;

/// Hand the two windows of `head` their physical windows, and give the scanning
/// one the line-buffer capacity for `width` pixels.
///
/// Apart from `core_channel_init`, since neither head nor mode is known then,
/// as in the shipping driver: its init leaves every window at flags-only, and
/// the modeset raises the window it uses. A window's sibling still gets a
/// physical window: on a lit desktop, head 3's windows 6 and 7 hold physical
/// windows 0 and 1 while only window 6 carries a pixel count
pub fn core_window_usage(
    push: &mut PushBuf,
    head: u32,
    width: u32,
    phywins: [u32; 2],
) -> Result<(), PushError> {
    for k in 0..2u32 {
        let w = (head * 2 + k) * WINDOW_STRIDE;
        let pixels = if k == 0 {
            max_pixels_fetched_per_line(width, SCALE_FACTOR_1X)
        } else {
            0
        };
        // Formats, on the scanning window only: RGB packed at 1, 2 and 4 bytes
        // per pixel, not the 8-byte format. The hardware budgets bandwidth and
        // buffers against these bounds, so overclaiming reserves for a worst
        // case
        if k == 0 {
            push.method(M_WINDOW_SET_FORMAT_USAGE_BOUNDS + w, &[FORMAT_USAGE_RGB, 0])?;
        }
        // SET_USAGE_BOUNDS and SET_PHYSICAL are adjacent, so one packet covers
        // both, as in the driver's stream
        push.method(M_WINDOW_SET_USAGE_BOUNDS + w, &[USAGE_FLAGS | pixels, phywins[k as usize]])?;
    }
    Ok(())
}

/// `UPDATE` with RELEASE_ELV set: commit the methods staged so far.
///
/// RELEASE_ELV applies the change at the next safe point without holding the
/// pipeline; SPECIAL_HANDLING NONE and INHIBIT_INTERRUPTS FALSE are zero, so
/// the whole word is bit 0
const UPDATE_RELEASE_ELV: u32 = 1;

/// Initialise the core channel: declare what each window and head may do, then
/// commit.
///
/// `coreca7d_init` from nouveau. Nothing is displayed: the methods are usage
/// bounds, the hardware's promise about what a later modeset may ask for. Runs
/// before any real modeset, the smallest stream proving the channel executes
/// methods
pub fn core_channel_init(push: &mut PushBuf) -> Result<(), PushError> {
    // The scaler coefficient RAM is not loaded here, although
    // `EvoInitChannelC9` loads it at core-channel init: four heads' worth is
    // 1536 dwords and PUT tops out at 1024. It goes into the modeset batch for
    // the one head being lit: 384 dwords

    for i in 0..CORE_WINDOWS {
        let w = i * WINDOW_STRIDE;
        // No formats here: a window not scanning out reads 0 on a lit desktop,
        // and the one that is gets its formats in `core_window_usage`
        push.method(
            M_WINDOW_SET_FORMAT_USAGE_BOUNDS + w,
            &[FORMAT_USAGE_NONE, 0],
        )?;
        // Permissions only, no capacity: MAX_PIXELS_FETCHED_PER_LINE stays 0
        // and is raised in `core_window_usage` for the window that scans out.
        //
        // nouveau declares 0x7fff on all eight windows at init, but this field
        // reserves line buffer, a fixed shared resource. Eight windows asking
        // for 32767 pixels each is a quarter of a million pixels of buffer that
        // does not exist, and a window whose reservation cannot be satisfied is
        // never selected for fetch: a black screen behind a good raster. On a
        // lit desktop this chip carries WINDOW_USAGE_BOUNDS 0x10110000 on every
        // idle window and raises only the scanning one
        push.method(M_WINDOW_SET_USAGE_BOUNDS + w, &[USAGE_FLAGS, PHYSICAL_NONE])?;
        let _ = M_WINDOW_SET_ROTATED_FORMAT_USAGE_BOUNDS;
    }

    for i in 0..CORE_HEADS {
        let h = i * HEAD_STRIDE;
        // CURSOR 2:0 = USAGE_W256_H256 (4), OLUT_ALLOWED 4:4,
        // OUTPUT_SCALER_TAPS 14:12 = TAPS_2 (1). UPSCALING_ALLOWED 8:8 stays
        // off: nouveau's init sets it, the shipping driver on a lit desktop
        // does not (0x1014)
        let bounds = 4 | (1 << 4) | (1 << 12);
        push.mthd1(M_HEAD_SET_HEAD_USAGE_BOUNDS + h, bounds)?;
        // Tiles and physical windows are assigned in `core_claim_tiles`
    }
    Ok(())
}

/// Take the tile and physical windows this head was born with, and take them
/// away from every head not being lit.
///
/// The hardware comes up with tile n on head n and physical window n on window
/// n. `EvoInitWindowMappingCA` clears those defaults on every inactive head
/// during the first modeset after the core channel is allocated: otherwise a
/// later modeset can hand a tile or physical window to two heads at once,
/// "which is not allowed and causes XID 56 or display engine hang". nouveau's
/// `coreca7d_init` writes `TILE_MASK = BIT(i)` to all four heads and leaves it
/// there.
///
/// For the head being lit, the inherited assignment is kept:
/// `EvoInitHwHeadMultiTileConfigCA` reads `TILE_MASK` and `WINDOW_SET_PHYSICAL`
/// back from the channel and adopts what the hardware chose. The tile number
/// need not equal the head number: on a lit desktop this chip drives head 3
/// through tile 4.
///
/// Returns the tile index this head owns, for `TILE_SET_TILE_SIZE`
pub fn core_claim_tiles(
    push: &mut PushBuf,
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    head: u32,
    width: u32,
    keep: &[(u32, u32)],
    assigned: Option<u32>,
) -> Result<u32, PushError> {
    claim_tiles(push, &armed_tile_masks(bar0), head, width, keep, assigned)
}

/// Every head's armed tile mask, read once - what `claim_tiles` decides from
pub fn armed_tile_masks(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion) -> [u32; CORE_HEADS as usize] {
    core::array::from_fn(|h| armed_tile_mask(bar0, h as u32))
}

/// `core_claim_tiles` with the armed masks already read: methods out, nothing
/// read, so the display layer's update can be built and counted before the core
/// channel is touched (`blackwell/apply.rs`)
pub fn claim_tiles(
    push: &mut PushBuf,
    armed: &[u32; CORE_HEADS as usize],
    head: u32,
    width: u32,
    keep: &[(u32, u32)],
    assigned: Option<u32>,
) -> Result<u32, PushError> {
    let armed_mask = |h: u32| armed.get(h as usize).copied().unwrap_or(0);

    // Tiles somebody else is compositing into. Two heads on one tile is the
    // state NVKMS warns ends in a display engine hang, and the power-on
    // assignment does not avoid it
    let taken: u32 = keep.iter().map(|&(h, _)| armed_mask(h)).fold(0, |a, m| a | m);

    // What `modeset::assign` handed out, when the caps were read. Otherwise
    // this head's own inherited assignment minus what is spoken for, then its
    // like-numbered tile, then the lowest free one: a head with no tile
    // composites nothing and reports nothing
    let inherited = armed_mask(head) & !taken;
    let tiles = if let Some(t) = assigned.filter(|&t| t != 0) {
        t
    } else if inherited != 0 {
        inherited
    } else if taken & (1 << head) == 0 {
        1 << head
    } else {
        let free = (!taken).trailing_zeros();
        if free >= CORE_HEADS {
            return Err(PushError::Full);
        }
        1 << free
    };

    for i in 0..CORE_HEADS {
        if i == head {
            push.mthd1(M_HEAD_SET_TILE_MASK + i * HEAD_STRIDE, tiles)?;
        } else if let Some(&(_, w)) = keep.iter().find(|(h, _)| *h == i) {
            // A head already scanning keeps what it has, re-staged from the
            // armed half: an UPDATE arms the assembly copy wholesale, so every
            // head's assembly has to be true at every commit
            let m = armed_mask(i);
            push.mthd1(M_HEAD_SET_TILE_MASK + i * HEAD_STRIDE, m)?;
            if m != 0 {
                push.mthd1(
                    M_TILE_SET_TILE_SIZE + m.trailing_zeros() * TILE_STRIDE, w << 16,
                )?;
            }
        } else {
            push.mthd1(M_HEAD_SET_TILE_MASK + i * HEAD_STRIDE, 0)?;
        }
    }
    // Heads past those this chip drives get their like-numbered tile at
    // power-on too, so tile 4, the one head 3 composites into on a lit desktop,
    // starts owned by head 4 as well. 615's `EvoInitWindowMappingCA` clears
    // every inactive head the class has, "some chips have more than
    // pDevEvo->numHeads"
    for i in CORE_HEADS..CLASS_HEADS {
        push.mthd1(M_HEAD_SET_TILE_MASK + i * HEAD_STRIDE, 0)?;
    }
    // Every window belonging to no driven head gives up its physical window;
    // this head's two are claimed in `core_window_usage`, and a lit head's two
    // are already claimed
    for w in 0..CORE_WINDOWS {
        let owner = w / 2;
        if owner != head && !keep.iter().any(|&(h, _)| h == owner) {
            push.mthd1(M_WINDOW_SET_PHYSICAL + w * WINDOW_STRIDE, PHYSICAL_NONE)?;
        }
    }

    // One tile spanning the whole visible width: `SetTileSize` gives the last
    // tile `hActive - tileStart`. WIDTH is 30:16 and START 14:0
    let tile = tiles.trailing_zeros();
    push.mthd1(M_TILE_SET_TILE_SIZE + tile * TILE_STRIDE, width << 16)?;
    Ok(tile)
}

/// Give a head's tiles and its windows' physical windows back: what
/// `EvoSetMultiTileConfigCA` writes for a head being shut down: an empty
/// config, `HEAD_SET_TILE_MASK` 0 and `WINDOW_SET_PHYSICAL` none for each of
/// its layers (`dev/display-nvkms.md`, decision 7)
pub fn head_release_tiles(push: &mut PushBuf, head: u32, windows: &[u32]) -> Result<(), PushError> {
    push.mthd1(M_HEAD_SET_TILE_MASK + head * HEAD_STRIDE, 0)?;
    for &w in windows {
        push.mthd1(M_WINDOW_SET_PHYSICAL + w * WINDOW_STRIDE, PHYSICAL_NONE)?;
    }
    Ok(())
}

/// Armed core-channel state lives 0x8000 above the staging copy
const CORE_ARM_BASE: u32 = 0x0068_8000;

/// The tiles a head composites into now, from the armed half: what
/// `EvoInitHwHeadMultiTileConfigCA` reads back as the head's inherited tiles
pub fn armed_tile_mask(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion, head: u32) -> u32 {
    bar0.read32(CORE_ARM_BASE + M_HEAD_SET_TILE_MASK + head * HEAD_STRIDE)
}

/// The physical windows a window fetches through right now, from the armed
/// half (`WINDOW_SET_PHYSICAL`, the word after `WINDOW_SET_USAGE_BOUNDS`)
pub fn armed_window_physical(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion, window: u32) -> u32 {
    bar0.read32(CORE_ARM_BASE + M_WINDOW_SET_PHYSICAL + window * WINDOW_STRIDE)
}

/// Commit staged methods. `curs_interlock` and `wndw_interlock` are zero until
/// cursor and window channels exist
pub fn core_update(
    push: &mut PushBuf,
    curs_interlock: u32,
    wndw_interlock: u32,
) -> Result<(), PushError> {
    push.method(M_SET_INTERLOCK_FLAGS, &[curs_interlock, wndw_interlock])?;
    let _ = M_SET_WINDOW_INTERLOCK_FLAGS;
    push.mthd1(M_UPDATE, UPDATE_RELEASE_ELV)
}

/// The same commit, asking the hardware to report when it is done.
///
/// `GET` catching `PUT` says only that the channel read the methods, and an
/// interlocked update that never finds its partner waits forever with no error.
/// `coreca7d_update` uses a notifier: a 16-byte record whose first word carries
/// STATUS in bits 31:30, set to FINISHED when the update completes.
///
/// `notifier_phys` is 16-byte aligned and prepared with `notifier_reset`.
/// `target` says which memory: `coreca7d_update` puts the record in video
/// memory and addresses it as `PHYSICAL_NVM`, the reference driver's only
/// combination. The class header also lists `PHYSICAL_PCI_COHERENT`, but this
/// engine rejects a system-memory scanout surface, and an unwritten notifier
/// looks like an update that never completed
pub fn core_update_notify(
    push: &mut PushBuf,
    curs_interlock: u32,
    wndw_interlock: u32,
    notifier_phys: u64,
    target: u32,
) -> Result<(), PushError> {
    push.mthd1(M_SET_SURFACE_ADDRESS_HI_NOTIFIER, (notifier_phys >> 32) as u32)?;
    push.mthd1(
        M_SET_SURFACE_ADDRESS_LO_NOTIFIER,
        (notifier_phys as u32 & !0xf) | (target << 2) | 1,
    )?;
    // MODE_WRITE (0) plus NOTIFY_ENABLE at bit 12
    push.mthd1(M_SET_NOTIFIER_CONTROL, 1 << 12)?;
    push.method(M_SET_INTERLOCK_FLAGS, &[curs_interlock, wndw_interlock])?;
    push.mthd1(M_UPDATE, UPDATE_RELEASE_ELV)?;
    // Left armed, the next unrelated update would overwrite the record
    push.mthd1(M_SET_NOTIFIER_CONTROL, 0)
}

const M_SET_NOTIFIER_CONTROL: u32 = 0x020c;
const M_SET_SURFACE_ADDRESS_HI_NOTIFIER: u32 = 0x0260;
const M_SET_SURFACE_ADDRESS_LO_NOTIFIER: u32 = 0x0264;
/// `SET_SURFACE_ADDRESS_LO_NOTIFIER_TARGET`, bits 3:2
pub const NOTIFIER_TARGET_VRAM: u32 = 1;
pub const NOTIFIER_TARGET_PCI_COHERENT: u32 = 3;

/// STATUS is bits 31:30: NOT_BEGUN 0, BEGUN 1, FINISHED 2
pub const NOTIFIER_STATUS_FINISHED: u32 = 2;

/// Put the record back to NOT_BEGUN before arming an update on it
pub fn notifier_reset(mem: &mut [u8]) {
    for b in mem.iter_mut().take(16) {
        *b = 0;
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// Current STATUS of the record
pub fn notifier_status(mem: &[u8]) -> u32 {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    let w = u32::from_le_bytes([mem[0], mem[1], mem[2], mem[3]]);
    w >> 30
}

// DisplayPort AUX channel

/// `NV0073_CTRL_CMD_DP_AUXCH_CTRL`: run one transaction on a DisplayPort AUX
/// channel. RM drives the wire.
///
/// AUX is the sideband link every DP monitor answers on, and DPCD is its
/// register space. The monitor's claims about lane count and bandwidth are what
/// training negotiates down from
pub const NV0073_CTRL_CMD_DP_AUXCH_CTRL: u32 = 0x0073_1341;

/// Bytes per transaction. DPCD reads longer than this have to be split
pub const AUX_MAX_DATA: usize = 16;

/// `cmd` field: bit 3 selects AUX over I2C, bits 1:0 the request type
const AUX_CMD_TYPE_AUX: u32 = 1 << 3;
const AUX_REQ_READ: u32 = 1;

/// Read up to 16 bytes of DPCD from the monitor behind `display_id`.
///
/// Returns what the monitor sent, possibly shorter than asked: RM writes the
/// achieved length back into `size`
pub fn dpcd_read(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    addr: u32,
    len: usize,
) -> Result<Vec<u8>, RmError> {
    let len = len.min(AUX_MAX_DATA);
    // {subDeviceInstance, displayId, bAddrOnly, cmd, addr, data[16], size,
    // replyType, retryTimeMs}: 48 bytes, no padding
    let mut params = alloc::vec![0u8; 48];
    le::put_u32(&mut params, 4, display_id);
    params[8] = 0; // bAddrOnly: we want data back, not just an address probe
    le::put_u32(&mut params, 12, AUX_CMD_TYPE_AUX | AUX_REQ_READ);
    le::put_u32(&mut params, 16, addr);
    // 'size' is the count minus one, the way the DP spec encodes it
    le::put_u32(&mut params, 36, (len as u32).saturating_sub(1));

    let out = client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DP_AUXCH_CTRL, &params)?;
    if out.len() < 48 {
        return Err(RmError::ShortReply);
    }
    let got = (le::u32_or0(&out, 36) as usize).min(AUX_MAX_DATA);
    Ok(out[20..20 + got].to_vec())
}

/// `NV0073_CTRL_DP_AUXCH_CMD_REQ_TYPE_WRITE`, and the one reply that means the
/// sink took it (`_REPLYTYPE_ACK`)
const AUX_REQ_WRITE: u32 = 0;
const AUX_REPLY_ACK: u32 = 0;
/// What a DPCD write not acknowledged reports, with the AUX reply type (NACK 1,
/// DEFER 2, TIMEOUT 3) in the low bits: RM said OK, the sink did not
pub const AUX_NOT_ACKED: u32 = 0xa0c0_0000;
/// `NV_ERR_INVALID_ARGUMENT`
const NV_ERR_INVALID_ARGUMENT: u32 = 0x1f;

/// Write up to 16 DPCD bytes at `addr`: the control of `dpcd_read`, with the
/// request type a native AUX write. Success is the sink's ACK as well as RM's
/// status
pub fn dpcd_write(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
    addr: u32,
    data: &[u8],
) -> Result<(), RmError> {
    if data.is_empty() || data.len() > AUX_MAX_DATA {
        return Err(RmError::Status(NV_ERR_INVALID_ARGUMENT));
    }
    let mut params = alloc::vec![0u8; 48];
    le::put_u32(&mut params, 4, display_id);
    le::put_u32(&mut params, 12, AUX_CMD_TYPE_AUX | AUX_REQ_WRITE);
    le::put_u32(&mut params, 16, addr);
    params[20..20 + data.len()].copy_from_slice(data);
    le::put_u32(&mut params, 36, data.len() as u32 - 1);
    let out = client.control(rpc, H_DISPLAY, NV0073_CTRL_CMD_DP_AUXCH_CTRL, &params)?;
    let reply = le::u32_or0(&out, 40) & 0xf;
    if reply != AUX_REPLY_ACK {
        return Err(RmError::Status(AUX_NOT_ACKED | reply));
    }
    Ok(())
}

/// DPCD 0x0000E bit 7: the sink keeps its real capabilities at 0x2200, the
/// Extended Receiver Capability field (DPCD 1.4 and later)
const DPCD_TRAINING_AUX_RD_INTERVAL: usize = 0x0e;
const DPCD_EXTENDED_RX_CAP: u8 = 1 << 7;
const DPCD_EXTENDED_REV: u32 = 0x2200;

/// The sink's capability block as `DPCDHALImpl::parseAndReadCaps` reads it
/// (`dp_configcaps.cpp:158-174`): the first sixteen bytes, and when they say
/// the extended field is present, the sixteen at 0x2200: a 1.4 sink may report
/// 1.2 at 0x0000 for old sources and keep its real rate and capabilities there.
/// True in the second place when the extended block was parsed
pub fn read_dpcd_caps(
    rpc: &mut GspRpc,
    client: &RmClient,
    display_id: u32,
) -> Result<Option<(DpcdCaps, bool)>, RmError> {
    let base = dpcd_read(rpc, client, display_id, 0, 16)?;
    let extended = base.get(DPCD_TRAINING_AUX_RD_INTERVAL).is_some_and(|b| b & DPCD_EXTENDED_RX_CAP != 0);
    if extended {
        // A failed read of the extended block is NVKMS's "failed to read caps";
        // here the base block still answers
        if let Ok(ext) = dpcd_read(rpc, client, display_id, DPCD_EXTENDED_REV, 16) {
            if let Some(c) = DpcdCaps::parse(&ext) {
                return Ok(Some((c, true)));
            }
        }
    }
    Ok(DpcdCaps::parse(&base).map(|c| (c, false)))
}

/// The first 16 DPCD bytes, which every DP sink implements, decoded whole
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DpcdCaps {
    /// DPCD 0x00000: revision, packed as major<<4 | minor
    pub rev: u8,
    /// DPCD 0x00001: max link rate in 270 MHz units
    pub max_link_rate: u8,
    /// DPCD 0x00002 bits 4:0
    pub max_lane_count: u8,
    /// DPCD 0x00002 bit 6
    pub tps3_supported: bool,
    /// DPCD 0x00002 bit 7. Enhanced framing replaces the idle pattern between
    /// frames with a scrambled one, and both ends agree; link training is told
    /// whether to ask for it
    pub enhanced_frame_cap: bool,
    /// DPCD 0x00002 bit 5. The sink may ask for drive-level changes after
    /// training has finished
    pub post_lt_adj_cap: bool,
    /// DPCD 0x00003 bit 7. A TPS4-capable sink handles post-training adjustment
    /// its own way and must not be granted the older mechanism
    pub tps4_supported: bool,
    /// DPCD 0x00007 bit 6. The sink can ignore the stream's timing, what makes
    /// a DP sink Adaptive-Sync (`dp_configcaps.cpp:349`, `modeset::vrr`)
    pub msa_timing_par_ignored: bool,
}

impl DpcdCaps {
    pub fn parse(d: &[u8]) -> Option<Self> {
        if d.len() < 3 {
            return None;
        }
        Some(DpcdCaps {
            rev: d[0],
            max_link_rate: d[1],
            max_lane_count: d[2] & 0x1f,
            tps3_supported: d[2] & 0x40 != 0,
            enhanced_frame_cap: d[2] & 0x80 != 0,
            post_lt_adj_cap: d[2] & 0x20 != 0,
            tps4_supported: d.len() > 3 && d[3] & 0x80 != 0,
            msa_timing_par_ignored: d.len() > 7
                && d[7] & super::modeset::vrr::DPCD_MSA_TIMING_PAR_IGNORED != 0,
        })
    }

    /// Link rate as a name. The byte counts 270 MHz units, and the DP spec
    /// names the common ones
    pub fn rate_name(&self) -> &'static str {
        match self.max_link_rate {
            0x06 => "RBR 1.62G",
            0x0a => "HBR 2.7G",
            0x14 => "HBR2 5.4G",
            0x1e => "HBR3 8.1G",
            _ => "?",
        }
    }

    /// Peak link bandwidth in megabits per second, before 8b/10b overhead.
    /// 1920x1080 at 240 Hz in 24-bit colour needs roughly 12 Gbit/s of payload
    pub fn link_mbps(&self) -> u32 {
        self.max_link_rate as u32 * 270 * self.max_lane_count as u32
    }
}


// EDID parsing is `drivers::gpu::edid`: `manufacturer_str`, `name_str`,
// `product` and every timing, including those in the extensions.
