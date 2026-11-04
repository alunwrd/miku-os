// NVIDIA chip identification via PMC_BOOT_0 (offset 0x000000)
//
// PMC_BOOT_0 layout, from NV_PMC_BOOT_0_* in OpenRM's nv_ref.h:108-136:
//   [28:24] architecture (ARCHITECTURE_0)
//   [23:20] implementation (the chip within the architecture)
//   [8]     ARCHITECTURE_1, an extension of the architecture field, not decoded here
//   [7:4]   major revision
//   [3:0]   minor revision
//
// Architecture codes:
//   Turing      = 0x16   Ampere   = 0x17   Hopper = 0x18   Ada Lovelace = 0x19
//   Blackwell   = 0x1A (datacenter, GB100) / 0x1B (consumer, GB200)
//
// Implementations seen in the wild (impl nibble):
//   Turing: TU102=0x2 TU104=0x4 TU106=0x6 TU117=0x7 TU116=0x8
//   Ampere: GA100=0x0 GA102=0x2 GA103=0x3 GA104=0x4 GA106=0x6 GA107=0x7
//   Ada:    AD102=0x2 AD103=0x3 AD104=0x4 AD106=0x6 AD107=0x7
//
// GTX 1650 is TU117 (primary variant) or TU116 (some GDDR6 refreshes)

// The register this module decodes. One definition, in the host register map
pub use super::regs::PMC_BOOT_0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Architecture {
    Turing,
    Ampere,
    Hopper,
    AdaLovelace,
    Blackwell,
    Unknown(u8),
}

impl Architecture {
    /// Decode the architecture from the high nibble of the arch byte
    /// (PMC_BOOT_0 bits [28:24]). Unknown codes are preserved verbatim
    pub fn from_code(code: u8) -> Self {
        match code {
            0x16 => Architecture::Turing,
            0x17 => Architecture::Ampere,
            0x18 => Architecture::Hopper,
            0x19 => Architecture::AdaLovelace,
            0x1A | 0x1B => Architecture::Blackwell,
            other => Architecture::Unknown(other),
        }
    }

    /// Human-readable family name
    pub fn name(&self) -> &'static str {
        match self {
            Architecture::Turing      => "Turing",
            Architecture::Ampere      => "Ampere",
            Architecture::Hopper      => "Hopper",
            Architecture::AdaLovelace => "Ada Lovelace",
            Architecture::Blackwell   => "Blackwell",
            Architecture::Unknown(_)  => "unknown",
        }
    }

    /// True if the driver recognizes this family well enough to attempt a
    /// host-side bring-up (BAR map, chip-ID, MSI, VBIOS, PMC, thermal,
    /// Falcon liveness). Unknown families are still probed but flagged
    pub fn is_known(&self) -> bool {
        !matches!(self, Architecture::Unknown(_))
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ChipId {
    /// The register this was decoded from: BOOT_0 or BOOT_42, whichever
    /// constructor made it
    pub raw: u32,
    pub arch: Architecture,
    /// The architecture as a number, which is what tells GB10x from GB20x
    arch_code: u8,
    pub implementation: u8,
    pub major_rev: u8,
    pub minor_rev: u8,
}

impl ChipId {
    pub fn from_boot0(raw: u32) -> Self {
        let arch_code = ((raw >> 24) & 0x1F) as u8;
        let arch = Architecture::from_code(arch_code);
        let implementation = ((raw >> 20) & 0xF) as u8;
        let major_rev = ((raw >> 4) & 0xF) as u8;
        let minor_rev = (raw & 0xF) as u8;
        Self { raw, arch, arch_code, implementation, major_rev, minor_rev }
    }

    /// Decode PMC_BOOT_42 (0xa00): ARCHITECTURE 29:24, IMPLEMENTATION 23:20,
    /// MAJOR_REVISION 19:16, MINOR_REVISION 15:12 (`nv_ref.h:149-155`).
    ///
    /// OpenRM chooses a chip's HAL from this one (`gpu_mgr.c:1264`). BOOT_0
    /// gives the same architecture and implementation on every chip here
    pub fn from_boot42(raw: u32) -> Self {
        let arch_code = ((raw >> 24) & 0x3F) as u8;
        Self {
            raw,
            arch: Architecture::from_code(arch_code),
            arch_code,
            implementation: ((raw >> 20) & 0xF) as u8,
            major_rev: ((raw >> 16) & 0xF) as u8,
            minor_rev: ((raw >> 12) & 0xF) as u8,
        }
    }

    /// The architecture number. Needed to tell the two Blackwell lines apart:
    /// 0x1A is datacenter (GB10x), 0x1B is consumer (GB20x, RTX 50 series);
    /// both decode to Architecture::Blackwell
    pub fn arch_byte(&self) -> u8 {
        self.arch_code
    }

    /// Short codename such as "TU117" or "GB206". Covers Turing through
    /// Blackwell; unrecognized (family, impl) pairs return "unknown"
    pub fn codename(&self) -> &'static str {
        // Blackwell needs the raw arch byte: GB10x and GB20x share the enum
        if self.arch == Architecture::Blackwell {
            return match (self.arch_byte(), self.implementation) {
                (0x1A, 0x0) => "GB100",
                (0x1A, 0x2) => "GB102",
                (0x1B, 0x2) => "GB202", // RTX 5090
                (0x1B, 0x3) => "GB203", // RTX 5080 / 5070 Ti
                (0x1B, 0x5) => "GB205", // RTX 5070
                (0x1B, 0x6) => "GB206", // RTX 5060 Ti / 5060
                (0x1B, 0x7) => "GB207", // RTX 5050
                _ => "unknown",
            };
        }
        match (self.arch, self.implementation) {
            (Architecture::Turing, 0x2) => "TU102",
            (Architecture::Turing, 0x4) => "TU104",
            (Architecture::Turing, 0x6) => "TU106",
            (Architecture::Turing, 0x7) => "TU117",
            (Architecture::Turing, 0x8) => "TU116",
            (Architecture::Ampere, 0x0) => "GA100",
            (Architecture::Ampere, 0x2) => "GA102",
            (Architecture::Ampere, 0x3) => "GA103",
            (Architecture::Ampere, 0x4) => "GA104",
            (Architecture::Ampere, 0x6) => "GA106",
            (Architecture::Ampere, 0x7) => "GA107",
            (Architecture::Hopper, 0x0) => "GH100",
            (Architecture::AdaLovelace, 0x2) => "AD102",
            (Architecture::AdaLovelace, 0x3) => "AD103",
            (Architecture::AdaLovelace, 0x4) => "AD104",
            (Architecture::AdaLovelace, 0x6) => "AD106",
            (Architecture::AdaLovelace, 0x7) => "AD107",
            _ => "unknown",
        }
    }
}
