// NVIDIA host register map, shared by every GSP-era chip
//
// These offsets are stable across the NV50..Blackwell range for PMC / PBUS /
// PTIMER / PFIFO / PTOP / PFB. A few blocks change between generations and live
// with their architecture: the write-protect-region registers are in
// `turing/regs.rs`, the FSP and display blocks in `blackwell/regs.rs`.
//
// Primary sources: nouveau (drivers/gpu/drm/nouveau) and envytools rnndb

// PMC (Master Controller) - 0x00000000..0x00001000
// A register map is complete, including names nothing reads yet, so dead_code
// stays off for this file (see `mod.rs`).
#![allow(dead_code)]

pub const PMC_BOOT_0:             u32 = 0x0000_0000;  // chip ID
pub const PMC_BOOT_1:             u32 = 0x0000_0004;  // usually 0
pub const PMC_BOOT_2:             u32 = 0x0000_0008;  // straps
pub const PMC_INTR_0:             u32 = 0x0000_0100;  // pending interrupt bits
pub const PMC_INTR_EN_0:          u32 = 0x0000_0140;  // enable mask
pub const PMC_ENABLE:             u32 = 0x0000_0200;  // engine enables
pub const PMC_DEBUG_1:            u32 = 0x0000_020C;
pub const PMC_BOOT_42:            u32 = 0x0000_0A00;  // extended chip ID (Turing+)

// PBUS - 0x00001000..0x00002000
// Note: the real floorsweep information on Turing lives under PFUSE
// (0x00021000+), not PBUS
pub const PBUS_INTR_0:            u32 = 0x0000_1100;
pub const PBUS_INTR_EN_0:         u32 = 0x0000_1140;

// PTIMER - 0x00009000..0x0000A000
pub const PTIMER_TIME_0:          u32 = 0x0000_9400;  // low 32 bits (ns)
pub const PTIMER_TIME_1:          u32 = 0x0000_9410;  // high 32 bits (ns)
pub const PTIMER_NUMERATOR:       u32 = 0x0000_9200;
pub const PTIMER_DENOMINATOR:     u32 = 0x0000_9210;

// PFB (Framebuffer Controller) - 0x00100000+
pub const PFB_PRI_MMU_CTRL:       u32 = 0x0010_0CC0;
pub const PFB_FB_MMU_CTRL:        u32 = 0x0010_FC20;

// NV_PFB_PRI_MMU_LOCAL_MEMORY_RANGE: the local (on-board) VRAM size. Layout
// (from nouveau gp100_fb_oneinit / envytools): bits [3:0] = scale, bits [9:4] =
// magnitude; size in bytes = magnitude << (scale+20). Bit 16 clear means ECC
// reservation is active and trims usable VRAM by 1/16. Best-effort; cross-check
// against BAR1 size
pub const PFB_LOCAL_MEMORY_RANGE:        u32 = 0x0010_0CE0;
pub const PFB_LMR_SCALE_MASK:            u32 = 0x0000_000F;
pub const PFB_LMR_MAG_SHIFT:             u32 = 4;
pub const PFB_LMR_MAG_MASK:              u32 = 0x0000_003F;
pub const PFB_LMR_ECC_RESERVED:          u32 = 1 << 16;

// PFIFO (GP FIFO / host channel dispatcher) - 0x00002000+
pub const PFIFO_INTR_0:           u32 = 0x0000_2100;
pub const PFIFO_INTR_EN_0:        u32 = 0x0000_2140;

// PTOP (topology info)
pub const PTOP_DEVICE_INFO:       u32 = 0x0000_2600;  // array, 64 entries on Turing
pub const PTOP_DEVICE_INFO_COUNT: u32 = 64;

// PTHERM (on-die thermal sensor) - 0x00020000..0x00021000
// The internal temperature sensor on Pascal+ (and unchanged on Turing) is
// read from a single fixed-point register. Bit [29] = value valid,
// bit [30] = sensor was SHADOWed (stale); the value field is bits [16:3]
// which give integer degrees Celsius once shifted right by 8
pub const PTHERM_TEMP_SENSOR:      u32 = 0x0002_0460;
pub const PTHERM_TEMP_VALID:       u32 = 1 << 29;
pub const PTHERM_TEMP_SHADOWED:    u32 = 1 << 30;
pub const PTHERM_TEMP_VALUE_MASK:  u32 = 0x0001_FFF8;
pub const PTHERM_TEMP_VALUE_SHIFT: u32 = 8;
// Software-readable slowdown / shutdown thresholds programmed by VBIOS devinit.
// Zero until devinit runs
pub const PTHERM_THRS_SLOWDOWN:    u32 = 0x0002_0480;
pub const PTHERM_THRS_SHUTDOWN:    u32 = 0x0002_0484;

// Falcon register offsets are relative to an engine base, so they live in
// `nvidia/falcon.rs`.

// PMC_ENABLE bits (subset)
pub const PMC_ENABLE_HOST:        u32 = 1 << 8;
pub const PMC_ENABLE_GR:          u32 = 1 << 12;   // graphics
pub const PMC_ENABLE_PWR:         u32 = 1 << 13;   // PMU
pub const PMC_ENABLE_CE0:         u32 = 1 << 14;   // copy engine 0
pub const PMC_ENABLE_DISP:        u32 = 1 << 30;   // display

// PMC_INTR bits (subset)
pub const PMC_INTR_PFIFO:         u32 = 1 << 8;
pub const PMC_INTR_PGRAPH:        u32 = 1 << 12;
pub const PMC_INTR_PTIMER:        u32 = 1 << 20;
pub const PMC_INTR_PBUS:          u32 = 1 << 28;

//      PTOP_DEVICE_INFO entry parser constants
// Each entry is a single u32 on Turing (NV_PTOP_DEVICE_INFO2 exists too but is
// more complex). The legacy single-word form relies on the "chain" bit (bit 31)
// and the engine ID / runlist ID fields
pub const PTOP_ENTRY_CHAIN:       u32 = 1 << 31;
pub const PTOP_ENTRY_TYPE_SHIFT:  u32 = 24;
pub const PTOP_ENTRY_TYPE_MASK:   u32 = 0xF;
pub const PTOP_ENTRY_TYPE_ENUM:   u32 = 2;
pub const PTOP_ENTRY_TYPE_DATA:   u32 = 1;
