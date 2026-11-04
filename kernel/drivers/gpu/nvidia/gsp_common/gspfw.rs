// Firmware containers the GSP understands, independent of which chip boots it
//
// Turing gets GSP-RM running through SEC2/booter, Blackwell through FSP and a
// chain-of-trust message, but both consume the same three things: the
// `gsp-<line>.bin` ELF64 container, the radix3 page table the bootloader walks
// to find the image, and the 256-byte WPR meta block the booter validates.
//
// Field layouts and constants come from nouveau (include/nvfw/gsp.h,
// nvkm/subdev/gsp/r535.c) and NVIDIA's open-gpu-kernel-modules GSP-RM ABI
// headers.

use crate::drivers::gpu::nvidia::le;
use core::mem::size_of;

use crate::mm::dma::{DmaBuffer, DmaBufError};

const PAGE_SIZE: u64 = 4096;
const PTES_PER_PAGE: usize = (PAGE_SIZE as usize) / 8; // 512 u64 entries

/// What can go wrong building the structures in this module
#[derive(Debug, Copy, Clone)]
pub enum GspFwError {
    /// A sysmem allocation for a page-table level or the meta block failed.
    /// Carries the underlying allocator error so the page count survives
    Alloc(DmaBufError),
    /// The caller handed in an empty or oversized page list
    BadPageList,
}

impl From<DmaBufError> for GspFwError {
    fn from(e: DmaBufError) -> Self { GspFwError::Alloc(e) }
}

// WPR meta block (version 1, r535 line) //

/// Magic the GSP booter checks at the head of the WPR-meta block
pub const GSP_FW_WPR_META_MAGIC: u64 = 0xdc3a_ae21_371a_60b3;
/// Revision of the WPR-meta layout this code targets
pub const GSP_FW_WPR_META_REVISION: u64 = 1;

/// 'verified' sentinel the booter writes once it has validated and locked the
/// meta block in WPR2. Part of the ABI
#[allow(dead_code)]
pub const GSP_FW_WPR_META_VERIFIED: u64 = 0xa0a0_a0a0_a0a0_a0a0;

/// The full version-1 WPR meta block, byte-exact with 'GspFwWprMeta' from
/// open-gpu-kernel-modules
/// (src/nvidia/arch/nvalloc/common/inc/gsp/gsp_fw_wpr_meta.h, 615.78.08).
///
/// Exactly 256 bytes and '#[repr(C)]'; the booter DMAs it from sysmem (address
/// handed in SEC2 MAILBOX0/1), validates the magic + revision, then locks it
/// into WPR2 and stamps 'verified'. The two unions in the C original are
/// flattened to the variants used here:
///     union 1 -> sysmem_addr_of_signature / size_of_signature
///     union 2 -> the CrashCat queue (`sysmemAddrOfCrashReportQueue`), over the bytes of the partition-RPC and ELF fields, which only a partition bootstrapper patches in
/// Every reserved/trailing field is present, for the 256-byte layout the booter
/// checksums
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct GspFwWprMeta {
    pub magic: u64,
    pub revision: u64,
    // data in SYSMEM (consumed by Booter for DMA)
    pub sysmem_addr_of_radix3_elf: u64,
    pub size_of_radix3_elf: u64,
    pub sysmem_addr_of_bootloader: u64,
    pub size_of_bootloader: u64,
    pub bootloader_code_offset: u64,
    pub bootloader_data_offset: u64,
    pub bootloader_manifest_offset: u64,
    // union 1 (initial-boot variant): signature
    pub sysmem_addr_of_signature: u64,
    pub size_of_signature: u64,
    // FB layout
    pub gsp_fw_rsvd_start: u64,
    pub non_wpr_heap_offset: u64,
    pub non_wpr_heap_size: u64,
    pub gsp_fw_wpr_start: u64,
    pub gsp_fw_heap_offset: u64,
    pub gsp_fw_heap_size: u64,
    pub gsp_fw_offset: u64,
    pub boot_bin_offset: u64,
    pub frts_offset: u64,
    pub frts_size: u64,
    pub gsp_fw_wpr_end: u64,
    pub fb_size: u64,
    pub vga_workspace_offset: u64,
    pub vga_workspace_size: u64,
    pub boot_count: u64,
    // union 2, CrashCat variant: 16 bytes over partitionRpcAddr and its two
    // offsets and elfCodeOffset, then the queue, then lsUcodeVersion's place.
    // Zero means no queue; Turing never sets it
    pub partition_rpc_padding: [u32; 4],
    pub sysmem_addr_of_crash_report_queue: u64,
    pub size_of_crash_report_queue: u32,
    pub ls_ucode_version_padding: u32,
    // trailing
    pub gsp_fw_heap_vf_partition_count: u8,
    pub flags: u8,
    /// `NV_REG_STR_RM_GSP_PAGING_CONFIG` (615, where 570 had two bytes of
    /// padding); 0 is OpenRM's default, nothing paged
    pub paging_config: u16,
    pub pmu_reserved_size: u32,
    /// 0 = unverified; GSP_FW_WPR_META_VERIFIED once the booter accepts it
    pub verified: u64,
}

// Compile-time guarantee that the layout matches the 256-byte ABI block
const _: () = assert!(size_of::<GspFwWprMeta>() == 256);
// And the CrashCat queue where `gsp_fw_wpr_meta.h` puts it: union 2 starts at
// 208, the queue 16 bytes in
const _: () = assert!(core::mem::offset_of!(GspFwWprMeta, sysmem_addr_of_crash_report_queue) == 224);
// 615's paging config and the PMU size after it (gcc, `dev/gsp-615.md` section
// 5)
const _: () = assert!(core::mem::offset_of!(GspFwWprMeta, paging_config) == 242);
const _: () = assert!(core::mem::offset_of!(GspFwWprMeta, pmu_reserved_size) == 244);
const _: () = assert!(core::mem::offset_of!(GspFwWprMeta, gsp_fw_heap_vf_partition_count) == 240);
const _: () = assert!(core::mem::offset_of!(GspFwWprMeta, paging_config) == 242);

impl GspFwWprMeta {

    /// Serialize the block into 'dst' (little-endian, '#[repr(C)]' layout).
    /// 'dst' must be at least 'size_of::<GspFwWprMeta>() bytes; extra
    /// bytes are left untouched. The struct is all 'u64', so no padding
    pub fn write_bytes(&self, dst: &mut [u8]) {
        let n = size_of::<GspFwWprMeta>();
        debug_assert!(dst.len() >= n);
        let src = unsafe {
            core::slice::from_raw_parts((self as *const GspFwWprMeta).cast::<u8>(), n)
        };
        dst[..n].copy_from_slice(src);
    }
}

// Radix3 page table
//
// The GSP bootloader walks a three-level table of physical page numbers
// ("radix3"). Each 4 KiB level page holds 512 u64 entries, every entry being
// the byte address of the next-level page (or, at level 2, of a data page of
// the ELF). 'sysmemAddrOfRadix3Elf' in the WPR meta points at the single
// level-0 page.

pub struct Radix3 {
    /// level 0 (root) - always exactly one page
    pub lvl0: DmaBuffer,
    /// level 1 - ceil(lvl2_pages / 512) pages
    pub lvl1: DmaBuffer,
    /// level 2 - ceil(data_pages / 512) pages, entries point at data
    pub lvl2: DmaBuffer,
}

impl Radix3 {
    /// Physical address of the level-0 page (the value that goes into the
    /// WPR meta's 'sysmem_addr_of_radix3_elf')
    pub fn root_phys(&self) -> u64 { self.lvl0.phys() }

    /// Build a radix3 table describing 'data_page_phys' (the ordered list
    /// of physical page addresses making up the GSP-RM ELF). Allocates
    /// fresh sysmem for each level and fills the PTE arrays
    pub fn build(data_page_phys: &[u64]) -> Result<Self, GspFwError> {
        let n = data_page_phys.len();
        if n == 0 { return Err(GspFwError::BadPageList); }

        let lvl2_pages = n.div_ceil(PTES_PER_PAGE);
        let lvl1_pages = lvl2_pages.div_ceil(PTES_PER_PAGE);
        // lvl1 must itself fit in a single root page worth of entries
        debug_assert!(lvl1_pages <= PTES_PER_PAGE);

        let mut lvl2 = DmaBuffer::alloc(lvl2_pages)?;
        let mut lvl1 = DmaBuffer::alloc(lvl1_pages)?;
        let mut lvl0 = DmaBuffer::alloc(1)?;
        lvl2.zero();
        lvl1.zero();
        lvl0.zero();

        // level 2 -> data pages
        write_ptes(&mut lvl2, data_page_phys);

        // level 1 -> level 2 pages
        let lvl2_phys = page_phys_list(&lvl2);
        write_ptes(&mut lvl1, &lvl2_phys[..lvl2_pages]);

        // level 0 -> level 1 pages
        let lvl1_phys = page_phys_list(&lvl1);
        write_ptes(&mut lvl0, &lvl1_phys[..lvl1_pages]);

        DmaBuffer::write_barrier();
        Ok(Self { lvl0, lvl1, lvl2 })
    }
}

/// Physical addresses of each 4 KiB page inside a DMA buffer, in order.
/// Buffers used for radix levels are <= 512 pages; entries past that are
/// left zero
fn page_phys_list(buf: &DmaBuffer) -> [u64; PTES_PER_PAGE] {
    let mut out = [0u64; PTES_PER_PAGE];
    let pages = buf.pages().min(PTES_PER_PAGE);
    for (i, slot) in out.iter_mut().enumerate().take(pages) {
        *slot = buf.phys() + (i as u64) * PAGE_SIZE;
    }
    out
}

/// Write a list of physical addresses as little-endian u64 PTEs into the
/// front of 'buf' (one per 8 bytes). Stops when either runs out
fn write_ptes(buf: &mut DmaBuffer, addrs: &[u64]) {
    let s = buf.as_mut_slice();
    for (i, &a) in addrs.iter().enumerate() {
        let off = i * 8;
        if off + 8 > s.len() { break; }
        le::put_u64(s, off, a);
    }
}

// GSP-RM firmware ELF (gsp-<line>.bin) section parser
//
// nvidia/<chip>/gsp/gsp-<line>.bin is an ELF64 little-endian relocatable object
// (RISC-V e_machine; only its section table matters here). nouveau's
// r535_gsp_load reads three sections:
//   .fwimage               the GSP-RM image proper, placed in WPR2
//   .fwversion             ASCII version string, e.g. "570.144"
//   .fwsignature_<grp>     4 KiB signature for a chip group; for the
//                          GTX 1650/1660 (TU117/TU116) the group is "tu11x"
// (gsp-570.144.bin also carries .fwsignature_ga100 and .fwsignature_tu10x: the
// file is shared across the Turing + GA100 set.)

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;

/// Signature section name for the TU117 / TU116 chip group
pub const GSP_SIGNATURE_SECTION_TU11X: &str = ".fwsignature_tu11x";

/// `MissingSection`'s field is read through `Debug` only, which `dead_code`
/// does not count
#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub enum ElfError {
    TooShort,
    BadMagic,
    NotElf64Le,
    BadSectionTable,
    MissingSection(&'static str),
}

#[inline] fn rd_u16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
#[inline] fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline] fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

fn cstr_at(b: &[u8], off: usize) -> &[u8] {
    let mut e = off;
    while e < b.len() && b[e] != 0 { e += 1; }
    &b[off..e]
}

/// Byte range of a section within the original ELF blob
#[derive(Copy, Clone, Debug)]
struct SectionLoc { off: usize, size: usize }

/// Locate a named section in an ELF64-LE blob
fn find_section(blob: &[u8], name: &'static str) -> Result<SectionLoc, ElfError> {
    if blob.len() < 64 { return Err(ElfError::TooShort); }
    if blob[0..4] != ELF_MAGIC { return Err(ElfError::BadMagic); }
    if blob[4] != ELFCLASS64 || blob[5] != ELFDATA2LSB { return Err(ElfError::NotElf64Le); }

    let e_shoff = rd_u64(blob, 0x28) as usize;
    let e_shentsize = rd_u16(blob, 0x3a) as usize;
    let e_shnum = rd_u16(blob, 0x3c) as usize;
    let e_shstrndx = rd_u16(blob, 0x3e) as usize;
    if e_shentsize < 64 || e_shnum == 0 || e_shstrndx >= e_shnum {
        return Err(ElfError::BadSectionTable);
    }
    let table_end = e_shnum
        .checked_mul(e_shentsize)
        .and_then(|n| n.checked_add(e_shoff))
        .ok_or(ElfError::BadSectionTable)?;
    if table_end > blob.len() { return Err(ElfError::BadSectionTable); }

    // section-header string table
    let shstr_hdr = e_shoff + e_shstrndx * e_shentsize;
    let shstr_off = rd_u64(blob, shstr_hdr + 24) as usize;
    if shstr_off >= blob.len() { return Err(ElfError::BadSectionTable); }

    for i in 0..e_shnum {
        let hdr = e_shoff + i * e_shentsize;
        let sh_name = rd_u32(blob, hdr) as usize;
        let sh_off = rd_u64(blob, hdr + 24) as usize;
        let sh_size = rd_u64(blob, hdr + 32) as usize;
        if shstr_off + sh_name >= blob.len() { continue; }
        if cstr_at(blob, shstr_off + sh_name) == name.as_bytes() {
            if sh_off.checked_add(sh_size).map_or(true, |e| e > blob.len()) {
                return Err(ElfError::BadSectionTable);
            }
            return Ok(SectionLoc { off: sh_off, size: sh_size });
        }
    }
    Err(ElfError::MissingSection(name))
}

/// The three GSP-RM sections this driver consumes, as slices over the
/// original blob
pub struct GspRmFw<'a> {
    pub fwimage: &'a [u8],
    pub fwversion: &'a [u8],
    pub signature: &'a [u8],
}

/// Parse a GSP-RM firmware ELF, returning the '.fwimage', '.fwversion' and
/// TU11x '.fwsignature' sections
pub fn parse_gsp_rm(blob: &[u8]) -> Result<GspRmFw<'_>, ElfError> {
    parse_gsp_rm_sig(blob, GSP_SIGNATURE_SECTION_TU11X)
}

/// Same, with a caller-chosen '.fwsignature_<grp>' section: the gsp-<line>.bin
/// container is shared across chip families and each carries its own signature
/// section (Blackwell consumer: ".fwsignature_gb20x")
pub fn parse_gsp_rm_sig<'a>(blob: &'a [u8], sig_section: &'static str) -> Result<GspRmFw<'a>, ElfError> {
    if blob.is_empty() { return Err(ElfError::TooShort); }
    let img = find_section(blob, ".fwimage")?;
    let ver = find_section(blob, ".fwversion")?;
    let sig = find_section(blob, sig_section)?;
    Ok(GspRmFw {
        fwimage:   &blob[img.off..img.off + img.size],
        fwversion: &blob[ver.off..ver.off + ver.size],
        signature: &blob[sig.off..sig.off + sig.size],
    })
}

/// The GSP RISC-V bootloader (the "monitor"), parsed from the NVFW-wrapped
/// 'bootloader-<line>.bin'. The container's 'header_offset' region holds an
/// 'RM_RISCV_UCODE_DESC'; the 'data' region is the image the booter DMAs into
/// WPR2. Field offsets and usage match nouveau 'r535_gsp_rm_boot_ctor'.
pub struct GspBootloader<'a> {
    /// Bootloader image (DMA'd into WPR2 at 'boot_bin_offset')
    pub image: &'a [u8],
    /// RM_RISCV_UCODE_DESC.monitorCodeOffset
    pub monitor_code_offset: u32,
    /// RM_RISCV_UCODE_DESC.monitorDataOffset
    pub monitor_data_offset: u32,
    /// RM_RISCV_UCODE_DESC.manifestOffset
    pub manifest_offset: u32,
    /// RM_RISCV_UCODE_DESC.appVersion
    pub app_version: u32,
}

/// Parse 'bootloader-<line>.bin': validate the NVFW container, read the
/// monitor/manifest offsets from the 'RM_RISCV_UCODE_DESC' at 'header_offset',
/// and return the image payload. The desc field order is the canonical
/// RM_RISCV_UCODE_DESC (version, bootloader{Offset,Size}, bootloaderParam{..},
/// riscvElf{..}, appVersion, manifest{..}, monitorData{..}, monitorCode{..}).
pub fn parse_gsp_bootloader(blob: &[u8]) -> Option<GspBootloader<'_>> {
    let hdr = crate::drivers::gpu::nvidia::nvfw_hs::NvfwBinHdr::parse(blob)?;
    let ho = hdr.header_offset as usize;
    // u32 index within the desc: 7=appVersion 8=manifestOffset
    // 10=monitorDataOffset 12=monitorCodeOffset
    let rd = |i: usize| -> Option<u32> {
        let o = ho + i * 4;
        if o.checked_add(4)? > blob.len() { return None; }
        Some(u32::from_le_bytes([blob[o], blob[o + 1], blob[o + 2], blob[o + 3]]))
    };
    Some(GspBootloader {
        image: hdr.data(blob),
        app_version:         rd(7)?,
        manifest_offset:     rd(8)?,
        monitor_data_offset: rd(10)?,
        monitor_code_offset: rd(12)?,
    })
}
