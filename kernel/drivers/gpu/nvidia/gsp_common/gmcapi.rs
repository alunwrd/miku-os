// GMCAPI, GSP-RM's command interface beside the RM RPCs (615 on)
//
// A GMCAPI record is an NVDM payload of its own type (`NVDM_GMCAPI`): a 40-byte
// header and the command's data (`gmcapi_base.h`, `dev/gsp-615.md` section 3):
//
//   0  command   id in 23:0 (category << 16 | number), flags in 31:24, of
//                which bit 24 says "this is the response"
//   4  size      bytes of data after the header
//   8  sequence  u64; a response carries its request's
//   16 request: the most the caller will take back; response: NV_STATUS
//   20 reserved[5], zero
//
// Ids at or below 0xFFFF are reserved so they never collide with an RM RPC's
// function number, so the transport hands both out as one
// `RpcMessage::function`. The commands GSP-RM sends the CPU are all fire and
// forget (`gmcapi_impl.c`), and none is sent to a GB20x in a normal boot; they
// replaced Turing's CPU sequencer.

use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::le;

use super::rpc::{GspRpc, QueueMem, RpcError, RpcMessage};

/// Header size (`ct_assert(sizeof(GMCAPI_HEADER) == 40)`)
pub const HEADER: usize = 40;
/// `GMCAPI_HEADER_COMMAND_FLAGS_RESPONSE`
pub const FLAG_RESPONSE: u32 = 0x0100_0000;
const ID_MASK: u32 = 0x00FF_FFFF;

/// `GMCAPI_ID(GSP_MGMT, n)`
const fn gsp_mgmt(n: u32) -> u32 {
    0x01 << 16 | n
}

/// System info in, static info out: the first thing said to a GSP-RM just come
/// up (`kgspSendInitRpcs_IMPL`)
pub const GSP_INIT: u32 = gsp_mgmt(0x0001);
/// GSP-RM asks the CPU to run a generic bootloader (Turing, Ampere)
pub const EXEC_GENERIC_BOOTLOADER: u32 = gsp_mgmt(0x0002);
/// GSP-RM asks the CPU to run an HS binary (Turing, Ampere)
pub const EXEC_HS_BINARY: u32 = gsp_mgmt(0x0003);
/// The unload: GSP-RM saves what it must and parks (`kgspUnloadRm_IMPL`)
pub const GSP_SUSPEND: u32 = gsp_mgmt(0x0004);
/// GSP-RM has resumed
pub const GSP_RESUME_DONE: u32 = gsp_mgmt(0x0005);

/// The most `GSP_INIT`'s answer may be (`maxStaticInfoDataSize`)
pub const GSP_INIT_MAX_RESPONSE: u32 = 48 * 1024;

/// A command's name, for the log
pub fn name(id: u32) -> &'static str {
    match id {
        GSP_INIT => "GSP_INIT",
        EXEC_GENERIC_BOOTLOADER => "EXEC_GENERIC_BOOTLOADER",
        EXEC_HS_BINARY => "EXEC_HS_BINARY",
        GSP_SUSPEND => "GSP_SUSPEND",
        GSP_RESUME_DONE => "GSP_RESUME_DONE",
        _ => "?",
    }
}

/// `GSP_SUSPEND` carries one u64 of flags (`GmcApiGspSuspendParams`,
/// `gmcapi_gsp.h`): PM_TRANSITION, GC6_ENTERING, HIBERNATE, FAST, FORCE in bits
/// 0-4. A plain unload sets none

/// The header of an outbound request
pub fn request(command: u32, size: u32, sequence: u32, max_response: u32) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    le::put_u32(&mut h, 0, command);
    le::put_u32(&mut h, 4, size);
    le::put_u64(&mut h, 8, sequence as u64);
    le::put_u32(&mut h, 16, max_response);
    h
}

/// An inbound header
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub command: u32,
    pub size: u32,
    pub sequence: u64,
    /// A response's NV_STATUS, a request's `max_response_size`
    pub status_or_max: u32,
}

impl Header {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < HEADER {
            return None;
        }
        Some(Self {
            command: le::u32_at(b, 0),
            size: le::u32_at(b, 4),
            sequence: le::u64_at(b, 8),
            status_or_max: le::u32_at(b, 16),
        })
    }

    pub fn id(&self) -> u32 {
        self.command & ID_MASK
    }

    pub fn is_response(&self) -> bool {
        self.command & FLAG_RESPONSE != 0
    }
}

/// Why a response is not an answer
#[derive(Debug, Clone, Copy)]
pub enum GmcError {
    /// GSP-RM answered with this NV_STATUS
    Status(u32),
    /// More data than the request said it would take
    TooLong(usize),
}

/// The data of a command's response, once it is known to be one
/// (`gpuSendGmcRpc_KERNEL` after its poll): a status of zero, and no more
/// than the request's `max_response`
pub fn response_data(reply: RpcMessage, max_response: u32) -> Result<Vec<u8>, GmcError> {
    if reply.rpc_result != 0 {
        return Err(GmcError::Status(reply.rpc_result));
    }
    if reply.payload.len() > max_response as usize {
        return Err(GmcError::TooLong(reply.payload.len()));
    }
    Ok(reply.payload)
}

impl<Q: QueueMem> GspRpc<'_, Q> {
    /// Send `command` and do not wait: GMCAPI_FLAGS_FIRE_AND_FORGET commands
    /// get no response
    pub fn gmcapi_post(&mut self, command: u32, data: &[u8], timeout_ns: u64) -> Result<(), RpcError> {
        self.send_gmcapi(command, data, 0, timeout_ns).map(|_| ())
    }
}
