//////////////////////////////////////////////////////////////////////////////////////
// TU116 chip constants and GTX 1650 / 1660 PCI device-ID table.                    //
//                                                                                  //
// TU116 is the Turing variant without RT and Tensor cores. It powers the           //
// GTX 1660, GTX 1660 Ti, GTX 1660 Super, GTX 1650 Super, and the late              //
// GDDR6 refresh of the desktop GTX 1650. From the host side TU116 looks            //
// nearly identical to TU117: same PMC layout, same 16 MiB BAR0, same               //
// PTIMER, same Falcon/GSP boot path. Only the device-ID range and the              //
// PMC_BOOT_0 implementation field (0x8 vs 0x7) differ.                             //
//                                                                                  //
// The user's card is a GTX 1650 with TU116 silicon (GDDR6 refresh, device          //
// id 0x2188). That SKU is the primary target for this module.                      //
//                                                                                  //
// Device IDs cross-referenced with nouveau and pci-ids.ucw.cz                      //
//////////////////////////////////////////////////////////////////////////////////////

pub const ARCH_TURING: u8 = 0x16;
pub const IMPL_TU116:  u8 = 0x8;

/// PCI device IDs for the TU116-based GTX 1650 / 1660 family
pub const DEVICE_IDS: &[u16] = &[
    0x2182, // GeForce GTX 1660 Ti
    0x2183, // GeForce GTX 1660 Ti (alt SKU)
    0x2184, // GeForce GTX 1660
    0x2187, // GeForce GTX 1650 SUPER
    0x2188, // GeForce GTX 1650 (TU116, GDDR6 refresh)
    0x2189, // GeForce GTX 1660 SUPER
    0x2191, // GeForce GTX 1660 Ti Mobile
    0x2192, // GeForce GTX 1650 Ti Mobile (TU116)
    0x21C4, // GeForce GTX 1660 Super (alt SKU)
    0x21D1, // GeForce GTX 1660 Ti Mobile (Max-Q)
];

pub fn model_name(device_id: u16) -> &'static str {
    match device_id {
        0x2182 | 0x2183 => "GeForce GTX 1660 Ti",
        0x2184          => "GeForce GTX 1660",
        0x2187          => "GeForce GTX 1650 SUPER",
        0x2188          => "GeForce GTX 1650 (TU116, GDDR6)",
        0x2189 | 0x21C4 => "GeForce GTX 1660 SUPER",
        0x2191 | 0x21D1 => "GeForce GTX 1660 Ti (Mobile)",
        0x2192          => "GeForce GTX 1650 Ti (Mobile, TU116)",
        _               => "TU116 (unknown SKU)",
    }
}

/// True if `device_id` is one of the TU116 SKUs we recognise.
pub fn matches(device_id: u16) -> bool {
    DEVICE_IDS.iter().any(|&id| id == device_id)
}

/// BAR0 MMIO size for TU116 - 16 MiB, identical to TU117.
pub const EXPECTED_BAR0_SIZE: u64 = 16 * 1024 * 1024;

/// Minimum BAR1 (framebuffer aperture) for the TU116 cards in this list.
/// GTX 1650 GDDR6 ships with 4 GiB of VRAM, but the firmware-assigned
/// aperture can still be a 256 MiB window when ReBAR is off.
pub const EXPECTED_BAR1_MIN: u64 = 256 * 1024 * 1024;
