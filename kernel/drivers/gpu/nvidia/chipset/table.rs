// OpenRM's chipset tables, the x86 part (`dev/gsp-foundation.md`, step 1.17)
//
// `chipsetInfo` (`arch/nvalloc/common/inc/nvcst.h:105`), `rootPortInfo` and
// `upstreamPortInfo` (`src/kernel/platform/chipset/chipset_info.c:54`, `:89`),
// with each setup function rewritten as the properties it sets. The rows are
// OpenRM's in OpenRM's order, the first row with a device on the bus winning,
// as in `clFindFHBAndGetChipsetInfoIndex`, and the `chipset` numbers are the
// `nvdevid.h` enum of 615, what GSP-RM is handed.
//
// Not here:
// - the rows for Tegra, ARM, POWER and RISC-V hosts;
// - the chipset-specific ECAM base registers the old Intel and SiS setup
//   functions read (`Intel_25XX`, `_29XX`, `_4003`, the Nehalem decoder...).
//   They set `PCIE_CONFIG_SKIP_MCFG_READ` as in OpenRM, but extended config
//   space comes from `MCFG` alone, which every machine with a Blackwell-capable
//   slot has (`mod.rs`, `config_accessible`).

use super::{Chipset, Prop};

/// One row of `chipsetInfo`
pub struct Row {
    pub vendor: u16,
    pub device: u16,
    /// `CS_*`
    pub chipset: u32,
    pub name: &'static str,
    pub setup: Option<fn(&mut Chipset)>,
}

/// One row of `rootPortInfo` or `upstreamPortInfo`
pub struct PortRow {
    pub vendor: u16,
    pub device: u16,
    pub setup: Option<fn(&mut Chipset)>,
}

/// `CS_UNKNOWN`: the row OpenRM's table ends with, "Unknown", no setup
pub const CS_UNKNOWN: u32 = 0;

pub static CHIPSETS: &[Row] = &[
    Row { vendor: 0x8086, device: 0x2580, chipset: 0x1001, name: "Grantsdale", setup: Some(intel_25xx) },  // CS_INTEL_2580
    Row { vendor: 0x8086, device: 0x2584, chipset: 0x1002, name: "Alderwood", setup: Some(intel_25xx) },  // CS_INTEL_2584
    Row { vendor: 0x8086, device: 0x2588, chipset: 0x1003, name: "Intel2588", setup: Some(intel_25xx) },  // CS_INTEL_2588
    Row { vendor: 0x8086, device: 0x2590, chipset: 0x1004, name: "Alviso", setup: Some(intel_25xx) },  // CS_INTEL_2590
    Row { vendor: 0x8086, device: 0x25c0, chipset: 0x1005, name: "Greencreek", setup: Some(intel_25e0) },  // CS_INTEL_25E0
    Row { vendor: 0x8086, device: 0x25e0, chipset: 0x1005, name: "Greencreek", setup: Some(intel_25e0) },  // CS_INTEL_25E0
    Row { vendor: 0x8086, device: 0x29b0, chipset: 0x1006, name: "IntelQ35", setup: Some(intel_29x0) },  // CS_INTEL_29X0
    Row { vendor: 0x8086, device: 0x29c0, chipset: 0x1006, name: "BearlakeB", setup: Some(intel_29x0) },  // CS_INTEL_29X0
    Row { vendor: 0x8086, device: 0x29d0, chipset: 0x1006, name: "IntelQ33", setup: Some(intel_29x0) },  // CS_INTEL_29X0
    Row { vendor: 0x8086, device: 0x29e0, chipset: 0x1007, name: "BearlakeX", setup: Some(intel_29e0) },  // CS_INTEL_29E0
    Row { vendor: 0x8086, device: 0x359e, chipset: 0x1008, name: "Tumwater", setup: Some(intel_359e) },  // CS_INTEL_359E
    Row { vendor: 0x8086, device: 0x4000, chipset: 0x1009, name: "Stoakley", setup: Some(intel_4000) },  // CS_INTEL_4000
    Row { vendor: 0x8086, device: 0x4003, chipset: 0x100a, name: "SkullTrail", setup: Some(intel_4003) },  // CS_INTEL_4003
    Row { vendor: 0x8086, device: 0x3400, chipset: 0x100b, name: "IntelX58", setup: Some(intel_3400) },  // CS_INTEL_3400
    Row { vendor: 0x8086, device: 0x3403, chipset: 0x100b, name: "IntelX58", setup: Some(intel_3400) },  // CS_INTEL_3400
    Row { vendor: 0x8086, device: 0x3405, chipset: 0x100b, name: "IntelX58", setup: Some(intel_3400) },  // CS_INTEL_3400
    Row { vendor: 0x8086, device: 0x3406, chipset: 0x100b, name: "Tylersburg", setup: Some(intel_3400) },  // CS_INTEL_3400
    Row { vendor: 0x8086, device: 0x2770, chipset: 0x100d, name: "Lakeport", setup: Some(intel_25xx) },  // CS_INTEL_2770
    Row { vendor: 0x8086, device: 0x2774, chipset: 0x100e, name: "Glenwood", setup: Some(intel_27xx) },  // CS_INTEL_2774
    Row { vendor: 0x8086, device: 0x277c, chipset: 0x100f, name: "Glenwood", setup: Some(intel_27xx) },  // CS_INTEL_277C
    Row { vendor: 0x8086, device: 0x2a40, chipset: 0x1010, name: "Montevina", setup: Some(intel_2a40) },  // CS_INTEL_2A40
    Row { vendor: 0x8086, device: 0x2e00, chipset: 0x1011, name: "Eaglelake", setup: Some(intel_2e00) },  // CS_INTEL_2E00
    Row { vendor: 0x8086, device: 0x2e10, chipset: 0x1011, name: "Eaglelake", setup: Some(intel_2e00) },  // CS_INTEL_2E00
    Row { vendor: 0x8086, device: 0x2e20, chipset: 0x1011, name: "Eaglelake", setup: Some(intel_2e00) },  // CS_INTEL_2E00
    Row { vendor: 0x8086, device: 0x2e30, chipset: 0x1011, name: "Eaglelake", setup: Some(intel_2e00) },  // CS_INTEL_2E00
    Row { vendor: 0x8086, device: 0x0044, chipset: 0x1012, name: "Arrandale/Auburndale", setup: Some(intel_0040) },  // CS_INTEL_0040
    Row { vendor: 0x8086, device: 0x0062, chipset: 0x1012, name: "Arrandale/Auburndale", setup: Some(intel_0040) },  // CS_INTEL_0040
    Row { vendor: 0x8086, device: 0xd130, chipset: 0x100c, name: "Clarksfield", setup: Some(intel_3b42) },  // CS_INTEL_3B42
    Row { vendor: 0x8086, device: 0xd132, chipset: 0x100c, name: "Clarksfield", setup: Some(intel_3b42) },  // CS_INTEL_3B42
    Row { vendor: 0x8086, device: 0x3b42, chipset: 0x100c, name: "P55/PM55/H57", setup: Some(intel_3b42) },  // CS_INTEL_3B42
    Row { vendor: 0x8086, device: 0x1c46, chipset: 0x1014, name: "IntelP67-CougarPoint", setup: Some(intel_1c46) },  // CS_INTEL_1C46
    Row { vendor: 0x8086, device: 0x1c4b, chipset: 0x1014, name: "HuronRiver-HM67", setup: Some(intel_1c4b) },  // CS_INTEL_1C46
    Row { vendor: 0x8086, device: 0x1c4f, chipset: 0x1014, name: "HuronRiver-QM67", setup: Some(intel_1c4b) },  // CS_INTEL_1C46
    Row { vendor: 0x8086, device: 0x1c49, chipset: 0x1015, name: "HuronRiver-HM65", setup: Some(intel_1c49) },  // CS_INTEL_1C49
    Row { vendor: 0x8086, device: 0x1c44, chipset: 0x1014, name: "IntelZ68", setup: Some(intel_1c46) },  // CS_INTEL_1C46
    Row { vendor: 0x8086, device: 0x1c10, chipset: 0x1013, name: "IntelP67", setup: Some(intel_1c10) },  // CS_INTEL_1C10
    Row { vendor: 0x8086, device: 0x1d40, chipset: 0x1016, name: "IntelX79", setup: Some(intel_1d40) },  // CS_INTEL_1D40
    Row { vendor: 0x8086, device: 0x1d41, chipset: 0x1016, name: "IntelX79", setup: Some(intel_1d40) },  // CS_INTEL_1D40
    Row { vendor: 0x8086, device: 0x1e10, chipset: 0x1018, name: "IntelZ75", setup: Some(intel_1e10) },  // CS_INTEL_1E10
    Row { vendor: 0x8086, device: 0x0150, chipset: 0x1018, name: "IntelZ77A-GD55", setup: Some(intel_1e10) },  // CS_INTEL_1E10
    Row { vendor: 0x8086, device: 0x0151, chipset: 0x1018, name: "IntelZ77A-GD55", setup: Some(intel_1e10) },  // CS_INTEL_1E10
    Row { vendor: 0x8086, device: 0x0100, chipset: 0x1018, name: "IntelZ77A-GD55", setup: Some(intel_1e10) },  // CS_INTEL_1E10
    Row { vendor: 0x8086, device: 0x8c4b, chipset: 0x1019, name: "SharkBay-HM87", setup: Some(intel_8c4b) },  // CS_INTEL_8C4B
    Row { vendor: 0x8086, device: 0x8c44, chipset: 0x1019, name: "SharkBay-Z87", setup: Some(intel_8c4b) },  // CS_INTEL_8C4B
    Row { vendor: 0x8086, device: 0x8c41, chipset: 0x1019, name: "SharkBay-H8x/P8x", setup: Some(intel_8c4b) },  // CS_INTEL_8C4B
    Row { vendor: 0x8086, device: 0x8c49, chipset: 0x1019, name: "SharkBay-HM86", setup: Some(intel_8c4b) },  // CS_INTEL_8C4B
    Row { vendor: 0x8086, device: 0x8c52, chipset: 0x1019, name: "SharkBay-E3", setup: Some(intel_8c4b) },  // CS_INTEL_8C4B
    Row { vendor: 0x8086, device: 0x8cc4, chipset: 0x101a, name: "IntelZ97", setup: Some(intel_8cc4) },  // CS_INTEL_8CC4
    Row { vendor: 0x8086, device: 0x8cc3, chipset: 0x101a, name: "IntelHM97", setup: Some(intel_8cc4) },  // CS_INTEL_8CC4
    Row { vendor: 0x8086, device: 0xa145, chipset: 0x101b, name: "IntelZ170", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0xa14e, chipset: 0x101b, name: "IntelHM170", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0xa150, chipset: 0x101b, name: "IntelHM170", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0xa149, chipset: 0x101b, name: "SkyLake C236", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0xa14a, chipset: 0x101b, name: "SkyLake C232", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0xa14d, chipset: 0x101b, name: "SkyLake-H", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0xa244, chipset: 0x101b, name: "SkyLake C620", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0xa1c8, chipset: 0x101b, name: "SkyLake C620", setup: Some(intel_a145) },  // CS_INTEL_A145
    Row { vendor: 0x8086, device: 0x8d47, chipset: 0x1017, name: "IntelX99", setup: Some(intel_8d47) },  // CS_INTEL_8D47
    Row { vendor: 0x8086, device: 0x8d44, chipset: 0x1017, name: "IntelC612", setup: Some(intel_8d44) },  // CS_INTEL_8D47
    Row { vendor: 0x8086, device: 0xa2c5, chipset: 0x101c, name: "IntelZ270", setup: Some(intel_a2c5) },  // CS_INTEL_A2C5
    Row { vendor: 0x8086, device: 0xa154, chipset: 0x101c, name: "IntelZ270", setup: Some(intel_a2c5) },  // CS_INTEL_A2C5
    Row { vendor: 0x8086, device: 0xa152, chipset: 0x101c, name: "IntelRX9S", setup: Some(intel_a2c5) },  // CS_INTEL_A2C5
    Row { vendor: 0x8086, device: 0xa242, chipset: 0x101d, name: "IntelC422", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa241, chipset: 0x101d, name: "IntelC422", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa243, chipset: 0x101d, name: "IntelC422", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa2d2, chipset: 0x101e, name: "IntelX299", setup: Some(intel_a2d2) },  // CS_INTEL_A2D2
    Row { vendor: 0x8086, device: 0xa2d3, chipset: 0x101d, name: "IntelC422", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa1c1, chipset: 0x101d, name: "IntelC621", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa1c2, chipset: 0x101d, name: "IntelC622", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa1c3, chipset: 0x101d, name: "IntelC624", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa1c4, chipset: 0x101d, name: "IntelC625", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa1c5, chipset: 0x101d, name: "IntelC626", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa1c6, chipset: 0x101d, name: "IntelC627", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa1c7, chipset: 0x101d, name: "IntelC628", setup: Some(intel_a242) },  // CS_INTEL_A242
    Row { vendor: 0x8086, device: 0xa2c9, chipset: 0x101f, name: "IntelZ370", setup: Some(intel_a2c9) },  // CS_INTEL_A2C9
    Row { vendor: 0x8086, device: 0xa310, chipset: 0x101f, name: "IntelZ370", setup: Some(intel_a2c9) },  // CS_INTEL_A2C9
    Row { vendor: 0x8086, device: 0xa30e, chipset: 0x101f, name: "IntelZ370", setup: Some(intel_a2c9) },  // CS_INTEL_A2C9
    Row { vendor: 0x8086, device: 0xa305, chipset: 0x101f, name: "IntelZ390", setup: Some(intel_a2c9) },  // CS_INTEL_A2C9
    Row { vendor: 0x8086, device: 0xa30d, chipset: 0x101f, name: "IntelH370", setup: Some(intel_a2c9) },  // CS_INTEL_A2C9
    Row { vendor: 0x8086, device: 0xa301, chipset: 0x1020, name: "Intel-CannonLake", setup: Some(intel_a301) },  // CS_INTEL_A301
    Row { vendor: 0x8086, device: 0x0685, chipset: 0x1021, name: "Intel-CometLake", setup: Some(intel_0685) },  // CS_INTEL_0685
    Row { vendor: 0x8086, device: 0xa1cb, chipset: 0x105a, name: "Intel-IceLake", setup: None },  // CS_INTEL_C620
    Row { vendor: 0x8086, device: 0x4381, chipset: 0x1022, name: "Intel-RocketLake", setup: Some(intel_4381) },  // CS_INTEL_4381
    Row { vendor: 0x8086, device: 0x4385, chipset: 0x1022, name: "Intel-RocketLake", setup: Some(intel_4381) },  // CS_INTEL_4381
    Row { vendor: 0x8086, device: 0x7a82, chipset: 0x1023, name: "Intel-AlderLake", setup: Some(intel_7a82) },  // CS_INTEL_7A82
    Row { vendor: 0x8086, device: 0x7a84, chipset: 0x1023, name: "Intel-AlderLake", setup: Some(intel_7a82) },  // CS_INTEL_7A82
    Row { vendor: 0x8086, device: 0x1b81, chipset: 0x1060, name: "Intel-SapphireRapids", setup: None },  // CS_INTEL_1B81
    Row { vendor: 0x8086, device: 0x7a8a, chipset: 0x1060, name: "Intel-SapphireRapids", setup: None },  // CS_INTEL_1B81
    Row { vendor: 0x8086, device: 0x18dc, chipset: 0x1061, name: "Intel-IceLake", setup: None },  // CS_INTEL_18DC
    Row { vendor: 0x8086, device: 0x7a04, chipset: 0x1062, name: "Intel-RaptorLake", setup: Some(intel_7a04) },  // CS_INTEL_7A04
    Row { vendor: 0x8086, device: 0x5795, chipset: 0x1063, name: "Intel-GraniteRapids", setup: None },  // CS_INTEL_5795
    Row { vendor: 0x8086, device: 0xa70d, chipset: 0x106c, name: "Intel-B660", setup: Some(intel_a70d) },  // CS_INTEL_B660
    Row { vendor: 0x8086, device: 0x7d06, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d1a, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d1b, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d1c, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d2a, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d2d, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d2e, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d2f, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d35, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d67, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x8086, device: 0x7d30, chipset: 0x1024, name: "Intel-Arrowlake", setup: Some(intel_7d06) },  // CS_INTEL_7D06
    Row { vendor: 0x1039, device: 0x0649, chipset: 0x1033, name: "649", setup: Some(sis_656) },  // CS_SIS_649
    Row { vendor: 0x1039, device: 0x0656, chipset: 0x1034, name: "656", setup: Some(sis_656) },  // CS_SIS_656
    Row { vendor: 0x1002, device: 0x5a31, chipset: 0x1035, name: "RS400", setup: Some(ati_rs400) },  // CS_ATI_RS400
    Row { vendor: 0x1002, device: 0x5a33, chipset: 0x1035, name: "RS400", setup: Some(ati_rs400) },  // CS_ATI_RS400
    Row { vendor: 0x1002, device: 0x5950, chipset: 0x1037, name: "RS480", setup: Some(ati_rs480) },  // CS_ATI_RS480
    Row { vendor: 0x1002, device: 0x5951, chipset: 0x1037, name: "RS480", setup: Some(ati_rs480) },  // CS_ATI_RS480
    Row { vendor: 0x1002, device: 0x5956, chipset: 0x103b, name: "FX790", setup: Some(amd_fx790) },  // CS_ATI_FX790
    Row { vendor: 0x1002, device: 0x5a11, chipset: 0x103f, name: "FX890", setup: Some(amd_fx890) },  // CS_ATI_FX890
    Row { vendor: 0x1002, device: 0x5a13, chipset: 0x103c, name: "RD850", setup: Some(ati_rd870) },  // CS_ATI_RD850
    Row { vendor: 0x1002, device: 0x5a12, chipset: 0x103d, name: "RD870", setup: Some(ati_rd870) },  // CS_ATI_RD870
    Row { vendor: 0x1002, device: 0x5a10, chipset: 0x103e, name: "RD890", setup: Some(ati_rd890) },  // CS_ATI_RD890
    Row { vendor: 0x1002, device: 0x5957, chipset: 0x1040, name: "RX780", setup: Some(ati_rx780) },  // CS_ATI_RX780
    Row { vendor: 0x1002, device: 0x5a14, chipset: 0x1041, name: "FX990/X990/970", setup: Some(ati_fx990) },  // CS_ATI_FX990
    Row { vendor: 0x1022, device: 0x9601, chipset: 0x1042, name: "GX890", setup: Some(amd_fx890) },  // CS_AMD_GX890
    Row { vendor: 0x1022, device: 0x9600, chipset: 0x1039, name: "RS780", setup: Some(amd_rs780) },  // CS_AMD_RS780
    Row { vendor: 0x1022, device: 0x790e, chipset: 0x1043, name: "X370/X399/X470/ TRX40/X570/WRX80", setup: Some(amd_x370) },  // CS_AMD_X370
    Row { vendor: 0x1022, device: 0x14d8, chipset: 0x106b, name: "AMD-Raphael", setup: Some(amd_14d8) },  // CS_AMD_RPH
    Row { vendor: 0x1106, device: 0x0308, chipset: 0x103a, name: "VT8369B", setup: None },  // CS_VIA_VT8369B
    Row { vendor: 0x1106, device: 0x0410, chipset: 0x1044, name: "VX900", setup: Some(via_vx900) },  // CS_VIA_VX900
    Row { vendor: 0x1b36, device: 0x0008, chipset: 0x1050, name: "QEMU Redhat", setup: Some(qemu) },  // CS_REDHAT_QEMU
    Row { vendor: 0x1d94, device: 0x790e, chipset: 0x105b, name: "Hygon-C86-7151", setup: None },  // CS_HYGON_C86
];

// The chipset setup functions
//
// Named after OpenRM's (`Intel_25XX_setupFunc` is `intel_25xx`). Rows share a
// function as in OpenRM

/// `_Set_ASPM_L0S_L1` (`chipset_info.c:1464`): the chipset is on the list
/// that decides ASPM, and maybe not capable of L0s or L1
fn aspm(c: &mut Chipset, disable_l0s: bool, disable_l1: bool) {
    c.set(Prop::IsChipsetInAspmPorList);
    if disable_l0s {
        c.set(Prop::AspmL0sChipsetDisabled);
    }
    if disable_l1 {
        c.set(Prop::AspmL1ChipsetDisabled);
    }
}

/// The old Intel chipsets carrying their ECAM base in a register of their own.
/// OpenRM reads it and skips `MCFG`; here `MCFG` is the only source, and the
/// property says what OpenRM's would
fn ecam_from_chipset(c: &mut Chipset) {
    c.set(Prop::PcieConfigSkipMcfgRead);
}

fn intel_25xx(c: &mut Chipset) { ecam_from_chipset(c); }
fn intel_25e0(c: &mut Chipset) { ecam_from_chipset(c); }
fn intel_27xx(c: &mut Chipset) { ecam_from_chipset(c); }
fn intel_359e(c: &mut Chipset) { ecam_from_chipset(c); }
fn intel_4000(c: &mut Chipset) { ecam_from_chipset(c); }
fn intel_4003(c: &mut Chipset) { ecam_from_chipset(c); }

/// `Intel_2A40` (Montevina): ASLM only past revision 5, and only where its
/// config space was reachable
fn intel_2a40(c: &mut Chipset) {
    ecam_from_chipset(c);
    if crate::drivers::bus::pci::has_ecam()
        && c.fhb.map(|f| f.revision >= 0x05).unwrap_or(false)
    {
        c.set(Prop::AslmSupportsNvLinkUpgrade);
    }
}

fn intel_2e00(c: &mut Chipset) {
    aspm(c, true, false);
    ecam_from_chipset(c);
}

fn intel_29x0(c: &mut Chipset) {
    ecam_from_chipset(c);
    aspm(c, true, true);
}

/// `Intel_29E0` (Bearlake X): an X38 is told apart from an X48 by bit 25 of
/// the host bridge's capability identifier, at 0xE0 + 8
fn intel_29e0(c: &mut Chipset) {
    ecam_from_chipset(c);
    if let Some(f) = c.fhb {
        let cap_id = crate::drivers::bus::pci::pci_read32(f.bus, f.dev, f.func, 0xE8);
        if cap_id & (1 << 25) == 0 {
            c.set(Prop::PcieGen1Gen2SwitchChipsetDisabledGeforce);
        }
    }
    aspm(c, true, false);
    c.set(Prop::PcieGen2AtLessThanX16Disabled);
}

/// `Intel_Core_Nehalem_Processor_setupFunc`: finds the ECAM base in the
/// QuickPath decoder. `MCFG` says the same
fn nehalem(_c: &mut Chipset) {}

fn intel_0040(c: &mut Chipset) {
    c.set(Prop::AslmSupportsGen2LinkUpgrade);
    nehalem(c);
}

fn intel_3400(c: &mut Chipset) {
    aspm(c, true, false);
    nehalem(c);
}

/// `Intel_3B42` (P55/H57): the H57 root port needs a workaround, and a
/// Clarkdale processor on an H55/H57 takes the ASPM list
fn intel_3b42(c: &mut Chipset) {
    use crate::drivers::bus::pci::pci_read32;
    let lpc = pci_read32(0, 31, 0, 0);
    let lpc_dev = (lpc >> 16) as u16;
    if lpc & 0xFFFF != 0xFFFF && lpc_dev == 0x3B08 {
        c.set(Prop::IntelCpuRootport1NeedsH57War);
    }
    if lpc & 0xFFFF != 0xFFFF && (lpc_dev == 0x3B06 || lpc_dev == 0x3B08) {
        let cpu = pci_read32(0, 0, 0, 0);
        if cpu & 0xFFFF == 0x8086 && (cpu >> 16) == 0x0040 {
            aspm(c, false, false);
        }
    }
    nehalem(c);
}

fn intel_huron_river(c: &mut Chipset) {
    c.set(Prop::AslmSupportsGen2LinkUpgrade);
    c.set(Prop::IsChipsetInAspmPorList);
    aspm(c, true, true);
    c.set(Prop::AspmL0sChipsetEnabledMobileOnly);
    c.set(Prop::AspmL1ChipsetEnabledMobileOnly);
}

fn intel_1c4b(c: &mut Chipset) { intel_huron_river(c); }
fn intel_1c49(c: &mut Chipset) { intel_huron_river(c); }
fn intel_1c10(_c: &mut Chipset) {}
fn intel_1c46(_c: &mut Chipset) {}

/// `Intel_1D40` (X79 Patsburg): Gen3 on Patsburg, allowed with an Ivy
/// Bridge-E host bridge
fn intel_1d40(c: &mut Chipset) {
    c.set(Prop::OnPcieGen3Patsburg);
    if c.fhb.map(|f| f.device == 0x0E00).unwrap_or(false) {
        c.set(Prop::AllowPcieGen3OnPatsburgWithIvbeCpu);
    }
    c.set(Prop::IsChipsetInAspmPorList);
    aspm(c, true, true);
    c.set(Prop::AspmL0sChipsetEnabledMobileOnly);
}

fn intel_8d47(c: &mut Chipset) {
    c.set(Prop::IsChipsetInAspmPorList);
    aspm(c, true, true);
    c.set(Prop::AspmL0sChipsetEnabledMobileOnly);
    c.set(Prop::Bug1681803WarDisableMscg);
}

fn intel_8d44(c: &mut Chipset) {
    c.set(Prop::IsChipsetInAspmPorList);
    aspm(c, true, true);
    c.set(Prop::AspmL0sChipsetEnabledMobileOnly);
}

/// `Intel_1E10` (Z75/Z77): Gen3 is off on two Gigabyte Sniper 3 boards
fn intel_1e10(c: &mut Chipset) {
    aspm(c, false, false);
    if let Some(id) = c.chipset_id {
        if id.subvendor == 0x1458 && (id.subdevice == 0x5000 || id.subdevice == 0x5001) {
            c.set(Prop::Bug1340801DisableGen3OnGigabyteSniper3);
        }
    }
}

fn intel_8c4b(c: &mut Chipset) {
    aspm(c, true, false);
    c.set(Prop::AspmL0sChipsetEnabledMobileOnly);
}

fn intel_8cc4(_c: &mut Chipset) {}
fn intel_a145(_c: &mut Chipset) {}
fn intel_a2c5(_c: &mut Chipset) {}
fn intel_a242(_c: &mut Chipset) {}
fn intel_a2d2(_c: &mut Chipset) {}
fn intel_a2c9(_c: &mut Chipset) {}
fn intel_a301(_c: &mut Chipset) {}
fn intel_0685(_c: &mut Chipset) {}

/// `Intel_4381` (Rocket Lake): resizable BAR has an issue, and on an ASUS
/// board the link speed needs a workaround
fn intel_4381(c: &mut Chipset) {
    c.set(Prop::HasResizableBarIssue);
    if c.chipset_id.map(|id| id.subvendor == 0x1043).unwrap_or(false) {
        c.set(Prop::Bug3751839GenSpeedWar);
    }
}

fn intel_7a82(c: &mut Chipset) { c.set(Prop::HasResizableBarIssue); }
fn intel_7a04(c: &mut Chipset) { c.set(Prop::HasResizableBarIssue); }
fn intel_a70d(c: &mut Chipset) { c.set(Prop::War4802761Enabled); }
/// `Intel_7D06`: Arrow Lake (Z890), 615 on
fn intel_7d06(c: &mut Chipset) { c.set(Prop::HasResizableBarIssue); }
fn amd_14d8(c: &mut Chipset) { c.set(Prop::War4802761Enabled); }

fn sis_656(c: &mut Chipset) { ecam_from_chipset(c); }

/// `ATI_RS400`: the revision is in bit 0 of the host bridge's 0x9C
fn ati_rs400(c: &mut Chipset) {
    if let Some(f) = c.fhb {
        let nbcfg = crate::drivers::bus::pci::pci_read32(f.bus, f.dev, f.func, 0x9C);
        c.chipset = if nbcfg & 1 != 0 { CS_ATI_RS400 } else { CS_ATI_RS400_A21 };
    }
}

/// `ATI_RS480`: A21 is revision 0
fn ati_rs480(c: &mut Chipset) {
    if let Some(f) = c.fhb {
        let rev = crate::drivers::bus::pci::pci_read32(f.bus, f.dev, f.func, 0x08);
        c.chipset = if rev & 0xFF != 0 { CS_ATI_RS480 } else { CS_ATI_RS480_A21 };
    }
}

const CS_ATI_RS400: u32 = 0x1034;
const CS_ATI_RS400_A21: u32 = 0x1035;
const CS_ATI_RS480: u32 = 0x1036;
const CS_ATI_RS480_A21: u32 = 0x1037;

fn amd_rs780(c: &mut Chipset) {
    if c.fhb.is_some() {
        aspm(c, false, true);
    }
}

fn amd_fx790(c: &mut Chipset) {
    if c.fhb.is_some() {
        aspm(c, true, false);
    }
}

fn ati_rd870(_c: &mut Chipset) {}
fn ati_rd890(c: &mut Chipset) { c.set(Prop::RelaxedOrderingNotCapable); }
fn ati_rx780(c: &mut Chipset) { c.set(Prop::RelaxedOrderingNotCapable); }
fn amd_fx890(c: &mut Chipset) { aspm(c, false, false); }

fn ati_fx990(c: &mut Chipset) {
    c.set(Prop::Bug999673P2pArbitrarySplitWar);
    aspm(c, false, false);
}

/// `AMD_X370`: X370, X399, X470, TRX40, X570, WRX80 by their FCH
fn amd_x370(c: &mut Chipset) {
    // Bug 5107271, 615 on
    c.set(Prop::WarAmd5107271);
    aspm(c, false, false);
}

fn via_vx900(c: &mut Chipset) { c.set(Prop::PcieGen1Gen2SwitchChipsetDisabled); }

fn qemu(c: &mut Chipset) { c.set(Prop::IsChipsetIoCoherent); }

// The port tables (`chipset_info.c:54`, `:89`; functions in
// `chipset_pcie.c:3171-3466`)

pub static ROOT_PORTS: &[PortRow] = &[
    PortRow { vendor: 0x1166, device: 0x0140, setup: Some(broadcom_ht2100) },
    PortRow { vendor: 0x1166, device: 0x0142, setup: Some(broadcom_ht2100) },
    PortRow { vendor: 0x1166, device: 0x0144, setup: Some(broadcom_ht2100) },
    PortRow { vendor: 0x8086, device: 0x2581, setup: Some(intel_rp25xx) },
    PortRow { vendor: 0x8086, device: 0x2585, setup: Some(intel_rp25xx) },
    PortRow { vendor: 0x8086, device: 0x2589, setup: Some(intel_rp25xx) },
    PortRow { vendor: 0x8086, device: 0x2591, setup: Some(intel_rp25xx) },
    PortRow { vendor: 0x8086, device: 0x3597, setup: None },
    PortRow { vendor: 0x8086, device: 0x2775, setup: Some(intel_rp25xx) },
    PortRow { vendor: 0x8086, device: 0x2771, setup: Some(intel_rp25xx) },
    PortRow { vendor: 0x8086, device: 0x8110, setup: Some(intel_rp81xx) },
    PortRow { vendor: 0x8086, device: 0x8112, setup: Some(intel_rp81xx) },
    PortRow { vendor: 0x8086, device: 0x8180, setup: Some(intel_rp81xx) },
    PortRow { vendor: 0x8086, device: 0x8181, setup: Some(intel_rp81xx) },
    PortRow { vendor: 0x8086, device: 0x8184, setup: Some(intel_rp81xx) },
    PortRow { vendor: 0x8086, device: 0x8185, setup: Some(intel_rp81xx) },
    PortRow { vendor: 0x8086, device: 0x3c02, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c03, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c04, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c05, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c06, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c07, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c08, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c09, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c0a, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x3c0b, setup: Some(intel_rp3c0x) },
    PortRow { vendor: 0x8086, device: 0x2f04, setup: Some(intel_rp2f0x) },
    PortRow { vendor: 0x8086, device: 0x2f08, setup: Some(intel_rp2f0x) },
    PortRow { vendor: 0x8086, device: 0x0c01, setup: Some(intel_rp0c0x) },
];

pub static UPSTREAM_PORTS: &[PortRow] = &[
    PortRow { vendor: 0x8086, device: 0x1901, setup: Some(intel_skylake) },
    PortRow { vendor: 0x8086, device: 0x9d18, setup: Some(intel_skylake_u_pch) },
    PortRow { vendor: 0x8086, device: 0xa117, setup: Some(intel_skylake_h_pch) },
    PortRow { vendor: 0x8086, device: 0xa118, setup: Some(intel_skylake_h_pch) },
    PortRow { vendor: 0x8086, device: 0x9c98, setup: Some(intel_broadwell) },
    PortRow { vendor: 0x8086, device: 0x9d10, setup: Some(intel_kabylake_y) },
    PortRow { vendor: 0x1022, device: 0x1483, setup: Some(amd_rp_l0s) },
    PortRow { vendor: 0x1022, device: 0x1480, setup: Some(amd_rp_l0s) },
    PortRow { vendor: 0x1022, device: 0x1630, setup: Some(amd_rp_l0s) },
];

fn broadcom_ht2100(c: &mut Chipset) { c.set(Prop::ExtendedTagFieldNotCapable); }

/// `Intel_RP25XX`: an AER capability the port does not advertise. Only the
/// pointer changes, which nothing here reads
fn intel_rp25xx(_c: &mut Chipset) {}

fn intel_rp81xx(c: &mut Chipset) {
    c.set(Prop::NosnoopNotCapable);
    c.set(Prop::RelaxedOrderingNotCapable);
}

fn intel_rp3c0x(c: &mut Chipset) { c.set(Prop::RootportNeedsNosnoopWar); }
fn intel_rp0c0x(c: &mut Chipset) { c.set(Prop::RootportNeedsNosnoopWar); }

/// `Intel_RP2F0X` (Haswell-E): the no-snoop workaround. The C0-stepping
/// detection after it sets a system property GSP-RM is not told
fn intel_rp2f0x(c: &mut Chipset) { c.set(Prop::RootportNeedsNosnoopWar); }

fn intel_broadwell(c: &mut Chipset) {
    c.set(Prop::IsChipsetInAspmPorList);
    match c.fhb.map(|f| f.device) {
        Some(0x1604) => {
            aspm(c, true, true);
            c.set(Prop::AspmL0sChipsetEnabledMobileOnly);
            c.set(Prop::AspmL1ChipsetEnabledMobileOnly);
        }
        Some(0x1614) => aspm(c, false, false),
        _ => {}
    }
}

fn intel_skylake(c: &mut Chipset) {
    c.set(Prop::IsChipsetInAspmPorList);
    c.set(Prop::RootportNeedsNosnoopWar);
    match c.fhb.map(|f| f.device) {
        Some(0x1904) | Some(0x5910) => {
            aspm(c, true, true);
            c.set(Prop::AspmL1ChipsetEnabledMobileOnly);
        }
        Some(0x191F) | Some(0x1910) => aspm(c, true, false),
        _ => {}
    }
}

fn intel_skylake_u_pch(c: &mut Chipset) {
    c.set(Prop::IsChipsetInAspmPorList);
    aspm(c, true, true);
    c.upstream_l1_por_supported = true;
    c.upstream_l1_por_mobile_only = true;
    c.set(Prop::RootportNeedsNosnoopWar);
}

fn intel_skylake_h_pch(c: &mut Chipset) {
    c.set(Prop::IsChipsetInAspmPorList);
    aspm(c, true, false);
    c.upstream_l1_por_supported = true;
    c.set(Prop::RootportNeedsNosnoopWar);
}

fn intel_kabylake_y(c: &mut Chipset) {
    c.set(Prop::IsChipsetInAspmPorList);
    aspm(c, true, true);
    c.set(Prop::AspmL1ChipsetEnabledMobileOnly);
    c.set(Prop::RootportNeedsNosnoopWar);
}

/// `AMD_RP1483`, `_1480`, `_1630` (Matisse/Castle Peak/Renoir-H): L0s off,
/// bug 200533783
fn amd_rp_l0s(c: &mut Chipset) { aspm(c, true, false); }

/// The row for a vendor/device pair, if OpenRM has one
pub fn row(vendor: u16, device: u16) -> Option<&'static Row> {
    CHIPSETS.iter().find(|r| r.vendor == vendor && r.device == device)
}

/// The port row for a vendor/device pair in `table`
pub fn port_row(table: &'static [PortRow], vendor: u16, device: u16) -> Option<&'static PortRow> {
    table.iter().find(|r| r.vendor == vendor && r.device == device)
}
