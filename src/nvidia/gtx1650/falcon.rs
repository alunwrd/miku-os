// Falcon engine primitives shared by SEC2 / GSP / NVDEC / FECS / GPCCS on TU116
//
// Every NVIDIA secure micro-controller is a "Falcon" core. Its register
// window has the same shape regardless of which engine wraps it; only the
// base offset inside BAR0 changes. This module covers:
//     engine base addresses for TU116
//     Falcon register offsets relative to that base
//     IMEM (instruction memory) and DMEM (data memory) upload via the
//     port-based PIO interface
//     CPUCTL kick (start/stop) and HALT polling
//
// Sources cross-checked: nouveau (drivers/gpu/drm/nouveau/nvkm/falcon),
// open-gpu-kernel-modules (src/common/inc/swref/published/turing/tu102),
// and envytools rnndb (rnnutil/falcon.xml)
//
// What this module does NOT do:
//     DMA-based image upload (Falcon FBIF / DMACTL). That requires a real
//     DMA buffer in VRAM or contiguous sysmem; the host PIO path here is
//     enough for HS-bin and bootloader staging but is too slow for
//     production VRAM scrub or full GR ctx restore.
//     signed-image verification. The Falcon's HSCB (HS Code Block) checks
//     the signature in hardware once the image is in IMEM, so we do not
//     re-check it in software

#![allow(dead_code)]

use crate::nvidia::mmio::MmioRegion;

// Engine base offsets inside BAR0 (TU116 / TU117 share the same map)
/// SEC2 falcon - runs ACR ucode, sets up the Write Protect Region
pub const PSEC_BASE:    u32 = 0x0084_0000;
/// GSP falcon - runs the booter HS image and then GSP-RM
pub const PGSP_BASE:    u32 = 0x0011_0000;
/// NVDEC falcon - runs the VRAM scrubber
pub const PNVDEC_BASE:  u32 = 0x0008_4000;
/// FECS (PGRAPH front-end Falcon)
pub const PFECS_BASE:   u32 = 0x0040_9000;
/// GPCCS (per-GPC ctx Falcon). TU116 has 2 GPCs; offset stride 0x8000
pub const PGPCCS0_BASE: u32 = 0x0050_2800;
pub const PGPCCS1_BASE: u32 = 0x0050_A800;

// Falcon register offsets (relative to the engine base)
pub const FALCON_IRQSCLR:      u32 = 0x004;
pub const FALCON_IRQSTAT:      u32 = 0x008;
pub const FALCON_IRQMSET:      u32 = 0x010;
pub const FALCON_IRQMCLR:      u32 = 0x014;
pub const FALCON_IRQDEST:      u32 = 0x01c;

pub const FALCON_MAILBOX0:     u32 = 0x040;
pub const FALCON_MAILBOX1:     u32 = 0x044;

pub const FALCON_RM:           u32 = 0x084;

/// Falcon engine reset register. Source: open-gpu-kernel-modules
/// 'NV_PFALCON_FALCON_ENGINE'. Writing 1 to bit 0 asserts engine reset
/// (halts CPU, clears IMEM/DMEM virtual state, clears EXCI). Must be
/// followed by writing 0 to deassert. Available on Maxwell+, used by
/// nouveau 'gp102_flcn_reset_eng' for Volta+ falcons. This is the
/// authoritative reset path - 'FALCON_RM' at 0x084 is NOT a reset
pub const FALCON_ENGINE:        u32 = 0x3c0;
pub const FALCON_ENGINE_RESET:  u32 = 1 << 0;
pub const FALCON_CPUCTL:       u32 = 0x100;
pub const FALCON_BOOTVEC:      u32 = 0x104;
pub const FALCON_HWCFG:        u32 = 0x108;
pub const FALCON_DMACTL:       u32 = 0x10c;
pub const FALCON_DMATRFBASE:   u32 = 0x110;
pub const FALCON_DMATRFMOFFS:  u32 = 0x114;
pub const FALCON_DMATRFCMD:    u32 = 0x118;
pub const FALCON_DMATRFFBOFFS: u32 = 0x11c;
/// Upper 32 bits of the DMA source physical address. Turing+ supports
/// 49-bit physical addressing; pre-Turing chips do not have this register
pub const FALCON_DMATRFBASE1:  u32 = 0x128;

pub const FALCON_IMEM_C0:      u32 = 0x180;
pub const FALCON_IMEM_D0:      u32 = 0x184;
pub const FALCON_IMEM_T0:      u32 = 0x188;

pub const FALCON_DMEM_C0:      u32 = 0x1c0;
pub const FALCON_DMEM_D0:      u32 = 0x1c4;

// CPUCTL bits
pub const CPUCTL_IINVAL:    u32 = 1 << 0;
pub const CPUCTL_STARTCPU:  u32 = 1 << 1;
pub const CPUCTL_HALTED:    u32 = 1 << 4;
pub const CPUCTL_STOPPED:   u32 = 1 << 5;

// DMACTL bits
//   REQUIRE_CTX: when 1 (the post-reset default!), the engine refuses any
//   DMATRF* kick unless a current ctx has been loaded via the PFIFO ctxsw
//   path. Host-driven DMA (i.e., bring-up before any channel exists) must
//   clear this bit first or the kick is silently dropped: no busy state,
//   no ERROR bit, no data movement. NVIDIA OpenGPU's host load path clears
//   it for the same reason
//   DMEM/IMEM_SCRUBBING are status bits: 1 means hw is currently zeroing
//   that memory after a reset and DMA targeting it must wait
pub const DMACTL_REQUIRE_CTX:    u32 = 1 << 0;
pub const DMACTL_DMEM_SCRUBBING: u32 = 1 << 1;
pub const DMACTL_IMEM_SCRUBBING: u32 = 1 << 2;

// IMEM_C / DMEM_C control bits. AINCW = auto-increment on writes,
// AINCR = auto-increment on reads, SECURE selects HSCB upload mode
pub const MEM_C_AINCW:      u32 = 1 << 24;
pub const MEM_C_AINCR:      u32 = 1 << 25;
pub const MEM_C_SECURE:     u32 = 1 << 28;

// FALCON_DMATRFCMD bit layout.
// Reference: NVIDIA open-gpu-kernel-modules dev_falcon_v4.h plus envytools
// rnndb hw/falcon/falcon.xml. IDLE at bit 1 is confirmed empirically against
// TU116 silicon (sec2/gsp/fecs all read 0x00000002 post-POST, which is the
// IDLE-only state). An earlier version of this file placed IDLE at bit 0 -
// that was a misreading
//
//   [0]     WRITE_OR_FREE write-only "release/free" sub-command. NOT a
//                         normal-transfer flag: when set, the engine performs
//                         a release operation and stays IDLE without moving
//                         data. OpenGPU's falcon load path leaves it 0;
//                         setting it from the host gives a no-op transfer
//                         that returns ok with empty IMEM/DMEM.
//   [1]     IDLE          read-only status; hw sets when transfer drains
//   [4]     WRITE         direction: 1 = falcon -> FB, 0 = FB -> falcon
//   [5]     IMEM          target: 1 = IMEM, 0 = DMEM
//   [10:8]  SIZE          log2 chunk: 0=4B, 1=8B, ..., 6=256B (max)
//   [14:12] CTXDMA        FBIF channel index (0..7), pre-configured by caller
//   [16]    SET_IMEM_TAG  set the IMEM virtual tag during transfer
//   [25]    ERROR         hw-set on aperture/permission/EBM failure
pub const DMATRFCMD_WRITE_OR_FREE: u32 = 1 << 0;
pub const DMATRFCMD_IDLE:         u32 = 1 << 1;
pub const DMATRFCMD_WRITE:        u32 = 1 << 4;
pub const DMATRFCMD_IMEM:         u32 = 1 << 5;
pub const DMATRFCMD_SET_IMEM_TAG: u32 = 1 << 16;
pub const DMATRFCMD_ERROR:        u32 = 1 << 25;
pub const DMATRFCMD_SIZE_SHIFT:   u32 = 8;
pub const DMATRFCMD_SIZE_MASK:    u32 = 0x7 << DMATRFCMD_SIZE_SHIFT;
pub const DMATRFCMD_CTXDMA_SHIFT: u32 = 12;
pub const DMATRFCMD_CTXDMA_MASK:  u32 = 0x7 << DMATRFCMD_CTXDMA_SHIFT;

/// Maximum legal 'size_log2' value for 'DmaTransfer'. 6 = 256-byte chunks
pub const DMA_SIZE_LOG2_MAX: u8 = 6;

/// Minimum source-address alignment in bytes (matches DMA chunk granularity)
pub const DMA_MIN_ALIGN: u64 = 256;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DmaTarget {
    /// Transfer touches the Falcon's instruction memory
    Imem,
    /// Transfer touches the Falcon's data memory
    Dmem,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DmaDirection {
    /// FB -> Falcon (typical: load ucode/data into Falcon)
    ToFalcon,
    /// Falcon -> FB (typical: read out a result from DMEM)
    ToFb,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum DmaError {
    /// Hardware did not idle within the spin budget. 'cmd' is the last
    /// observed DMATRFCMD value, useful for diagnosing ERROR vs stuck-busy
    Timeout { cmd: u32 },
    /// DMATRFCMD reported the ERROR bit after the transfer
    Hardware { cmd: u32 },
    /// 'size_log2' exceeded 'DMA_SIZE_LOG2_MAX'
    BadSize,
    /// 'src_phys' or 'src_off_bytes' was not 256-byte aligned
    BadAlignment,
}

/// One DMA transfer descriptor
///
/// The Falcon DMA engine reads 256-byte aligned chunks. We model the address
/// in bytes and reject mis-alignment at kick time - the math to convert to
/// the hardware's chunk-relative encoding lives inside 'Engine::dma_load'
///
/// 'ctxdma' selects an FBIF context-DMA the caller has already set up via
/// the engine's FBIF register window (TRANSCFG/REGIONCFG). FBIF setup is
/// chip-specific and intentionally out of scope here
#[derive(Copy, Clone, Debug)]
pub struct DmaTransfer {
    /// Physical address of the buffer base. Must be 256-byte aligned
    pub src_phys:      u64,
    /// Byte offset from 'src_phys' to the first byte to transfer
    /// Must be a multiple of '4 << size_log2' (and >= 4 in practice)
    pub src_off_bytes: u32,
    /// Byte offset within the target IMEM/DMEM. For IMEM this should be
    /// 256-byte aligned to keep IMEM tags coherent
    pub dst_off_bytes: u32,
    /// log2 of chunk size in bytes; legal range 0..=DMA_SIZE_LOG2_MAX
    /// Chunk size = 4 << size_log2. Total transferred bytes = chunk_size, so to transfer N bytes you issue ceil(N / chunk) kicks
    pub size_log2:     u8,
    /// FBIF channel id (0..7)
    pub ctxdma:        u8,
    pub target:        DmaTarget,
    pub dir:           DmaDirection,
    /// True iff the IMEM tag should be updated by the transfer (only meaningful when 'target = Imem'). Required for HS image staging
    pub set_imem_tag:  bool,
}

// PRI bus sentinel
// On Turing, when the host reads a register window that belongs to a gated
// or floor-swept engine, the GPU's PRI hub does NOT return all-ones - it
// returns a fixed '0xBADF_xxxx' value where the low 16 bits encode the
// source/error class. Every register in that window reads back the same
// value, so a naive 'hw != 0 && hw != 0xFFFFFFFF' check incorrectly treats
// gated engines as alive. Filtering this pattern is required
pub const PRI_SENTINEL_MASK: u32 = 0xFFFF_0000;
pub const PRI_SENTINEL_VAL:  u32 = 0xBADF_0000;

/// True iff 'val' looks like a PRI bus sentinel (0xBADF_xxxx)
#[inline]
pub fn is_pri_sentinel(val: u32) -> bool {
    val & PRI_SENTINEL_MASK == PRI_SENTINEL_VAL
}

/// Coarse classification of a Falcon register-window read
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Liveness {
    /// HWCFG decodes to a plausible imem/dmem geometry
    Alive,
    /// Window returns the '0xBADF_xxxx' PRI sentinel: engine is gated
    /// in PMC_ENABLE / NV_PMC_DEVICE_ENABLE or floor-swept
    GatedPriSentinel,
    /// Window returns 0 or all-ones - bus stalled or no decode at all
    NoResponse,
    /// HWCFG is non-sentinel but its imem/dmem fields are zero
    BadHwcfg,
}

// Engine - typed handle over a Falcon at a given BAR0-relative base.

#[derive(Copy, Clone)]
pub struct Engine<'a> {
    bar0: &'a MmioRegion,
    base: u32,
    pub name: &'static str,
}

impl<'a> Engine<'a> {
    pub fn new(bar0: &'a MmioRegion, base: u32, name: &'static str) -> Self {
        Self { bar0, base, name }
    }

    #[inline] pub fn base(&self) -> u32 { self.base }

    /// Access to the underlying BAR0 region for absolute MMIO accesses
    /// (e.g., PMC registers, which are not engine-relative)
    #[inline] pub fn bar0(&self) -> &'a MmioRegion { self.bar0 }

    #[inline]
    pub fn read(&self, off: u32) -> u32 {
        self.bar0.read32(self.base + off)
    }

    #[inline]
    pub fn write(&self, off: u32, val: u32) {
        self.bar0.write32(self.base + off, val);
    }

    /// IMEM size in bytes. HWCFG bits[8:0] count 256-byte blocks
    pub fn imem_size(&self) -> u32 {
        (self.read(FALCON_HWCFG) & 0x1ff) * 256
    }

    /// DMEM size in bytes. HWCFG bits[17:9]
    pub fn dmem_size(&self) -> u32 {
        ((self.read(FALCON_HWCFG) >> 9) & 0x1ff) * 256
    }

    /// True if the falcon CPU is currently halted
    pub fn is_halted(&self) -> bool {
        self.read(FALCON_CPUCTL) & CPUCTL_HALTED != 0
    }

    /// Spin until HALTED bit is set or 'max_spins' elapsed
    pub fn wait_halted(&self, max_spins: u32) -> bool {
        for _ in 0..max_spins {
            if self.is_halted() { return true; }
            core::hint::spin_loop();
        }
        false
    }

    /// Spin until HALTED or timeout_ns of wall-clock has elapsed, measured
    /// by PTIMER (the free-running ns counter at BAR0+0x9400). Returns true
    /// if the engine halted.
    ///
    /// This is the preferred halt poll: an HS falcon either halts within a
    /// few hundred microseconds or never, so a raw spin-count budget of tens
    /// of millions just burns seconds of MMIO reads on a failed boot. Bounding
    /// by real time keeps the wait short and identical across host CPU speeds.
    /// A backstop iteration cap guards the rare case where PTIMER is not
    /// advancing (VBIOS devinit has not run), so this never spins forever.
    pub fn wait_halted_ns(&self, timeout_ns: u64) -> bool {
        const PTIMER_TIME_0: u32 = 0x0000_9400;
        const BACKSTOP_ITERS: u32 = 2_000_000;
        let start = self.bar0.read32(PTIMER_TIME_0);
        let budget = timeout_ns.min(u32::MAX as u64) as u32;
        for _ in 0..BACKSTOP_ITERS {
            if self.is_halted() { return true; }
            let now = self.bar0.read32(PTIMER_TIME_0);
            if now.wrapping_sub(start) >= budget { return false; }
            core::hint::spin_loop();
        }
        false
    }

    /// Read 16 bytes of IMEM starting at byte offset 'dst'. Uses the
    /// same C0/D0 windowed port as 'imem_load' but in AINCR mode.
    /// Returns zeros if the page is SECURE and HSCB blocks host
    /// readback; this is a normal protection feature, not an error.
    /// Useful as a sanity check that our IMEM port writes landed:
    /// reading back the first non-secure dwords should match the bytes
    /// we wrote
    pub fn imem_peek16(&self, dst: u32) -> [u8; 16] {
        let ctrl = (dst & 0x0000_FFFF) | MEM_C_AINCR;
        self.write(FALCON_IMEM_C0, ctrl);
        let mut out = [0u8; 16];
        for i in 0..4 {
            let w = self.read(FALCON_IMEM_D0);
            out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    /// Upload 'data' to IMEM starting at byte offset 'dst'. 'tag' is the
    /// virtual page tag (usually 'dst >> 8') used by the Falcon MMU
    /// 'secure' selects HSCB-mode upload, required for HS images
    /// Returns the number of bytes uploaded (rounded up to 4)
    pub fn imem_load(&self, dst: u32, tag: u32, data: &[u8], secure: bool) -> u32 {
        let mut ctrl = (dst & 0x0000_FFFF) | MEM_C_AINCW;
        if secure { ctrl |= MEM_C_SECURE; }
        self.write(FALCON_IMEM_C0, ctrl);
        self.write(FALCON_IMEM_T0, tag);

        let mut written = 0u32;
        let mut i = 0usize;
        while i + 4 <= data.len() {
            let w = u32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
            self.write(FALCON_IMEM_D0, w);
            i += 4;
            written += 4;
        }
        // Tail-pad with zeros so the IMEM port stays 4-byte aligned
        if i < data.len() {
            let mut tail = [0u8; 4];
            tail[..data.len() - i].copy_from_slice(&data[i..]);
            self.write(FALCON_IMEM_D0, u32::from_le_bytes(tail));
            written += 4;
        }
        written
    }

    /// Upload 'data' to DMEM starting at byte offset 'dst'. Returns bytes
    /// written (rounded up to a 4-byte boundary)
    pub fn dmem_load(&self, dst: u32, data: &[u8]) -> u32 {
        self.write(FALCON_DMEM_C0, (dst & 0x0000_FFFF) | MEM_C_AINCW);
        let mut written = 0u32;
        let mut i = 0usize;
        while i + 4 <= data.len() {
            let w = u32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
            self.write(FALCON_DMEM_D0, w);
            i += 4;
            written += 4;
        }
        if i < data.len() {
            let mut tail = [0u8; 4];
            tail[..data.len() - i].copy_from_slice(&data[i..]);
            self.write(FALCON_DMEM_D0, u32::from_le_bytes(tail));
            written += 4;
        }
        written
    }

    /// Set the boot vector (PC at startup) and start the CPU
    pub fn start_at(&self, bootvec: u32) {
        self.write(FALCON_BOOTVEC, bootvec);
        // Invalidate IMEM tag cache, then start
        self.write(FALCON_CPUCTL, CPUCTL_IINVAL | CPUCTL_STARTCPU);
    }

    /// True iff the Falcon's HWCFG reports a plausible imem/dmem geometry
    /// AND the read did not come back as the PRI sentinel. This is the
    /// strict version: gated engines fail the check
    pub fn is_alive(&self) -> bool {
        matches!(self.liveness(), Liveness::Alive)
    }

    /// Classify the engine register window in one of four states
    /// Useful for diagnostics that need to distinguish gated vs dead
    pub fn liveness(&self) -> Liveness {
        let hw = self.read(FALCON_HWCFG);
        if hw == 0 || hw == 0xFFFF_FFFF {
            Liveness::NoResponse
        } else if is_pri_sentinel(hw) {
            Liveness::GatedPriSentinel
        } else if (hw & 0x1ff) == 0 {
            Liveness::BadHwcfg
        } else {
            Liveness::Alive
        }
    }

    /// Raw HWCFG value, exposed for diagnostic UIs that want to print it
    /// alongside the classification
    #[inline]
    pub fn hwcfg(&self) -> u32 {
        self.read(FALCON_HWCFG)
    }

    // DMA upload path (FBIF + DMATRF)
    //
    // This is the production code path for getting ucode and data into
    // a Falcon. PIO via IMEM_C/DMEM_C works for the few-KB booter HS
    // image (see 'imem_load' above) but is too slow for full GR ctx
    // restore or VRAM scrubbing. Once the caller has:
    //   1) allocated a DMA-visible buffer (sysmem PA or VRAM offset),
    //   2) populated it with the ucode/data,
    //   3) configured an FBIF context-DMA channel for that buffer,
    // they can issue one or more 'dma_load' calls to transfer chunks
    // into IMEM/DMEM with the falcon halted.
    //
    // FBIF setup is intentionally NOT in this module - its register
    // offsets are chip-specific and modelled separately in step (A) of
    // the bring-up plan. This module's contract is: "given a properly
    // configured ctxdma, kick the DMATRF state machine and wait."

    /// Spin until DMATRFCMD.IDLE is set or the budget elapses. A return
    /// value of 'false' indicates the DMA engine is still working -
    /// caller should treat this as a fatal hardware-state error
    pub fn wait_dma_idle(&self, max_spins: u32) -> bool {
        for _ in 0..max_spins {
            if self.read(FALCON_DMATRFCMD) & DMATRFCMD_IDLE != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Issue one Falcon DMA transfer and wait for completion
    ///
    /// Caller invariants (NOT checked here):
    ///     The Falcon CPU is halted (HALTED bit set in CPUCTL).
    ///     'xfer.ctxdma' has been programmed in the engine's FBIF window
    ///     to point at the right aperture (sysmem-coherent / VRAM / etc).
    ///     The buffer at 'xfer.src_phys + xfer.src_off_bytes' is
    ///     readable by the FBIF for at least '4 << xfer.size_log2' bytes
    ///
    /// Returns Ok(()) on a successful idle. The transfer moves exactly
    /// '4 << size_log2' bytes (one chunk); larger uploads need a loop
    pub fn dma_load(&self, xfer: DmaTransfer) -> Result<(), DmaError> {
        if xfer.size_log2 > DMA_SIZE_LOG2_MAX {
            return Err(DmaError::BadSize);
        }
        if xfer.src_phys & (DMA_MIN_ALIGN - 1) != 0 {
            return Err(DmaError::BadAlignment);
        }
        // Drain any prior in-flight DMA before we reprogram the regs
        if !self.wait_dma_idle(1_000_000) {
            return Err(DmaError::Timeout { cmd: self.read(FALCON_DMATRFCMD) });
        }

        // Clear REQUIRE_CTX so the kick isn't gated on a ctx-load that
        // never happens during host bring-up. Without this, DMATRFCMD is
        // silently dropped (no busy, no ERROR, no transfer). Other DMACTL
        // bits (scrubbing-status) are read-only, so a plain mask is safe
        let dmactl = self.read(FALCON_DMACTL);
        self.write(FALCON_DMACTL, dmactl & !DMACTL_REQUIRE_CTX);

        // Hardware encodes the source as 'phys >> 8' (256-byte chunks)
        // Split across BASE (low 32 bits) and BASE1 (upper bits, Turing+)
        let phys_chunks = xfer.src_phys >> 8;
        self.write(FALCON_DMATRFBASE,  phys_chunks as u32);
        self.write(FALCON_DMATRFBASE1, (phys_chunks >> 32) as u32);
        // FBOFFS is also chunk-counted
        self.write(FALCON_DMATRFFBOFFS, xfer.src_off_bytes >> 8);
        // MOFFS is the byte offset inside IMEM/DMEM
        self.write(FALCON_DMATRFMOFFS,  xfer.dst_off_bytes);

        // The kick word: just SIZE + CTXDMA + (optional WRITE/IMEM/SET_DMTAG)
        // Bit 0 (WRITE_OR_FREE) must remain 0 - setting it issues a release
        // sub-command instead of a transfer, which returns IDLE+no-error but
        // never touches IMEM/DMEM. Verified by NVIDIA OpenGPU's load path
        let mut cmd = (((xfer.size_log2 as u32) << DMATRFCMD_SIZE_SHIFT) & DMATRFCMD_SIZE_MASK)
                    | (((xfer.ctxdma as u32) << DMATRFCMD_CTXDMA_SHIFT) & DMATRFCMD_CTXDMA_MASK);
        if matches!(xfer.dir,    DmaDirection::ToFb) { cmd |= DMATRFCMD_WRITE; }
        if matches!(xfer.target, DmaTarget::Imem)    { cmd |= DMATRFCMD_IMEM;  }
        if xfer.set_imem_tag                         { cmd |= DMATRFCMD_SET_IMEM_TAG; }
        self.write(FALCON_DMATRFCMD, cmd);

        if !self.wait_dma_idle(10_000_000) {
            return Err(DmaError::Timeout { cmd: self.read(FALCON_DMATRFCMD) });
        }
        let post = self.read(FALCON_DMATRFCMD);
        if post & DMATRFCMD_ERROR != 0 {
            return Err(DmaError::Hardware { cmd: post });
        }
        Ok(())
    }

    /// Convenience wrapper: load 'total_bytes' worth of data by issuing
    /// repeated 256-byte DMA chunks. Total must be a multiple of 256.
    /// Useful for HS image staging where the image is several KB; for
    /// small transfers (<=256B) call 'dma_load' directly
    pub fn dma_load_blocks(
        &self,
        target: DmaTarget,
        dst_off_bytes: u32,
        src_phys: u64,
        src_off_bytes: u32,
        total_bytes: u32,
        ctxdma: u8,
        set_imem_tag: bool,
    ) -> Result<u32, DmaError> {
        if total_bytes % 256 != 0 {
            return Err(DmaError::BadSize);
        }
        let mut moved = 0u32;
        while moved < total_bytes {
            self.dma_load(DmaTransfer {
                src_phys,
                src_off_bytes: src_off_bytes + moved,
                dst_off_bytes: dst_off_bytes + moved,
                size_log2: DMA_SIZE_LOG2_MAX, // 256-byte chunk
                ctxdma,
                target,
                dir: DmaDirection::ToFalcon,
                set_imem_tag,
            })?;
            moved += 256;
        }
        Ok(moved)
    }
}
