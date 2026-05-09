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
// Turing: arch=0x16, implementations:                                                      //
//   TU102=0x2  TU104=0x4  TU106=0x6  TU116=0x8  TU117=0x7                                  //
//                                                                                          //
// GTX 1650 is TU117 (primary variant) or TU116 (some GDDR6 refreshes)                      //
//////////////////////////////////////////////////////////////////////////////////////////////

pub const PMC_BOOT_0: u32 = 0x0000_0000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Architecture {
    Turing,
    Ampere,
    AdaLovelace,
    Unknown(u8),
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
        let arch = match arch_code {
            0x16 => Architecture::Turing,
            0x17 => Architecture::Ampere,
            0x19 => Architecture::AdaLovelace,
            other => Architecture::Unknown(other),
        };
        let implementation = ((raw >> 20) & 0xF) as u8;
        let major_rev = ((raw >> 16) & 0xF) as u8;
        let minor_rev = ((raw >> 8) & 0xFF) as u8;
        let stepping  = (raw & 0xFF) as u8;
        Self { raw, arch, implementation, major_rev, minor_rev, stepping }
    }

    /// Short codename such as "TU117" or "GA102"
    pub fn codename(&self) -> &'static str {
        match (self.arch, self.implementation) {
            (Architecture::Turing, 0x2) => "TU102",
            (Architecture::Turing, 0x4) => "TU104",
            (Architecture::Turing, 0x6) => "TU106",
            (Architecture::Turing, 0x7) => "TU117",
            (Architecture::Turing, 0x8) => "TU116",
            (Architecture::Ampere, 0x0) => "GA100",
            (Architecture::Ampere, 0x2) => "GA102",
            (Architecture::Ampere, 0x4) => "GA104",
            (Architecture::Ampere, 0x6) => "GA106",
            (Architecture::Ampere, 0x7) => "GA107",
            _ => "unknown",
        }
    }
}
