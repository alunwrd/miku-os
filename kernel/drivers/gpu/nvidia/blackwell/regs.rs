// GB206 (Blackwell consumer) register map: host-visible pieces
//
// Sources, in order of authority:
//     nouveau (linux master, r570 era):
//       nvkm/subdev/fsp/{gh100.c, gb202.c, base.c}
//       nvkm/subdev/gsp/{gh100.c, gb202.c}
//       include/nvhw/ref/gh100/dev_fsp_pri.h
//     open-gpu-kernel-modules 570.144 (fsp_nvdm_format.h, dev_fsp_pri.h)
//
// The Hopper/Blackwell boot model replaces the Turing chain (FWSEC-FRTS on GSP
// falcon + booter on SEC2) with the FSP, an always-on security processor that
// boots at devinit. The host:
//   1) stages GSP-FMC (firmware management chain) + GSP-RM in sysmem,
//   2) sends one NVDM_TYPE_COT message to the FSP over MCTP-over-EMEM,
//   3) waits for secure-boot-complete, then talks CMDQ/MSGQ to GSP-RM as on
//      Turing
//
// Offsets marked VERIFY were cross-read from nouveau function summaries and
// must be confirmed against a register dump on real silicon

// Registers not specific to GB206: the host block is the same on every GSP-era
// part, and the Falcon offsets are relative to an engine base. A register map
// is complete, as in `nvidia/regs.rs`
#![allow(dead_code)]

pub use crate::drivers::gpu::nvidia::regs::{PMC_BOOT_42, PTIMER_TIME_0};
pub use crate::drivers::gpu::nvidia::falcon::{FALCON_MAILBOX0, FALCON_MAILBOX1, FALCON_OS};

// PMC / identification (stable across the GSP era)



// FSP (Firmware Security Processor): NV_PFSP window 0x8F0000..0x8F4000

/// FSP falcon register window base (nouveau: nvkm_fsp_new_ -> falcon @
/// 0x8f2000)
pub const PFSP_FALCON_BASE: u32 = 0x008F_2000;

/// EMEM PIO port 0 (control / data). Control encodes the byte offset plus
/// auto-increment bits; same scheme as the SEC2 EMEM on Turing.
/// dev_fsp_pri.h: NV_PFSP_EMEMC(i) / NV_PFSP_EMEMD(i), i in 0..8, stride 8
pub const PFSP_EMEMC0: u32 = 0x008F_2AC0;
pub const PFSP_EMEMD0: u32 = 0x008F_2AC4;

/// EMEMC bit fields: [7:2] offset within block, [15:8] block number,
/// bit 24 AINCW (auto-increment on write), bit 25 AINCR (on read)
pub const EMEMC_AINCW: u32 = 1 << 24;
pub const EMEMC_AINCR: u32 = 1 << 25;

/// Host -> FSP command queue 0 (doorbell pair)
/// dev_fsp_pri.h: NV_PFSP_QUEUE_HEAD(i) = 0x8F2C00 + i*8
pub const PFSP_QUEUE_HEAD0: u32 = 0x008F_2C00;
pub const PFSP_QUEUE_TAIL0: u32 = 0x008F_2C04;

/// FSP -> host message queue 0
/// dev_fsp_pri.h: NV_PFSP_MSGQ_HEAD(i) = 0x8F2C80 + i*8
pub const PFSP_MSGQ_HEAD0: u32 = 0x008F_2C80;
pub const PFSP_MSGQ_TAIL0: u32 = 0x008F_2C84;

/// NV_PFSP_FALCON_COMMON_SCRATCH_GROUP_2(0..3) = 0x8F0320 + i*4. The FSP
/// posts boot/COT status breadcrumbs here; OpenRM dumps these four words
/// whenever GSP boot goes sideways (kfspDumpDebugState_GH100)
pub const PFSP_COMMON_SCRATCH_GROUP_2_0: u32 = 0x008F_0320;

/// FSP writes 0xFF here when its own secure boot completes (and again after it
/// processed the COT and released the GPU). nouveau:
/// NV_THERM_I2CS_SCRATCH_FSP_BOOT_COMPLETE_STATUS, success value 0xFF. The
/// THERM block moved on Blackwell: gb202/dev_therm.h puts I2CS_SCRATCH at
/// 0xAD00BC (GH100 at 0x200BC); the old offset reads 0x00 on a healthy card
pub const THERM_I2CS_SCRATCH: u32 = 0x00AD_00BC;
/// The whole register, not its low byte: `_FSP_BOOT_COMPLETE_STATUS` is 31:0
/// (`gb202/dev_therm_addendum.h:28`)
pub const FSP_BOOT_COMPLETE_SUCCESS: u32 = 0xFF;

/// `NV_PFB_PRI_MMU_WPR2_ADDR_HI`, `_VAL` 31:4 (`tu102/dev_fb.h:37`). OpenRM
/// reads this address on every chip up to GB20x (`kgspIsWpr2Up_GH100` calls the
/// TU102 function); non-zero means a write-protected region is up
pub const PFB_PRI_MMU_WPR2_ADDR_HI: u32 = 0x001F_A828;

/// The secure timer, and the offset that turns it into wall-clock time for
/// PTIMER (`gh100/dev_gc6_island.h:27-41`, used for GB20x by
/// `tmrSetCurrentTime_GH100`). Writing `_OFFSET_0` with `_UPDATE` (bit 0) set
/// triggers the update
pub const PGC6_SCI_SEC_TIMER_TIME_0: u32 = 0x0011_8F54;
pub const PGC6_SCI_SEC_TIMER_TIME_1: u32 = 0x0011_8F58;
pub const PGC6_SCI_SYS_TIMER_OFFSET_0: u32 = 0x0011_8DF4;
pub const PGC6_SCI_SYS_TIMER_OFFSET_1: u32 = 0x0011_8DF8;
pub const SYS_TIMER_OFFSET_UPDATE_TRIGGER: u32 = 1;

/// Where the GPU reads when it flushes towards system memory
/// (`gb10b/dev_fbhub.h:27-34`; GB202-207 use the GB10B HAL). `_LO` takes the
/// low 32 bits of the address, 256-byte aligned; `_HI` bits 51:32
pub const PFB_FBHUB0_PCIE_FLUSH_SYSMEM_ADDR_LO: u32 = 0x008A_1D58;
pub const PFB_FBHUB0_PCIE_FLUSH_SYSMEM_ADDR_HI: u32 = 0x008A_1D5C;
pub const FLUSH_SYSMEM_ADDR_HI_MASK: u32 = 0x000F_FFFF;

/// `NV_EP_PCFG_GPU_VSEC_DEBUG_SEC_2` (`gb202/dev_xtl_ep_pcfg_gpu.h:28`), a
/// configuration-space register past the first 256 bytes, read through BAR0's
/// mirror of config space (`NV_EP_PCFGM`, the base `GspSystemInfo` names).
/// `_BAR_FIREWALL_ENGAGE` is bit 24, 0 when disengaged
pub const EP_PCFGM_BASE: u32 = 0x0009_2000;
pub const VSEC_DEBUG_SEC_2: u32 = 0x2B8;
pub const BAR_FIREWALL_ENGAGE: u32 = 1 << 24;

// MCTP / NVDM framing (fsp/gh100.c + dev_fsp_pri.h field layouts)

// MCTP transport header (u32):
//   [3:0] VERSION  [7:4] RSVD  [15:8] DEID  [23:16] SEID
//   [26:24] TAG    [27] TO     [29:28] SEQ  [30] EOM  [31] SOM
pub const MCTP_HEADER_SOM: u32 = 1 << 31;
pub const MCTP_HEADER_EOM: u32 = 1 << 30;

/// Single-packet message: SOM=1 EOM=1 SEID=0 SEQ=0 (nouveau gh100_fsp_send)
pub const MCTP_HEADER_SINGLE: u32 = MCTP_HEADER_SOM | MCTP_HEADER_EOM;

// MCTP message header (u32):
//   [6:0] TYPE  [7] IC  [23:8] VENDOR_ID  [31:24] NVDM_TYPE
pub const MCTP_MSG_TYPE_VENDOR_PCI: u32 = 0x7E;
pub const MCTP_VENDOR_ID_NV: u32 = 0x10DE;

pub const NVDM_TYPE_COT:          u32 = 0x14;
pub const NVDM_TYPE_FSP_RESPONSE: u32 = 0x15;

/// Assemble the MCTP message header for a given NVDM type
pub const fn nvdm_header(nvdm_type: u32) -> u32 {
    MCTP_MSG_TYPE_VENDOR_PCI | (MCTP_VENDOR_ID_NV << 8) | (nvdm_type << 24)
}

// GSP falcon / RISC-V (post-COT handshake)

/// GSP falcon window; unchanged since Turing (nouveau reuses ga102_gsp_flcn for
/// GB202). The Ampere+ "falcon2" extension window sits at +0x1000
pub const PGSP_FALCON_BASE:  u32 = 0x0011_0000;
pub const PGSP_FALCON2_BASE: u32 = 0x0011_1000;

/// Standard falcon registers (offsets within the falcon window)

/// NV_PFALCON_FALCON_HWCFG2: nouveau's gsp lockdown poll reads this until
/// RISCV_BR_PRIV_LOCKDOWN clears after the FMC hands off to GSP-RM. VERIFY the
/// lockdown bit position on silicon (nouveau: bit 13 on ga102+)
pub const FALCON_HWCFG2: u32 = 0x0F4;
pub const HWCFG2_RISCV_BR_PRIV_LOCKDOWN: u32 = 1 << 13;

/// NV_PFALCON_FALCON_OS (app-version handshake register)

/// NV_PRISCV_RISCV_CPUCTL, in the falcon2 window (gh100/dev_riscv_pri.h:
/// NV_FALCON2_GSP_BASE + 0x388). OpenRM's kflcnIsRiscvActive_GA102 reads
/// ACTIVE_STAT here after lockdown release, and gh100_gsp_fini polls HALTED on
/// the way down
pub const PRISCV_RISCV_CPUCTL: u32 = PGSP_FALCON2_BASE + 0x388;
pub const RISCV_CPUCTL_HALTED: u32 = 1 << 4;
pub const RISCV_CPUCTL_ACTIVE_STAT: u32 = 1 << 7;

/// NV_PRISCV_RISCV_IRQMASK, also in the falcon2 window: which of the falcon's
/// interrupt sources the RISC-V core has enabled. `r535_gsp_intr` reads IRQSTAT
/// and this together: a bit pending but not enabled is not this handler's
/// business. The offset is `ga102_gsp_flcn.riscv_irqmask` (0x528) inside
/// `addr2` (0x1000), and `gb202_gsp` uses the same falcon description
pub const PRISCV_RISCV_IRQMASK: u32 = PGSP_FALCON2_BASE + 0x528;

/// PRI decoupler sentinel. While the GSP TARGET_MASK is applied by the
/// bootrom/ACR, CPU reads of the whole NV_PGSP block return 0 or 0xbadf41xx,
/// never a real register value (OpenRM _kfspIsGspTargetMaskReleased)
pub const PRI_TARGET_LOCKED: u32 = 0xBADF_4100;
pub const PRI_TARGET_LOCKED_MASK: u32 = 0xFFFF_FF00;

/// NV_PBUS_SW_SCRATCH(0x37) = 0x1400 + 0x37*4. The GSP-FMC writes its fatal
/// error code here; OpenRM zeroes it before every boot attempt
/// (gpuResetGspFmcErrorCode_GH100) and decodes it on any boot failure
/// (gpuReportGspFmcErrorCode_GH100). The code never reaches the MSGQ
pub const PBUS_SW_SCRATCH_GSP_FMC_ERROR: u32 = 0x0000_14DC;

/// FSP debug scratch. GROUP_2(0..3) is what kfspDumpDebugState_GH100 prints
/// on a boot failure; GROUP_3(1) holds the FSP microcode version
/// (gb202/dev_fsp_addendum.h: NV_GFW_FSP_UCODE_VERSION, MAJOR 11:8,
/// MINOR 7:0)
pub const PFSP_COMMON_SCRATCH_GROUP_3_0: u32 = 0x008F_0330;
pub const GFW_FSP_UCODE_VERSION: u32 = 0x008F_0334;

/// Usable FB size in MiB (nouveau ga102_fb_vidmem_size, reused for GH100 and
/// Blackwell: rd32(0x1183a4) << 20). In the PGC6 AON scratch space, valid once
/// devinit ran
pub const FB_USABLE_SIZE_MB: u32 = 0x0011_83A4;

/// GFW (devinit) boot progress: NV_PGC6_AON_SECURE_SCRATCH_GROUP_05(0),
/// PROGRESS in bits [7:0], 0xFF = COMPLETED (tu102/dev_gc6_island*.h, same
/// offsets through Blackwell). GSP-RM asserts on this register during init (a
/// NOCAT record on live GB206)
pub const PGC6_AON_SECURE_SCRATCH_GROUP_05_0: u32 = 0x0011_8234;
pub const GFW_BOOT_PROGRESS_COMPLETED: u32 = 0xFF;
/// Read-protection mask for the scratch group; LEVEL0 read must be
/// ENABLE (bit 0 = 1) for CPU reads of GROUP_05 to return real data
/// (OpenRM kgspWaitForGfwBootOk checks this first)
pub const PGC6_AON_SECURE_SCRATCH_GROUP_05_PLM: u32 = 0x0011_8128;

/// The GC6 island PRI window, NV_PGC6 = 0x118000..0x118fff (tu102
/// dev_gc6_island.h; unchanged through Blackwell: GROUP_42 at 0x1183a4 still
/// returns the usable FB size on GB206). NVIDIA publishes no Blackwell
/// dev_gc6_island.h, so a dump of it is how to look for the GFW progress byte
pub const PGC6_BASE: u32 = 0x0011_8000;
pub const PGC6_SIZE: u32 = 0x1000;

/// Decoded NV_PBUS_SW_SCRATCH_GSP_FMC_ERROR
/// (gsp_fmc_error_code_formatting.h: VARIANT 31:28, PARTITION 27:24,
/// PAYLOAD 23:0; SK payload = ERROR 15:8 / PHASE 7:0, generic payload =
/// ADDITIONAL_INFO 23:16 / ERROR_CODE 15:0)
#[derive(Debug, Clone, Copy)]
pub enum FmcErrorCode {
    /// Separation-kernel error: (error, phase)
    Sk { partition: u32, error: u32, phase: u32 },
    /// Generic partition error: (code, extra info)
    Generic { partition: u32, code: u32, info: u32 },
    /// Variant this driver does not know how to decode
    Unknown(u32),
}

/// gpuReportGspFmcErrorCode_GH100. Returns None when the register still
/// reads GSP_FMC_OK (0)
pub fn decode_fmc_error(raw: u32) -> Option<FmcErrorCode> {
    if raw == 0 {
        return None;
    }
    let variant = raw >> 28;
    // Partition ID is stored biased by 1 (GSP_FMC_PARTITION_ID_OFFSET)
    let partition = ((raw >> 24) & 0xF).wrapping_sub(1) & 0xF;
    Some(match variant {
        0 => FmcErrorCode::Sk {
            partition,
            error: (raw >> 8) & 0xFF,
            phase: raw & 0xFF,
        },
        1 => FmcErrorCode::Generic {
            partition,
            code: raw & 0xFFFF,
            info: (raw >> 16) & 0xFF,
        },
        _ => FmcErrorCode::Unknown(raw),
    })
}

// FRTS layout constants (gsp/gh100.c wpr meta init)

/// FRTS carve-out size at the top of VRAM (gh100_gsp_wpr_meta_init)
pub const FRTS_SIZE: u64 = 0x10_0000; // 1 MiB
/// FRTS vidmem offset alignment requirement (nova-core COT patch)
pub const FRTS_ALIGN: u64 = 0x20_0000; // 2 MiB
