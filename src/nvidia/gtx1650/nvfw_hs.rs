// Heavy-Secure (HS) firmware container parsing for Turing ACR images
//
// NVIDIA wraps every signed HS ucode in three nested headers:
//
//   [bin_hdr] [hs_header] [hs_load_header [+app[]]] [code/data]
//      24B       32B        12B + 8*N        ucode payload
//
// The outer nvfw_bin_hdr lives in tu116_fw::NvfwBinHdr. The middle and
// inner headers are described here. Layouts come straight from
// nouveau (drivers/gpu/drm/nouveau/include/nvfw/hs.h) and have been
// stable across Maxwell..Turing; HS-v2 (9-field) is used by newer
// Ampere/Ada images and is not what TU116 ACR ships
//
// All offsets in the HS header are absolute within the NVFW container
// blob, not relative to header_offset. Confirmed against TU116 ACR
// dumps where header_offset=0x100, sig_dbg_offset=0x120 (i.e. 0x20
// past the HS header itself), and the signature bytes at container
// offset 0x120 match the layout exactly

#![allow(dead_code)]

/// HS header (legacy / pre-v2 layout). 8 u32 = 32 bytes
///
/// Source: drivers/gpu/drm/nouveau/include/nvfw/hs.h 'struct nvfw_hs_header'.
#[derive(Copy, Clone, Debug)]
pub struct NvfwHsHeader {
    /// Absolute offset within the NVFW blob to the debug signature(s)
    pub sig_dbg_offset:  u32,
    /// Total bytes of debug signature data (typically 16 per signature)
    pub sig_dbg_size:    u32,
    /// Absolute offset within the NVFW blob to the production signature(s)
    pub sig_prod_offset: u32,
    /// Total bytes of production signature data
    pub sig_prod_size:   u32,
    /// Absolute offset to a u32 holding the patch-location offset
    pub patch_loc:       u32,
    /// Absolute offset to a u32 holding the patch-signature offset
    pub patch_sig:       u32,
    /// Absolute offset to the HS load-header that follows
    pub hdr_offset:      u32,
    /// Size in bytes of the HS load-header (including its app[] tail)
    pub hdr_size:        u32,
}

impl NvfwHsHeader {
    pub const SIZE: usize = 32;

    /// Parse 32 bytes starting at 'at' inside 'blob'. Returns None if the
    /// blob is too short or any declared offset/size lies outside the blob
    pub fn parse(blob: &[u8], at: usize) -> Option<Self> {
        if at.checked_add(Self::SIZE)? > blob.len() { return None; }
        let r = |o: usize| u32::from_le_bytes(
            [blob[at+o], blob[at+o+1], blob[at+o+2], blob[at+o+3]]);
        let h = NvfwHsHeader {
            sig_dbg_offset:  r(0),
            sig_dbg_size:    r(4),
            sig_prod_offset: r(8),
            sig_prod_size:   r(12),
            patch_loc:       r(16),
            patch_sig:       r(20),
            hdr_offset:      r(24),
            hdr_size:        r(28),
        };

        // Sanity - every declared region must lie within the blob
        let blob_end = blob.len() as u32;
        let in_bounds = |off: u32, size: u32| -> bool {
            off.checked_add(size).map(|e| e <= blob_end).unwrap_or(false)
        };
        if !in_bounds(h.sig_dbg_offset, h.sig_dbg_size)   { return None; }
        if !in_bounds(h.sig_prod_offset, h.sig_prod_size) { return None; }
        if !in_bounds(h.hdr_offset, h.hdr_size)           { return None; }
        // patch_loc / patch_sig point to a u32 each
        if !in_bounds(h.patch_loc, 4) { return None; }
        if !in_bounds(h.patch_sig, 4) { return None; }

        Some(h)
    }

    /// Read the u32 sitting at the patch_loc pointer
    pub fn read_patch_loc_value(&self, blob: &[u8]) -> Option<u32> {
        read_u32(blob, self.patch_loc as usize)
    }

    /// Read the u32 sitting at the patch_sig pointer
    pub fn read_patch_sig_value(&self, blob: &[u8]) -> Option<u32> {
        read_u32(blob, self.patch_sig as usize)
    }

    /// Check whether the parsed HS-header values are self-consistent
    /// against the enclosing NVFW container. The HS header lives in the
    /// region [container_hdr_off .. container_data_off]. All declared
    /// signature/load-header offsets must point inside that region,
    /// strictly past the 32-byte HS header itself
    ///
    /// bl.bin / unload_bl.bin are NVFW-wrapped but do NOT carry an HS
    /// header - their 0x100..0x200 region is an LS-style bootloader
    /// descriptor. Reading 32 bytes there yields garbage values that
    /// fail this check
    pub fn looks_valid(&self, container_hdr_off: u32, container_data_off: u32) -> bool {
        let hdr_min = container_hdr_off.saturating_add(Self::SIZE as u32);
        let hdr_max = container_data_off;
        let in_hdr_region = |off: u32, size: u32| -> bool {
            let end = match off.checked_add(size) { Some(e) => e, None => return false };
            off >= hdr_min && end <= hdr_max
        };
        in_hdr_region(self.sig_dbg_offset,  self.sig_dbg_size)
            && in_hdr_region(self.sig_prod_offset, self.sig_prod_size)
            && in_hdr_region(self.hdr_offset, self.hdr_size)
            && in_hdr_region(self.patch_loc, 4)
            && in_hdr_region(self.patch_sig, 4)
    }
}

/// HS load header
///
/// Layout (drivers/gpu/drm/nouveau/include/nvfw/hs.h
/// 'struct nvfw_hs_load_header'):
///
///   u32 non_sec_code_off    // LS-style code offset, payload-relative
///   u32 non_sec_code_size
///   u32 data_dma_base       // DMA base for the data segment
///   u32 data_size
///   u32 num_apps
///   struct { u32 code_off; u32 code_size; } app[num_apps];
///
#[derive(Clone, Debug)]
pub struct NvfwHsLoadHeader {
    /// Non-secure (LS-style) code offset within the payload region
    pub non_sec_code_off:  u32,
    /// Non-secure code size in bytes
    pub non_sec_code_size: u32,
    /// DMA base address used when uploading the data segment
    pub data_dma_base:     u32,
    /// Data segment size in bytes
    pub data_size:         u32,
    /// Number of secure apps that follow
    pub num_apps:          u32,
    /// Per-app (sec_code_off, sec_code_size) pairs
    pub apps:              [(u32, u32); MAX_APPS],
}

/// Cap on apps we will parse. ACR HS images on Turing have 1-2 apps; we
/// keep a small fixed-size array to avoid heap usage in 'no_std'
pub const MAX_APPS: usize = 8;

impl NvfwHsLoadHeader {
    pub const PROLOGUE: usize = 20;

    /// Parse the load-header at 'at' inside 'blob', given the declared
    /// 'hdr_size' (from the enclosing HS header). Layout is a 5 u32
    /// prologue followed by 'num_apps' (code_off, code_size) pairs
    pub fn parse(blob: &[u8], at: usize, hdr_size: u32) -> Option<Self> {
        let hdr_size = hdr_size as usize;
        if at.checked_add(hdr_size)? > blob.len() { return None; }
        if hdr_size < Self::PROLOGUE { return None; }

        let r = |o: usize| u32::from_le_bytes(
            [blob[at+o], blob[at+o+1], blob[at+o+2], blob[at+o+3]]);

        let non_sec_code_off  = r(0);
        let non_sec_code_size = r(4);
        let data_dma_base     = r(8);
        let data_size         = r(12);
        let num_apps          = r(16);

        let app_count = num_apps as usize;
        if app_count > MAX_APPS { return None; }
        let need = Self::PROLOGUE + app_count * 8;
        if hdr_size < need { return None; }

        let mut apps = [(0u32, 0u32); MAX_APPS];
        for i in 0..app_count {
            apps[i] = (r(Self::PROLOGUE + i*8), r(Self::PROLOGUE + i*8 + 4));
        }

        Some(NvfwHsLoadHeader {
            non_sec_code_off,
            non_sec_code_size,
            data_dma_base,
            data_size,
            num_apps,
            apps,
        })
    }

    /// Iterator over (code_off, code_size) for each declared app
    pub fn apps_iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        let n = (self.num_apps as usize).min(MAX_APPS);
        (0..n).map(move |i| self.apps[i])
    }
}

#[inline]
fn read_u32(blob: &[u8], off: usize) -> Option<u32> {
    if off.checked_add(4)? > blob.len() { return None; }
    Some(u32::from_le_bytes([blob[off], blob[off+1], blob[off+2], blob[off+3]]))
}
