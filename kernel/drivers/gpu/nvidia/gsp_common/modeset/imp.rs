// IMP: asking RM whether the display engine can carry a set of heads
//
// The link check (`blackwell::modeset::mode_possible`) answers for the cable.
// The engine has its own budget (display clock, memory bandwidth, tiles per
// head), and RM knows it: `NVC372_CTRL_CMD_IS_MODE_POSSIBLE`, sent to an
// `NVC372_DISPLAY_SW` object under the device (`nvEvoAllocRmCtrlObjectC3`,
// `nvkms-evo3.c:6985`). The control is `ROUTE_TO_PHYSICAL`, so GSP-RM answers
// it.
//
// What goes in is NVKMS's: `nvEvoSetCtrlIsModePossibleParams3`
// (`nvkms-evo3.c:3298`), `AssignPerHeadImpParams`, `AssignPerWindowImpParams`,
// `nvComputeMinFrameIdle` (`:1324`) and the GB20x half
// `EvoSetCtrlIsModePossibleParamsCA` (`nvkms-evo4.c:1885`). What comes back is
// read as in `EvoIsModePossibleCA` (`:2324`): possible or not, and how many
// tiles each head needs (`tilingAssignments[0]`).
//
// A modeset's question also carries `NEED_MIN_VPSTATE`, as NVKMS's does when it
// reallocates bandwidth (`nvEvoSetCtrlIsModePossibleParams3`); a validation's
// does not. NVKMS never reads the answer on a dGPU; this driver keeps
// `minPState` per head for `nvidia perf`, the lowest P-state the lit display
// allows (`dev/gpu-perf.md`).
//
// A second opinion (`dev/display-nvkms.md`, decision 2): logged beside the
// link's answer, refusing nothing.
//
// The parameter block is 2048 bytes of C structs. Every offset below was
// printed by compiling `ctrlc372chnc.h` with `offsetof` (x86-64 and the GSP's
// RISC-V share the natural-alignment rules); the selftest pins them.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use super::super::disp;
use crate::drivers::gpu::nvidia::le;
use super::super::rmapi::{RmClient, RmError};
use super::super::rpc::GspRpc;
use super::Mode;

/// `NVC372_DISPLAY_SW`
pub const CLASS_DISPLAY_SW: u32 = 0xc372;
/// By the driver's `class << 16` convention
pub const H_DISPLAY_SW: u32 = CLASS_DISPLAY_SW << 16;
pub const NVC372_CTRL_CMD_IS_MODE_POSSIBLE: u32 = 0xc372_0101;

/// `sizeof(NVC372_CTRL_IS_MODE_POSSIBLE_PARAMS)`
pub const PARAMS_SIZE: usize = 2048;
/// `NVC372_CTRL_MAX_POSSIBLE_HEADS`, `_WINDOWS`, `_TILES`
pub const MAX_HEADS: usize = 8;
pub const MAX_WINDOWS: usize = 32;
pub const MAX_TILES: usize = 8;

// NVC372_CTRL_IS_MODE_POSSIBLE_PARAMS
pub const OFF_NUM_HEADS: usize = 4;
pub const OFF_NUM_WINDOWS: usize = 5;
pub const OFF_HEAD: usize = 8;
pub const HEAD_SIZE: usize = 92;
pub const OFF_WINDOW: usize = 744;
pub const WINDOW_SIZE: usize = 36;
pub const OFF_OPTIONS: usize = 1896;
pub const OFF_IS_POSSIBLE: usize = 1904;
pub const OFF_MIN_IMP_VPSTATE: usize = 1916;
pub const OFF_MIN_PSTATE: usize = 1920;
pub const OFF_MIN_REQUIRED_BANDWIDTH: usize = 1924;
pub const OFF_WORST_CASE_MARGIN: usize = 2000;
pub const OFF_DISP_CLK: usize = 2004;
pub const OFF_NUM_TILING_ASSIGNMENTS: usize = 2008;
pub const OFF_TILING_ASSIGNMENTS: usize = 2012;
pub const OFF_TILE_LIST: usize = 2020;
pub const OFF_WORST_CASE_DOMAIN: usize = 2036;

// NVC372_CTRL_IMP_HEAD
pub const H_HEAD_INDEX: usize = 0;
pub const H_MAX_PIXEL_CLK: usize = 4;
pub const H_RASTER_SIZE: usize = 8;
pub const H_RASTER_BLANK_START: usize = 16;
pub const H_RASTER_BLANK_END: usize = 24;
pub const H_CONTROL: usize = 40;
pub const H_MAX_DOWNSCALE: usize = 56;
pub const H_OUTPUT_SCALER_VTAPS: usize = 64;
pub const H_UPSCALING_ALLOWED_V: usize = 65;
pub const H_MIN_FRAME_IDLE: usize = 68;
pub const H_LUT: usize = 72;
pub const H_CURSOR_SIZE_32P: usize = 73;
pub const H_TILE_MASK: usize = 74;
pub const H_ENABLE_DSC: usize = 75;
pub const H_YUV420: usize = 88;
pub const H_2HEAD1OR: usize = 89;

// NVC372_CTRL_IMP_WINDOW
pub const W_WINDOW_INDEX: usize = 0;
pub const W_OWNING_HEAD: usize = 4;
pub const W_FORMAT_USAGE_BOUND: usize = 8;
pub const W_MAX_PIXELS_FETCHED: usize = 16;
pub const W_MAX_DOWNSCALE: usize = 20;
pub const W_INPUT_SCALER_VTAPS: usize = 28;
pub const W_UPSCALING_ALLOWED_V: usize = 29;
pub const W_LUT: usize = 31;
pub const W_TMO_LUT: usize = 32;

/// `NVC372_CTRL_IS_MODE_POSSIBLE_OPTIONS_NEED_MIN_VPSTATE`
pub const OPTIONS_NEED_MIN_VPSTATE: u32 = 0x2;

/// `NV_DISP_LOCK_MODE_NO_LOCK`, `NV_DISP_LOCK_PIN_UNSPECIFIED`
const LOCK_MODE_NO_LOCK: u32 = 0;
const LOCK_PIN_UNSPECIFIED: u32 = 0x10;
/// `NVC372_CTRL_IMP_LUT_USAGE_1025`: NVKMS assumes the full LUT everywhere
const LUT_USAGE_1025: u8 = 2;
/// `NV_EVO_SCALE_FACTOR_1X`, 1.10 fixed point
const SCALE_1X: u32 = 1 << 10;
/// A head that does not scale takes `NV_EVO_SCALER_2TAPS` in both directions
/// (`nvAssignScalerTaps`)
const MIN_SCALER_TAPS: u8 = 2;
/// `NVC372_CTRL_FORMAT_RGB_PACKED_1_BPP | _2_BPP | _4_BPP`: what
/// `disp::core_window_usage` declares on the scanning window
const FORMAT_RGB_PACKED_1_2_4: u32 = 0x1 | 0x2 | 0x4;

/// One head in the question
#[derive(Clone, Copy, Debug)]
pub struct ImpHead {
    pub head: u32,
    pub mode: Mode,
    /// The tiles it has, when lit and not being changed. Zero for a head being
    /// set: RM says how many it needs (`EvoSetCtrlIsModePossibleParamsCA`)
    pub tiles: u32,
}

/// What RM answered
#[derive(Clone, Debug, Default)]
pub struct Imp {
    pub possible: bool,
    /// Per hardware head, how many tiles the required assignment gives it
    pub tiles_per_head: [u8; MAX_HEADS],
    pub dispclk_khz: u32,
    pub min_bandwidth_kbps: u32,
    pub worst_margin: u32,
    /// The domain that limits it, as RM names it ("dispclk", "fbp", ...)
    pub worst_domain: String,
    /// `minImpVPState` and `minPState`, when the question asked for them
    /// (`NEED_MIN_VPSTATE`); P0 is 0
    pub min_vpstate: Option<u32>,
    pub min_pstate: Option<u32>,
}

impl Imp {
    pub fn describe(&self, head: u32) -> String {
        if self.possible {
            alloc::format!("yes (dispclk {} MHz, {} tile(s), {} MB/s, margin {}, {}{})",
                self.dispclk_khz / 1000, self.tiles_per_head[head as usize & 7],
                self.min_bandwidth_kbps / 1000, self.worst_margin, self.worst_domain,
                match (self.min_pstate, self.min_vpstate) {
                    (Some(p), Some(v)) => alloc::format!(", needs P{} (vP{}) or faster", p, v),
                    (Some(p), None) => alloc::format!(", needs P{} or faster", p),
                    _ => String::new(),
                })
        } else {
            alloc::format!("no ({}, margin {})", self.worst_domain, self.worst_margin)
        }
    }
}

/// `nvComputeMinFrameIdle` with no overscan border: the lines from vsync to
/// the first active line, and from the last active line to the end of the
/// frame. `None` below the hardware minimum of 2 leading lines
pub fn min_frame_idle(mode: &Mode) -> Option<(u16, u16)> {
    let r = mode.raster();
    let leading = r.v_blanke + 1;
    let trailing = r.v_active.checked_sub(leading + mode.v_display)?;
    if leading < 2 {
        return None;
    }
    Some((leading as u16, trailing as u16))
}

/// Build the parameter block for `heads` (`nvEvoSetCtrlIsModePossibleParams3` +
/// `EvoSetCtrlIsModePossibleParamsCA`). Each head brings its main window,
/// `2 * head`, declared as in `disp::core_window_usage`; a head's second window
/// carries no formats and is no layer for IMP (`pUsage->layer[layer].usable` is
/// false). `need_min_vpstate` is a modeset's question, against a validation's.
/// `None` for a mode the hardware cannot raster
pub fn params(heads: &[ImpHead], need_min_vpstate: bool) -> Option<Vec<u8>> {
    let mut p = alloc::vec![0u8; PARAMS_SIZE];
    // base.subdeviceIndex is 0: one GPU
    let n = heads.len().min(MAX_HEADS);
    p[OFF_NUM_HEADS] = n as u8;
    p[OFF_NUM_WINDOWS] = n as u8;
    for (i, h) in heads.iter().take(n).enumerate() {
        let b = OFF_HEAD + i * HEAD_SIZE;
        let r = h.mode.raster();
        let (leading, trailing) = min_frame_idle(&h.mode)?;
        p[b + H_HEAD_INDEX] = h.head as u8;
        le::put_u32(&mut p, b + H_MAX_PIXEL_CLK, h.mode.clock_khz);
        le::put_u32(&mut p, b + H_RASTER_SIZE, r.h_active);
        le::put_u32(&mut p, b + H_RASTER_SIZE + 4, r.v_active);
        le::put_u32(&mut p, b + H_RASTER_BLANK_START, r.h_blanks);
        le::put_u32(&mut p, b + H_RASTER_BLANK_START + 4, r.v_blanks);
        le::put_u32(&mut p, b + H_RASTER_BLANK_END, r.h_blanke);
        le::put_u32(&mut p, b + H_RASTER_BLANK_END + 4, r.v_blanke);
        // rasterVertBlank2 is for interlaced modes, and nothing here is
        le::put_u32(&mut p, b + H_CONTROL, LOCK_MODE_NO_LOCK);
        le::put_u32(&mut p, b + H_CONTROL + 4, LOCK_PIN_UNSPECIFIED);
        le::put_u32(&mut p, b + H_CONTROL + 8, LOCK_MODE_NO_LOCK);
        le::put_u32(&mut p, b + H_CONTROL + 12, LOCK_PIN_UNSPECIFIED);
        le::put_u32(&mut p, b + H_MAX_DOWNSCALE, SCALE_1X);
        le::put_u32(&mut p, b + H_MAX_DOWNSCALE + 4, SCALE_1X);
        p[b + H_OUTPUT_SCALER_VTAPS] = MIN_SCALER_TAPS;
        p[b + H_UPSCALING_ALLOWED_V] = 0;
        p[b + H_MIN_FRAME_IDLE..b + H_MIN_FRAME_IDLE + 2].copy_from_slice(&leading.to_le_bytes());
        p[b + H_MIN_FRAME_IDLE + 2..b + H_MIN_FRAME_IDLE + 4].copy_from_slice(&trailing.to_le_bytes());
        p[b + H_LUT] = LUT_USAGE_1025;
        // "Assume we use the maximum size": 256 pixels in units of 32
        p[b + H_CURSOR_SIZE_32P] = (256u32 / 32) as u8;
        p[b + H_TILE_MASK] = h.tiles as u8;
        p[b + H_ENABLE_DSC] = 0;
        p[b + H_YUV420] = 0;
        p[b + H_2HEAD1OR] = 0;

        // The head's main window (`AssignPerWindowImpParams`)
        let w = OFF_WINDOW + i * WINDOW_SIZE;
        le::put_u32(&mut p, w + W_WINDOW_INDEX, h.head * 2);
        le::put_u32(&mut p, w + W_OWNING_HEAD, h.head);
        le::put_u32(&mut p, w + W_FORMAT_USAGE_BOUND, FORMAT_RGB_PACKED_1_2_4);
        le::put_u32(&mut p, w + W_MAX_PIXELS_FETCHED,
            disp::max_pixels_fetched_per_line(h.mode.h_display, disp::SCALE_FACTOR_1X));
        le::put_u32(&mut p, w + W_MAX_DOWNSCALE, SCALE_1X);
        le::put_u32(&mut p, w + W_MAX_DOWNSCALE + 4, SCALE_1X);
        p[w + W_INPUT_SCALER_VTAPS] = MIN_SCALER_TAPS;
        p[w + W_UPSCALING_ALLOWED_V] = 0;
        p[w + W_LUT] = LUT_USAGE_1025;
        p[w + W_TMO_LUT] = LUT_USAGE_1025;
    }
    // Lock modes and pins are NO_LOCK above, as NVKMS assumes when it asks for
    // the v-pstate ("For now, just assume no locking")
    if need_min_vpstate {
        le::put_u32(&mut p, OFF_OPTIONS, OPTIONS_NEED_MIN_VPSTATE);
    }
    Some(p)
}

/// Read RM's answer from the block (`EvoIsModePossibleCA`), the minimum
/// v-pstate included when the block's `options` asked for it
pub fn parse(p: &[u8], heads: &[ImpHead]) -> Imp {
    parse_asked(p, heads, le::u32_or0(p, OFF_OPTIONS) & OPTIONS_NEED_MIN_VPSTATE != 0)
}

/// `parse`, told whether the question asked for the minimum v-pstate: the
/// request says so, whatever RM left in the reply's `options`
pub fn parse_asked(p: &[u8], heads: &[ImpHead], asked: bool) -> Imp {
    let mut imp = Imp {
        possible: p.get(OFF_IS_POSSIBLE).is_some_and(|&b| b != 0),
        dispclk_khz: le::u32_or0(p, OFF_DISP_CLK),
        min_bandwidth_kbps: le::u32_or0(p, OFF_MIN_REQUIRED_BANDWIDTH),
        worst_margin: le::u32_or0(p, OFF_WORST_CASE_MARGIN),
        min_vpstate: asked.then(|| le::u32_or0(p, OFF_MIN_IMP_VPSTATE)),
        min_pstate: asked.then(|| le::u32_or0(p, OFF_MIN_PSTATE)),
        ..Imp::default()
    };
    imp.worst_domain = p
        .get(OFF_WORST_CASE_DOMAIN..OFF_WORST_CASE_DOMAIN + 8)
        .map(|d| d.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect())
        .unwrap_or_default();
    let assignments = le::u32_or0(p, OFF_NUM_TILING_ASSIGNMENTS);
    if imp.possible && assignments == 0 {
        // "No tiling assigment returned by RM-IMP": NVKMS calls that impossible
        imp.possible = false;
    }
    if imp.possible {
        // The first assignment is the required one; each entry of the tile list
        // names the IMP head (an index into `head[]`) it goes to
        let tiles = p.get(OFF_TILING_ASSIGNMENTS).copied().unwrap_or(0) as usize;
        for t in 0..tiles.min(MAX_TILES) {
            let imp_head = p.get(OFF_TILE_LIST + t * 2).copied().unwrap_or(0xff) as usize;
            if let Some(h) = heads.get(imp_head) {
                imp.tiles_per_head[h.head as usize & 7] += 1;
            }
        }
    }
    imp
}

/// Allocate the object IMP is asked through. Once per display lifetime; it
/// goes with the client on unload
pub fn alloc_object(rpc: &mut GspRpc, client: &RmClient) -> Result<(), RmError> {
    client.alloc(rpc, client.h_device, H_DISPLAY_SW, CLASS_DISPLAY_SW, &[])
}

/// Ask RM (`EvoIsModePossibleCA`). `need_min_vpstate` for a modeset's question
pub fn is_mode_possible(
    rpc: &mut GspRpc,
    client: &RmClient,
    heads: &[ImpHead],
    need_min_vpstate: bool,
) -> Result<Imp, RmError> {
    let Some(p) = params(heads, need_min_vpstate) else {
        return Ok(Imp { worst_domain: "raster".into(), ..Imp::default() });
    };
    let reply = client.control(rpc, H_DISPLAY_SW, NVC372_CTRL_CMD_IS_MODE_POSSIBLE, &p)?;
    Ok(parse_asked(&reply, heads, need_min_vpstate))
}

// How often the link check and IMP agreed
static AGREED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static DISAGREED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Count one pair of answers about the same mode
pub fn tally(link: bool, imp: bool) {
    use core::sync::atomic::Ordering;
    if link == imp { &AGREED } else { &DISAGREED }.fetch_add(1, Ordering::Relaxed);
}

/// `(agreed, disagreed)` since boot
pub fn agreement() -> (u32, u32) {
    use core::sync::atomic::Ordering;
    (AGREED.load(Ordering::Relaxed), DISAGREED.load(Ordering::Relaxed))
}

/// The lowest P-state each head's display allows, as the last modeset's IMP
/// said: the P-state number plus one, 0 for not known
static FLOORS: [core::sync::atomic::AtomicU32; MAX_HEADS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_HEADS];

/// Keep what a modeset's IMP answer said `head` needs
pub fn note_floor(head: u32, min_pstate: Option<u32>) {
    use core::sync::atomic::Ordering;
    if let Some(f) = FLOORS.get(head as usize) {
        f.store(min_pstate.map_or(0, |p| p.saturating_add(1)), Ordering::Relaxed);
    }
}

/// The lowest P-state `head`'s display allows, if a modeset's IMP said
pub fn floor(head: u32) -> Option<u32> {
    use core::sync::atomic::Ordering;
    FLOORS.get(head as usize).map(|f| f.load(Ordering::Relaxed)).filter(|&v| v != 0).map(|v| v - 1)
}

/// The display is gone, and what its IMP said with it
pub fn forget_floors() {
    use core::sync::atomic::Ordering;
    for f in &FLOORS {
        f.store(0, Ordering::Relaxed);
    }
}
