// GSP_SET_SYSTEM_INFO / SET_REGISTRY payloads (r570 line) and the
// post-INIT_DONE RPC sequencer
//
// GspSystemInfo is byte-exact with rm/r570/nvrm/gsp.h: same field order,
// same types (NvBool = u8, NV_STATUS = u32), #[repr(C)] on both sides
// yields the same padding. The compile-time size assert (928 bytes on
// x86_64) guards against transcription slips
//
// The registry is a PACKED_REGISTRY_TABLE with the two DWORD keys nouveau
// always sets ("RMSecBusResetEnable"=1, "RMForcePcieConfigSave"=1)

#![allow(dead_code)]

use alloc::vec::Vec;
use core::mem::size_of;

use crate::drivers::gpu::nvidia::pci::GpuDevice;
use crate::serial_println;

use super::rpc::{GspRpc, RpcError, FN_GSP_SET_SYSTEM_INFO, FN_SET_REGISTRY};

// GspSystemInfo (r570)

pub const ACPI_ID_MAP_MAX_DISPLAYS: usize = 16;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Businfo {
    pub device_id: u16,
    pub vendor_id: u16,
    pub subdevice_id: u16,
    pub subvendor_id: u16,
    pub revision_id: u8,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct DodMethodData {
    pub status: u32,
    pub acpi_id_list_len: u32,
    pub acpi_id_list: [u32; ACPI_ID_MAP_MAX_DISPLAYS],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct JtMethodData {
    pub status: u32,
    pub jt_caps: u32,
    pub jt_rev_id: u16,
    pub b_sbios_caps: u8,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct MuxMethodDataElement {
    pub acpi_id: u32,
    pub mode: u32,
    pub status: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct MuxMethodData {
    pub table_len: u32,
    pub acpi_id_mux_mode_table: [MuxMethodDataElement; ACPI_ID_MAP_MAX_DISPLAYS],
    pub acpi_id_mux_part_table: [MuxMethodDataElement; ACPI_ID_MAP_MAX_DISPLAYS],
    pub acpi_id_mux_state_table: [MuxMethodDataElement; ACPI_ID_MAP_MAX_DISPLAYS],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct CapsMethodData {
    pub status: u32,
    pub optimus_caps: u32,
}

/// All-zero on a machine where the host did not evaluate any ACPI methods
/// (what a CONFIG_ACPI=n Linux kernel sends)
#[repr(C)]
#[derive(Copy, Clone)]
pub struct AcpiMethodData {
    pub b_valid: u8,
    pub dod: DodMethodData,
    pub jt: JtMethodData,
    pub mux: MuxMethodData,
    pub caps: CapsMethodData,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct GspVfInfo {
    pub total_vfs: u32,
    pub first_vf_offset: u32,
    pub first_vf_bar0: u64,
    pub first_vf_bar1: u64,
    pub first_vf_bar2: u64,
    pub b64bit_bar0: u8,
    pub b64bit_bar1: u8,
    pub b64bit_bar2: u8,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct GspSystemInfo {
    pub gpu_phys_addr: u64,
    pub gpu_phys_fb_addr: u64,
    pub gpu_phys_inst_addr: u64,
    pub gpu_phys_io_addr: u64,
    pub nv_domain_bus_device_func: u64,
    pub sim_access_buf_phys_addr: u64,
    pub notify_op_shared_surface_phys_addr: u64,
    pub pcie_atomics_op_mask: u64,
    pub console_mem_size: u64,
    pub max_user_va: u64,
    pub pci_config_mirror_base: u32,
    pub pci_config_mirror_size: u32,
    pub pci_device_id: u32,
    pub pci_sub_device_id: u32,
    pub pci_revision_id: u32,
    pub pcie_atomics_cpl_device_cap_mask: u32,
    pub oor_arch: u8,
    pub cl_pdb_properties: u64,
    pub chipset: u32,
    pub b_gpu_behind_bridge: u8,
    pub b_flr_supported: u8,
    pub b_64b_bar0_supported: u8,
    pub b_mnoc_available: u8,
    pub chipset_l1ss_enable: u32,
    pub b_upstream_l0s_unsupported: u8,
    pub b_upstream_l1_unsupported: u8,
    pub b_upstream_l1_por_supported: u8,
    pub b_upstream_l1_por_mobile_only: u8,
    pub b_system_has_mux: u8,
    pub upstream_address_valid: u8,
    pub fhb_bus_info: Businfo,
    pub chipset_id_info: Businfo,
    pub acpi_method_data: AcpiMethodData,
    pub hypervisor_type: u32,
    pub b_is_passthru: u8,
    pub sys_timer_offset_ns: u64,
    pub gsp_vf_info: GspVfInfo,
    pub b_is_primary: u8,
    pub is_grid_build: u8,
    pub pcie_config_reg_link_cap: u32,
    pub grid_build_csp: u32,
    pub b_preserve_video_memory_allocations: u8,
    pub b_tdr_event_supported: u8,
    pub b_feature_stretch_vblank_capable: u8,
    pub b_enable_dynamic_granularity_page_arrays: u8,
    pub b_clock_boost_supported: u8,
    pub b_route_disp_intrs_to_cpu: u8,
    pub host_page_size: u64,
}

const _: () = assert!(size_of::<GspSystemInfo>() == 928);

/// PCI config-space mirror in BAR0. Turing through Ada: NV_PCFG at
/// 0x88000. Hopper/Blackwell: NV_EP_PCFGM at 0x92000 (gh100
/// dev_xtl_ep_pri.h; nouveau gh100_pci_new .cfg.addr)
pub const PCI_CONFIG_MIRROR_BASE: u32 = 0x0008_8000;
pub const PCI_CONFIG_MIRROR_BASE_GH100: u32 = 0x0009_2000;
pub const PCI_CONFIG_MIRROR_SIZE: u32 = 0x1000;

/// Linux x86_64 TASK_SIZE; GSP-RM only uses this to size internal VA maps
pub const MAX_USER_VA: u64 = 0x0000_7FFF_FFFF_F000;

impl GspSystemInfo {
    /// Mirror of r570_gsp_set_system_info's field fill for a bare-metal
    /// x86 host without ACPI method data. Turing/Ada config-mirror base;
    /// Blackwell callers use for_gpu_gh100()
    pub fn for_gpu(gpu: &GpuDevice) -> Self {
        let mut info: Self = unsafe { core::mem::zeroed() };
        info.gpu_phys_addr = gpu.bars[0].phys;
        info.gpu_phys_fb_addr = gpu.bars[1].phys;
        // BAR3 is the RM "instance" BAR (64-bit BAR pair 3/4); fall back
        // to BAR2 if firmware assigned it there
        info.gpu_phys_inst_addr = if gpu.bars[3].phys != 0 {
            gpu.bars[3].phys
        } else {
            gpu.bars[2].phys
        };
        info.nv_domain_bus_device_func =
            ((gpu.bus as u64) << 8) | ((gpu.dev as u64) << 3) | gpu.func as u64;
        info.max_user_va = MAX_USER_VA;
        info.pci_config_mirror_base = PCI_CONFIG_MIRROR_BASE;
        info.pci_config_mirror_size = PCI_CONFIG_MIRROR_SIZE;
        info.pci_device_id = ((gpu.device as u32) << 16) | gpu.vendor as u32;
        info.pci_sub_device_id =
            ((gpu.subsystem_device as u32) << 16) | gpu.subsystem_vendor as u32;
        info.pci_revision_id = gpu.revision as u32;
        info.b_is_primary = 1;
        info
    }

    /// Hopper/Blackwell variant: same fill, NV_EP_PCFGM mirror base
    pub fn for_gpu_gh100(gpu: &GpuDevice) -> Self {
        let mut info = Self::for_gpu(gpu);
        info.pci_config_mirror_base = PCI_CONFIG_MIRROR_BASE_GH100;
        info
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                (self as *const Self).cast::<u8>(),
                size_of::<Self>(),
            )
        }
    }
}

// PACKED_REGISTRY_TABLE

/// REGISTRY_TABLE_ENTRY_TYPE_DWORD
const REGISTRY_TYPE_DWORD: u8 = 1;
/// Entry: nameOffset u32, type u8 (+3 pad), data u32, length u32
const REGISTRY_ENTRY_SIZE: usize = 16;

/// Serialize a PACKED_REGISTRY_TABLE with DWORD entries. Layout:
/// { size u32, numEntries u32, entries[], name strings (NUL-terminated) }
pub fn build_registry(entries: &[(&str, u32)]) -> Vec<u8> {
    let hdr = 8;
    let strings_off = hdr + entries.len() * REGISTRY_ENTRY_SIZE;
    let strings_len: usize = entries.iter().map(|(k, _)| k.len() + 1).sum();
    let total = strings_off + strings_len;

    let mut out = alloc::vec![0u8; total];
    out[0..4].copy_from_slice(&(total as u32).to_le_bytes());
    out[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());

    let mut soff = strings_off;
    for (i, (key, value)) in entries.iter().enumerate() {
        let e = hdr + i * REGISTRY_ENTRY_SIZE;
        out[e..e + 4].copy_from_slice(&(soff as u32).to_le_bytes());
        out[e + 4] = REGISTRY_TYPE_DWORD;
        out[e + 8..e + 12].copy_from_slice(&value.to_le_bytes());
        out[e + 12..e + 16].copy_from_slice(&4u32.to_le_bytes());
        out[soff..soff + key.len()].copy_from_slice(key.as_bytes());
        soff += key.len() + 1; // NUL already zero
    }
    out
}

// Post-INIT_DONE sequencer

#[derive(Debug, Clone, Copy, Default)]
pub struct SequencerReport {
    /// Messages drained before the first command (INIT_DONE and friends)
    pub drained: usize,
    pub system_info_ok: bool,
    pub registry_ok: bool,
}

/// 4 s per command, same budget nouveau gives RPC completion
const RPC_TIMEOUT_NS: u32 = 4_000_000_000;

/// Run the first-contact RPC sequence: drain boot events, then
/// GSP_SET_SYSTEM_INFO, then SET_REGISTRY. Both are fire-and-forget on
/// the r535/r570 line (nouveau sends them with NVKM_GSP_RPC_REPLY_NOSEQ,
/// GSP-RM does not post a reply element) - success here means the cmdq
/// accepted the elements; a rejected payload shows up asynchronously as
/// an OS_ERROR_LOG event, which the trailing drain surfaces.
/// GET_GSP_STATIC_INFO (a REPLY_RECV call) comes next once
/// GspStaticConfigInfo is mirrored (docs phase 4b)
pub fn first_contact(rpc: &mut GspRpc, gpu: &GpuDevice) -> Result<SequencerReport, RpcError> {
    let mut report = SequencerReport {
        drained: rpc.drain_events(),
        ..Default::default()
    };

    // Only the GB206 command path calls this today, so the GH100+ config
    // mirror is the right one; a Turing caller would need for_gpu()
    let info = GspSystemInfo::for_gpu_gh100(gpu);
    serial_println!(
        "[gsp/rpc] SET_SYSTEM_INFO: bar0={:#x} fb={:#x} inst={:#x} ids={:#010x}/{:#010x}",
        info.gpu_phys_addr, info.gpu_phys_fb_addr, info.gpu_phys_inst_addr,
        info.pci_device_id, info.pci_sub_device_id
    );
    rpc.send(FN_GSP_SET_SYSTEM_INFO, info.as_bytes(), RPC_TIMEOUT_NS)?;
    report.system_info_ok = true;
    serial_println!("[gsp/rpc] SET_SYSTEM_INFO queued (fire-and-forget)");

    let registry = build_registry(&[
        ("RMSecBusResetEnable", 1),
        ("RMForcePcieConfigSave", 1),
    ]);
    rpc.send(FN_SET_REGISTRY, &registry, RPC_TIMEOUT_NS)?;
    report.registry_ok = true;
    serial_println!("[gsp/rpc] SET_REGISTRY queued ({} B table)", registry.len());

    // Give GSP-RM a moment to chew and surface any complaint events
    rpc.wait_quiesce(200_000_000);
    report.drained += rpc.drain_events();

    Ok(report)
}
