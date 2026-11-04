// NV_VGPU_MSG_FUNCTION_* and NV_VGPU_MSG_EVENT_*: what a message on the rings
// is, by number

// NV_VGPU_MSG_FUNCTION_* (`rpc_global_enums.h`, 615). GSP_SET_SYSTEM_INFO,
// SET_REGISTRY, GET_GSP_STATIC_INFO, SET_GUEST_SYSTEM_INFO and
// UNLOADING_GUEST_DRIVER are GMCAPI now (`gmcapi.rs`)
pub const FN_CONTINUATION_RECORD: u32 = 71;

// NV_VGPU_MSG_EVENT_*: what GSP-RM sends unasked
// (`rpc_global_enums.h:250-299`). Only those named here get a constant;
// `event_name` knows all. GSP_INIT_DONE and GSP_RUN_CPU_SEQUENCER are numbered,
// and 615 sends neither
pub const EV_FIRST_EVENT: u32 = 0x1000;
pub const EV_POST_EVENT: u32 = 0x1003;
pub const EV_RC_TRIGGERED: u32 = 0x1004;
pub const EV_MMU_FAULT_QUEUED: u32 = 0x1005;
pub const EV_OS_ERROR_LOG: u32 = 0x1006;
pub const EV_GPUACCT_PERFMON_UTIL_SAMPLES: u32 = 0x1008;
pub const EV_SEMAPHORE_SCHEDULE_CALLBACK: u32 = 0x100B;
pub const EV_UCODE_LIBOS_PRINT: u32 = 0x100C;
pub const EV_PERF_GPU_BOOST_SYNC_LIMITS_CALLBACK: u32 = 0x100E;
pub const EV_PERF_BRIDGELESS_INFO_UPDATE: u32 = 0x100F;
pub const EV_TIMED_SEMAPHORE_RELEASE: u32 = 0x1018;
pub const EV_GSP_LOCKDOWN_NOTICE: u32 = 0x101C;
pub const EV_UPDATE_GSP_TRACE: u32 = 0x101E;
pub const EV_GSP_POST_NOCAT_RECORD: u32 = 0x1020;
pub const EV_FECS_ERROR: u32 = 0x1021;
pub const EV_RECOVERY_ACTION: u32 = 0x1022;
pub const EV_PFM_REQ_HNDLR_STATE_SYNC_CALLBACK: u32 = 0x101A;
pub const EV_TRIGGER_BUGCHECK: u32 = 0x1023;
pub const EV_FORCED_DRIVER_SHUTDOWN: u32 = 0x1025;
pub const EV_PMU_HALTED: u32 = 0x1029;
/// `NV_VGPU_MSG_EVENT_NUM_EVENTS`: one past the last event there is
pub const EV_NUM_EVENTS: u32 = 0x102E;

/// Every event's name, `EV_FIRST_EVENT` onward, as `rpc_global_enums.h` spells
/// it, what `_getRpcName` prints
const EVENT_NAMES: [&str; (EV_NUM_EVENTS - EV_FIRST_EVENT) as usize] = [
    "FIRST_EVENT",
    "GSP_INIT_DONE",
    "GSP_RUN_CPU_SEQUENCER",
    "POST_EVENT",
    "RC_TRIGGERED",
    "MMU_FAULT_QUEUED",
    "OS_ERROR_LOG",
    "RG_LINE_INTR",
    "GPUACCT_PERFMON_UTIL_SAMPLES",
    "SIM_READ",
    "SIM_WRITE",
    "SEMAPHORE_SCHEDULE_CALLBACK",
    "UCODE_LIBOS_PRINT",
    "VGPU_GSP_PLUGIN_TRIGGERED",
    "PERF_GPU_BOOST_SYNC_LIMITS_CALLBACK",
    "PERF_BRIDGELESS_INFO_UPDATE",
    "VGPU_CONFIG",
    "DISPLAY_MODESET",
    "EXTDEV_INTR_SERVICE",
    "NVLINK_INBAND_RECEIVED_DATA_256",
    "NVLINK_INBAND_RECEIVED_DATA_512",
    "NVLINK_INBAND_RECEIVED_DATA_1024",
    "NVLINK_INBAND_RECEIVED_DATA_2048",
    "NVLINK_INBAND_RECEIVED_DATA_4096",
    "TIMED_SEMAPHORE_RELEASE",
    "NVLINK_IS_GPU_DEGRADED",
    "PFM_REQ_HNDLR_STATE_SYNC_CALLBACK",
    "NVLINK_FAULT_UP",
    "GSP_LOCKDOWN_NOTICE",
    "MIG_CI_CONFIG_UPDATE",
    "UPDATE_GSP_TRACE",
    "NVLINK_FATAL_ERROR_RECOVERY",
    "GSP_POST_NOCAT_RECORD",
    "FECS_ERROR",
    "RECOVERY_ACTION",
    "TRIGGER_BUGCHECK",
    "BIND_BAR2",
    "FORCED_DRIVER_SHUTDOWN",
    "GSP_LOAD_EXEC_GENERIC_BOOTLOADER",
    "GSP_LOAD_EXEC_HS_BINARY",
    "UPDATE_GRID_DISPLAYLESS_PARAMS",
    "PMU_HALTED",
    "ECC_POSSIBLE_ERR_FAST_PATH",
    "NVLINK_TRAFFIC_QUIESCE_NOTIFY",
    "GPU_GFM_STATE_CHANGE",
    "NVLINK_ABM_FABRIC_HEALTH_MASK_UPDATE",
];

/// An event's name, or "?" for anything that is not an event
pub fn event_name(function: u32) -> &'static str {
    match function.checked_sub(EV_FIRST_EVENT) {
        Some(i) if (i as usize) < EVENT_NAMES.len() => EVENT_NAMES[i as usize],
        _ => "?",
    }
}
