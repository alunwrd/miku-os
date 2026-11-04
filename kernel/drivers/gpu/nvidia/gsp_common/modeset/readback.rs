// Reading back armed state, and decoding the exceptions the display engine
// raises when it rejects a method.


// The modeset modules are re-exported flat from `mod.rs`
use super::*;

// Reading back what the hardware took

// A display channel's method state is not write-only. The core channel's BAR0
// region is that state, addressed by method offset: '0x680000 + method' is what
// is staged, and '0x688000 + method' what the hardware uses. nvkm reads this to
// recover an inherited mode ('gv100_head_state' reads 0x682064 for assembly and
// 0x68a064 for armed, RASTER_SIZE at 0x2064 under both bases).

pub const CORE_ASSY_BASE: u32 = 0x0068_0000;
pub const CORE_ARM_BASE: u32 = 0x0068_8000;

/// One method's staged and armed values
pub struct ArmedMethod {
    pub name: &'static str,
    pub assy: u32,
    pub arm: u32,
}

/// Read back the core channel state a modeset depends on
pub fn read_core_state(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    head: u32,
    sor: u32,
    window: u32,
) -> alloc::vec::Vec<ArmedMethod> {
    let h = head * HEAD_STRIDE;
    let methods: [(&'static str, u32); 13] = [
        // First: the newest write, the stage scaling everything the windows
        // composited
        ("CURSOR_COMP", M_HEAD_SET_CONTROL_CURSOR_COMPOSITION + h),
        ("RASTER_SIZE", M_HEAD_SET_RASTER_SIZE + h),
        ("RASTER_SYNC_END", M_HEAD_SET_RASTER_SIZE + 4 + h),
        ("RASTER_BLANK_END", M_HEAD_SET_RASTER_SIZE + 8 + h),
        ("RASTER_BLANK_START", M_HEAD_SET_RASTER_SIZE + 12 + h),
        ("PIXEL_CLOCK", M_HEAD_SET_PIXEL_CLOCK_FREQUENCY + h),
        ("OUTPUT_RESOURCE", M_HEAD_SET_CONTROL_OUTPUT_RESOURCE + h),
        ("DISPLAY_ID", M_HEAD_SET_DISPLAY_ID + h),
        ("VIEWPORT_SIZE_OUT", M_HEAD_SET_VIEWPORT_SIZE_OUT + h),
        ("TILE_MASK", M_HEAD_SET_TILE_MASK + h),
        ("TILE_SIZE", M_TILE_SET_TILE_SIZE + head * TILE_STRIDE),
        ("SOR_CONTROL", M_SOR_SET_CONTROL + sor * SOR_STRIDE),
        ("WINDOW_CONTROL", M_WINDOW_SET_CONTROL + window * WINDOW_STRIDE),
    ];
    methods
        .iter()
        .map(|(name, m)| ArmedMethod {
            name,
            assy: bar0.read32(CORE_ASSY_BASE + m),
            arm: bar0.read32(CORE_ARM_BASE + m),
        })
        .collect()
}

// Display engine exceptions

// A method the display engine rejects raises an exception, stops the channel
// and records what happened: "method 0x0670 was rejected as INVALID_ARG".
//
// 'gv100_disp_exception' in nvkm: per-channel triples at 0x611020 + chid*12
// holding {stat, data, code}, where stat has the error type in bits 14:12 and
// the offending method, divided by four, in bits 11:0. Channel ids are the
// display's own: 0 is the core channel, 1 + n a window, 33 + n a
// window-immediate. Pending exceptions are flagged in 0x611854 (core in bit 0)
// and 0x611850 (window-immediate).

pub const DISP_EXC_STAT: u32 = 0x0061_1020;
pub const DISP_EXC_OTHER: u32 = 0x0061_1854;
pub const DISP_EXC_WINIM: u32 = 0x0061_1850;

pub fn exception_type_name(t: u32) -> &'static str {
    match t {
        0 => "NONE",
        1 => "PUSHBUFFER_ERR",
        2 => "TRAP",
        3 => "RESERVED_METHOD",
        4 => "INVALID_ARG",
        5 => "INVALID_STATE",
        7 => "UNRESOLVABLE_HANDLE",
        _ => "?",
    }
}

/// Channel ids as the display engine numbers them
pub const CHID_CORE: u32 = 0;
pub fn chid_window(window: u32) -> u32 {
    1 + window
}
pub fn chid_wimm(window: u32) -> u32 {
    33 + window
}

/// Front-end channel state, how nvkm decides a channel has settled.
///
/// 'gv100_disp_core_idle' polls '0x610630' for state '0xb' in bits 20:16, and
/// 'gv100_disp_dmac_idle' polls '0x610664 + (chid - 1) * 4' for state '0x4' in
/// bits 19:16. Nothing from 'ga102_disp' on overrides the pair.
///
/// A channel that read its methods but holds an update it cannot issue shows
/// only here: 'GET' catching 'PUT' reports the parser, and the assembly region
/// what was staged
pub fn channel_state(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    window: u32,
) -> (u32, u32) {
    (
        bar0.read32(REG_CHNSTATUS_CORE),
        bar0.read32(REG_CHNSTATUS_WIN + (chid_window(window) - 1) * 4),
    )
}

pub const REG_CHNSTATUS_CORE: u32 = 0x0061_0630;
pub const REG_CHNSTATUS_WIN: u32 = 0x0061_0664;

/// Report any exception the display engine has recorded.
///
/// Read-only: the pending flags are not cleared, so a later dump still sees the
/// first failure
pub fn read_exceptions(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    window: u32,
    report: &dyn Fn(&str),
) {
    report(&alloc::format!(
        "exc pending: other {:#010x} winim {:#010x}",
        bar0.read32(DISP_EXC_OTHER),
        bar0.read32(DISP_EXC_WINIM)
    ));
    for (name, chid) in [
        ("core", CHID_CORE),
        ("wndw", chid_window(window)),
        ("wimm", chid_wimm(window)),
    ] {
        let base = DISP_EXC_STAT + chid * 12;
        let stat = bar0.read32(base);
        if stat == 0 {
            report(&alloc::format!("exc {} (chid {}): none", name, chid));
            continue;
        }
        let ty = (stat >> 12) & 0x7;
        let mthd = (stat & 0xfff) << 2;
        report(&alloc::format!(
            "exc {} (chid {}): stat {:#010x} {} mthd {:#06x} data {:#010x} code {:#010x}",
            name, chid, stat, exception_type_name(ty), mthd,
            bar0.read32(base + 4), bar0.read32(base + 8)
        ));
    }
}
