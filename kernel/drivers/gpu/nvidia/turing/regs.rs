// Turing-only registers, on top of the shared host map
//
// `nvidia/regs.rs` is re-exported below, so `use super::regs::*` sees one flat
// namespace. Here is the write-protect-region block: FWSEC/FRTS and the booter
// program it, at the TU102 addresses (identical on TU116/TU117).

pub use crate::drivers::gpu::nvidia::regs::*;

// NV_PFB_PRI_MMU_WPR* (Turing TU10x/TU11x): write-protect-region lock
// registers. FWSEC running the FRTS command sets WPR2, and the booter re-checks
// it. Addresses are the TU102 values from open-gpu-kernel-modules
// 'src/common/inc/swref/published/turing/tu102/dev_fb.h'
// (NV_PFB_PRI_MMU_WPR2_ADDR_LO = 0x001FA824), identical on TU116/TU117. WPR1
// follows the gh100 dev_fb.h layout (same MMU base, consecutive registers just
// below WPR2).
//
// Not the 0x100CXX MMU_CTRL block (0x100CE0 is LOCAL_MEMORY_RANGE), which does
// not reflect the WPR2 lock.
//
// Encoding: the _VAL field is bits [31:4] and stores 'byte_addr >> 12'
// (ALIGNMENT = 0xc), so the address is 'val_field << 12'; in raw terms '(reg &
// 0xFFFF_FFF0) << 8'. WPR2 is "locked" iff LO <= HI and LO != 0.
pub const PFB_PRI_MMU_WPR1_ADDR_LO:  u32 = 0x001F_A81C;
pub const PFB_PRI_MMU_WPR1_ADDR_HI:  u32 = 0x001F_A820;
pub const PFB_PRI_MMU_WPR2_ADDR_LO:  u32 = 0x001F_A824;
pub const PFB_PRI_MMU_WPR2_ADDR_HI:  u32 = 0x001F_A828;
// Diagnostic-only WPR access-control registers, printed by the nvidia debug
// cmd; not used by the FWSEC/booter path
pub const PFB_PRI_MMU_ALLOW_READ:    u32 = 0x0010_0CE4;
pub const PFB_PRI_MMU_ALLOW_WRITE:   u32 = 0x0010_0CE8;

/// Decode a raw WPR address register to a byte address (0 if unset). The _VAL
/// field [31:4] holds 'byte_addr >> 12', so the masked value shifts left by 8.
/// Verified against ogkm 'DRF_VAL(_PFB,_PRI_MMU_WPR2_ADDR_LO,_VAL)' with
/// 'expectedLoVal = frtsOffset >> 12'.
#[inline]
pub fn decode_wpr_addr(reg: u32) -> u64 {
    ((reg as u64) & 0xFFFF_FFF0) << 8
}
