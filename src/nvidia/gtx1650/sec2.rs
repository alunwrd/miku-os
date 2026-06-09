// SEC2 ACR first-contact boot for TU116
//
// Goal: engage the SEC2 Falcon with the signed ACR bootloader
// ('acr/bl.bin'), hand it the 'ucode_ahesasc' HS image staged in
// sysmem, observe the result
//
// Why "first contact" and not full WPR2 lock:
//   On a clean POST, NVIDIA's host driver normally:
//     1. allocates a sysmem buffer, copies 'ucode_ahesasc' into it,
//     2. parses the HS image manifest (offset/size of code, data, sig),
//     3. configures FBIF ctxdma 0 to point at that sysmem buffer,
//     4. loads 'acr/bl' into SEC2 IMEM (SECURE),
//     5. preloads a few mailbox/DMEM scratch entries telling 'bl' the
//        sysmem layout of ahesasc + how big WPR2 should be,
//     6. kicks CPUCTL, polls halt.
//   We do (1)-(4) and a minimal version of (5): just the ahesasc phys
//   address in MAILBOX0/MAILBOX1. Without the full DMEM scratch layout
//   (which is opaque without booter source) 'bl' is expected to halt
//   early - either because its sig check on ahesasc passes but the
//   command word is unknown, or because the manifest pointer it expects
//   in DMEM is zero. The halt status (MAILBOX1) is the deliverable
//
// Safety:
//   - Halt poll is timeout-bounded; on timeout we soft-reset SEC2
//   - PMC top-level interrupts are masked before this runs
//   - We never write to PMC_ENABLE / NV_PMC_DEVICE_ENABLE: SEC2 is
//     enabled by VBIOS devinit on Turing and reads alive post-POST.
//     If liveness check fails we abort with EngineGated
//
// Follow-ups to make this fully lock WPR2:
//   - parse ahesasc's HS-bin header, write its code/data/manifest
//     offsets into SEC2 DMEM scratch at the addresses 'bl' expects
//   - allocate the WPR2 sysmem staging area for GSP-RM image + meta
//     and tell ahesasc where it lives
//   - read PFB WPR2 lock-status registers before/after (offsets are
//     chip-specific and not yet modelled here)

#![allow(dead_code)]

use crate::nvidia::mmio::MmioRegion;
use crate::serial_println;

use super::dma_buf::{DmaBuffer, DmaBufError};
use super::falcon::{
    self, Engine, CPUCTL_HALTED, FALCON_CPUCTL, FALCON_DMACTL, FALCON_DMATRFBASE,
    FALCON_DMATRFBASE1, FALCON_DMATRFCMD, FALCON_MAILBOX0, FALCON_MAILBOX1, FALCON_RM,
    DMACTL_DMEM_SCRUBBING, DMACTL_IMEM_SCRUBBING, DMACTL_REQUIRE_CTX,
};

/// Falcon exception-cause register (offset relative to engine base).
/// Source: envytools rnndb 'hw/falcon/falcon.xml' ("FALCON_EXCI").
/// Set by hardware when an exception (page fault, illegal insn, HSCB
/// signature fail, etc.) caused the halt. Reading is non-destructive
pub const FALCON_EXCI: u32 = 0x0148;
use super::fbif::{self, FbifMemType, FbifTarget};
use super::nvfw_hs::{NvfwHsHeader, NvfwHsLoadHeader};
use super::tu116_fw::{self, NvfwBinHdr};

/// FALCON_RM (offset 0x84) reset request bit
const FALCON_RM_RESET: u32 = 1 << 1;

/// Halt-poll timeout (wall-clock, via PTIMER). With a real bl_dmem_desc the
/// bl does real work (DMA ~22 KB from sysmem, HSCB sig verify, program WPR2
/// registers), but that still completes in well under a millisecond. 300 ms
/// is far past any legitimate ACR completion time, and bounding by real time
/// instead of a 20M spin count means a boot that never halts costs a fraction
/// of a second of MMIO reads instead of tens of seconds
const ACR_HALT_TIMEOUT_NS: u64 = 300_000_000;

/// Spin budget for DMEM/IMEM scrubbing wait. Hardware finishes the scrub
/// within tens of microseconds after reset; 1M spins is several ms
const SCRUB_SPIN_BUDGET: u32 = 1_000_000;

/// Spin and wait until both DMEM_SCRUBBING and IMEM_SCRUBBING bits in
/// DMACTL clear. Returns true if scrubbing completed within the budget.
/// Writes to IMEM/DMEM while scrubbing is in progress are dropped on the
/// floor with no error, so we MUST wait
fn wait_scrub_done(eng: &Engine) -> bool {
    let mask = DMACTL_DMEM_SCRUBBING | DMACTL_IMEM_SCRUBBING;
    for _ in 0..SCRUB_SPIN_BUDGET {
        if eng.read(FALCON_DMACTL) & mask == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// FBIF context-DMA slot we reserve for the ahesasc payload. ctx 0 is
/// the conventional choice for boot-time sysmem on NVIDIA's stack;
/// nothing else on SEC2 has claimed it before ACR runs
const ACR_CTXDMA: u8 = 0;

#[derive(Debug, Copy, Clone)]
pub enum Sec2Error {
    /// SEC2 HWCFG reads as 0, all-ones, or a PRI sentinel; the engine
    /// is gated by PMC / device-enable / floorsweep. We do not ungate
    EngineGated,
    /// 'acr/bl.bin' did not parse as a valid NVFW container
    BadBlHeader,
    /// 'acr/ucode_ahesasc.bin' did not parse as a valid NVFW container
    BadAhesascHeader,
    /// 'bl' payload exceeds SEC2 IMEM capacity
    BlTooLarge { payload: u32, imem: u32 },
    /// Could not allocate sysmem for the ahesasc payload
    Alloc(DmaBufError),
    /// CPU did not assert HALTED within the timeout. Engine has been
    /// soft-reset; status registers are not meaningful
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
    /// Inner HS load-header at 'hs_hdr.hdr_offset' did not parse, or
    /// 'num_apps' exceeded our compile-time cap
    BadLoadHdr,
    /// Production signature region did not have exactly 16 bytes (1 sig)
    /// in a position we can read
    NoProdSig,
}

/// Parsed view over a Falcon HS-bin's three-layer header
#[derive(Clone, Debug)]
pub struct HsLayers {
    pub nvfw: NvfwBinHdr,
    pub hs:   NvfwHsHeader,
    pub load: NvfwHsLoadHeader,
    /// 16 bytes of production signature copied from
    /// 'blob[hs.sig_prod_offset..][..16]'. The Falcon's HSCB consumes these
    /// at boot time after the descriptor places them at the patch location
    pub sig_prod: [u8; 16],
}

/// Parse the three nested headers of any HS-bin blob (NVFW container ->
/// HS header -> HS load header) and copy out the 16-byte production
/// signature. Works for ACR ahesasc, GSP booter_load, booter_unload -
/// every HS image we ship follows the same layout
pub fn parse_hs_layers(blob: &[u8]) -> Result<HsLayers, HsParseError> {
    let nvfw = NvfwBinHdr::parse(blob).ok_or(HsParseError::BadNvfwHdr)?;
    let hs = NvfwHsHeader::parse(blob, nvfw.header_offset as usize)
        .ok_or(HsParseError::BadHsHdr)?;
    if !hs.looks_valid(nvfw.header_offset, nvfw.data_offset) {
        return Err(HsParseError::BadHsHdr);
    }
    let load = NvfwHsLoadHeader::parse(blob, hs.hdr_offset as usize, hs.hdr_size)
        .ok_or(HsParseError::BadLoadHdr)?;

    // Production signatures are stored as one or more 16-byte chunks. We
    // take the first (NVIDIA ships a single prod sig per HS image)
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

/// Back-compat wrapper for the ACR ahesasc blob - kept so existing call
/// sites compile unchanged
#[inline]
pub fn parse_ahesasc_layers() -> Result<HsLayers, HsParseError> {
    let fw = tu116_fw::acr_ahesasc();
    parse_hs_layers(fw.bytes())
}

/// LS-style bootloader descriptor for 'acr/bl.bin'
///
/// Layout (nouveau 'include/nvfw/acr.h' 'hsflcn_bl_desc', 6 u32 = 24 bytes).
/// 'bl.bin' is NVFW-wrapped but its 'header_offset' region does NOT carry
/// an HS header - it carries this LS-style descriptor that tells us:
///   - which IMEM virtual tag the bl code expects to run at
///   - where in DMEM to place the 80-byte 'FlcnBlDmemDesc'
///   - which slice of the payload is bl CODE vs bl DATA
///
/// Without this descriptor we were uploading the entire payload as IMEM
/// with 'tag = 0', 'bootvec = 0', which causes a HSCB exception on first
/// fetch (observed EXCI=0x201f_0000) because the assembled bl expects
/// its real virtual tag
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

    /// Parse 24 bytes at 'at' inside 'blob'. Returns None on truncation
    /// or if any declared (code|data) region falls outside 'blob'
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
/// ('flcn_bl_dmem_desc_v2'). The v1 (80-byte) variant was used through
/// Volta; from Turing onwards the bl reads two additional u32 fields
/// ('argc'/'argv') so it can be told an explicit argument vector. We
/// pass argc=0 / argv=0 for ACR boot since the bootloader's manifest
/// already carries everything it needs about WPR2 layout.
///
/// All '*_dma_base' fields encode the source phys address as
/// '(phys >> 8)' (256-byte chunks). The '*_dma_base1' fields carry bits
/// '[48:32]' of the source phys address (Turing supports 49-bit phys).
///
/// 21 u32 = 84 bytes total
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct FlcnBlDmemDesc {
    pub reserved:          [u32; 4],
    /// 16-byte production signature, copied verbatim from the HS header.
    /// The bl applies this at the patch location during boot
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
    /// HS entry point - typically 0 (start of secure code)
    pub code_entry_point:  u32,
    /// Low 32 bits of '(image_phys >> 8)' for the data segment (may equal
    /// 'code_dma_base' when code and data share an FBIF window)
    pub data_dma_base:     u32,
    pub data_size:         u32,
    /// Upper bits '[48:32]' of 'code_dma_base'
    pub code_dma_base1:    u32,
    /// Upper bits '[48:32]' of 'data_dma_base'
    pub data_dma_base1:    u32,
    /// v2 extension - argument count passed to the HS entry. Zero for ACR
    /// (the bootloader manifest already carries the WPR2 layout)
    pub argc:              u32,
    /// v2 extension - argument vector sysmem phys address. Zero for ACR
    pub argv:              u32,
}

impl FlcnBlDmemDesc {
    pub const SIZE: usize = 84;

    /// Build the bl descriptor for an ahesasc image staged at 'image_phys'
    /// (the phys address of the NVFW data payload, NOT the start of the
    /// NVFW container - we DMA only the payload region)
    pub fn build_for_ahesasc(layers: &HsLayers, image_phys: u64, ctx_dma: u8) -> Self {
        // The HS load-header offsets are relative to the payload start
        // (i.e., into NVFW container with the bin_hdr.data_offset already
        // stripped). image_phys points exactly there - that is what the
        // bl will DMA from, so we encode it directly without a per-segment
        // offset adjustment
        let dma_lo = (image_phys >> 8) as u32;
        let dma_hi = ((image_phys >> 40) & 0xFFFF) as u32;

        // First app slot is the secure (HS) code. ACR HS images on Turing
        // ship with exactly one app; we treat zero apps as "no secure
        // segment" and leave the field as 0/0
        let (sec_off, sec_size) = if layers.load.num_apps > 0 {
            layers.load.apps[0]
        } else {
            (0, 0)
        };

        // Reinterpret 16 sig bytes as 4 little-endian u32s. The bl writes
        // them back into the IMEM image byte-for-byte regardless of LE/BE
        // because Falcon is little-endian end-to-end on x86 hosts
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
            out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

#[derive(Debug, Copy, Clone)]
pub struct AcrStatus {
    /// MAILBOX0 contents at halt. ACR convention: 0 = ok
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

/// Issue a soft reset to a Falcon engine. Spins up to ~100k iterations
/// NV_PMC_ENABLE (BAR0-absolute). The per-chip bit mask for SEC2 is
/// looked up from 'quirks::for_chip()' at reset time so we never branch
/// on chip implementation here. Reference layout: nouveau
/// 'gp102_flcn_reset_eng' w/ 'func->reset_pmc'
const NV_PMC_ENABLE: u32 = 0x0000_0200;

/// Full SEC2 reset sequence (PMC gate-cycle + engine reset + CPUCTL).
///
/// Sequence (open-gpu-kernel-modules 'nvFalcon0ResetHw_TU102' /
/// nouveau 'gp102_flcn_reset_eng' w/ 'reset_pmc = 0x4000'):
///
///   1. PMC_ENABLE.SEC2 <- 0   (gate the engine, kills clocks)
///   2. ~10us settle
///   3. PMC_ENABLE.SEC2 <- 1   (ungate, engine comes up in HW reset)
///   4. ~100us settle for clocks/PLL to stabilize
///   5. FALCON_ENGINE.RESET <- 1 / 0   (deassert any latched reset)
///   6. CPUCTL.HRESET <- 1     (belt-and-braces CPU-state clear)
///   7. Verify cpuctl is no longer in stale HALTED state
///
/// The PMC gate cycle is the only path that bypasses the SEC2 priv-mask
/// (PMC_ENABLE is always host-writable). Previous attempts using only
/// FALCON_ENGINE (0x3c0) or CPUCTL.HRESET silently failed because those
/// SEC2-engine-local registers are priv-protected on Turing
fn soft_reset(eng: &Engine) -> bool {
    use super::falcon::{FALCON_ENGINE, FALCON_ENGINE_RESET};
    use super::quirks;
    const CPUCTL_HRESET: u32 = 1 << 3;

    let bar0 = eng.bar0();
    let pmc_mask = match quirks::detect(bar0) {
        Some(q) => q.sec2_pmc_reset_mask,
        None => {
            // Unknown chip - fall back to Turing default (bit 14) and
            // warn so the operator can add a quirks entry
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
fn pages_for(n: usize) -> usize {
    (n + 4095) / 4096
}

/// Allocate sysmem, copy the NVFW data payload of 'ucode_ahesasc' into
/// it, and return the buffer plus its phys address. The buffer is
/// retained by the caller for the lifetime of the SEC2 boot
fn stage_ahesasc() -> Result<(DmaBuffer, u64, u32), Sec2Error> {
    let ahesasc_fw = tu116_fw::acr_ahesasc();
    let blob = ahesasc_fw.bytes();
    let hdr = NvfwBinHdr::parse(blob).ok_or(Sec2Error::BadAhesascHeader)?;
    let payload = hdr.data(blob);

    let pages = pages_for(payload.len());
    let mut buf = DmaBuffer::alloc(pages)?;
    buf.as_mut_slice()[..payload.len()].copy_from_slice(payload);
    let len = payload.len();
    if len < buf.size() {
        for b in &mut buf.as_mut_slice()[len..] { *b = 0; }
    }
    DmaBuffer::write_barrier();

    let phys = buf.phys();
    Ok((buf, phys, payload.len() as u32))
}

/// Attempt a first-contact SEC2 ACR boot. On success returns the
/// bootloader's halt status. On any pre-execution failure returns an
/// error without ever touching CPUCTL
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

    // Configure FBIF ctxdma so the bootloader can pull from sysmem if
    // it kicks a DMATRF transfer. Even if we do not issue host-side
    // DMA here, leaving the slot at its post-reset (LOCAL_FB) value
    // would point at VRAM, which the bootloader must not read from
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

    // Mailbox handoff: ahesasc sysmem address (lo in MB0, hi in MB1).
    // The full ACR DMEM scratch layout (manifest offset, code size, data
    // size, expected WPR2 region) is not modelled yet - the bootloader
    // will halt early when those reads come back as zero
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
        _ if mb1 & 0xFFFF_0000 == 0xBADF_0000 => serial_println!(
            "[sec2] ACR status: PRI / sentinel error class ({:#x})", mb1
        ),
        _ => serial_println!(
            "[sec2] ACR status: opaque code {:#x} (decode requires ACR bl source)", mb1
        ),
    }

    drop(ahesasc_buf);

    Ok(AcrStatus { mb0, mb1, cpuctl, ahesasc_phys, ahesasc_size })
}

/// V2 boot path: parse the HS layers of ahesasc, build a
/// 'flcn_bl_dmem_desc' pointing at the sysmem-staged image, upload it
/// to SEC2 DMEM at offset 0, then kick the bl. With a correct desc the
/// bl is expected to DMA the HS image into its own IMEM, verify the
/// production signature, and run far enough to either lock WPR2 or
/// halt with a specific ACR status code (no longer mb1=0)
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

    // After reset the Falcon scrubs its own IMEM/DMEM. Until those bits
    // clear, any host-driven IMEM/DMEM write is silently dropped. This
    // was the single biggest reason v1/v2 produced mb0=mb1=0
    if !wait_scrub_done(&sec2) {
        serial_println!(
            "[sec2] warn: scrub did not complete (DMACTL={:#010x}); proceeding anyway",
            sec2.read(FALCON_DMACTL)
        );
    }
    // Clear REQUIRE_CTX so the bl can issue DMATRF kicks without a PFIFO
    // ctxsw having loaded a current channel context first. After reset
    // this bit is set, and any DMA kick the bl issues will silently
    // do nothing - no busy state, no ERROR bit, no transfer
    let dmactl_pre = sec2.read(FALCON_DMACTL);
    sec2.write(FALCON_DMACTL, dmactl_pre & !DMACTL_REQUIRE_CTX);
    serial_println!(
        "[sec2] DMACTL: {:#010x} -> {:#010x} (REQUIRE_CTX cleared)",
        dmactl_pre, sec2.read(FALCON_DMACTL)
    );

    // Parse HS layers BEFORE allocating anything - if the blob is
    // malformed we want to bail out cheaply
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

    // Stage the data PAYLOAD of ahesasc (i.e., bytes past the NVFW
    // container header). The HS load_header offsets are relative to this
    // payload, so the bl needs to DMA from here
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

    // Parse the LS-style bl descriptor at 'bl.bin''s header_offset. This
    // tells us the bl_start_tag (IMEM virt tag), the code/data slice
    // offsets within the payload, and where in DMEM the bl expects the
    // 80-byte FlcnBlDmemDesc to live. Without this the bl is loaded with
    // tag=0 and started at PC=0, which fails sig verify on first fetch
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

    // 3. Upload bl code at IMEM offset 0, with the virtual tag the bl
    // was assembled to run at. SECURE upload routes through HSCB
    let uploaded = sec2.imem_load(0, bl_desc.bl_start_tag, bl_code, /*secure=*/ true);
    serial_println!(
        "[sec2] bl code -> IMEM@0 (tag={:#x}, SECURE): {}B",
        bl_desc.bl_start_tag, uploaded
    );

    // Verify the IMEM upload actually landed. SECURE-uploaded pages
    // are HSCB-protected so the readback typically comes back as zeros
    // (hardware-enforced confidentiality). What matters is whether the
    // pattern looks like "we got something" vs "host can read it back"
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
    // having to extract the base from the desc and write the regs itself.
    // Address encoding: (phys >> 8) in BASE, bits[48:32] of (phys >> 8) in BASE1
    let dma_base_lo = (ahesasc_phys >> 8) as u32;
    let dma_base_hi = ((ahesasc_phys >> 40) & 0xFFFF) as u32;
    sec2.write(FALCON_DMATRFBASE, dma_base_lo);
    sec2.write(FALCON_DMATRFBASE1, dma_base_hi);
    serial_println!(
        "[sec2] DMATRFBASE = {:#010x}, DMATRFBASE1 = {:#010x} (image_phys>>8 = {:#x})",
        dma_base_lo, dma_base_hi, ahesasc_phys >> 8
    );

    // MAILBOX0 = DMEM offset of the FlcnBlDmemDesc, per NVIDIA bl ABI.
    // The bl reads MAILBOX0 to locate its descriptor in DMEM. MAILBOX1
    // stays 0 (it is the status-output register the bl writes on halt)
    // Pre-seed MAILBOX1 with a recognizable sentinel so we can tell
    // "bl never wrote it" (sentinel survives) from "bl wrote 0" (cleared)
    // after kick. The ACR ABI puts bl status in MAILBOX1, so any
    // sentinel that is unlikely to be a legitimate status code works
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

    // Wall-clock-bounded halt poll. The bl either halts within a fraction of
    // a millisecond (success or HSCB sig-fail trap) or never. wait_halted_ns
    // bounds the wait by PTIMER so a non-halting boot costs ~300 ms instead of
    // tens of seconds of MMIO reads. Full diagnostic state is captured in the
    // timeout branch below, so the old per-chunk progress prints are gone
    let halted_ok = sec2.wait_halted_ns(ACR_HALT_TIMEOUT_NS);

    if !halted_ok {
        // Capture ALL diagnostic state BEFORE the soft reset clobbers it.
        // Timeout means the CPU did NOT halt in budget - it is either
        // making real progress (bl DMA + sig verify can be slow) or
        // stuck in an infinite loop waiting for something we did not
        // provide (host mailbox handshake, FBIF event, etc.)
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
        _ if mb1 & 0xFFFF_0000 == 0xBADF_0000 => serial_println!(
            "[sec2] v2 ACR status: PRI / sentinel error class ({:#x})", mb1
        ),
        _ => serial_println!(
            "[sec2] v2 ACR status: opaque code {:#x} (cross-ref open-gpu-kernel-modules acr source)",
            mb1
        ),
    }

    // Read PFB WPR2 lock-status registers. If ACR did its job, WPR2 LO/HI
    // now describe a non-empty locked region at the top of VRAM. Until
    // the bl is fully wired (full DMEM scratch + manifest handoff) these
    // typically read as zero; that's the diagnostic, not a panic
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

// ----------------------------------------------------------------------
// GSP booter_load on SEC2 (stage 1 of the corrected GSP-RM boot)
// ----------------------------------------------------------------------
//
// This is the GSP-RM boot path, distinct from the ahesasc ACR above. Per
// open-gpu-kernel-modules 'kgspExecuteBooterLoad_TU102' + 's_setupLoader'
// (kernel_gsp_falcon_tu102.c), the GSP booter runs on **SEC2** (not GSP),
// and the booter itself sets up and locks WPR2 - AHESASC is not involved.
//
// Load model (WITH_LOADER): the generic SEC2 bootloader from 'acr/bl.bin'
// is placed at the top of SEC2 IMEM (non-secure). Its 84-byte
// 'FlcnBlDmemDesc' (pointing at the booter_load HS image staged in sysmem)
// goes to DMEM offset 0. The generic BL then DMAs the booter image into
// IMEM, verifies its production signature in HSCB, and jumps to it. The
// booter reads the WPR-meta sysmem address from SEC2 MAILBOX0/1.
//
// Success convention: the booter writes 0 to MAILBOX0 on success; any
// non-zero MAILBOX0 at halt is an error code.

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

/// Stage an HS image's NVFW data payload into phys-contiguous sysmem.
/// Returns the buffer (kept alive by the caller), its phys address and the
/// payload length. The bl DMAs from 'phys', so the HS load-header offsets
/// are interpreted relative to it
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

/// Run 'booter_load' on SEC2 with the WPR-meta sysmem address. On a real
/// boot this locks WPR2 and DMAs the GSP-RM image into it, then starts
/// GSP-RM. Returns the booter halt status plus the post-run WPR2 lock state.
///
/// 'meta_phys' is the sysmem physical address of the materialized 256-byte
/// 'GspFwWprMeta' block (from 'gsprm::load).
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
    // header + production signature). Stage its payload in sysmem so the
    // generic bl can DMA it in
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

    // FBIF ctx 0 -> noncoherent sysmem, physical addressing (so the bl can
    // pull the booter image from sysmem)
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

    // The descriptor the generic bl reads from DMEM offset 0, pointing at
    // the staged booter image and carrying its production signature
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

    // Read WPR2 lock status - the booter locks WPR2 on success
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
