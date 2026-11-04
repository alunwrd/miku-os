// VESA DMT, and the EDID established-timing bits that name DMT modes
//
// Generated from OpenRM 570.144: `DMT[]` in
// `src/common/modeset/timing/nvt_dmt.c` (the `DMT_TIMING` rows, the table
// `NvTiming_CalcDMT` searches, skipping the reduced-blanking ones and the one
// row with a border) and `EDID_EST[]` in `nvt_edid.c`, whose position is the
// bit it answers for (`parseEdidEstablishedTiming`: byte 35 bit 7 first, byte
// 36 bit 0 last). Established rows with no pixel clock are timings OpenRM does
// not program, and the 640x480 @ 72 row has a border: both are `None`.
//
// Regenerate rather than edit. Columns: h_visible, h_front_porch, h_sync_width,
// h_total, h_sync_negative, v_visible, v_front_porch, v_sync_width, v_total,
// v_sync_negative, refresh, pixel clock in 10 kHz.

pub struct Dmt {
    pub h_visible: u16,
    pub h_front_porch: u16,
    pub h_sync_width: u16,
    pub h_total: u16,
    pub h_negative: bool,
    pub v_visible: u16,
    pub v_front_porch: u16,
    pub v_sync_width: u16,
    pub v_total: u16,
    pub v_negative: bool,
    pub refresh: u16,
    pub pclk_10khz: u32,
}

#[allow(clippy::too_many_arguments)]
const fn d(
    h_visible: u16, h_front_porch: u16, h_sync_width: u16, h_total: u16, h_negative: bool,
    v_visible: u16, v_front_porch: u16, v_sync_width: u16, v_total: u16, v_negative: bool,
    refresh: u16, pclk_10khz: u32,
) -> Dmt {
    Dmt {
        h_visible, h_front_porch, h_sync_width, h_total, h_negative,
        v_visible, v_front_porch, v_sync_width, v_total, v_negative,
        refresh, pclk_10khz,
    }
}

pub static DMT: [Dmt; 58] = [
    d(640, 32, 64, 832, false, 350, 32, 3, 445, true, 85, 3150), // DMT id 0x01
    d(640, 32, 64, 832, true, 400, 1, 3, 445, false, 85, 3150), // DMT id 0x02
    d(720, 36, 72, 936, true, 400, 1, 3, 446, false, 85, 3550), // DMT id 0x03
    d(640, 8, 96, 800, true, 480, 2, 2, 525, true, 60, 2518), // DMT id 0x04
    d(640, 16, 64, 840, true, 480, 1, 3, 500, true, 75, 3150), // DMT id 0x06
    d(640, 56, 56, 832, true, 480, 1, 3, 509, true, 85, 3600), // DMT id 0x07
    d(800, 24, 72, 1024, false, 600, 1, 2, 625, false, 56, 3600), // DMT id 0x08
    d(800, 40, 128, 1056, false, 600, 1, 4, 628, false, 60, 4000), // DMT id 0x09
    d(800, 56, 120, 1040, false, 600, 37, 6, 666, false, 72, 5000), // DMT id 0x0a
    d(800, 16, 80, 1056, false, 600, 1, 3, 625, false, 75, 4950), // DMT id 0x0b
    d(800, 32, 64, 1048, false, 600, 1, 3, 631, false, 85, 5625), // DMT id 0x0c
    d(848, 16, 112, 1088, false, 480, 6, 8, 517, false, 60, 3375), // DMT id 0x0e
    d(1024, 8, 176, 1264, false, 768, 0, 4, 817, false, 43, 4490), // DMT id 0x0f
    d(1024, 24, 136, 1344, true, 768, 3, 6, 806, true, 60, 6500), // DMT id 0x10
    d(1024, 24, 136, 1328, true, 768, 3, 6, 806, true, 70, 7500), // DMT id 0x11
    d(1024, 16, 96, 1312, false, 768, 1, 3, 800, false, 75, 7875), // DMT id 0x12
    d(1024, 48, 96, 1376, false, 768, 1, 3, 808, false, 85, 9450), // DMT id 0x13
    d(1152, 64, 128, 1600, false, 864, 1, 3, 900, false, 75, 10800), // DMT id 0x15
    d(1280, 64, 128, 1664, true, 768, 3, 7, 798, false, 60, 7950), // DMT id 0x17
    d(1280, 80, 128, 1696, true, 768, 3, 7, 805, false, 75, 10225), // DMT id 0x18
    d(1280, 80, 136, 1712, true, 768, 3, 7, 809, false, 85, 11750), // DMT id 0x19
    d(1280, 72, 128, 1680, true, 800, 3, 6, 831, false, 60, 8350), // DMT id 0x1c
    d(1280, 80, 128, 1696, true, 800, 3, 6, 838, false, 75, 10650), // DMT id 0x1d
    d(1280, 80, 136, 1712, true, 800, 3, 6, 843, false, 85, 12250), // DMT id 0x1e
    d(1280, 96, 112, 1800, false, 960, 1, 3, 1000, false, 60, 10800), // DMT id 0x20
    d(1280, 64, 160, 1728, false, 960, 1, 3, 1011, false, 85, 14850), // DMT id 0x21
    d(1280, 48, 112, 1688, false, 1024, 1, 3, 1066, false, 60, 10800), // DMT id 0x23
    d(1280, 16, 144, 1688, false, 1024, 1, 3, 1066, false, 75, 13500), // DMT id 0x24
    d(1280, 64, 160, 1728, false, 1024, 1, 3, 1072, false, 85, 15750), // DMT id 0x25
    d(1360, 64, 112, 1792, false, 768, 3, 6, 795, false, 60, 8550), // DMT id 0x27
    d(1400, 88, 144, 1864, true, 1050, 3, 4, 1089, false, 60, 12175), // DMT id 0x2a
    d(1400, 104, 144, 1896, true, 1050, 3, 4, 1099, false, 75, 15600), // DMT id 0x2b
    d(1400, 104, 152, 1912, true, 1050, 3, 4, 1105, false, 85, 17950), // DMT id 0x2c
    d(1440, 80, 152, 1904, true, 900, 3, 6, 934, false, 60, 10650), // DMT id 0x2f
    d(1440, 96, 152, 1936, true, 900, 3, 6, 942, false, 75, 13675), // DMT id 0x30
    d(1440, 104, 152, 1952, true, 900, 3, 6, 948, false, 85, 15700), // DMT id 0x31
    d(1600, 64, 192, 2160, false, 1200, 1, 3, 1250, false, 60, 16200), // DMT id 0x33
    d(1600, 64, 192, 2160, false, 1200, 1, 3, 1250, false, 65, 17550), // DMT id 0x34
    d(1600, 64, 192, 2160, false, 1200, 1, 3, 1250, false, 70, 18900), // DMT id 0x35
    d(1600, 64, 192, 2160, false, 1200, 1, 3, 1250, false, 75, 20250), // DMT id 0x36
    d(1600, 64, 192, 2160, false, 1200, 1, 3, 1250, false, 85, 22950), // DMT id 0x37
    d(1680, 104, 176, 2240, true, 1050, 3, 6, 1089, false, 60, 14625), // DMT id 0x3a
    d(1680, 120, 176, 2272, true, 1050, 3, 6, 1099, false, 75, 18700), // DMT id 0x3b
    d(1680, 128, 176, 2288, true, 1050, 3, 6, 1105, false, 85, 21475), // DMT id 0x3c
    d(1792, 128, 200, 2448, true, 1344, 1, 3, 1394, false, 60, 20475), // DMT id 0x3e
    d(1792, 96, 216, 2456, true, 1344, 1, 3, 1417, false, 75, 26100), // DMT id 0x3f
    d(1856, 96, 224, 2528, true, 1392, 1, 3, 1439, false, 60, 21825), // DMT id 0x41
    d(1856, 128, 224, 2560, true, 1392, 1, 3, 1500, false, 75, 28800), // DMT id 0x42
    d(1920, 136, 200, 2592, true, 1200, 3, 6, 1245, false, 60, 19325), // DMT id 0x45
    d(1920, 136, 208, 2608, true, 1200, 3, 6, 1255, false, 75, 24525), // DMT id 0x46
    d(1920, 144, 208, 2624, true, 1200, 3, 6, 1262, false, 85, 28125), // DMT id 0x47
    d(1920, 128, 208, 2600, true, 1440, 1, 3, 1500, false, 60, 23400), // DMT id 0x49
    d(1920, 144, 224, 2640, true, 1440, 1, 3, 1500, false, 75, 29700), // DMT id 0x4a
    d(2560, 192, 280, 3504, true, 1600, 3, 6, 1658, false, 60, 34850), // DMT id 0x4d
    d(2560, 208, 280, 3536, true, 1600, 3, 6, 1672, false, 75, 44325), // DMT id 0x4e
    d(2560, 208, 280, 3536, true, 1600, 3, 6, 1682, false, 85, 50525), // DMT id 0x4f
    d(1920, 88, 44, 2200, false, 1080, 4, 5, 1125, false, 60, 14850), // DMT id 0x52
    d(1280, 110, 40, 1650, false, 720, 5, 5, 750, false, 60, 7425), // DMT id 0x55
];

pub static ESTABLISHED: [Option<Dmt>; 16] = [
    None, // 720x400 @ 70
    None, // 720x400 @ 88
    Some(d(640, 16, 96, 800, true, 480, 10, 2, 525, true, 60, 2518)), // 640x480 @ 60
    None, // 640x480 @ 67
    None, // 640x480 @ 72
    Some(d(640, 16, 64, 840, true, 480, 1, 3, 500, true, 75, 3150)), // 640x480 @ 75
    Some(d(800, 24, 72, 1024, false, 600, 1, 2, 625, false, 56, 3600)), // 800x600 @ 56
    Some(d(800, 40, 128, 1056, false, 600, 1, 4, 628, false, 60, 4000)), // 800x600 @ 60
    Some(d(800, 56, 120, 1040, false, 600, 37, 6, 666, false, 72, 5000)), // 800x600 @ 72
    Some(d(800, 16, 80, 1056, false, 600, 1, 3, 625, false, 75, 4950)), // 800x600 @ 75
    None, // 832x624 @ 75
    None, // 1024x768 @ 87i
    Some(d(1024, 24, 136, 1344, true, 768, 3, 6, 806, true, 60, 6500)), // 1024x768 @ 60
    Some(d(1024, 24, 136, 1328, true, 768, 3, 6, 806, true, 70, 7500)), // 1024x768 @ 70
    Some(d(1024, 16, 96, 1312, false, 768, 1, 3, 800, false, 75, 7875)), // 1024x768 @ 75
    Some(d(1280, 16, 144, 1688, false, 1024, 1, 3, 1066, false, 75, 13500)), // 1280x1024 @ 75
];
