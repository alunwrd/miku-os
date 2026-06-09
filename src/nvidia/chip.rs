//////////////////////////////////////////////////////////////////////////////////////////////
//               NVIDIA chip identification via PMC_BOOT_0 (offset 0x000000)                //
//                                                                                          //
// PMC_BOOT_0 layout:                                                                       //
//   [31:28] architecture family (NV4/NV10/.../Turing=0x16, Ampere=0x17)                    //
//   [27:20] implementation (specific chip within the family)                               //
//   [19:16] major revision                                                                 //
//   [15:8]  minor revision                                                                 //
//   [7:0]   stepping                                                                       //
//                                                                                          //
// Architecture codes (high nibble of the arch byte):                                       //
//   Turing      = 0x16   Ampere   = 0x17   Hopper = 0x18   Ada Lovelace = 0x19             //
//   Blackwell   = 0x1A (consumer) / 0x1B (datacenter, GB100)                               //
//                                                                                          //
// Implementations seen in the wild (impl nibble):                                          //
//   Turing: TU102=0x2 TU104=0x4 TU106=0x6 TU117=0x7 TU116=0x8                              //
//   Ampere: GA100=0x0 GA102=0x2 GA103=0x3 GA104=0x4 GA106=0x6 GA107=0x7                    //
//   Ada:    AD102=0x2 AD103=0x3 AD104=0x4 AD106=0x6 AD107=0x7                              //
//                                                                                          //
// GTX 1650 is TU117 (primary variant) or TU116 (some GDDR6 refreshes)                      //
//////////////////////////////////////////////////////////////////////////////////////////////

pub const PMC_BOOT_0: u32 = 0x0000_0000;

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

    /// True for every architecture that ships a GSP (GPU System Processor)
    /// and therefore requires the signed-firmware offload path for anything
    /// past host-side bring-up. Turing is the first GSP generation; every
    /// later family keeps it
    pub fn has_gsp(&self) -> bool {
        matches!(
            self,
            Architecture::Turing
                | Architecture::Ampere
                | Architecture::Hopper
                | Architecture::AdaLovelace
                | Architecture::Blackwell
        )
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
    pub raw: u32,
    pub arch: Architecture,
    pub implementation: u8,
    pub major_rev: u8,
    pub minor_rev: u8,
    pub stepping: u8,
}

impl ChipId {
    pub fn from_boot0(raw: u32) -> Self {
        let arch_code = ((raw >> 24) & 0x1F) as u8;
        let arch = Architecture::from_code(arch_code);
        let implementation = ((raw >> 20) & 0xF) as u8;
        let major_rev = ((raw >> 16) & 0xF) as u8;
        let minor_rev = ((raw >> 8) & 0xFF) as u8;
        let stepping  = (raw & 0xFF) as u8;
        Self { raw, arch, implementation, major_rev, minor_rev, stepping }
    }

    /// Short codename such as "TU117" or "GA102". Covers Turing, Ampere and
    /// Ada Lovelace; unrecognized (family, impl) pairs return "unknown"
    pub fn codename(&self) -> &'static str {
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
