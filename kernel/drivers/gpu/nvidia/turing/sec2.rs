// SEC2 ACR first-contact boot for TU116
//
// Engage the SEC2 Falcon with the signed ACR bootloader ('acr/bl.bin'), hand it
// the 'ucode_ahesasc' HS image staged in sysmem, observe the result.
//
// On a clean POST, NVIDIA's host driver:
//     1. allocates a sysmem buffer, copies 'ucode_ahesasc' into it,
//     2. parses the HS image manifest (offset/size of code, data, sig),
//     3. configures FBIF ctxdma 0 to point at that sysmem buffer,
//     4. loads 'acr/bl' into SEC2 IMEM (SECURE),
//     5. preloads mailbox/DMEM scratch entries telling 'bl' the sysmem layout
//        of ahesasc + how big WPR2 should be,
//     6. kicks CPUCTL, polls halt.
// Here (1)-(4) and a minimal (5): the ahesasc phys address in
// MAILBOX0/MAILBOX1. Without the full DMEM scratch layout 'bl' halts early,
// when its sig check passes with an unknown command word or the manifest
// pointer in DMEM is zero. The halt status (MAILBOX1) is the result.
//
// Safety:
//   - Halt poll is timeout-bounded; on timeout SEC2 is soft-reset
//   - PMC top-level interrupts are masked first
//   - PMC_ENABLE / NV_PMC_DEVICE_ENABLE are not written: VBIOS devinit enables
//     SEC2 on Turing, and it reads alive post-POST. A failed liveness check
//     aborts with EngineGated
//
// To lock WPR2:
//   - parse ahesasc's HS-bin header and write its code/data/manifest offsets
//     into SEC2 DMEM scratch where 'bl' expects them
//   - allocate the WPR2 sysmem staging area for GSP-RM image + meta and tell
//     ahesasc where it lives
//   - read PFB WPR2 lock-status registers before/after

#![allow(dead_code)]

use crate::drivers::gpu::nvidia::le;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::serial_println;

use crate::mm::dma::{DmaBuffer, DmaBufError};
use crate::drivers::gpu::nvidia::falcon::{
    self, Engine, CPUCTL_HALTED, FALCON_CPUCTL, FALCON_DMACTL, FALCON_DMATRFBASE,
    FALCON_DMATRFBASE1, FALCON_DMATRFCMD, FALCON_MAILBOX0, FALCON_MAILBOX1, DMACTL_REQUIRE_CTX, wait_scrub_done, pages_for,
};

/// Falcon exception-cause register (offset relative to engine base). Source:
/// envytools rnndb 'hw/falcon/falcon.xml' ("FALCON_EXCI"). Set by hardware when
/// an exception (page fault, illegal insn, HSCB signature fail, etc.) caused
/// the halt. Reading is non-destructive
pub const FALCON_EXCI: u32 = 0x0148;
use crate::drivers::gpu::nvidia::fbif::{self, FbifMemType, FbifTarget};
use crate::drivers::gpu::nvidia::nvfw_hs::{NvfwHsHeader, NvfwHsLoadHeader};
use super::tu116_fw::{self, NvfwBinHdr};

/// FALCON_RM (offset 0x84) reset request bit
const FALCON_RM_RESET: u32 = 1 << 1;

/// Halt-poll timeout (wall-clock, via PTIMER). With a real bl_dmem_desc the bl
/// DMAs ~22 KB from sysmem, verifies the HSCB sig and programs WPR2 registers,
/// well under a millisecond. 300 ms is far past that, and a boot that never
/// halts costs a fraction of a second of MMIO reads
const ACR_HALT_TIMEOUT_NS: u64 = 300_000_000;



/// FBIF context-DMA slot for the ahesasc payload. ctx 0 is the conventional
/// choice for boot-time sysmem on NVIDIA's stack
const ACR_CTXDMA: u8 = 0;

#[derive(Debug, Copy, Clone)]
pub enum Sec2Error {
    /// SEC2 HWCFG reads as 0, all-ones, or a PRI sentinel: gated by PMC /
    /// device-enable / floorsweep. No ungating
    EngineGated,
    /// 'acr/bl.bin' did not parse as a valid NVFW container
    BadBlHeader,
    /// 'acr/ucode_ahesasc.bin' did not parse as a valid NVFW container
    BadAhesascHeader,
    /// 'bl' payload exceeds SEC2 IMEM capacity
    BlTooLarge { payload: u32, imem: u32 },
    /// Could not allocate sysmem for the ahesasc payload
    Alloc(DmaBufError),
    /// CPU did not assert HALTED within the timeout. The engine was soft-reset;
    /// status registers mean nothing
    Timeout,
}

impl From<DmaBufError> for Sec2Error {
    fn from(e: DmaBufError) -> Self { Sec2Error::Alloc(e) }
}

/// Errors specific to parsing the ahesasc HS image layers
#[derive(Debug, Copy, Clone)]
pub enum HsParseError {
    /// Outer NVFW container header (magic 0x000010de) did not parse
    BadNvfwHdr,
    /// Middle HS header at 'bin_hdr.header_offset' did not parse
    BadHsHdr,
    /// Inner HS load-header at 'hs_hdr.hdr_offset' did not parse, or 'num_apps'
    /// exceeded our cap
    BadLoadHdr,
    /// Production signature region did not have exactly 16 bytes (1 sig) at a
    /// readable position
    NoProdSig,
}

/// Parsed view over a Falcon HS-bin's three-layer header
#[derive(Clone, Debug)]
pub struct HsLayers {
    pub nvfw: NvfwBinHdr,
    pub hs:   NvfwHsHeader,
    pub load: NvfwHsLoadHeader,
    /// 16 bytes of production signature from
    /// 'blob[hs.sig_prod_offset..][..16]'. The Falcon's HSCB consumes these at
    /// boot after the descriptor places them at the patch location
    pub sig_prod: [u8; 16],
}

/// Parse the three nested headers of any HS-bin blob (NVFW container -> HS
/// header -> HS load header) and copy out the 16-byte production signature. For
/// ACR ahesasc, GSP booter_load, booter_unload
pub fn parse_hs_layers(blob: &[u8]) -> Result<HsLayers, HsParseError> {
    let nvfw = NvfwBinHdr::parse(blob).ok_or(HsParseError::BadNvfwHdr)?;
    let hs = NvfwHsHeader::parse(blob, nvfw.header_offset as usize)
        .ok_or(HsParseError::BadHsHdr)?;
    if !hs.looks_valid(nvfw.header_offset, nvfw.data_offset) {
        return Err(HsParseError::BadHsHdr);
    }
    let load = NvfwHsLoadHeader::parse(blob, hs.hdr_offset as usize, hs.hdr_size)
        .ok_or(HsParseError::BadLoadHdr)?;

    // Production signatures are stored as 16-byte chunks; NVIDIA ships a single
    // prod sig per HS image, the first
    if hs.sig_prod_size < 16 {
        return Err(HsParseError::NoProdSig);
    }
    let sig_off = hs.sig_prod_offset as usize;
    if sig_off.checked_add(16).map(|e| e > blob.len()).unwrap_or(true) {
        return Err(HsParseError::NoProdSig);
    }
    let mut sig_prod = [0u8; 16];
    sig_prod.copy_from_slice(&blob[sig_off..sig_off + 16]);

    Ok(HsLayers { nvfw, hs, load, sig_prod })
}

/// Wrapper for the ACR ahesasc blob
#[inline]
pub fn parse_ahesasc_layers() -> Result<HsLayers, HsParseError> {
    let fw = tu116_fw::acr_ahesasc();
    parse_hs_layers(fw.bytes())
}

/// LS-style bootloader descriptor for 'acr/bl.bin'
///
/// Layout (nouveau 'include/nvfw/acr.h' 'hsflcn_bl_desc', 6 u32 = 24 bytes).
/// 'bl.bin' is NVFW-wrapped, but its 'header_offset' region carries this
/// descriptor, not an HS header:
///   - which IMEM virtual tag the bl code expects to run at
///   - where in DMEM to place the 'FlcnBlDmemDesc'
///   - which slice of the payload is bl CODE vs bl DATA
///
/// Uploading the entire payload as IMEM with 'tag = 0', 'bootvec = 0' causes a
/// HSCB exception on first fetch (EXCI=0x201f_0000)
#[derive(Copy, Clone, Debug)]
pub struct HsflcnBlDesc {
    pub bl_start_tag:          u32,
    pub bl_dmem_desc_load_off: u32,
    pub bl_code_off:           u32,
    pub bl_code_size:          u32,
    pub bl_data_off:           u32,
    pub bl_data_size:          u32,
}

impl HsflcnBlDesc {
    pub const SIZE: usize = 24;

    /// Parse 24 bytes at 'at' inside 'blob'. None on truncation or a
    /// (code|data) region outside 'blob'
    pub fn parse(blob: &[u8], at: usize, payload_off: u32, payload_size: u32) -> Option<Self> {
        if at.checked_add(Self::SIZE)? > blob.len() { return None; }
        let r = |o: usize| u32::from_le_bytes(
            [blob[at+o], blob[at+o+1], blob[at+o+2], blob[at+o+3]]);
        let d = HsflcnBlDesc {
            bl_start_tag:          r(0),
            bl_dmem_desc_load_off: r(4),
            bl_code_off:           r(8),
            bl_code_size:          r(12),
            bl_data_off:           r(16),
            bl_data_size:          r(20),
        };
        let in_payload = |off: u32, size: u32| -> bool {
            let end = match off.checked_add(size) { Some(e) => e, None => return false };
            end <= payload_size
        };
        if !in_payload(d.bl_code_off, d.bl_code_size) { return None; }
        if !in_payload(d.bl_data_off, d.bl_data_size) { return None; }
        let _ = payload_off; // present for caller clarity, not used in bounds
        Some(d)
    }
}

/// Falcon bootloader DMEM descriptor (flcn_bl_dmem_desc v2, Turing).
///
/// Layout: nouveau 'drivers/gpu/drm/nouveau/include/nvfw/flcn.h'
/// ('flcn_bl_dmem_desc_v2'). From Turing the bl reads two more u32 fields than
/// v1's 80 bytes ('argc'/'argv'); argc=0 / argv=0 for ACR boot, since the
/// manifest carries the WPR2 layout.
///
/// All '*_dma_base' fields encode the source phys address as '(phys >> 8)'
/// (256-byte chunks). The '*_dma_base1' fields carry bits '[48:32]' (Turing
/// supports 49-bit phys).
///
/// 21 u32 = 84 bytes total
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct FlcnBlDmemDesc {
    pub reserved:          [u32; 4],
    /// 16-byte production signature, from the HS header. The bl applies it at
    /// the patch location during boot
    pub signature:         [u32; 4],
    /// FBIF context-DMA index (we use ctx 0)
    pub ctx_dma:           u32,
    /// Low 32 bits of '(image_phys >> 8)' for the code segment
    pub code_dma_base:     u32,
    /// Offset within the staged image of the non-secure (LS-style) code
    pub non_sec_code_off:  u32,
    pub non_sec_code_size: u32,
    /// Offset within the staged image of the secure (HS) code (first app)
    pub sec_code_off:      u32,
    pub sec_code_size:     u32,
    /// HS entry point, typically 0 (start of secure code)
    pub code_entry_point:  u32,
    /// Low 32 bits of '(image_phys >> 8)' for the data segment (may equal
    /// 'code_dma_base' when code and data share an FBIF window)
    pub data_dma_base:     u32,
    pub data_size:         u32,
    /// Upper bits '[48:32]' of 'code_dma_base'
    pub code_dma_base1:    u32,
    /// Upper bits '[48:32]' of 'data_dma_base'
    pub data_dma_base1:    u32,
    /// v2: argument count passed to the HS entry. Zero for ACR; the bootloader
    /// manifest carries the WPR2 layout
    pub argc:              u32,
    /// v2: argument vector sysmem phys address. Zero for ACR
    pub argv:              u32,
}

impl FlcnBlDmemDesc {
    pub const SIZE: usize = 84;

    /// Build the bl descriptor for an ahesasc image staged at 'image_phys', the
    /// phys address of the NVFW data payload, not of the container: only the
    /// payload is DMAed
    pub fn build_for_ahesasc(layers: &HsLayers, image_phys: u64, ctx_dma: u8) -> Self {
        // The HS load-header offsets are relative to the payload start, where
        // image_phys points, so no per-segment adjustment
        let dma_lo = (image_phys >> 8) as u32;
        let dma_hi = ((image_phys >> 40) & 0xFFFF) as u32;

        // The first app slot is the secure (HS) code. ACR HS images on Turing
        // have exactly one app; zero apps means no secure segment and the field
        // stays 0/0
        let (sec_off, sec_size) = if layers.load.num_apps > 0 {
            layers.load.apps[0]
        } else {
            (0, 0)
        };

        // 16 sig bytes as 4 little-endian u32s. The bl writes them back into
        // the IMEM image byte-for-byte (Falcon and the x86 host are both
        // little-endian)
        let s = layers.sig_prod;
        let sig = [
            u32::from_le_bytes([s[0],  s[1],  s[2],  s[3] ]),
            u32::from_le_bytes([s[4],  s[5],  s[6],  s[7] ]),
            u32::from_le_bytes([s[8],  s[9],  s[10], s[11]]),
            u32::from_le_bytes([s[12], s[13], s[14], s[15]]),
        ];

        FlcnBlDmemDesc {
            reserved:          [0; 4],
            signature:         sig,
            ctx_dma:           ctx_dma as u32,
            code_dma_base:     dma_lo,
            non_sec_code_off:  layers.load.non_sec_code_off,
            non_sec_code_size: layers.load.non_sec_code_size,
            sec_code_off:      sec_off,
            sec_code_size:     sec_size,
            code_entry_point:  0,
            data_dma_base:     dma_lo,
            data_size:         layers.load.data_size,
            code_dma_base1:    dma_hi,
            data_dma_base1:    dma_hi,
            argc:              0,
            argv:              0,
        }
    }

    /// Serialize to 84 bytes ready for 'Engine::dmem_load'
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let words: [u32; 21] = [
            self.reserved[0], self.reserved[1], self.reserved[2], self.reserved[3],
            self.signature[0], self.signature[1], self.signature[2], self.signature[3],
            self.ctx_dma,
            self.code_dma_base,
            self.non_sec_code_off, self.non_sec_code_size,
            self.sec_code_off, self.sec_code_size,
            self.code_entry_point,
            self.data_dma_base, self.data_size,
            self.code_dma_base1, self.data_dma_base1,
            self.argc, self.argv,
        ];
        for (i, w) in words.iter().enumerate() {
            le::put_u32(&mut out, i*4, *w);
        }
        out
    }
}

#[derive(Debug, Copy, Clone)]
pub struct AcrStatus {
    /// MAILBOX0 at halt. ACR convention: 0 = ok
    pub mb0: u32,
    /// MAILBOX1 contents at halt. Carries a status / sub-error code
    pub mb1: u32,
    /// CPUCTL contents at halt
    pub cpuctl: u32,
    /// Physical address of the ahesasc sysmem staging area
    pub ahesasc_phys: u64,
    /// Size in bytes of the ahesasc payload staged in sysmem
    pub ahesasc_size: u32,
}

/// NV_PMC_ENABLE (BAR0-absolute). The per-chip SEC2 bit mask comes from
/// 'quirks::for_chip()' at reset time. Reference layout: nouveau
/// 'gp102_flcn_reset_eng' with 'func->reset_pmc'
const NV_PMC_ENABLE: u32 = 0x0000_0200;

/// Full SEC2 reset sequence (PMC gate-cycle + engine reset + CPUCTL).
///
/// Sequence (open-gpu-kernel-modules 'nvFalcon0ResetHw_TU102', nouveau
/// 'gp102_flcn_reset_eng' with 'reset_pmc = 0x4000'):
///
///   1. PMC_ENABLE.SEC2 <- 0   (gate the engine, kills clocks)
///   2. ~10us settle
///   3. PMC_ENABLE.SEC2 <- 1   (ungate, engine comes up in HW reset)
///   4. ~100us settle for clocks/PLL to stabilize
///   5. FALCON_ENGINE.RESET <- 1 / 0   (deassert any latched reset)
///   6. CPUCTL.HRESET <- 1     (clears CPU state)
///   7. Verify cpuctl is out of the stale HALTED state
///
/// The PMC gate cycle alone bypasses the SEC2 priv-mask: PMC_ENABLE is always
/// host-writable, while FALCON_ENGINE (0x3c0) and CPUCTL.HRESET are
/// priv-protected on Turing and silently ignore host writes
fn soft_reset(eng: &Engine) -> bool {
    use crate::drivers::gpu::nvidia::falcon::{FALCON_ENGINE, FALCON_ENGINE_RESET};
    use super::quirks;
    const CPUCTL_HRESET: u32 = 1 << 3;

    let bar0 = eng.bar0();
    let pmc_mask = match quirks::detect(bar0) {
        Some(q) => q.sec2_pmc_reset_mask,
        None => {
            // Unknown chip: Turing default (bit 14), with a warning asking for
            // a quirks entry
            crate::println!("  WARN: unknown chip, using default SEC2 PMC mask 0x4000");
            0x0000_4000
        }
    };

    let pmc_cur = bar0.read32(NV_PMC_ENABLE);
    bar0.write32(NV_PMC_ENABLE, pmc_cur & !pmc_mask);
    let pmc_after_clear = bar0.read32(NV_PMC_ENABLE);
    for _ in 0..10_000 { core::hint::spin_loop(); }
    bar0.write32(NV_PMC_ENABLE, pmc_cur | pmc_mask);
    let pmc_after_set = bar0.read32(NV_PMC_ENABLE);
    for _ in 0..100_000 { core::hint::spin_loop(); }
    crate::println!(
        "  PMC_ENABLE mask={:#x}: {:#x} -> clear -> {:#x} -> set -> {:#x} (bit set={})",
        pmc_mask, pmc_cur, pmc_after_clear, pmc_after_set,
        if pmc_after_set & pmc_mask != 0 { 1 } else { 0 }
    );

    eng.write(FALCON_ENGINE, FALCON_ENGINE_RESET);
    for _ in 0..1_000 {
        let _ = eng.read(FALCON_ENGINE);
        core::hint::spin_loop();
    }
    eng.write(FALCON_ENGINE, 0);

    eng.write(FALCON_CPUCTL, CPUCTL_HRESET);
    for _ in 0..10_000 {
        let _ = eng.read(FALCON_CPUCTL);
        core::hint::spin_loop();
    }

    for _ in 0..100_000 {
        let c = eng.read(FALCON_CPUCTL);
        if c & (CPUCTL_HALTED | CPUCTL_HRESET) == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Round 'n' up to the next multiple of 4 KiB and return the page count

/// Allocate sysmem and copy the NVFW data payload of 'ucode_ahesasc' into it.
/// Returns the buffer and its phys address; the caller keeps the buffer for the
/// whole SEC2 boot
fn stage_ahesasc() -> Result<(DmaBuffer, u64, u32), Sec2Error> {
    let ahesasc_fw = tu116_fw::acr_ahesasc();
    stage_hs_payload(ahesasc_fw.bytes())
}

/// First-contact SEC2 ACR boot. Returns the bootloader's halt status, or an
/// error without touching CPUCTL
pub fn attempt_acr(bar0: &MmioRegion) -> Result<AcrStatus, Sec2Error> {
    let sec2 = Engine::new(bar0, falcon::PSEC_BASE, "sec2");

    if !sec2.is_alive() {
        serial_println!(
            "[sec2] engine gated: HWCFG={:#x} - aborting (PMC/device-enable bring-up not yet implemented)",
            sec2.read(falcon::FALCON_HWCFG)
        );
        return Err(Sec2Error::EngineGated);
    }

    let imem = sec2.imem_size();
    let dmem = sec2.dmem_size();
    serial_println!(
        "[sec2] engine alive: imem={}B dmem={}B cpuctl_pre={:#010x}",
        imem, dmem, sec2.read(FALCON_CPUCTL)
    );

    if !soft_reset(&sec2) {
        serial_println!("[sec2] warn: soft reset did not complete; proceeding anyway");
    }

    let (ahesasc_buf, ahesasc_phys, ahesasc_size) = stage_ahesasc()?;
    serial_println!(
        "[sec2] ahesasc staged @ {:#x} ({} bytes, {} pages)",
        ahesasc_phys, ahesasc_size, ahesasc_buf.pages()
    );

    // FBIF ctxdma for a bootloader DMATRF from sysmem. The post-reset slot
    // value (LOCAL_FB) points at VRAM, which the bootloader must not read
    let prev_transcfg = fbif::read_transcfg_raw(&sec2, ACR_CTXDMA);
    let prev_fbif_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
    let new_transcfg = fbif::program_transcfg(
        &sec2, ACR_CTXDMA, FbifTarget::NoncoherentSysmem, FbifMemType::Physical,
    );
    sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl | fbif::FBIF_CTL_ALLOW_PHYS_NO_CTX);
    serial_println!(
        "[sec2] FBIF ctx{}: {:#010x} -> {:#010x} (NONCOHERENT_SYSMEM phys), CTL: {:#010x} -> {:#010x}",
        ACR_CTXDMA, prev_transcfg, new_transcfg,
        prev_fbif_ctl, sec2.read(fbif::FBIF_CTL_OFFSET)
    );

    let acr_bl_fw = tu116_fw::acr_bl();
    let bl = acr_bl_fw.bytes();
    let bl_hdr = NvfwBinHdr::parse(bl).ok_or(Sec2Error::BadBlHeader)?;
    let bl_payload = bl_hdr.data(bl);
    serial_println!(
        "[sec2] acr/bl: payload {} bytes (NVFW hdr: header_off={:#x} data_off={:#x})",
        bl_payload.len(), bl_hdr.header_offset, bl_hdr.data_offset
    );

    if (bl_payload.len() as u32) > imem {
        return Err(Sec2Error::BlTooLarge { payload: bl_payload.len() as u32, imem });
    }

    let uploaded = sec2.imem_load(0, 0, bl_payload, /*secure=*/ true);
    serial_println!("[sec2] uploaded {} bytes to IMEM (SECURE)", uploaded);

    // Mailbox handoff: ahesasc sysmem address (lo in MB0, hi in MB1). The full
    // ACR DMEM scratch layout (manifest offset, code size, data size, expected
    // WPR2 region) is not modelled; the bootloader halts early reading zeros
    sec2.write(FALCON_MAILBOX0, ahesasc_phys as u32);
    sec2.write(FALCON_MAILBOX1, (ahesasc_phys >> 32) as u32);
    serial_println!(
        "[sec2] mailboxes: MB0={:#010x} MB1={:#010x} (ahesasc @ {:#x})",
        ahesasc_phys as u32, (ahesasc_phys >> 32) as u32, ahesasc_phys
    );

    serial_println!("[sec2] CPUCTL kick: bootvec=0, polling for halt...");
    sec2.start_at(0);

    if !sec2.wait_halted_ns(ACR_HALT_TIMEOUT_NS) {
        let cpuctl_live = sec2.read(FALCON_CPUCTL);
        serial_println!(
            "[sec2] timeout waiting for halt (cpuctl={:#010x}); issuing soft reset",
            cpuctl_live
        );
        soft_reset(&sec2);
        drop(ahesasc_buf);
        return Err(Sec2Error::Timeout);
    }

    let mb0 = sec2.read(FALCON_MAILBOX0);
    let mb1 = sec2.read(FALCON_MAILBOX1);
    let cpuctl = sec2.read(FALCON_CPUCTL);

    serial_println!(
        "[sec2] halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x} (HALTED bit={})",
        mb0, mb1, cpuctl,
        if cpuctl & CPUCTL_HALTED != 0 { 1 } else { 0 }
    );

    match mb1 {
        0 => serial_println!(
            "[sec2] ACR status: 0 (early exit - DMEM scratch layout not populated yet)"
        ),
        _ if crate::drivers::gpu::nvidia::pri::is_falcon(mb1) => serial_println!(
            "[sec2] ACR status: PRI / sentinel error class ({:#x})", mb1
        ),
        _ => serial_println!(
            "[sec2] ACR status: opaque code {:#x} (decode requires ACR bl source)", mb1
        ),
    }

    drop(ahesasc_buf);

    Ok(AcrStatus { mb0, mb1, cpuctl, ahesasc_phys, ahesasc_size })
}

/// V2 boot path: parse the HS layers of ahesasc, build a 'flcn_bl_dmem_desc'
/// pointing at the sysmem-staged image, upload it to SEC2 DMEM at offset 0,
/// kick the bl. With a correct desc the bl DMAs the HS image into its IMEM,
/// verifies the production signature, and locks WPR2 or halts with an ACR
/// status code
pub fn attempt_acr_v2(bar0: &MmioRegion) -> Result<AcrStatus, Sec2Error> {
    let sec2 = Engine::new(bar0, falcon::PSEC_BASE, "sec2");

    if !sec2.is_alive() {
        serial_println!(
            "[sec2] engine gated: HWCFG={:#x} - aborting",
            sec2.read(falcon::FALCON_HWCFG)
        );
        return Err(Sec2Error::EngineGated);
    }

    let imem = sec2.imem_size();
    let dmem = sec2.dmem_size();
    let exci_baseline = sec2.read(FALCON_EXCI);
    let cpuctl_baseline = sec2.read(FALCON_CPUCTL);
    serial_println!(
        "[sec2] v2 engine alive: imem={}B dmem={}B cpuctl_pre={:#010x} EXCI_pre={:#010x}",
        imem, dmem, cpuctl_baseline, exci_baseline
    );
    crate::println!(
        "  v2 baseline: cpuctl={:#x} EXCI={:#x} (before reset)",
        cpuctl_baseline, exci_baseline
    );

    if !soft_reset(&sec2) {
        serial_println!("[sec2] warn: soft reset did not complete; proceeding anyway");
    }
    let exci_post_reset = sec2.read(FALCON_EXCI);
    let cpuctl_post_reset = sec2.read(FALCON_CPUCTL);
    crate::println!(
        "  v2 post-reset: cpuctl={:#x} EXCI={:#x}",
        cpuctl_post_reset, exci_post_reset
    );

    // After reset the Falcon scrubs its IMEM/DMEM. Until those bits clear,
    // host-driven IMEM/DMEM writes are silently dropped
    if !wait_scrub_done(&sec2) {
        serial_println!(
            "[sec2] warn: scrub did not complete (DMACTL={:#010x}); proceeding anyway",
            sec2.read(FALCON_DMACTL)
        );
    }
    // Clear REQUIRE_CTX so the bl can issue DMATRF kicks without a PFIFO ctxsw
    // loading a channel context. It is set after reset, and with it set a DMA
    // kick does nothing: no busy state, no ERROR bit, no transfer
    let dmactl_pre = sec2.read(FALCON_DMACTL);
    sec2.write(FALCON_DMACTL, dmactl_pre & !DMACTL_REQUIRE_CTX);
    serial_println!(
        "[sec2] DMACTL: {:#010x} -> {:#010x} (REQUIRE_CTX cleared)",
        dmactl_pre, sec2.read(FALCON_DMACTL)
    );

    // Parse HS layers before allocating anything
    let layers = match parse_ahesasc_layers() {
        Ok(l)  => l,
        Err(e) => {
            serial_println!("[sec2] ahesasc HS parse failed: {:?}", e);
            return Err(Sec2Error::BadAhesascHeader);
        }
    };
    serial_println!(
        "[sec2] HS layers: non_sec[off={:#x} sz={:#x}] data[off={:#x} sz={:#x}] apps={} sec[off={:#x} sz={:#x}]",
        layers.load.non_sec_code_off, layers.load.non_sec_code_size,
        layers.load.data_dma_base, layers.load.data_size,
        layers.load.num_apps,
        layers.load.apps[0].0, layers.load.apps[0].1
    );
    serial_println!(
        "[sec2] sig_prod: {:02x}{:02x}{:02x}{:02x}...{:02x}{:02x}{:02x}{:02x}",
        layers.sig_prod[0], layers.sig_prod[1], layers.sig_prod[2], layers.sig_prod[3],
        layers.sig_prod[12], layers.sig_prod[13], layers.sig_prod[14], layers.sig_prod[15]
    );

    // Stage the data payload of ahesasc (past the NVFW container header). The
    // HS load_header offsets are relative to it, so the bl DMAs from here
    let (ahesasc_buf, ahesasc_phys, ahesasc_size) = stage_ahesasc()?;
    serial_println!(
        "[sec2] ahesasc staged @ {:#x} ({} bytes, {} pages)",
        ahesasc_phys, ahesasc_size, ahesasc_buf.pages()
    );

    // FBIF ctx 0 -> noncoherent sysmem, physical addressing
    let prev_transcfg = fbif::read_transcfg_raw(&sec2, ACR_CTXDMA);
    let prev_fbif_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
    let new_transcfg = fbif::program_transcfg(
        &sec2, ACR_CTXDMA, FbifTarget::NoncoherentSysmem, FbifMemType::Physical,
    );
    sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl | fbif::FBIF_CTL_ALLOW_PHYS_NO_CTX);
    serial_println!(
        "[sec2] FBIF ctx{}: {:#010x} -> {:#010x}, CTL: {:#010x} -> {:#010x}",
        ACR_CTXDMA, prev_transcfg, new_transcfg,
        prev_fbif_ctl, sec2.read(fbif::FBIF_CTL_OFFSET)
    );

    // The LS-style bl descriptor at 'bl.bin''s header_offset: bl_start_tag
    // (IMEM virt tag), code/data slice offsets within the payload, where in
    // DMEM the FlcnBlDmemDesc goes. Loaded with tag=0 and started at PC=0, the
    // bl fails sig verify on first fetch
    let acr_bl_fw = tu116_fw::acr_bl();
    let bl = acr_bl_fw.bytes();
    let bl_hdr = NvfwBinHdr::parse(bl).ok_or(Sec2Error::BadBlHeader)?;
    let bl_payload = bl_hdr.data(bl);
    let bl_desc = HsflcnBlDesc::parse(
        bl, bl_hdr.header_offset as usize,
        bl_hdr.data_offset, bl_hdr.data_size,
    ).ok_or(Sec2Error::BadBlHeader)?;
    serial_println!(
        "[sec2] bl LS desc: start_tag={:#x} dmem_desc_load_off={:#x} code[{:#x}..+{:#x}] data[{:#x}..+{:#x}]",
        bl_desc.bl_start_tag, bl_desc.bl_dmem_desc_load_off,
        bl_desc.bl_code_off, bl_desc.bl_code_size,
        bl_desc.bl_data_off, bl_desc.bl_data_size,
    );
    crate::println!(
        "  bl LS desc: tag={:#x} desc_off={:#x} code_sz={:#x} data_sz={:#x}",
        bl_desc.bl_start_tag, bl_desc.bl_dmem_desc_load_off,
        bl_desc.bl_code_size, bl_desc.bl_data_size,
    );
    if bl_desc.bl_code_size > imem {
        return Err(Sec2Error::BlTooLarge { payload: bl_desc.bl_code_size, imem });
    }

    let bl_code = &bl_payload[bl_desc.bl_code_off as usize
        .. (bl_desc.bl_code_off + bl_desc.bl_code_size) as usize];
    let bl_data = &bl_payload[bl_desc.bl_data_off as usize
        .. (bl_desc.bl_data_off + bl_desc.bl_data_size) as usize];

    // 1. Load bl's own data segment at DMEM offset 0 (bl-internal vars)
    let dwritten_data = sec2.dmem_load(0, bl_data);
    serial_println!("[sec2] bl data -> DMEM@0: {}B", dwritten_data);

    // 2. Place the 80-byte FlcnBlDmemDesc at the offset the bl expects
    let desc = FlcnBlDmemDesc::build_for_ahesasc(&layers, ahesasc_phys, ACR_CTXDMA);
    let desc_bytes = desc.to_bytes();
    if bl_desc.bl_dmem_desc_load_off.saturating_add(desc_bytes.len() as u32) > dmem {
        serial_println!("[sec2] desc would overflow DMEM (off={:#x} sz={} dmem={})",
            bl_desc.bl_dmem_desc_load_off, desc_bytes.len(), dmem);
        return Err(Sec2Error::BlTooLarge { payload: desc_bytes.len() as u32, imem: dmem });
    }
    let dwritten = sec2.dmem_load(bl_desc.bl_dmem_desc_load_off, &desc_bytes);
    serial_println!(
        "[sec2] desc -> DMEM@{:#x}: {}B (ctx_dma={} code_phys={:#x} sec[off={:#x} sz={:#x}] data_sz={:#x})",
        bl_desc.bl_dmem_desc_load_off, dwritten,
        desc.ctx_dma, ((desc.code_dma_base1 as u64) << 40) | ((desc.code_dma_base as u64) << 8),
        desc.sec_code_off, desc.sec_code_size, desc.data_size
    );

    // Upload bl code at IMEM offset 0, with the virtual tag the bl was
    // assembled to run at. SECURE upload routes through HSCB
    let uploaded = sec2.imem_load(0, bl_desc.bl_start_tag, bl_code, /*secure=*/ true);
    serial_println!(
        "[sec2] bl code -> IMEM@0 (tag={:#x}, SECURE): {}B",
        bl_desc.bl_start_tag, uploaded
    );

    // Check of the IMEM upload. SECURE-uploaded pages are HSCB-protected, so
    // the readback is typically zeros; a readable pattern means they are not
    let peek = sec2.imem_peek16(0);
    let expect: [u8; 16] = {
        let mut e = [0u8; 16];
        let n = core::cmp::min(16, bl_code.len());
        e[..n].copy_from_slice(&bl_code[..n]);
        e
    };
    let matches = peek == expect;
    let all_zero = peek.iter().all(|&b| b == 0);
    crate::println!(
        "  IMEM peek@0: first4 read={:02x}{:02x}{:02x}{:02x} expect={:02x}{:02x}{:02x}{:02x} match={} zero={}",
        peek[0], peek[1], peek[2], peek[3],
        expect[0], expect[1], expect[2], expect[3],
        matches, all_zero
    );

    // Pre-program DMATRFBASE/BASE1 so the bl can DMA from sysmem without
    // writing them from the desc. Encoding: (phys >> 8) in BASE, bits[48:32] of
    // (phys >> 8) in BASE1
    let dma_base_lo = (ahesasc_phys >> 8) as u32;
    let dma_base_hi = ((ahesasc_phys >> 40) & 0xFFFF) as u32;
    sec2.write(FALCON_DMATRFBASE, dma_base_lo);
    sec2.write(FALCON_DMATRFBASE1, dma_base_hi);
    serial_println!(
        "[sec2] DMATRFBASE = {:#010x}, DMATRFBASE1 = {:#010x} (image_phys>>8 = {:#x})",
        dma_base_lo, dma_base_hi, ahesasc_phys >> 8
    );

    // MAILBOX0 = DMEM offset of the FlcnBlDmemDesc, per the NVIDIA bl ABI.
    // MAILBOX1 is the status the bl writes on halt; seeded with a sentinel to
    // tell "bl never wrote it" (sentinel survives) from "bl wrote 0"
    const MAILBOX1_SENTINEL: u32 = 0xCAFE_BABE;
    sec2.write(FALCON_MAILBOX0, bl_desc.bl_dmem_desc_load_off);
    sec2.write(FALCON_MAILBOX1, MAILBOX1_SENTINEL);
    crate::println!("  pre-kick: MB0={:#x} MB1=sentinel({:#x})",
        bl_desc.bl_dmem_desc_load_off, MAILBOX1_SENTINEL);

    let bootvec = bl_desc.bl_start_tag << 8;
    serial_println!(
        "[sec2] v2 pre-kick state: DMACTL={:#010x} DMATRFCMD={:#010x}",
        sec2.read(FALCON_DMACTL), sec2.read(FALCON_DMATRFCMD)
    );
    serial_println!(
        "[sec2] v2 CPUCTL kick: bootvec={:#x} (start_tag={:#x}), polling for halt...",
        bootvec, bl_desc.bl_start_tag
    );
    sec2.start_at(bootvec);
    let c_t0 = sec2.read(FALCON_CPUCTL);
    let c_t1 = sec2.read(FALCON_CPUCTL);
    let c_t2 = sec2.read(FALCON_CPUCTL);
    let bootvec_rb = sec2.read(falcon::FALCON_BOOTVEC);
    crate::println!(
        "  post-kick: cpuctl t0={:#x} t1={:#x} t2={:#x} BOOTVEC={:#x}",
        c_t0, c_t1, c_t2, bootvec_rb
    );

    // Halt poll bounded by PTIMER. The bl halts within a fraction of a
    // millisecond (success or HSCB sig-fail trap) or never, so a non-halting
    // boot costs ~300 ms. The timeout branch below captures the diagnostic
    // state
    let halted_ok = sec2.wait_halted_ns(ACR_HALT_TIMEOUT_NS);

    if !halted_ok {
        // Capture the diagnostic state before the soft reset clobbers it. A
        // timeout means the CPU did not halt in budget: progress (bl DMA + sig
        // verify can be slow) or an infinite loop waiting for something not
        // provided (host mailbox handshake, FBIF event)
        let cpuctl_live = sec2.read(FALCON_CPUCTL);
        let mb0_live    = sec2.read(FALCON_MAILBOX0);
        let mb1_live    = sec2.read(FALCON_MAILBOX1);
        let exci_live   = sec2.read(FALCON_EXCI);
        let dmactl_live = sec2.read(FALCON_DMACTL);
        let dmatrf_live = sec2.read(FALCON_DMATRFCMD);
        serial_println!(
            "[sec2] TIMEOUT live: cpuctl={:#010x} mb0={:#010x} mb1={:#010x}",
            cpuctl_live, mb0_live, mb1_live
        );
        serial_println!(
            "[sec2] TIMEOUT live: EXCI={:#010x} DMACTL={:#010x} DMATRFCMD={:#010x}",
            exci_live, dmactl_live, dmatrf_live
        );
        crate::println!(
            "  TIMEOUT: cpuctl={:#x} mb0={:#x} mb1={:#x}",
            cpuctl_live, mb0_live, mb1_live
        );
        crate::println!(
            "  TIMEOUT: EXCI={:#x} DMACTL={:#x} DMATRFCMD={:#x}",
            exci_live, dmactl_live, dmatrf_live
        );
        soft_reset(&sec2);
        drop(ahesasc_buf);
        return Err(Sec2Error::Timeout);
    }

    let mb0 = sec2.read(FALCON_MAILBOX0);
    let mb1 = sec2.read(FALCON_MAILBOX1);
    let cpuctl = sec2.read(FALCON_CPUCTL);
    let exci = sec2.read(FALCON_EXCI);
    let dmactl_post = sec2.read(FALCON_DMACTL);
    let dmatrf_post = sec2.read(FALCON_DMATRFCMD);

    serial_println!(
        "[sec2] v2 halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x} (HALTED bit={})",
        mb0, mb1, cpuctl,
        if cpuctl & CPUCTL_HALTED != 0 { 1 } else { 0 }
    );
    serial_println!(
        "[sec2] v2 post-state: EXCI={:#010x} DMACTL={:#010x} DMATRFCMD={:#010x}",
        exci, dmactl_post, dmatrf_post
    );

    match mb1 {
        0 => serial_println!("[sec2] v2 ACR status: 0 (clean exit or pre-DMA halt)"),
        _ if crate::drivers::gpu::nvidia::pri::is_falcon(mb1) => serial_println!(
            "[sec2] v2 ACR status: PRI / sentinel error class ({:#x})", mb1
        ),
        _ => serial_println!(
            "[sec2] v2 ACR status: opaque code {:#x} (cross-ref open-gpu-kernel-modules acr source)",
            mb1
        ),
    }

    // PFB WPR2 lock status. After a successful ACR, WPR2 LO/HI describe a
    // non-empty locked region at the top of VRAM. Without the full DMEM scratch
    // + manifest handoff they read as zero
    use super::regs::{
        decode_wpr_addr, PFB_PRI_MMU_WPR1_ADDR_HI, PFB_PRI_MMU_WPR1_ADDR_LO,
        PFB_PRI_MMU_WPR2_ADDR_HI, PFB_PRI_MMU_WPR2_ADDR_LO,
    };
    let wpr1_lo_raw = bar0.read32(PFB_PRI_MMU_WPR1_ADDR_LO);
    let wpr1_hi_raw = bar0.read32(PFB_PRI_MMU_WPR1_ADDR_HI);
    let wpr2_lo_raw = bar0.read32(PFB_PRI_MMU_WPR2_ADDR_LO);
    let wpr2_hi_raw = bar0.read32(PFB_PRI_MMU_WPR2_ADDR_HI);
    let wpr2_lo = decode_wpr_addr(wpr2_lo_raw);
    let wpr2_hi = decode_wpr_addr(wpr2_hi_raw);
    let wpr2_locked = wpr2_lo != 0 && wpr2_lo <= wpr2_hi;
    serial_println!(
        "[sec2] PFB WPR1: lo={:#010x} hi={:#010x}  WPR2: lo={:#010x} hi={:#010x}",
        wpr1_lo_raw, wpr1_hi_raw, wpr2_lo_raw, wpr2_hi_raw
    );
    if wpr2_locked {
        serial_println!(
            "[sec2] WPR2 LOCKED: {:#x} .. {:#x} ({} MiB)",
            wpr2_lo, wpr2_hi, (wpr2_hi.saturating_sub(wpr2_lo)) >> 20
        );
    } else {
        serial_println!(
            "[sec2] WPR2 not locked (lo=0 or lo>hi) - ACR did not complete WPR setup"
        );
    }

    drop(ahesasc_buf);

    Ok(AcrStatus { mb0, mb1, cpuctl, ahesasc_phys, ahesasc_size })
}

// GSP booter_load on SEC2 (stage 1 of the GSP-RM boot)
//
// The GSP-RM boot path, distinct from the ahesasc ACR above. Per
// open-gpu-kernel-modules 'kgspExecuteBooterLoad_TU102' + 's_setupLoader'
// (kernel_gsp_falcon_tu102.c), the GSP booter runs on SEC2, not GSP, and the
// booter itself sets up and locks WPR2; AHESASC is not involved.
//
// Load model (WITH_LOADER): the generic SEC2 bootloader from 'acr/bl.bin' is
// placed at the top of SEC2 IMEM (non-secure). Its 84-byte 'FlcnBlDmemDesc'
// (pointing at the booter_load HS image staged in sysmem) goes to DMEM offset
// 0. The generic BL DMAs the booter image into IMEM, verifies its production
// signature in HSCB, and jumps to it. The booter reads the WPR-meta sysmem
// address from SEC2 MAILBOX0/1.
//
// The booter writes 0 to MAILBOX0 on success; non-zero at halt is an error
// code.

/// Result of a booter_load attempt on SEC2
#[derive(Debug, Copy, Clone)]
pub struct BooterStatus {
    pub mb0: u32,
    pub mb1: u32,
    pub cpuctl: u32,
    pub exci: u32,
    /// True iff PFB reported WPR2 locked after the booter ran
    pub wpr2_locked: bool,
    pub wpr2_lo: u64,
    pub wpr2_hi: u64,
}

/// Stage an HS image's NVFW data payload into phys-contiguous sysmem. Returns
/// the buffer (kept alive by the caller), its phys address and the payload
/// length. The bl DMAs from 'phys'; the HS load-header offsets are relative to
/// it
fn stage_hs_payload(blob: &[u8]) -> Result<(DmaBuffer, u64, u32), Sec2Error> {
    let hdr = NvfwBinHdr::parse(blob).ok_or(Sec2Error::BadAhesascHeader)?;
    let payload = hdr.data(blob);
    let pages = pages_for(payload.len());
    let mut buf = DmaBuffer::alloc(pages)?;
    buf.zero();
    buf.as_mut_slice()[..payload.len()].copy_from_slice(payload);
    DmaBuffer::write_barrier();
    let phys = buf.phys();
    Ok((buf, phys, payload.len() as u32))
}

/// Run 'booter_load' on SEC2 with the WPR-meta sysmem address. On success it
/// locks WPR2, DMAs the GSP-RM image into it and starts GSP-RM. Returns the
/// booter halt status plus the post-run WPR2 lock state.
///
/// 'meta_phys': sysmem physical address of the materialized 256-byte
/// 'GspFwWprMeta' block (from 'gsprm::load').
pub fn attempt_booter_load(bar0: &MmioRegion, meta_phys: u64) -> Result<BooterStatus, Sec2Error> {
    let sec2 = Engine::new(bar0, falcon::PSEC_BASE, "sec2");

    if !sec2.is_alive() {
        serial_println!(
            "[sec2/booter] engine gated: HWCFG={:#x} - aborting",
            sec2.read(falcon::FALCON_HWCFG)
        );
        return Err(Sec2Error::EngineGated);
    }

    let imem = sec2.imem_size();
    let dmem = sec2.dmem_size();
    serial_println!(
        "[sec2/booter] engine alive: imem={}B dmem={}B; WPR-meta @ {:#x}",
        imem, dmem, meta_phys
    );

    if !soft_reset(&sec2) {
        serial_println!("[sec2/booter] warn: soft reset did not complete; proceeding");
    }
    if !wait_scrub_done(&sec2) {
        serial_println!(
            "[sec2/booter] warn: scrub not done (DMACTL={:#010x})", sec2.read(FALCON_DMACTL)
        );
    }
    let dmactl_pre = sec2.read(FALCON_DMACTL);
    sec2.write(FALCON_DMACTL, dmactl_pre & !DMACTL_REQUIRE_CTX);

    // Parse the booter_load HS image (NVFW container -> HS header -> load
    // header + production signature) and stage its payload in sysmem for the
    // generic bl
    let booter_fw = tu116_fw::booter_load_570();
    let layers = match parse_hs_layers(booter_fw.bytes()) {
        Ok(l)  => l,
        Err(e) => {
            serial_println!("[sec2/booter] booter_load HS parse failed: {:?}", e);
            return Err(Sec2Error::BadAhesascHeader);
        }
    };
    let (booter_buf, booter_phys, booter_size) = stage_hs_payload(booter_fw.bytes())?;
    serial_println!(
        "[sec2/booter] booter_load staged @ {:#x} ({} bytes); ns[{:#x}+{:#x}] app0[{:#x}+{:#x}] data[{:#x}+{:#x}]",
        booter_phys, booter_size,
        layers.load.non_sec_code_off, layers.load.non_sec_code_size,
        layers.load.apps[0].0, layers.load.apps[0].1,
        layers.load.data_dma_base, layers.load.data_size
    );

    // FBIF ctx 0 -> noncoherent sysmem, physical addressing, so the bl can pull
    // the booter image
    let prev_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
    fbif::program_transcfg(&sec2, ACR_CTXDMA, FbifTarget::NoncoherentSysmem, FbifMemType::Physical);
    sec2.write(fbif::FBIF_CTL_OFFSET, prev_ctl | fbif::FBIF_CTL_ALLOW_PHYS_NO_CTX);

    // Parse the generic SEC2 bootloader from acr/bl.bin
    let acr_bl_fw = tu116_fw::acr_bl();
    let bl = acr_bl_fw.bytes();
    let bl_hdr = NvfwBinHdr::parse(bl).ok_or(Sec2Error::BadBlHeader)?;
    let bl_payload = bl_hdr.data(bl);
    let bl_desc = HsflcnBlDesc::parse(
        bl, bl_hdr.header_offset as usize, bl_hdr.data_offset, bl_hdr.data_size,
    ).ok_or(Sec2Error::BadBlHeader)?;
    let bl_code = &bl_payload[bl_desc.bl_code_off as usize
        ..(bl_desc.bl_code_off + bl_desc.bl_code_size) as usize];
    serial_println!(
        "[sec2/booter] generic bl: start_tag={:#x} code[{:#x}+{:#x}]",
        bl_desc.bl_start_tag, bl_desc.bl_code_off, bl_desc.bl_code_size
    );

    // The descriptor the generic bl reads from DMEM offset 0: the staged booter
    // image and its production signature
    let desc = FlcnBlDmemDesc::build_for_ahesasc(&layers, booter_phys, ACR_CTXDMA);
    let desc_bytes = desc.to_bytes();
    sec2.dmem_load(0, &desc_bytes);

    // Generic bl code at the top of IMEM (non-secure), per s_setupLoader.
    // imemDstBlk = imem_size_blocks - bl_size_blocks (256-byte blocks)
    let bl_size_aligned = (bl_desc.bl_code_size + 0xFF) & !0xFF;
    let dst = imem.saturating_sub(bl_size_aligned) & !0xFF;
    let uploaded = sec2.imem_load(dst, bl_desc.bl_start_tag, bl_code, /*secure=*/ false);
    serial_println!(
        "[sec2/booter] generic bl -> IMEM@{:#x} (tag={:#x}, {}B)",
        dst, bl_desc.bl_start_tag, uploaded
    );

    // Pre-program the DMA base so the bl can pull the booter image
    sec2.write(FALCON_DMATRFBASE, (booter_phys >> 8) as u32);
    sec2.write(FALCON_DMATRFBASE1, ((booter_phys >> 40) & 0xFFFF) as u32);

    // WPR-meta address -> MAILBOX0/1 (the booter's argument). Per
    // kgspExecuteBooterLoad_TU102: mailbox0 = lo32, mailbox1 = hi32
    sec2.write(FALCON_MAILBOX0, meta_phys as u32);
    sec2.write(FALCON_MAILBOX1, (meta_phys >> 32) as u32);

    // Boot vector = generic bl virtual tag << 8
    let bootvec = bl_desc.bl_start_tag << 8;
    serial_println!("[sec2/booter] CPUCTL kick: bootvec={:#x}, polling for halt...", bootvec);
    sec2.start_at(bootvec);

    if !sec2.wait_halted_ns(ACR_HALT_TIMEOUT_NS) {
        let cpuctl = sec2.read(FALCON_CPUCTL);
        let exci = sec2.read(FALCON_EXCI);
        serial_println!(
            "[sec2/booter] TIMEOUT: cpuctl={:#010x} mb0={:#010x} EXCI={:#010x}",
            cpuctl, sec2.read(FALCON_MAILBOX0), exci
        );
        soft_reset(&sec2);
        drop(booter_buf);
        return Err(Sec2Error::Timeout);
    }

    let mb0 = sec2.read(FALCON_MAILBOX0);
    let mb1 = sec2.read(FALCON_MAILBOX1);
    let cpuctl = sec2.read(FALCON_CPUCTL);
    let exci = sec2.read(FALCON_EXCI);
    serial_println!(
        "[sec2/booter] halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x} EXCI={:#010x}",
        mb0, mb1, cpuctl, exci
    );
    if mb0 == 0 {
        serial_println!("[sec2/booter] booter reported SUCCESS (mb0=0)");
    } else {
        serial_println!("[sec2/booter] booter error code mb0={:#010x}", mb0);
    }

    // WPR2 lock status: the booter locks it on success
    use super::regs::{
        decode_wpr_addr, PFB_PRI_MMU_WPR2_ADDR_HI, PFB_PRI_MMU_WPR2_ADDR_LO,
    };
    let wpr2_lo = decode_wpr_addr(bar0.read32(PFB_PRI_MMU_WPR2_ADDR_LO));
    let wpr2_hi = decode_wpr_addr(bar0.read32(PFB_PRI_MMU_WPR2_ADDR_HI));
    let wpr2_locked = wpr2_lo != 0 && wpr2_lo <= wpr2_hi;
    if wpr2_locked {
        serial_println!(
            "[sec2/booter] WPR2 LOCKED: {:#x}..{:#x} ({} MiB)",
            wpr2_lo, wpr2_hi, (wpr2_hi.saturating_sub(wpr2_lo)) >> 20
        );
    } else {
        serial_println!("[sec2/booter] WPR2 not locked (lo={:#x} hi={:#x})", wpr2_lo, wpr2_hi);
    }

    drop(booter_buf);
    Ok(BooterStatus { mb0, mb1, cpuctl, exci, wpr2_locked, wpr2_lo, wpr2_hi })
}
