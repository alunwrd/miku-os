// The compute engine on a channel
//
// The copy engine in `chan.rs` needs only a channel. A compute engine needs a
// context: a dozen buffers RM knows the sizes of, holding the register file,
// the patch list, the attribute buffers and the privilege maps, allocated,
// mapped into the channel's address space, and promoted: handed to RM one by
// one so it can initialise the ones needing it.
//
// The reference is OpenRM: `kgrobjConstruct`, `kgrobjGetPromoteIds_FWCLIENT`
// and the two `kgrctxPrepare*CtxBuffer` functions in
// `refsrc/openrm/src/nvidia/src/kernel/gpu/gr/`. Two changes matching it
// exactly (the shorter non-3D buffer list, and promote entries with no virtual
// address) break the card: the compute channel takes a
// `ROBUST_CHANNEL_PBDMA_ERROR` on the desktop's first frame. Both are marked
// where they sit. Something in how this driver presents itself to RM differs
// from what those functions assume, so the reference is a guide here. Sizes are
// asked of RM:
//
//   1. `NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO` on the
//      internal subdevice: how large each context buffer is and its alignment
//   2. allocate each one and map it into the channel's address space, except
//      the privilege map, which RM wants physically and unmapped
//   3. `NV2080_CTRL_CMD_GPU_PROMOTE_CTX` on the subdevice: one entry for each
//      buffer needing initialising. An entry carries a physical address, a
//      size, `physAttr`, `bInitialize`, `bNonmapped` and `gpuVirtAddr`, the
//      address in our page tables. The reference does not send that address for
//      an externally-owned space, but this card needs it; see `promote_params`
//   4. allocate the engine class on the channel: `BLACKWELL_COMPUTE_B`, with no
//      parameters
//
// Every step succeeds or names its own error.

extern crate alloc;
use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use crate::drivers::gpu::nvidia::le;
use alloc::vec::Vec;

use super::rmapi::{RmClient, RmError};
use super::rpc::GspRpc;

/// `BLACKWELL_COMPUTE_B`, `openrm/.../class/clcec0.h`.
///
/// The Blackwell compute headers carry only the class number; the methods are
/// in `clc7c0.h`, and compatible
pub const CLASS_COMPUTE: u32 = 0x0000_cec0;

/// `NV2080_ENGINE_TYPE_GRAPHICS`. The compute engine lives on the graphics
/// runlist and its context is a graphics context, unlike the copy engine's
/// avoidance of runlist 0
pub const ENGINE_TYPE_GRAPHICS: u32 = 0x0000_0001;

pub const NV2080_CTRL_CMD_GPU_PROMOTE_CTX: u32 = 0x2080_012b;
pub const NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO: u32 = 0x2080_0a32;
pub const NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_INFO: u32 = 0x2080_0a2a;

/// `NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_*`, `ctrl2080gpu.h`
pub mod buf {
    pub const MAIN: u16 = 0;
    #[allow(dead_code)] // engine context-buffer id; the compute channel allocates a subset
    pub const PM: u16 = 1;
    pub const PATCH: u16 = 2;
    pub const BUFFER_BUNDLE_CB: u16 = 3;
    pub const PAGEPOOL: u16 = 4;
    pub const ATTRIBUTE_CB: u16 = 5;
    pub const RTV_CB_GLOBAL: u16 = 6;
    #[allow(dead_code)] // engine context-buffer id; the compute channel allocates a subset
    pub const GFXP_POOL: u16 = 7;
    #[allow(dead_code)] // engine context-buffer id; the compute channel allocates a subset
    pub const GFXP_CTRL_BLK: u16 = 8;
    pub const FECS_EVENT: u16 = 9;
    pub const PRIV_ACCESS_MAP: u16 = 10;
    pub const UNRESTRICTED_PRIV_ACCESS_MAP: u16 = 11;
    #[allow(dead_code)] // engine context-buffer id; the compute channel allocates a subset
    pub const GLOBAL_PRIV_ACCESS_MAP: u16 = 12;
}

/// `NV2080_CTRL_INTERNAL_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_COUNT`
const ENGINE_ID_COUNT: usize = 0x1a;
/// `NV2080_CTRL_INTERNAL_GR_MAX_ENGINES`: the reply carries one full engine
/// array per graphics engine, and a chip with one still sends all eight
const GR_MAX_ENGINES: usize = 8;
/// The whole parameter block. RM checks the length against the structure the
/// command id names and refuses a mismatch
const BUFFERS_INFO_BYTES: usize = GR_MAX_ENGINES * ENGINE_ID_COUNT * 8;
/// `NV2080_CTRL_GPU_PROMOTE_CONTEXT_MAX_ENTRIES`
const MAX_ENTRIES: usize = 16;
/// One `NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ENTRY`: three aligned 64-bit fields,
/// then physAttr, bufferId, bInitialize, bNonmapped: 32 bytes with padding
const ENTRY_SIZE: usize = 32;
/// The fixed head of `NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS` before
/// `promoteEntry`:
/// engineType, hClient, ChID, hChanClient, hObject, hVirtMemory, then two
/// 8-aligned 64-bit fields and entryCount, padded to 8 for the array
const PARAMS_HEAD: usize = 0x30;

/// What one context buffer needs. Sizes come from RM; which are initialised,
/// mapped or read-only comes from OpenRM (see `CTX_BUFFERS`)
#[derive(Clone, Copy)]
pub struct CtxBufSpec {
    pub id: u16,
    /// Index into the engine array of the buffers-info reply
    pub engine_index: usize,
    /// RM fills this one at promotion time, so it must be handed over
    /// physically as well as virtually
    pub init: bool,
    /// RM wants it physical and not mapped: the privilege map, alone
    pub nonmapped: bool,
    /// Does this buffer belong to the graphics engine, not a channel?
    ///
    /// `GR_GLOBALCTX_BUFFER_*` in
    /// `refsrc/openrm/src/nvidia/src/kernel/gpu/gr/kernel_graphics_context.c:155`.
    /// It decides whether RM's "already initialised" flag survives the channel
    /// that first promoted it: for a global buffer the flag lives on the engine
    /// (`pCtxBuffers->bInitialized[]`) and does; for `MAIN` and `PATCH` it
    /// lives on the `KernelGraphicsContext`, new for a new channel, and does
    /// not
    pub global: bool,
    /// Extra bytes beyond what RM reports.
    ///
    /// Only the main context: `kgraphicsGetMainCtxBufferSize`
    /// (`refsrc/openrm/src/nvidia/src/kernel/gpu/gr/kernel_graphics.c:1665`)
    /// takes `engine[GRAPHICS].size`, aligns it up to a page, and adds
    /// `RM_PAGE_SIZE * maxSubctx`, room for a per-VEID header in the golden
    /// context. `0x40000` is 4096 x 64, and 64 is the subcontext slots
    /// `write_inst_block` puts in the instance block. RM reports the real
    /// maximum in `NV2080_CTRL_GR_INFO_INDEX_MAX_SUBCONTEXT_COUNT`
    pub extra: u32,
}

/// `NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_*`,
/// `ctrl0080fifo.h`.
///
/// These index the reply, a different numbering from the buffer ids going back
/// to RM in the promotion. The pairing is a table
pub mod eng {
    pub const GRAPHICS: usize = 0x00;
    pub const PAGEPOOL: usize = 0x0d;
    pub const PATCH: usize = 0x10;
    pub const BUNDLE_CB: usize = 0x11;
    pub const ATTRIBUTE_CB: usize = 0x13;
    pub const RTV_CB_GLOBAL: usize = 0x14;
    pub const FECS_EVENT: usize = 0x17;
    pub const PRIV_ACCESS_MAP: usize = 0x18;
}

/// The buffers the context is built from.
///
/// Nine. OpenRM's `kgrobjGetPromoteIds_FWCLIENT`
/// (`refsrc/openrm/src/nvidia/src/kernel/gpu/gr/kernel_graphics_object.c:571`)
/// offers a shorter, non-3D list for a compute object
/// (`MAIN, PM, PATCH, FECS_EVENT, PRIV_ACCESS_MAP, UNRESTRICTED_PRIV_ACCESS_MAP`),
/// but cutting the four 3D buffers breaks the effects pass on the card with a
/// `ROBUST_CHANNEL_PBDMA_ERROR` on its first frame, so something else here
/// depends on them.
///
/// `ATTRIBUTE_CB` alone is 51552 KiB
pub const CTX_BUFFERS: [CtxBufSpec; 9] = [
    CtxBufSpec { id: buf::MAIN,             engine_index: eng::GRAPHICS,        init: true,  nonmapped: false, global: false, extra: 0x40000 },
    CtxBufSpec { id: buf::PATCH,            engine_index: eng::PATCH,           init: true,  nonmapped: false, global: false, extra: 0 },
    CtxBufSpec { id: buf::BUFFER_BUNDLE_CB, engine_index: eng::BUNDLE_CB,       init: false, nonmapped: false, global: true,  extra: 0 },
    CtxBufSpec { id: buf::PAGEPOOL,         engine_index: eng::PAGEPOOL,        init: false, nonmapped: false, global: true,  extra: 0 },
    CtxBufSpec { id: buf::ATTRIBUTE_CB,     engine_index: eng::ATTRIBUTE_CB,    init: false, nonmapped: false, global: true,  extra: 0 },
    CtxBufSpec { id: buf::RTV_CB_GLOBAL,    engine_index: eng::RTV_CB_GLOBAL,   init: false, nonmapped: false, global: true,  extra: 0 },
    CtxBufSpec { id: buf::FECS_EVENT,       engine_index: eng::FECS_EVENT,      init: true,  nonmapped: false, global: true,  extra: 0 },
    CtxBufSpec { id: buf::PRIV_ACCESS_MAP,  engine_index: eng::PRIV_ACCESS_MAP, init: true,  nonmapped: true,  global: true,  extra: 0 },
    CtxBufSpec { id: buf::UNRESTRICTED_PRIV_ACCESS_MAP, engine_index: eng::PRIV_ACCESS_MAP, init: true, nonmapped: false, global: true, extra: 0 },
];

/// Size and alignment of one context buffer, as RM reports them
#[derive(Clone, Copy, Default, Debug)]
pub struct CtxBufInfo {
    pub size: u32,
    pub alignment: u32,
}

/// Ask RM how large the graphics context buffers are on this chip.
///
/// The reply is one `{size, alignment}` pair per engine index, one array per
/// graphics engine. Only the first array is read, as in
/// `r570_gr_get_ctxbufs_and_zcull_info`, but all eight are asked for, since the
/// block's size identifies the call.
///
/// Asked of the internal subdevice, as `pick_copy_engine` does: RM's own static
/// properties
pub fn context_buffers_info(
    rpc: &mut GspRpc,
    internal: &RmClient,
) -> Result<[CtxBufInfo; ENGINE_ID_COUNT], RmError> {
    let reply = internal.control_query(
        rpc,
        internal.h_subdevice,
        NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO,
        BUFFERS_INFO_BYTES,
    )?;

    let mut out = [CtxBufInfo::default(); ENGINE_ID_COUNT];
    for (i, slot) in out.iter_mut().enumerate() {
        let at = i * 8;
        if at + 8 > reply.len() {
            break;
        }
        slot.size = le::u32_at(&reply, at);
        slot.alignment = u32::from_le_bytes(reply[at + 4..at + 8].try_into().unwrap());
    }
    Ok(out)
}

/// `NV2080_CTRL_INTERNAL_STATIC_GR_GET_INFO_PARAMS`: eight engines, each with
/// `NV0080_CTRL_GR_INFO_MAX_SIZE` `{index, data}` pairs, 63 in 615 (58 in 570).
/// A block of the old size is refused with NV_ERR_INVALID_ARGUMENT
const GR_INFO_ENTRIES: usize = 0x3f;
const GR_INFO_BYTES: usize = GR_MAX_ENGINES * GR_INFO_ENTRIES * 8;
// gcc's sizeof on the 615 header
const _: () = assert!(GR_INFO_BYTES == 4032);
/// `NV0080_CTRL_GR_INFO_INDEX_SM_VERSION`
const GR_INFO_SM_VERSION: usize = 0x0c;

/// Ask RM which shader instruction set this chip speaks.
///
/// The launch descriptor carries this number and the machine checks it, in the
/// reply holding the graphics constants.
///
/// The version is in RM's encoding, which the descriptor wants folded: the
/// major nibbles down by four bits, the minor left alone. `sass_version` in
/// tinygrad, the same expression
pub fn sass_version(rpc: &mut GspRpc, internal: &RmClient) -> Result<(u32, u32), RmError> {
    let reply = internal.control_query(
        rpc,
        internal.h_subdevice,
        NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_INFO,
        GR_INFO_BYTES,
    )?;
    let at = GR_INFO_SM_VERSION * 8 + 4;
    if at + 4 > reply.len() {
        return Err(RmError::ShortReply);
    }
    let sm = le::u32_at(&reply, at);
    Ok((sm, ((sm & 0xf00) >> 4) | (sm & 0xf)))
}

// The context, the UVM way
//
// Everything above follows `kgrobjConstruct`, the non-UVM path: the driver
// allocates the graphics context buffers and hands RM physical addresses,
// sizes, `physAttr` and `bInitialize`. It works on a first promotion and not on
// a second.
//
// This driver's address space is externally owned, the UVM model, and the UVM
// path is open: `nvGpuOpsBindChannelResources`
// (`refsrc/openrm/src/nvidia/src/kernel/rmapi/nv_gpu_ops.c:10313`). There RM
// owns the context buffers, and a promotion carries two fields per entry:
//
//     pParams->promoteEntry[i].bufferId    = channelResourceBindParams[i].resourceId;
//     pParams->promoteEntry[i].gpuVirtAddr = channelResourceBindParams[i].resourceVa;
//
// The block is zeroed, so `bInitialize`, `bNonmapped`, the physical address and
// the size are zero. Afterwards RM sets `bIsContextBound`, which
// `kchannelIsSchedulable` tests before an externally owned channel may run.
//
// So a promotion here binds addresses to buffers RM owns. Three controls do the
// job, and RM reports the rest, including which buffers are the engine's.

/// `NV2080_CTRL_CMD_GR_GET_CTX_BUFFER_INFO`, `ctrl2080gr.h:1103`
const NV2080_CTRL_CMD_GR_GET_CTX_BUFFER_INFO: u32 = 0x2080_1219;
/// `NV2080_CTRL_CMD_KGR_GET_CTX_BUFFER_PTES`, `ctrl2080internal.h:362`
const NV2080_CTRL_CMD_KGR_GET_CTX_BUFFER_PTES: u32 = 0x2080_0a28;

/// `NV2080_CTRL_GR_MAX_CTX_BUFFER_COUNT`
const MAX_CTX_BUFFERS: usize = 64;
/// `NV2080_CTRL_KGR_MAX_BUFFER_PTES`
const MAX_BUFFER_PTES: usize = 128;

/// Offsets in `NV2080_CTRL_GR_CTX_BUFFER_INFO`, confirmed by compiling the C:
/// the entry is 80 bytes and the array starts at 16
const CTX_INFO_ENTRY: usize = 80;
const CTX_INFO_ARRAY: usize = 16;
/// `NV2080_CTRL_GR_GET_CTX_BUFFER_INFO_PARAMS` is 5136 bytes
const CTX_INFO_PARAMS: usize = CTX_INFO_ARRAY + MAX_CTX_BUFFERS * CTX_INFO_ENTRY;
/// `NV2080_CTRL_KGR_GET_CTX_BUFFER_PTES_PARAMS` is 1056, addresses at 24
const PTES_PARAMS: usize = 1056;
const PTES_ARRAY: usize = 24;

/// `ADDR_SYSMEM`, `g_mem_desc_nvoc.h:113`
const ADDR_SYSMEM: u32 = 1;
const ADDR_FBMEM: u32 = 2;

/// One graphics context buffer, as RM describes its own
#[derive(Clone, Copy)]
pub struct CtxBuf {
    /// `bufferType`, the same number the promotion calls `bufferId`
    pub id: u32,
    pub size: u64,
    pub alignment: u64,
    pub page_size: u32,
    pub pages: u64,
    pub phys: u64,
    pub aperture: u32,
    pub contig: bool,
    /// The graphics engine's rather than this channel's, as RM reports
    pub global: bool,
}

/// Ask RM for the graphics context buffers of a channel that already has a
/// graphics object on it.
///
/// `{hUserClient, hChannel}` in, a count and a description of each buffer out.
/// On this client's own subdevice, as `nvGpuOpsRetainChannelResources` uses the
/// session's
pub fn ctx_buffer_info(
    rpc: &mut GspRpc,
    rm: &RmClient,
    channel_handle: u32,
) -> Result<Vec<CtxBuf>, RmError> {
    let mut p = alloc::vec![0u8; CTX_INFO_PARAMS];
    le::put_u32(&mut p, 0x00, rm.h_client);
    le::put_u32(&mut p, 0x04, channel_handle);
    let reply = rm.control(rpc, rm.h_subdevice, NV2080_CTRL_CMD_GR_GET_CTX_BUFFER_INFO, &p)?;
    if reply.len() < CTX_INFO_ARRAY {
        return Err(RmError::ShortReply);
    }
    let count = (le::u32_at(&reply, 0x08) as usize).min(MAX_CTX_BUFFERS);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = CTX_INFO_ARRAY + i * CTX_INFO_ENTRY;
        if at + CTX_INFO_ENTRY > reply.len() {
            return Err(RmError::ShortReply);
        }
        out.push(CtxBuf {
            alignment: le::u64_at(&reply, at),
            size: le::u64_at(&reply, at + 8),
            pages: le::u64_at(&reply, at + 24),
            phys: le::u64_at(&reply, at + 32),
            id: le::u32_at(&reply, at + 40),
            aperture: le::u32_at(&reply, at + 44),
            page_size: le::u32_at(&reply, at + 52),
            contig: reply[at + 56] != 0,
            global: reply[at + 57] != 0,
        });
    }
    Ok(out)
}

/// One page of physical addresses of a context buffer.
///
/// Returns the addresses and whether RM says that was the last. A scattered
/// buffer needs several calls; `_shadowMemdescCreate` walks it the same way,
/// 128 at a time
pub fn ctx_buffer_ptes(
    rpc: &mut GspRpc,
    rm: &RmClient,
    channel_handle: u32,
    buffer_type: u32,
    first_page: u32,
) -> Result<(Vec<u64>, bool), RmError> {
    let mut p = alloc::vec![0u8; PTES_PARAMS];
    le::put_u32(&mut p, 0x00, rm.h_client);
    le::put_u32(&mut p, 0x04, channel_handle);
    le::put_u32(&mut p, 0x08, buffer_type);
    le::put_u32(&mut p, 0x0c, first_page);
    let reply = rm.control(rpc, rm.h_subdevice, NV2080_CTRL_CMD_KGR_GET_CTX_BUFFER_PTES, &p)?;
    if reply.len() < PTES_PARAMS {
        return Err(RmError::ShortReply);
    }
    let n = (le::u32_at(&reply, 0x10) as usize).min(MAX_BUFFER_PTES);
    let mut pages = Vec::with_capacity(n);
    for i in 0..n {
        pages.push(le::u64_at(&reply, PTES_ARRAY + i * 8));
    }
    Ok((pages, reply[1048] != 0))
}

/// Bind context buffers to the addresses this driver mapped them at.
///
/// The whole parameter block is zero apart from the three header fields and,
/// per entry, the buffer id and the virtual address:
/// `nvGpuOpsBindChannelResources`
pub fn promote_bind(
    rpc: &mut GspRpc,
    rm: &RmClient,
    channel_handle: u32,
    binds: &[(u32, u64)],
) -> Result<usize, RmError> {
    if binds.is_empty() {
        return Ok(0);
    }
    let n = binds.len().min(MAX_ENTRIES);
    let mut p = alloc::vec![0u8; PARAMS_HEAD + MAX_ENTRIES * ENTRY_SIZE];
    le::put_u32(&mut p, 0x00, ENGINE_TYPE_GRAPHICS);
    le::put_u32(&mut p, 0x0c, rm.h_client);       // hChanClient
    le::put_u32(&mut p, 0x10, channel_handle);    // hObject: the channel
    le::put_u32(&mut p, 0x28, n as u32);
    for (i, (id, va)) in binds.iter().take(n).enumerate() {
        let at = PARAMS_HEAD + i * ENTRY_SIZE;
        le::put_u64(&mut p, at + 8, *va);                 // gpuVirtAddr
        p[at + 28..at + 30].copy_from_slice(&(*id as u16).to_le_bytes()); // bufferId
    }
    rm.control(rpc, rm.h_subdevice, NV2080_CTRL_CMD_GPU_PROMOTE_CTX, &p)?;
    Ok(n)
}

/// What RM last said about its own context buffers, and where this driver put
/// them, the only place the card describes its graphics context.
/// `nvidia rm-tree` prints it
static CTX_SEEN: spin::Mutex<Vec<(CtxBuf, u64)>> = spin::Mutex::new(Vec::new());

/// The graphics context as RM described it, one line per buffer
pub fn context_report() -> Vec<alloc::string::String> {
    use core::fmt::Write;
    CTX_SEEN
        .lock()
        .iter()
        .map(|(b, va)| {
            let mut l = alloc::string::String::new();
            let _ = write!(
                l,
                "id {:2}  {:>7} KiB  align {:#7x}  page {:>5}  {:10}  {:12}  va {:#x}",
                b.id,
                b.size / 1024,
                b.alignment,
                b.page_size,
                if b.contig { "contiguous" } else { "scattered" },
                if b.global { "the engine's" } else { "the channel's" },
                va
            );
            l
        })
        .collect()
}

/// Map every context buffer RM owns, and tell RM where they are.
///
/// RM says how many buffers there are, how big each is, where its pages are and
/// which belong to the engine.
///
/// Returns the mapped ranges for a teardown: the pages are RM's and are never
/// freed here, but the mappings are ours
pub fn bind_context(
    rpc: &mut GspRpc,
    rm: &RmClient,
    bar0: &MmioRegion,
    pdb: u64,
    channel_handle: u32,
    say: &dyn Fn(&str),
) -> Result<(Vec<(u64, u64)>, u64), &'static str> {
    let bufs = match ctx_buffer_info(rpc, rm, channel_handle) {
        Ok(b) => b,
        Err(e) => {
            say(&alloc::format!("compute: RM would not list its context buffers: {:?}", e));
            return Err("the context buffer list was refused");
        }
    };
    if bufs.is_empty() {
        return Err("RM reports no graphics context buffers for this channel");
    }
    say(&alloc::format!("compute: RM owns {} context buffer(s)", bufs.len()));

    let mut mapped: Vec<(u64, u64)> = Vec::new();
    let mut binds: Vec<(u32, u64)> = Vec::new();
    let mut va = CTX_BASE_VA;
    CTX_SEEN.lock().clear();

    for b in bufs.iter() {
        if b.size == 0 {
            continue;
        }
        let aperture = match b.aperture {
            ADDR_SYSMEM => gmmu::Aperture::Sysmem,
            ADDR_FBMEM => gmmu::Aperture::Vram,
            other => {
                say(&alloc::format!(
                    "compute: buffer {} lives in aperture {} - not memory this driver can map",
                    b.id, other));
                return Err("a context buffer is in an aperture this driver cannot map");
            }
        };
        let align = b.alignment.max(4096);
        let page = (b.page_size as u64).max(4096);
        let bytes = (b.size + page - 1) / page * page;
        va = (va + align - 1) / align * align;
        if va + bytes > CTX_LIMIT_VA {
            return Err("the context buffers do not fit in the space reserved for them");
        }

        // A contiguous buffer is one range; a scattered one is its page list,
        // asked for 128 at a time as `_shadowMemdescCreate` asks
        if b.contig {
            vaspace::map_priv(bar0, pdb, va, b.phys, bytes, aperture)
                .map_err(|_| "mapping a context buffer failed")?;
        } else {
            let mut at = va;
            let mut first = 0u32;
            // RM's own page count: a size not a whole number of pages would
            // walk one page too far or short
            let want = b.pages.max(bytes / page);
            loop {
                let (pages, last) = match ctx_buffer_ptes(rpc, rm, channel_handle, b.id, first) {
                    Ok(v) => v,
                    Err(e) => {
                        say(&alloc::format!(
                            "compute: RM would not give the pages of buffer {}: {:?}", b.id, e));
                        return Err("a context buffer's page list was refused");
                    }
                };
                if pages.is_empty() {
                    break;
                }
                for phys in pages.iter() {
                    if (at - va) / page >= want {
                        break;
                    }
                    vaspace::map_priv(bar0, pdb, at, *phys, page, aperture)
                        .map_err(|_| "mapping a context buffer page failed")?;
                    at += page;
                }
                first += pages.len() as u32;
                if last || (at - va) / page >= want {
                    break;
                }
            }
        }

        say(&alloc::format!(
            "compute: buffer {:2} - {} KiB, {}, {} - va {:#x}",
            b.id, b.size / 1024,
            if b.contig { "contiguous" } else { "scattered" },
            if b.global { "the engine's" } else { "this channel's" },
            va));

        CTX_SEEN.lock().push((*b, va));
        mapped.push((va, bytes));
        binds.push((b.id, va));
        va += bytes;
    }

    match promote_bind(rpc, rm, channel_handle, &binds) {
        Ok(n) => say(&alloc::format!("compute: {} context buffer(s) bound", n)),
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("compute: binding the context failed: {:?}", e));
            for (v, _) in mapped.iter() {
                vaspace::unmap(bar0, pdb, *v);
            }
            return Err("RM refused the context binding");
        }
    }
    Ok((mapped, va))
}

/// One buffer as it will be handed to RM
#[derive(Clone, Copy)]
pub struct PromoteEntry {
    pub id: u16,
    pub phys: u64,
    /// Where the buffer is in the channel's address space.
    ///
    /// OpenRM's `kgrctxPreparePromoteCtxBuffer` does not fill this for an
    /// externally-owned address space (`kernel_graphics_context.c:1884`), but
    /// dropping it breaks the engine on the card, so something in how this
    /// driver declares its address space to RM differs. The address stays
    pub virt: u64,
    pub size: u64,
    pub initialize: bool,
    pub nonmapped: bool,
    /// The graphics engine's, not this channel's; see `CtxBufSpec`
    pub global: bool,
}

// `virt` and `nonmapped` are both on this wire, though
// `kgrctxPreparePromoteCtxBuffer` fills neither for an externally-owned address
// space (`kernel_graphics_context.c:1884`). A working card wants them; see
// `promote_params`.

/// `physAttr`, from `NV2080_CTRL_GPU_INITIALIZE_CTX_*` in `ctrl2080gpu.h`.
///
/// APERTURE is 1:0 (VIDMEM 0, COH_SYS 1, NCOH_SYS 2) and GPU_CACHEABLE is 2:2,
/// where 1 means not cacheable. It tells RM where a physical address lives:
/// `kgrctxPrepareInitializeCtxBuffer` derives it from the buffer's address
/// space, and RM builds its descriptor from the field.
///
/// Our context buffers are ordinary system memory, coherent through the CPU's
/// cache. nouveau's 4 is for context buffers in video memory
const PHYS_ATTR_COH_SYS: u32 = 1;
const PHYS_ATTR_NOT_GPU_CACHEABLE: u32 = 1 << 2;
const PHYS_ATTR_SYSMEM: u32 = PHYS_ATTR_COH_SYS | PHYS_ATTR_NOT_GPU_CACHEABLE;

/// Encode `NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS`.
///
/// `physAttr` is set on every entry that is initialised and zero on the rest.
///
/// `initialize` is the caller's, since `bInitialize` is a property of RM's
/// state: `kgrctxPrepareInitializeCtxBuffer`
/// (`refsrc/openrm/src/nvidia/src/kernel/gpu/gr/kernel_graphics_context.c:1704`)
/// checks a `bInitialized` flag for every buffer id and adds no entry when it
/// is set: `bKGrMainCtxBufferInitialized` for MAIN,
/// `bKGrPatchCtxBufferInitialized` for PATCH, and `pCtxBuffers->bInitialized[]`
/// for FECS_EVENT and the two privilege maps, the graphics engine's, outliving
/// any channel.
///
/// So the first promotion initialises and later ones must not: asking RM to
/// initialise again is `NV_ERR_INVALID_ARGUMENT`
pub fn promote_params(
    client_handle: u32,
    channel_handle: u32,
    entries: &[PromoteEntry],
    first_time: bool,
    rebind: bool,
) -> (Vec<u8>, usize) {
    // `kgrobjConstruct` (`kernel_graphics_object.c:89`) fills one entry per
    // buffer from two halves. `kgrctxPrepareInitializeCtxBuffer` (`:1704`) sets
    // the physical address, size, `physAttr`, `bInitialize` and
    // `bNonmapped = TRUE`; `kgrctxPreparePromoteCtxBuffer` (`:1861`) then
    // overwrites `bNonmapped` with `FALSE` and adds `gpuVirtAddr` on the same
    // entry, and returns early, writing neither, when the address space is
    // externally owned:
    //
    //     if (gvaspaceIsExternallyOwned(pGVAS)) return NV_OK;
    //
    // This driver's space is declared externally owned, but sending no virtual
    // addresses and `bNonmapped = 1` everywhere gets the compute channel a
    // `ROBUST_CHANNEL_PBDMA_ERROR` on the desktop's first frame. So: every
    // entry with its virtual address, `bNonmapped` from the buffer's own spec.
    //
    // A rebuild sends every buffer too. Without the global ones RM answers
    // `NV_ERR_OBJECT_NOT_FOUND`: the new context never learned where they are.
    // With `bInitialize` on them, `NV_ERR_INVALID_ARGUMENT`: the engine has
    // initialised them. A global buffer's "already initialised" flag lives on
    // the graphics engine (`kgrctxMarkCtxBufferInitialized`, the `pKCtxBuffers`
    // branch) and outlives everything here; its address has to reach the new
    // context, a child of the channel group. So the entry goes and only the
    // flag comes off.
    //
    // A new group means a new `KernelGraphicsContext`, so `MAIN` and `PATCH`
    // need initialising and carry `bInitialize`
    let keep = |e: &&PromoteEntry| first_time || (e.initialize && !e.global);
    let kept: Vec<&PromoteEntry> = entries.iter().filter(keep).take(MAX_ENTRIES).collect();
    let n = kept.len();
    let mut p = alloc::vec![0u8; PARAMS_HEAD + MAX_ENTRIES * ENTRY_SIZE];
    let put32 = |p: &mut Vec<u8>, at: usize, v: u32| {
        le::put_u32(p, at, v);
    };
    let put64 = |p: &mut Vec<u8>, at: usize, v: u64| {
        le::put_u64(p, at, v);
    };

    // Four fields, the rest zero: `kgrobjConstruct` clears the whole parameter
    // block and sets `engineType`, `hChanClient`, `hObject` and `entryCount`.
    // `hClient`, `ChID` and `hVirtMemory` stay zero
    put32(&mut p, 0x00, ENGINE_TYPE_GRAPHICS);
    put32(&mut p, 0x0c, client_handle);   // hChanClient
    put32(&mut p, 0x10, channel_handle);  // hObject
    put32(&mut p, 0x28, n as u32);

    for (i, e) in kept.iter().enumerate() {
        // True for every kept entry on a rebuild, and the buffer's own answer
        // on a first promotion.
        //
        // `rebind` is a promotion into a context that exists and is complete:
        // the group kept it across a teardown. The new channel misses the
        // binding: RM writes the graphics engine's context pointer into the
        // channel's instance block when the context is promoted to it, and a
        // channel without it faults its first pushbuffer fetch with
        // `ROBUST_CHANNEL_PBDMA_ERROR`. So the addresses go over and no
        // `bInitialize` does
        let init = !rebind && e.initialize && (first_time || !e.global);
        let at = PARAMS_HEAD + i * ENTRY_SIZE;
        put64(&mut p, at, if init { e.phys } else { 0 });
        put64(&mut p, at + 8, if e.nonmapped { 0 } else { e.virt });
        put64(&mut p, at + 16, if init { e.size } else { 0 });
        put32(&mut p, at + 24, if init { PHYS_ATTR_SYSMEM } else { 0 });
        p[at + 28..at + 30].copy_from_slice(&e.id.to_le_bytes());
        p[at + 30] = init as u8;
        p[at + 31] = e.nonmapped as u8;
    }
    (p, n)
}

/// Promote the context buffers, after which the channel can run a program
pub fn promote(
    rpc: &mut GspRpc,
    rm: &RmClient,
    channel_handle: u32,
    entries: &[PromoteEntry],
    first_time: bool,
    rebind: bool,
) -> Result<usize, RmError> {
    // `hObject` here is the channel, not the graphics object or the client:
    // `kgrobjConstruct` sets it from
    // `RES_GET_PARENT_HANDLE(pChannelDescendant)`, and a graphics object's
    // parent is its channel. Checked
    if let Some(class) = rm.class_of(channel_handle) {
        if class != super::chan::CLASS_CHANNEL_GPFIFO {
            crate::kerror!("[compute] promote on {:#x}: that is {}, not a channel",
                channel_handle, super::rmapi::class_str(class));
            return Err(RmError::WrongClass {
                handle: channel_handle,
                want: super::chan::CLASS_CHANNEL_GPFIFO,
                got: class,
            });
        }
    }
    let (params, n) = promote_params(rm.h_client, channel_handle, entries, first_time, rebind);
    // "Nothing to promote, so return early": `kgrobjConstruct` issues no
    // control when no buffer needs anything
    if n == 0 {
        return Ok(0);
    }
    rm.control(rpc, rm.h_subdevice, NV2080_CTRL_CMD_GPU_PROMOTE_CTX, &params)?;
    Ok(n)
}

/// Put the compute engine on a channel that already has its context.
///
/// `BLACKWELL_COMPUTE_B` takes no allocation parameters: a zero-length body.
/// From nouveau and tinygrad; not traced to OpenRM
pub fn alloc_class(
    rpc: &mut GspRpc,
    rm: &RmClient,
    channel_handle: u32,
    handle: u32,
) -> Result<(), RmError> {
    rm.alloc(rpc, channel_handle, handle, CLASS_COMPUTE, &[])
}

// Allocating and mapping the context

use crate::drivers::gpu::nvidia::gmmu;

use crate::drivers::gpu::nvidia::vaspace;
use crate::mm::dma::DmaBuffer;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

/// Where the compute channel's own mappings start, clear of the copy engine's
/// ring, pushbuffer and semaphore at 0x100000, 0x200000 and 0x300000. The
/// context buffers are the largest thing in this address space, with the main
/// context alone megabytes
pub const CTX_BASE_VA: u64 = 0x0100_0000;
/// The first address the context must not reach. How large it gets is RM's
/// answer, so the room is checked: two mappings overlapping is a fault whose
/// address belongs to neither. Two hundred and forty megabytes, far more than
/// RM has reported, leaving half the translatable space free
pub const CTX_LIMIT_VA: u64 = 0x1000_0000;
/// The programs, their constant banks and their descriptors, above the ring.
/// One slot per program in each region, so a program's three pieces are found
/// by index.
///
/// A program's slot is 128 KiB: aosh's painter (`paint.cu`, `dev/aosh-gpu.md`)
/// is 82 KiB of code, everything inlined, since a call would need a stack these
/// launches do not set up
pub const PROGRAM_VA: u64 = 0x1100_0000;
pub const PROGRAM_STRIDE: u64 = 0x0002_0000;
/// How many programs fit before the constant banks begin.
///
/// `(CONST0_VA - PROGRAM_VA) / PROGRAM_STRIDE`, written so moving either
/// address moves this. The build fills the first few; the rest are for
/// `gpu_program`, and running out is a refusal
pub const MAX_PROGRAMS: usize = ((CONST0_VA - PROGRAM_VA) / PROGRAM_STRIDE) as usize;
pub const CONST0_VA: u64 = 0x1120_0000;
pub const CONST0_STRIDE: u64 = 0x1000;
pub const QMD_VA: u64 = 0x1130_0000;
/// A page each, though a descriptor is 384 bytes: two in one page would be two
/// mappings of one page at two addresses, the second replacing the first
pub const QMD_STRIDE: u64 = 0x1000;
/// The buffer the fill test writes into
pub const SCRATCH_VA: u64 = 0x1140_0000;
/// Where video memory surfaces are mapped for a kernel to read and write.
/// Sixteen megabytes apart, more than a 4K surface needs
pub const SURFACE_VA: u64 = 0x1200_0000;
pub const SURFACE_STRIDE: u64 = 0x0100_0000;

/// The context, held as long as the channel exists.
///
/// RM has the buffers' addresses, and the hardware walks them while a program
/// runs
#[allow(dead_code)] // `bufs` is held, not read; `next_va` is where the next mapping starts
pub struct ComputeCtx {
    /// Ranges this driver mapped for RM's own context buffers, on the UVM path.
    /// The pages are RM's and never freed here; the mappings come out on a
    /// teardown, or the next channel maps over them
    pub mapped: alloc::vec::Vec<(u64, u64)>,
    /// Has RM initialised these buffers? Set by the first successful promotion
    /// and never cleared: RM's flags outlive a channel too, the buffers
    /// belonging to the graphics engine
    pub initialized: bool,
    pub bufs: alloc::vec::Vec<DmaBuffer>,
    pub entries: alloc::vec::Vec<PromoteEntry>,
    /// The first address after everything mapped here, for whatever comes next
    pub next_va: u64,
    /// What this chip answered when asked which instruction set it speaks, and
    /// the folded form the launch descriptor carries
    pub sm_version: u32,
    pub sass_version: u32,
}

/// Allocate every context buffer and map it into the channel's address space.
///
/// Sizes and alignment come from RM's reply. The privilege map alone is not
/// mapped into the channel's address space.
///
/// That special case is nouveau's. OpenRM maps one of the two register-access
/// maps into the channel's space, read-only: the one
/// `kgrctxGetRegisterAccessMapId_HAL` picks (`kgrctxMapCtxBuffers_IMPL`,
/// `kernel_graphics_context.c:1651`). `promote_params` sends each buffer's own
/// `nonmapped` and says why
pub fn allocate_context(
    bar0: &MmioRegion,
    pdb: u64,
    info: &[CtxBufInfo],
    say: &dyn Fn(&str),
) -> Result<ComputeCtx, &'static str> {
    allocate_buffers(bar0, pdb, info, &|_| true, say)
}

/// `allocate_context` for the buffers `which` picks
fn allocate_buffers(
    bar0: &MmioRegion,
    pdb: u64,
    info: &[CtxBufInfo],
    which: &dyn Fn(&CtxBufSpec) -> bool,
    say: &dyn Fn(&str),
) -> Result<ComputeCtx, &'static str> {
    let mut bufs = alloc::vec::Vec::new();
    let mut entries = alloc::vec::Vec::new();
    let mut va = CTX_BASE_VA;

    for spec in CTX_BUFFERS.iter().filter(|s| which(s)) {
        let Some(i) = info.get(spec.engine_index) else {
            return Err("the context buffers reply is shorter than the buffer list");
        };
        // RM says "nothing here" with zero, and with all ones for a slot it
        // never filled in. All ones reads as a four-gigabyte buffer, the sign
        // of a wrong engine index
        if i.size == 0 || i.size == u32::MAX {
            say(&alloc::format!("compute: buffer {} is not used on this chip", spec.id));
            continue;
        }
        let align = (i.alignment as u64).max(4096);
        let bytes = ((i.size as u64 + spec.extra as u64) + align - 1) / align * align;
        let pages = (bytes as usize + 4095) / 4096;

        // Said before anything is allocated, so a buffer failing on its size
        // has had its size printed
        say(&alloc::format!(
            "compute: buffer {:2} - RM says {} KiB align {:#x}, taking {} KiB",
            spec.id, i.size / 1024, i.alignment, bytes / 1024));

        let mut buf = DmaBuffer::alloc(pages)
            .map_err(|_| "not enough system memory for a context buffer")?;
        buf.zero();
        let phys = buf.phys();

        let mut virt = 0;
        if !spec.nonmapped {
            va = (va + align - 1) / align * align;
            if va + bytes > CTX_LIMIT_VA {
                return Err("the context buffers do not fit in the space reserved for them");
            }
            vaspace::map_priv(bar0, pdb, va, phys, bytes, gmmu::Aperture::Sysmem)
                .map_err(|_| "mapping a context buffer failed")?;
            virt = va;
            va += bytes;
        }

        say(&alloc::format!(
            "compute: buffer {:2}   phys {:#x}{}",
            spec.id, phys,
            if spec.nonmapped {
                alloc::format!(", unmapped")
            } else {
                alloc::format!(", va {:#x}", virt)
            }));

        entries.push(PromoteEntry {
            id: spec.id,
            phys,
            virt,
            size: bytes,
            initialize: spec.init,
            nonmapped: spec.nonmapped,
            global: spec.global,
        });
        bufs.push(buf);
    }

    Ok(ComputeCtx {
        mapped: alloc::vec::Vec::new(), initialized: false, bufs, entries, next_va: va, sm_version: 0, sass_version: 0 })
}

/// The graphics engine's global context buffers, as the kernel's context
/// allocated them and its first promotion initialised them: what a process's
/// context borrows (`allocate_process_context`). Set once that promotion has
/// succeeded, cleared with the context when GSP-RM goes
static GLOBALS: spin::Mutex<Vec<PromoteEntry>> = spin::Mutex::new(Vec::new());

/// A context for a channel in a process's own address space, as OpenRM builds a
/// second `KernelGraphicsContext` (`dev/gpu-memory-compute.md`, 3.3).
///
/// Its own `MAIN` and `PATCH`, allocated here and initialised by the promotion:
/// their flags live on the context (`bKGrMainCtxBufferInitialized`,
/// `bKGrPatchCtxBufferInitialized`), and a new group means a new context
/// (`kgrctxAllocCtxBuffers_IMPL`, `kernel_graphics_context.c:1355`).
///
/// The engine's global buffers, borrowed: the kernel's context's pages, mapped
/// into this space at this space's addresses (`kgrctxMapGlobalCtxBuffers_IMPL`,
/// `:1527`), and promoted by address only. Their flags live on the engine
/// (`pGlobalCtxBuffers->bInitialized[]`, set by
/// `kgrctxMarkCtxBufferInitialized` after the kernel's promotion), so
/// `kgrctxPrepareInitializeCtxBuffer` (`:1689`) adds no initialise half.
/// Initialising them again at new physical addresses stops the kernel's live
/// context.
///
/// Which globals: the four 3D buffers of the kernel's context (`CTX_BUFFERS`),
/// and of the two register-access maps the one `kgrctxGetRegisterAccessMapId`
/// picks for a non-administrator's channel, the restricted `PRIV_ACCESS_MAP`
/// (`:3311`), mapped (`kgrctxMapCtxBuffers_IMPL`, `:1652`). `FECS_EVENT` is
/// left out: OpenRM never maps it into a channel's space, so
/// `kgrctxPreparePromoteCtxBuffer` finds no address and, with its
/// initialisation done, sends no entry
pub fn allocate_process_context(
    bar0: &MmioRegion,
    pdb: u64,
    info: &[CtxBufInfo],
    say: &dyn Fn(&str),
) -> Result<ComputeCtx, &'static str> {
    let globals = GLOBALS.lock().clone();
    if globals.is_empty() {
        return Err("the kernel's context has not been promoted - there are no global buffers to share");
    }
    let mut ctx = allocate_buffers(bar0, pdb, info, &|s| !s.global, say)?;
    let mut va = ctx.next_va;
    for g in globals.iter().filter(|g| {
        g.id != buf::FECS_EVENT && g.id != buf::UNRESTRICTED_PRIV_ACCESS_MAP
    }) {
        va = va.next_multiple_of(BIG_PAGE_ALIGN);
        if va + g.size > CTX_LIMIT_VA {
            return Err("the shared context buffers do not fit in the space reserved for them");
        }
        vaspace::map_priv(bar0, pdb, va, g.phys, g.size, gmmu::Aperture::Sysmem)
            .map_err(|_| "mapping a shared context buffer failed")?;
        say(&alloc::format!(
            "compute: buffer {:2} - the engine's, {} KiB at phys {:#x}, shared at va {:#x}",
            g.id, g.size / 1024, g.phys, va));
        ctx.entries.push(PromoteEntry {
            id: g.id,
            phys: g.phys,
            virt: va,
            size: g.size,
            initialize: false,
            nonmapped: false,
            global: true,
        });
        va += g.size;
    }
    ctx.next_va = va;
    Ok(ctx)
}

/// Where a shared buffer starts in a process's space. The kernel's context
/// aligns each buffer to what RM reported; 64 KiB covers every alignment RM has
/// reported for these
const BIG_PAGE_ALIGN: u64 = 0x1_0000;

// Bringing the whole thing up

use super::chan::{Channel, ChanVaSpace};
use super::rpc::GspRpc as Rpc;

/// The memory a compute channel needs, allocated and mapped by the caller. The
/// caller owns the allocator and the video-memory carve; this file owns the
/// sequence
pub struct ChanMem {
    /// The timeslice group for this channel, if one is kept alive across a
    /// teardown. `None` allocates a fresh one
    pub group: Option<u32>,
    /// Allocate a graphics context for this channel, not the parked one. True
    /// for a channel in an address space of its own: the parked context's
    /// buffers are mapped in the kernel's page directory
    pub own_context: bool,
    /// The channel id, taken by the caller since the addresses below derive
    /// from it
    pub chid: u32,
    /// Instance block and USERD, in video memory, `CHAN_MEM_SIZE` bytes
    pub inst: u64,
    /// Method buffer, in system memory, the size RM asked for
    pub mthdbuf: u64,
    pub mthdbuf_size: u64,
    /// Ring, pushbuffer and semaphore: physical addresses and where they are
    /// mapped in the channel's address space
    pub gpfifo: (u64, u64, u64),
    pub pushbuf: (u64, u64, u64),
    pub sema: (u64, u64, u64),
}

/// The graphics context, which outlives any one channel.
///
/// Seven of its nine buffers are the graphics engine's
/// (`GR_GLOBALCTX_BUFFER_*`), so a torn-down channel does not make them stale
/// and a rebuild does not allocate them. Held here, handed to the current
/// channel, and put back when it goes
static CONTEXT: spin::Mutex<Option<ComputeCtx>> = spin::Mutex::new(None);

/// GSP-RM was unloaded, and the graphics engine with it: the context's buffers
/// and RM's flags belong to a processor that is gone. The next boot builds a
/// context from nothing. The pages are retired, as with a torn-down channel
/// (`chan::retire_dma`)
pub fn forget() {
    if let Some(ctx) = CONTEXT.lock().take() {
        super::chan::retire_dma(ctx.bufs);
    }
    GLOBALS.lock().clear();
    CTX_SEEN.lock().clear();
}

/// Hand the graphics context back, for a channel being torn down. Its buffers
/// stay allocated and mapped: RM knows where they are, and the next channel
/// promotes the same addresses
pub fn park_context(ctx: ComputeCtx) {
    *CONTEXT.lock() = Some(ctx);
}

/// Give a half-built channel back to RM.
///
/// Not `Channel::destroy`: nothing to quiesce, nothing mapped through it, no
/// engine on it. RM has written down a handle, and an unfreed handle fails the
/// next attempt
fn abandon(rpc: &mut GspRpc, rm: &RmClient, chan: Channel, say: &dyn Fn(&str)) {
    let handle = chan.handle;
    match rm.free(rpc, handle) {
        Ok(()) => say(&alloc::format!("compute: gave channel {:#x} back", handle)),
        Err(e) => say(&alloc::format!(
            "compute: RM would not take channel {:#x} back: {:?} - the next attempt \
             will be refused as a duplicate",
            handle, e
        )),
    }
    // The group it hangs off, after it, only if this channel made it: a group
    // from a previous build carries RM's graphics context
    if chan.group != 0 && chan.owns_group {
        if let Err(e) = rm.free(rpc, chan.group) {
            say(&alloc::format!(
                "compute: RM would not take group {:#x} back: {:?}", chan.group, e));
        }
    }
    // The ring pointers are raw and the buffers owned elsewhere here, so only
    // the struct is left
    core::mem::forget(chan);
}

/// Ask RM for its context buffers: the UVM path. It does not work on this
/// machine.
///
/// `ctxuvm` on the GRUB line, off by default.
///
/// `nvGpuOpsBindChannelResources` promotes two fields per entry, and this
/// driver's address space is externally owned, but UVM is a client of CPU-RM,
/// and `NV2080_CTRL_CMD_GR_GET_CTX_BUFFER_INFO` is answered by CPU-RM from its
/// own `KernelGraphicsContext` (`subdeviceCtrlCmdKGrGetCtxBufferInfo_IMPL`,
/// `refsrc/openrm/src/nvidia/src/kernel/gpu/gr/kernel_graphics.c:3909`),
/// forwarded to the host only under vGPU. Sent to GSP-RM it comes back
/// `NV_ERR_INVALID_OBJECT`.
///
/// This driver is CPU-RM: the context buffers are ours to allocate and
/// describe, and `kgrobjConstruct` is the reference
fn uvm_context() -> bool {
    crate::arch::x86_64::grub::cmdline_flag("ctxuvm")
}

/// Put a context back when a bring-up gives up part way. The kernel's goes into
/// the parked slot, keeping its buffers and RM's record of them. A process's is
/// dropped, with its buffers
fn park_or_drop(own_context: bool, ctx: ComputeCtx) {
    if !own_context {
        *CONTEXT.lock() = Some(ctx);
    }
}

/// Everything, in the order RM insists on. The steps are numbered in the file
/// header, and each says what failed
pub fn bring_up(
    rpc: &mut Rpc,
    client: &RmClient,
    internal: &RmClient,
    bar0: &MmioRegion,
    vas: &ChanVaSpace,
    mem: &ChanMem,
    say: &dyn Fn(&str),
) -> Option<(Channel, ComputeCtx)> {
    // 1. how large the context buffers are on this chip
    let info = match context_buffers_info(rpc, internal) {
        Ok(i) => i,
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("compute: context buffer sizes refused: {:?}", e));
            return None;
        }
    };
    say(&alloc::format!(
        "compute: main context {} KiB, patch {} KiB (as RM reports them)",
        info[eng::GRAPHICS].size / 1024, info[eng::PATCH].size / 1024));

    // 2. allocate them and map them where the channel can reach them, unless a
    //    context exists, which is reused.
    //
    //    Seven of the nine buffers below are global context buffers: `GR_GLOBALCTX_BUFFER_{BUNDLE_CB, PAGEPOOL, ATTRIBUTE_CB, RTV_CB, FECS_EVENT, PRIV_ACCESS_MAP, UNRESTRICTED_PRIV_ACCESS_MAP}` in `refsrc/openrm/src/.../gr/kernel_graphics_context.c:155`, belonging to the graphics engine. Offering RM a global buffer at a different physical address gets `NV_ERR_INVALID_ARGUMENT` on the second promotion.
    //
    //    So the context outlives the channel, and a rebuild promotes the addresses RM knows to the new channel. A space of its own means a context of its own: the parked context's buffers are mapped in the kernel's page directory. That one is not routed through `CONTEXT`, the slot holding the kernel's context between teardowns
    let mut ctx = if mem.own_context {
        match allocate_process_context(bar0, vas.pdb, &info, say) {
            Ok(c) => c,
            Err(e) => {
                said(Level::Error, &say, &alloc::format!("compute: context allocation failed: {}", e));
                return None;
            }
        }
    } else {
        if CONTEXT.lock().is_none() {
            match allocate_context(bar0, vas.pdb, &info, say) {
                Ok(c) => *CONTEXT.lock() = Some(c),
                Err(e) => {
                    said(Level::Error, &say, &alloc::format!("compute: context allocation failed: {}", e));
                    return None;
                }
            }
        } else {
            say("compute: reusing the graphics context - its buffers have not moved");
        }
        CONTEXT.lock().take()?
    };

    match sass_version(rpc, internal) {
        Ok((sm, sass)) => {
            ctx.sm_version = sm;
            ctx.sass_version = sass;
            say(&alloc::format!(
                "compute: SM version {:#x}, descriptor wants {:#x}", sm, sass));
        }
        Err(e) => {
            say(&alloc::format!("compute: the chip would not say its SM version: {:?}", e));
            park_or_drop(mem.own_context, ctx);
            return None;
        }
    }

    // 3. the channel, on the graphics engine this time
    let mut chan = match Channel::create(
        rpc, client, bar0, vas, mem.inst, mem.mthdbuf, mem.mthdbuf_size,
        mem.gpfifo.1, mem.gpfifo.2, ENGINE_TYPE_GRAPHICS, mem.chid,
        // The group from the last build, if any. It owns the graphics context,
        // so a rebuilt channel inherits a context RM believes in
        mem.group,
        say,
    ) {
        Ok(c) => c,
        Err(RmError::Status(s)) => {
            said(Level::Error, &say, &alloc::format!("compute: channel refused: {:#x}", s));
            park_or_drop(mem.own_context, ctx);
            return None;
        }
        Err(e) => {
            said(Level::Error, &say, &alloc::format!("compute: channel failed: {:?}", e));
            park_or_drop(mem.own_context, ctx);
            return None;
        }
    };
    say(&alloc::format!(
        "compute: channel {:#x} chid {} on the graphics engine", chan.handle, chan.chid));

    // 4 and 5. the context and the engine class. The order differs between the
    // two paths.
    //
    // On the default path the context is promoted first and the class second.
    // Class first, to match `kgrobjConstruct`, where the promotion happens
    // inside the class object's constructor, gets the first effects frame
    // `ROBUST_CHANNEL_FIFO_ERROR_MMU_ERR_FLT` with a fault address. There RM
    // does both; here they are separate RPCs and the engine latches something
    // from the context when the class is allocated.
    //
    // On the `ctxuvm` path it is the other way round, since the context buffers
    // do not exist until a graphics object does (off; see `uvm_context`)
    if uvm_context() {
        if let Err(e) = alloc_class(rpc, client, chan.handle, H_COMPUTE | chan.chid) {
            said(Level::Error, &say, &alloc::format!("compute: class {:#x} refused: {:?}", CLASS_COMPUTE, e));
            abandon(rpc, client, chan, say);
            park_or_drop(mem.own_context, ctx);
            return None;
        }
        match bind_context(rpc, client, bar0, vas.pdb, chan.handle, say) {
            Ok((mapped, next)) => {
                ctx.bufs.clear();
                ctx.entries.clear();
                ctx.mapped = mapped;
                ctx.next_va = next;
                ctx.initialized = true;
            }
            Err(why) => {
                say(&alloc::format!("compute: {}", why));
                abandon(rpc, client, chan, say);
                park_or_drop(mem.own_context, ctx);
                return None;
            }
        }
    } else if mem.group.is_some() && ctx.initialized {
        // A rebuild into a group that kept RM's graphics context. RM promotes
        // nothing here and says so with `NV_ERR_INVALID_ARGUMENT`. What is
        // missing is the engine context pointer in the new channel's instance
        // block, written below as `gv100_ectx_bind` writes it
        say("compute: the group kept its graphics context - binding it to the new channel");
    } else {
        let promoted = match promote(rpc, client, chan.handle, &ctx.entries, !ctx.initialized, false) {
            Ok(n) => n,
            Err(e) => {
                said(Level::Error, &say, &alloc::format!("compute: promoting the context failed: {:?}", e));
                // The channel exists by now, and giving up without freeing it
                // leaves the handle registered with RM; the next attempt is
                // refused as a duplicate
                abandon(rpc, client, chan, say);
                park_or_drop(mem.own_context, ctx);
                return None;
            }
        };
        ctx.initialized = true;
        // The engine's buffers are initialised now, at these addresses, what
        // every process's context borrows
        if !mem.own_context {
            *GLOBALS.lock() = ctx.entries.iter().filter(|e| e.global).copied().collect();
        }
        say(&alloc::format!(
            "compute: {} of {} context buffers needed promoting",
            promoted, ctx.entries.len()
        ));
        super::chan::probe_inst(bar0, mem.inst, mem.chid, "promoted", say);

        if let Err(e) = alloc_class(rpc, client, chan.handle, H_COMPUTE | chan.chid) {
            said(Level::Error, &say, &alloc::format!("compute: class {:#x} refused: {:?}", CLASS_COMPUTE, e));
            abandon(rpc, client, chan, say);
            park_or_drop(mem.own_context, ctx);
            return None;
        }
    }

    super::chan::probe_inst(bar0, mem.inst, mem.chid, "class", say);

    // Point the instance block at the main context buffer. On a first promotion
    // GSP-RM has written these words and this repeats the value; on a rebuild
    // only this writes them
    let main_va = ctx
        .entries
        .iter()
        .find(|e| e.id == buf::MAIN)
        .map(|e| e.virt)
        .unwrap_or(0);
    if main_va != 0 {
        chan.bind_gr_context(bar0, main_va, say);
        say(&alloc::format!(
            "compute: instance block points at the graphics context at {:#x}", main_va));
    } else {
        say("compute: no main context buffer address - the instance block is not bound");
    }

    // The doorbell is the graphics runlist's, not the copy engine's
    chan.doorbell = chan.doorbell_handle(0);
    say(&alloc::format!(
        "compute: class {:#x} is on the channel, doorbell {:#010x}",
        CLASS_COMPUTE, chan.doorbell));

    if let Err(e) = chan.attach_ring(
        crate::arch::x86_64::grub::phys_to_virt(mem.pushbuf.0) as *mut u32,
        mem.pushbuf.1,
        (mem.pushbuf.2 / 4) as usize,
        crate::arch::x86_64::grub::phys_to_virt(mem.gpfifo.0) as *mut u32,
        crate::arch::x86_64::grub::phys_to_virt(mem.sema.0) as *mut u32,
        mem.sema.1,
        mem.sema.0,
    ) {
        say(&alloc::format!("compute: channel ring refused: {}", e));
        return None;
    }

    // Everything RM writes into this page has been written. The first one is
    // kept; each later one is reported as a difference against it
    super::chan::probe_inst(bar0, mem.inst, mem.chid, "ring", say);
    // An instrument: `bind_gr_context` sets the missing bit, so a rebuild's
    // instance block should differ from a working one in nothing. A reported
    // word means something else stopped being written
    let rebuilt = ctx.initialized && mem.group.is_some();
    chan.snapshot_inst(bar0, if rebuilt { "rebuilt" } else { "first" }, false, say);

    Some((chan, ctx))
}

/// Our handle for the compute object on the channel, tagged like the others
pub const H_COMPUTE: u32 = 0xc000_0000;

// Launching a program

use super::qmd::{Launch, Qmd, QMD_BYTES};

/// What the driver puts in constant bank 0 before the parameters.
///
/// A compiled kernel reads more than its arguments from that bank:
///
///   0x358  the descriptor global stores go through on this architecture
///   0x360  threads per block, one word each for x, y and z, which the program
///          multiplies by the block index to work out which pixel it owns
///   0x37c  the stack pointer, loaded into R1 by the first instruction
///
/// The offsets are read from the disassembly of the kernels we ship: the
/// compiler's ABI, which moved between architectures. A one-dimensional kernel
/// never reads 0x364 and a two-dimensional one does
pub const CB_GLOBAL_DESC: usize = 0x358;
pub const CB_NTID_X: usize = 0x360;
pub const CB_NTID_Y: usize = 0x364;
pub const CB_NTID_Z: usize = 0x368;
pub const CB_STACK: usize = 0x37c;
/// The value the reference puts in the stack pointer: the top of the local
/// memory window. Nothing we run pushes anything, but the load happens
pub const STACK_TOP: u32 = 0x00ff_fdc0;

/// Build the descriptor for a program, in a buffer the caller has mapped
pub fn build_qmd(qmd_bytes: &mut [u8], launch: &Launch) -> Result<(), &'static str> {
    if qmd_bytes.len() < QMD_BYTES {
        return Err("the descriptor does not fit");
    }
    let q = Qmd::new(launch);
    for (i, w) in q.as_words().iter().enumerate() {
        le::put_u32(qmd_bytes, i * 4, *w);
    }
    Ok(())
}

// Programs, loaded once

use crate::drivers::gpu::nvidia::shaders;

/// How many dispatches one submission can carry.
///
/// A dispatch is a descriptor plus the constant bank it points at, and the same
/// program launched twice with different arguments needs two of both. So these
/// are per launch in a chain, and one blurred window is seven: three levels
/// down, three back up, and the compose
pub const MAX_SLOTS: usize = 48;

/// Launch slots held back for ring 3.
///
/// The effects pass takes its slots from the bottom and ring 3 the top, so the
/// two never pick one descriptor. A process has a single submission in flight
/// (`MAX_IN_FLIGHT` in the syscall adapter), but a submission can be a chain of
/// launches (`GPU_OP_CHAIN`): aosh draws a frame, its layers then its windows,
/// as one. Sixteen, the longest chain the adapter takes; the effects pass keeps
/// 32
pub const CLIENT_SLOTS: usize = 16;

/// The first slot a submission from ring 3 uses: its `n`th launch is in
/// `CLIENT_SLOT + n`
pub const CLIENT_SLOT: usize = MAX_SLOTS - CLIENT_SLOTS;

/// Where a pool bound by `gpu_bind` is mapped.
///
/// At 4 GiB: clear of the kernel's own map (ending at 800 MiB, the last of
/// `blur.rs`'s window slots) and the per-channel ring windows at 1 GiB, with
/// room for a pool of the largest size `may_pool` allows at every slot
pub const CLIENT_VA_BASE: u64 = 0x1_0000_0000;
/// One `MAX_POOL_BYTES` apart, so a bind can never reach into its neighbour
pub const CLIENT_VA_STRIDE: u64 = 64 << 20;
/// How many pools ring 3 may have bound at once, all processes together. aosh's
/// commands, textures and window buffers take three to five (`dev/aosh-gpu.md`)
pub const MAX_CLIENT_BINDS: usize = 16;

/// Where the `n`th client bind lands
pub fn client_va(slot: usize) -> u64 {
    CLIENT_VA_BASE + slot as u64 * CLIENT_VA_STRIDE
}

pub struct Programs {
    /// Kept alive since the GPU addresses them
    _bufs: alloc::vec::Vec<DmaBuffer>,
    /// Indexed by the number a dispatch names, so an entry never moves. A
    /// process's program leaves a hole when it exits, and the next load takes
    /// the hole: its address is fixed by its index
    progs: alloc::vec::Vec<Option<Prog>>,
    slots: alloc::vec::Vec<Slot>,
}

/// The pointers are into the direct map of memory this struct owns, tied to no
/// processor
unsafe impl Send for Programs {}

struct Prog {
    /// Owned, because a program loaded at run time has no name in the binary.
    /// The built-in ones copy theirs out of the generated table
    name: alloc::string::String,
    prog_va: u64,
    /// How much is mapped at `prog_va`: the code and the slack page after it
    map_bytes: u64,
    code_bytes: u32,
    const_size: u32,
    regs: u32,
    shared: u32,
    /// The process that loaded it, or `None` for one the build compiled
    owner: Option<u64>,
    /// The code's pages, for a loaded program; the built-in ones live in
    /// `_bufs`
    buf: Option<DmaBuffer>,
}

struct Slot {
    const_va: u64,
    const_ptr: *mut u8,
    qmd_va: u64,
    qmd_ptr: *mut u8,
}

impl Programs {
    /// Upload and map everything `build.rs` compiled, plus the slots a chain of
    /// launches draws from
    pub fn load(bar0: &MmioRegion, pdb: u64, say: &dyn Fn(&str)) -> Result<Self, &'static str> {
        let mut bufs = alloc::vec::Vec::new();
        let mut progs = alloc::vec::Vec::new();
        let mut slots = alloc::vec::Vec::new();

        for (i, p) in shaders::PROGRAMS.iter().enumerate() {
            // A page of slack after the code: the front end reads ahead past
            // the end of a kernel, and an unmapped page there is a fault
            let code_pages = (p.code.len() + 4095) / 4096 + 1;
            // Mapped a stride apart: longer would be mapped over the next one's
            // first pages
            if code_pages as u64 * 4096 > PROGRAM_STRIDE {
                say(&alloc::format!("compute: program '{}' is {} bytes, more than a {} KiB slot holds",
                                    p.name, p.code.len(), PROGRAM_STRIDE / 1024));
                return Err("a built-in program does not fit its slot");
            }
            let mut code = DmaBuffer::alloc(code_pages).map_err(|_| "no memory for a program")?;
            code.zero();
            let phys = code.phys();
            let prog_va = PROGRAM_VA + i as u64 * PROGRAM_STRIDE;
            vaspace::map(bar0, pdb, prog_va, phys, (code_pages * 4096) as u64,
                         gmmu::Aperture::Sysmem)
                .map_err(|_| "mapping a program failed")?;

            let ptr = crate::arch::x86_64::grub::phys_to_virt(phys) as *mut u8;
            unsafe { core::ptr::copy_nonoverlapping(p.code.as_ptr(), ptr, p.code.len()) };

            progs.push(Some(Prog {
                name: alloc::string::String::from(p.name),
                prog_va,
                map_bytes: (code_pages * 4096) as u64,
                code_bytes: p.code.len() as u32,
                const_size: p.const0,
                regs: p.regs,
                shared: p.shared,
                owner: None,
                buf: None,
            }));
            bufs.push(code);
        }

        for j in 0..MAX_SLOTS {
            let mut cbuf = DmaBuffer::alloc(1).map_err(|_| "no memory for a constant bank")?;
            cbuf.zero();
            let mut qmd = DmaBuffer::alloc(1).map_err(|_| "no memory for a descriptor")?;
            qmd.zero();
            let (cp, qp) = (cbuf.phys(), qmd.phys());
            let const_va = CONST0_VA + j as u64 * CONST0_STRIDE;
            let qmd_va = QMD_VA + j as u64 * QMD_STRIDE;
            for (va, phys) in [(const_va, cp), (qmd_va, qp)] {
                vaspace::map(bar0, pdb, va, phys, 4096, gmmu::Aperture::Sysmem)
                    .map_err(|_| "mapping a launch slot failed")?;
            }
            slots.push(Slot {
                const_va,
                const_ptr: crate::arch::x86_64::grub::phys_to_virt(cp) as *mut u8,
                qmd_va,
                qmd_ptr: crate::arch::x86_64::grub::phys_to_virt(qp) as *mut u8,
            });
            bufs.push(cbuf);
            bufs.push(qmd);
        }

        DmaBuffer::write_barrier();
        say(&alloc::format!(
            "compute: {} program(s) uploaded, {} launch slots mapped", progs.len(), slots.len()));
        Ok(Programs { _bufs: bufs, progs, slots })
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.progs.iter().position(|p| p.as_ref().is_some_and(|p| p.name == name))
    }

    /// Every range this set of programs mapped (the code, the constant banks
    /// and the launch descriptors) as `(va, bytes)`: what a teardown takes out
    /// before the pages can go. These are `map_at` mappings, so
    /// `gmmu::unmap_at` needs the length
    pub fn mapped_ranges(&self) -> alloc::vec::Vec<(u64, u64)> {
        let mut out = alloc::vec::Vec::new();
        for p in self.progs.iter().flatten() {
            out.push((p.prog_va, p.map_bytes));
        }
        for s in self.slots.iter() {
            out.push((s.const_va, 4096));
            out.push((s.qmd_va, 4096));
        }
        out
    }

    /// The pages, for retiring. Consumes the set: the addresses above are no
    /// longer mapped by the time anybody wants these
    pub fn into_buffers(self) -> alloc::vec::Vec<DmaBuffer> {
        let mut bufs = self._bufs;
        bufs.extend(self.progs.into_iter().flatten().filter_map(|p| p.buf));
        bufs
    }

    /// Every program's name, in index order; an index nobody holds is an empty
    /// name. `gpu_info` prints it, so a client can name one
    pub fn names(&self) -> alloc::vec::Vec<&str> {
        self.progs.iter().map(|p| p.as_ref().map_or("", |p| p.name.as_str())).collect()
    }

    /// Every process that has loaded something into this set
    pub fn owners(&self) -> Vec<u64> {
        let mut out: Vec<u64> = self.progs.iter().flatten().filter_map(|p| p.owner).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Has `owner` loaded anything into this set?
    pub fn owned_by(&self, owner: u64) -> bool {
        self.progs.iter().flatten().any(|p| p.owner == Some(owner))
    }

    /// Take out every program `owner` loaded, and hand back its pages.
    ///
    /// For a process that exited. Its index becomes a hole the next load fills.
    /// The caller has made sure the engine is not running any of it
    pub fn remove_owned(
        &mut self,
        bar0: &MmioRegion,
        pdb: u64,
        owner: u64,
    ) -> alloc::vec::Vec<DmaBuffer> {
        let mut bufs = alloc::vec::Vec::new();
        for entry in self.progs.iter_mut() {
            if entry.as_ref().is_some_and(|p| p.owner == Some(owner)) {
                let p = entry.take().unwrap();
                vaspace::unmap(bar0, pdb, p.prog_va);
                bufs.extend(p.buf);
            }
        }
        // Keep the list as short as the highest index in use, so `gpu_info`
        // prints no tail of blanks
        while self.progs.last().is_some_and(|p| p.is_none()) {
            self.progs.pop();
        }
        bufs
    }

    /// Load a program the build did not compile.
    ///
    /// What `gpu_program` ends up in: a ring-3 process handed over a cubin,
    /// `nvidia/cubin.rs` found a kernel in it, and the code is uploaded and
    /// mapped as the built-in ones are, with the same slack page.
    ///
    /// Returns the index a dispatch names, the kind `gpu_info` prints; a loaded
    /// program joins the end of the one list
    pub fn add(
        &mut self,
        bar0: &MmioRegion,
        pdb: u64,
        owner: u64,
        name: &str,
        code: &[u8],
        const0: u32,
        regs: u32,
        shared: u32,
    ) -> Result<usize, &'static str> {
        let index = match self.progs.iter().position(|p| p.is_none()) {
            Some(hole) => hole,
            None if self.progs.len() < MAX_PROGRAMS => self.progs.len(),
            None => return Err("no program slot is free"),
        };
        if code.is_empty() {
            return Err("the program has no code");
        }
        // The constant bank is one page per slot, with the launch's own words
        // at the front. A program reading further would read whatever the next
        // page holds
        if const0 as u64 > CONST0_STRIDE {
            return Err("the program's constant bank does not fit a slot");
        }

        let code_pages = (code.len() + 4095) / 4096 + 1;
        if code_pages as u64 * 4096 > PROGRAM_STRIDE {
            return Err("the program is longer than a program slot");
        }
        let mut buf = DmaBuffer::alloc(code_pages).map_err(|_| "no memory for a program")?;
        buf.zero();
        let phys = buf.phys();
        let prog_va = PROGRAM_VA + index as u64 * PROGRAM_STRIDE;
        let map_bytes = (code_pages * 4096) as u64;
        vaspace::map(bar0, pdb, prog_va, phys, map_bytes, gmmu::Aperture::Sysmem)
            .map_err(|_| "mapping the program failed")?;

        let ptr = crate::arch::x86_64::grub::phys_to_virt(phys) as *mut u8;
        unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), ptr, code.len()) };
        DmaBuffer::write_barrier();

        let prog = Prog {
            name: alloc::string::String::from(name),
            prog_va,
            map_bytes,
            code_bytes: code.len() as u32,
            const_size: const0,
            regs,
            shared,
            owner: Some(owner),
            buf: Some(buf),
        };
        if index == self.progs.len() {
            self.progs.push(Some(prog));
        } else {
            self.progs[index] = Some(prog);
        }
        Ok(index)
    }


    /// Fill in one slot's constant bank and descriptor, and say where the
    /// descriptor is.
    ///
    /// `params` goes at `PARAM_BASE`, laid out as the compiler expects: the C
    /// rule, each value on its own alignment, in declaration order.
    ///
    /// Nothing is submitted here: a chain is prepared slot by slot and handed
    /// over in one piece
    pub fn prepare(
        &self,
        slot: usize,
        program: usize,
        params: &[u8],
        block: (u32, u32, u32),
        grid: (u32, u32, u32),
        sass_version: u32,
    ) -> Result<u64, &'static str> {
        let p = self.progs.get(program).and_then(Option::as_ref).ok_or("no such program")?;
        let s = self.slots.get(slot).ok_or("more launches than there are slots")?;
        if PARAM_BASE as usize + params.len() > p.const_size as usize {
            return Err("the arguments do not fit in the program's constant bank");
        }

        unsafe {
            let put = |off: usize, v: u32| {
                core::ptr::write_volatile(s.const_ptr.add(off) as *mut u32, v)
            };
            put(CB_GLOBAL_DESC, 0);
            put(CB_GLOBAL_DESC + 4, 0);
            put(CB_NTID_X, block.0);
            put(CB_NTID_Y, block.1);
            put(CB_NTID_Z, block.2);
            put(CB_STACK, STACK_TOP);
            core::ptr::copy_nonoverlapping(
                params.as_ptr(), s.const_ptr.add(PARAM_BASE as usize), params.len());
            // What the program reads past what it was given is nought, not the
            // last launch's words from this slot, perhaps another process's
            let end = p.const_size as usize;
            let from = PARAM_BASE as usize + params.len();
            if end > from {
                core::ptr::write_bytes(s.const_ptr.add(from), 0, end - from);
            }

            let qmd = core::slice::from_raw_parts_mut(s.qmd_ptr, QMD_BYTES);
            qmd.fill(0);
            build_qmd(qmd, &Launch {
                program: p.prog_va,
                code_bytes: p.code_bytes,
                const0: s.const_va,
                const0_size: p.const_size,
                block,
                grid,
                regs: p.regs,
                shared: p.shared,
                sass_version,
            })?;
        }
        Ok(s.qmd_va)
    }
}

/// Where a kernel's arguments begin in its constant bank, from the build
pub use shaders::PARAM_BASE;
