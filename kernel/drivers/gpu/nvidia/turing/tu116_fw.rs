// TU116 firmware bundle
//
// 26 firmware blobs on the read-only firmware store (see `fwload.rs`), laid out
// like Linux /lib/firmware:
//
//   /nvidia/tu116/acr/      ACR (Access Control Region) HS bins for SEC2:
//              ucode_ahesasc, ucode_asb, ucode_unload, plus their bootloaders
//              (bl, unload_bl). SEC2 runs these first to set up the WPR
//              (Write Protect Region) in VRAM, which holds signed ucode.
//
//   /nvidia/tu116/gsp/      GSP "booter" HS images and the GSP-RM image:
//                booter_load brings GSP up; booter_unload tears it down.
//                gsp-570.144.bin is the ~29 MiB GSP-RM RISC-V OS.
//
//   /nvidia/tu116/nvdec/    NVDEC scrubber. Runs before VRAM is touched, to
//                clear firmware-leftover state and zero the WPR region.
//
//   /nvidia/tu116/sec2/     SEC2 image, descriptor and signature.
//
//   /nvidia/tu116/gr/       Graphics-engine context: FECS / GPCCS bootloaders,
//                instance images, signatures and data segments, plus the
//                host-side sw_* tables that program PGRAPH state.
//
// All blobs are NVIDIA-proprietary, loaded as opaque byte slices via
// fwload::request(). Parsing is restricted to the public NVFW container header
// (nvfw_bin_hdr, magic 0x000010de) documented in nouveau and
// open-gpu-kernel-modules.
//
// Each blob is fetched on demand: the driver calls the matching helper below
// (e.g. acr_bl()), uses the bytes, and drops the Firmware to free the buffer
// (the request_firmware/release_firmware model). An absent or unreadable blob
// comes back empty and logged, and fails in the caller's
// NvfwBinHdr::parse(..).ok_or(..).

#![allow(dead_code)]

use crate::kcore::firmware::load::{self as fwload, Firmware};

/// Root of the TU116 firmware tree on the firmware store.
const FW_DIR: &str = "nvidia/tu116";

/// Fetch a blob by its path relative to FW_DIR; empty + logged on miss. Call
/// sites name only the file (e.g. "acr/bl.bin").
fn fetch(rel: &str) -> Firmware {
    // Build "nvidia/tu116/<rel>" without needing format! machinery.
    let mut path = alloc::string::String::with_capacity(FW_DIR.len() + 1 + rel.len());
    path.push_str(FW_DIR);
    path.push('/');
    path.push_str(rel);
    fwload::request_or_empty(&path)
}

// ACR (SEC2 access-control region)
pub fn acr_bl()        -> Firmware { fetch("acr/bl.bin") }
pub fn acr_unload_bl() -> Firmware { fetch("acr/unload_bl.bin") }
pub fn acr_ahesasc()   -> Firmware { fetch("acr/ucode_ahesasc.bin") }
pub fn acr_asb()       -> Firmware { fetch("acr/ucode_asb.bin") }
pub fn acr_unload()    -> Firmware { fetch("acr/ucode_unload.bin") }

// GSP RISC-V bootloader (the "monitor" loaded into WPR2; NVFW-wrapped, with an
// RM_RISCV_UCODE_DESC at header_offset). Not the generic SEC2 bootloader
// (acr/bl): the WPR-meta bootloader fields point at this one, and the booter
// DMAs it into WPR2 to bring up the GSP RISC-V core.
pub fn gsp_bootloader_570() -> Firmware { fetch("gsp/bootloader-570.144.bin") }

// GSP booter (570.144 line)
pub fn booter_load_570()   -> Firmware { fetch("gsp/booter_load-570.144.bin") }
pub fn booter_unload_570() -> Firmware { fetch("gsp/booter_unload-570.144.bin") }

// GSP-RM image ELF (gsp-<line>.bin), ~29 MiB. An absent store returns an empty
// Firmware, and the GSP-RM load path reports MissingFirmware.
pub fn gsp_rm_570() -> Firmware { fetch("gsp/gsp-570.144.bin") }

// NVDEC
pub fn nvdec_scrubber() -> Firmware { fetch("nvdec/scrubber.bin") }

// SEC2
pub fn sec2_desc()  -> Firmware { fetch("sec2/desc.bin") }
pub fn sec2_image() -> Firmware { fetch("sec2/image.bin") }
pub fn sec2_sig()   -> Firmware { fetch("sec2/sig.bin") }

// Graphics engine
pub fn fecs_bl()         -> Firmware { fetch("gr/fecs_bl.bin") }
pub fn fecs_inst()       -> Firmware { fetch("gr/fecs_inst.bin") }
pub fn fecs_data()       -> Firmware { fetch("gr/fecs_data.bin") }
pub fn fecs_sig()        -> Firmware { fetch("gr/fecs_sig.bin") }
pub fn gpccs_bl()        -> Firmware { fetch("gr/gpccs_bl.bin") }
pub fn gpccs_inst()      -> Firmware { fetch("gr/gpccs_inst.bin") }
pub fn gpccs_data()      -> Firmware { fetch("gr/gpccs_data.bin") }
pub fn gpccs_sig()       -> Firmware { fetch("gr/gpccs_sig.bin") }
pub fn sw_bundle_init()  -> Firmware { fetch("gr/sw_bundle_init.bin") }
pub fn sw_ctx()          -> Firmware { fetch("gr/sw_ctx.bin") }
pub fn sw_method_init()  -> Firmware { fetch("gr/sw_method_init.bin") }
pub fn sw_nonctx()       -> Firmware { fetch("gr/sw_nonctx.bin") }
pub fn sw_veid_init()    -> Firmware { fetch("gr/sw_veid_bundle_init.bin") }

// nvfw_bin_hdr: NVIDIA firmware container header. Magic 0x000010de marks
// NVFW-wrapped blobs (booter, acr/ahesasc, acr/asb, acr/unload, scrubber).
// Layout as in nouveau (drivers/gpu/drm/nouveau/include/nvfw/fw.h):
//
//   u32 bin_magic        // 0x000010de
//   u32 bin_ver          // currently 1
//   u32 bin_size         // total file size
//   u32 header_offset    // offset to per-image header (HS/LS/etc.)
//   u32 data_offset      // offset to opaque ucode data
//   u32 data_size        // size of ucode data
//
// The "header" at header_offset is a HS-bin or Falcon-ucode-desc header, parsed
// by the SEC2/Falcon driver.

// The NVFW container header is in `nvidia/nvfw_hs.rs`: the same 24 bytes
// precede every signed NVIDIA blob. Re-exported as `tu116_fw::NvfwBinHdr` for
// this driver.
pub use crate::drivers::gpu::nvidia::nvfw_hs::NvfwBinHdr;

// Every firmware file with a printable name, its path on the firmware store,
// and the consuming engine. For the probe step in init.rs and the diagnostic
// commands, which stat sizes via fwload::size_of and parse headers via
// fwload::request.

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
    /// Short logical name used in diagnostics (e.g. "acr/bl").
    pub name:    &'static str,
    /// Path on the firmware store relative to its root (e.g.
    /// "nvidia/tu116/acr/bl.bin"), for fwload::size_of / fwload::request.
    pub path:    &'static str,
    pub engine:  Engine,
    /// True if this blob is wrapped in an nvfw_bin_hdr container; false for
    /// raw ucode (sec2 image, fecs/gpccs segments, sw_ host tables)
    pub wrapped: bool,
    /// True for blobs that may be absent (the GSP-RM image, when the store
    /// ships without it).
    pub optional: bool,
}

/// Default GSP firmware line for TU116
/// 570.144 supports the full RPC set used by the rest of the driver
pub const GSP_DEFAULT_LINE: &str = "570.144";

/// All firmware files for a full TU116 bring-up, in runtime order:
///   1. NVDEC scrubber (zero the WPR region in VRAM)
///   2. SEC2 ACR ucodes (set up WPR)
///   3. GSP booter_load (start GSP-RM)
///   4. FECS / GPCCS (graphics-engine context)
///   5. host-side sw_* tables (PGRAPH state init via host CPU)
///
/// Teardown reverses the order, with booter_unload + acr/unload.
pub const TU116_FIRMWARE: &[BlobInfo] = &[
    BlobInfo { name: "nvdec/scrubber",            path: "nvidia/tu116/nvdec/scrubber.bin",            engine: Engine::Nvdec,  wrapped: true,  optional: false },
    BlobInfo { name: "sec2/desc",                 path: "nvidia/tu116/sec2/desc.bin",                 engine: Engine::Sec2,   wrapped: false, optional: false },
    BlobInfo { name: "sec2/image",                path: "nvidia/tu116/sec2/image.bin",                engine: Engine::Sec2,   wrapped: false, optional: false },
    BlobInfo { name: "sec2/sig",                  path: "nvidia/tu116/sec2/sig.bin",                  engine: Engine::Sec2,   wrapped: false, optional: false },
    BlobInfo { name: "acr/bl",                    path: "nvidia/tu116/acr/bl.bin",                    engine: Engine::Sec2,   wrapped: true,  optional: false },
    BlobInfo { name: "acr/unload_bl",             path: "nvidia/tu116/acr/unload_bl.bin",             engine: Engine::Sec2,   wrapped: true,  optional: false },
    BlobInfo { name: "acr/ucode_ahesasc",         path: "nvidia/tu116/acr/ucode_ahesasc.bin",         engine: Engine::Sec2,   wrapped: true,  optional: false },
    BlobInfo { name: "acr/ucode_asb",             path: "nvidia/tu116/acr/ucode_asb.bin",             engine: Engine::Sec2,   wrapped: true,  optional: false },
    BlobInfo { name: "acr/ucode_unload",          path: "nvidia/tu116/acr/ucode_unload.bin",          engine: Engine::Sec2,   wrapped: true,  optional: false },
    BlobInfo { name: "gsp/booter_load-570.144",   path: "nvidia/tu116/gsp/booter_load-570.144.bin",   engine: Engine::Gsp,    wrapped: true,  optional: false },
    BlobInfo { name: "gsp/booter_unload-570.144", path: "nvidia/tu116/gsp/booter_unload-570.144.bin", engine: Engine::Gsp,    wrapped: true,  optional: false },
    BlobInfo { name: "gr/fecs_bl",                path: "nvidia/tu116/gr/fecs_bl.bin",                engine: Engine::Fecs,   wrapped: false, optional: false },
    BlobInfo { name: "gr/fecs_inst",              path: "nvidia/tu116/gr/fecs_inst.bin",              engine: Engine::Fecs,   wrapped: false, optional: false },
    BlobInfo { name: "gr/fecs_data",              path: "nvidia/tu116/gr/fecs_data.bin",              engine: Engine::Fecs,   wrapped: false, optional: false },
    BlobInfo { name: "gr/fecs_sig",               path: "nvidia/tu116/gr/fecs_sig.bin",               engine: Engine::Fecs,   wrapped: false, optional: false },
    BlobInfo { name: "gr/gpccs_bl",               path: "nvidia/tu116/gr/gpccs_bl.bin",               engine: Engine::Gpccs,  wrapped: false, optional: false },
    BlobInfo { name: "gr/gpccs_inst",             path: "nvidia/tu116/gr/gpccs_inst.bin",             engine: Engine::Gpccs,  wrapped: false, optional: false },
    BlobInfo { name: "gr/gpccs_data",             path: "nvidia/tu116/gr/gpccs_data.bin",             engine: Engine::Gpccs,  wrapped: false, optional: false },
    BlobInfo { name: "gr/gpccs_sig",              path: "nvidia/tu116/gr/gpccs_sig.bin",              engine: Engine::Gpccs,  wrapped: false, optional: false },
    BlobInfo { name: "gr/sw_bundle_init",         path: "nvidia/tu116/gr/sw_bundle_init.bin",         engine: Engine::HostSw, wrapped: false, optional: false },
    BlobInfo { name: "gr/sw_ctx",                 path: "nvidia/tu116/gr/sw_ctx.bin",                 engine: Engine::HostSw, wrapped: false, optional: false },
    BlobInfo { name: "gr/sw_method_init",         path: "nvidia/tu116/gr/sw_method_init.bin",         engine: Engine::HostSw, wrapped: false, optional: false },
    BlobInfo { name: "gr/sw_nonctx",              path: "nvidia/tu116/gr/sw_nonctx.bin",              engine: Engine::HostSw, wrapped: false, optional: false },
    BlobInfo { name: "gr/sw_veid_bundle_init",    path: "nvidia/tu116/gr/sw_veid_bundle_init.bin",    engine: Engine::HostSw, wrapped: false, optional: false },
];

/// Total bytes of the TU116 firmware bundle, from the firmware store without
/// reading file data. 0 when no store is mounted.
pub fn total_size() -> u64 {
    TU116_FIRMWARE
        .iter()
        .filter_map(|b| fwload::size_of(b.path))
        .sum()
}
