// Chip-independent GSP-RM plumbing (post-boot RPC world)
//
// Once GSP-RM runs, whether through the Turing booter/SEC2 chain or the
// Blackwell FSP/COT chain, the host talks to it through the same CMDQ/MSGQ
// shared region and RPC framing. The per-chip drivers differ in how they boot
// the RISC-V core.
//
// One GSP-RM release is spoken, as in OpenRM: firmware and driver from the same
// build (`dev/gsp-615.md`)

/// The GSP-RM release this driver speaks, as a literal for `concat!`
#[macro_export]
macro_rules! gsp_rm_version {
    () => {
        "615.78.08"
    };
}

/// The same, as a constant
pub const GSP_RM_VERSION: &str = gsp_rm_version!();

pub mod bootargs;
pub mod gmcapi;
pub mod nvkv;
pub mod gspfw;
pub mod intr;
pub mod kgsp;
pub mod compute;
pub mod qmd;
pub mod chan;
pub mod logdecode;
pub mod disp;
pub mod dispcaps;
pub mod modeset;
pub mod output;
pub mod perf;
pub mod pushbuf;
pub mod refstate;
pub mod rm_names;
pub mod rmapi;
pub mod rpc;
pub mod staticinfo;
pub mod vram;
pub mod sysinfo;
