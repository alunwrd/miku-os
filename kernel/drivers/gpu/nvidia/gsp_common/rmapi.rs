// RM object layer: RmAlloc / RmControl / RmFree proxied to GSP-RM over the
// message queue
//
// Everything a booted GSP-RM does is reached through two calls. RmAlloc creates
// an object of some class under a parent, RmControl invokes a command on an
// existing object.
//
// The object graph starts as a fixed chain, the one every NVIDIA driver builds
// first:
//
//     NV01_ROOT (client)
//       └── NV01_DEVICE_0 (device)
//             └── NV20_SUBDEVICE_0 (subdevice)
//
// Most NV2080_CTRL_* commands are addressed to the subdevice, so bootstrap()
// comes first.
//
// Wire format is `rpc_gsp_rm_alloc_v03_00` / `rpc_gsp_rm_control_v03_00` from
// OpenRM's g_rpc-structures.h: a fixed header followed by the class- or
// command-specific parameter block, which GSP-RM writes back in place. The
// reply's `status` field is an NV_STATUS, distinct from the RPC transport's
// result: a call can be delivered and still fail.

use crate::drivers::gpu::nvidia::le;
use alloc::vec::Vec;

use super::rpc::{GspRpc, RpcError};

/// NV_VGPU_MSG_FUNCTION_* ids (OpenRM `gsp_abi_check.c`, which pins each to a
/// literal at build time)
pub const FN_FREE: u32 = 10;
pub const FN_GSP_RM_CONTROL: u32 = 76;
pub const FN_GSP_RM_ALLOC: u32 = 103;

/// Class ids for the bootstrap chain
pub const CLASS_ROOT: u32 = 0x0000; // NV01_ROOT, cl0000.h
pub const CLASS_DEVICE_0: u32 = 0x0080; // NV01_DEVICE_0, cl0080.h
pub const CLASS_SUBDEVICE_0: u32 = 0x2080; // NV20_SUBDEVICE_0, cl2080.h

/// Handles. Arbitrary, unique within the client, following nouveau's convention
/// so traces read alike
pub const H_CLIENT: u32 = 0xc1d0_0000;
pub const H_DEVICE: u32 = 0xde1d_0000;
pub const H_SUBDEVICE: u32 = 0x5d1d_0000;

/// Header sizes, checked against the C structs: `rpc_gsp_rm_alloc_v03_00` is 32
/// bytes, `rpc_gsp_rm_control_v03_00` 40 in 615 (the flags word became
/// `rmapiRpcFlags`, `rmctrlFlags`, `rmctrlAccessRight` and a reserved u64,
/// `dev/gsp-615.md` section 7)
const ALLOC_HDR: usize = 32;
const CONTROL_HDR: usize = 40;

/// Timeout for a single RM call: what `_kgspRpcRecvPoll` allows, 1.5x the GPU's
/// default timeout, which `osGetTimeoutParams` makes 4 s in graphics mode: 6 s.
/// A core-channel allocation on a reloaded GSP-RM can take longer than 2 s
const CALL_TIMEOUT_NS: u64 = 6_000_000_000;

/// Payloads are read through `Debug` when the error is printed
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum RmError {
    /// The RPC itself failed: never delivered, or the reply never came
    Transport(RpcError),
    /// Delivered and answered, but GSP-RM returned a non-zero NV_STATUS
    Status(u32),
    /// The reply was shorter than its own header claims
    ShortReply,
    /// This driver already has a live object under that handle.
    ///
    /// Caught before the RPC: RM's answer is `NV_ERR_INSERT_DUPLICATE_NAME` on
    /// the allocation, when what is wrong is that something earlier did not
    /// free
    HandleInUse { handle: u32, class: u32 },
    /// The parent named for this object is not one this driver allocated.
    ///
    /// RM would take it (the device parents almost anything), and the object
    /// would lack what its real parent gives it
    NoSuchParent { handle: u32, parent: u32 },
    /// A handle passed where an object of a different class belongs. An RPC
    /// carries a number, so RM refuses for another reason, or accepts
    WrongClass { handle: u32, want: u32, got: u32 },
}

impl From<RpcError> for RmError {
    fn from(e: RpcError) -> Self {
        RmError::Transport(e)
    }
}

/// Human-readable form for an NV_STATUS, generated from `nvstatuscodes.h` in
/// the 615.78.08 package. The list is alphabetical by name, so neighbouring
/// numbers are unrelated
pub fn status_name(status: u32) -> &'static str {
    match status {
        0x0000_0000 => "NV_OK",
        0x0000_ffff => "NV_ERR_GENERIC",
        0x0000_0001 => "NV_ERR_BROKEN_FB",
        0x0000_0002 => "NV_ERR_BUFFER_TOO_SMALL",
        0x0000_0003 => "NV_ERR_BUSY_RETRY",
        0x0000_0004 => "NV_ERR_CALLBACK_NOT_SCHEDULED",
        0x0000_0005 => "NV_ERR_CARD_NOT_PRESENT",
        0x0000_0006 => "NV_ERR_CYCLE_DETECTED",
        0x0000_0007 => "NV_ERR_DMA_IN_USE",
        0x0000_0008 => "NV_ERR_DMA_MEM_NOT_LOCKED",
        0x0000_0009 => "NV_ERR_DMA_MEM_NOT_UNLOCKED",
        0x0000_000a => "NV_ERR_DUAL_LINK_INUSE",
        0x0000_000b => "NV_ERR_ECC_ERROR",
        0x0000_000c => "NV_ERR_FIFO_BAD_ACCESS",
        0x0000_000d => "NV_ERR_FREQ_NOT_SUPPORTED",
        0x0000_000e => "NV_ERR_GPU_DMA_NOT_INITIALIZED",
        0x0000_000f => "NV_ERR_GPU_IS_LOST",
        0x0000_0010 => "NV_ERR_GPU_IN_FULLCHIP_RESET",
        0x0000_0011 => "NV_ERR_GPU_NOT_FULL_POWER",
        0x0000_0012 => "NV_ERR_GPU_UUID_NOT_FOUND",
        0x0000_0013 => "NV_ERR_HOT_SWITCH",
        0x0000_0014 => "NV_ERR_I2C_ERROR",
        0x0000_0015 => "NV_ERR_I2C_SPEED_TOO_HIGH",
        0x0000_0016 => "NV_ERR_ILLEGAL_ACTION",
        0x0000_0017 => "NV_ERR_IN_USE",
        0x0000_0018 => "NV_ERR_INFLATE_COMPRESSED_DATA_FAILED",
        0x0000_0019 => "NV_ERR_INSERT_DUPLICATE_NAME",
        0x0000_001a => "NV_ERR_INSUFFICIENT_RESOURCES",
        0x0000_001b => "NV_ERR_INSUFFICIENT_PERMISSIONS",
        0x0000_001c => "NV_ERR_INSUFFICIENT_POWER",
        0x0000_001d => "NV_ERR_INVALID_ACCESS_TYPE",
        0x0000_001e => "NV_ERR_INVALID_ADDRESS",
        0x0000_001f => "NV_ERR_INVALID_ARGUMENT",
        0x0000_0020 => "NV_ERR_INVALID_BASE",
        0x0000_0021 => "NV_ERR_INVALID_CHANNEL",
        0x0000_0022 => "NV_ERR_INVALID_CLASS",
        0x0000_0023 => "NV_ERR_INVALID_CLIENT",
        0x0000_0024 => "NV_ERR_INVALID_COMMAND",
        0x0000_0025 => "NV_ERR_INVALID_DATA",
        0x0000_0026 => "NV_ERR_INVALID_DEVICE",
        0x0000_0027 => "NV_ERR_INVALID_DMA_SPECIFIER",
        0x0000_0028 => "NV_ERR_INVALID_EVENT",
        0x0000_0029 => "NV_ERR_INVALID_FLAGS",
        0x0000_002a => "NV_ERR_INVALID_FUNCTION",
        0x0000_002b => "NV_ERR_INVALID_HEAP",
        0x0000_002c => "NV_ERR_INVALID_INDEX",
        0x0000_002d => "NV_ERR_INVALID_IRQ_LEVEL",
        0x0000_002e => "NV_ERR_INVALID_LIMIT",
        0x0000_002f => "NV_ERR_INVALID_LOCK_STATE",
        0x0000_0030 => "NV_ERR_INVALID_METHOD",
        0x0000_0031 => "NV_ERR_INVALID_OBJECT",
        0x0000_0032 => "NV_ERR_INVALID_OBJECT_BUFFER",
        0x0000_0033 => "NV_ERR_INVALID_OBJECT_HANDLE",
        0x0000_0034 => "NV_ERR_INVALID_OBJECT_NEW",
        0x0000_0035 => "NV_ERR_INVALID_OBJECT_OLD",
        0x0000_0036 => "NV_ERR_INVALID_OBJECT_PARENT",
        0x0000_0037 => "NV_ERR_INVALID_OFFSET",
        0x0000_0038 => "NV_ERR_INVALID_OPERATION",
        0x0000_0039 => "NV_ERR_INVALID_OWNER",
        0x0000_003a => "NV_ERR_INVALID_PARAM_STRUCT",
        0x0000_003b => "NV_ERR_INVALID_PARAMETER",
        0x0000_003c => "NV_ERR_INVALID_PATH",
        0x0000_003d => "NV_ERR_INVALID_POINTER",
        0x0000_003e => "NV_ERR_INVALID_REGISTRY_KEY",
        0x0000_003f => "NV_ERR_INVALID_REQUEST",
        0x0000_0040 => "NV_ERR_INVALID_STATE",
        0x0000_0041 => "NV_ERR_INVALID_STRING_LENGTH",
        0x0000_0042 => "NV_ERR_INVALID_READ",
        0x0000_0043 => "NV_ERR_INVALID_WRITE",
        0x0000_0044 => "NV_ERR_INVALID_XLATE",
        0x0000_0045 => "NV_ERR_IRQ_NOT_FIRING",
        0x0000_0046 => "NV_ERR_IRQ_EDGE_TRIGGERED",
        0x0000_0047 => "NV_ERR_MEMORY_TRAINING_FAILED",
        0x0000_0048 => "NV_ERR_MISMATCHED_SLAVE",
        0x0000_0049 => "NV_ERR_MISMATCHED_TARGET",
        0x0000_004a => "NV_ERR_MISSING_TABLE_ENTRY",
        0x0000_004b => "NV_ERR_MODULE_LOAD_FAILED",
        0x0000_004c => "NV_ERR_MORE_DATA_AVAILABLE",
        0x0000_004d => "NV_ERR_MORE_PROCESSING_REQUIRED",
        0x0000_004e => "NV_ERR_MULTIPLE_MEMORY_TYPES",
        0x0000_004f => "NV_ERR_NO_FREE_FIFOS",
        0x0000_0050 => "NV_ERR_NO_INTR_PENDING",
        0x0000_0051 => "NV_ERR_NO_MEMORY",
        0x0000_0052 => "NV_ERR_NO_SUCH_DOMAIN",
        0x0000_0053 => "NV_ERR_NO_VALID_PATH",
        0x0000_0054 => "NV_ERR_NOT_COMPATIBLE",
        0x0000_0055 => "NV_ERR_NOT_READY",
        0x0000_0056 => "NV_ERR_NOT_SUPPORTED",
        0x0000_0057 => "NV_ERR_OBJECT_NOT_FOUND",
        0x0000_0058 => "NV_ERR_OBJECT_TYPE_MISMATCH",
        0x0000_0059 => "NV_ERR_OPERATING_SYSTEM",
        0x0000_005a => "NV_ERR_OTHER_DEVICE_FOUND",
        0x0000_005b => "NV_ERR_OUT_OF_RANGE",
        0x0000_005c => "NV_ERR_OVERLAPPING_UVM_COMMIT",
        0x0000_005d => "NV_ERR_PAGE_TABLE_NOT_AVAIL",
        0x0000_005e => "NV_ERR_PID_NOT_FOUND",
        0x0000_005f => "NV_ERR_PROTECTION_FAULT",
        0x0000_0060 => "NV_ERR_RC_ERROR",
        0x0000_0061 => "NV_ERR_REJECTED_VBIOS",
        0x0000_0062 => "NV_ERR_RESET_REQUIRED",
        0x0000_0063 => "NV_ERR_STATE_IN_USE",
        0x0000_0064 => "NV_ERR_SIGNAL_PENDING",
        0x0000_0065 => "NV_ERR_TIMEOUT",
        0x0000_0066 => "NV_ERR_TIMEOUT_RETRY",
        0x0000_0067 => "NV_ERR_TOO_MANY_PRIMARIES",
        0x0000_0068 => "NV_ERR_UVM_ADDRESS_IN_USE",
        0x0000_0069 => "NV_ERR_MAX_SESSION_LIMIT_REACHED",
        0x0000_006a => "NV_ERR_LIB_RM_VERSION_MISMATCH",
        0x0000_006b => "NV_ERR_PRIV_SEC_VIOLATION",
        0x0000_006c => "NV_ERR_GPU_IN_DEBUG_MODE",
        0x0000_006d => "NV_ERR_FEATURE_NOT_ENABLED",
        0x0000_006e => "NV_ERR_RESOURCE_LOST",
        0x0000_006f => "NV_ERR_PMU_NOT_READY",
        0x0000_0070 => "NV_ERR_FLCN_ERROR",
        0x0000_0071 => "NV_ERR_FATAL_ERROR",
        0x0000_0072 => "NV_ERR_MEMORY_ERROR",
        0x0000_0073 => "NV_ERR_INVALID_LICENSE",
        0x0000_0074 => "NV_ERR_NVLINK_INIT_ERROR",
        0x0000_0075 => "NV_ERR_NVLINK_MINION_ERROR",
        0x0000_0076 => "NV_ERR_NVLINK_CLOCK_ERROR",
        0x0000_0077 => "NV_ERR_NVLINK_TRAINING_ERROR",
        0x0000_0078 => "NV_ERR_NVLINK_CONFIGURATION_ERROR",
        0x0000_0079 => "NV_ERR_RISCV_ERROR",
        0x0000_007a => "NV_ERR_FABRIC_MANAGER_NOT_PRESENT",
        0x0000_007b => "NV_ERR_ALREADY_SIGNALLED",
        0x0000_007c => "NV_ERR_QUEUE_TASK_SLOT_NOT_AVAILABLE",
        0x0000_007d => "NV_ERR_KEY_ROTATION_IN_PROGRESS",
        0x0000_007e => "NV_ERR_TEST_ONLY_CODE_NOT_ENABLED",
        0x0000_007f => "NV_ERR_SECURE_BOOT_FAILED",
        0x0000_0080 => "NV_ERR_INSUFFICIENT_ZBC_ENTRY",
        0x0000_0081 => "NV_ERR_NVLINK_FABRIC_NOT_READY",
        0x0000_0082 => "NV_ERR_NVLINK_FABRIC_FAILURE",
        0x0000_0083 => "NV_ERR_GPU_MEMORY_ONLINING_FAILURE",
        0x0000_0084 => "NV_ERR_REDUCTION_MANAGER_NOT_AVAILABLE",
        0x0000_0085 => "NV_ERR_THRESHOLD_CROSSED",
        0x0000_0086 => "NV_ERR_RESOURCE_RETIREMENT_ERROR",
        0x0000_0087 => "NV_ERR_FABRIC_STATE_OUT_OF_SYNC",
        0x0000_0088 => "NV_ERR_BUFFER_FULL",
        0x0000_0089 => "NV_ERR_BUFFER_EMPTY",
        0x0000_008a => "NV_ERR_MC_FLA_OFFSET_TABLE_FULL",
        0x0000_008b => "NV_ERR_OPERATION_ABORTED",
        0x0000_008c => "NV_ERR_DMA_XFER_FAILED",
        0x0000_008d => "NV_ERR_RESOURCE_ACCOUNTING_HARD_LIMIT_EXCEEDED",
        0x0000_008e => "NV_ERR_NVLINK_FW_NOT_LOADED",
        0x0000_008f => "NV_ERR_INTR_STUCK",
        0x0000_0090 => "NV_ERR_GMCAPI_INVALID_MCTP",
        0x0000_0091 => "NV_ERR_GMCAPI_INVALID_NVDM_TYPE",
        0x0000_0092 => "NV_ERR_GMCAPI_INVALID_COMMAND_ID",
        0x0000_0093 => "NV_ERR_GMCAPI_REQUEST_SIZE_TOO_LARGE",
        0x0000_0094 => "NV_ERR_GMCAPI_REQUEST_SIZE_TOO_SMALL",
        0x0000_0095 => "NV_ERR_GMCAPI_MAX_RESPONSE_SIZE_TOO_SMALL",
        0x0000_0096 => "NV_ERR_GMCAPI_RESERVED_FIELD_NOT_ZERO",
        0x0000_0097 => "NV_ERR_GMCAPI_INVALID_FLAGS",
        0x0000_009e => "NV_ERR_GMCAPI_INTERNAL_RM_ERROR",
        0x0000_009f => "NV_ERR_GMCAPI_GENERIC_ERROR",
        0x0001_0001 => "NV_WARN_HOT_SWITCH",
        0x0001_0002 => "NV_WARN_INCORRECT_PERFMON_DATA",
        0x0001_0003 => "NV_WARN_MISMATCHED_SLAVE",
        0x0001_0004 => "NV_WARN_MISMATCHED_TARGET",
        0x0001_0005 => "NV_WARN_MORE_PROCESSING_REQUIRED",
        0x0001_0006 => "NV_WARN_NOTHING_TO_DO",
        0x0001_0007 => "NV_WARN_NULL_OBJECT",
        0x0001_0008 => "NV_WARN_OUT_OF_RANGE",
        0x0001_0009 => "NV_WARN_THRESHOLD_CROSSED",
        0x0001_000a => "NV_WARN_RESOURCE_ACCOUNTING_SOFT_LIMIT_EXCEEDED",
        _ => "",
    }
}

/// Format a status for a one-line message: the name if known, else the raw
/// value
pub fn status_str(status: u32) -> alloc::string::String {
    let name = status_name(status);
    if name.is_empty() {
        alloc::format!("{:#x}", status)
    } else {
        alloc::format!("{} ({:#x})", name, status)
    }
}

/// `NV01_EVENT_KERNEL_CALLBACK_EX`. "Kernel callback" is RM's name; on the GSP
/// path the notification comes back as an RPC
pub const CLASS_EVENT_KERNEL_CALLBACK_EX: u32 = 0x0000_007e;

/// `NV01_EVENT_CLIENT_RM`: the bit saying this notifier is one of RM's own
pub const NV01_EVENT_CLIENT_RM: u32 = 0x0400_0000;

/// `NV2080_CTRL_CMD_EVENT_SET_NOTIFICATION`
pub const NV2080_CTRL_CMD_EVENT_SET_NOTIFICATION: u32 = 0x2080_0301;

/// `..._ACTION_REPEAT`: keep telling us, not once and then silence
pub const EVENT_SET_NOTIFICATION_ACTION_REPEAT: u32 = 2;

/// A client and the fixed object chain hanging off it
#[derive(Clone, Copy, Debug)]
pub struct RmClient {
    pub h_client: u32,
    pub h_device: u32,
    pub h_subdevice: u32,
}

/// `NV0000_ALLOC_PARAMETERS`, 120 bytes (confirmed by compiling the C struct).
/// `hClient` is the first member and equals the handle being allocated, as RM
/// comments say (CORERM-2934)
fn root_params(h_client: u32) -> Vec<u8> {
    let mut p = alloc::vec![0u8; 120];
    le::put_u32(&mut p, 0, h_client);
    // processID 0 and an empty processName are what a kernel-side client sends
    p
}

/// `NV0080_ALLOC_PARAMETERS`, 56 bytes. Every field zero is the plain "this
/// GPU, default VA space" device nouveau allocates
fn device_params(device_id: u32) -> Vec<u8> {
    let mut p = alloc::vec![0u8; 56];
    le::put_u32(&mut p, 0, device_id);
    p
}

/// `NV2080_ALLOC_PARAMETERS`, 4 bytes: which subdevice of the device. Always 0
/// on a single-GPU board
fn subdevice_params(sub_device_id: u32) -> Vec<u8> {
    sub_device_id.to_le_bytes().to_vec()
}

// The object tree

/// One live RM object: what it is, and whose child it is.
///
/// The smallest version of OpenRM's resource server
/// (`refsrc/openrm/src/nvidia/src/libraries/resserv`): every object has a class
/// and a parent, freeing a parent frees its children, and a handle is looked
/// up. RM enforces this on its side; here it catches a bring-up that did not
/// free its channel, a freed channel with a leaked group, the order "engine
/// object, then the channel it hangs off", and a channel with no timeslice
/// group.
///
/// A `Vec`: tens of objects for the life of the machine
#[derive(Clone, Copy)]
struct Obj {
    client: u32,
    handle: u32,
    parent: u32,
    class: u32,
}

static TREE: spin::Mutex<Vec<Obj>> = spin::Mutex::new(Vec::new());

/// What every object of this class is called in a listing
fn class_name(class: u32) -> &'static str {
    match class {
        CLASS_ROOT => "client",
        CLASS_DEVICE_0 => "device",
        CLASS_SUBDEVICE_0 => "subdevice",
        CLASS_EVENT_KERNEL_CALLBACK_EX => "event",
        0x0000_90f1 => "vaspace",
        0x0000_a06c => "channel group",
        0x0000_ca6f => "channel",
        0x0000_cab5 => "copy engine",
        0x0000_cec0 => "compute",
        0x0000_0073 => "display common",
        _ => "",
    }
}

fn record(client: u32, handle: u32, parent: u32, class: u32) {
    TREE.lock().push(Obj { client, handle, parent, class });
}

fn forget_obj(client: u32, handle: u32) {
    TREE.lock().retain(|o| !(o.client == client && o.handle == handle));
}

fn live(client: u32, handle: u32) -> Option<Obj> {
    TREE.lock().iter().find(|o| o.client == client && o.handle == handle).copied()
}

fn children_of(client: u32, handle: u32) -> Vec<u32> {
    TREE.lock()
        .iter()
        .filter(|o| o.client == client && o.parent == handle && o.handle != handle)
        .map(|o| o.handle)
        .collect()
}

/// Every live object, deepest last, as `depth, handle, class`, for
/// `nvidia rm-tree`
pub fn tree_lines() -> Vec<alloc::string::String> {
    use core::fmt::Write;
    let objs: Vec<Obj> = TREE.lock().clone();
    let mut out = Vec::new();
    // Roots first: an object whose parent is itself, or not in the tree, the
    // client
    let mut stack: Vec<(u32, usize)> = objs
        .iter()
        .filter(|o| o.parent == o.handle)
        .map(|o| (o.handle, 0usize))
        .collect();
    stack.reverse();
    while let Some((h, depth)) = stack.pop() {
        for o in objs.iter().filter(|o| o.handle == h) {
            let mut line = alloc::string::String::new();
            for _ in 0..depth {
                line.push_str("  ");
            }
            let _ = write!(line, "{:#010x} class {:#06x} {}", o.handle, o.class, class_name(o.class));
            out.push(line);
            let kids: Vec<u32> = objs
                .iter()
                .filter(|k| k.client == o.client && k.parent == h && k.handle != h)
                .map(|k| k.handle)
                .collect();
            for k in kids.into_iter().rev() {
                stack.push((k, depth + 1));
            }
        }
    }
    out
}

/// How many objects RM is holding for us, for the teardown test
pub fn object_count() -> usize {
    TREE.lock().len()
}

impl RmClient {
    /// Build the client -> device -> subdevice chain. Call once GSP_INIT has
    /// been answered; every later call needs the handles returned
    pub fn bootstrap(rpc: &mut GspRpc) -> Result<Self, RmError> {
        // A new client chain means a new RM: GSP-RM has just booted and
        // remembers nothing, so neither may this tree. Otherwise a second
        // `gb206-boot` is refused every allocation as a duplicate
        TREE.lock().clear();

        // The root is its own parent: there is nothing above a client
        raw_alloc(rpc, H_CLIENT, H_CLIENT, H_CLIENT, CLASS_ROOT, &root_params(H_CLIENT))?;
        record(H_CLIENT, H_CLIENT, H_CLIENT, CLASS_ROOT);
        raw_alloc(rpc, H_CLIENT, H_CLIENT, H_DEVICE, CLASS_DEVICE_0, &device_params(0))?;
        record(H_CLIENT, H_DEVICE, H_CLIENT, CLASS_DEVICE_0);
        raw_alloc(
            rpc, H_CLIENT, H_DEVICE, H_SUBDEVICE, CLASS_SUBDEVICE_0, &subdevice_params(0),
        )?;
        record(H_CLIENT, H_SUBDEVICE, H_DEVICE, CLASS_SUBDEVICE_0);
        Ok(RmClient {
            h_client: H_CLIENT,
            h_device: H_DEVICE,
            h_subdevice: H_SUBDEVICE,
        })
    }

    /// Allocate an object of `h_class` under `h_parent`, and record it.
    ///
    /// Two checks before the RPC:
    ///
    ///   - a handle already live comes back as `NV_ERR_INSERT_DUPLICATE_NAME`,
    ///     reading as "this allocation is wrong" when something earlier did not
    ///     free. Refusing here names the object holding it.
    ///   - a parent not in the tree, as a channel with the device for a parent
    ///     and no timeslice group. The client is its own root and may parent
    ///     anything
    pub fn alloc(
        &self,
        rpc: &mut GspRpc,
        h_parent: u32,
        h_object: u32,
        h_class: u32,
        params: &[u8],
    ) -> Result<(), RmError> {
        if let Some(o) = live(self.h_client, h_object) {
            crate::kerror!("[rm] alloc {} as {:#x}: the handle is still live as {} - something did not free it",
                class_str(h_class), h_object, class_str(o.class));
            return Err(RmError::HandleInUse { handle: h_object, class: o.class });
        }
        if h_parent != self.h_client && live(self.h_client, h_parent).is_none() {
            crate::kerror!("[rm] alloc {} as {:#x}: parent {:#x} is not an object this driver holds",
                class_str(h_class), h_object, h_parent);
            return Err(RmError::NoSuchParent { handle: h_object, parent: h_parent });
        }
        raw_alloc(rpc, self.h_client, h_parent, h_object, h_class, params)?;
        record(self.h_client, h_object, h_parent, h_class);
        Ok(())
    }

    /// What class an object was allocated as, if it is still live
    pub fn class_of(&self, h_object: u32) -> Option<u32> {
        live(self.h_client, h_object).map(|o| o.class)
    }

    /// Invoke `cmd` on `h_object`. `params` goes in and the returned buffer is
    /// what GSP-RM wrote back: most NV*_CTRL_* commands use one buffer both
    /// ways, so pass a zeroed block of the right size for a query
    pub fn control(
        &self,
        rpc: &mut GspRpc,
        h_object: u32,
        cmd: u32,
        params: &[u8],
    ) -> Result<Vec<u8>, RmError> {
        let (status, out) = self.control_status(rpc, h_object, cmd, params)?;
        if status != 0 {
            return Err(RmError::Status(status));
        }
        Ok(out)
    }

    /// `control`, with RM's status handed back beside the parameter block, for
    /// the controls saying why they failed in the block:
    /// `NV0073_CTRL_CMD_DP_CTRL` fills `err` with the training phase that
    /// broke, and the fallback is chosen from it. Transport failures are still
    /// errors
    pub fn control_status(
        &self,
        rpc: &mut GspRpc,
        h_object: u32,
        cmd: u32,
        params: &[u8],
    ) -> Result<(u32, Vec<u8>), RmError> {
        let mut msg = Vec::with_capacity(CONTROL_HDR + params.len());
        msg.extend_from_slice(&self.h_client.to_le_bytes());
        msg.extend_from_slice(&h_object.to_le_bytes());
        msg.extend_from_slice(&cmd.to_le_bytes());
        msg.extend_from_slice(&0u32.to_le_bytes()); // status, filled by GSP-RM
        msg.extend_from_slice(&(params.len() as u32).to_le_bytes());
        // rmapiRpcFlags: neither serialized (every control here is a flat
        // struct) nor copy-out-on-error: the reply carries the parameters
        // either way, and `control_status` hands them back with the status.
        // rmctrlFlags and rmctrlAccessRight are GSP-RM's to fill
        msg.extend_from_slice(&0u32.to_le_bytes());
        msg.extend_from_slice(&0u32.to_le_bytes());
        msg.extend_from_slice(&0u32.to_le_bytes());
        msg.extend_from_slice(&0u64.to_le_bytes());
        debug_assert_eq!(msg.len(), CONTROL_HDR);
        msg.extend_from_slice(params);

        let t0 = crate::kcore::time::Stopwatch::start();
        let reply = match rpc.call(FN_GSP_RM_CONTROL, &msg, CALL_TIMEOUT_NS) {
            Ok(r) => r,
            Err(e) => {
                super::rpc::note_failure(&alloc::format!("control {} on {:#x}", ctrl_str(cmd), h_object),
                    &e, t0.elapsed_us());
                return Err(e.into());
            }
        };
        if reply.payload.len() < CONTROL_HDR {
            crate::kwarn!("[rm] control {} on {:#x}: reply shorter than its header", ctrl_str(cmd), h_object);
            return Err(RmError::ShortReply);
        }
        let status = le::u32_at(&reply.payload, 12);
        if status == 0 {
            crate::kdebug!("[rm] control {} on {:#x} ({} B) ok in {} us",
                ctrl_str(cmd), h_object, params.len(), t0.elapsed_us());
        } else {
            // Said even when the caller expected it and moves on, as DP
            // training falls back on this
            crate::kwarn!("[rm] control {} on {:#x}: {}", ctrl_str(cmd), h_object, status_str(status));
        }
        Ok((status, reply.payload[CONTROL_HDR..].to_vec()))
    }

    /// Convenience wrapper for the common case: a query whose parameter block
    /// is `size` bytes of output and no input
    pub fn control_query(
        &self,
        rpc: &mut GspRpc,
        h_object: u32,
        cmd: u32,
        size: usize,
    ) -> Result<Vec<u8>, RmError> {
        self.control(rpc, h_object, cmd, &alloc::vec![0u8; size])
    }

    /// Subscribe to one of RM's own event notifications.
    ///
    /// Two steps, `r535_gsp_device_event_ctor`:
    ///
    /// 1. Allocate an `NV01_EVENT_KERNEL_CALLBACK_EX` under the subdevice,
    ///    naming the notifier in `notifyIndex`: what RM posts to.
    /// 2. `NV2080_CTRL_CMD_EVENT_SET_NOTIFICATION` with `ACTION_REPEAT`, which
    ///    turns the notifier on; REPEAT since a cable can be plugged in more
    ///    than once.
    ///
    /// The notifications arrive as `NV_VGPU_MSG_EVENT_POST_EVENT` on the
    /// ordinary message queue, told apart by the `notifyIndex` they carry.
    ///
    /// `handle` is ours, unique within the client; nouveau uses
    /// `0x007e0000 + n`, matched so traces read alike
    pub fn subscribe_event(
        &self,
        rpc: &mut GspRpc,
        handle: u32,
        notifier: u32,
    ) -> Result<(), RmError> {
        // NV0005_ALLOC_PARAMETERS: 24 bytes, `data` an 8-aligned pointer left
        // null: the message queue is the callback. Confirmed by compiling the C
        let mut p = [0u8; 24];
        le::put_u32(&mut p, 0, self.h_client);   // hParentClient
        le::put_u32(&mut p, 4, 0);               // hSrcResource: the subdevice itself
        le::put_u32(&mut p, 8, CLASS_EVENT_KERNEL_CALLBACK_EX);
        le::put_u32(&mut p, 12, NV01_EVENT_CLIENT_RM | notifier);
        self.alloc(rpc, self.h_subdevice, handle, CLASS_EVENT_KERNEL_CALLBACK_EX, &p)?;

        // NV2080_CTRL_EVENT_SET_NOTIFICATION_PARAMS: 20 bytes: {event, action,
        // bNotifyState, info32, info16}
        let mut c = [0u8; 20];
        le::put_u32(&mut c, 0, notifier);
        le::put_u32(&mut c, 4, EVENT_SET_NOTIFICATION_ACTION_REPEAT);
        self.control(rpc, self.h_subdevice, NV2080_CTRL_CMD_EVENT_SET_NOTIFICATION, &c)?;
        Ok(())
    }

    /// Release an object. Children go with it, so freeing the client tears the
    /// whole chain down
    pub fn free(&self, rpc: &mut GspRpc, h_object: u32) -> Result<(), RmError> {
        // Children first, deepest last. RM frees a parent's children on its
        // side; this keeps our tree true, as `kchannelDestruct` frees the group
        // it allocated.
        //
        // Siblings newest first: something allocated later can lean on
        // something before it (a window or cursor channel on the core channel),
        // and NVKMS frees the display that way: cursors, windows, the core, the
        // display object (`nvFreeCoreChannelEvo`). Freeing the core channel
        // under its windows makes GSP-RM assert on that FREE, and the next
        // GSP-RM's core channel allocation times out after four seconds
        for child in children_of(self.h_client, h_object).into_iter().rev() {
            let _ = self.free(rpc, child);
        }

        // NVOS00_PARAMETERS: hRoot, hObjectParent, hObjectOld, status.
        //
        // `hObjectParent` is sent and never read: `_nv01Free`
        // (`refsrc/openrm/src/nvidia/src/kernel/rmapi/entry_points.c:307`)
        // calls `FreeWithSecInfo(hRoot, hObjectOld)`
        let mut msg = Vec::with_capacity(16);
        msg.extend_from_slice(&self.h_client.to_le_bytes());
        msg.extend_from_slice(&self.h_client.to_le_bytes());
        msg.extend_from_slice(&h_object.to_le_bytes());
        msg.extend_from_slice(&0u32.to_le_bytes());
        let class = live(self.h_client, h_object).map_or(0, |o| o.class);
        let reply = match rpc.call(FN_FREE, &msg, CALL_TIMEOUT_NS) {
            Ok(r) => r,
            Err(e) => {
                super::rpc::note_failure(&alloc::format!("free {:#x} ({})", h_object, class_str(class)), &e, 0);
                return Err(e.into());
            }
        };
        if reply.payload.len() >= 16 {
            let status = le::u32_at(&reply.payload, 12);
            if status != 0 {
                crate::kwarn!("[rm] free {:#x} ({}): {} - still recorded as live",
                    h_object, class_str(class), status_str(status));
                // Still recorded: RM believes in this object, so the handle is
                // not free, and the next allocation of it is refused here and
                // not by RM a round later
                return Err(RmError::Status(status));
            }
        }
        forget_obj(self.h_client, h_object);
        crate::kdebug!("[rm] free {:#x} ({})", h_object, class_str(class));
        Ok(())
    }
}

/// The RmAlloc wire call, without an RmClient: bootstrap() creates the client
/// with it
fn raw_alloc(
    rpc: &mut GspRpc,
    h_client: u32,
    h_parent: u32,
    h_object: u32,
    h_class: u32,
    params: &[u8],
) -> Result<(), RmError> {
    let mut msg = Vec::with_capacity(ALLOC_HDR + params.len());
    msg.extend_from_slice(&h_client.to_le_bytes());
    msg.extend_from_slice(&h_parent.to_le_bytes());
    msg.extend_from_slice(&h_object.to_le_bytes());
    msg.extend_from_slice(&h_class.to_le_bytes());
    msg.extend_from_slice(&0u32.to_le_bytes()); // status, filled by GSP-RM
    msg.extend_from_slice(&(params.len() as u32).to_le_bytes());
    msg.extend_from_slice(&0u32.to_le_bytes()); // flags
    msg.extend_from_slice(&[0u8; 4]); // reserved
    msg.extend_from_slice(params);

    let t0 = crate::kcore::time::Stopwatch::start();
    let reply = match rpc.call(FN_GSP_RM_ALLOC, &msg, CALL_TIMEOUT_NS) {
        Ok(r) => r,
        Err(e) => {
            super::rpc::note_failure(&alloc::format!("alloc {} as {:#x}", class_str(h_class), h_object),
                &e, t0.elapsed_us());
            return Err(e.into());
        }
    };
    if reply.payload.len() < ALLOC_HDR {
        crate::kwarn!("[rm] alloc {} as {:#x}: reply shorter than its header", class_str(h_class), h_object);
        return Err(RmError::ShortReply);
    }
    let status = le::u32_at(&reply.payload, 16);
    if status != 0 {
        crate::kwarn!("[rm] alloc {} as {:#x} under {:#x}: {}",
            class_str(h_class), h_object, h_parent, status_str(status));
        return Err(RmError::Status(status));
    }
    crate::kdebug!("[rm] alloc {} as {:#x} under {:#x} ({} B) in {} us",
        class_str(h_class), h_object, h_parent, params.len(), t0.elapsed_us());
    Ok(())
}

/// A class as the log names it: `NVCA73_DISP_CAPABILITIES (0xca73)`
pub fn class_str(class: u32) -> alloc::string::String {
    match super::rm_names::class(class) {
        Some(n) => alloc::format!("{} ({:#06x})", n, class),
        None => alloc::format!("class {:#06x}", class),
    }
}

/// A control command as the log names it
pub fn ctrl_str(cmd: u32) -> alloc::string::String {
    match super::rm_names::control(cmd) {
        Some(n) => alloc::format!("{} ({:#010x})", n, cmd),
        None => alloc::format!("{:#010x}", cmd),
    }
}

/// NV2080_CTRL_CMD_BUS_GET_PCI_INFO, the first control call the shell makes:
/// the answer is four values known from PCI config space, so a mismatch shows
/// the plumbing is wrong
pub const NV2080_CTRL_CMD_BUS_GET_PCI_INFO: u32 = 0x2080_1801;

/// `NV2080_CTRL_BUS_GET_PCI_INFO_PARAMS`, 16 bytes of pure output.
///
/// `device_id` and `subsystem_id` each pack two 16-bit ids: device in the high
/// half, vendor in the low. The header says so for `pciDeviceId` ("the internal
/// PCI device and vendor identifiers"), and `pciExtDeviceId` is the same device
/// id alone, a cross-check on the unpacking
#[derive(Debug, Clone, Copy, Default)]
pub struct PciInfo {
    pub device_id: u32,
    pub subsystem_id: u32,
    pub revision_id: u32,
    pub ext_device_id: u32,
}

impl PciInfo {
    pub const SIZE: usize = 16;

    /// The 16-bit PCI device id, e.g. 0x2d04 for this GB206
    #[inline]
    pub fn device(&self) -> u16 {
        (self.device_id >> 16) as u16
    }

    /// The PCI vendor id, 0x10de for anything this driver talks to
    #[inline]
    pub fn vendor(&self) -> u16 {
        (self.device_id & 0xffff) as u16
    }

    /// Subsystem device id: the board partner's model
    #[inline]
    pub fn subsystem_device(&self) -> u16 {
        (self.subsystem_id >> 16) as u16
    }

    /// Subsystem vendor id, e.g. 0x1043 for ASUS
    #[inline]
    pub fn subsystem_vendor(&self) -> u16 {
        (self.subsystem_id & 0xffff) as u16
    }

    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < Self::SIZE {
            return None;
        }
        Some(PciInfo {
            device_id: le::u32_at(b, 0),
            subsystem_id: le::u32_at(b, 4),
            revision_id: le::u32_at(b, 8),
            ext_device_id: le::u32_at(b, 12),
        })
    }
}
