// TU116 firmware bundle
//
// All 26 firmware blobs shipped under src/nvidia/gtx1650/tu116/ are embedded
// into the kernel image via include_bytes! and exposed as &'static [u8]
// slices. Layout mirrors the directory tree:
//
//   acr/       ACR (Access Control Region) HS bins for SEC2:
//              ucode_ahesasc, ucode_asb, ucode_unload, plus their bootloaders
//              (bl, unload_bl). These are the first thing SEC2 runs to set
//              up the WPR (Write Protect Region) in VRAM where signed
//              ucode is stored.
//
//   gsp/       GSP "booter" HS images. booter_load brings GSP up; booter_unload
//              tears it down. Two firmware lines are shipped:
//                535.113.01 -- Long-term-support
//                570.144    -- newer driver line
//              Pick by feature set; 570.144 supports more RPC verbs and is
//              the default unless the user pins the LTS line.
//
//   nvdec/     NVDEC scrubber. Run before VRAM is touched to clear any
//              firmware-leftover state and zero the WPR region.
//
//   sec2/      SEC2 image, descriptor and signature. The descriptor (desc.bin)
//              is a Falcon ucode descriptor; the image is the running text;
//              sig.bin is the production signature blob.
//
//   gr/        Graphics-engine context: FECS (Frontend) and GPCCS (per-GPC)
//              bootloaders, instance images, signatures and data segments,
//              plus the host-side "sw_*" tables that program PGRAPH state.
//
// All blobs are NVIDIA-proprietary and are loaded as opaque byte slices.
// Parsing is restricted to the public NVFW container header
// (nvfw_bin_hdr, magic 0x000010de) which is documented in nouveau and the
// open-gpu-kernel-modules tree

#![allow(dead_code)]

// ACR (SEC2 access-control region)
pub static ACR_BL:           &[u8] = include_bytes!("tu116/acr/bl.bin");
pub static ACR_UNLOAD_BL:    &[u8] = include_bytes!("tu116/acr/unload_bl.bin");
pub static ACR_AHESASC:      &[u8] = include_bytes!("tu116/acr/ucode_ahesasc.bin");
pub static ACR_ASB:          &[u8] = include_bytes!("tu116/acr/ucode_asb.bin");
pub static ACR_UNLOAD:       &[u8] = include_bytes!("tu116/acr/ucode_unload.bin");

// GSP booter (two driver lines)
pub static BOOTER_LOAD_535:   &[u8] = include_bytes!("tu116/gsp/booter_load-535.113.01.bin");
pub static BOOTER_UNLOAD_535: &[u8] = include_bytes!("tu116/gsp/booter_unload-535.113.01.bin");
pub static BOOTER_LOAD_570:   &[u8] = include_bytes!("tu116/gsp/booter_load-570.144.bin");
pub static BOOTER_UNLOAD_570: &[u8] = include_bytes!("tu116/gsp/booter_unload-570.144.bin");

// NVDEC
pub static NVDEC_SCRUBBER:    &[u8] = include_bytes!("tu116/nvdec/scrubber.bin");

// SEC2
pub static SEC2_DESC:         &[u8] = include_bytes!("tu116/sec2/desc.bin");
pub static SEC2_IMAGE:        &[u8] = include_bytes!("tu116/sec2/image.bin");
pub static SEC2_SIG:          &[u8] = include_bytes!("tu116/sec2/sig.bin");

// Graphics engine
pub static FECS_BL:           &[u8] = include_bytes!("tu116/gr/fecs_bl.bin");
pub static FECS_INST:         &[u8] = include_bytes!("tu116/gr/fecs_inst.bin");
pub static FECS_DATA:         &[u8] = include_bytes!("tu116/gr/fecs_data.bin");
pub static FECS_SIG:          &[u8] = include_bytes!("tu116/gr/fecs_sig.bin");
pub static GPCCS_BL:          &[u8] = include_bytes!("tu116/gr/gpccs_bl.bin");
pub static GPCCS_INST:        &[u8] = include_bytes!("tu116/gr/gpccs_inst.bin");
pub static GPCCS_DATA:        &[u8] = include_bytes!("tu116/gr/gpccs_data.bin");
pub static GPCCS_SIG:         &[u8] = include_bytes!("tu116/gr/gpccs_sig.bin");
pub static SW_BUNDLE_INIT:    &[u8] = include_bytes!("tu116/gr/sw_bundle_init.bin");
pub static SW_CTX:            &[u8] = include_bytes!("tu116/gr/sw_ctx.bin");
pub static SW_METHOD_INIT:    &[u8] = include_bytes!("tu116/gr/sw_method_init.bin");
pub static SW_NONCTX:         &[u8] = include_bytes!("tu116/gr/sw_nonctx.bin");
pub static SW_VEID_INIT:      &[u8] = include_bytes!("tu116/gr/sw_veid_bundle_init.bin");

// nvfw_bin_hdr - NVIDIA firmware container header
// Magic 0x000010de marks blobs that are NVFW-wrapped (booter, acr/ahesasc, acr/asb, acr/unload, scrubber). The plain-text header layout, same as in
// nouveau (drivers/gpu/drm/nouveau/include/nvfw/fw.h):
//
//   u32 bin_magic        // 0x000010de
//   u32 bin_ver          // currently 1
//   u32 bin_size         // total file size
//   u32 header_offset    // offset to per-image header (HS/LS/etc.)
//   u32 data_offset      // offset to opaque ucode data
//   u32 data_size        // size of ucode data
//
// The "header" pointed at by header_offset is a HS-bin or Falcon-ucode-desc
// header; we don't parse those here - the SEC2/Falcon driver will, once
// it can address the engine and stage data via a real DMA path.

pub const NVFW_BIN_MAGIC: u32 = 0x0000_10de;

#[derive(Copy, Clone, Debug)]
pub struct NvfwBinHdr {
    pub bin_magic:     u32,
    pub bin_ver:       u32,
    pub bin_size:      u32,
    pub header_offset: u32,
    pub data_offset:   u32,
    pub data_size:     u32,
}

impl NvfwBinHdr {
    /// Parse the leading 24 bytes of an NVFW-wrapped blob
    /// Returns None if the magic mismatches or the buffer is too short
    pub fn parse(blob: &[u8]) -> Option<Self> {
        if blob.len() < 24 { return None; }
        let r = |o: usize| u32::from_le_bytes([blob[o], blob[o+1], blob[o+2], blob[o+3]]);
        let h = NvfwBinHdr {
            bin_magic:     r(0),
            bin_ver:       r(4),
            bin_size:      r(8),
            header_offset: r(12),
            data_offset:   r(16),
            data_size:     r(20),
        };
        if h.bin_magic != NVFW_BIN_MAGIC { return None; }
        let end = h.data_offset.checked_add(h.data_size)?;
        if (end as usize) > blob.len() { return None; }
        if (h.header_offset as usize) >= blob.len() { return None; }
        Some(h)
    }

    /// Return the opaque ucode payload as a slice over the original blob
    pub fn data<'a>(&self, blob: &'a [u8]) -> &'a [u8] {
        let s = self.data_offset as usize;
        let e = s + self.data_size as usize;
        &blob[s..e]
    }
}

// Bundle - enumerates every embedded blob with a printable name and the engine that consumes it. Used by the probe step in init.rs

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Engine {
    Sec2,
    Gsp,
    Nvdec,
    Fecs,
    Gpccs,
    HostSw,
}

#[derive(Copy, Clone)]
pub struct BlobInfo {
    pub name:    &'static str,
    pub engine:  Engine,
    pub bytes:   &'static [u8],
    /// True if this blob is wrapped in an nvfw_bin_hdr container; false for
    /// raw ucode (sec2 image, fecs/gpccs segments, sw_ host tables)
    pub wrapped: bool,
}

/// Default GSP firmware line for TU116
/// 570.144 supports the full RPC set used by the rest of the driver
pub const GSP_DEFAULT_LINE: &str = "570.144";

/// All firmware blobs needed for a full TU116 bring-up. The order is the
/// order in which they are consumed at runtime:
///   1. NVDEC scrubber (zero the WPR region in VRAM)
///   2. SEC2 ACR ucodes (set up WPR)
///   3. GSP booter_load (start GSP-RM)
///   4. FECS / GPCCS (graphics-engine context)
///   5. host-side sw_* tables (PGRAPH state init via host CPU)
///
/// Teardown reverses the order and uses booter_unload + acr/unload
pub const TU116_FIRMWARE: &[BlobInfo] = &[
    BlobInfo { name: "nvdec/scrubber",            engine: Engine::Nvdec,  bytes: NVDEC_SCRUBBER,    wrapped: true  },
    BlobInfo { name: "sec2/desc",                 engine: Engine::Sec2,   bytes: SEC2_DESC,         wrapped: false },
    BlobInfo { name: "sec2/image",                engine: Engine::Sec2,   bytes: SEC2_IMAGE,        wrapped: false },
    BlobInfo { name: "sec2/sig",                  engine: Engine::Sec2,   bytes: SEC2_SIG,          wrapped: false },
    BlobInfo { name: "acr/bl",                    engine: Engine::Sec2,   bytes: ACR_BL,            wrapped: true  },
    BlobInfo { name: "acr/unload_bl",             engine: Engine::Sec2,   bytes: ACR_UNLOAD_BL,     wrapped: true  },
    BlobInfo { name: "acr/ucode_ahesasc",         engine: Engine::Sec2,   bytes: ACR_AHESASC,       wrapped: true  },
    BlobInfo { name: "acr/ucode_asb",             engine: Engine::Sec2,   bytes: ACR_ASB,           wrapped: true  },
    BlobInfo { name: "acr/ucode_unload",          engine: Engine::Sec2,   bytes: ACR_UNLOAD,        wrapped: true  },
    BlobInfo { name: "gsp/booter_load-570.144",   engine: Engine::Gsp,    bytes: BOOTER_LOAD_570,   wrapped: true  },
    BlobInfo { name: "gsp/booter_unload-570.144", engine: Engine::Gsp,    bytes: BOOTER_UNLOAD_570, wrapped: true  },
    BlobInfo { name: "gr/fecs_bl",                engine: Engine::Fecs,   bytes: FECS_BL,           wrapped: false },
    BlobInfo { name: "gr/fecs_inst",              engine: Engine::Fecs,   bytes: FECS_INST,         wrapped: false },
    BlobInfo { name: "gr/fecs_data",              engine: Engine::Fecs,   bytes: FECS_DATA,         wrapped: false },
    BlobInfo { name: "gr/fecs_sig",               engine: Engine::Fecs,   bytes: FECS_SIG,          wrapped: false },
    BlobInfo { name: "gr/gpccs_bl",               engine: Engine::Gpccs,  bytes: GPCCS_BL,          wrapped: false },
    BlobInfo { name: "gr/gpccs_inst",             engine: Engine::Gpccs,  bytes: GPCCS_INST,        wrapped: false },
    BlobInfo { name: "gr/gpccs_data",             engine: Engine::Gpccs,  bytes: GPCCS_DATA,        wrapped: false },
    BlobInfo { name: "gr/gpccs_sig",              engine: Engine::Gpccs,  bytes: GPCCS_SIG,         wrapped: false },
    BlobInfo { name: "gr/sw_bundle_init",         engine: Engine::HostSw, bytes: SW_BUNDLE_INIT,    wrapped: false },
    BlobInfo { name: "gr/sw_ctx",                 engine: Engine::HostSw, bytes: SW_CTX,            wrapped: false },
    BlobInfo { name: "gr/sw_method_init",         engine: Engine::HostSw, bytes: SW_METHOD_INIT,    wrapped: false },
    BlobInfo { name: "gr/sw_nonctx",              engine: Engine::HostSw, bytes: SW_NONCTX,         wrapped: false },
    BlobInfo { name: "gr/sw_veid_bundle_init",    engine: Engine::HostSw, bytes: SW_VEID_INIT,      wrapped: false },
];

/// Total embedded bytes for the TU116 firmware bundle
pub fn total_size() -> usize {
    TU116_FIRMWARE.iter().map(|b| b.bytes.len()).sum()
}
