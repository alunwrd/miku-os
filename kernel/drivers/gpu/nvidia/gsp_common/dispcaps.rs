// What this display engine has: heads, windows, tiles, SORs, and their limits
//
// `EvoGetCapabilitiesCA` (`nvkms-evo4.c`) allocates the capabilities object
// (`NVCA73_DISP_CAPABILITIES`) under the display, maps it, and reads what the
// engine has from its register block: `nvEvoGetCapabilities3`
// (`nvkms-evo3.c:6052`), `EvoParseCapabilityNotifier3` (`:5482`),
// `nvEvoParseCapabilityNotifier6` (`:5892`), `EvoParseCapabilityNotifierCA`
// (`nvkms-evo4.c`). The mapping is `NV_PDISP_FE_SW`, BAR0 0x640000
// (`kdispGetDisplayCapsBaseAndSize_v03_00`), one page of registers
// (`NvCA73DispCapabilities_Map`).
//
// The constants here (four heads, eight windows, one tile per head) are the
// fallback when the block reads as nonsense; `dev/display-nvkms.md`, decision
// 1.

use crate::drivers::gpu::nvidia::gsp_common::rmapi::{RmClient, RmError};
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

/// `NV_PDISP_FE_SW`
pub const CAPS_BASE: u32 = 0x0064_0000;

// Offsets into the block (`clca73.h`)
const SYS_CAP: u32 = 0x0;
const SYS_CAPB: u32 = 0x4;
const SYS_CAPC: u32 = 0x20;
const IHUB_COMMON_CAPF: u32 = 0x28;
const fn sor_cap(i: u32) -> u32 { 0x144 + i * 8 }
/// `NVC373_HEAD_CLK_CAP(i)`: NVKMS defines it itself as a WAR (bug 4028718);
/// the class headers do not
const fn head_clk_cap(i: u32) -> u32 { 0x5e8 + i * 4 }
const fn sor_clk_cap(i: u32) -> u32 { 0x608 + i * 4 }
const fn postcomp_hdr_capa(tile: u32) -> u32 { 0x680 + tile * 32 }

// `NVCA73_SYS_CAP_*_EXISTS__SIZE_1` and friends
const MAX_HEADS: u32 = 8;
const MAX_SORS: u32 = 8;
const MAX_TILES: u32 = 8;

/// `NV_EVO_HW_TILE_TYPE_0`: the top variant, with a scaler and a vertical
/// filter, what a scaled or YUV 4:2:0 head needs
pub const TILE_TYPE_0: u8 = 0;
/// `NV_EVO_HW_TILE_TYPE_1`: trimmed, enough for a head that does neither
pub const TILE_TYPE_1: u8 = 1;

/// The allocation handle, by the driver's `class << 16` convention
pub const H_CAPS: u32 = super::disp::GB202_DISP_CAPS << 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    /// `SYS_CAP_HEAD_EXISTS`, one bit per head
    pub heads: u32,
    /// `SYS_CAP_SOR_EXISTS`
    pub sors: u32,
    /// `SYS_CAPB_WINDOW_EXISTS`
    pub windows: u32,
    /// `SYS_CAPC_TILE_EXISTS`
    pub tiles: u32,
    /// `SYS_CAPC_TILE_SUPPORT_MULTI_TILE`
    pub multi_tile: u32,
    /// Per tile, `TILE_TYPE_0` or `_1` (`EvoGetHwTileType`)
    pub tile_type: [u8; MAX_TILES as usize],
    /// `IHUB_COMMON_CAPF_PHYWIN_SUPPORT_MULTI_TILE`, one bit per physical
    /// window
    pub phywin_multi_tile: u32,
    /// Per head, the pixel-clock ceiling in kHz (`HEAD_CLK_CAP_PCLK_MAX` x 10
    /// MHz)
    pub pclk_max_khz: [u32; MAX_HEADS as usize],
    /// SORs that can drive dual-link TMDS
    pub sor_dual_tmds: u32,
    /// Per SOR, the TMDS clock ceiling in kHz (`SOR_CLK_CAP_TMDS_MAX` x 10 MHz)
    pub sor_tmds_max_khz: [u32; MAX_SORS as usize],
}

fn bit(v: u32, i: u32) -> bool {
    v >> i & 1 != 0
}

/// How many bits are set from bit 0 up without a gap: `UsableWindowCount` and
/// `UsableHwTilesCount` assert the usable ones start at 0 and are contiguous
fn contiguous(mask: u32) -> u32 {
    (!mask).trailing_zeros()
}

impl Caps {
    /// Decode the block, reading each register through `read(offset)`
    pub fn parse(read: &dyn Fn(u32) -> u32) -> Caps {
        let sys_cap = read(SYS_CAP);
        let sys_capb = read(SYS_CAPB);
        let sys_capc = read(SYS_CAPC);
        let mut c = Caps {
            heads: sys_cap & 0xff,
            sors: (sys_cap >> 8) & 0xff,
            windows: sys_capb,
            tiles: sys_capc & 0xff,
            multi_tile: (sys_capc >> 8) & 0xff,
            phywin_multi_tile: read(IHUB_COMMON_CAPF),
            ..Caps::default()
        };
        for h in 0..MAX_HEADS {
            if bit(c.heads, h) {
                c.pclk_max_khz[h as usize] = (read(head_clk_cap(h)) & 0xff) * 10_000;
            }
        }
        for s in 0..MAX_SORS {
            if bit(c.sors, s) {
                if bit(read(sor_cap(s)), 11) {
                    c.sor_dual_tmds |= 1 << s;
                }
                c.sor_tmds_max_khz[s as usize] = ((read(sor_clk_cap(s)) >> 16) & 0xff) * 10_000;
            }
        }
        for t in 0..MAX_TILES {
            // `EvoGetHwTileType`: the scaler and the vertical filter both
            // present make the top variant
            let capa = read(postcomp_hdr_capa(t));
            c.tile_type[t as usize] = if bit(c.tiles, t) && bit(capa, 18) && bit(capa, 23) {
                TILE_TYPE_0
            } else {
                TILE_TYPE_1
            };
        }
        c
    }

    pub fn num_heads(&self) -> u32 {
        contiguous(self.heads)
    }

    /// `UsableWindowCount`
    pub fn num_windows(&self) -> u32 {
        contiguous(self.windows)
    }

    /// `UsableHwTilesCount`
    pub fn num_tiles(&self) -> u32 {
        contiguous(self.tiles)
    }

    /// "In GB20X, the number of physical windows is same as the number of
    /// windows" (`EvoGetCapabilitiesCA`)
    pub fn num_phywins(&self) -> u32 {
        self.num_windows()
    }

    /// Whether this reads like a display engine and not an unpopulated block or
    /// a PRI error. Every mask contiguous from bit 0 and non-empty, and no more
    /// windows than two per head explain
    pub fn plausible(&self) -> bool {
        let packed = |m: u32| m != 0 && m != u32::MAX && m.count_ones() == contiguous(m);
        packed(self.heads)
            && packed(self.windows)
            && packed(self.tiles)
            && self.sors != 0
            && self.num_windows() <= 2 * MAX_HEADS
    }

    /// One line for the boot log
    pub fn describe(&self) -> alloc::string::String {
        let type0: u32 = (0..MAX_TILES)
            .filter(|&t| bit(self.tiles, t) && self.tile_type[t as usize] == TILE_TYPE_0)
            .fold(0, |m, t| m | 1 << t);
        let pclk = self.pclk_max_khz.iter().copied().max().unwrap_or(0);
        let tmds = self.sor_tmds_max_khz.iter().copied().max().unwrap_or(0);
        alloc::format!(
            "heads {:#x} windows {:#x} tiles {:#x} (scaler {:#x}, multi {:#x}) sors {:#x} (dual TMDS {:#x}), pclk max {} MHz, TMDS max {} MHz",
            self.heads, self.windows, self.tiles, type0, self.multi_tile, self.sors,
            self.sor_dual_tmds, pclk / 1000, tmds / 1000
        )
    }
}

/// Allocate the capabilities object under the display root and read the block.
/// The read is BAR0; NVKMS allocates the object before mapping the block, and
/// it is kept for the display's lifetime
pub fn read(
    rpc: &mut GspRpc,
    client: &RmClient,
    h_disp_root: u32,
    bar0: &MmioRegion,
) -> Result<Caps, RmError> {
    client.alloc(rpc, h_disp_root, H_CAPS, super::disp::GB202_DISP_CAPS, &[])?;
    Ok(Caps::parse(&|off| bar0.read32(CAPS_BASE + off)))
}

static CAPS: spin::Mutex<Option<Caps>> = spin::Mutex::new(None);

/// Keep what was read, if it is believable
pub fn set(caps: Option<Caps>) {
    *CAPS.lock() = caps.filter(Caps::plausible);
}

/// What the engine has, when a believable block was read this boot
pub fn get() -> Option<Caps> {
    *CAPS.lock()
}
