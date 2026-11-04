// What GSP-RM says about the board once it is up: GSP_INIT's answer
//
// The usable video-memory map. GSP-RM owns the framebuffer layout: its heap,
// the WPR regions and what the firmware reserved are carved out, and the
// leftovers arrive as a list of regions with per-region flags. `vram.rs` hands
// those addresses out.
//
// GSP-RM's internal client/device/subdevice handles. The
// NV2080_CTRL_CMD_INTERNAL_* family, including handing GSP-RM the display's
// instance memory, is addressed to those.
//
// From 615 on this is an NVKV list (`dev/gsp-615.md`, section 4), decoded as
// `_kgspStaticInfoKeyHandler` does (`gmcapi_gsp_config.h` for the keys). A key
// not read here is skipped, and so is one read with the wrong shape, as
// OpenRM's `NVKV_CASE_*` do.

use alloc::string::String;
use alloc::vec::Vec;

use super::nvkv::{self, Value};

mod key {
    pub const FB_REGION_COUNT: u16 = 0x0010;
    pub const FB_REGION_FLAGS: u16 = 0x0012;
    pub const VGX_VERSION: u16 = 0x0210;
    pub const INTERNAL_CLIENT_HANDLE: u16 = 0x0FF0;
    pub const INTERNAL_DEVICE_HANDLE: u16 = 0x0FF1;
    pub const INTERNAL_SUBDEVICE_HANDLE: u16 = 0x0FF2;
    pub const FB_LENGTH: u16 = 0x1010;
    pub const FB_REGION_BASE: u16 = 0x1011;
    pub const FB_REGION_LIMIT: u16 = 0x1012;
    pub const FB_REGION_RESERVED: u16 = 0x1013;
    pub const BAR1_PDE_BASE: u16 = 0x1020;
    pub const BAR2_PDE_BASE: u16 = 0x1021;
    pub const GPU_NAME_STRING: u16 = 0x2000;
}

/// `NV2080_CTRL_CMD_FB_GET_FB_REGION_INFO_MAX_ENTRIES`
const MAX_REGIONS: usize = 16;

/// One region of the video-memory map
#[derive(Debug, Clone, Copy, Default)]
pub struct FbRegion {
    /// First byte of the region, as a physical video-memory address
    pub base: u64,
    /// Last byte, inclusive: RM reports a limit, not a length
    pub limit: u64,
    /// Bytes RM has already spoken for inside this region
    pub reserved: u64,
    pub support_compressed: bool,
    pub support_iso: bool,
    pub protected: bool,
}

impl FbRegion {
    #[inline]
    pub fn size(&self) -> u64 {
        self.limit.saturating_sub(self.base).saturating_add(1)
    }

    /// nouveau's rule for "the driver may allocate here"
    /// (`r535_gsp_get_static_info_fb`): nothing reserved, not protected, and
    /// supporting both compression and isochronous access. Display scanout
    /// needs ISO, since it cannot miss a deadline
    #[inline]
    pub fn usable(&self) -> bool {
        self.reserved == 0 && !self.protected && self.support_compressed && self.support_iso
    }
}

/// The parts of the static configuration this driver keeps
#[derive(Debug, Clone, Default)]
pub struct StaticInfo {
    pub fb_length: u64,
    /// Page directory bases RM built for its own apertures
    pub bar1_pde_base: u64,
    pub bar2_pde_base: u64,
    /// GSP-RM's own object handles, for the NV2080_CTRL_CMD_INTERNAL_* family
    pub h_internal_client: u32,
    pub h_internal_device: u32,
    pub h_internal_subdevice: u32,
    pub regions: Vec<FbRegion>,
    /// The RPC version GSP-RM speaks for the vGPU-style RPCs
    /// (`rpcSetIpVersion`), major << 16 | minor
    pub vgx_version: u32,
    pub gpu_name: String,
    /// Keys read, and keys skipped (unknown here, or not the shape expected)
    pub keys: u32,
    pub skipped: u32,
}

impl StaticInfo {
    /// Regions the driver is allowed to allocate from
    pub fn usable_regions(&self) -> impl Iterator<Item = &FbRegion> {
        self.regions.iter().filter(|r| r.usable())
    }
}

/// Decode GSP_INIT's answer
pub fn decode(bytes: &[u8]) -> Result<StaticInfo, nvkv::DecodeError> {
    let mut info = StaticInfo::default();
    let mut regions = [FbRegion::default(); MAX_REGIONS];
    let mut count = 0usize;
    nvkv::decode(bytes, |index, k, v| {
        info.keys += 1;
        let i = index as usize;
        let read = match (k, v) {
            (key::FB_LENGTH, Value::U64(x)) => { info.fb_length = x; true }
            (key::BAR1_PDE_BASE, Value::U64(x)) => { info.bar1_pde_base = x; true }
            (key::BAR2_PDE_BASE, Value::U64(x)) => { info.bar2_pde_base = x; true }
            (key::INTERNAL_CLIENT_HANDLE, Value::U32(x)) => { info.h_internal_client = x; true }
            (key::INTERNAL_DEVICE_HANDLE, Value::U32(x)) => { info.h_internal_device = x; true }
            (key::INTERNAL_SUBDEVICE_HANDLE, Value::U32(x)) => { info.h_internal_subdevice = x; true }
            (key::VGX_VERSION, Value::U32(x)) => { info.vgx_version = x; true }
            (key::GPU_NAME_STRING, v @ Value::Bytes(_)) => { info.gpu_name = v.as_str().unwrap_or_default(); true }
            // `NVKV_CASE_MAX`: a count past the table is not taken
            (key::FB_REGION_COUNT, Value::U32(x)) if (x as usize) <= MAX_REGIONS => { count = x as usize; true }
            // `NVKV_CASE_IDX`: an index past the count is not taken; the count
            // comes first in GSP-RM's list
            (key::FB_REGION_BASE, Value::U64(x)) if i < count => { regions[i].base = x; true }
            (key::FB_REGION_LIMIT, Value::U64(x)) if i < count => { regions[i].limit = x; true }
            (key::FB_REGION_RESERVED, Value::U64(x)) if i < count => { regions[i].reserved = x; true }
            (key::FB_REGION_FLAGS, Value::U32(x)) if i < count => {
                regions[i].support_compressed = x & 1 != 0;
                regions[i].support_iso = x & 2 != 0;
                regions[i].protected = x & 4 != 0;
                true
            }
            _ => false,
        };
        if !read {
            info.skipped += 1;
        }
        true
    })?;
    info.regions = regions[..count].to_vec();
    Ok(info)
}
