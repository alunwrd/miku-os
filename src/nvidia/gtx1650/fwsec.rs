// FWSEC / FRTS on TU116 - the stage that locks WPR2
//
// On Turing the booter (running on SEC2) refuses to lock WPR2 by itself;
// the FRTS (FW Runtime Security) region must first be carved out by FWSEC,
// a signed Falcon ucode that ships inside the VBIOS (not the firmware/
// bundle). FWSEC runs on the GSP Falcon, reads a command block we patch
// into its DMEM image, and on the FRTS command it programs the WPR2 lock
// registers (NV_PFB_PRI_MMU_WPR2_ADDR_LO/HI) to cover a 1 MiB region just
// below the VGA workspace.
//
// This is the missing piece behind "WPR2 never locked in hardware tests":
// the booter was being asked to lock a region FWSEC had not yet set up.
//
// Authoritative references (cross-checked, not guessed):
//     nouveau drivers/gpu/drm/nouveau/nvkm/subdev/gsp/fwsec.c
//       nvkm_gsp_fwsec_frts / _init / _v2 / _patch
//     nouveau .../gsp/tu102.c  tu102_gsp_fwsec_load_bld  (the v2 bld desc)
//     nouveau .../bios/pmu.c   nvbios_pmuTe / _pmuEp     (VBIOS PMU lookup)
//     nouveau .../falcon/gm200.c gm200_flcn_fw_load / _boot
//     open-gpu-kernel-modules kernel_gsp_frts_tu102.c    (WPR2 verify)
//
// Boot order on Turing (ogkm kgspBootstrap_TU102 / nouveau tu102 oneinit):
//   scrubber -> stage GSP-RM image -> *FWSEC FRTS (locks WPR2)* -> GSP
//   RISC-V reset -> libos boot args -> booter_load on SEC2. This module
//   implements the FWSEC FRTS step.

#![allow(dead_code)]

use crate::nvidia::mmio::MmioRegion;
use crate::nvidia::vbios;
use crate::serial_println;

use super::dma_buf::{DmaBuffer, DmaBufError};
use super::falcon::{
    self, Engine, CPUCTL_HALTED, CPUCTL_STARTCPU, DMACTL_DMEM_SCRUBBING, DMACTL_IMEM_SCRUBBING,
    DMACTL_REQUIRE_CTX, FALCON_BOOTVEC, FALCON_CPUCTL, FALCON_DMACTL, FALCON_DMATRFBASE,
    FALCON_DMATRFBASE1, FALCON_MAILBOX0, FALCON_MAILBOX1,
};
use super::fbif::{self, FbifMemType, FbifTarget};
use super::sec2::HsflcnBlDesc;
use super::tu116_fw::{self, NvfwBinHdr};

// Constants from the references //

/// VBIOS PMU ucode type tag for FWSEC. nouveau nvkm_gsp_fwsec_init scans
/// the PMU table for 'flcn_ucode.type == 0x85'
const PMU_UCODE_TYPE_FWSEC: u8 = 0x85;

/// appif entry id for the DMEM mapper (the one carrying the FRTS command).
/// nouveau NVFW_FALCON_APPIF_ID_DMEMMAPPER
const APPIF_ID_DMEMMAPPER: u32 = 0x0000_0004;

/// init_cmd value selecting the FRTS sub-command in the DMEM mapper.
/// nouveau NVFW_FALCON_APPIF_DMEMMAPPER_CMD_FRTS
const DMEMMAPPER_CMD_FRTS: u32 = 0x0000_0015;

/// FBIF context-DMA slot used for the FWSEC image. nouveau's
/// tu102_gsp_fwsec_load_bld uses FALCON_DMAIDX_PHYS_SYS_NCOH (= 4) and
/// programs TRANSCFG[4] = 0x5 (sysmem, physical). We mirror both: slot 4
/// programmed to sysmem-physical, and ctx_dma = 4 inside the bld descriptor
const FWSEC_CTXDMA: u8 = 4;

/// FRTS region type "FB" (the region lives in VRAM). nouveau
/// NVFW_FRTS_CMD_REGION_TYPE_FB
const FRTS_CMD_REGION_TYPE_FB: u32 = 0x0000_0002;

/// FRTS error scratch register. nouveau verifies FRTS via
/// 'rd32(0x001400 + 0xe*4) >> 16' (the top 16 bits are the error code).
/// 0x1400 + 0x38 = 0x1438 (NV_PGC6_AON_SECURE_SCRATCH_GROUP_03).
const FWSEC_FRTS_ERR_SCRATCH: u32 = 0x0000_1438;

/// Wall-clock halt timeout for FWSEC (via PTIMER). FWSEC does real work
/// (DMA its own image in, run HS, program WPR2) but still completes well
/// under a millisecond; 300 ms is far past any legitimate completion time
const FWSEC_HALT_TIMEOUT_NS: u64 = 300_000_000;

/// Spin budget for the post-reset IMEM/DMEM scrub wait
const SCRUB_SPIN_BUDGET: u32 = 1_000_000;

// Errors //

#[derive(Debug, Copy, Clone)]
pub enum FwsecError {
    /// Could not read or parse the VBIOS expansion ROM
    NoVbios,
    /// BIT header / 'p' (PMU) token not found in the VBIOS
    NoPmuTable,
    /// No PMU entry of type 0x85 (FWSEC) found
    NoFwsecUcode,
    /// The falcon ucode descriptor header was invalid (bit0 clear)
    BadUcodeDesc,
    /// Descriptor version is not 2. v3 (Ampere+/PKC-signed) needs the
    /// VBIOS signature patch path which TU116 does not use; flagged so a
    /// future chip can implement it instead of silently mis-booting
    UnsupportedDescVersion(u8),
    /// The ucode image or appif structures fell outside the VBIOS bounds
    OutOfBounds,
    /// No DMEM-mapper appif entry (cannot inject the FRTS command)
    NoDmemMapper,
    /// 'acr/bl.bin'  did not parse as a valid NVFW container / bl descriptor
    BadBootloader,
    /// GSP Falcon is gated (HWCFG sentinel / zero) - cannot run FWSEC
    EngineGated,
    /// Could not allocate sysmem for the FWSEC image
    Alloc(DmaBufError),
    /// FWSEC did not halt within the timeout
    Timeout,
    /// FWSEC halted but the FRTS error scratch was non-zero
    FrtsError(u16),
}

impl From<DmaBufError> for FwsecError {
    fn from(e: DmaBufError) -> Self { FwsecError::Alloc(e) }
}

// VBIOS PMU table lookup -> FWSEC ucode descriptor offset //

/// Read a little-endian u32 at 'off' in 'rom', bounds-checked
#[inline]
fn rd32(rom: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    if end > rom.len() { return None; }
    Some(u32::from_le_bytes([rom[off], rom[off + 1], rom[off + 2], rom[off + 3]]))
}

#[inline]
fn rd8(rom: &[u8], off: usize) -> Option<u8> {
    rom.get(off).copied()
}


/// Locate the FWSEC falcon ucode descriptor inside the VBIOS.
///
/// Mirrors nouveau's chain:
///   bit_entry('p') -> PMU table pointer (u32 at token offset 0)
///   PMU table header: ver@0, hdr@1, len@2, cnt@3
///   entry[i] at table + hdr + i*len:  type@0 (u8), data@2 (u32)
///   pick the entry with type == 0x85, return its 'data' (a VBIOS offset
///   to the falcon ucode descriptor).
///
/// The PMU table and the ucode descriptor live in the VBIOS data image, so
/// every dereference goes through 'VbiosView', which reproduces nouveau's
/// 'nvbios_addr' remap. Returns the descriptor's raw ROM offset (already
/// translated), from which the descriptor and ucode image are contiguous.
pub fn find_fwsec_ucode_offset(rom: &[u8]) -> Result<usize, FwsecError> {
    let hdr = match vbios::find_bit_header(rom) {
        Some(h) => h,
        None => {
            crate::print_warn!("    [fwsec] no BIT header in VBIOS");
            return Err(FwsecError::NoPmuTable);
        }
    };
    let tokens = vbios::parse_tokens(rom, &hdr);
    crate::println!(
        "    [fwsec] BIT @ {:#x}: {} tokens (has 'p'={})",
        hdr.offset, tokens.len(),
        vbios::find_token(&tokens, b'p').is_some()
    );
    // Image map: BIT pointers are based at the start of the image that
    // holds them. If the PMU pointer falls outside that image, the base
    // assumption (ROM offset 0) is wrong - this dump makes that visible
    for (off, sz, ct, last) in vbios::image_map(rom) {
        crate::println!(
            "    [fwsec] image @ {:#x} size={:#x} code_type={:#x} last={}",
            off, sz, ct, last
        );
    }

    // The 'p' token (data version 2) holds, at its data pointer, a u32
    // pointing at the PMU table. nouveau: bit_p.version == 2, length >= 4
    let p = vbios::find_token(&tokens, b'p').ok_or(FwsecError::NoPmuTable)?;
    if p.data_version != 2 || p.data_size < 4 {
        serial_println!(
            "[fwsec] 'p' token v{} size {} - expected v2 size>=4",
            p.data_version, p.data_size
        );
        crate::print_warn!(
            "    [fwsec] 'p' token v{} size {} (expected v2 size>=4)",
            p.data_version, p.data_size
        );
        return Err(FwsecError::NoPmuTable);
    }
    // NVIDIA BIT pointers live in a virtual space: anything at or beyond the
    // legacy image (image[0]) is remapped into the data image (PCIR type
    // 0xe0). The PMU table pointer lands there, so all dereferences below go
    // through the translating view; reading the raw ROM flat lands in the
    // legacy image's padding and yields garbage (the original bug here).
    let view = vbios::VbiosView::new(rom);
    crate::println!(
        "    [fwsec] view: image0_size={:#x} imaged_addr={:#x}",
        view.image0_size, view.imaged_addr
    );

    let pmu_table = view.rd32(p.data_ptr as usize).ok_or(FwsecError::OutOfBounds)? as usize;
    crate::println!(
        "    [fwsec] 'p' v{} size {} data_ptr={:#x} -> pmu_table={:#x} (phys {:#x})",
        p.data_version, p.data_size, p.data_ptr, pmu_table, view.phys(pmu_table)
    );
    if pmu_table == 0 {
        return Err(FwsecError::NoPmuTable);
    }

    let _ver = view.rd8(pmu_table).ok_or(FwsecError::OutOfBounds)?;
    let thdr = view.rd8(pmu_table + 1).ok_or(FwsecError::OutOfBounds)? as usize;
    let tlen = view.rd8(pmu_table + 2).ok_or(FwsecError::OutOfBounds)? as usize;
    let tcnt = view.rd8(pmu_table + 3).ok_or(FwsecError::OutOfBounds)? as usize;
    serial_println!(
        "[fwsec] PMU table @ {:#x}: hdr={} len={} cnt={}",
        pmu_table, thdr, tlen, tcnt
    );
    crate::println!(
        "    [fwsec] PMU table @ {:#x}: hdr={} len={} cnt={}",
        pmu_table, thdr, tlen, tcnt
    );
    if tlen < 6 {
        return Err(FwsecError::NoPmuTable);
    }

    for i in 0..tcnt {
        let entry = pmu_table + thdr + i * tlen;
        let etype = view.rd8(entry).ok_or(FwsecError::OutOfBounds)?;
        if etype == PMU_UCODE_TYPE_FWSEC {
            // 'data'  is a virtual VBIOS address; translate to a raw ROM
            // offset so the descriptor + ucode image (contiguous in the data
            // image) can be read flat from there
            let data = view.rd32(entry + 2).ok_or(FwsecError::OutOfBounds)? as usize;
            let phys = view.phys(data);
            serial_println!(
                "[fwsec] FWSEC ucode (type {:#x}) at PMU entry {}: desc @ {:#x} (phys {:#x})",
                PMU_UCODE_TYPE_FWSEC, i, data, phys
            );
            crate::println!(
                "    [fwsec] FWSEC ucode (type 0x85) entry {}: desc @ {:#x} (phys {:#x})",
                i, data, phys
            );
            if data == 0 || phys >= rom.len() {
                return Err(FwsecError::OutOfBounds);
            }
            return Ok(phys);
        }
    }
    Err(FwsecError::NoFwsecUcode)
}

// NEW Falcon ucode descriptor (Turing FWSEC)
//
// nouveau 'struct nvkm_falcon_ucode_desc_v2' (union nvfw_falcon_ucode_desc).
// Header word: bit0 = valid, bits[15:8] = version, bits[31:16] = desc size.
// The ucode image follows the descriptor at 'desc_offset + desc_size'

#[derive(Copy, Clone, Debug)]
pub struct UcodeDescV2 {
    /// Byte offset of this descriptor within the VBIOS
    pub desc_offset: usize,
    /// Descriptor size in bytes (image starts at desc_offset + desc_size)
    pub desc_size: u32,
    pub interface_offset: u32,
    pub imem_phys_base: u32,
    pub imem_load_size: u32,
    pub imem_sec_base: u32,
    pub imem_sec_size: u32,
    pub dmem_offset: u32,
    pub dmem_phys_base: u32,
    pub dmem_load_size: u32,
}

impl UcodeDescV2 {
    /// Parse the descriptor at 'off' in 'rom'. Returns the parsed v2 view,
    /// or an error if the header is invalid or the version is not 2
    pub fn parse(rom: &[u8], off: usize) -> Result<Self, FwsecError> {
        let hdr = rd32(rom, off).ok_or(FwsecError::OutOfBounds)?;
        if hdr & 0x1 == 0 {
            return Err(FwsecError::BadUcodeDesc);
        }
        let version = ((hdr >> 8) & 0xff) as u8;
        let desc_size = (hdr >> 16) & 0xffff;
        if version != 2 {
            return Err(FwsecError::UnsupportedDescVersion(version));
        }
        // u32 field indices into the v2 descriptor
        let w = |i: usize| rd32(rom, off + i * 4).ok_or(FwsecError::OutOfBounds);
        Ok(UcodeDescV2 {
            desc_offset: off,
            desc_size,
            interface_offset: w(4)?,
            imem_phys_base: w(5)?,
            imem_load_size: w(6)?,
            imem_sec_base: w(8)?,
            imem_sec_size: w(9)?,
            dmem_offset: w(10)?,
            dmem_phys_base: w(11)?,
            dmem_load_size: w(12)?,
        })
    }

    /// Total ucode image length = IMEM + DMEM load sizes
    #[inline]
    pub fn image_len(&self) -> usize {
        self.imem_load_size as usize + self.dmem_load_size as usize
    }

    /// Byte offset of the ucode image within the VBIOS
    #[inline]
    pub fn image_offset(&self) -> usize {
        self.desc_offset + self.desc_size as usize
    }
}

// FWSEC bootloader descriptor (flcn_bl_dmem_desc_v2, inline u64 form)
//
// This is NOT the same layout as the booter's 'FlcnBlDmemDesc' in sec2.rs.
// nouveau's 'struct flcn_bl_dmem_desc_v2' stores code_dma_base and
// data_dma_base as inline u64s (full physical addresses, not >>8), with no
// trailing code_dma_base1/data_dma_base1 split. tu102_gsp_fwsec_load_bld
// fills it and writes it to the falcon DMEM at offset 0.
//
// Layout (84 bytes):
//   reserved[4], signature[4], ctx_dma(u32), code_dma_base(u64),
//   non_sec_code_off, non_sec_code_size, sec_code_off, sec_code_size,
//   code_entry_point, data_dma_base(u64), data_size, argc, argv

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct FwsecBlDmemDescV2 {
    pub reserved: [u32; 4],
    pub signature: [u32; 4],
    pub ctx_dma: u32,
    pub code_dma_base: u64,
    pub non_sec_code_off: u32,
    pub non_sec_code_size: u32,
    pub sec_code_off: u32,
    pub sec_code_size: u32,
    pub code_entry_point: u32,
    pub data_dma_base: u64,
    pub data_size: u32,
    pub argc: u32,
    pub argv: u32,
}

impl FwsecBlDmemDescV2 {
    pub const SIZE: usize = 84;

    /// Build the descriptor for a FWSEC image staged at 'image_phys'.
    /// Mirrors tu102_gsp_fwsec_load_bld exactly: full physical addresses in
    /// the u64 base fields, code/data section geometry from the ucode desc.
    /// FWSEC carries its signature inside its own image, so 'signature' here
    /// stays zero (unlike the HS booter path which patches a prod sig in).
    pub fn build(desc: &UcodeDescV2, image_phys: u64) -> Self {
        // nmem (non-secure imem) = IMEMLoadSize - IMEMSecSize, based at
        // IMEMPhysBase; imem (secure) = IMEMSecSize based at IMEMSecBase
        let non_sec_code_size = desc.imem_load_size.saturating_sub(desc.imem_sec_size);
        FwsecBlDmemDescV2 {
            reserved: [0; 4],
            signature: [0; 4],
            ctx_dma: FWSEC_CTXDMA as u32,
            code_dma_base: image_phys,
            non_sec_code_off: desc.imem_phys_base,
            non_sec_code_size,
            sec_code_off: desc.imem_sec_base,
            sec_code_size: desc.imem_sec_size,
            code_entry_point: 0,
            data_dma_base: image_phys + desc.dmem_offset as u64,
            data_size: desc.dmem_load_size,
            argc: 0,
            argv: 0,
        }
    }

    /// Serialize to 84 bytes for 'Engine::dmem_load', matching the packed
    /// C layout (the u64 base fields sit immediately after their preceding
    /// u32, with no extra alignment padding)
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let mut cur = 0usize;
        let mut put = |bytes: &[u8]| {
            out[cur..cur + bytes.len()].copy_from_slice(bytes);
            cur += bytes.len();
        };
        for r in self.reserved { put(&r.to_le_bytes()); }
        for s in self.signature { put(&s.to_le_bytes()); }
        put(&self.ctx_dma.to_le_bytes());
        put(&self.code_dma_base.to_le_bytes());        // u64
        put(&self.non_sec_code_off.to_le_bytes());
        put(&self.non_sec_code_size.to_le_bytes());
        put(&self.sec_code_off.to_le_bytes());
        put(&self.sec_code_size.to_le_bytes());
        put(&self.code_entry_point.to_le_bytes());
        put(&self.data_dma_base.to_le_bytes());        // u64
        put(&self.data_size.to_le_bytes());
        put(&self.argc.to_le_bytes());
        put(&self.argv.to_le_bytes());
        debug_assert_eq!(cur, Self::SIZE);
        out
    }
}

// FRTS command injection into the FWSEC DMEM image //

#[inline]
fn write_img32(img: &mut [u8], off: usize, v: u32) -> Result<(), FwsecError> {
    let end = off.checked_add(4).ok_or(FwsecError::OutOfBounds)?;
    if end > img.len() {
        return Err(FwsecError::OutOfBounds);
    }
    img[off..end].copy_from_slice(&v.to_le_bytes());
    Ok(())
}

#[inline]
fn read_img32(img: &[u8], o: usize) -> Option<u32> {
    let e = o.checked_add(4)?;
    if e > img.len() { return None; }
    Some(u32::from_le_bytes([img[o], img[o + 1], img[o + 2], img[o + 3]]))
}

/// Patch the FWSEC image (in our staged sysmem buffer) so that, when FWSEC
/// runs, its DMEM mapper executes the FRTS command for the given region.
///
/// Mirrors nouveau nvkm_gsp_fwsec_patch: walk the appif header at
/// 'dmem_base_img + interface_offset', find the DMEM-mapper entry, set its
/// init_cmd to FRTS, then fill the command-in buffer with read_vbios +
/// frts_region (the region addr/size are encoded as '>> 12', 4 KiB units).
///
/// 'img' is the staged image (IMEM||DMEM). 'frts_addr'/'frts_size' are the
/// VRAM byte address and size of the FRTS region from the WPR layout.
fn patch_frts_command(
    img: &mut [u8],
    desc: &UcodeDescV2,
    frts_addr: u64,
    frts_size: u64,
) -> Result<(), FwsecError> {
    let dmem_base = desc.dmem_offset as usize;
    let if_off = desc.interface_offset as usize;
    let hdr_off = dmem_base.checked_add(if_off).ok_or(FwsecError::OutOfBounds)?;

    // appif header v1: ver@0, hdr@1, len@2, cnt@3
    let ver = *img.get(hdr_off).ok_or(FwsecError::OutOfBounds)?;
    if ver != 1 {
        serial_println!("[fwsec] appif header version {} (expected 1)", ver);
        return Err(FwsecError::NoDmemMapper);
    }
    let app_hdr = *img.get(hdr_off + 1).ok_or(FwsecError::OutOfBounds)? as usize;
    let app_len = *img.get(hdr_off + 2).ok_or(FwsecError::OutOfBounds)? as usize;
    let app_cnt = *img.get(hdr_off + 3).ok_or(FwsecError::OutOfBounds)? as usize;
    if app_len < 8 {
        return Err(FwsecError::NoDmemMapper);
    }

    for i in 0..app_cnt {
        let app = hdr_off + app_hdr + i * app_len;
        let id = read_img32(img, app).ok_or(FwsecError::OutOfBounds)?;
        if id != APPIF_ID_DMEMMAPPER {
            continue;
        }
        let dmem_app_base = read_img32(img, app + 4).ok_or(FwsecError::OutOfBounds)? as usize;
        let dmemmap = dmem_base.checked_add(dmem_app_base).ok_or(FwsecError::OutOfBounds)?;

        // dmemmapper v3: init_cmd is at offset 0x2c, cmd_in_buffer_offset
        // at offset 0x08 (both relative to the DMEM section start)
        let cmd_in = read_img32(img, dmemmap + 0x08).ok_or(FwsecError::OutOfBounds)? as usize;

        // init_cmd <- FRTS
        write_img32(img, dmemmap + 0x2c, DMEMMAPPER_CMD_FRTS)?;

        // command-in buffer: read_vbios (24 B) then frts_region (20 B)
        let cmd = dmem_base.checked_add(cmd_in).ok_or(FwsecError::OutOfBounds)?;
        // read_vbios { ver=1, hdr=24, addr(u64)=0, size=0, flags=2 }
        write_img32(img, cmd + 0x00, 1)?;        // ver
        write_img32(img, cmd + 0x04, 24)?;       // hdr = sizeof(read_vbios)
        write_img32(img, cmd + 0x08, 0)?;        // addr lo
        write_img32(img, cmd + 0x0c, 0)?;        // addr hi
        write_img32(img, cmd + 0x10, 0)?;        // size
        write_img32(img, cmd + 0x14, 2)?;        // flags
        // frts_region { ver=1, hdr=20, addr>>12, size>>12, type=FB } @ +24
        write_img32(img, cmd + 0x18, 1)?;        // ver
        write_img32(img, cmd + 0x1c, 20)?;       // hdr = sizeof(frts_region)
        write_img32(img, cmd + 0x20, (frts_addr >> 12) as u32)?;
        write_img32(img, cmd + 0x24, (frts_size >> 12) as u32)?;
        write_img32(img, cmd + 0x28, FRTS_CMD_REGION_TYPE_FB)?;

        serial_println!(
            "[fwsec] FRTS command patched: dmemmap@{:#x} cmd_in@{:#x} region addr={:#x} size={:#x} (4K units {:#x}/{:#x})",
            dmemmap, cmd, frts_addr, frts_size, frts_addr >> 12, frts_size >> 12
        );
        return Ok(());
    }
    Err(FwsecError::NoDmemMapper)
}

// GSP Falcon reset (PMC gate cycle) //

const NV_PMC_ENABLE: u32 = 0x0000_0200;

/// Reset the GSP Falcon via the PMC gate cycle, the same proven path SEC2
/// uses (FALCON_ENGINE / CPUCTL alone are priv-protected on Turing). Uses
/// the chip's GSP PMC mask from quirks. Then waits for the post-reset
/// IMEM/DMEM scrub to finish and clears REQUIRE_CTX so the bl can DMA.
fn reset_gsp_falcon(gsp: &Engine) -> bool {
    use super::falcon::{FALCON_ENGINE, FALCON_ENGINE_RESET};
    use super::quirks;
    const CPUCTL_HRESET: u32 = 1 << 3;

    let bar0 = gsp.bar0();
    let pmc_mask = match quirks::detect(bar0) {
        Some(q) => q.gsp_pmc_reset_mask,
        None => {
            crate::println!("  WARN: unknown chip, using default GSP PMC mask 0x2000");
            0x0000_2000
        }
    };

    let pmc_cur = bar0.read32(NV_PMC_ENABLE);
    bar0.write32(NV_PMC_ENABLE, pmc_cur & !pmc_mask);
    for _ in 0..10_000 { core::hint::spin_loop(); }
    bar0.write32(NV_PMC_ENABLE, pmc_cur | pmc_mask);
    for _ in 0..100_000 { core::hint::spin_loop(); }

    gsp.write(FALCON_ENGINE, FALCON_ENGINE_RESET);
    for _ in 0..1_000 { let _ = gsp.read(FALCON_ENGINE); core::hint::spin_loop(); }
    gsp.write(FALCON_ENGINE, 0);

    gsp.write(FALCON_CPUCTL, CPUCTL_HRESET);
    for _ in 0..10_000 { let _ = gsp.read(FALCON_CPUCTL); core::hint::spin_loop(); }

    for _ in 0..100_000 {
        let c = gsp.read(FALCON_CPUCTL);
        if c & (CPUCTL_HALTED | CPUCTL_HRESET) == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Wait until IMEM/DMEM scrubbing completes after a reset. Host writes to
/// IMEM/DMEM are dropped while scrubbing is in progress
fn wait_scrub_done(gsp: &Engine) -> bool {
    let mask = DMACTL_DMEM_SCRUBBING | DMACTL_IMEM_SCRUBBING;
    for _ in 0..SCRUB_SPIN_BUDGET {
        if gsp.read(FALCON_DMACTL) & mask == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

// Result + orchestration

#[derive(Debug, Copy, Clone)]
pub struct FrtsStatus {
    /// MAILBOX0 at halt (FWSEC convention: 0 = ok)
    pub mb0: u32,
    pub mb1: u32,
    pub cpuctl: u32,
    /// Top 16 bits of the FRTS error scratch (0 = success)
    pub frts_err: u16,
    /// True iff PFB reports WPR2 locked after FWSEC ran
    pub wpr2_locked: bool,
    pub wpr2_lo: u64,
    pub wpr2_hi: u64,
}

fn pages_for(n: usize) -> usize { (n + 4095) / 4096 }

/// Run FWSEC with the FRTS command to lock WPR2 for the region at
/// 'frts_addr'/'frts_size' (from the GSP WPR layout). 'rom' is the VBIOS
/// image (full expansion ROM). The GSP Falcon must be alive (devinit run).
///
/// On success WPR2 is locked and 'FrtsStatus.wpr2_locked' is true. The
/// staged image buffer is dropped on return (FWSEC has consumed it).
pub fn run_frts(
    bar0: &MmioRegion,
    rom: &[u8],
    frts_addr: u64,
    frts_size: u64,
) -> Result<FrtsStatus, FwsecError> {
    // 1) Locate + parse the FWSEC ucode descriptor in the VBIOS
    let desc_off = find_fwsec_ucode_offset(rom)?;
    let desc = UcodeDescV2::parse(rom, desc_off)?;
    serial_println!(
        "[fwsec] desc v2 @ {:#x}: size={:#x} if_off={:#x} imem[load={:#x} sec_base={:#x} sec_size={:#x} phys_base={:#x}] dmem[off={:#x} load={:#x} phys_base={:#x}]",
        desc.desc_offset, desc.desc_size, desc.interface_offset,
        desc.imem_load_size, desc.imem_sec_base, desc.imem_sec_size, desc.imem_phys_base,
        desc.dmem_offset, desc.dmem_load_size, desc.dmem_phys_base
    );

    crate::println!(
        "    [fwsec] desc v2 @ {:#x}: size={:#x} imem_load={:#x} dmem_off={:#x} dmem_load={:#x}",
        desc.desc_offset, desc.desc_size, desc.imem_load_size, desc.dmem_offset, desc.dmem_load_size
    );

    let img_off = desc.image_offset();
    let img_len = desc.image_len();
    let img_end = img_off.checked_add(img_len).ok_or(FwsecError::OutOfBounds)?;
    if img_end > rom.len() {
        serial_println!(
            "[fwsec] ucode image [{:#x}..{:#x}] exceeds VBIOS ({} bytes)",
            img_off, img_end, rom.len()
        );
        crate::print_warn!(
            "    [fwsec] ucode image [{:#x}..{:#x}] exceeds VBIOS ({} bytes)",
            img_off, img_end, rom.len()
        );
        return Err(FwsecError::OutOfBounds);
    }
    crate::println!("    [fwsec] ucode image [{:#x}..{:#x}] in VBIOS, staging...", img_off, img_end);

    // 2) Stage the ucode image (IMEM||DMEM) in phys-contiguous sysmem and
    //    patch the FRTS command into its DMEM section
    let pages = pages_for(img_len).max(1);
    let mut buf = DmaBuffer::alloc(pages)?;
    buf.zero();
    buf.as_mut_slice()[..img_len].copy_from_slice(&rom[img_off..img_end]);
    patch_frts_command(&mut buf.as_mut_slice()[..img_len], &desc, frts_addr, frts_size)?;
    DmaBuffer::write_barrier();
    let img_phys = buf.phys();
    serial_println!(
        "[fwsec] FWSEC image staged @ {:#x} ({} bytes, {} pages)",
        img_phys, img_len, pages
    );

    // 3) Bring up the GSP Falcon and load the generic acr/bl bootloader,
    //    which will DMA the FWSEC image in, verify it, and run it
    let gsp = Engine::new(bar0, falcon::PGSP_BASE, "gsp");
    if !gsp.is_alive() {
        serial_println!(
            "[fwsec] GSP engine gated: HWCFG={:#x} - aborting",
            gsp.read(falcon::FALCON_HWCFG)
        );
        crate::print_warn!("    [fwsec] GSP engine gated (HWCFG={:#x})", gsp.read(falcon::FALCON_HWCFG));
        return Err(FwsecError::EngineGated);
    }
    let imem = gsp.imem_size();
    let dmem = gsp.dmem_size();
    serial_println!("[fwsec] GSP falcon alive: imem={}B dmem={}B", imem, dmem);

    if !reset_gsp_falcon(&gsp) {
        serial_println!("[fwsec] warn: GSP reset did not complete; proceeding");
    }
    if !wait_scrub_done(&gsp) {
        serial_println!(
            "[fwsec] warn: scrub not done (DMACTL={:#010x})", gsp.read(FALCON_DMACTL)
        );
    }
    let dmactl_pre = gsp.read(FALCON_DMACTL);
    gsp.write(FALCON_DMACTL, dmactl_pre & !DMACTL_REQUIRE_CTX);

    // FBIF slot 4 -> sysmem, physical (TRANSCFG = 0x5 per nouveau). Also
    // allow phys DMA without a bound instance block (host-driven load)
    let prev_ctl = gsp.read(fbif::FBIF_CTL_OFFSET);
    let transcfg = fbif::program_transcfg(
        &gsp, FWSEC_CTXDMA, FbifTarget::CoherentSysmem, FbifMemType::Physical,
    );
    gsp.write(fbif::FBIF_CTL_OFFSET, prev_ctl | fbif::FBIF_CTL_ALLOW_PHYS_NO_CTX);
    serial_println!(
        "[fwsec] FBIF ctx{} = {:#010x} (sysmem phys), CTL {:#010x} -> {:#010x}",
        FWSEC_CTXDMA, transcfg, prev_ctl, gsp.read(fbif::FBIF_CTL_OFFSET)
    );

    // 4) Parse the generic acr/bl bootloader (NVFW container + bl desc)
    let acr_bl_fw = tu116_fw::acr_bl();
    let bl = acr_bl_fw.bytes();
    let bl_hdr = NvfwBinHdr::parse(bl).ok_or(FwsecError::BadBootloader)?;
    let bl_payload = bl_hdr.data(bl);
    let bl_desc = HsflcnBlDesc::parse(
        bl, bl_hdr.header_offset as usize, bl_hdr.data_offset, bl_hdr.data_size,
    ).ok_or(FwsecError::BadBootloader)?;
    let bl_code = &bl_payload[bl_desc.bl_code_off as usize
        ..(bl_desc.bl_code_off + bl_desc.bl_code_size) as usize];
    serial_println!(
        "[fwsec] acr/bl: start_tag={:#x} code[{:#x}+{:#x}]",
        bl_desc.bl_start_tag, bl_desc.bl_code_off, bl_desc.bl_code_size
    );

    // 5) Build + load the v2 bld descriptor at DMEM offset 0
    let bld = FwsecBlDmemDescV2::build(&desc, img_phys);
    let bld_bytes = bld.to_bytes();
    gsp.dmem_load(0, &bld_bytes);
    serial_println!(
        "[fwsec] bld v2 -> DMEM@0: ctx_dma={} code_base={:#x} non_sec[{:#x}+{:#x}] sec[{:#x}+{:#x}] data_base={:#x} data_size={:#x}",
        bld.ctx_dma, bld.code_dma_base, bld.non_sec_code_off, bld.non_sec_code_size,
        bld.sec_code_off, bld.sec_code_size, bld.data_dma_base, bld.data_size
    );

    // Generic bl code at the top of IMEM (non-secure), per gm200_flcn_fw_load
    // (falcon->code.limit - boot_size), tag = boot_addr >> 8 = start_tag
    let bl_size_aligned = (bl_desc.bl_code_size + 0xFF) & !0xFF;
    let dst = imem.saturating_sub(bl_size_aligned) & !0xFF;
    gsp.imem_load(dst, bl_desc.bl_start_tag, bl_code, /*secure=*/ false);
    serial_println!("[fwsec] acr/bl code -> IMEM@{:#x} (tag={:#x})", dst, bl_desc.bl_start_tag);

    // Pre-program the DMA base so the bl can pull the FWSEC image
    gsp.write(FALCON_DMATRFBASE, (img_phys >> 8) as u32);
    gsp.write(FALCON_DMATRFBASE1, ((img_phys >> 40) & 0xFFFF) as u32);

    // 6) Kick. FWSEC takes mbox0 = 0 (input); success convention mbox0 == 0.
    //    bootvec = boot_addr = start_tag << 8 (gm200_flcn_fw_boot)
    gsp.write(FALCON_MAILBOX0, 0);
    gsp.write(FALCON_MAILBOX1, 0);
    let bootvec = bl_desc.bl_start_tag << 8;
    gsp.write(FALCON_BOOTVEC, bootvec);
    serial_println!("[fwsec] CPUCTL kick: bootvec={:#x}, polling for halt...", bootvec);
    gsp.write(FALCON_CPUCTL, CPUCTL_STARTCPU);

    if !gsp.wait_halted_ns(FWSEC_HALT_TIMEOUT_NS) {
        let cpuctl = gsp.read(FALCON_CPUCTL);
        serial_println!(
            "[fwsec] TIMEOUT: cpuctl={:#010x} mb0={:#010x} mb1={:#010x}",
            cpuctl, gsp.read(FALCON_MAILBOX0), gsp.read(FALCON_MAILBOX1)
        );
        crate::print_warn!("    [fwsec] TIMEOUT waiting for halt (cpuctl={:#x})", cpuctl);
        reset_gsp_falcon(&gsp);
        drop(buf);
        return Err(FwsecError::Timeout);
    }

    let mb0 = gsp.read(FALCON_MAILBOX0);
    let mb1 = gsp.read(FALCON_MAILBOX1);
    let cpuctl = gsp.read(FALCON_CPUCTL);
    let frts_err = (bar0.read32(FWSEC_FRTS_ERR_SCRATCH) >> 16) as u16;
    serial_println!(
        "[fwsec] halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x} frts_err={:#06x}",
        mb0, mb1, cpuctl, frts_err
    );
    crate::println!(
        "    [fwsec] halted: mb0={:#x} mb1={:#x} frts_err={:#06x}", mb0, mb1, frts_err
    );

    // 7) Read the WPR2 lock window FWSEC should have programmed
    use super::regs::{decode_wpr_addr, PFB_PRI_MMU_WPR2_ADDR_HI, PFB_PRI_MMU_WPR2_ADDR_LO};
    let wpr2_lo = decode_wpr_addr(bar0.read32(PFB_PRI_MMU_WPR2_ADDR_LO));
    let wpr2_hi = decode_wpr_addr(bar0.read32(PFB_PRI_MMU_WPR2_ADDR_HI));
    let wpr2_locked = wpr2_lo != 0 && wpr2_lo <= wpr2_hi;
    if wpr2_locked {
        serial_println!(
            "[fwsec] WPR2 LOCKED: {:#x}..{:#x} ({} MiB)",
            wpr2_lo, wpr2_hi, (wpr2_hi.saturating_sub(wpr2_lo)) >> 20
        );
    } else {
        serial_println!("[fwsec] WPR2 not locked (lo={:#x} hi={:#x})", wpr2_lo, wpr2_hi);
    }

    drop(buf);

    if frts_err != 0 {
        return Err(FwsecError::FrtsError(frts_err));
    }
    Ok(FrtsStatus { mb0, mb1, cpuctl, frts_err, wpr2_locked, wpr2_lo, wpr2_hi })
}
