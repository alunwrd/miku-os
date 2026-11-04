// NVIDIA chip-profile registry
//
// The engine code (Falcon / GSP / SEC2 / NVDEC) is generic across every GSP-era
// family (Turing onward): the register windows have the same shape, only their
// base offsets and the per-chip firmware change. This maps a decoded 'ChipId'
// onto:
//
//   - the set of Falcon engine base offsets inside BAR0,
//   - whether a signed firmware bundle is embedded for this chip (only the
//     TU116 GTX 1650), and so whether the full GSP-RM offload pipeline can run
//     or the driver stops at host-side bring-up,
//   - a printable fallback model string when no SKU table claims the device.
//
// A new chip takes a 'ChipProfile' entry (and, for full GSP support, an
// embedded firmware bundle module like 'turing/tu116_fw.rs'). 'nvidia::generic'
// consumes a profile without knowing the silicon.
//
// Engine base offsets below are the Turing map (cross-checked against nouveau
// and open-gpu-kernel-modules). Ampere and Ada keep the same GSP / SEC2 / FECS
// bases; NVDEC instance count and the GPCCS per-GPC stride vary by chip, but
// the liveness probe issues only HWCFG reads, so a wrong base reads back the
// PRI sentinel and is reported as gated

use crate::drivers::gpu::nvidia::chip::{Architecture, ChipId};
use crate::drivers::gpu::nvidia::falcon;

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
    /// The Turing-family base map, reused for Ampere and Ada. GPCCS1 exists
    /// only on multi-GPC parts; on single-GPC chips its window reads back as
    /// the PRI sentinel (reported gated)
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

/// Everything the generic bring-up needs to know about a chip without being
/// hard-coded to one SKU
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub struct ChipProfile {
    /// Architecture family (Turing / Ampere / Ada / ...)
    pub arch: Architecture,
    /// Chip codename ("TU116", "GA104", ...) or "unknown"
    pub codename: &'static str,
    /// Falcon engine base offsets for this chip
    pub engines: EngineBases,
    /// True if an embedded, signed firmware bundle exists for this chip and the
    /// full GSP-RM offload pipeline (scrubber -> ACR -> booter -> RM) can be
    /// attempted. Only the TU116 GTX 1650 ships one
    pub has_firmware: bool,
    /// Printable fallback when no per-SKU model table matches the device id
    pub model_hint: &'static str,
}

impl ChipProfile {
    /// Resolve the profile for a decoded chip. Every GSP-era family gets the
    /// Turing engine map and host-side bring-up; full-firmware support is gated
    /// on an embedded bundle
    pub fn resolve(chip: &ChipId) -> Self {
        let codename = chip.codename();
        let engines = EngineBases::turing_family();

        // Only TU116 carries an embedded firmware bundle (turing/tu116_fw).
        // TU117 shares the host map and ships none, so it stops at host-side
        // bring-up unless routed through the turing firmware path by device-id
        // match
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
}
