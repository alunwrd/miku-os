// Per-chip quirks for TU116 / TU117 (and room for further Turing SKUs)
//
// Centralizes everything that differs across Turing implementations so
// driver code never has to branch on 'chip.implementation' at the call
// site. To add a new chip (TU102/TU104/TU106 someday, or Ampere later):
//   1) Add a 'static QUIRKS_<chip>: TuringQuirks' entry below.
//   2) Add it to 'for_chip()' selection.
//   3) Provide the chip-specific firmware blob references in
//      'firmware: FirmwarePaths'.
//
// What goes here:
//     PMC_ENABLE bit positions per secure engine (SEC2, GSP, ...)
//     Falcon engine base offsets if they ever diverge between chips
//     References to embedded firmware blobs the chip needs
//     Boolean feature flags (has_gsp_rm, needs_acr_wpr2, ...)
//
// What does NOT go here:
//     Anything you can derive at runtime from HWCFG / PTOP_DEVICE_INFO
//     Anything identical across all Turing chips (those live in
//     'falcon.rs' / 'regs.rs')

use crate::nvidia::chip::{Architecture, ChipId};

/// Bundle of firmware store paths a chip variant needs. Each field is
/// 'Option<&str>' (a path on the firmware store relative to its root, e.g.
/// "nvidia/tu116/acr/bl.bin") so a variant we have not yet wired up reads as
/// 'None' and the caller fails cleanly instead of booting with the wrong
/// blob. The bytes are fetched on demand via fwload::request()
#[derive(Copy, Clone)]
pub struct FirmwarePaths {
    pub acr_bl:       Option<&'static str>,
    pub acr_ahesasc:  Option<&'static str>,
    pub gsp_rm:       Option<&'static str>,
    pub gsp_booter_load:   Option<&'static str>,
    pub gsp_booter_unload: Option<&'static str>,
}

/// Compile-time quirks descriptor for one Turing implementation.
#[derive(Copy, Clone)]
pub struct TuringQuirks {
    /// Short name used in diagnostics
    pub codename: &'static str,
    /// PMC_BOOT_0 implementation field
    pub impl_id: u8,

    /// NV_PMC_ENABLE bit mask that gates the SEC2 Falcon. Toggle off/on
    /// to power-cycle the engine bypassing the engine-local priv mask
    pub sec2_pmc_reset_mask: u32,
    /// NV_PMC_ENABLE bit mask that gates the GSP Falcon
    pub gsp_pmc_reset_mask:  u32,

    /// True if this chip uses the WPR2-locking ACR boot path (Turing+)
    pub needs_acr_wpr2: bool,

    pub firmware: FirmwarePaths,
}

/// NV_PMC_ENABLE bits common across Turing (nouveau / open-gpu-kernel
/// modules)
mod pmc_bits {
    pub const SEC: u32 = 0x0000_4000; // bit 14 - SEC2 on Turing+
    pub const PWR: u32 = 0x0000_2000; // bit 13 - PMU/GSP gate on Turing
}

pub static QUIRKS_TU116: TuringQuirks = TuringQuirks {
    codename: "TU116",
    impl_id:  0x8,
    sec2_pmc_reset_mask: pmc_bits::SEC,
    gsp_pmc_reset_mask:  pmc_bits::PWR,
    needs_acr_wpr2: true,
    firmware: FirmwarePaths {
        acr_bl:       Some("nvidia/tu116/acr/bl.bin"),
        acr_ahesasc:  Some("nvidia/tu116/acr/ucode_ahesasc.bin"),
        gsp_rm:       None,
        gsp_booter_load:   None,
        gsp_booter_unload: None,
    },
};

pub static QUIRKS_TU117: TuringQuirks = TuringQuirks {
    codename: "TU117",
    impl_id:  0x7,
    sec2_pmc_reset_mask: pmc_bits::SEC,
    gsp_pmc_reset_mask:  pmc_bits::PWR,
    needs_acr_wpr2: true,
    // The HS images in 'tu116_fw' are signed for the entire Turing
    // family and work on TU117 as well; nouveau ships identical blobs
    // for both. A TU117-specific blob set would slot in here without
    // touching call sites
    firmware: FirmwarePaths {
        acr_bl:       Some("nvidia/tu116/acr/bl.bin"),
        acr_ahesasc:  Some("nvidia/tu116/acr/ucode_ahesasc.bin"),
        gsp_rm:       None,
        gsp_booter_load:   None,
        gsp_booter_unload: None,
    },
};

/// Resolve the quirks descriptor for a chip. Returns 'None' for any
/// non-Turing chip or for a Turing impl we have not characterized yet
pub fn for_chip(chip: ChipId) -> Option<&'static TuringQuirks> {
    if chip.arch != Architecture::Turing {
        return None;
    }
    match chip.implementation {
        0x8 => Some(&QUIRKS_TU116),
        0x7 => Some(&QUIRKS_TU117),
        _   => None,
    }
}

/// Detect chip variant from BAR0 and return the matching quirks.
/// Convenience wrapper combining ChipId::from_boot0 + for_chip
pub fn detect(bar0: &crate::nvidia::mmio::MmioRegion) -> Option<&'static TuringQuirks> {
    let boot0 = bar0.read32(crate::nvidia::chip::PMC_BOOT_0);
    let chip  = ChipId::from_boot0(boot0);
    for_chip(chip)
}
