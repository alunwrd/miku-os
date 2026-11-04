// CTA-861 Video Identification Codes 1-219, as timings
//
// Generated from `EIA861B[]` in OpenRM 570.144,
// `src/common/modeset/timing/nvt_edidext_861.c`: every row, in table order, so
// `VIC[n - 1]` is VIC n (`NvTiming_EnumCEA861bTiming`). Rows 128-192 are the
// table's placeholders for the VICs CTA-861 forbids, kept for the indices after
// them and marked `forbidden`. VICs 29 and 30 carry `'_'` for their vertical
// polarity in the original, which the macro reads as "not negative"; so does
// this.
//
// Regenerate rather than edit. The pixel clock is derived as `RRx1kToPclk` does
// (`nvt_util.c:344`).

pub struct Vic {
    pub h_visible: u16,
    pub h_front_porch: u16,
    pub h_sync_width: u16,
    pub h_total: u16,
    pub h_negative: bool,
    /// Per field when interlaced
    pub v_visible: u16,
    pub v_front_porch: u16,
    pub v_sync_width: u16,
    pub v_total: u16,
    pub v_negative: bool,
    /// Refresh rate x 1000
    pub rrx1k: u32,
    pub interlaced: bool,
    /// Pixel-repetition factors allowed, bit n = n + 1 times
    pub repetition: u16,
    /// One of the table's placeholders for VICs 128-192
    pub forbidden: bool,
}

#[allow(clippy::too_many_arguments)]
const fn v(
    h_visible: u16, h_front_porch: u16, h_sync_width: u16, h_total: u16, h_negative: bool,
    v_visible: u16, v_front_porch: u16, v_sync_width: u16, v_total: u16, v_negative: bool,
    rrx1k: u32, interlaced: bool, repetition: u16, forbidden: bool,
) -> Vic {
    Vic {
        h_visible, h_front_porch, h_sync_width, h_total, h_negative,
        v_visible, v_front_porch, v_sync_width, v_total, v_negative,
        rrx1k, interlaced, repetition, forbidden,
    }
}

pub static VIC: [Vic; 219] = [
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, false), // 1
    v(720, 16, 62, 858, true, 480, 9, 6, 525, true, 59940, false, 0x1, false), // 2
    v(720, 16, 62, 858, true, 480, 9, 6, 525, true, 59940, false, 0x1, false), // 3
    v(1280, 110, 40, 1650, false, 720, 5, 5, 750, false, 59940, false, 0x1, false), // 4
    v(1920, 88, 44, 2200, false, 540, 2, 5, 562, false, 59940, true, 0x1, false), // 5
    v(1440, 38, 124, 1716, true, 240, 4, 3, 262, true, 59940, true, 0x2, false), // 6
    v(1440, 38, 124, 1716, true, 240, 4, 3, 262, true, 59940, true, 0x2, false), // 7
    v(1440, 38, 124, 1716, true, 240, 4, 3, 263, true, 59940, false, 0x2, false), // 8
    v(1440, 38, 124, 1716, true, 240, 4, 3, 263, true, 59940, false, 0x2, false), // 9
    v(2880, 76, 248, 3432, true, 240, 4, 3, 262, true, 59940, true, 0x3ff, false), // 10
    v(2880, 76, 248, 3432, true, 240, 4, 3, 262, true, 59940, true, 0x3ff, false), // 11
    v(2880, 76, 248, 3432, true, 240, 5, 3, 263, true, 59940, false, 0x3ff, false), // 12
    v(2880, 76, 248, 3432, true, 240, 5, 3, 263, true, 59940, false, 0x3ff, false), // 13
    v(1440, 32, 124, 1716, true, 480, 9, 6, 525, true, 59940, false, 0x3, false), // 14
    v(1440, 32, 124, 1716, true, 480, 9, 6, 525, true, 59940, false, 0x3, false), // 15
    v(1920, 88, 44, 2200, false, 1080, 4, 5, 1125, false, 59940, false, 0x1, false), // 16
    v(720, 12, 64, 864, true, 576, 5, 5, 625, true, 50000, false, 0x1, false), // 17
    v(720, 12, 64, 864, true, 576, 5, 5, 625, true, 50000, false, 0x1, false), // 18
    v(1280, 440, 40, 1980, false, 720, 5, 5, 750, false, 50000, false, 0x1, false), // 19
    v(1920, 528, 44, 2640, false, 540, 2, 5, 562, false, 50000, true, 0x1, false), // 20
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 50000, true, 0x2, false), // 21
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 50000, true, 0x2, false), // 22
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 50000, false, 0x2, false), // 23
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 50000, false, 0x2, false), // 24
    v(2880, 48, 252, 3456, true, 288, 2, 3, 312, true, 50000, true, 0x3ff, false), // 25
    v(2880, 48, 252, 3456, true, 288, 2, 3, 312, true, 50000, true, 0x3ff, false), // 26
    v(2880, 48, 252, 3456, true, 288, 2, 3, 312, true, 50000, false, 0x3ff, false), // 27
    v(2880, 48, 252, 3456, true, 288, 2, 3, 312, true, 50000, false, 0x3ff, false), // 28
    v(1440, 24, 128, 1728, true, 576, 5, 5, 625, false, 50000, false, 0x3, false), // 29
    v(1440, 24, 128, 1728, true, 576, 5, 5, 625, false, 50000, false, 0x3, false), // 30
    v(1920, 528, 44, 2640, false, 1080, 4, 5, 1125, false, 50000, false, 0x1, false), // 31
    v(1920, 638, 44, 2750, false, 1080, 4, 5, 1125, false, 23976, false, 0x1, false), // 32
    v(1920, 528, 44, 2640, false, 1080, 4, 5, 1125, false, 25000, false, 0x1, false), // 33
    v(1920, 88, 44, 2200, false, 1080, 4, 5, 1125, false, 29970, false, 0x1, false), // 34
    v(2880, 64, 248, 3432, true, 480, 9, 6, 525, true, 59940, false, 0x7, false), // 35
    v(2880, 64, 248, 3432, true, 480, 9, 6, 525, true, 59940, false, 0x7, false), // 36
    v(2880, 48, 256, 3456, true, 576, 5, 5, 625, true, 50000, false, 0x7, false), // 37
    v(2880, 48, 256, 3456, true, 576, 5, 5, 625, true, 50000, false, 0x7, false), // 38
    v(1920, 32, 168, 2304, false, 540, 23, 5, 625, true, 50000, true, 0x1, false), // 39
    v(1920, 528, 44, 2640, false, 540, 2, 5, 562, false, 100000, true, 0x1, false), // 40
    v(1280, 440, 40, 1980, false, 720, 5, 5, 750, false, 100000, false, 0x1, false), // 41
    v(720, 12, 64, 864, true, 576, 5, 5, 625, true, 100000, false, 0x1, false), // 42
    v(720, 12, 64, 864, true, 576, 5, 5, 625, true, 100000, false, 0x1, false), // 43
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 100000, true, 0x2, false), // 44
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 100000, true, 0x2, false), // 45
    v(1920, 88, 44, 2200, false, 540, 2, 5, 562, false, 119880, true, 0x1, false), // 46
    v(1280, 110, 40, 1650, false, 720, 5, 5, 750, false, 119880, false, 0x1, false), // 47
    v(720, 16, 62, 858, true, 480, 9, 6, 525, true, 119880, false, 0x1, false), // 48
    v(720, 16, 62, 858, true, 480, 9, 6, 525, true, 119880, false, 0x1, false), // 49
    v(1440, 38, 124, 1716, true, 240, 4, 3, 262, true, 119880, true, 0x2, false), // 50
    v(1440, 38, 124, 1716, true, 240, 4, 3, 262, true, 119880, true, 0x2, false), // 51
    v(720, 12, 64, 864, true, 576, 5, 5, 625, true, 200000, false, 0x1, false), // 52
    v(720, 12, 64, 864, true, 576, 5, 5, 625, true, 200000, false, 0x1, false), // 53
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 200000, true, 0x2, false), // 54
    v(1440, 24, 126, 1728, true, 288, 2, 3, 312, true, 200000, true, 0x2, false), // 55
    v(720, 16, 62, 858, true, 480, 9, 6, 525, true, 239760, false, 0x1, false), // 56
    v(720, 16, 62, 858, true, 480, 9, 6, 525, true, 239760, false, 0x1, false), // 57
    v(1440, 38, 124, 1716, true, 240, 4, 3, 262, true, 239760, true, 0x2, false), // 58
    v(1440, 38, 124, 1716, true, 240, 4, 3, 262, true, 239760, true, 0x2, false), // 59
    v(1280, 1760, 40, 3300, false, 720, 5, 5, 750, false, 23976, false, 0x1, false), // 60
    v(1280, 2420, 40, 3960, false, 720, 5, 5, 750, false, 25000, false, 0x1, false), // 61
    v(1280, 1760, 40, 3300, true, 720, 5, 5, 750, false, 29970, false, 0x1, false), // 62
    v(1920, 88, 44, 2200, false, 1080, 4, 5, 1125, false, 119880, false, 0x1, false), // 63
    v(1920, 528, 44, 2640, false, 1080, 4, 5, 1125, false, 100000, false, 0x1, false), // 64
    v(1280, 1760, 40, 3300, false, 720, 5, 5, 750, false, 23976, false, 0x1, false), // 65
    v(1280, 2420, 40, 3960, false, 720, 5, 5, 750, false, 25000, false, 0x1, false), // 66
    v(1280, 1760, 40, 3300, false, 720, 5, 5, 750, false, 29970, false, 0x1, false), // 67
    v(1280, 440, 40, 1980, false, 720, 5, 5, 750, false, 50000, false, 0x1, false), // 68
    v(1280, 110, 40, 1650, false, 720, 5, 5, 750, false, 59940, false, 0x1, false), // 69
    v(1280, 440, 40, 1980, false, 720, 5, 5, 750, false, 100000, false, 0x1, false), // 70
    v(1280, 110, 40, 1650, false, 720, 5, 5, 750, false, 119880, false, 0x1, false), // 71
    v(1920, 638, 44, 2750, false, 1080, 4, 5, 1125, false, 23976, false, 0x1, false), // 72
    v(1920, 528, 44, 2640, false, 1080, 4, 5, 1125, false, 25000, false, 0x1, false), // 73
    v(1920, 88, 44, 2200, false, 1080, 4, 5, 1125, false, 29970, false, 0x1, false), // 74
    v(1920, 528, 44, 2640, false, 1080, 4, 5, 1125, false, 50000, false, 0x1, false), // 75
    v(1920, 88, 44, 2200, false, 1080, 4, 5, 1125, false, 59940, false, 0x1, false), // 76
    v(1920, 528, 44, 2640, false, 1080, 4, 5, 1125, false, 100000, false, 0x1, false), // 77
    v(1920, 88, 44, 2200, false, 1080, 4, 5, 1125, false, 119880, false, 0x1, false), // 78
    v(1680, 1360, 40, 3300, false, 720, 5, 5, 750, false, 23976, false, 0x1, false), // 79
    v(1680, 1228, 40, 3168, false, 720, 5, 5, 750, false, 25000, false, 0x1, false), // 80
    v(1680, 700, 40, 2640, false, 720, 5, 5, 750, false, 29970, false, 0x1, false), // 81
    v(1680, 260, 40, 2200, false, 720, 5, 5, 750, false, 50000, false, 0x1, false), // 82
    v(1680, 260, 40, 2200, false, 720, 5, 5, 750, false, 59940, false, 0x1, false), // 83
    v(1680, 60, 40, 2000, false, 720, 5, 5, 825, false, 100000, false, 0x1, false), // 84
    v(1680, 60, 40, 2000, false, 720, 5, 5, 825, false, 119880, false, 0x1, false), // 85
    v(2560, 998, 44, 3750, false, 1080, 4, 5, 1100, false, 23976, false, 0x1, false), // 86
    v(2560, 448, 44, 3200, false, 1080, 4, 5, 1125, false, 25000, false, 0x1, false), // 87
    v(2560, 768, 44, 3520, false, 1080, 4, 5, 1125, false, 29970, false, 0x1, false), // 88
    v(2560, 548, 44, 3300, false, 1080, 4, 5, 1125, false, 50000, false, 0x1, false), // 89
    v(2560, 248, 44, 3000, false, 1080, 4, 5, 1100, false, 59940, false, 0x1, false), // 90
    v(2560, 218, 44, 2970, false, 1080, 4, 5, 1250, false, 100000, false, 0x1, false), // 91
    v(2560, 548, 44, 3300, false, 1080, 4, 5, 1250, false, 119880, false, 0x1, false), // 92
    v(3840, 1276, 88, 5500, false, 2160, 8, 10, 2250, false, 23976, false, 0x1, false), // 93
    v(3840, 1056, 88, 5280, false, 2160, 8, 10, 2250, false, 25000, false, 0x1, false), // 94
    v(3840, 176, 88, 4400, false, 2160, 8, 10, 2250, false, 29970, false, 0x1, false), // 95
    v(3840, 1056, 88, 5280, false, 2160, 8, 10, 2250, false, 50000, false, 0x1, false), // 96
    v(3840, 176, 88, 4400, false, 2160, 8, 10, 2250, false, 59940, false, 0x1, false), // 97
    v(4096, 1020, 88, 5500, false, 2160, 8, 10, 2250, false, 23976, false, 0x1, false), // 98
    v(4096, 968, 88, 5280, false, 2160, 8, 10, 2250, false, 25000, false, 0x1, false), // 99
    v(4096, 88, 88, 4400, false, 2160, 8, 10, 2250, false, 29970, false, 0x1, false), // 100
    v(4096, 968, 88, 5280, false, 2160, 8, 10, 2250, false, 50000, false, 0x1, false), // 101
    v(4096, 88, 88, 4400, false, 2160, 8, 10, 2250, false, 59940, false, 0x1, false), // 102
    v(3840, 1276, 88, 5500, false, 2160, 8, 10, 2250, false, 23976, false, 0x1, false), // 103
    v(3840, 1056, 88, 5280, false, 2160, 8, 10, 2250, false, 25000, false, 0x1, false), // 104
    v(3840, 176, 88, 4400, false, 2160, 8, 10, 2250, false, 29970, false, 0x1, false), // 105
    v(3840, 1056, 88, 5280, false, 2160, 8, 10, 2250, false, 50000, false, 0x1, false), // 106
    v(3840, 176, 88, 4400, false, 2160, 8, 10, 2250, false, 59940, false, 0x1, false), // 107
    v(1280, 960, 40, 2500, false, 720, 5, 5, 750, false, 47950, false, 0x1, false), // 108
    v(1280, 960, 40, 2500, false, 720, 5, 5, 750, false, 47950, false, 0x1, false), // 109
    v(1680, 810, 40, 2750, false, 720, 5, 5, 750, false, 47950, false, 0x1, false), // 110
    v(1920, 638, 44, 2750, false, 1080, 4, 5, 1125, false, 47950, false, 0x1, false), // 111
    v(1920, 638, 44, 2750, false, 1080, 4, 5, 1125, false, 47950, false, 0x1, false), // 112
    v(2560, 998, 44, 3750, false, 1080, 4, 5, 1100, false, 47950, false, 0x1, false), // 113
    v(3840, 1276, 88, 5500, false, 2160, 8, 10, 2250, false, 47950, false, 0x1, false), // 114
    v(4096, 1020, 88, 5500, false, 2160, 8, 10, 2250, false, 47950, false, 0x1, false), // 115
    v(3840, 1276, 88, 5500, false, 2160, 8, 10, 2250, false, 47950, false, 0x1, false), // 116
    v(3840, 1056, 88, 5280, false, 2160, 8, 10, 2250, false, 100000, false, 0x1, false), // 117
    v(3840, 176, 88, 4400, false, 2160, 8, 10, 2250, false, 119880, false, 0x1, false), // 118
    v(3840, 1056, 88, 5280, false, 2160, 8, 10, 2250, false, 100000, false, 0x1, false), // 119
    v(3840, 176, 88, 4400, false, 2160, 8, 10, 2250, false, 119880, false, 0x1, false), // 120
    v(5120, 1996, 88, 7500, false, 2160, 8, 10, 2200, false, 23976, false, 0x1, false), // 121
    v(5120, 1696, 88, 7200, false, 2160, 8, 10, 2200, false, 25000, false, 0x1, false), // 122
    v(5120, 664, 88, 6000, false, 2160, 8, 10, 2200, false, 29970, false, 0x1, false), // 123
    v(5120, 746, 88, 6250, false, 2160, 8, 10, 2475, false, 47950, false, 0x1, false), // 124
    v(5120, 1096, 88, 6600, false, 2160, 8, 10, 2250, false, 50000, false, 0x1, false), // 125
    v(5120, 164, 88, 5500, false, 2160, 8, 10, 2250, false, 59940, false, 0x1, false), // 126
    v(5120, 1096, 88, 6600, false, 2160, 8, 10, 2250, false, 100000, false, 0x1, false), // 127
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 128
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 129
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 130
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 131
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 132
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 133
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 134
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 135
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 136
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 137
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 138
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 139
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 140
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 141
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 142
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 143
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 144
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 145
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 146
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 147
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 148
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 149
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 150
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 151
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 152
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 153
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 154
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 155
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 156
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 157
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 158
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 159
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 160
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 161
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 162
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 163
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 164
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 165
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 166
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 167
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 168
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 169
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 170
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 171
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 172
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 173
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 174
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 175
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 176
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 177
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 178
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 179
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 180
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 181
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 182
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 183
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 184
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 185
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 186
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 187
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 188
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 189
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 190
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 191
    v(640, 16, 96, 800, true, 480, 10, 2, 525, true, 59940, false, 0x1, true), // 192
    v(5120, 164, 88, 5500, false, 2160, 8, 10, 2250, false, 120000, false, 0x1, false), // 193
    v(7680, 2552, 176, 11000, false, 4320, 16, 20, 4500, false, 23976, false, 0x1, false), // 194
    v(7680, 2352, 176, 10800, false, 4320, 16, 20, 4400, false, 25000, false, 0x1, false), // 195
    v(7680, 552, 176, 9000, false, 4320, 16, 20, 4400, false, 29970, false, 0x1, false), // 196
    v(7680, 2552, 176, 11000, false, 4320, 16, 20, 4500, false, 47950, false, 0x1, false), // 197
    v(7680, 2352, 176, 10800, false, 4320, 16, 20, 4400, false, 50000, false, 0x1, false), // 198
    v(7680, 552, 176, 9000, false, 4320, 16, 20, 4400, false, 59940, false, 0x1, false), // 199
    v(7680, 2112, 176, 10560, false, 4320, 16, 20, 4500, false, 100000, false, 0x1, false), // 200
    v(7680, 352, 176, 8800, false, 4320, 16, 20, 4500, false, 119880, false, 0x1, false), // 201
    v(7680, 2552, 176, 11000, false, 4320, 16, 20, 4500, false, 23976, false, 0x1, false), // 202
    v(7680, 2352, 176, 10800, false, 4320, 16, 20, 4400, false, 25000, false, 0x1, false), // 203
    v(7680, 552, 176, 9000, false, 4320, 16, 20, 4400, false, 29970, false, 0x1, false), // 204
    v(7680, 2552, 176, 11000, false, 4320, 16, 20, 4500, false, 47950, false, 0x1, false), // 205
    v(7680, 2352, 176, 10800, false, 4320, 16, 20, 4400, false, 50000, false, 0x1, false), // 206
    v(7680, 552, 176, 9000, false, 4320, 16, 20, 4400, false, 59940, false, 0x1, false), // 207
    v(7680, 2112, 176, 10560, false, 4320, 16, 20, 4500, false, 100000, false, 0x1, false), // 208
    v(7680, 352, 176, 8800, false, 4320, 16, 20, 4500, false, 119880, false, 0x1, false), // 209
    v(10240, 1492, 176, 12500, false, 4320, 16, 20, 4950, false, 23976, false, 0x1, false), // 210
    v(10240, 2492, 176, 13500, false, 4320, 16, 20, 4400, false, 25000, false, 0x1, false), // 211
    v(10240, 288, 176, 11000, false, 4320, 16, 20, 4500, false, 29970, false, 0x1, false), // 212
    v(10240, 1492, 176, 12500, false, 4320, 16, 20, 4950, false, 47950, false, 0x1, false), // 213
    v(10240, 2492, 176, 13500, false, 4320, 16, 20, 4400, false, 50000, false, 0x1, false), // 214
    v(10240, 288, 176, 11000, false, 4320, 16, 20, 4500, false, 59940, false, 0x1, false), // 215
    v(10240, 2192, 176, 13200, false, 4320, 16, 20, 4500, false, 100000, false, 0x1, false), // 216
    v(10240, 288, 176, 11000, false, 4320, 16, 20, 4500, false, 119880, false, 0x1, false), // 217
    v(4096, 800, 88, 5280, false, 2160, 8, 10, 2250, false, 100000, false, 0x1, false), // 218
    v(4096, 88, 88, 4400, false, 2160, 8, 10, 2250, false, 119880, false, 0x1, false), // 219
];
