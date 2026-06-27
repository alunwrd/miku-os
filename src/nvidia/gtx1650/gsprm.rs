// GSP-RM bring-up infrastructure for TU116 (Turing).
//
// This is the layer between 'gsp.rs' (which only kicks the booter HS image
// and watches it halt) and a working GSP-RM offload driver. It collects
// the pieces that are well-defined and can be built on a cold system:
//
//     VRAM size discovery from PFB
//     the WPR2 firmware layout at the top of VRAM (heap / FW / FRTS)
//     the radix3 page table the GSP bootloader uses to find the FW ELF
//     the GSP RPC message header and the msgq ring-buffer headers
//     the WPR meta block magic / revision the booter validates
//
// What it deliberately does NOT do, and why:
//   The signed GSP-RM ELF ('gsp-*.bin', tens of MiB) is not shipped in
//   this tree. Without it there is nothing to stage in WPR2, so the boot
//   path bottoms out at 'GsprmError::MissingFirmware'. Everything up to
//   that point - register reads, layout maths, sysmem buffer allocation,
//   radix3 construction over a caller-supplied page list - is real.
//
// Field layouts and constants are taken from the nouveau driver
// (drivers/gpu/drm/nouveau/{include/nvfw/gsp.h, nvkm/subdev/gsp/r535.c})
// and from NVIDIA's open-gpu-kernel-modules GSP-RM ABI headers. The
// version-1 WPR-meta layout below matches the r535 firmware line; later
// firmware revisions extend it and would need their own struct.

#![allow(dead_code)]

use core::mem::size_of;

use spin::Mutex;

use crate::nvidia::mmio::MmioRegion;
use crate::serial_println;

use super::dma_buf::{DmaBuffer, DmaBufError};
use super::regs::{
    PFB_LMR_ECC_RESERVED, PFB_LMR_MAG_MASK, PFB_LMR_MAG_SHIFT,
    PFB_LMR_SCALE_MASK, PFB_LOCAL_MEMORY_RANGE,
};

const PAGE_SIZE: u64 = 4096;
const PTES_PER_PAGE: usize = (PAGE_SIZE as usize) / 8; // 512 u64 entries

// Errors //

#[derive(Debug, Copy, Clone)]
pub enum GsprmError {
    /// PFB reports a VRAM size of zero (devinit has not run, or the FB
    /// controller is not up). GSP layout maths cannot proceed
    NoVram,
    /// A sysmem DMA buffer allocation failed
    Alloc(DmaBufError),
    /// The radix3 builder was handed an empty page list
    EmptyImage,
    /// We have all the scaffolding but no signed GSP-RM ELF to place in
    /// WPR2. This is the expected stop point until a firmware blob ships
    MissingFirmware,
}

impl From<DmaBufError> for GsprmError {
    fn from(e: DmaBufError) -> Self { GsprmError::Alloc(e) }
}

// VRAM size discovery //

#[derive(Copy, Clone, Debug)]
pub struct VramInfo {
    /// Raw NV_PFB_PRI_MMU_LOCAL_MEMORY_RANGE register value
    pub raw: u32,
    /// Decoded total VRAM in bytes (best-effort)
    pub total_bytes: u64,
    /// True if the ECC reservation bit indicates 1/16 is carved out
    pub ecc_reserved: bool,
    /// Usable VRAM after the ECC carve-out, if any
    pub usable_bytes: u64,
}

/// Read and decode the local-memory-range register. Returns 'NoVram' if
/// the decoded size is zero
pub fn vram_info(bar0: &MmioRegion) -> Result<VramInfo, GsprmError> {
    let raw = bar0.read32(PFB_LOCAL_MEMORY_RANGE);
    let scale = raw & PFB_LMR_SCALE_MASK;
    let mag = (raw >> PFB_LMR_MAG_SHIFT) & PFB_LMR_MAG_MASK;
    let total_bytes = (mag as u64) << (scale + 20);
    if total_bytes == 0 {
        return Err(GsprmError::NoVram);
    }
    let ecc_reserved = raw & PFB_LMR_ECC_RESERVED == 0;
    let usable_bytes = if ecc_reserved {
        total_bytes - total_bytes / 16
    } else {
        total_bytes
    };
    Ok(VramInfo { raw, total_bytes, ecc_reserved, usable_bytes })
}

// WPR2 firmware layout at the top of VRAM
//
// The GSP firmware reserves a region at the very top of VRAM. From the
// top down: a small VGA workspace, then the GSP FW ELF + bootloader, then
// the GSP heap, with the WPR2 lock spanning the FW + heap. The heap size
// is firmware-line dependent; the value below is the r535 default for a
// single-GPU non-vGPU configuration. FRTS (a fixed-size scratch the
// booter relocates) sits just below the FW ELF.
//
// All offsets are absolute VRAM byte addresses, 64 KiB-aligned per the
// WPR granularity. These are the inputs to the WPR-meta block.

/// VGA workspace reserved at the top of VRAM (nouveau: 256 KiB)
pub const VGA_WORKSPACE_SIZE: u64 = 0x40000;
/// GSP non-WPR heap (libos sysmem-side scratch). r535 default
pub const NON_WPR_HEAP_SIZE: u64 = 0x100000;
/// GSP WPR heap. r535 default for a desktop single-GPU config
pub const WPR_HEAP_SIZE: u64 = 8 << 20; // 8 MiB
/// FRTS region the booter relocates (fixed 1 MiB on Turing)
pub const FRTS_SIZE: u64 = 0x100000;
/// WPR alignment granularity
pub const WPR_ALIGN: u64 = 0x10000; // 64 KiB

#[inline]
fn align_down(v: u64, a: u64) -> u64 { v & !(a - 1) }

#[derive(Copy, Clone, Debug, Default)]
pub struct WprLayout {
    pub fb_size: u64,
    pub vga_workspace_offset: u64,
    pub vga_workspace_size: u64,
    pub gsp_fw_wpr_end: u64,
    pub frts_offset: u64,
    pub frts_size: u64,
    pub gsp_fw_offset: u64,
    pub gsp_fw_size: u64,
    pub boot_bin_offset: u64,
    pub gsp_fw_heap_offset: u64,
    pub gsp_fw_heap_size: u64,
    pub gsp_fw_wpr_start: u64,
    pub gsp_fw_rsvd_start: u64,
    pub non_wpr_heap_offset: u64,
    pub non_wpr_heap_size: u64,
}

/// Compute the WPR2 layout for a card with 'fb_size' bytes of VRAM and a
/// GSP-RM image of 'gsp_fw_size' bytes (the radix3-described ELF) plus a
/// 'boot_bin_size'-byte bootloader. Mirrors the top-down arrangement
/// nouveau's r535_gsp_wpr_meta_init builds
pub fn compute_layout(fb_size: u64, gsp_fw_size: u64, boot_bin_size: u64) -> WprLayout {
    // VGA workspace at the very top
    let vga_workspace_size = VGA_WORKSPACE_SIZE;
    let vga_workspace_offset = align_down(fb_size - vga_workspace_size, WPR_ALIGN);

    // WPR2 end is just below the VGA workspace
    let gsp_fw_wpr_end = vga_workspace_offset;

    // FRTS sits directly under the VGA workspace, inside WPR2
    let frts_size = FRTS_SIZE;
    let frts_offset = align_down(gsp_fw_wpr_end - frts_size, WPR_ALIGN);

    // Bootloader image directly below FRTS
    let boot_bin_offset = align_down(frts_offset - boot_bin_size, WPR_ALIGN);

    // GSP FW ELF below the bootloader
    let gsp_fw_offset = align_down(boot_bin_offset - gsp_fw_size, WPR_ALIGN);

    // WPR heap below the FW ELF
    let gsp_fw_heap_size = WPR_HEAP_SIZE;
    let gsp_fw_heap_offset = align_down(gsp_fw_offset - gsp_fw_heap_size, WPR_ALIGN);

    // WPR2 starts at the heap base
    let gsp_fw_wpr_start = gsp_fw_heap_offset;

    // Non-WPR heap sits just below WPR2 (not lock-protected)
    let non_wpr_heap_size = NON_WPR_HEAP_SIZE;
    let non_wpr_heap_offset = align_down(gsp_fw_wpr_start - non_wpr_heap_size, WPR_ALIGN);

    // Everything the GSP FW reserves starts here
    let gsp_fw_rsvd_start = non_wpr_heap_offset;

    WprLayout {
        fb_size,
        vga_workspace_offset,
        vga_workspace_size,
        gsp_fw_wpr_end,
        frts_offset,
        frts_size,
        gsp_fw_offset,
        gsp_fw_size,
        boot_bin_offset,
        gsp_fw_heap_offset,
        gsp_fw_heap_size,
        gsp_fw_wpr_start,
        gsp_fw_rsvd_start,
        non_wpr_heap_offset,
        non_wpr_heap_size,
    }
}

// WPR meta block (version 1, r535 line) //

/// Magic the GSP booter checks at the head of the WPR-meta block
pub const GSP_FW_WPR_META_MAGIC: u64 = 0xdc3a_ae21_371a_60b3;
/// Revision of the WPR-meta layout this code targets
pub const GSP_FW_WPR_META_REVISION: u64 = 1;

/// 'verified' sentinel written by the booter once it has validated and
/// locked the meta block in WPR2
pub const GSP_FW_WPR_META_VERIFIED: u64 = 0xa0a0_a0a0_a0a0_a0a0;

/// The full version-1 WPR meta block, byte-exact with
/// 'GspFwWprMeta' from open-gpu-kernel-modules
/// (src/nvidia/arch/nvalloc/common/inc/gsp/gsp_fw_wpr_meta.h, 595.71.05).
///
/// This struct is exactly 256 bytes and '#[repr(C)]'; the booter DMAs it
/// from sysmem (address handed in SEC2 MAILBOX0/1), validates the magic +
/// revision, then locks it into WPR2 and stamps 'verified'. The two unions
/// in the C original are flattened to their initial-boot variants here:
///     union 1 -> sysmem_addr_of_signature / size_of_signature
///     union 2 -> the partitionRpc/elf fields, all zero at first boot
/// Every reserved/trailing field is present so the 256-byte layout the
/// booter checksums matches exactly. An earlier version stopped at
/// 'verified' after 'boot_count', which put 'verified' at the wrong offset
/// and made the block only ~216 bytes - the booter would reject it.
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
    // union 2 (initial-boot variant, all zero): partition RPC + elf fields
    pub partition_rpc_addr: u64,
    pub partition_rpc_request_offset: u16,
    pub partition_rpc_reply_offset: u16,
    pub elf_code_offset: u32,
    pub elf_data_offset: u32,
    pub elf_code_size: u32,
    pub elf_data_size: u32,
    pub ls_ucode_version: u32,
    // trailing
    pub gsp_fw_heap_vf_partition_count: u8,
    pub flags: u8,
    pub padding: [u8; 2],
    pub pmu_reserved_size: u32,
    /// 0 = unverified; GSP_FW_WPR_META_VERIFIED once the booter accepts it
    pub verified: u64,
}

// Compile-time guarantee that the layout matches the 256-byte ABI block
const _: () = assert!(size_of::<GspFwWprMeta>() == 256);

impl GspFwWprMeta {
    /// Build the meta block from a computed layout plus the sysmem
    /// addresses/sizes of the radix3 root, bootloader and signature blobs
    #[allow(clippy::too_many_arguments)]
    pub fn from_layout(
        l: &WprLayout,
        radix3_root_phys: u64,
        radix3_elf_size: u64,
        bootloader_phys: u64,
        bootloader_size: u64,
        bootloader_code_off: u64,
        bootloader_data_off: u64,
        bootloader_manifest_off: u64,
        signature_phys: u64,
        signature_size: u64,
    ) -> Self {
        Self {
            magic: GSP_FW_WPR_META_MAGIC,
            revision: GSP_FW_WPR_META_REVISION,
            sysmem_addr_of_radix3_elf: radix3_root_phys,
            size_of_radix3_elf: radix3_elf_size,
            sysmem_addr_of_bootloader: bootloader_phys,
            size_of_bootloader: bootloader_size,
            bootloader_code_offset: bootloader_code_off,
            bootloader_data_offset: bootloader_data_off,
            bootloader_manifest_offset: bootloader_manifest_off,
            sysmem_addr_of_signature: signature_phys,
            size_of_signature: signature_size,
            gsp_fw_rsvd_start: l.gsp_fw_rsvd_start,
            non_wpr_heap_offset: l.non_wpr_heap_offset,
            non_wpr_heap_size: l.non_wpr_heap_size,
            gsp_fw_wpr_start: l.gsp_fw_wpr_start,
            gsp_fw_heap_offset: l.gsp_fw_heap_offset,
            gsp_fw_heap_size: l.gsp_fw_heap_size,
            gsp_fw_offset: l.gsp_fw_offset,
            boot_bin_offset: l.boot_bin_offset,
            frts_offset: l.frts_offset,
            frts_size: l.frts_size,
            gsp_fw_wpr_end: l.gsp_fw_wpr_end,
            fb_size: l.fb_size,
            vga_workspace_offset: l.vga_workspace_offset,
            vga_workspace_size: l.vga_workspace_size,
            boot_count: 0,
            verified: 0,
            // union 2 + trailing fields are all zero at initial boot
            ..Default::default()
        }
    }

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

// GSP RPC and message-queue headers //

/// Header version reported in 'GspRpcHeader::header_version'. Nouveau uses
/// 0x03000000 for the r535 line
pub const GSP_RPC_HEADER_VERSION: u32 = 0x0300_0000;
/// Fixed signature word ("VGPU") at the head of every GSP RPC
pub const GSP_RPC_SIGNATURE: u32 = 0x4756_5055; // 'VGPU' little-endian

/// Header that prefixes every message in the GSP RPC ring. Mirrors
/// nouveau's 'struct nvfw_gsp_rpc' (the first 8 u32s; payload follows)
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct GspRpcHeader {
    pub header_version: u32,
    pub signature: u32,
    pub length: u32,
    pub function: u32,
    pub rpc_result: u32,
    pub rpc_result_private: u32,
    pub sequence: u32,
    pub cpu_rm_gfid: u32,
}

impl GspRpcHeader {
    pub fn new(function: u32, payload_len: u32, sequence: u32) -> Self {
        Self {
            header_version: GSP_RPC_HEADER_VERSION,
            signature: GSP_RPC_SIGNATURE,
            length: payload_len + size_of::<GspRpcHeader>() as u32,
            function,
            rpc_result: 0xffff_ffff,
            rpc_result_private: 0xffff_ffff,
            sequence,
            cpu_rm_gfid: 0,
        }
    }
}

/// Per-direction ring header at the base of each msgq command/status page.
/// Mirrors nouveau's 'struct msgqTxHeader' / 'struct msgqRxHeader'. The
/// GSP and the host each own one of these in a shared sysmem page
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct MsgqTxHeader {
    pub version: u32,
    pub size: u32,        // ring size in bytes
    pub msg_size: u32,    // element size in bytes
    pub msg_count: u32,   // number of elements
    pub write_ptr: u32,   // producer index
    pub flags: u32,
    pub rx_hdr_off: u32,  // byte offset to the matching MsgqRxHeader
    pub entry_off: u32,   // byte offset to the first ring element
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct MsgqRxHeader {
    pub read_ptr: u32,    // consumer index
}

/// msgq protocol version nouveau negotiates with r535 GSP
pub const MSGQ_VERSION: u32 = 0;

// Radix3 page table
//
// The GSP bootloader does not consume the FW ELF as a flat buffer; it
// walks a three-level table of physical page numbers ("radix3"). Each
// 4 KiB level page holds 512 u64 entries, every entry being the byte
// address of the next-level page (or, at level 2, of a data page of the
// ELF). 'sysmemAddrOfRadix3Elf' in the WPR meta points at the single
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
    pub fn build(data_page_phys: &[u64]) -> Result<Self, GsprmError> {
        let n = data_page_phys.len();
        if n == 0 { return Err(GsprmError::EmptyImage); }

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
        s[off..off + 8].copy_from_slice(&a.to_le_bytes());
    }
}

// Top-level prepare //

/// Result of 'prepare': everything we managed to set up before hitting the
/// missing-firmware wall
#[derive(Copy, Clone, Debug)]
pub struct GsprmPrep {
    pub vram: VramInfo,
    pub layout: WprLayout,
}

/// Walk as far toward a GSP-RM boot as is possible: read VRAM size,
/// compute the WPR2 layout (using the real GSP-RM ELF size when the
/// firmware is embedded, falling back to representative defaults when
/// it isn't), and report. Allocates nothing it cannot free.
///
/// Note: VRAM staging of the FW image is not the host driver's job - the
/// booter itself DMAs the radix3-described pages from sysmem into WPR2
/// once ACR has locked the region. 'load() is the function that builds
/// the radix3 + WPR-meta the booter consumes
pub fn prepare(bar0: &MmioRegion) -> Result<GsprmPrep, GsprmError> {
    let vram = vram_info(bar0)?;
    serial_println!(
        "[gsprm] VRAM: {} MiB total ({} MiB usable, ecc_reserved={}) raw={:#010x}",
        vram.total_bytes >> 20, vram.usable_bytes >> 20, vram.ecc_reserved, vram.raw
    );

    // Pick the FW size for layout maths from the embedded GSP-RM ELF if
    // present; fall back to a 36 MiB stand-in so the printed offsets still
    // resemble a real upload when the feature is off
    const REPRESENTATIVE_GSP_FW_SIZE: u64 = 36 << 20;
    const REPRESENTATIVE_BOOT_BIN_SIZE: u64 = 1 << 20;
    let gsp_rm_fw = super::tu116_fw::gsp_rm_570();
    let fw_size = match parse_gsp_rm(gsp_rm_fw.bytes()) {
        Ok(fw) => fw.fwimage.len() as u64,
        Err(_) => REPRESENTATIVE_GSP_FW_SIZE,
    };
    let layout = compute_layout(
        vram.total_bytes,
        fw_size,
        REPRESENTATIVE_BOOT_BIN_SIZE,
    );
    serial_println!(
        "[gsprm] WPR2 layout: start={:#x} end={:#x} heap@{:#x}+{:#x} fw@{:#x} boot@{:#x} frts@{:#x} non_wpr@{:#x}",
        layout.gsp_fw_wpr_start, layout.gsp_fw_wpr_end,
        layout.gsp_fw_heap_offset, layout.gsp_fw_heap_size,
        layout.gsp_fw_offset, layout.boot_bin_offset, layout.frts_offset,
        layout.non_wpr_heap_offset
    );
    serial_println!(
        "[gsprm] meta block: magic={:#018x} rev={} size={}B",
        GSP_FW_WPR_META_MAGIC, GSP_FW_WPR_META_REVISION,
        size_of::<GspFwWprMeta>()
    );

    Ok(GsprmPrep { vram, layout })
}

// Dry-run exerciser //

/// Outcome of 'dryrun': confirms the radix3 chain resolves and the WPR
/// meta block is internally consistent
#[derive(Copy, Clone, Debug)]
pub struct DryrunReport {
    pub fake_elf_pages: usize,
    pub fake_elf_phys: u64,
    pub lvl2_pages: usize,
    pub lvl1_pages: usize,
    pub radix3_root_phys: u64,
    /// Physical address the level-0 -> level-1 -> level-2 chain resolves to
    /// for data page 0 (must equal 'fake_elf_phys')
    pub resolved_first_page: u64,
    /// True iff 'resolved_first_page == fake_elf_phys' and the WPR meta
    /// magic/revision came back as written
    pub ok: bool,
    pub meta_size: usize,
}

/// Read a little-endian u64 PTE at index 'i' from a radix-level buffer
fn read_pte(buf: &DmaBuffer, i: usize) -> u64 {
    let s = buf.as_slice();
    let off = i * 8;
    let mut b = [0u8; 8];
    b.copy_from_slice(&s[off..off + 8]);
    u64::from_le_bytes(b)
}

/// Exercise the radix3 + WPR-meta construction without a real firmware
/// image. Pins 'fake_elf_pages' of contiguous sysmem, treats it as the
/// GSP-RM ELF, builds the radix3 over its pages, fills a WPR-meta block
/// from the live VRAM layout, then walks lvl0 -> lvl1 -> lvl2 to confirm
/// the chain resolves back to the first data page. Frees everything on
/// return. This is a self-test of the otherwise firmware-gated path
pub fn dryrun(bar0: &MmioRegion, fake_elf_pages: usize) -> Result<DryrunReport, GsprmError> {
    let pages = fake_elf_pages.max(1);
    let vram = vram_info(bar0)?;

    // Pin a contiguous "ELF" and collect its page addresses
    let elf = DmaBuffer::alloc(pages)?;
    let mut data_phys = alloc_phys_vec(&elf);
    let n = elf.pages();
    data_phys.truncate(n);

    let r = Radix3::build(&data_phys)?;
    let lvl2_pages = n.div_ceil(PTES_PER_PAGE);
    let lvl1_pages = lvl2_pages.div_ceil(PTES_PER_PAGE);

    // Resolve data page 0 through the table: lvl0[0] -> a lvl1 page,
    // lvl1[0] -> a lvl2 page, lvl2[0] -> data page 0
    let lvl1_page0 = read_pte(&r.lvl0, 0);
    let lvl2_page0 = if lvl1_page0 == r.lvl1.phys() { read_pte(&r.lvl1, 0) } else { 0 };
    let resolved_first_page = if lvl2_page0 == r.lvl2.phys() { read_pte(&r.lvl2, 0) } else { 0 };

    // Build a WPR-meta block from a representative layout so the consistency
    // of the struct round-trips
    let layout = compute_layout(vram.total_bytes, (pages as u64) * PAGE_SIZE, 1 << 20);
    let meta = GspFwWprMeta::from_layout(
        &layout,
        r.root_phys(), (pages as u64) * PAGE_SIZE,
        /*bootloader*/ 0, 0, 0, 0, 0,
        /*signature*/ 0, 0,
    );

    let ok = resolved_first_page == elf.phys()
        && meta.magic == GSP_FW_WPR_META_MAGIC
        && meta.revision == GSP_FW_WPR_META_REVISION
        && meta.gsp_fw_wpr_start == layout.gsp_fw_wpr_start;

    serial_println!(
        "[gsprm] dryrun: elf={} pages @ {:#x}; radix3 root @ {:#x}; lvl1={} lvl2={}; resolved page0 = {:#x} ({})",
        n, elf.phys(), r.root_phys(), lvl1_pages, lvl2_pages,
        resolved_first_page, if ok { "OK" } else { "MISMATCH" }
    );

    Ok(DryrunReport {
        fake_elf_pages: n,
        fake_elf_phys: elf.phys(),
        lvl2_pages,
        lvl1_pages,
        radix3_root_phys: r.root_phys(),
        resolved_first_page,
        ok,
        meta_size: size_of::<GspFwWprMeta>(),
    })
    // 'elf' and 'r' (with its three level buffers) drop here, freeing all
    // pinned sysmem
}

/// Physical addresses of every 4 KiB page in a DMA buffer, in order
fn alloc_phys_vec(buf: &DmaBuffer) -> alloc::vec::Vec<u64> {
    let mut v = alloc::vec::Vec::with_capacity(buf.pages());
    for i in 0..buf.pages() {
        v.push(buf.phys() + (i as u64) * PAGE_SIZE);
    }
    v
}

// GSP-RM firmware ELF (gsp-<line>.bin) section parser 
//
// nvidia/<chip>/gsp/gsp-<line>.bin is an ELF64 little-endian relocatable
// object (RISC-V e_machine, but only its section table matters here).
// nouveau's r535_gsp_load reads three sections out of it:
//   .fwimage               the GSP-RM image proper, placed in WPR2
//   .fwversion             ASCII version string, e.g. "570.144"
//   .fwsignature_<grp>     4 KiB signature for a chip group; for the
//                          GTX 1650/1660 (TU117/TU116) the group is "tu11x"
// (gsp-570.144.bin also carries .fwsignature_ga100 and .fwsignature_tu10x
// because the file is shared across the whole Turing + GA100 set.)

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;

/// Signature section name for the TU117 / TU116 chip group
pub const GSP_SIGNATURE_SECTION_TU11X: &str = ".fwsignature_tu11x";

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
    if blob.is_empty() { return Err(ElfError::TooShort); }
    let img = find_section(blob, ".fwimage")?;
    let ver = find_section(blob, ".fwversion")?;
    let sig = find_section(blob, GSP_SIGNATURE_SECTION_TU11X)?;
    Ok(GspRmFw {
        fwimage:   &blob[img.off..img.off + img.size],
        fwversion: &blob[ver.off..ver.off + ver.size],
        signature: &blob[sig.off..sig.off + sig.size],
    })
}

// GSP-RM load: stage the real image + build radix3 + meta 

#[derive(Debug, Copy, Clone)]
pub enum LoadError {
    /// 'blob' was empty - the GSP-RM image is missing from the firmware
    /// store (gsp-570.144.bin absent or unreadable)
    NoFirmware,
    /// The GSP-RM ELF did not parse
    Elf(ElfError),
    /// A downstream gsprm step failed (VRAM read, allocation, radix3)
    Gsprm(GsprmError),
    /// The GSP RISC-V bootloader (bootloader-<line>.bin) did not parse
    BadBootloader,
}
impl From<ElfError> for LoadError { fn from(e: ElfError) -> Self { LoadError::Elf(e) } }
impl From<GsprmError> for LoadError { fn from(e: GsprmError) -> Self { LoadError::Gsprm(e) } }

#[derive(Copy, Clone, Debug)]
pub struct LoadReport {
    pub fwimage_len: usize,
    pub signature_len: usize,
    /// NUL-padded copy of the '.fwversion' string
    pub version: [u8; 16],
    pub vram_total: u64,
    pub layout: WprLayout,
    pub staged_phys: u64,
    pub staged_pages: usize,
    pub radix3_root_phys: u64,
    pub radix3_lvl1_pages: usize,
    pub radix3_lvl2_pages: usize,
    /// True iff the level-0 -> level-1 -> level-2 chain resolves back to
    /// the first staged page
    pub radix3_resolves: bool,
    /// Sysmem physical address of the materialized WPR-meta block
    pub meta_phys: u64,
    pub meta_size: usize,
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
    let hdr = super::tu116_fw::NvfwBinHdr::parse(blob)?;
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

/// Parse the GSP-RM ELF, stage '.fwimage' into a phys-contiguous sysmem
/// buffer, build the radix3 page table over it, compute the WPR2 layout
/// with the real image size, materialize a 'GspFwWprMeta' block into its
/// own sysmem page, verify the radix3 chain resolves, and pin all of it
/// in 'GSP_RM' for the later booter handoff. Use 'unload()' to release.
///
/// This is everything the host can do toward a GSP-RM boot before the
/// SEC2 ACR sequence (which locks WPR2) and the DMA-to-VRAM staging of
/// the image + bootloader - neither is modelled yet
pub fn load(bar0: &MmioRegion, blob: &[u8]) -> Result<LoadReport, LoadError> {
    if blob.is_empty() { return Err(LoadError::NoFirmware); }
    let fw = parse_gsp_rm(blob)?;
    serial_println!(
        "[gsprm] gsp-rm ELF: fwimage={} bytes signature={} bytes version={}",
        fw.fwimage.len(), fw.signature.len(),
        core::str::from_utf8(fw.fwversion).unwrap_or("?")
    );

    let vram = vram_info(bar0)?;

    // Stage .fwimage into phys-contiguous sysmem (the radix3 then points
    // at these pages). Zero-pad the tail of the last page
    let pages = fw.fwimage.len().div_ceil(PAGE_SIZE as usize);
    let mut staged = DmaBuffer::alloc(pages).map_err(GsprmError::Alloc)?;
    staged.zero();
    staged.as_mut_slice()[..fw.fwimage.len()].copy_from_slice(fw.fwimage);
    DmaBuffer::write_barrier();

    // radix3 over the staged pages
    let data_phys = alloc_phys_vec(&staged);
    let r = Radix3::build(&data_phys)?;
    let lvl2_pages = pages.div_ceil(PTES_PER_PAGE);
    let lvl1_pages = lvl2_pages.div_ceil(PTES_PER_PAGE);

    // verify page-0 resolution through the table
    let l1p0 = read_pte(&r.lvl0, 0);
    let l2p0 = if l1p0 == r.lvl1.phys() { read_pte(&r.lvl1, 0) } else { 0 };
    let resolved = if l2p0 == r.lvl2.phys() { read_pte(&r.lvl2, 0) } else { 0 };
    let radix3_resolves = resolved == staged.phys();

    // Parse + stage the GSP RISC-V bootloader. The booter DMAs this into
    // WPR2 at boot_bin_offset; WprMeta carries its sysmem address + the
    // monitor/manifest offsets the booter needs
    let gsp_bl_fw = super::tu116_fw::gsp_bootloader_570();
    let blf = parse_gsp_bootloader(gsp_bl_fw.bytes())
        .ok_or(LoadError::BadBootloader)?;
    let bl_pages = blf.image.len().div_ceil(PAGE_SIZE as usize).max(1);
    let mut bl_buf = DmaBuffer::alloc(bl_pages).map_err(GsprmError::Alloc)?;
    bl_buf.zero();
    bl_buf.as_mut_slice()[..blf.image.len()].copy_from_slice(blf.image);
    DmaBuffer::write_barrier();
    let bl_phys = bl_buf.phys();
    let bl_size = blf.image.len() as u64;
    serial_println!(
        "[gsprm] bootloader staged @ {:#x} ({}B); monitor_code={:#x} monitor_data={:#x} manifest={:#x} appver={}",
        bl_phys, bl_size, blf.monitor_code_offset, blf.monitor_data_offset,
        blf.manifest_offset, blf.app_version
    );

    // Stage the GSP-RM image production signature in its own sysmem page so
    // WprMeta.sysmemAddrOfSignature points at it (the booter verifies it)
    let sig_pages = fw.signature.len().div_ceil(PAGE_SIZE as usize).max(1);
    let mut sig_buf = DmaBuffer::alloc(sig_pages).map_err(GsprmError::Alloc)?;
    sig_buf.zero();
    sig_buf.as_mut_slice()[..fw.signature.len()].copy_from_slice(fw.signature);
    DmaBuffer::write_barrier();
    let sig_phys = sig_buf.phys();

    // Layout with the real FW image size and the real bootloader size
    let layout = compute_layout(vram.total_bytes, fw.fwimage.len() as u64, bl_size);
    let meta = GspFwWprMeta::from_layout(
        &layout,
        r.root_phys(), fw.fwimage.len() as u64,
        bl_phys, bl_size,
        blf.monitor_code_offset as u64,
        blf.monitor_data_offset as u64,
        blf.manifest_offset as u64,
        sig_phys, fw.signature.len() as u64,
    );

    // Materialize the meta block into its own sysmem page so a real
    // address can be handed to the booter later
    let mut meta_buf = DmaBuffer::alloc(1).map_err(GsprmError::Alloc)?;
    meta_buf.zero();
    meta.write_bytes(meta_buf.as_mut_slice());
    DmaBuffer::write_barrier();

    let mut version = [0u8; 16];
    let vn = fw.fwversion.len().min(15);
    version[..vn].copy_from_slice(&fw.fwversion[..vn]);

    serial_println!(
        "[gsprm] staged @ {:#x} ({} pages); radix3 root @ {:#x} (lvl1={} lvl2={}); resolves={}",
        staged.phys(), pages, r.root_phys(), lvl1_pages, lvl2_pages, radix3_resolves
    );
    serial_println!(
        "[gsprm] WPR2 (real fw): start={:#x} end={:#x} fw@{:#x}+{:#x} heap@{:#x}+{:#x} non_wpr@{:#x}",
        layout.gsp_fw_wpr_start, layout.gsp_fw_wpr_end,
        layout.gsp_fw_offset, layout.gsp_fw_size,
        layout.gsp_fw_heap_offset, layout.gsp_fw_heap_size,
        layout.non_wpr_heap_offset
    );
    serial_println!(
        "[gsprm] WPR-meta materialized @ {:#x} ({} bytes)", meta_buf.phys(), size_of::<GspFwWprMeta>()
    );
    serial_println!(
        "[gsprm] next wall: SEC2 ACR boot (sets up WPR2) + bootloader-<line> parse + DMA of image/bootloader into VRAM"
    );

    let report = LoadReport {
        fwimage_len: fw.fwimage.len(),
        signature_len: fw.signature.len(),
        version,
        vram_total: vram.total_bytes,
        layout,
        staged_phys: staged.phys(),
        staged_pages: pages,
        radix3_root_phys: r.root_phys(),
        radix3_lvl1_pages: lvl1_pages,
        radix3_lvl2_pages: lvl2_pages,
        radix3_resolves,
        meta_phys: meta_buf.phys(),
        meta_size: size_of::<GspFwWprMeta>(),
    };

    // Persist the state - the buffers must stay pinned for the booter and
    // for the eventual DMA-to-VRAM staging
    *GSP_RM.lock() = Some(GspRmState {
        staged,
        radix3: r,
        meta_buf,
        meta,
        layout,
        fwimage_len: fw.fwimage.len(),
        version,
        bootloader: bl_buf,
        signature: sig_buf,
        app_version: blf.app_version,
    });

    Ok(report)
}

// Persistent GSP-RM state + booter handoff

/// Everything 'load()' pins for the rest of the GSP bring-up. Dropping
/// this frees all the sysmem buffers
pub struct GspRmState {
    /// '.fwimage' staged in phys-contiguous sysmem
    pub staged: DmaBuffer,
    /// radix3 page table over 'staged'
    pub radix3: Radix3,
    /// sysmem page holding the materialized 'GspFwWprMeta'
    pub meta_buf: DmaBuffer,
    /// the meta block, in struct form
    pub meta: GspFwWprMeta,
    pub layout: WprLayout,
    pub fwimage_len: usize,
    pub version: [u8; 16],
    /// GSP RISC-V bootloader image staged in sysmem (DMA'd into WPR2)
    pub bootloader: DmaBuffer,
    /// GSP-RM production signature staged in sysmem
    pub signature: DmaBuffer,
    /// RM_RISCV_UCODE_DESC.appVersion from the bootloader. Written to the GSP
    /// FALCON_OS register after the booter hands off to the RISC-V core
    pub app_version: u32,
}

impl GspRmState {
    /// Sysmem physical address of the WPR-meta block
    pub fn meta_phys(&self) -> u64 { self.meta_buf.phys() }
    /// Sysmem physical address of the staged GSP RISC-V bootloader
    pub fn bootloader_phys(&self) -> u64 { self.bootloader.phys() }
}

static GSP_RM: Mutex<Option<GspRmState>> = Mutex::new(None);

/// True if 'load()' has run and the GSP-RM state is pinned
pub fn is_loaded() -> bool { GSP_RM.lock().is_some() }

/// Run a closure with the pinned GSP-RM state, if any
pub fn with_state<R>(f: impl FnOnce(&GspRmState) -> R) -> Option<R> {
    GSP_RM.lock().as_ref().map(f)
}

/// Drop the pinned GSP-RM state, freeing all its sysmem buffers
pub fn unload() { *GSP_RM.lock() = None; }

#[derive(Debug, Copy, Clone)]
pub enum BooterError {
    /// 'load()' has not run - no WPR-meta to hand the booter
    NotLoaded,
    /// The GSP booter itself failed (engine gated, bad header, timeout, ...)
    Gsp(super::gsp::GspError),
}

/// Hand the materialized 'GspFwWprMeta' address to the GSP booter and kick
/// 'booter_load'. The booter expects WPR2 to already be locked by the SEC2
/// ACR sequence; until that is implemented it will halt early with an
/// error class in MAILBOX1 - but the meta handoff and booter kick are real
/// and reusable. Returns the booter's halt status
pub fn boot_booter(bar0: &MmioRegion) -> Result<super::gsp::BootStatus, BooterError> {
    let meta_phys = with_state(|s| s.meta_phys()).ok_or(BooterError::NotLoaded)?;
    serial_println!("[gsprm] booting GSP booter_load with WPR-meta @ {:#x}", meta_phys);
    super::gsp::attempt_boot_with_arg(bar0, Some(meta_phys)).map_err(BooterError::Gsp)
}

// Full GSP-RM boot orchestrator
//
// One command that drives the whole documented boot sequence in order and
// stops at the first stage whose hardware precondition is not met, with a
// precise diagnostic. This is the single entry point to test the pipeline
// end-to-end on real TU116 silicon. Each stage is non-destructive on
// failure (the device stays registered and inspectable).
//
// Pipeline (matches the tu116_fw consume order):
//   1) NVDEC scrubber     zero the VRAM region ACR will lock
//   2) gsprm::load        stage .fwimage + radix3 + WPR-meta in sysmem
//   3) SEC2 ACR (v2)      lock WPR2 at the top of VRAM
//   4) verify WPR2 lock   read PFB WPR2 LO/HI; GATE: no lock -> stop
//   5) booter_load        hand WPR-meta to GSP; booter DMAs the radix3
//                         image into WPR2 and starts GSP-RM
//   5) boot args + booter build the libos init args + GSP_ARGUMENTS_CACHED +
//                         CMDQ/MSGQ shared region (bootargs.rs), hand the
//                         libos address to GSP MAILBOX0/1, run the SEC2 booter
//                         (DMAs the radix3 image into WPR2 and starts GSP-RM),
//                         then set GSP FALCON_OS to the bootloader app version
//   6) GSP handshake      poll the GSP-owned MSGQ in the shared region for
//                         GSP-RM's first message (the GSP_INIT_DONE event)
//
// The boot arguments are now wired (bootargs::GspBootArgs), so GSP-RM knows
// where our queue lives and can post to it. The remaining hard gate is
// upstream at stage 4: AHESASC needs the RM_FLCN_ACR_DESC region descriptor
// staged in SEC2 DMEM to lock WPR2. If WPR2 never locks, the booter has no
// protected region to DMA into and the pipeline stops there with a precise
// diagnostic before any GSP register is touched.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BootStage {
    /// NVDEC scrubber pass
    Scrubber,
    /// GSP-RM ELF staged + radix3 + WPR-meta materialized
    Load,
    /// FWSEC ran the FRTS command on the GSP Falcon (sets up + locks WPR2)
    Fwsec,
    /// SEC2 booter_load attempted
    Acr,
    /// WPR2 confirmed locked in PFB
    Wpr2Locked,
    /// booter_load ran and GSP-RM was kicked
    Booter,
    /// GSP-RM posted its first MSGQ message
    GspHandshake,
}

#[derive(Copy, Clone, Debug)]
pub struct FullBootReport {
    /// The furthest stage reached successfully
    pub reached: BootStage,
    /// True iff PFB reported WPR2 locked after ACR
    pub wpr2_locked: bool,
    /// WPR2 LO/HI byte addresses as decoded from PFB (0 if unlocked)
    pub wpr2_lo: u64,
    pub wpr2_hi: u64,
    /// booter halt status, if stage 5 ran
    pub booter_mb1: u32,
    /// FWSEC FRTS error scratch (top 16 bits; 0 = ok), if the FWSEC stage ran
    pub frts_err: u16,
    /// First GSP message function code, if stage 6 saw one (0 = none)
    pub gsp_msg_function: u32,
    /// GSP message rpc_result, if any
    pub gsp_msg_result: u32,
}

/// Wall-clock timeout for the post-boot MSGQ poll (via PTIMER). GSP-RM init
/// takes a while on real silicon; 500 ms is comfortably past the firmware's
/// own init time. Bounding by real time keeps the shell responsive instead of
/// burning a fixed huge spin count
const GSP_HANDSHAKE_TIMEOUT_NS: u64 = 500_000_000;
/// PTIMER nanosecond counter (absolute BAR0 offset)
const PTIMER_TIME_0: u32 = 0x0000_9400;

/// Pinned RPC driver for the booted GSP-RM. Holds the CMDQ/MSGQ region for
/// the lifetime of the session so the rings stay mapped after 'boot'
static RPC: Mutex<Option<super::rpc::RpcDriver>> = Mutex::new(None);

/// Run a closure with the pinned RPC driver, if GSP-RM handshake set one up
pub fn with_rpc<R>(f: impl FnOnce(&super::rpc::RpcDriver) -> R) -> Option<R> {
    RPC.lock().as_ref().map(f)
}

/// Pinned GSP-RM boot arguments (libos table + CMDQ/MSGQ shared region + log
/// buffers). GSP-RM reads these from sysmem for the life of the session, so
/// they must stay allocated after 'boot' returns
static BOOT_ARGS: Mutex<Option<super::bootargs::GspBootArgs>> = Mutex::new(None);

/// Run a closure with the pinned GSP boot args, if a boot has set them up
pub fn with_boot_args<R>(f: impl FnOnce(&super::bootargs::GspBootArgs) -> R) -> Option<R> {
    BOOT_ARGS.lock().as_ref().map(f)
}

/// Read PFB and decode the WPR2 lock window. Returns (locked, lo, hi)
fn read_wpr2(bar0: &MmioRegion) -> (bool, u64, u64) {
    use super::regs::{
        decode_wpr_addr, PFB_PRI_MMU_WPR2_ADDR_HI, PFB_PRI_MMU_WPR2_ADDR_LO,
    };
    let lo = decode_wpr_addr(bar0.read32(PFB_PRI_MMU_WPR2_ADDR_LO));
    let hi = decode_wpr_addr(bar0.read32(PFB_PRI_MMU_WPR2_ADDR_HI));
    let locked = lo != 0 && lo <= hi;
    (locked, lo, hi)
}

/// Allocate the CMDQ/MSGQ pair, pin it, and poll the MSGQ for the first
/// GSP-RM message. Returns Some((function, rpc_result)) if a message
/// arrives within the budget, else None. The queue phys is logged so it
/// can be cross-referenced once the GSP boot-args handoff is wired
fn gsp_handshake(bar0: &MmioRegion) -> Option<(u32, u32)> {
    let msgq = match super::msgq::Msgq::alloc(super::msgq::MSGQ_DEFAULT_ENTRIES) {
        Ok(q) => q,
        Err(e) => {
            serial_println!("[gsprm] handshake: msgq alloc failed: {:?}", e);
            return None;
        }
    };
    let mut rpc = super::rpc::RpcDriver::new(msgq);
    serial_println!(
        "[gsprm] handshake: CMDQ/MSGQ pinned @ {:#x}; polling MSGQ for GSP init message...",
        rpc.queue_phys()
    );
    serial_println!(
        "[gsprm] handshake: NOTE GSP-RM must be told this queue address via the GSP boot args"
    );
    serial_println!(
        "[gsprm] handshake:      (libos / GSP_ARGUMENTS_CACHED in the non-WPR heap) for traffic to arrive"
    );

    // Poll the MSGQ, bounded by a PTIMER wall-clock deadline. try_recv reads
    // sysmem (not MMIO), but we still cap it by real time so the shell never
    // hangs for a fixed huge spin count
    let mut seen: Option<(u32, u32)> = None;
    let start = bar0.read32(PTIMER_TIME_0);
    let budget = GSP_HANDSHAKE_TIMEOUT_NS.min(u32::MAX as u64) as u32;
    'poll: loop {
        if let Some((hdr, _payload)) = rpc.try_recv() {
            serial_println!(
                "[gsprm] handshake: GSP message function={:#x} result={:#x} seq={} len={}",
                hdr.function, hdr.rpc_result, hdr.sequence, hdr.length
            );
            seen = Some((hdr.function, hdr.rpc_result));
            break 'poll;
        }
        if bar0.read32(PTIMER_TIME_0).wrapping_sub(start) >= budget { break 'poll; }
        core::hint::spin_loop();
    }
    if seen.is_none() {
        serial_println!(
            "[gsprm] handshake: no GSP message within budget (expected until boot-args handoff lands)"
        );
    }

    *RPC.lock() = Some(rpc);
    seen
}

/// Drive the full GSP-RM boot pipeline end to end. Non-fatal at every
/// stage: returns the furthest stage reached plus the diagnostic state so
/// a caller (shell command) can print exactly where the boot stopped and
/// why. The 'bar0' region must already be mapped (init has run)
pub fn boot(bar0: &MmioRegion, gpu: &crate::nvidia::pci::GpuDevice) -> FullBootReport {
    let mut report = FullBootReport {
        reached: BootStage::Scrubber,
        wpr2_locked: false,
        wpr2_lo: 0,
        wpr2_hi: 0,
        booter_mb1: 0,
        frts_err: 0,
        gsp_msg_function: 0,
        gsp_msg_result: 0,
    };

    // Stage 1: NVDEC scrubber (zero the WPR region). Non-fatal
    serial_println!("[gsprm] boot stage 1/6: NVDEC scrubber");
    match super::nvdec::attempt_scrub(bar0) {
        Ok(st) => serial_println!("[gsprm]   scrubber halted: mb0={:#010x} mb1={:#010x}", st.mb0, st.mb1),
        Err(e) => serial_println!("[gsprm]   scrubber aborted: {:?} (continuing)", e),
    }

    // Stage 2: stage the GSP-RM ELF + radix3 + WPR-meta in sysmem
    serial_println!("[gsprm] boot stage 2/6: stage GSP-RM image");
    let gsp_rm_fw = super::tu116_fw::gsp_rm_570();
    match load(bar0, gsp_rm_fw.bytes()) {
        Ok(rep) => {
            report.reached = BootStage::Load;
            serial_println!(
                "[gsprm]   staged: fwimage={}B radix3@{:#x} meta@{:#x} resolves={}",
                rep.fwimage_len, rep.radix3_root_phys, rep.meta_phys, rep.radix3_resolves
            );
        }
        Err(e) => {
            serial_println!("[gsprm]   load failed: {:?} - cannot proceed without a staged image", e);
            return report;
        }
    }

    // Stage 3: FWSEC FRTS. Per nouveau tu102_gsp_oneinit / ogkm
    // kgspBootstrap_TU102, FWSEC (a signed ucode from the VBIOS) runs on the
    // GSP Falcon, carves out the FRTS region and programs the WPR2 lock. This
    // is what actually locks WPR2 - the SEC2 booter, which runs later, relies
    // on the region FWSEC sets up. FRTS addr/size come from the WPR layout.
    serial_println!("[gsprm] boot stage 3/6: FWSEC FRTS on GSP (locks WPR2)");
    crate::println!("    stage 3: FWSEC FRTS on GSP (locks WPR2)");
    let frts = with_state(|s| (s.layout.frts_offset, s.layout.frts_size)).unwrap_or((0, 0));
    crate::println!("    FRTS region: addr={:#x} size={:#x}", frts.0, frts.1);
    // Prefer the PRAMIN shadow: on Turing the PCI expansion ROM only carries
    // the legacy + EFI images, while the data image (PCIR type 0xe0) with the
    // FWSEC ucode lives only in the VRAM BIOS shadow. nouveau scores PRAMIN
    // ahead of PROM for the same reason. Fall back to PROM if PRAMIN is not
    // available (e.g. display path not yet initialised).
    let (rom_src, rom_opt) = match crate::nvidia::vbios::read_rom_pramin(bar0) {
        Some(r) => ("PRAMIN", Some(r)),
        None => ("PROM", crate::nvidia::vbios::read_rom(gpu)),
    };
    match rom_opt {
        Some(rom) => {
            crate::println!("    VBIOS: read {} bytes from {}", rom.len(), rom_src);
            match super::fwsec::run_frts(bar0, &rom, frts.0, frts.1) {
                Ok(st) => {
                    report.reached = BootStage::Fwsec;
                    report.frts_err = st.frts_err;
                    report.wpr2_locked = st.wpr2_locked;
                    report.wpr2_lo = st.wpr2_lo;
                    report.wpr2_hi = st.wpr2_hi;
                    serial_println!(
                        "[gsprm]   FWSEC frts_err={:#06x} wpr2_locked={}",
                        st.frts_err, st.wpr2_locked
                    );
                    crate::println!("    FWSEC ran: mb0={:#x} frts_err={:#06x} wpr2_locked={}",
                        st.mb0, st.frts_err, st.wpr2_locked);
                }
                Err(e) => {
                    serial_println!("[gsprm]   FWSEC FRTS failed: {:?} - STOP", e);
                    crate::print_warn!("    FWSEC FRTS failed: {:?}", e);
                    return report;
                }
            }
        }
        None => {
            serial_println!("[gsprm]   could not read VBIOS for FWSEC - STOP");
            crate::print_warn!("    could not read VBIOS (rom_phys={:#x} rom_size={:#x})",
                gpu.rom_phys, gpu.rom_size);
            return report;
        }
    }

    // Stage 4: verify WPR2 locked. GATE - if FWSEC did not lock WPR2, the
    // later booter cannot DMA the GSP-RM image into a protected region
    if report.wpr2_locked {
        serial_println!(
            "[gsprm] boot stage 4/6: WPR2 LOCKED {:#x}..{:#x} ({} MiB)",
            report.wpr2_lo, report.wpr2_hi,
            (report.wpr2_hi.saturating_sub(report.wpr2_lo)) >> 20
        );
    } else {
        serial_println!("[gsprm] boot stage 4/6: WPR2 NOT locked - STOP");
        serial_println!(
            "[gsprm]   gate: FWSEC ran but WPR2 lock did not take (check frts_err + desc parse above)"
        );
        return report;
    }

    // Stage 5: boot-arg handoff + booter_load on SEC2. Per nouveau
    // r535_gsp_init the host first builds the libos boot args (CMDQ/MSGQ
    // shared region + GSP_ARGUMENTS_CACHED + the three log regions), writes
    // the libos table address into the GSP falcon MAILBOX0/1, then runs the
    // SEC2 booter (kgspExecuteBooterLoad_TU102): it DMAs the radix3 GSP-RM
    // image into the now-locked WPR2 and starts the GSP RISC-V core. The
    // WPR-meta phys is handed in SEC2 MAILBOX0/1. Finally the GSP FALCON_OS
    // register gets the bootloader app version so GSP-RM can complete init.
    serial_println!("[gsprm] boot stage 5/6: build boot args + booter_load on SEC2");

    // Build and pin the GSP-RM boot arguments. GSP-RM reads these from sysmem
    // once it is running, so they must outlive the boot; stash them statically.
    let (libos_phys, shared_phys) = match super::bootargs::GspBootArgs::build() {
        Ok(ba) => {
            let lp = ba.libos_phys();
            let sp = ba.shared_phys();
            *BOOT_ARGS.lock() = Some(ba);
            serial_println!(
                "[gsprm]   boot args built: libos @ {:#x} shared @ {:#x}", lp, sp
            );
            (lp, sp)
        }
        Err(e) => {
            serial_println!("[gsprm]   boot args alloc failed: {:?} - STOP", e);
            return report;
        }
    };

    // Hand the libos table address to the GSP falcon BEFORE the booter runs
    // (r535_gsp_oneinit: PGSP MAILBOX0 = lo32, MAILBOX1 = hi32 of libos.addr).
    {
        use super::falcon::{Engine, FALCON_MAILBOX0, FALCON_MAILBOX1, PGSP_BASE};
        let gsp = Engine::new(bar0, PGSP_BASE, "gsp");
        gsp.write(FALCON_MAILBOX0, libos_phys as u32);
        gsp.write(FALCON_MAILBOX1, (libos_phys >> 32) as u32);
        serial_println!(
            "[gsprm]   GSP MAILBOX0/1 <- libos @ {:#x} (MB0={:#010x} MB1={:#010x})",
            libos_phys, libos_phys as u32, (libos_phys >> 32) as u32
        );
    }

    let meta_phys = with_state(|s| s.meta_phys()).unwrap_or(0);
    match super::sec2::attempt_booter_load(bar0, meta_phys) {
        Ok(st) => {
            report.reached = BootStage::Wpr2Locked;
            report.booter_mb1 = st.mb1;
            if st.mb0 == 0 { report.reached = BootStage::Booter; }
            serial_println!(
                "[gsprm]   booter mb0={:#010x} mb1={:#010x} wpr2_locked={}",
                st.mb0, st.mb1, st.wpr2_locked
            );
        }
        Err(e) => {
            serial_println!("[gsprm]   booter_load aborted: {:?} - STOP", e);
            return report;
        }
    }

    // After the booter hands off, set the GSP FALCON_OS register to the
    // bootloader app version (r535_gsp_booter_load: PGSP+0x080 = app_version).
    // GSP-RM reads this during its own init handshake.
    {
        use super::falcon::{Engine, FALCON_CPUCTL, FALCON_OS, PGSP_BASE};
        let app_version = with_state(|s| s.app_version).unwrap_or(0);
        let gsp = Engine::new(bar0, PGSP_BASE, "gsp");
        gsp.write(FALCON_OS, app_version);
        serial_println!(
            "[gsprm]   GSP FALCON_OS <- app_version={:#x}; GSP cpuctl={:#010x}",
            app_version, gsp.read(FALCON_CPUCTL)
        );
    }

    // Stage 6: post-boot handshake. Poll the GSP-owned MSGQ in the shared
    // region (the one whose address we passed through the boot args) for
    // GSP-RM's first posted message - the GSP_INIT_DONE event.
    serial_println!("[gsprm] boot stage 6/6: GSP-RM MSGQ handshake (shared @ {:#x})", shared_phys);
    let msg = BOOT_ARGS.lock().as_ref().and_then(|ba| {
        ba.poll_first_message(
            || bar0.read32(PTIMER_TIME_0),
            GSP_HANDSHAKE_TIMEOUT_NS,
        )
    });
    match msg {
        Some((function, signature)) => {
            report.reached = BootStage::GspHandshake;
            report.gsp_msg_function = function;
            report.gsp_msg_result = signature;
            let init_done = function == super::bootargs::NV_VGPU_MSG_EVENT_GSP_INIT_DONE;
            serial_println!(
                "[gsprm] FULL BOOT: GSP-RM posted a message (function={:#x} signature={:#010x}{})",
                function, signature,
                if init_done { " = GSP_INIT_DONE" } else { "" }
            );
            if signature != super::bootargs::GSP_RPC_SIGNATURE {
                serial_println!(
                    "[gsprm]   note: rpc signature {:#010x} != 'VGPU' - element may not be a real RPC yet",
                    signature
                );
            }
        }
        None => serial_println!(
            "[gsprm]   no GSP message within budget - booter ran but GSP-RM has not posted to the MSGQ"
        ),
    }

    report
}
