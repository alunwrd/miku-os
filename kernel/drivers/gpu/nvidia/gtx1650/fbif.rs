 // FBIF (Falcon Bus Interface) programming for TU116.
//
// Each standalone Falcon (SEC2, GSP, NVDEC) has an FBIF block which holds
// 8 context-DMA descriptors (TRANSCFG[0..7]). A descriptor tells the
// Falcon DMA engine where in memory the source/destination of a given
// 'ctxdma' index lives - local FB, coherent sysmem, or noncoherent
// sysmem - and whether the address it programs into DMATRFBASE is a
// physical or virtual address
//
// Confirmed via 'nvidia fbif-scan' against live TU116 silicon: FBIF lives
// at engine_base + 0x600 for SEC2 and GSP. POST defaults all 8 contexts
// to (LOCAL_FB, virtual, L2C_WR=1, WACHK0=1) = 0x110, except GSP context
// 2 which is (LOCAL_FB, physical) = 0x004 - GSP's direct-phys channel
//
// Bit layout source: envytools rnndb hw/falcon/fbif.xml. Stable across
// Maxwell..Ampere; re-verify if extending to a newer architecture
//
// Contract: this module never touches contexts other than the one the
// caller asks about. We do not reset FBIF or perturb GSP's ctx 2

#![allow(dead_code)]

use super::falcon::Engine;

/// FBIF block lives at this offset relative to a standalone Falcon's base
/// Confirmed against TU116 (SEC2 @ 0x840000, GSP @ 0x110000); FECS lives
/// inside PGRAPH and uses a different DMA path entirely
pub const FBIF_BASE_OFFSET: u32 = 0x600;

/// Number of context-DMA descriptors per Falcon
pub const FBIF_TRANSCFG_COUNT: u8 = 8;

/// Stride between consecutive TRANSCFG slots, in bytes
pub const FBIF_TRANSCFG_STRIDE: u32 = 4;

/// FBIF_CTL register, offset relative to the Falcon's MMIO base
/// Source: envytools rnndb hw/falcon/fbif.xml - offset 0x024 inside FBIF
pub const FBIF_CTL_OFFSET: u32 = FBIF_BASE_OFFSET + 0x024;

/// FBIF_CTL.ALLOW_PHYS_NO_CTX - when set, the engine accepts physical
/// DMA kicks without a bound instance block. Nouveau and OpenGPU set this
/// during init for host-driven ucode loads. Without it, a phys-mode
/// DMATRFCMD echoes back but the memif silently drops the fetch
pub const FBIF_CTL_ALLOW_PHYS_NO_CTX: u32 = 1 << 7;

// TRANSCFG bit layout
pub const TRANSCFG_TARGET_SHIFT: u32 = 0;
pub const TRANSCFG_TARGET_MASK:  u32 = 0x3;

pub const TRANSCFG_TARGET_LOCAL_FB:           u32 = 0;
pub const TRANSCFG_TARGET_COHERENT_SYSMEM:    u32 = 1;
pub const TRANSCFG_TARGET_NONCOHERENT_SYSMEM: u32 = 2;

pub const TRANSCFG_MEM_TYPE_PHYS:    u32 = 1 << 2;  // 0=virtual, 1=physical
pub const TRANSCFG_L2C_WR_EVICT:     u32 = 1 << 4;
pub const TRANSCFG_L2C_RD_EVICT:     u32 = 1 << 5;
pub const TRANSCFG_WACHK0:           u32 = 1 << 8;
pub const TRANSCFG_WACHK1:           u32 = 1 << 9;
pub const TRANSCFG_RACHK0:           u32 = 1 << 12;
pub const TRANSCFG_RACHK1:           u32 = 1 << 13;

/// Aperture selector for a context-DMA
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FbifTarget {
    /// Local frame buffer (VRAM). DMATRFBASE is a VRAM offset
    LocalFb,
    /// Coherent (cacheable) sysmem - DMA snoops CPU caches
    CoherentSysmem,
    /// Noncoherent sysmem - caller manages cache flushes
    NoncoherentSysmem,
}

/// Address-translation mode for a context-DMA
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FbifMemType {
    /// DMATRFBASE goes through the Falcon's MMU
    Virtual,
    /// DMATRFBASE is a raw physical address
    Physical,
}

/// Decoded TRANSCFG, for human-readable reporting
#[derive(Copy, Clone, Debug)]
pub struct DecodedTranscfg {
    pub raw:        u32,
    pub target:     FbifTarget,
    pub mem_type:   FbifMemType,
    pub l2c_wr:     bool,
    pub l2c_rd:     bool,
    pub wachk0:     bool,
    pub wachk1:     bool,
    pub rachk0:     bool,
    pub rachk1:     bool,
}

#[inline]
pub fn transcfg_offset(ctx: u8) -> u32 {
    FBIF_BASE_OFFSET + (ctx as u32) * FBIF_TRANSCFG_STRIDE
}

/// Read the raw TRANSCFG[ctx] value from the engine's FBIF window
pub fn read_transcfg_raw(eng: &Engine, ctx: u8) -> u32 {
    eng.read(transcfg_offset(ctx))
}

pub fn decode_transcfg(raw: u32) -> DecodedTranscfg {
    let target = match raw & TRANSCFG_TARGET_MASK {
        TRANSCFG_TARGET_LOCAL_FB           => FbifTarget::LocalFb,
        TRANSCFG_TARGET_COHERENT_SYSMEM    => FbifTarget::CoherentSysmem,
        TRANSCFG_TARGET_NONCOHERENT_SYSMEM => FbifTarget::NoncoherentSysmem,
        _ => FbifTarget::LocalFb, // value 3 reserved; treat as LOCAL_FB
    };
    let mem_type = if raw & TRANSCFG_MEM_TYPE_PHYS != 0 {
        FbifMemType::Physical
    } else {
        FbifMemType::Virtual
    };
    DecodedTranscfg {
        raw,
        target,
        mem_type,
        l2c_wr: raw & TRANSCFG_L2C_WR_EVICT != 0,
        l2c_rd: raw & TRANSCFG_L2C_RD_EVICT != 0,
        wachk0: raw & TRANSCFG_WACHK0 != 0,
        wachk1: raw & TRANSCFG_WACHK1 != 0,
        rachk0: raw & TRANSCFG_RACHK0 != 0,
        rachk1: raw & TRANSCFG_RACHK1 != 0,
    }
}

/// Program one TRANSCFG slot with the given target + memory-type, then
/// read back to flush the write across the FBIF PRI bus.
///
/// Caching / address-check bits are deliberately left at zero: for a
/// sysmem context they are meaningless (no L2 between us and PCIe), and
/// for a VRAM context the caller can OR them in if needed. Returns the
/// post-write value so the caller can verify the write took
pub fn program_transcfg(
    eng: &Engine,
    ctx: u8,
    target: FbifTarget,
    mem_type: FbifMemType,
) -> u32 {
    let target_bits = match target {
        FbifTarget::LocalFb           => TRANSCFG_TARGET_LOCAL_FB,
        FbifTarget::CoherentSysmem    => TRANSCFG_TARGET_COHERENT_SYSMEM,
        FbifTarget::NoncoherentSysmem => TRANSCFG_TARGET_NONCOHERENT_SYSMEM,
    };
    let mt_bits = match mem_type {
        FbifMemType::Virtual  => 0,
        FbifMemType::Physical => TRANSCFG_MEM_TYPE_PHYS,
    };
    let val = target_bits | mt_bits;
    eng.write(transcfg_offset(ctx), val);
    eng.read(transcfg_offset(ctx))
}
