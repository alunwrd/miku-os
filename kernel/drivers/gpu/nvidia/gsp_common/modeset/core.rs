// The core channel method stream (NVCA7D): head setup, the LUT and scaler
// tables it points at, and the BAR0 readback of what the head took.

use super::super::pushbuf::{PushBuf, PushError};
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

// The modeset modules are re-exported flat from `mod.rs`
use super::*;

// Core channel method stream (NVCA7D)

pub const M_SOR_SET_CONTROL: u32 = 0x0300;
pub const SOR_STRIDE: u32 = 0x20;

pub const M_HEAD_SET_PROCAMP: u32 = 0x2000;
pub const M_HEAD_SET_CONTROL_OUTPUT_RESOURCE: u32 = 0x2004;
pub const M_HEAD_SET_CONTROL: u32 = 0x2008;
pub const M_HEAD_SET_PIXEL_CLOCK_FREQUENCY: u32 = 0x200c;
pub const M_HEAD_SET_DITHER_CONTROL: u32 = 0x2018;
pub const M_HEAD_SET_DISPLAY_ID: u32 = 0x2020;
pub const M_HEAD_SET_PIXEL_CLOCK_FREQUENCY_MAX: u32 = 0x2028;
pub const M_HEAD_SET_CONTROL_OUTPUT_SCALER: u32 = 0x2014;
pub const M_HEAD_SET_MAX_OUTPUT_SCALE_FACTOR: u32 = 0x202c;
pub const M_HEAD_SET_VIEWPORT_POINT_IN: u32 = 0x2048;
pub const M_HEAD_SET_OVERSCAN_COLOR: u32 = 0x2078;
pub const M_HEAD_SET_PRESENT_CONTROL_CURSOR: u32 = 0x2098;
pub const M_HEAD_SET_DESKTOP_COLOR_ALPHA_RED: u32 = 0x2220;
pub const M_HEAD_SET_DESKTOP_COLOR_GREEN_BLUE: u32 = 0x2224;

/// HORIZONTAL_TAPS 3:0 and VERTICAL_TAPS 7:4, both 'TAPS_2' (1)
pub const OUTPUT_SCALER_TAPS_2: u32 = 0x11;
/// HORIZONTAL 15:0 and VERTICAL 31:16, 1.0 in 1.10 fixed point
pub const SCALE_FACTOR_1X_HV: u32 = 0x0400_0400;
/// 'PRESENT_CONTROL_CURSOR_MODE_MONO', what NVKMS writes when there is no
/// cursor
pub const PRESENT_CURSOR_MONO: u32 = 0;

/// Diagnostic desktop colour: pure red.
///
/// A third saturated colour, against green outside the viewport and white
/// inside it: whatever comes up red is the desktop colour, composition with no
/// memory in the path, and what stays white is the window's rectangle.
///
/// The components are half-floats, hence 0x3c00 and not 0xffff, a negative NaN.
/// Alpha is 8 bits and unsigned (7:0 against 31:16), and the colour goes
/// straight into the FP16 composition stage with no LUT.
///
/// All three at 0 is the shipping behaviour of 'InitDesktopColorC9'. Set one to
/// `FP16_ONE` for the diagnostic
pub const DESKTOP_DIAG_RED: u32 = 0;
pub const DESKTOP_DIAG_GREEN: u32 = 0;
pub const DESKTOP_DIAG_BLUE: u32 = 0;

pub const M_HEAD_SET_VIEWPORT_SIZE_IN: u32 = 0x204c;
pub const M_HEAD_SET_VIEWPORT_SIZE_OUT: u32 = 0x2058;
pub const M_HEAD_SET_RASTER_SIZE: u32 = 0x2064;
pub const M_HEAD_SET_RASTER_BLANK_END: u32 = 0x206c;
pub const M_HEAD_SET_RASTER_BLANK_START: u32 = 0x2070;
/// Bits 34:31 of the pixel clock. Class 4 widened the clock past the 31 bits
/// `SET_PIXEL_CLOCK_FREQUENCY` holds; NVKMS writes both halves
/// (`nvkms-evo4.c:EvoSetRasterParams9`, 209-271)
pub const M_HEAD_SET_PIXEL_CLOCK_FREQUENCY_HI: u32 = 0x20c0;
/// Every `NVCA7D_HEAD_*(a)` method is `base + a * 0x800` (`clca7d.h:832`)
pub const HEAD_STRIDE: u32 = 0x800;

/// Where head `head`'s armed copy of core method `method` sits in BAR0: what
/// the head runs now, against what is staged
pub const fn armed_head_reg(head: u32, method: u32) -> u32 {
    CORE_ARM_BASE + method + head * HEAD_STRIDE
}

/// The pixel clock in hertz, from the two method words carrying it. `lo` holds
/// bits 30:0 and the ADJ1000DIV1001 flag in bit 31, `hi` bits 34:31. The flag
/// means the real clock is 1000/1001 of the number, the NTSC-derived rates;
/// NVKMS never sets it, but a firmware-lit head could
pub const fn decode_pixel_clock(lo: u32, hi: u32) -> u64 {
    let hz = (lo & 0x7fff_ffff) as u64 | (((hi & 0xf) as u64) << 31);
    if lo & 0x8000_0000 != 0 {
        hz * 1000 / 1001
    } else {
        hz
    }
}

/// 'SOR_SET_CONTROL_PROTOCOL_DP_A' / '_DP_B', the two DP sublinks
pub const SOR_PROTOCOL_DP_A: u32 = 8;
pub const SOR_PROTOCOL_DP_B: u32 = 9;

/// 'HEAD_SET_CONTROL_OUTPUT_RESOURCE_PIXEL_DEPTH_BPP_24_444'
pub const PIXEL_DEPTH_BPP_24_444: u32 = 4;
/// '..._BPP_30_444', for a 10-bit panel
#[allow(dead_code)] // one depth of the class's PIXEL_DEPTH enum
pub const PIXEL_DEPTH_BPP_30_444: u32 = 5;

/// Bind a SOR to a head and tell it what protocol to speak.
///
/// 'OWNER_MASK' is a mask since one SOR can be driven by two heads in a
/// two-head-one-OR configuration. A single head is one bit
pub fn sor_set_control(
    push: &mut PushBuf,
    or: u32,
    head: u32,
    protocol: u32,
) -> Result<(), PushError> {
    let ctrl = (1u32 << head) | (protocol << 8);
    push.mthd1(M_SOR_SET_CONTROL + or * SOR_STRIDE, ctrl)
}

/// Take every head off an output resource.
///
/// `OWNER_MASK` zero is legal: no head drives this SOR, so it stops sending. A
/// display sleeps this way with nothing torn down: the window keeps scanning,
/// the surface stays, the console keeps painting into it, and none of it leaves
/// the card. Waking is the same method with the mask back
pub fn sor_detach(push: &mut PushBuf, or: u32) -> Result<(), PushError> {
    push.mthd1(M_SOR_SET_CONTROL + or * SOR_STRIDE, 0)
}

/// Program a head to generate 'mode' and feed it to the output resource.
///
/// Nothing here commits: display methods are staged until an UPDATE, which
/// makes a modeset atomic. 'core_update' does that
pub fn head_set_mode(
    push: &mut PushBuf,
    head: u32,
    mode: &Mode,
    display_id: u32,
    olut: Option<u64>,
    // What this head should do to colour
    colour: &super::HeadColour,
) -> Result<(), PushError> {
    let h = head * HEAD_STRIDE;
    let r = mode.raster();

    // RASTER_SIZE, SYNC_END, BLANK_END and BLANK_START are four consecutive
    // methods, so one auto-incrementing packet covers them
    push.method(
        M_HEAD_SET_RASTER_SIZE + h,
        &[
            r.h_active | (r.v_active << 16),
            r.h_synce | (r.v_synce << 16),
            r.h_blanke | (r.v_blanke << 16),
            r.h_blanks | (r.v_blanks << 16),
        ],
    )?;

    // STRUCTURE = PROGRESSIVE, everything else default. Interlaced modes need
    // the second-field blanking methods this does not write
    push.mthd1(M_HEAD_SET_CONTROL + h, 0)?;

    // Both clock methods take hertz in bits 30:0. MAX is what the hardware
    // budgets bandwidth against; with no dynamic clock changes it equals the
    // clock
    let hz = mode.clock_khz.saturating_mul(1000) & 0x7fff_ffff;
    push.mthd1(M_HEAD_SET_PIXEL_CLOCK_FREQUENCY + h, hz)?;
    push.mthd1(M_HEAD_SET_PIXEL_CLOCK_FREQUENCY_MAX + h, hz)?;

    // No scaling: what a window produces is what the raster carries. POINT_IN
    // is where in the composited surface the viewport starts; an inherited
    // non-zero origin shifts the picture off the panel.
    //
    // A viewport smaller than the active raster makes the head fill the
    // remainder with its overscan colour, generated by the head with no memory.
    // A green border says the head, the SOR, the link and the panel work end to
    // end. Set to false for the full viewport
    let viewport = viewport_width(mode) | (viewport_height(mode) << 16);
    push.mthd1(M_HEAD_SET_VIEWPORT_POINT_IN + h, 0)?;
    push.mthd1(M_HEAD_SET_VIEWPORT_SIZE_IN + h, viewport)?;
    push.mthd1(M_HEAD_SET_VIEWPORT_SIZE_OUT + h, viewport)?;
    // Both scaler axes at TAPS_2 and a 1.0 output scale factor, in 1.10 fixed
    // point. A lit desktop carries 0x11 and 0x04000400
    push.mthd1(M_HEAD_SET_CONTROL_OUTPUT_SCALER + h, OUTPUT_SCALER_TAPS_2)?;
    push.mthd1(M_HEAD_SET_MAX_OUTPUT_SCALE_FACTOR + h, SCALE_FACTOR_1X_HV)?;
    // RED_CR 9:0, GREEN_Y 19:10, BLUE_CB 29:20, ten bits each. Full green,
    // which no other stage here produces: the framebuffer pattern, the desktop
    // colour and this are three colours on three paths
    push.mthd1(
        M_HEAD_SET_OVERSCAN_COLOR + h,
        if DIAG_SMALL_VIEWPORT { 0x3ff << 10 } else { 0 },
    )?;
    push.mthd1(M_HEAD_SET_PRESENT_CONTROL_CURSOR + h, PRESENT_CURSOR_MONO)?;

    // What the head composites onto, where no window covers it.
    //
    // Black, as NVKMS's 'InitDesktopColorC9' writes it: alpha 255 and the
    // colour at zero. `DESKTOP_DIAG_*` makes it visible: the colour then means
    // the head and the output are fine and only the window is not compositing
    push.mthd1(
        M_HEAD_SET_DESKTOP_COLOR_ALPHA_RED + h,
        255 | (DESKTOP_DIAG_RED << 16),
    )?;
    push.mthd1(
        M_HEAD_SET_DESKTOP_COLOR_GREEN_BLUE + h,
        DESKTOP_DIAG_GREEN | (DESKTOP_DIAG_BLUE << 16),
    )?;

    push.mthd1(
        M_HEAD_SET_CONTROL_OUTPUT_RESOURCE + h,
        output_resource_value(mode, colour),
    )?;

    // What the transmitter tells the monitor to expect, and what the head does
    // with more bits than the panel has. Both come from the head's colour state
    // (`dev/colour-pipeline.md`); the arithmetic is in `modeset/colour` and the
    // methods appear only here.
    //
    // `HeadColour::DEFAULT` encodes to zeroes. The LUT's address goes in below,
    // inside the matrix's own off/on window
    head_colour_methods(push, h, colour, None)?;

    // Which connector this head's pixels are for. RM associates the head with
    // the DP stream it configured
    push.mthd1(M_HEAD_SET_DISPLAY_ID + h, display_id)?;

    // The output LUT, not optional on this class.
    //
    // 'headca7d' and 'headc57d' are the two head classes nouveau marks
    // 'olut_identity = true', which decides what happens with no gamma ramp:
    // older classes take 'asyh->olut.handle = 0' and disable the LUT, these two
    // go to 'head->func->olut(head, asyh, 0)' and load an identity table.
    // Disabling gives a lit, synced, black panel.
    //
    // The output colour matrix goes around the OLUT, in three steps, NVKMS's
    // order: 'EvoSetLUTContextDmaC9' switches OCSC0 off, programs the LUT, and
    // switches it back on, "only enable OCSC0 after enabling the OLUT". Both
    // stages sit in the same post-composition block, and the hardware dislikes
    // the matrix live while the table underneath changes
    push.mthd1(M_HEAD_SET_OCSC0CONTROL + h, OCSC0_DISABLE)?;

    // Identity. Coefficients are 21-bit, two low bits unused, scaled by 65536:
    // 'nvCscCoefConvertS514' is 'floor(x * 65536 + 2) & 0x001ffffc', so 1.0 is
    // 0x00010000. An idle head on this card carries 0x366c / 0xb718 / 0x127c,
    // the BT.709 luma coefficients, which sum to 65536 and turn colour into
    // greyscale; zeroes turn it into black.
    //
    // The constant column is 0x20: 'floor(x * 65536 + 2)', and the driver
    // carries that bias on the lit head
    push.method(
        M_HEAD_SET_OCSC0COEFFICIENT_C00 + h,
        &ocsc0_coefficients(colour),
    )?;

    // The output clamp: what range of values may leave the head. A head nobody
    // wrote carries LOW 0 / HIGH 0, every component clamped to zero, a black
    // picture. Both heads on a lit desktop hold 0x0fff0000, and
    // 'nvEvoGetOCsc1ClampRange' derives it: full-range colour is LOW 0x0, HIGH
    // 0xFFF. Limited range is 0x100..0xEB0.
    //
    // The clamp goes with the range, written by `head_colour_methods` above

    // Nothing waits on a raster lock. ENABLE 0:0 off, UNSTALL_MODE 12:12 as the
    // driver leaves it; a head inheriting an enabled stall lock waits for a pin
    // that never fires
    push.mthd1(M_HEAD_SET_STALL_LOCK + h, STALL_LOCK_OFF)?;
    push.mthd1(M_HEAD_SET_LOCK_CHAIN + h, 0)?;
    push.mthd1(M_HEAD_SET_CURSOR_COLOR_NORM_SCALE + h, CURSOR_NORM_SCALE_1X)?;

    match olut {
        Some(phys) => head_olut(push, h, phys)?,
        None => {
            push.mthd1(M_HEAD_SET_SURFACE_ADDRESS_LO_OLUT + h, 0)?;
        }
    }

    // Now the matrix may go live
    push.mthd1(M_HEAD_SET_OCSC0CONTROL + h, OCSC0_ENABLE)?;

    // Stream compression, off. 'DSC_CONTROL' and 'DSC_PPS_CONTROL' are
    // consecutive; the lit head holds zero in both while idle heads carry
    // leftovers, and an inherited enabled compressor sends a bitstream no
    // ordinary sink decodes
    push.method(M_HEAD_SET_DSC_CONTROL + h, &[0, 0])?;

    // The DisplayPort secondary-data packet carrying colorimetry. The shipping
    // driver enables it for 30 bits per pixel; plain 24-bit RGB needs none, so
    // it is switched off
    push.mthd1(M_HEAD_SET_VSC_SDP_CTRL + h, 0)?;

    // Leading and trailing raster lines, as the lit head has them:
    // 'v_blanke + 1' and 'v_total - v_blanks - 1'
    let leading = r.v_blanke + 1;
    let trailing = r.v_active.saturating_sub(r.v_blanks).saturating_sub(1);
    push.mthd1(
        M_HEAD_SET_MIN_FRAME_IDLE + h,
        (leading & 0x7fff) | ((trailing & 0x7fff) << 16),
    )?;

    // The tile this head composites into is claimed by
    // 'disp::core_claim_tiles',
    // which also takes the inherited tiles away from every other head

    // The cursor is its own plane on the head, decided later than the modeset:
    // its image does not exist until video memory is carved for it.
    // `head_set_cursor` says no here
    head_set_cursor(push, head, None)
}

/// What the head needs to scan a cursor out of video memory.
///
/// Position is absent: it lives on the cursor-immediate PIO channel
/// (`modeset::cursor`), so a hardware cursor costs no frame. This is
/// core-channel state, changing only with the image: a size, or a hot spot
/// inside it
#[derive(Debug, Clone, Copy)]
pub struct CursorArm {
    /// Physical address of the image. 256-byte aligned, pitch layout, A8R8G8B8,
    /// the only format the plane accepts
    /// (`nvEvoGetHeadSetControlCursorValueC3`, nvkms-evo3.c)
    pub surface: u64,
    /// One of `window::SURFACE_TARGET_*`
    pub target: u32,
    /// 32, 64, 128 or 256. The plane takes no other side
    pub size: u32,
    /// Where inside the image the pointer points. Eight bits each, so a hot
    /// spot cannot sit outside a 256-pixel cursor
    pub hot_x: u32,
    pub hot_y: u32,
}

/// The head's cursor plane: on, pointing at `arm`, or off.
///
/// Staged: takes effect on the next UPDATE.
///
/// Off is more than clearing the surface address: the cursor composition stage
/// is a separate register. `VIEWPORT_COLOR_FACTOR_SELECT` at 15:12 scales the
/// viewport, everything the windows composited, and its zero value is named
/// ZERO. Starting from what the firmware left, both branches write it. The
/// disabled one writes 0x72ff, the shipping driver's armed state on a lit
/// desktop: K1 at full scale, cursor factor K1, viewport factor
/// NEG_K1_TIMES_SRC. With no cursor the source alpha is zero, so that factor is
/// one and the viewport arrives unmodified
pub fn head_set_cursor(
    push: &mut PushBuf,
    head: u32,
    arm: Option<CursorArm>,
) -> Result<(), PushError> {
    let h = head * HEAD_STRIDE;
    let Some(a) = arm else {
        push.mthd1(M_HEAD_SET_SURFACE_ADDRESS_LO_CURSOR + h, 0)?;
        push.mthd1(
            M_HEAD_SET_CONTROL_CURSOR_COMPOSITION + h,
            CURSOR_COMPOSITION_OFF,
        )?;
        // ENABLE is bit 31, so a bare format leaves the cursor disabled
        return push.mthd1(M_HEAD_SET_CONTROL_CURSOR + h, CURSOR_FORMAT_A8R8G8B8);
    };

    // Two separate methods, not one auto-incrementing packet: these are eight
    // apart, indexed by stereo eye with eye 1 between them. A packet would put
    // the low word into `HI_CURSOR(head, 1)`
    push.mthd1(
        M_HEAD_SET_SURFACE_ADDRESS_HI_CURSOR + h,
        (a.surface >> 32) as u32,
    )?;
    // The low word carries the address in 31:4, the aperture in 3:2 and the
    // enable bit at 0
    push.mthd1(
        M_HEAD_SET_SURFACE_ADDRESS_LO_CURSOR + h,
        (a.surface as u32 & !0xf) | ((a.target & 3) << 2) | 1,
    )?;

    // Blending: the pointer has an alpha channel and what is under it survives.
    // NVKMS's NON_PREMULT_ALPHA branch (`EvoSetCursorImageC9`, nvkms-evo4.c):
    // cursor factor K1_TIMES_SRC, viewport factor NEG_K1_TIMES_SRC, K1 at full
    // scale, MODE_BLEND
    push.mthd1(
        M_HEAD_SET_CONTROL_CURSOR_COMPOSITION + h,
        CURSOR_COMPOSITION_BLEND_NON_PREMULT,
    )?;

    push.mthd1(
        M_HEAD_SET_CONTROL_CURSOR + h,
        CURSOR_ENABLE
            | CURSOR_FORMAT_A8R8G8B8
            | (cursor_size_field(a.size) << 8)
            | ((a.hot_x & 0xff) << 12)
            | ((a.hot_y & 0xff) << 20),
    )
}

/// `HEAD_SET_CONTROL_CURSOR_SIZE` at 9:8. Anything unrecognised reads as 64x64,
/// what everything in this tree allocates
fn cursor_size_field(px: u32) -> u32 {
    match px {
        32 => 0,
        128 => 2,
        256 => 3,
        _ => 1,
    }
}

/// Is the head's raster generator producing frames?
///
/// 'NV_PDISP_RG_IN_LOADV_COUNTER(head)' counts once per scanned-out frame;
/// 'kheadGetLoadVCounter_v05_01' in RM reads this.
///
/// 'NV_PDISP_POSTCOMP_LOADV_COUNTER(head)' is the same count after composition.
/// Both advancing means pixels reach the output and the fault is in the SOR or
/// the link. RG advancing without postcomp means the window never made it into
/// the frame
pub fn head_frame_counters(bar0: &MmioRegion, head: u32) -> (u32, u32) {
    (
        bar0.read32(REG_RG_IN_LOADV_COUNTER + head * 0x800),
        bar0.read32(REG_POSTCOMP_LOADV_COUNTER + head * 0x400),
    )
}

pub const REG_RG_IN_LOADV_COUNTER: u32 = 0x0061_6320;
pub const REG_POSTCOMP_LOADV_COUNTER: u32 = 0x0061_A11C;

// The head's own interrupt: vblank
//
// The front end raises one interrupt per head per frame, the event
// `head_frame_counters` counts.
//
// nouveau under GSP-RM handles these on the CPU (`r535_head_vblank_get` and
// `r535_disp_intr` in `rm/r535/disp.c`): `bRouteDispIntrsToCPU` in
// `GspSystemInfo` is set only on Windows, and the Linux path leaves it zero and
// still gets the interrupt.
//
// Three registers, all indexed by head:
//   EVT_STAT   write the bit to clear it, read it to see what happened
//   EVT_EN_SET which events this head may raise
//   PENDING    one bit per head, where the handler starts

/// NV_PDISP_FE_EVT_STAT_HEAD_TIMING(head): written to acknowledge
const REG_FE_EVT_STAT_HEAD_TIMING: u32 = 0x0061_1800;
/// The same event, read: `r535_disp_intr_head_timing` reads 0x611c00 and
/// acknowledges at 0x611800
const REG_FE_EVT_HEAD_TIMING: u32 = 0x0061_1C00;
/// NV_PDISP_FE_EVT_EN_SET_HEAD_TIMING(head)
const REG_FE_EVT_EN_SET_HEAD_TIMING: u32 = 0x0061_1D80;
/// One bit per head, set while that head has a timing event outstanding
const REG_FE_EVT_PENDING_HEAD_TIMING: u32 = 0x0061_1EC0;
/// LAST_DATA: the end of the visible region, vblank
pub const FE_EVT_HEAD_TIMING_LAST_DATA: u32 = 0x0000_0002;

/// Start raising vblank on this head.
///
/// Any stale event is cleared first: enabling on a pending one gives an
/// interrupt for a frame that ended before anybody asked
pub fn vblank_enable(bar0: &MmioRegion, head: u32) {
    bar0.write32(
        REG_FE_EVT_STAT_HEAD_TIMING + head * 4,
        FE_EVT_HEAD_TIMING_LAST_DATA,
    );
    bar0.modify32(
        REG_FE_EVT_EN_SET_HEAD_TIMING + head * 4,
        FE_EVT_HEAD_TIMING_LAST_DATA,
        FE_EVT_HEAD_TIMING_LAST_DATA,
    );
}

/// Stop raising it, for a head being torn down
pub fn vblank_disable(bar0: &MmioRegion, head: u32) {
    bar0.modify32(
        REG_FE_EVT_EN_SET_HEAD_TIMING + head * 4,
        FE_EVT_HEAD_TIMING_LAST_DATA,
        0,
    );
    bar0.write32(
        REG_FE_EVT_STAT_HEAD_TIMING + head * 4,
        FE_EVT_HEAD_TIMING_LAST_DATA,
    );
}

/// Which heads have a timing event outstanding. Eight bits, one per head
pub fn vblank_pending(bar0: &MmioRegion) -> u32 {
    bar0.read32(REG_FE_EVT_PENDING_HEAD_TIMING) & 0xff
}

/// Acknowledge whatever timing event this head holds, and say what it was.
/// `FE_EVT_HEAD_TIMING_LAST_DATA` in the result is a vblank.
///
/// Everything set is acknowledged: a bit stays asserted until written back and
/// the tree re-raises meanwhile, so an event this driver ignores would storm.
/// nouveau (`r535_disp_intr_head_timing`) acknowledges only LAST_DATA because
/// it enabled only that; GSP-RM has been running here and may have left others
/// enabled
pub fn head_timing_take(bar0: &MmioRegion, head: u32) -> u32 {
    let stat = bar0.read32(REG_FE_EVT_HEAD_TIMING + head * 4);
    if stat != 0 {
        bar0.write32(REG_FE_EVT_STAT_HEAD_TIMING + head * 4, stat);
    }
    stat
}

/// The raster the head scans, in lines.
///
/// `vtotal` is the whole raster, `vblanke` the last blanked line and `vblanks`
/// the first, so the visible rows are `vblanke + 1 ..= vblanks - 1`, and the
/// line counter in `RG_DPCA` runs 0..vtotal over all. The counter starts inside
/// the blanking interval, so a frame beginning at line zero has `vblanke` lines
/// of head start.
///
/// The armed core methods, read back: RASTER_SIZE, BLANK_END, BLANK_START and
/// the two pixel-clock words. Heads are `HEAD_STRIDE` apart, not 0x400, the
/// c37d stride of nouveau's `gv100_head_state`
pub fn head_raster(bar0: &MmioRegion, head: u32) -> RasterTiming {
    let armed = |method: u32| bar0.read32(armed_head_reg(head, method));
    let (vt, ht) = split(armed(M_HEAD_SET_RASTER_SIZE));
    let (vbe, _) = split(armed(M_HEAD_SET_RASTER_BLANK_END));
    let (vbs, _) = split(armed(M_HEAD_SET_RASTER_BLANK_START));
    RasterTiming {
        vtotal: vt,
        htotal: ht,
        vblanke: vbe,
        vblanks: vbs,
        pixel_clock_hz: decode_pixel_clock(
            armed(M_HEAD_SET_PIXEL_CLOCK_FREQUENCY),
            armed(M_HEAD_SET_PIXEL_CLOCK_FREQUENCY_HI),
        ),
    }
}

/// Vertical in the top half, horizontal in the bottom
pub fn split(data: u32) -> (u16, u16) {
    ((data >> 16) as u16, (data & 0xffff) as u16)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RasterTiming {
    pub vtotal: u16,
    pub htotal: u16,
    /// Last blanked line; the first visible row is the one after it
    pub vblanke: u16,
    /// First blanked line at the bottom
    pub vblanks: u16,
    /// The pixel clock, in hertz: not a refresh rate, which is `refresh_mhz`
    pub pixel_clock_hz: u64,
}

impl RasterTiming {
    /// Is this a raster at all, or a head not scanning?
    pub fn valid(&self) -> bool {
        self.vtotal > 0 && self.vblanks > self.vblanke
    }
    /// Visible rows
    pub fn active(&self) -> u16 {
        self.vblanks.saturating_sub(self.vblanke + 1)
    }
    /// Frames per second, in thousandths: one frame is `htotal * vtotal` pixel
    /// clocks. Zero for a head not scanning
    pub fn refresh_mhz(&self) -> u32 {
        let pixels = self.htotal as u64 * self.vtotal as u64;
        if pixels == 0 {
            return 0;
        }
        ((self.pixel_clock_hz * 1000 + pixels / 2) / pixels) as u32
    }
    /// Frames per second, rounded to the nearest whole one
    pub fn refresh_hz(&self) -> u32 {
        (self.refresh_mhz() + 500) / 1000
    }
}

// DP stream framer, per head: these turn a scanned-out raster into a
// DisplayPort video stream: how many symbols each blanking period carries, and
// the watermark pacing the transfer unit. 'nv50_disp_super_2_2_dp' writes them
// from the supervisor handler on the bare-metal path; on a GSP part the
// firmware owns them and 'NV0073_CTRL_CMD_DP_CONFIG_STREAM' is the only
// client-facing way in
pub const REG_SF_WATERMARK: u32 = 0x0061_6550;
pub const REG_SF_HBLANK_SYM: u32 = 0x0061_6568;
pub const REG_SF_VBLANK_SYM: u32 = 0x0061_656C;
pub const SF_HEAD_STRIDE: u32 = 0x800;

/// '(watermark, hblank_sym, vblank_sym)' raw register values for 'head'.
///
/// Read before anything is written: all-zero means the framer was never
/// configured, a monitor syncing to the link with no picture
pub fn sf_stream_state(bar0: &MmioRegion, head: u32) -> (u32, u32, u32) {
    let hoff = head * SF_HEAD_STRIDE;
    (
        bar0.read32(REG_SF_WATERMARK + hoff),
        bar0.read32(REG_SF_HBLANK_SYM + hoff),
        bar0.read32(REG_SF_VBLANK_SYM + hoff),
    )
}

/// Program the framer directly, field for field as 'gv100_sor_dp_watermark' and
/// 'gv100_sor_dp_audio_sym' do. Bit 27 of the watermark register is the enable
/// the reference driver sets with the value
pub fn sf_program_stream(
    bar0: &MmioRegion,
    head: u32,
    watermark: u32,
    hblank_sym: u32,
    vblank_sym: u32,
) {
    let hoff = head * SF_HEAD_STRIDE;
    bar0.modify32(REG_SF_HBLANK_SYM + hoff, 0x0000_ffff, hblank_sym & 0xffff);
    bar0.modify32(
        REG_SF_VBLANK_SYM + hoff,
        0x00ff_ffff,
        vblank_sym & 0x00ff_ffff,
    );
    bar0.modify32(
        REG_SF_WATERMARK + hoff,
        0x0c00_003f,
        0x0800_0000 | (watermark & 0x3f),
    );
}

// SOR DisplayPort transmitter state, indexed by 'sor * 0x800' ('nv50_ior_base')
pub const REG_SOR_DP_LINKCTL: u32 = 0x0061_C10C;
pub const REG_SOR_DP_PATTERN_A: u32 = 0x0061_C110;
pub const REG_SOR_DP_PATTERN_B: u32 = 0x0061_C12C;
pub const REG_SOR_DP_LANES: u32 = 0x0061_C130;
pub const REG_SOR_DP_TRIGGER: u32 = 0x0061_C034;
pub const SOR_BASE_STRIDE: u32 = 0x800;

/// What the DP transmitter is sending.
///
/// A link can be trained and a stream framer configured while the transmitter
/// emits a training pattern, a monitor that wakes up, holds sync and shows
/// black.
///
/// 'gm107_sor_dp_pattern' encodes the pattern one nibble-pair per lane: 0x10 is
/// normal video, 0x01/0x02/0x03 are training patterns 1-3, 0x1b is the
/// compliance pattern
pub fn sor_dp_state(bar0: &MmioRegion, sor: u32) -> [(&'static str, u32, u32); 5] {
    let soff = sor * SOR_BASE_STRIDE;
    let rd = |r: u32| (r + soff, bar0.read32(r + soff));
    let (a1, v1) = rd(REG_SOR_DP_LINKCTL);
    let (a2, v2) = rd(REG_SOR_DP_PATTERN_A);
    let (a3, v3) = rd(REG_SOR_DP_PATTERN_B);
    let (a4, v4) = rd(REG_SOR_DP_LANES);
    let (a5, v5) = rd(REG_SOR_DP_TRIGGER);
    [
        ("LINKCTL", a1, v1),
        ("PATTERN_A", a2, v2),
        ("PATTERN_B", a3, v3),
        ("LANES", a4, v4),
        ("TRIGGER", a5, v5),
    ]
}

/// Names for the training patterns, so a log line says TPS2 and not 2. Training
/// reports its outcome, not each step
#[allow(dead_code)]
pub fn dp_pattern_name(reg: u32) -> &'static str {
    match reg & 0x1f {
        0x10 => "normal video",
        0x01 => "training pattern 1",
        0x02 => "training pattern 2",
        0x03 => "training pattern 3",
        0x1b => "compliance pattern",
        0x00 => "idle/none",
        _ => "?",
    }
}

/// Read-modify-write of the bits in 'mask', the equivalent of nvkm's
/// 'nvkm_mask': these registers carry other state
// Tile state is written once by 'core_channel_init'; these let the armed-state
// dump read it back
pub const M_HEAD_SET_TILE_MASK: u32 = 0x2060;
pub const M_TILE_SET_TILE_SIZE: u32 = 0x6000;
pub const TILE_STRIDE: u32 = 0x200;

pub const M_HEAD_SET_CONTROL_CURSOR: u32 = 0x209c;
pub const M_HEAD_SET_MIN_FRAME_IDLE: u32 = 0x2218;
pub const M_HEAD_SET_STALL_LOCK: u32 = 0x2034;
pub const M_HEAD_SET_LOCK_CHAIN: u32 = 0x2044;
pub const M_HEAD_SET_CURSOR_COLOR_NORM_SCALE: u32 = 0x2228;
pub const M_HEAD_SET_CLAMP_RANGE_GREEN: u32 = 0x2238;
pub const M_HEAD_SET_CLAMP_RANGE_RED_BLUE: u32 = 0x223c;

/// LOW 11:0 = 0x000, HIGH 27:16 = 0xfff: the full-range clamp
pub const CLAMP_RANGE_FULL: u32 = 0x0fff_0000;
/// The narrower window a television expects, from `nvEvoGetOCsc1ClampRange`:
/// LOW 0x100, HIGH 0xEB0 against full range's 0x0 and 0xFFF
pub const CLAMP_RANGE_LIMITED: u32 = 0x0eb0_0100;

// The three head methods whose value is a colour decision, not a mode decision.
// Each is one function, the encoding beside the method it feeds; the arithmetic
// producing the decision is in `modeset/colour`.

/// The twelve coefficients of the head's output matrix.
///
/// Identity plus the bias, unless the state asks otherwise. `OCSC_ONE` and
/// `OCSC_BIAS` are the values a lit head carries under the shipping driver.
///
/// A head never written carries zeroes here, turning the picture black, and an
/// idle one the BT.709 luma coefficients, turning it greyscale
pub fn ocsc0_coefficients(colour: &super::HeadColour) -> [u32; 12] {
    /// 1.0 in the coefficient format: `floor(1.0 * 65536 + 2) & 0x001ffffc`
    const OCSC_ONE: u32 = 0x0001_0000;
    /// What the shipping driver carries in the constant column of a lit head
    const OCSC_BIAS: u32 = 0x20;

    match colour.output_matrix() {
        None => [
            OCSC_ONE, 0, 0, OCSC_BIAS, 0, OCSC_ONE, 0, OCSC_BIAS, 0, 0, OCSC_ONE, OCSC_BIAS,
        ],
        Some(m) => {
            let k = super::Matrix::coefficient;
            [
                k(m.c[0][0]),
                k(m.c[0][1]),
                k(m.c[0][2]),
                k(m.off[0]) | OCSC_BIAS,
                k(m.c[1][0]),
                k(m.c[1][1]),
                k(m.c[1][2]),
                k(m.off[1]) | OCSC_BIAS,
                k(m.c[2][0]),
                k(m.c[2][1]),
                k(m.c[2][2]),
                k(m.off[2]) | OCSC_BIAS,
            ]
        }
    }
}

/// `HEAD_SET_CONTROL_OUTPUT_RESOURCE`: sync polarity from the mode, pixel depth
/// from the colour state.
///
/// One function: the depth can change without a modeset, and the polarity is
/// computed once
pub fn output_resource_value(mode: &Mode, colour: &super::HeadColour) -> u32 {
    // Sync polarity belongs to the head; the OR serialises what the head hands
    // it
    let pixel_depth = match colour.depth {
        super::Depth::Bpc8 => PIXEL_DEPTH_BPP_24_444,
        super::Depth::Bpc10 => PIXEL_DEPTH_BPP_30_444,
    };
    let mut or_res = (pixel_depth & 0xf) << 4;
    if mode.nhsync {
        or_res |= 1 << 2;
    }
    if mode.nvsync {
        or_res |= 1 << 3;
    }
    // EXT_PACKET_WIN = NONE (0x3f): the head expects no window to supply
    // InfoFrames
    or_res |= 0x3f << 26;
    or_res
}

/// The two encoded values a colour state produces, for a test: a wrong bit
/// position in one is a green screen. `nvidia selftest` decides them in QEMU
pub fn colour_encodings(colour: &super::HeadColour) -> (u32, u32) {
    (procamp_value(colour), dither_value(colour))
}

/// The three head methods a colour state decides, in one place.
///
/// Called from `head_set_mode` during a modeset and from the display layer's
/// update afterwards (`blackwell/apply.rs`). `h` is the head's method stride,
/// already multiplied.
///
/// `olut` is a table to switch the head to, the next of its three (`OlutRing`),
/// holding the new curve, going in while the matrix is off. `None` leaves the
/// LUT
pub fn head_colour_methods(
    push: &mut PushBuf,
    h: u32,
    colour: &super::HeadColour,
    olut: Option<u64>,
) -> Result<(), PushError> {
    // The matrix goes around the LUT as in `EvoSetLUTContextDmaC9`: off,
    // program, on. The hardware dislikes the matrix live while the table
    // changes
    push.mthd1(M_HEAD_SET_OCSC0CONTROL + h, OCSC0_DISABLE)?;
    if let Some(phys) = olut {
        head_olut(push, h, phys)?;
    }
    push.method(
        M_HEAD_SET_OCSC0COEFFICIENT_C00 + h,
        &ocsc0_coefficients(colour),
    )?;
    push.mthd1(M_HEAD_SET_OCSC0CONTROL + h, OCSC0_ENABLE)?;

    push.mthd1(M_HEAD_SET_PROCAMP + h, procamp_value(colour))?;
    push.mthd1(M_HEAD_SET_DITHER_CONTROL + h, dither_value(colour))?;
    let clamp = clamp_value(colour);
    push.mthd1(M_HEAD_SET_CLAMP_RANGE_GREEN + h, clamp)?;
    push.mthd1(M_HEAD_SET_CLAMP_RANGE_RED_BLUE + h, clamp)
}

/// `HEAD_SET_PROCAMP`: what the monitor is told to expect.
///
/// COLOR_SPACE RGB, CHROMA_LPF disabled, DYNAMIC_RANGE at 1:1: VESA is the
/// full-range claim and zero, CEA the limited-range one. The claim and the
/// pixels agree through `HeadColour::matching`
fn procamp_value(c: &super::HeadColour) -> u32 {
    // `clca7d.h:631-644`: COLOR_SPACE is 1:0 and DYNAMIC_RANGE is 28:28. The
    // range bit at 1 sets COLOR_SPACE to 2, YUV_709, and RGB pixels announced
    // as YUV are a green screen
    const DYNAMIC_RANGE_CEA: u32 = 1 << 28;
    match c.range {
        super::Range::Full => 0,
        super::Range::Limited => DYNAMIC_RANGE_CEA,
    }
}

/// `HEAD_SET_DITHER_CONTROL`: ENABLE 0:0, BITS 5:4, MODE 10:8. Off is zero.
/// Spatial and temporal both dither to the depth the head drives
fn dither_value(c: &super::HeadColour) -> u32 {
    // `clca7d.h:844-862`. BITS is 5:4 and counts up from six, so 10 bits is 2;
    // 3 is TO_12_BITS. MODE is 10:8: 0 is DYNAMIC_ERR_ACC, 1 is STATIC_ERR_ACC,
    // and the temporal mode is a value of its own at 4
    const ENABLE: u32 = 1;
    const MODE_STATIC_ERR_ACC: u32 = 1 << 8;
    const MODE_TEMPORAL: u32 = 4 << 8;
    let bits = match c.depth {
        super::Depth::Bpc8 => 1 << 4,  // BITS_TO_8_BITS
        super::Depth::Bpc10 => 2 << 4, // BITS_TO_10_BITS
    };
    match c.dither {
        super::Dither::Off => 0,
        super::Dither::Spatial => ENABLE | bits | MODE_STATIC_ERR_ACC,
        super::Dither::Temporal => ENABLE | bits | MODE_TEMPORAL,
    }
}

/// `HEAD_SET_CLAMP_RANGE_*`: what the head does with values outside the range
fn clamp_value(c: &super::HeadColour) -> u32 {
    match c.range {
        super::Range::Full => CLAMP_RANGE_FULL,
        super::Range::Limited => CLAMP_RANGE_LIMITED,
    }
}
/// ENABLE 0:0 clear, UNSTALL_MODE 12:12 set, as the shipping driver leaves it
pub const STALL_LOCK_OFF: u32 = 0x1000;
/// What both heads of a lit desktop carry
pub const CURSOR_NORM_SCALE_1X: u32 = 0x1000;

pub const M_HEAD_SET_OCSC0CONTROL: u32 = 0x2240;
/// `OCSC0CONTROL_ENABLE` is a single bit at 0:0
pub const OCSC0_DISABLE: u32 = 0;
pub const OCSC0_ENABLE: u32 = 1;
pub const M_HEAD_SET_OCSC0COEFFICIENT_C00: u32 = 0x2244;
pub const M_HEAD_SET_DSC_CONTROL: u32 = 0x22d4;
pub const M_HEAD_SET_VSC_SDP_CTRL: u32 = 0x2448;

/// The head's inherited output colour matrix diagonal, before it is written.
/// Zeroes mean every colour is multiplied by nothing
pub fn armed_ocsc0_diagonal(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    head: u32,
) -> (u32, u32, u32) {
    let b = CORE_ARM_BASE + M_HEAD_SET_OCSC0COEFFICIENT_C00 + head * HEAD_STRIDE;
    (bar0.read32(b), bar0.read32(b + 0x14), bar0.read32(b + 0x28))
}

/// The clamp range, the OCSC0 enable bit and the stall lock as inherited,
/// sampled before the modeset overwrites them. A clamp of zero discards every
/// pixel
pub fn armed_output_gates(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    head: u32,
) -> (u32, u32, u32, u32) {
    let h = head * HEAD_STRIDE;
    (
        bar0.read32(CORE_ARM_BASE + M_HEAD_SET_CLAMP_RANGE_GREEN + h),
        bar0.read32(CORE_ARM_BASE + M_HEAD_SET_CLAMP_RANGE_RED_BLUE + h),
        bar0.read32(CORE_ARM_BASE + M_HEAD_SET_OCSC0CONTROL + h),
        bar0.read32(CORE_ARM_BASE + M_HEAD_SET_STALL_LOCK + h),
    )
}

pub const M_HEAD_SET_CONTROL_CURSOR_COMPOSITION: u32 = 0x20a0;
pub const COMPOSITION_FACTOR_K1: u32 = 2;
pub const COMPOSITION_FACTOR_K1_TIMES_SRC: u32 = 5;
pub const COMPOSITION_FACTOR_NEG_K1_TIMES_SRC: u32 = 7;

/// K1 7:0 at full scale, cursor factor 11:8, viewport factor 15:12, MODE 16:16
/// left at BLEND. The two values a head gets: 0x72ff with no cursor plane,
/// 0x75ff with one blending over the viewport
pub const CURSOR_COMPOSITION_OFF: u32 =
    0xff | (COMPOSITION_FACTOR_K1 << 8) | (COMPOSITION_FACTOR_NEG_K1_TIMES_SRC << 12);
pub const CURSOR_COMPOSITION_BLEND_NON_PREMULT: u32 =
    0xff | (COMPOSITION_FACTOR_K1_TIMES_SRC << 8) | (COMPOSITION_FACTOR_NEG_K1_TIMES_SRC << 12);

/// What the head's cursor-composition register held before we wrote it. Read
/// from the armed region: whether the viewport was being scaled to nothing
pub fn armed_cursor_composition(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    head: u32,
) -> u32 {
    bar0.read32(CORE_ARM_BASE + M_HEAD_SET_CONTROL_CURSOR_COMPOSITION + head * HEAD_STRIDE)
}

pub const M_HEAD_SET_SURFACE_ADDRESS_HI_OLUT: u32 = 0x2158;
pub const M_HEAD_SET_SURFACE_ADDRESS_LO_OLUT: u32 = 0x215c;
pub const M_HEAD_SET_OLUT_CONTROL: u32 = 0x2280;
pub const OLUT_MODE_DIRECT10: u32 = 2;

/// The output LUT buffer, laid out as 'headc57d_olut_load' writes it: a 32-byte
/// VSS header of zeros, 1024 entries of four 16-bit components, then one more
/// entry replicating the last.
///
/// INTERPOLATE mode reads entry `n + 1` to interpolate towards, so the final
/// real entry needs a successor, duplicated as in nouveau
pub const OLUT_ENTRIES: usize = 4 + 1024 + 1;
pub const OLUT_BYTES: usize = 0x20 + 1025 * 8;

/// Point the head at an output LUT: the address, then MODE DIRECT10, SIZE in
/// entries and FP_NORM_SCALE at full scale, the two control words consecutive.
/// INTERPOLATE stays off: nouveau asks for it, the shipping driver on a lit
/// desktop does not, with armed OLUT_CONTROL 0x00040508 against 0x00040509. `h`
/// is the head's method stride, already multiplied
pub fn head_olut(push: &mut PushBuf, h: u32, phys: u64) -> Result<(), PushError> {
    push.method(
        M_HEAD_SET_SURFACE_ADDRESS_HI_OLUT + h,
        &[
            (phys >> 32) as u32,
            (phys as u32 & !0xf) | (SURFACE_TARGET_VRAM << 2) | 1,
        ],
    )?;
    push.method(
        M_HEAD_SET_OLUT_CONTROL + h,
        &[
            (OLUT_MODE_DIRECT10 << 2) | ((OLUT_ENTRIES as u32) << 8),
            0xffff_ffff,
        ],
    )
}

/// How many output LUTs a head keeps, and how far apart: one 4 KiB-aligned slot
/// each, `OLUT_BYTES` rounded up
pub const OLUT_SLOTS: u8 = 3;
pub const OLUT_STRIDE: u64 = (OLUT_BYTES as u64).next_multiple_of(0x1000);
/// One allocation holds all three
pub const OLUT_RING_BYTES: u64 = OLUT_SLOTS as u64 * OLUT_STRIDE;

/// A head's output LUTs, three, as NVKMS keeps them (`nvEvoSetLut`,
/// `nvkms-evo.c:8671`).
///
/// The head re-reads its table live, so a curve written into the table it reads
/// shows mid-frame, before its update, and stays if that update is refused. A
/// new curve goes into the next slot, and a method in the same update switches
/// the head to it. The slot after is neither on screen nor waiting for its
/// vblank, hence three
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OlutRing {
    /// Video-memory address of slot 0
    pub base: u64,
    /// The slot the head was last pointed at
    pub cur: u8,
}

impl OlutRing {
    pub fn new(base: u64) -> Self {
        OlutRing { base, cur: 0 }
    }

    pub fn slot(&self, i: u8) -> u64 {
        self.base + (i % OLUT_SLOTS) as u64 * OLUT_STRIDE
    }

    /// The table the head reads now
    pub fn live(&self) -> u64 {
        self.slot(self.cur)
    }

    /// The ring with the head moved on one slot: what to fill, and to record
    /// once the update pointing at it has landed
    pub fn next(&self) -> OlutRing {
        OlutRing {
            base: self.base,
            cur: (self.cur + 1) % OLUT_SLOTS,
        }
    }
}

/// Write an identity output LUT into 'buf', 'OLUT_BYTES' long.
///
/// 'nv50_lut_load' builds the no-gamma table as '(i << 16) >> 10' per component
/// over 1024 entries, and 'drm_color_lut_extract(v, 16)' is the identity for
/// 16-bit precision, so each component is 'i * 64'.
///
/// Diagnostic: a viewport smaller than the active raster leaves a border the
/// head fills with 'OVERSCAN_COLOR', needing no memory. Set to true to show
/// green overscan, red desktop colour and the test pattern in the window in one
/// frame
pub const DIAG_SMALL_VIEWPORT: bool = false;

/// Diagnostic: bring the display up on the test pattern, not a blank surface.
/// Each way the surface can be wrong looks different on the eight bars, the
/// border and the diagonal, but it is the first thing the monitor shows until
/// the console arrives
pub const DIAG_TEST_PATTERN: bool = false;

pub fn viewport_width(mode: &Mode) -> u32 {
    if DIAG_SMALL_VIEWPORT {
        mode.h_display.min(1280)
    } else {
        mode.h_display
    }
}

pub fn viewport_height(mode: &Mode) -> u32 {
    if DIAG_SMALL_VIEWPORT {
        mode.v_display.min(720)
    } else {
        mode.v_display
    }
}

/// The five-tap scaler coefficients, from `scalerTaps5Coeff` in
/// nvkms-evo3.c:213. Three ratios, sixteen phases, four values.
///
/// The values are signed and the hardware field is ten bits wide, so negatives
/// go in two's complement, as `DRF_NUM` masks a negative `NvU32` to the field
/// width
pub const SCALER_TAPS5_COEFF: [[[i16; 4]; 16]; 3] = [
    // ratio 1
    [
        [0, 0, -16, 144],
        [0, -5, 5, 0],
        [0, -9, 11, 0],
        [-1, -12, 18, -1],
        [-1, -15, 25, -1],
        [-1, -18, 33, -2],
        [-2, -20, 42, -3],
        [-2, -21, 51, -3],
        [-3, -22, 60, -5],
        [-3, -22, 70, -6],
        [-4, -22, 81, -7],
        [-4, -22, 91, -9],
        [-5, -21, 102, -10],
        [-5, -20, 113, -12],
        [-5, -19, 125, -13],
        [-6, -18, 136, -15],
    ],
    // ratio 2
    [
        [3, 60, 20, 108],
        [3, 57, 63, 4],
        [2, 54, 66, 4],
        [2, 51, 69, 5],
        [2, 48, 72, 6],
        [1, 45, 75, 7],
        [1, 43, 78, 7],
        [1, 40, 81, 8],
        [1, 37, 84, 9],
        [0, 35, 88, 10],
        [0, 33, 91, 12],
        [0, 30, 94, 13],
        [0, 28, 97, 14],
        [0, 26, 99, 16],
        [0, 24, 102, 17],
        [0, 22, 105, 19],
    ],
    // ratio 4
    [
        [4, 62, 23, 105],
        [4, 59, 64, 5],
        [3, 56, 67, 6],
        [3, 53, 70, 7],
        [2, 51, 73, 8],
        [2, 48, 76, 8],
        [2, 45, 79, 9],
        [1, 43, 81, 10],
        [1, 40, 84, 12],
        [1, 38, 87, 13],
        [1, 36, 90, 14],
        [0, 34, 92, 15],
        [0, 31, 95, 17],
        [0, 29, 97, 18],
        [0, 27, 100, 20],
        [0, 25, 102, 22],
    ],
];

pub const M_HEAD_SET_OUTPUT_SCALER_COEFF_VALUE: u32 = 0x2214;
pub const M_WNDW_SET_INPUT_SCALER_COEFF_VALUE: u32 = 0x02ac;

/// One coefficient word: DATA is 9:0, INDEX is 19:12, and the index packs the
/// ratio, the phase and the position within the phase
pub fn scaler_coeff_word(ratio: usize, phase: usize, idx: usize) -> u32 {
    let coeff = SCALER_TAPS5_COEFF[ratio][phase][idx] as u32 & 0x3ff;
    let index = ((ratio as u32) << 6) | ((phase as u32) << 2) | idx as u32;
    coeff | (index << 12)
}

/// Load the scaler coefficient RAM, on the core channel for every head.
///
/// `EvoInitChannelC9` does this once, when the core channel is initialised, as
/// does the C5 path; no modeset method mentions it.
///
/// Reading `SET_OUTPUT_SCALER_COEFF_VALUE` back returns 0xbadf5040, so it is
/// absent from the reference tables: with the window's
/// `SET_INPUT_SCALER_COEFF_VALUE`, one of two entries a comparison cannot see.
///
/// The table is the five-tap one, and the head runs
/// `CONTROL_OUTPUT_SCALER = 0x11`, two taps in both directions; the two-tap
/// path may not read this RAM
pub fn init_output_scaler_coeffs_head(push: &mut PushBuf, head: u32) -> Result<(), PushError> {
    init_output_scaler_coeffs(push, head, head + 1)
}

pub fn init_output_scaler_coeffs(
    push: &mut PushBuf,
    first: u32,
    heads: u32,
) -> Result<(), PushError> {
    for h in first..heads {
        for ratio in 0..SCALER_TAPS5_COEFF.len() {
            for phase in 0..SCALER_TAPS5_COEFF[ratio].len() {
                for idx in 0..SCALER_TAPS5_COEFF[ratio][phase].len() {
                    push.mthd1(
                        M_HEAD_SET_OUTPUT_SCALER_COEFF_VALUE + h * HEAD_STRIDE,
                        scaler_coeff_word(ratio, phase, idx),
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// The same table into the window's pre-composition scaler, which
/// `EvoInitChannelC9` loads on every window channel
pub fn init_input_scaler_coeffs(push: &mut PushBuf) -> Result<(), PushError> {
    for ratio in 0..SCALER_TAPS5_COEFF.len() {
        for phase in 0..SCALER_TAPS5_COEFF[ratio].len() {
            for idx in 0..SCALER_TAPS5_COEFF[ratio][phase].len() {
                push.mthd1(
                    M_WNDW_SET_INPUT_SCALER_COEFF_VALUE,
                    scaler_coeff_word(ratio, phase, idx),
                )?;
            }
        }
    }
    Ok(())
}

/// Half-precision 1.0.
///
/// nvdisplay 3's composition stage works in FP16: the window's input LUT
/// converts to it, and the head's output LUT back from it. Between those two a
/// 16-bit field is a half-float: 0xffff is a NaN with the sign bit set, and
/// every stage downstream of a NaN produces black
pub const FP16_ONE: u16 = 0x3c00;

/// `nvUnorm10ToFp16` from `common/softfloat/nvidia/nv-softfloat.h`: the 10-bit
/// index `i` as the half-float `i / 1024`.
///
/// Exact in integer arithmetic, with no denormal case. With `k` the index of
/// the highest set bit, the value is `2^(k-10) * (1 + f)`, so the exponent
/// field is `k - 10 + 15` and the mantissa the remaining bits shifted up into
/// the format's ten. `k` never exceeds 9, so the exponent field stays in 5..=14
/// and every entry is normal
pub const fn unorm10_to_fp16(i: u32) -> u16 {
    if i == 0 {
        return 0;
    }
    let k = 31 - i.leading_zeros();
    let exp = k + 5;
    let mantissa = (i - (1 << k)) << (10 - k);
    ((exp << 10) | mantissa) as u16
}

/// The head's output LUT: 16-bit fixed point, identity.
///
/// `EvoSetupIdentityOutputLutC5` writes `i << (16 - 10)` per component:
/// "nvdisplay 3 uses 16-bit fixed-point entries in the OLUT". This table sits
/// after composition, past FP16
pub fn build_identity_olut(buf: &mut [u8]) {
    build_olut(buf, &super::Curve::IDENTITY)
}

/// The head's output LUT, built from a colour curve.
///
/// The entries are 16-bit fixed point, so a 10-bit ramp value is scaled by 64.
/// An identity curve produces the identity table byte for byte
pub fn build_olut(buf: &mut [u8], curve: &super::Curve) {
    build_lut(buf, |i| (curve.at(i) * 64) as u16)
}

/// The window's input LUT: FP16, identity.
///
/// `EvoSetupIdentityBaseLutC5` writes `nvUnorm10ToFp16(i)`: "nvdisplay 3 uses
/// FP16 entries in the ILUT", the opposite encoding to the output LUT.
/// Fixed-point entries read as half-floats are denormals in the low half and
/// sign-bit-set in the top, so index 1023, white, decodes as a negative NaN,
/// and every fetched pixel maps to black or nothing
pub fn build_identity_ilut(buf: &mut [u8]) {
    build_lut(buf, unorm10_to_fp16)
}

/// The input LUT with every entry at 1.0.
///
/// Loaded into the window's input LUT it needs no correct pixel: whatever the
/// window fetches leaves this table as white. A white rectangle means the
/// window fetches and composites and the fault is in what memory holds or its
/// layout; black means the window contributes nothing.
///
/// The value is `FP16_ONE`: 0xffff is a NaN in this encoding and answers black
/// whatever the window does
pub fn build_saturated_ilut(buf: &mut [u8]) {
    build_lut(buf, |_| FP16_ONE)
}

pub fn build_lut(buf: &mut [u8], entry: impl Fn(u32) -> u16) {
    for b in buf.iter_mut().take(OLUT_BYTES) {
        *b = 0;
    }
    let mut off = 0x20;
    for i in 0..1024u32 {
        let v = entry(i).to_le_bytes();
        // Red, green and blue; the fourth component is left zero
        for c in 0..3 {
            buf[off + c * 2] = v[0];
            buf[off + c * 2 + 1] = v[1];
        }
        off += 8;
    }
    // The successor entry, a copy of the last real one
    for c in 0..6 {
        buf[off + c] = buf[off - 8 + c];
    }
}
/// HI and LO are eight apart: the pair is indexed by stereo eye, and only eye 0
/// is written (clca7d.h, `HEAD_SET_SURFACE_ADDRESS_HI_CURSOR`)
pub const M_HEAD_SET_SURFACE_ADDRESS_HI_CURSOR: u32 = 0x2170;
pub const M_HEAD_SET_SURFACE_ADDRESS_LO_CURSOR: u32 = 0x2178;
/// `HEAD_SET_CONTROL_CURSOR_FORMAT` at 7:0. A8R8G8B8 is the only format used
pub const CURSOR_FORMAT_A8R8G8B8: u32 = 0xcf;
/// `HEAD_SET_CONTROL_CURSOR_ENABLE` at 31:31
pub const CURSOR_ENABLE: u32 = 1 << 31;

/// 'WINDOW_SET_CONTROL(i)', on the core channel: which head each window feeds
pub fn core_window_owner(push: &mut PushBuf, windows: u32) -> Result<(), PushError> {
    for i in 0..windows {
        push.mthd1(M_WINDOW_SET_CONTROL + i * WINDOW_STRIDE, i >> 1)?;
    }
    Ok(())
}

pub const M_WINDOW_SET_CONTROL: u32 = 0x1000;
pub const WINDOW_STRIDE: u32 = 0x80;

////////////////////////////////////////////////////////////////////////
