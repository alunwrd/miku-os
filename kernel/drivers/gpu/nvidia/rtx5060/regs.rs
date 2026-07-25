// GB206 (Blackwell consumer) register map - host-visible pieces
//
// Sources, in order of authority:
//     nouveau (linux master, r570 era):
//       nvkm/subdev/fsp/{gh100.c, gb202.c, base.c}
//       nvkm/subdev/gsp/{gh100.c, gb202.c}
//       include/nvhw/ref/gh100/dev_fsp_pri.h
//     open-gpu-kernel-modules 570.144 (fsp_nvdm_format.h, dev_fsp_pri.h)
//
// The Hopper/Blackwell boot model replaces the whole Turing chain
// (FWSEC-FRTS on GSP falcon + booter on SEC2) with the FSP, an always-on
// security processor that boots at devinit. The host only:
//   1) stages GSP-FMC (firmware management chain) + GSP-RM in sysmem,
//   2) sends one NVDM_TYPE_COT message to the FSP over MCTP-over-EMEM,
//   3) waits for secure-boot-complete, then talks CMDQ/MSGQ to GSP-RM exactly like on Turing
//
// Offsets marked VERIFY were cross-read from nouveau function summaries
// rather than the raw header and must be confirmed against a register dump
// on real silicon (phase 0 of the bring-up plan)

///////////////////////////////////////////////////////////////////////////////////
//         PMC / identification (stable across the GSP era)                      //
///////////////////////////////////////////////////////////////////////////////////

pub const PMC_BOOT_0:  u32 = 0x0000_0000;
pub const PMC_BOOT_42: u32 = 0x0000_0A00;

pub const PTIMER_TIME_0: u32 = 0x0000_9400;

//////////////////////////////////////////////////////////////////////////////////////
//     FSP (Firmware Security Processor) - NV_PFSP window 0x8F0000..0x8F4000        //
//////////////////////////////////////////////////////////////////////////////////////

/// FSP falcon register window base (nouveau: nvkm_fsp_new_ -> falcon @ 0x8f2000)
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

/// FSP writes 0xFF here when its own secure boot completes (and again after
/// it processed the COT and released the GPU). nouveau:
/// NV_THERM_I2CS_SCRATCH_FSP_BOOT_COMPLETE_STATUS, success value 0xFF.
/// The THERM block moved on Blackwell: gb202/dev_therm.h puts I2CS_SCRATCH
/// at 0xAD00BC (GH100 had it at 0x200BC) - reading the old offset returns
/// 0x00 on a perfectly healthy card
pub const THERM_I2CS_SCRATCH: u32 = 0x00AD_00BC;
pub const FSP_BOOT_COMPLETE_SUCCESS: u32 = 0xFF;

//////////////////////////////////////////////////////////////////////////////////
//      MCTP / NVDM framing (fsp/gh100.c + dev_fsp_pri.h field layouts)         //
//////////////////////////////////////////////////////////////////////////////////

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

////////////////////////////////////////////////////////////////////////
//           GSP falcon / RISC-V (post-COT handshake)                 //
////////////////////////////////////////////////////////////////////////

/// GSP falcon window; unchanged since Turing (nouveau reuses ga102_gsp_flcn
/// for GB202). The Ampere+ "falcon2" extension window sits at +0x1000
pub const PGSP_FALCON_BASE:  u32 = 0x0011_0000;
pub const PGSP_FALCON2_BASE: u32 = 0x0011_1000;

/// Standard falcon registers (offsets within the falcon window)
pub const FALCON_MAILBOX0: u32 = 0x040;
pub const FALCON_MAILBOX1: u32 = 0x044;

/// NV_PFALCON_FALCON_HWCFG2: nouveau's gsp lockdown poll reads this until
/// RISCV_BR_PRIV_LOCKDOWN clears after the FMC hands off to GSP-RM.
/// VERIFY the lockdown bit position on silicon (nouveau: bit 13 on ga102+)
pub const FALCON_HWCFG2: u32 = 0x0F4;
pub const HWCFG2_RISCV_BR_PRIV_LOCKDOWN: u32 = 1 << 13;

/// NV_PFALCON_FALCON_OS (app-version handshake register)
pub const FALCON_OS: u32 = 0x080;

/// Usable FB size in MiB (nouveau ga102_fb_vidmem_size, reused for GH100
/// and Blackwell: rd32(0x1183a4) << 20). Lives in the PGC6 AON scratch
/// space, valid once devinit ran - i.e. always, on an FSP-posted card
pub const FB_USABLE_SIZE_MB: u32 = 0x0011_83A4;

/// GFW (devinit) boot progress: NV_PGC6_AON_SECURE_SCRATCH_GROUP_05(0),
/// PROGRESS in bits [7:0], 0xFF = COMPLETED (tu102/dev_gc6_island*.h,
/// same offsets through Blackwell). GSP-RM asserts on this very register
/// during init - confirmed by a NOCAT record on live GB206
pub const PGC6_AON_SECURE_SCRATCH_GROUP_05_0: u32 = 0x0011_8234;
pub const GFW_BOOT_PROGRESS_COMPLETED: u32 = 0xFF;
/// Read-protection mask for the scratch group; LEVEL0 read must be
/// ENABLE (bit 0 = 1) for CPU reads of GROUP_05 to return real data
/// (OpenRM kgspWaitForGfwBootOk checks this first)
pub const PGC6_AON_SECURE_SCRATCH_GROUP_05_PLM: u32 = 0x0011_8128;

/////////////////////////////////////////////////////////////////////
//    FRTS layout constants (gsp/gh100.c wpr meta init)            //
/////////////////////////////////////////////////////////////////////

/// FRTS carve-out size at the top of VRAM (gh100_gsp_wpr_meta_init)
pub const FRTS_SIZE: u64 = 0x10_0000; // 1 MiB
/// FRTS vidmem offset alignment requirement (nova-core COT patch)
pub const FRTS_ALIGN: u64 = 0x20_0000; // 2 MiB
