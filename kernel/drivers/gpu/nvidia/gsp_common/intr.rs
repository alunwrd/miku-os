// The interrupt vector table, as GSP-RM hands it out
//
// Every interrupt this GPU raises arrives at a tree of 32-bit leaves in the VFN
// register block (`nvidia::intr`), read by the CPU when the card's MSI fires.
// Which bit in which leaf belongs to which engine is not fixed in silicon or
// documented in any register: GSP-RM assigns it at boot and answers one control
// call with the whole assignment.
//
// That call is `NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE`, addressed to
// GSP-RM's internal subdevice, as the NV2080_CTRL_CMD_INTERNAL_* family and
// `disp::write_inst_mem` are.
//
// A "vector" here is a flat number: `leaf * 32 + bit`. Every engine gets two, a
// stalling one and a non-stalling one.
//
// The reply also says which top-level subtrees each category of interrupt was
// given (`subtreeMap`), telling the tree which subtrees are non-stalling and
// not armed with the rest (`_intrEnableStall_TU102`).
//
// Reference: `NV2080_CTRL_INTERNAL_INTR_GET_KERNEL_TABLE_PARAMS` in OpenRM's
// `ctrl2080internal.h:1441-1480`, the engine indices in
// `kernel/gpu/intr/engine_idx.h`

use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::le;

use super::rmapi::{RmClient, RmError};
use super::rpc::GspRpc;
use crate::drivers::gpu::nvidia::intr::Geometry;

/// `NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE`
pub const CTRL_INTERNAL_INTR_GET_KERNEL_TABLE: u32 = 0x2080_0a5c;

/// `NV2080_CTRL_INTERNAL_INTR_GET_KERNEL_TABLE_PARAMS`, 2112 bytes in 615, and
/// the offsets inside it, compiled from the header: `tableLen` at 0, a 128
/// entry array at 4 with a stride of 16, and the per-category subtree map at
/// 2056, 8-aligned. The entry is
/// `{ NvU16 engineIdx; NvU32 pmcIntrMask, vectorStall, vectorNonStall; }`, so
/// the u16 is followed by two bytes of padding
pub const PARAMS_SIZE: usize = 2112;
const OFF_TABLE: usize = 4;
const ENTRY_STRIDE: usize = 16;
const MAX_TABLE_SIZE: usize = 128;
const ENTRY_OFF_PMC: usize = 4;
const ENTRY_OFF_STALL: usize = 8;
const ENTRY_OFF_NONSTALL: usize = 12;
/// `subtreeMap[NV2080_INTR_CATEGORY_ENUM_COUNT]`: a `{ NvU64 subtreeMask; }`
/// per category after the 128 entries, 8-aligned
const OFF_SUBTREE_MAP: usize = (OFF_TABLE + MAX_TABLE_SIZE * ENTRY_STRIDE + 7) & !7;
const SUBTREE_MAP_STRIDE: usize = 8;
/// `NV2080_INTR_CATEGORY_ENUM_COUNT`
pub const CATEGORIES: usize = 7;
/// `NV2080_INTR_CATEGORY_ESCHED_DRIVEN_ENGINE_NOTIFICATION`: the non-stalling
/// interrupts of the engines, the category `intrGetIntrTopNonStallMask_GH100`
/// builds its mask from
pub const CATEGORY_ESCHED_DRIVEN_ENGINE_NOTIFICATION: usize = 2;

/// Engine indices the driver routes. The full list is OpenRM's
/// `kernel/gpu/intr/engine_idx.h`, and `engine_name` below names all of it.
/// GB20x splits the display interrupt in two: `DISP` is the stalling one and
/// `DISP_LOW` the low-priority one (`intr_gb202.c` routes both). 615 put GPIO
/// at 176 and DISP_LOW at 177
pub const MC_ENGINE_IDX_DISP: u16 = 2;
pub const MC_ENGINE_IDX_GSP: u16 = 50;
pub const MC_ENGINE_IDX_DISP_LOW: u16 = 177;

#[derive(Copy, Clone, Debug)]
pub struct IntrEntry {
    pub engine_idx: u16,
    pub pmc_intr_mask: u32,
    pub vector_stall: u32,
    pub vector_nonstall: u32,
}

/// The whole reply: every engine's vectors, and the subtrees each category
/// of interrupt was given
#[derive(Clone, Debug)]
pub struct KernelTable {
    pub entries: Vec<IntrEntry>,
    /// The subtrees of each category, one bit per top-level subtree; 0 where
    /// the reply stopped short
    pub subtrees: [u64; CATEGORIES],
}

impl KernelTable {
    /// Decode a reply. Pure, so the selftest can hand it one it made up
    pub fn parse(reply: &[u8]) -> Result<Self, RmError> {
        if reply.len() < OFF_TABLE {
            return Err(RmError::ShortReply);
        }
        // A length GSP-RM cannot have meant is clamped: the array is 128
        // entries, and a bogus count would index past the end of the caller's
        // Vec
        let len = (le::u32_at(reply, 0) as usize).min(MAX_TABLE_SIZE);
        let mut entries = Vec::with_capacity(len);
        for i in 0..len {
            let e = OFF_TABLE + i * ENTRY_STRIDE;
            if e + ENTRY_STRIDE > reply.len() {
                break;
            }
            entries.push(IntrEntry {
                engine_idx: le::u16_at(reply, e),
                pmc_intr_mask: le::u32_at(reply, e + ENTRY_OFF_PMC),
                vector_stall: le::u32_at(reply, e + ENTRY_OFF_STALL),
                vector_nonstall: le::u32_at(reply, e + ENTRY_OFF_NONSTALL),
            });
        }
        let mut subtrees = [0u64; CATEGORIES];
        for (c, slot) in subtrees.iter_mut().enumerate() {
            let at = OFF_SUBTREE_MAP + c * SUBTREE_MAP_STRIDE;
            if at + SUBTREE_MAP_STRIDE > reply.len() {
                break;
            }
            *slot = le::u64_at(reply, at);
        }
        Ok(Self { entries, subtrees })
    }

    /// One category's subtrees as a top-level mask
    /// (`intrGetIntrTopCategoryMask_IMPL`, 615): GSP-RM's mask, and nothing for
    /// one empty or all ones in any width, an uninitialised map OpenRM asserts
    /// on. The top-level registers are 32 bits wide
    pub fn category_mask(&self, category: usize) -> u32 {
        match self.subtrees.get(category).copied() {
            Some(0) | Some(0xffff) | Some(0xffff_ffff) | Some(u64::MAX) | None => 0,
            Some(m) => m as u32,
        }
    }

    /// The subtrees the non-stalling interrupts live in, which the tree is
    /// not to arm with the rest (`intrGetIntrTopNonStallMask_GH100`)
    pub fn nonstall_subtrees(&self) -> u32 {
        self.category_mask(CATEGORY_ESCHED_DRIVEN_ENGINE_NOTIFICATION)
    }

    /// The non-stalling vector for one engine, where its fence notifications
    /// arrive (`intrGetVectorFromEngineId(.., bNonStall)`, `intr.c:605`)
    pub fn nonstall_vector(&self, engine_idx: u16, geometry: Geometry) -> Option<u32> {
        self.entries
            .iter()
            .find(|e| e.engine_idx == engine_idx && geometry.split(e.vector_nonstall).is_some())
            .map(|e| e.vector_nonstall)
    }

    /// The stalling vector for one engine, if the table lists it and a tree
    /// of this shape can deliver it
    pub fn stall_vector(&self, engine_idx: u16, geometry: Geometry) -> Option<u32> {
        self.entries
            .iter()
            .find(|e| e.engine_idx == engine_idx && geometry.split(e.vector_stall).is_some())
            .map(|e| e.vector_stall)
    }
}

/// The name `engine_idx.h` gives an engine index. Numbered families (CE0-19,
/// GR0-7, NVDEC0-7, ...) come back as the family name, with the index printed
/// next to it. Index 0 is `NULL`, not an engine: OpenRM reserves it ("This must
/// be 0")
pub fn engine_name(idx: u16) -> &'static str {
    match idx {
        0 => "NULL",
        1 => "TMR",
        MC_ENGINE_IDX_DISP => "DISP",
        3 => "FB",
        4 => "FIFO",
        5 => "VIDEO",
        6 => "MD",
        7 => "BUS",
        8 => "PMGR",
        9 => "VP2",
        10 => "CIPHER",
        11 => "BIF",
        12 => "PPP",
        13 => "PRIVRING",
        14 => "PMU",
        15..=34 => "CE",
        35 => "VIC",
        36 => "ISOHUB",
        37 => "VGPU",
        38..=41 => "NVENC",
        42 => "C2C",
        43 => "LTC",
        44 => "FBHUB",
        45 => "HDACODEC",
        46 => "GMMU",
        47 => "SEC2",
        48 => "FSP",
        49 => "NVLINK",
        MC_ENGINE_IDX_GSP => "GSP",
        51..=58 => "NVJPG",
        59 => "REPLAYABLE_FAULT",
        60 => "ACCESS_CNTR",
        61 => "NON_REPLAYABLE_FAULT",
        62 => "REPLAYABLE_FAULT_ERROR",
        63 => "NON_REPLAYABLE_FAULT_ERROR",
        64 => "INFO_FAULT",
        65..=72 => "NVDEC",
        73 => "CPU_DOORBELL",
        74 => "PRIV_DOORBELL",
        75 => "MMU_ECC_ERROR",
        76 => "BLG",
        77 => "PERFMON",
        78 => "BUF_RESET",
        79 => "XBAR",
        80 => "ZPW",
        81..=82 => "OFA",
        83 => "TEGRA",
        84..=91 => "GR",
        92..=155 => "ESCHED",
        156..=163 => "GR_FECS_LOG",
        164 => "TMR_SWRL",
        165 => "DISP_GSP",
        166 => "REPLAYABLE_FAULT_CPU",
        167 => "NON_REPLAYABLE_FAULT_CPU",
        168 => "PXUC",
        169 => "SYSLTC",
        170 => "LRCC",
        171..=174 => "GSPLITE",
        175 => "DPAUX",
        176 => "GPIO",
        MC_ENGINE_IDX_DISP_LOW => "DISP_LOW",
        178 => "HFRP",
        179 => "I2C",
        180 => "LPSEC",
        _ => "",
    }
}

/// Ask GSP-RM for the whole table.
///
/// `internal` is GSP-RM's own client/device/subdevice triple from
/// `staticinfo::StaticInfo`; see the note at the top
pub fn kernel_table(rpc: &mut GspRpc, internal: &RmClient) -> Result<KernelTable, RmError> {
    let reply = internal.control_query(
        rpc,
        internal.h_subdevice,
        CTRL_INTERNAL_INTR_GET_KERNEL_TABLE,
        PARAMS_SIZE,
    )?;
    KernelTable::parse(&reply)
}
