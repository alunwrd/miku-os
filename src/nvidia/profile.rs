// NVIDIA chip-profile registry
//
// The driver's engine code (Falcon / GSP / SEC2 / NVDEC) is generic across
// every GSP-era family (Turing onward): the register *windows* have the
// same shape, only their base offsets and the per-chip firmware change.
// This module is the pluggable layer that maps a decoded 'ChipId' onto:
//
//   - the set of Falcon engine base offsets inside BAR0,
//   - whether a signed firmware bundle is embedded for this chip (only the
//     TU116 GTX 1650 today), and therefore whether the full GSP-RM offload
//     pipeline can run or the driver stops at host-side bring-up,
//   - a printable fallback model string when no SKU table claims the device.
//
// Adding a new chip is then a matter of adding a 'ChipProfile' entry (and,
// for full GSP support, an embedded firmware bundle module like
// 'gtx1650/tu116_fw.rs'). The generic bring-up in 'nvidia::generic' consumes
// a profile without knowing which silicon produced it.
//
// Engine base offsets below are the Turing map (cross-checked against
// nouveau and open-gpu-kernel-modules). Ampere and Ada keep the same GSP /
// SEC2 / FECS bases; NVDEC instance count and the GPCCS per-GPC stride vary
// by chip, but the liveness probe only issues HWCFG *reads*, so a base that
// is wrong for a given GPC simply reads back the PRI sentinel and is
// reported as gated - it never hangs or corrupts state.

use crate::nvidia::chip::{Architecture, ChipId};
use crate::nvidia::gtx1650::falcon;

/// Falcon engine base offsets inside BAR0 for one chip
#[derive(Copy, Clone, Debug)]
pub struct EngineBases {
    pub gsp: u32,
    pub sec2: u32,
    pub nvdec0: u32,
    pub fecs: u32,
    pub gpccs0: u32,
    pub gpccs1: u32,
}

impl EngineBases {
    /// The Turing-family base map. Reused for Ampere and Ada: the GSP,
    /// SEC2 and FECS bases are stable across these generations. GPCCS1 is
    /// only present on multi-GPC parts; on single-GPC chips its window
    /// reads back as the PRI sentinel (reported gated)
    pub const fn turing_family() -> Self {
        Self {
            gsp:    falcon::PGSP_BASE,
            sec2:   falcon::PSEC_BASE,
            nvdec0: falcon::PNVDEC_BASE,
            fecs:   falcon::PFECS_BASE,
            gpccs0: falcon::PGPCCS0_BASE,
            gpccs1: falcon::PGPCCS1_BASE,
        }
    }
}

/// Everything the generic bring-up needs to know about a chip without
/// being hard-coded to one SKU
#[derive(Copy, Clone, Debug)]
pub struct ChipProfile {
    /// Architecture family (Turing / Ampere / Ada / ...)
    pub arch: Architecture,
    /// Chip codename ("TU116", "GA104", ...) or "unknown"
    pub codename: &'static str,
    /// Falcon engine base offsets for this chip
    pub engines: EngineBases,
    /// True if an embedded, signed firmware bundle exists for this chip and
    /// the full GSP-RM offload pipeline (scrubber -> ACR -> booter -> RM)
    /// can be attempted. Today only the TU116 GTX 1650 ships one
    pub has_firmware: bool,
    /// Printable fallback when no per-SKU model table matches the device id
    pub model_hint: &'static str,
}

impl ChipProfile {
    /// Resolve the profile for a decoded chip. Every GSP-era family gets the
    /// Turing engine map and host-side bring-up; full-firmware support is
    /// gated on an embedded bundle, which only TU116 has so far
    pub fn resolve(chip: &ChipId) -> Self {
        let codename = chip.codename();
        let engines = EngineBases::turing_family();

        // Only TU116 carries an embedded firmware bundle (gtx1650/tu116_fw).
        // TU117 shares the host map but ships no bundle of its own yet, so
        // it also stops at host-side bring-up unless routed through the
        // gtx1650 firmware path by device-id match
        let has_firmware = matches!(chip.arch, Architecture::Turing) && chip.implementation == 0x8;

        let model_hint = match chip.arch {
            Architecture::Turing      => "NVIDIA Turing GPU",
            Architecture::Ampere      => "NVIDIA Ampere GPU",
            Architecture::Hopper      => "NVIDIA Hopper GPU",
            Architecture::AdaLovelace => "NVIDIA Ada Lovelace GPU",
            Architecture::Blackwell   => "NVIDIA Blackwell GPU",
            Architecture::Unknown(_)  => "NVIDIA GPU (unrecognized family)",
        };

        Self { arch: chip.arch, codename, engines, has_firmware, model_hint }
    }

    /// True if the driver should attempt host-side bring-up at all. Even
    /// unknown families are probed (reads are safe), but the caller may want
    /// to log a louder warning for those
    pub fn host_bringup_supported(&self) -> bool {
        // BAR0 register reads are non-destructive on any NVIDIA GPU, so we
        // always allow the host-side probe; the distinction is only how much
        // we trust the decoded values
        true
    }
}
