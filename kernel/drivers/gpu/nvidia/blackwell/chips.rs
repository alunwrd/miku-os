// Every GB20x this driver brings up, and the little that differs between them
//
// A chip is recognised as OpenRM recognises it: by PMC_BOOT_42, whose
// ARCHITECTURE and IMPLEMENTATION `gpumgr` hands the chip HAL
// (`gpu_mgr.c:1264-1265`; the fields are `nv_ref.h:149-167`). GB20x is
// architecture 0x1B, and the implementation picks the chip
// (`g_chips2halspec_nvoc.c:112-136`).
//
// Every OpenRM function this driver copies resolves to the same implementation
// on all five (`kgspResetHw` to GB100, `kfspWaitForSecureBoot` to GB202, the
// sysmem flush to GB10B, `kgspIsWpr2Up` to GH100), and the firmware is one set
// for the family: in linux-firmware gb203, gb205, gb206 and gb207 are symlinks
// to gb202 (`WHENCE`), and GSP-RM is ga102's image (`gsp_ga10x.bin` in the
// driver package), carrying the GB20x signature in `.fwsignature_gb20x`, with
// `ucodes_ga10x.bin` beside it. `dev/nvidia/fw_extract.py` builds the set
// (`dev/gsp-615.md`).
//
// The directory is the chip's own, as linux-firmware and nouveau name it
// (`nvidia/<chip>/gsp/...`). The build stages one folder per card
// (`NVIDIA_FW_CARDS` in `builder/src/main.rs`), only `nvidia/gb206` for now;
// another GB20x needs its folder staged the same way.

/// One chip of the family
#[derive(Debug)]
pub struct Chip {
    pub codename: &'static str,
    /// PMC_BOOT_42 IMPLEMENTATION (23:20)
    pub implementation: u8,
    /// `/lib/firmware` directory, `nvidia/<chip>`
    pub fw_dir: &'static str,
    /// The files a boot loads, under `fw_dir`
    pub fmc: &'static str,
    pub bootloader: &'static str,
    pub gsp: &'static str,
    /// The ucodes container GSP-RM takes its bindata from (r610 on)
    pub ucodes: &'static str,
    /// GSP-RM's logging ELF: the debug half of the same build, from NVIDIA's
    /// .run package, not linux-firmware, and optional (`gsp_common::logdecode`)
    pub gsp_log: &'static str,
    /// The section of the GSP-RM image that holds this chip's signature
    pub sig_section: &'static str,
}

/// PMC_BOOT_42 ARCHITECTURE for GB20x (`NV_PMC_BOOT_42_ARCHITECTURE_GB200`)
pub const ARCHITECTURE: u8 = 0x1B;

macro_rules! chip {
    ($name:literal, $dir:literal, $impl:literal) => {
        Chip {
            codename: $name,
            implementation: $impl,
            fw_dir: concat!("nvidia/", $dir),
            fmc: concat!("nvidia/", $dir, "/gsp/fmc-", crate::gsp_rm_version!(), ".bin"),
            bootloader: concat!("nvidia/", $dir, "/gsp/bootloader-", crate::gsp_rm_version!(), ".bin"),
            gsp: concat!("nvidia/", $dir, "/gsp/gsp-", crate::gsp_rm_version!(), ".bin"),
            ucodes: concat!("nvidia/", $dir, "/gsp/ucodes-", crate::gsp_rm_version!(), ".bin"),
            gsp_log: concat!("nvidia/", $dir, "/gsp/gsp_log-", crate::gsp_rm_version!(), ".bin"),
            sig_section: ".fwsignature_gb20x",
        }
    };
}

pub static CHIPS: [Chip; 5] = [
    chip!("GB202", "gb202", 0x2),
    chip!("GB203", "gb203", 0x3),
    chip!("GB205", "gb205", 0x5),
    chip!("GB206", "gb206", 0x6),
    chip!("GB207", "gb207", 0x7),
];

/// The chip a PMC_BOOT_42 value names, or `None` for anything not a GB20x,
/// including the datacenter Blackwells (architecture 0x1A), which boot
/// differently
pub fn by_boot42(boot42: u32) -> Option<&'static Chip> {
    let arch = ((boot42 >> 24) & 0x3f) as u8;
    let implementation = ((boot42 >> 20) & 0xf) as u8;
    if arch != ARCHITECTURE {
        return None;
    }
    CHIPS.iter().find(|c| c.implementation == implementation)
}
