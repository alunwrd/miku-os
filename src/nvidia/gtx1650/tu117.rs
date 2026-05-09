//////////////////////////////////////////////////////////////////////////////////////
// TU117 chip constants and GTX 1650 PCI device-ID table                            //
//                                                                                  //
// Device IDs cross-referenced with nouveau (thx) and pci-ids.ucw.cz GTX 1650 Ti    //
// desktop and GTX 1650 Super use TU116 silicon with different device IDs;          //
// they are not included here and would live in a sibling module                    //
//////////////////////////////////////////////////////////////////////////////////////

pub const ARCH_TURING: u8 = 0x16;
pub const IMPL_TU117: u8 = 0x7;

/// PCI device IDs for GTX 1650 and related TU117 SKUs
pub const DEVICE_IDS: &[u16] = &[
    0x1F82, // GeForce GTX 1650 (Desktop)
    0x1F91, // GeForce GTX 1650 Mobile / Max-Q
    0x1F92, // GeForce GTX 1650 Mobile (alternate SKU)
    0x1F94, // GeForce GTX 1650 Mobile
    0x1F95, // GeForce GTX 1650 Ti Mobile (TU117)
    0x1F96, // GeForce GTX 1650 Mobile (Max-Q)
    0x1F97, // GeForce MX450 (TU117, sibling)
    0x1F98, // GeForce MX450 (TU117, sibling)
    0x1F99, // GeForce GTX 1650 Mobile
    0x1FB0, // Quadro T1000 Mobile (TU117)
    0x1FB1, // Quadro T600 Mobile
    0x1FB2, // Quadro T400 Mobile
    0x1FB8, // Quadro T2000 Mobile
    0x1FB9, // Quadro T1000 Mobile
    0x1FBB, // Quadro T500 Mobile
];

pub fn model_name(device_id: u16) -> &'static str {
    match device_id {
        0x1F82 => "GeForce GTX 1650",
        0x1F91 | 0x1F94 | 0x1F96 | 0x1F99 => "GeForce GTX 1650 (Mobile)",
        0x1F92 => "GeForce GTX 1650 (Mobile, SKU2)",
        0x1F95 => "GeForce GTX 1650 Ti (Mobile)",
        0x1F97 | 0x1F98 => "GeForce MX450",
        0x1FB0 | 0x1FB9 => "Quadro T1000 (Mobile)",
        0x1FB1 => "Quadro T600 (Mobile)",
        0x1FB2 => "Quadro T400 (Mobile)",
        0x1FB8 => "Quadro T2000 (Mobile)",
        0x1FBB => "Quadro T500 (Mobile)",
        _ => "TU117 (unknown SKU)",
    }
}

/// Expected BAR0 size for TU117 MMIO registers: 16 MiB
pub const EXPECTED_BAR0_SIZE: u64 = 16 * 1024 * 1024;

/// Minimum expected BAR1 (framebuffer aperture) for GTX 1650: 256 MiB
pub const EXPECTED_BAR1_MIN: u64 = 256 * 1024 * 1024;
