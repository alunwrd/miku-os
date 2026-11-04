// The window channel method stream (NVCA7E): where a surface in memory becomes
// pixels.

use super::super::pushbuf::{PushBuf, PushError};

// The modeset modules are re-exported flat from `mod.rs`
use super::*;

// Window channel method stream (NVCA7E)

// A window is where a surface in memory becomes pixels. The head decided the
// timings; this decides what is inside them.

pub const M_WNDW_UPDATE: u32 = 0x0200;
pub const M_WNDW_SET_SIZE: u32 = 0x0224;
pub const M_WNDW_SET_POINT_IN: u32 = 0x0290;
pub const M_WNDW_SET_SIZE_IN: u32 = 0x0298;
pub const M_WNDW_SET_SIZE_OUT: u32 = 0x02a4;
pub const M_WNDW_SET_COMPOSITION_CONTROL: u32 = 0x02ec;
pub const M_WNDW_SET_PRESENT_CONTROL: u32 = 0x0308;
pub const M_WNDW_SET_INTERLOCK_FLAGS: u32 = 0x0370;
/// The FMT matrix, from NVC57E, which the Blackwell window class inherits:
/// nouveau's 'wndwca7e' reuses 'wndwc57e_csc_set'
pub const M_WNDW_SET_FMT_COEFFICIENT_C00: u32 = 0x0400;
pub const M_WNDW_SET_SURFACE_ADDRESS_LO_NOTIFIER: u32 = 0x0654;

// The window's acquire semaphore

// A window has two semaphore slots: the acquire one holds the arm back until
// memory reaches a value, the release one writes a value after the arm. NVKMS
// uses the first to make a flip wait for whoever fills the surface, never
// waiting on the processor: `EvoProgramSemaphore6`, `nvkms-evo3.c:3719`.
//
// The hardware waits, so the frame loop keeps its parallelism, and the head
// cannot latch a surface the copy engine is halfway through.
//
// Two things fail silently:
//
// * The address is physical, with an aperture: `ADDRESS_LO` is field 31:4, so
//   16-byte aligned. The copy engine writes the same page through its own
//   address space.
// * `ACQ_MODE` is strict equality. A repeating value can be matched by one the
//   hardware prefetched (NVKMS warns of this, `nvkms-kapi-notifiers.c:308`).
//   The copy engine's fence is a monotonic counter, used directly where NVKMS
//   needs a ring of 256 slots.

pub const M_WNDW_SET_ACQ_SEMAPHORE_CONTROL: u32 = 0x0330;
pub const M_WNDW_SET_ACQ_SEMAPHORE_VALUE: u32 = 0x0334;
pub const M_WNDW_SET_SURFACE_ADDRESS_HI_ACQ_SEMAPHORE: u32 = 0x0648;
pub const M_WNDW_SET_SURFACE_ADDRESS_LO_ACQ_SEMAPHORE: u32 = 0x064c;

/// `SET_ACQ_SEMAPHORE_CONTROL_ACQ_MODE`, field 13:12. `EQ` is what NVKMS uses
/// for a semaphore (a Tegra syncpt gets `CGEQ`)
pub const ACQ_MODE_EQ: u32 = 0;

/// Stop the window arming until `value` appears in the first dword at `phys`.
///
/// `target` is the aperture enum of the surface address: `PHYSICAL_NVM` for
/// video memory, `PHYSICAL_PCI_COHERENT` for system memory on a discrete card.
/// The display semaphore is sixteen bytes and the compared word is dword zero
/// (`NV_DISP_NOTIFIER__0`), where the copy engine's one-word release lands
pub fn window_set_acq_semaphore(
    push: &mut PushBuf,
    phys: u64,
    target: u32,
    value: u32,
) -> Result<(), PushError> {
    push.mthd1(M_WNDW_SET_SURFACE_ADDRESS_HI_ACQ_SEMAPHORE, (phys >> 32) as u32)?;
    push.mthd1(
        M_WNDW_SET_SURFACE_ADDRESS_LO_ACQ_SEMAPHORE,
        ((phys as u32) & !0xf) | (target << 2) | 1,
    )?;
    // PAYLOAD_SIZE stays 0, 32 bits, as NVKMS leaves it and as the copy
    // engine's one-word release writes
    push.mthd1(M_WNDW_SET_ACQ_SEMAPHORE_CONTROL, ACQ_MODE_EQ << 12)?;
    push.mthd1(M_WNDW_SET_ACQ_SEMAPHORE_VALUE, value)
}

/// Take the acquire off the window.
///
/// Needed on every path that stops using it: the slot is channel state, not
/// part of an update, and a window with an enabled acquire on a value nothing
/// writes never arms again
pub fn window_clear_acq_semaphore(push: &mut PushBuf) -> Result<(), PushError> {
    push.mthd1(M_WNDW_SET_SURFACE_ADDRESS_LO_ACQ_SEMAPHORE, 0)
}
pub const M_WNDW_SET_SURFACE_ADDRESS_HI_ISO: u32 = 0x0658;
pub const M_WNDW_SET_SURFACE_ADDRESS_LO_ISO: u32 = 0x0670;
pub const M_WNDW_SET_SURFACE_ADDRESS_HI_ILUT: u32 = 0x0688;
pub const M_WNDW_SET_SURFACE_ADDRESS_LO_ILUT: u32 = 0x068c;
pub const M_WNDW_SET_ILUT_CONTROL: u32 = 0x0440;
/// The four colour matrices and two LUTs of the window's HDR pipeline, each
/// with its own ENABLE bit. `ConfigureCsc0C5` and `ConfigureCsc1C5` write all
/// six on every flip; with no tone mapping or wide gamut all are disabled, and
/// `SetCscMatrixC5Wrapper` writes no coefficients for a matrix it is switching
/// off
pub const M_WNDW_SET_CSC00CONTROL: u32 = 0x045c;
pub const M_WNDW_SET_CSC0LUT_CONTROL: u32 = 0x04a0;
pub const M_WNDW_SET_CSC01CONTROL: u32 = 0x04bc;
pub const M_WNDW_SET_CSC10CONTROL: u32 = 0x053c;
pub const M_WNDW_SET_CSC1LUT_CONTROL: u32 = 0x0580;
pub const M_WNDW_SET_CSC11CONTROL: u32 = 0x059c;
pub const M_WNDW_SET_SURFACE_ADDRESS_HI_TMO_LUT: u32 = 0x0690;

/// 'SET_PARAMS_FORMAT_A8R8G8B8'
pub const FORMAT_A8R8G8B8: u32 = 0xcf;

/// 'SET_SURFACE_ADDRESS_LO_ISO_TARGET'
pub const SURFACE_TARGET_VRAM: u32 = 1;
#[allow(dead_code)] // one aperture of the class's SURFACE_TARGET enum
pub const SURFACE_TARGET_PCI: u32 = 2;
pub const SURFACE_TARGET_PCI_COHERENT: u32 = 3;

/// Pitch is stored in units of 64 bytes, so a scanline is a whole number of
/// them
pub const PITCH_GRANULARITY: u32 = 64;

/// Point a window at no surface: `SET_SURFACE_ADDRESS_LO_ISO` with ENABLE
/// clear, which is what a layer flipped to NULL gets (`nvkms-evo4.c:1615`).
/// For a head being shut down (`dev/display-nvkms.md`, decision 7)
pub fn window_disable(push: &mut PushBuf) -> Result<(), PushError> {
    push.mthd1(M_WNDW_SET_SURFACE_ADDRESS_HI_ISO, 0)?;
    push.mthd1(M_WNDW_SET_SURFACE_ADDRESS_LO_ISO, 0)
}

/// Point a window at a pitch-linear surface covering the whole viewport.
///
/// 'phys' may be system memory, as 'target' decides
pub fn window_set_image(
    push: &mut PushBuf,
    phys: u64,
    pitch: u32,
    width: u32,
    height: u32,
    target: u32,
) -> Result<(), PushError> {
    push.mthd1(M_WNDW_SET_SURFACE_ADDRESS_HI_ISO, (phys >> 32) as u32)?;
    // ADDRESS_LO occupies 31:4, so the address's low four bits are not stored
    // and the field shares its word with the flags below
    let lo = (phys as u32) & !0xf;
    push.mthd1(
        M_WNDW_SET_SURFACE_ADDRESS_LO_ISO,
        // KIND stays 0 = PITCH; block-linear would need a tiling layout the CPU
        // writes through
        lo | (target << 2) | 1,
    )?;

    // MIN_PRESENT_INTERVAL 1 with BEGIN_MODE NON_TEARING (0), what nouveau
    // forces when a window update is interlocked with the core:
    // 'asyw->image.interval = 1' in 'nv50_wndw_flush_set'. Interval 0 is the
    // free-running page-flip case
    push.mthd1(M_WNDW_SET_PRESENT_CONTROL, 1)?;

    // SET_SIZE, SET_STORAGE, SET_PARAMS and SET_PLANAR_STORAGE(0) are four
    // consecutive methods
    push.method(
        M_WNDW_SET_SIZE,
        &[
            width | (height << 16),
            0, // BLOCK_HEIGHT, meaningless for a pitch surface
            FORMAT_A8R8G8B8,
            pitch / PITCH_GRANULARITY,
        ],
    )?;

    // No cropping and no scaling: the surface, the sampled region and the
    // output rectangle are all the same size
    push.mthd1(M_WNDW_SET_POINT_IN, 0)?;
    push.mthd1(M_WNDW_SET_SIZE_IN, width | (height << 16))?;
    push.mthd1(M_WNDW_SET_SIZE_OUT, width | (height << 16))
}

/// Put the window's other pipeline stages into a known pass-through state.
///
/// The firmware left this window configured for its console, and an unwritten
/// method keeps its value. An inherited lookup table or a zeroed colour matrix
/// in the path gives black with every channel reporting success.
///
/// nouveau's window sub-blocks each have a '_set' and a '_clr', and a first
/// modeset runs '_clr' for what it does not use. This is that half.
pub fn window_reset_pipeline(push: &mut PushBuf, ilut: Option<u64>) -> Result<(), PushError> {
    // The pre-composition scaler's coefficient RAM, which `EvoInitChannelC9`
    // loads on every window channel
    init_input_scaler_coeffs(push)?;

    // No notifier. Bit 0 is ENABLE
    push.mthd1(M_WNDW_SET_SURFACE_ADDRESS_LO_NOTIFIER, 0)?;

    // The window's input LUT, as necessary as the head's output LUT.
    //
    // On a lit desktop this card holds 'SET_ILUT_CONTROL = 0x00040508' and an
    // enabled ILUT surface in video memory, the head's OLUT encoding, DIRECT10
    // with 1029 entries. nouveau's 'wndwca7e_ilut_clr' disables it, which
    // leaves black.
    //
    // The layout and the entry count match, so the head and the window can
    // point at one buffer
    match ilut {
        Some(phys) => {
            push.method(
                M_WNDW_SET_SURFACE_ADDRESS_HI_ILUT,
                &[
                    (phys >> 32) as u32,
                    (phys as u32 & !0xf) | (SURFACE_TARGET_VRAM << 2) | 1,
                ],
            )?;
            push.mthd1(
                M_WNDW_SET_ILUT_CONTROL,
                (OLUT_MODE_DIRECT10 << 2) | ((OLUT_ENTRIES as u32) << 8),
            )?;
        }
        None => push.mthd1(M_WNDW_SET_SURFACE_ADDRESS_LO_ILUT, 0)?,
    }

    // The FMT matrix, as identity. Twelve consecutive coefficients forming a
    // 3x4 matrix in 16.16 fixed point, so 1.0 is 0x00010000. A zero row would
    // silently drop a colour channel
    const ONE: u32 = 0x0001_0000;
    push.method(
        M_WNDW_SET_FMT_COEFFICIENT_C00,
        &[ONE, 0, 0, 0, 0, ONE, 0, 0, 0, 0, ONE, 0],
    )?;

    // The HDR pipeline, switched off stage by stage.
    //
    // Four colour matrices and two LUTs sit between the input LUT and
    // composition, converting a wide-gamut or PQ-encoded source into the
    // panel's gamut. The source and the output here are plain full-range RGB.
    // `ConfigureCsc0C5(enable = FALSE)` and its CSC1 twin write these six
    // methods on every flip of an ordinary desktop.
    //
    // An enabled matrix full of zeroes is a black window with every other
    // method correct, the head's OCSC0 trap one stage earlier
    push.mthd1(M_WNDW_SET_CSC00CONTROL, 0)?;
    push.mthd1(M_WNDW_SET_CSC0LUT_CONTROL, 0)?;
    push.mthd1(M_WNDW_SET_CSC01CONTROL, 0)?;
    push.mthd1(M_WNDW_SET_CSC10CONTROL, 0)?;
    push.mthd1(M_WNDW_SET_CSC1LUT_CONTROL, 0)?;
    push.mthd1(M_WNDW_SET_CSC11CONTROL, 0)?;

    // The tone-mapping LUT, disabled as `ConfigureTmoLut` does: by clearing the
    // surface. TARGET stays PHYSICAL_NVM with ENABLE clear, the lit driver's
    // 0x00000004
    push.method(
        M_WNDW_SET_SURFACE_ADDRESS_HI_TMO_LUT,
        &[0, SURFACE_TARGET_VRAM << 2],
    )?;

    // Composition: seven consecutive methods from SET_COMPOSITION_CONTROL to
    // SET_KEY_BLUE_CB.
    //
    // The shipping driver's values, read off a lit desktop: CONTROL 0x80,
    // CONSTANT_ALPHA 0, FACTOR_SELECT 0x11. Source factor ONE and destination
    // factor ZERO show the source, with no constant alpha involved
    const K1: u32 = 0;
    const DEPTH: u32 = 8;
    const FACTOR_ZERO: u32 = 0;
    const FACTOR_ONE: u32 = 1;
    let factors =
        FACTOR_ONE | (FACTOR_ONE << 4) | (FACTOR_ZERO << 8) | (FACTOR_ZERO << 12);
    // Colour keying off; an inherited key range would otherwise still apply
    const KEY_FULL: u32 = 0xffff_0000;
    push.method(
        M_WNDW_SET_COMPOSITION_CONTROL,
        &[DEPTH << 4, K1, factors, KEY_FULL, KEY_FULL, KEY_FULL, KEY_FULL],
    )
}

/// Commit the window's staged state.
///
/// 'SET_INTERLOCK_FLAGS' and 'SET_WINDOW_INTERLOCK_FLAGS' are two consecutive
/// methods: bit 0 of the first interlocks with the core channel, and the second
/// is a mask of window channels.
///
/// nouveau pairs the two updates on a modeset: the window's carries
/// INTERLOCK_WITH_CORE and the core's that follows carries the window's bit, so
/// the new surface and head state take effect on one frame. A standalone window
/// update is for a plain page flip
pub fn window_update(
    push: &mut PushBuf,
    interlock_core: bool,
    window_mask: u32,
) -> Result<(), PushError> {
    push.method(
        M_WNDW_SET_INTERLOCK_FLAGS,
        &[interlock_core as u32, window_mask],
    )?;
    push.mthd1(M_WNDW_UPDATE, 1)
}

/// What the window channel armed, as opposed to staged.
///
/// From a dump of this card under the shipping driver: the lit head's geometry
/// word is at both '0x696224' and '0x696a24' for window 6, the same method
/// 0x800 apart. Each window's 4 KiB page is split: staged methods at the base,
/// armed ones at base + 0x800, every window method offset below 0x800.
///
/// The staged half only reads back what we wrote; the raster reads this one
pub fn window_armed_state(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    window: u32,
) -> [(&'static str, u32); 6] {
    let base = super::super::pushbuf::wndw_chan_user(window) + WNDW_ARM_OFFSET;
    [
        // HI is method 0x658 and LO is 0x670, not adjacent, so two separate
        // methods; a surface above 4 GiB is wrong in a way only this half shows
        ("hi", bar0.read32(base + M_WNDW_SET_SURFACE_ADDRESS_HI_ISO)),
        ("iso", bar0.read32(base + M_WNDW_SET_SURFACE_ADDRESS_LO_ISO)),
        ("size", bar0.read32(base + M_WNDW_SET_SIZE)),
        ("params", bar0.read32(base + M_WNDW_SET_SIZE + 8)),
        ("fact", bar0.read32(base + M_WNDW_SET_COMPOSITION_CONTROL + 8)),
        ("ilut", bar0.read32(base + M_WNDW_SET_ILUT_CONTROL)),
    ]
}

pub const WNDW_ARM_OFFSET: u32 = 0x800;

/// Read back a window channel's staged method state.
///
/// The core channel's BAR0 region is its method state; a window channel's page
/// is 0x1000 and every window method fits inside, so the same overlay applies.
/// nvkm never reads window state, so the dump includes methods with known
/// values: 'SET_SIZE' reading back as the mode shows the mapping is real.
///
/// Each value goes to 'report' as it is read.
pub fn read_window_state(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    window: u32,
    report: &dyn Fn(&str, u32),
) {
    let base = super::super::pushbuf::wndw_chan_user(window);
    let methods: [(&'static str, u32); 10] = [
        ("SET_SIZE", M_WNDW_SET_SIZE),
        ("SET_PARAMS", M_WNDW_SET_SIZE + 8),
        ("SET_PLANAR_STORAGE", M_WNDW_SET_SIZE + 12),
        ("SIZE_OUT", M_WNDW_SET_SIZE_OUT),
        // The older per-component colour matrix. NVC57E moved the matrix to
        // SET_FMT_COEFFICIENT at 0x400; if this block still applies on the
        // Blackwell class, an inherited value would gate the picture with no
        // error
        ("CSC_RED2RED", 0x02bc),
        ("COMPOSITION_CONTROL", M_WNDW_SET_COMPOSITION_CONTROL),
        ("COMPOSITION_FACTORS", M_WNDW_SET_COMPOSITION_CONTROL + 8),
        ("FMT_COEFF_C00", M_WNDW_SET_FMT_COEFFICIENT_C00),
        ("SURFACE_HI_ISO", M_WNDW_SET_SURFACE_ADDRESS_HI_ISO),
        ("SURFACE_LO_ISO", M_WNDW_SET_SURFACE_ADDRESS_LO_ISO),
    ];
    for (name, m) in methods {
        report(name, bar0.read32(base + m));
    }
}

////////////////////////////////////////////////////////////////////////
