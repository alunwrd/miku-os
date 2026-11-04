// GSP-RM's transport: the rings it shares with the host, and what goes on them
//
// 615's MSGQ 2.0 (`dev/gsp-615.md`, sections 1-3). A record is MCTP-framed and
// carries one NVDM payload, an RM RPC or a GMCAPI command:
//
//   +---------------------------------+  slot boundary
//   | "MCTP", MCTP size, MCTP header, |  24 B: the frame
//   | NVDM header, NVDM size, 0       |
//   +---------------------------------+
//   | RPC header, 32 B ("VRPC")       |  version sig length function
//   |   or GMCAPI header, 40 B        |  result result_priv seq gfid
//   +---------------------------------+
//   | payload                         |
//   +---------------------------------+
//
// A record longer than a slot takes as many consecutive slots as its size says;
// an RM RPC longer than a record is several, the later ones
// CONTINUATION_RECORDs. The ring pointers are GSP registers, and writing ours
// is the doorbell.
//
//   fns.rs       function and event numbers, and their names
//   payloads.rs  what the payloads of the events this driver reads mean
//   queue.rs     the rings: framing, sending, receiving one record
//   poll.rs      the one receive path: wait for a reply, dispatch the rest
//   wait.rs      waiting for GSP-RM to say something, and the interrupt's half
//   loopback.rs  two rings in plain memory, for the selftest
//
// `dev/gsp-foundation.md` is the design

mod fns;
mod loopback;
mod payloads;
mod poll;
mod queue;
mod wait;

pub use fns::*;
pub use loopback::Loopback;
pub use payloads::*;
pub use poll::*;
pub use queue::*;
pub use wait::note_msgq_interrupt;

/// Why a message did not make it across.
///
/// The fields are read by `{:?}` only, which `dead_code` does not count: what a
/// log line needs
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum RpcError {
    /// Message would not fit even split into continuation elements
    TooLarge(usize),
    /// The element's RPC length field was outside [RPC_HDR_SIZE, 1 MiB].
    /// Trusting it would size a bogus allocation and advance the msgq read
    /// pointer by the wrong number of pages, desyncing the ring permanently
    BadLength(u32),
    /// GSP never freed enough cmdq pages
    CmdqTimeout,
    /// No reply within the timeout
    RecvTimeout,
    /// The status queue has not been linked (`rx_link`)
    NotLinked,
    /// A record's frame was not MCTP version 1 from NVIDIA with a known magic
    BadFrame { magic: u32, transport: u32, nvdm: u32 },
    /// An RM RPC whose header is not version 3.0 "VRPC"
    BadRpcHeader { version: u32, signature: u32 },
    /// A record of an NVDM type this transport does not carry
    UnknownNvdm(u8),
    /// More stale records in a row than GSP-RM can have left behind
    Stale,
    /// The reply's rpc_result was non-zero
    GspStatus { function: u32, status: u32 },
    /// The reply arrived intact but is smaller than its struct
    ShortPayload,
    /// A poll was started while another was waiting, from an event handler,
    /// which must record and return (`poll.rs`)
    NestedPoll,
    /// GSP-RM is dead (`NV_ERR_RESET_REQUIRED`); nothing goes out until the GPU
    /// is reset (`kgsp::health`)
    ResetRequired,
    /// The GPU is gone from the bus (`NV_ERR_GPU_IS_LOST`)
    GpuLost,
}

/// Name for an `rpc_result` from the 0xFF1000xx block
/// (`NV_VGPU_MSG_RESULT_RPC_*` in OpenRM's `rpc_headers.h`).
///
/// Transport-level verdicts, not NV_STATUS: the message never reached RM's
/// logic, usually a mistake on our side of the wire
pub fn rpc_result_name(result: u32) -> &'static str {
    match result {
        0xFF10_0001 => "RPC_UNKNOWN_FUNCTION",
        // Almost always a request whose payload is not the size GSP-RM expects
        0xFF10_0002 => "RPC_INVALID_MESSAGE_FORMAT",
        0xFF10_0003 => "RPC_HANDLE_NOT_FOUND",
        0xFF10_0004 => "RPC_HANDLE_EXISTS",
        0xFF10_0005 => "RPC_UNKNOWN_RM_ERROR",
        0xFF10_0006 => "RPC_UNKNOWN_VMIOP_ERROR",
        0xFF10_0007 => "RPC_RESERVED_HANDLE",
        0xFF10_0008 => "RPC_CUDA_PROFILING_DISABLED",
        0xFF10_0009 => "RPC_API_CONTROL_NOT_SUPPORTED",
        0xFFFF_FFFF => "RPC_PENDING",
        _ => "",
    }
}

/// Format an `rpc_result` as name plus value, or bare value if unknown
pub fn rpc_result_str(result: u32) -> alloc::string::String {
    let name = rpc_result_name(result);
    if name.is_empty() {
        alloc::format!("{:#010x}", result)
    } else {
        alloc::format!("{} ({:#010x})", name, result)
    }
}
