// What GSP-RM is told about the machine, and the registry it is given
//
// From 615 on this is one list of NVKV keys, the data of GMCAPI `GSP_INIT`
// (`_kgspEncodeSystemInfo`, `_kgspEncodePackedRegistryKeys`; `dev/gsp-615.md`,
// section 4). `SystemInfo` holds the values; `encode` writes the keys in
// OpenRM's order, each only when something is behind it.
//
// The key numbers are `gmcapi_system_info.h` and `..._unstable.h`.

use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::pci::GpuDevice;
use crate::serial_println;

use super::nvkv::Encoder;

mod key {
    pub const PCI_DEVICE_ID: u16 = 0x0001;
    pub const PCI_CONFIG_MIRROR_BASE: u16 = 0x0010;
    pub const PCIE_ATOMICS_CPL_DEV_CAP_MASK: u16 = 0x0020;
    pub const CHIPSET: u16 = 0x0030;
    pub const CHIPSET_L1SS_ENABLE: u16 = 0x0031;
    pub const HYPERVISOR_TYPE: u16 = 0x0040;
    pub const PCIE_POWER_CONTROL_VALUE: u16 = 0x0050;
    pub const GRID_BUILD_CSP: u16 = 0x0060;
    pub const HOST_ARCH: u16 = 0x0070;
    pub const HOST_OS_TYPE: u16 = 0x0071;
    pub const HOST_PAGE_SIZE: u16 = 0x0078;
    pub const VF_TOTAL_VFS: u16 = 0x0080;
    pub const FHB_BUSINFO_REVISION_ID: u16 = 0x0090;
    pub const ACPI_DOD_STATUS: u16 = 0x0200;
    pub const ACPI_DOD_ACPI_IDS: u16 = 0x0210;
    pub const ACPI_JT_STATUS: u16 = 0x0220;
    pub const ACPI_MUX_TABLE_LEN: u16 = 0x0230;
    pub const ACPI_CAPS_STATUS: u16 = 0x0270;
    pub const PCIE_FLAGS: u16 = 0x1000;
    pub const GPU_PHYS_ADDR: u16 = 0x1010;
    pub const NV_DOMAIN_BUS_DEVICE_FUNC: u16 = 0x1020;
    pub const PCIE_ATOMICS_OP_MASK: u16 = 0x1030;
    pub const CONSOLE_MEM_SIZE: u16 = 0x1031;
    pub const CL_FLAGS: u16 = 0x1033;
    pub const SYS_TIMER_OFFSET_NS: u16 = 0x1040;
    pub const VF_FIRST_BAR0_ADDRESS: u16 = 0x1050;
    pub const FHB_BUSINFO: u16 = 0x1090;
    pub const KERNEL_DRIVER_NAME: u16 = 0x1100;
    pub const KERNEL_DRIVER_VERSION_STRING: u16 = 0x1101;
    pub const KERNEL_DRIVER_CHANGELIST: u16 = 0x1102;
    pub const UNSTABLE_INTERFACE_SUPPORT: u16 = 0x1110;
    pub const REGKEY_NAME: u16 = 0x3070;
    pub const REGKEY_VALUE_U32: u16 = 0x3071;
}

/// `NVGMC_SI_PCIE_FLAGS_*` bits
mod pcie {
    pub const GPU_BEHIND_BRIDGE: u64 = 1 << 0;
    pub const FLR_SUPPORTED: u64 = 1 << 1;
    pub const BAR0_64B_SUPPORTED: u64 = 1 << 2;
    pub const UPSTREAM_L0S_UNSUPPORTED: u64 = 1 << 4;
    pub const UPSTREAM_L1_UNSUPPORTED: u64 = 1 << 5;
    pub const UPSTREAM_L1_POR_SUPPORTED: u64 = 1 << 6;
    pub const UPSTREAM_L1_POR_MOBILE_ONLY: u64 = 1 << 7;
    pub const UPSTREAM_ADDRESS_VALID: u64 = 1 << 10;
}

/// `NVGMC_SI_PLATFORM_FLAGS_IS_PRIMARY`
const PLATFORM_IS_PRIMARY: u64 = 1 << 2;

/// `NVGMC_SI_HOST_ARCH_X86_64`, `NVGMC_SI_HOST_OS_TYPE_UNIX`. Unix, as the
/// Linux driver says: the OS layer GSP-RM was built against
pub const HOST_ARCH_X86_64: u32 = 1;
pub const HOST_OS_UNIX: u32 = 1;

/// `NVGMC_SI_UNSTABLE_INTERFACE_SUPPORT_RPC | _EVENTS`: this driver uses the
/// vGPU-style RPCs and events, not ABI-stable
const UNSTABLE_RPC_AND_EVENTS: u32 = 0x3;

/// What the driver calls itself: "NVRM" and the release it speaks, from an
/// open-source build (no changelist, `nvBldVer.h` without a buildmeister)
pub const DRIVER_NAME: &str = "NVRM";
pub const DRIVER_CHANGELIST: u32 = 0;

/// Status every ACPI method is reported with: not evaluated. 0 would mean
/// success, and MikuOS has no AML interpreter to run `_DOD`, `JT` or the NVOP
/// caps query. 0xffff is what nouveau sends for a GPU whose ACPI node lacks
/// those methods
pub const ACPI_METHOD_UNAVAILABLE: u32 = 0xffff;

/// PCI config-space mirror in BAR0 (`NV_EP_PCFGM`, 0x92000-0x92fff on GH100 and
/// GB20x, `kbifGetPciConfigSpacePriMirror_GH100`)
pub const PCI_CONFIG_MIRROR_BASE_GH100: u32 = 0x0009_2000;
pub const PCI_CONFIG_MIRROR_SIZE: u32 = 0x1000;

/// Linux x86_64 TASK_SIZE; GSP-RM only uses this to size internal VA maps
pub const MAX_USER_VA: u64 = 0x0000_7FFF_FFFF_F000;

/// One PCI device's ids, as GSP-RM is told them (`BUSINFO`)
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Businfo {
    pub device_id: u16,
    pub vendor_id: u16,
    pub subdevice_id: u16,
    pub subvendor_id: u16,
    pub revision_id: u8,
}

impl Businfo {
    /// The 64-bit word `NVGMC_SI_BUSINFO_*` packs the four ids into
    fn word(&self) -> u64 {
        self.device_id as u64 | (self.vendor_id as u64) << 16
            | (self.subdevice_id as u64) << 32 | (self.subvendor_id as u64) << 48
    }
}

/// Everything `_kgspEncodeSystemInfo` sends that this driver has a value for
#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    pub pci_device_id: u32,
    pub pci_sub_device_id: u32,
    pub pci_revision_id: u32,
    /// BAR0, BAR1, the instance BAR, IO (none)
    pub gpu_phys: [u64; 4],
    pub nv_domain_bus_device_func: u64,
    pub pci_config_mirror_base: u32,
    pub pci_config_mirror_size: u32,
    pub flr_supported: bool,
    pub bar0_64bit: bool,
    pub chipset_l1ss_enable: u32,
    pub pcie_dev_ctrl2: u32,
    pub pcie_link_cap: u32,
    pub pcie_atomics_op_mask: u64,
    pub pcie_atomics_cpl_device_cap_mask: u32,
    pub console_mem_size: u64,
    /// `NVGMC_SI_CL_FLAGS_*`, one bit per chipset property
    pub cl_flags: u64,
    pub fhb: Businfo,
    pub chipset_id: Businfo,
    pub chipset: u32,
    pub sys_timer_offset_ns: u64,
    pub behind_bridge: bool,
    pub upstream_l0s_unsupported: bool,
    pub upstream_l1_unsupported: bool,
    pub upstream_l1_por_supported: bool,
    pub upstream_l1_por_mobile_only: bool,
    pub upstream_address_valid: bool,
    pub is_primary: bool,
}

/// How much vidmem the firmware console still scans out of, so GSP-RM reserves
/// it and does not fold it into its heap.
///
/// OpenRM (`rm_get_uefi_console_size` ->
/// `ReservedConsoleDispMemSize = ALIGN_UP(size, 0x10000)`) takes the size from
/// the EFI GOP framebuffer the OS inherited, only when it sits in this GPU's
/// BAR1. Sending 0 with the console live in BAR1 tells GSP-RM the aperture is
/// free
fn uefi_console_size(gpu: &GpuDevice) -> u64 {
    let Some(fbi) = crate::arch::x86_64::grub::framebuffer() else {
        return 0;
    };
    let Some(loc) = crate::drivers::gpu::nvidia::fb::find_in_bars(gpu, fbi.addr) else {
        return 0;
    };
    if loc.bar_index != 1 {
        serial_println!(
            "[gsp/sysinfo] boot fb is in BAR{} (+{:#x}), not the vidmem aperture - console size 0",
            loc.bar_index, loc.offset
        );
        return 0;
    }
    let bytes = (fbi.pitch as u64) * (fbi.height as u64);
    let size = (bytes + 0xFFFF) & !0xFFFF;
    serial_println!(
        "[gsp/sysinfo] UEFI console {}x{} pitch {} in BAR1 +{:#x} -> console size {:#x}",
        fbi.width, fbi.height, fbi.pitch, loc.offset, size
    );
    size
}

/// `PDB_PROP_GPU_PRIMARY_DEVICE` (`osinit.c`): the GPU drives a firmware
/// framebuffer console (the console size is not zero), or it is the legacy
/// VGA device the firmware routed
pub fn is_primary_device(gpu: &GpuDevice, console_mem_size: u64) -> bool {
    console_mem_size != 0 || crate::drivers::gpu::nvidia::pci::is_boot_vga(gpu)
}

impl SystemInfo {
    /// What a GH100 or GB20x on a bare-metal x86 host is: the BARs, the ids,
    /// the config mirror, the PCIe capability, the firmware console. The
    /// chipset half is `chipset::fill`
    pub fn for_gpu(gpu: &GpuDevice) -> Self {
        let mut info = Self {
            pci_device_id: ((gpu.device as u32) << 16) | gpu.vendor as u32,
            pci_sub_device_id: ((gpu.subsystem_device as u32) << 16) | gpu.subsystem_vendor as u32,
            pci_revision_id: gpu.revision as u32,
            gpu_phys: [
                gpu.bars[0].phys,
                gpu.bars[1].phys,
                // BAR3 is RM's instance BAR (the 64-bit pair 3/4); BAR2 if the
                // firmware put it there
                if gpu.bars[3].phys != 0 { gpu.bars[3].phys } else { gpu.bars[2].phys },
                0,
            ],
            nv_domain_bus_device_func: ((gpu.bus as u64) << 8) | ((gpu.dev as u64) << 3) | gpu.func as u64,
            pci_config_mirror_base: PCI_CONFIG_MIRROR_BASE_GH100,
            pci_config_mirror_size: PCI_CONFIG_MIRROR_SIZE,
            bar0_64bit: gpu.bars[0].is_64bit,
            ..Default::default()
        };
        info.console_mem_size = uefi_console_size(gpu);
        info.is_primary = is_primary_device(gpu, info.console_mem_size);
        if let Some(c) = crate::drivers::gpu::nvidia::reset::pcie_caps(gpu) {
            info.flr_supported = c.flr;
            info.pcie_link_cap = c.link_cap;
            info.pcie_dev_ctrl2 = c.dev_ctrl2;
        }
        info
    }

    /// The NVKV list `GSP_INIT` carries, in `_kgspEncodeSystemInfo`'s order,
    /// with the registry after it
    pub fn encode(&self, registry: &[(&str, u32)]) -> Vec<u8> {
        let mut e = Encoder::new();
        e.seq32(0, key::PCI_DEVICE_ID, &[self.pci_device_id, self.pci_sub_device_id, self.pci_revision_id]);
        e.seq64(0, key::GPU_PHYS_ADDR, &self.gpu_phys);
        e.u64(0, key::NV_DOMAIN_BUS_DEVICE_FUNC, self.nv_domain_bus_device_func);
        e.u32(0, key::HOST_ARCH, HOST_ARCH_X86_64);
        e.u32(0, key::HOST_OS_TYPE, HOST_OS_UNIX);

        // KernelBif's half
        e.seq32(0, key::PCI_CONFIG_MIRROR_BASE, &[self.pci_config_mirror_base, self.pci_config_mirror_size]);
        e.u32(0, key::CHIPSET_L1SS_ENABLE, self.chipset_l1ss_enable);
        // Power control value (not present), DevCtl2, link caps
        e.seq32(0, key::PCIE_POWER_CONTROL_VALUE, &[0, self.pcie_dev_ctrl2, self.pcie_link_cap]);
        e.u64(0, key::PCIE_ATOMICS_OP_MASK, self.pcie_atomics_op_mask);
        e.u32(0, key::PCIE_ATOMICS_CPL_DEV_CAP_MASK, self.pcie_atomics_cpl_device_cap_mask);

        e.seq64(0, key::CONSOLE_MEM_SIZE, &[self.console_mem_size, MAX_USER_VA]);

        // The chipset (`clSyncWithGsp`)
        e.u64(0, key::CL_FLAGS, self.cl_flags);
        e.seq64(0, key::FHB_BUSINFO, &[self.fhb.word(), self.chipset_id.word()]);
        e.seq32(0, key::FHB_BUSINFO_REVISION_ID, &[self.fhb.revision_id as u32, self.chipset_id.revision_id as u32]);
        e.u32(0, key::CHIPSET, self.chipset);

        e.u32(0, key::HOST_PAGE_SIZE, 4096);

        // ACPI: any key says the data is valid. Every method is reported as not
        // evaluated, with no display ids and no mux table
        e.u32(0, key::ACPI_DOD_STATUS, ACPI_METHOD_UNAVAILABLE);
        e.array32(0, key::ACPI_DOD_ACPI_IDS, &[]);
        e.seq32(0, key::ACPI_JT_STATUS, &[ACPI_METHOD_UNAVAILABLE, 0, 0, 0]);
        e.u32(0, key::ACPI_MUX_TABLE_LEN, 0);
        e.seq32(0, key::ACPI_CAPS_STATUS, &[ACPI_METHOD_UNAVAILABLE, 0]);

        // No hypervisor, no virtual functions
        e.seq32(0, key::HYPERVISOR_TYPE, &[0, 0]);
        e.seq32(0, key::VF_TOTAL_VFS, &[0, 0]);
        e.seq64(0, key::VF_FIRST_BAR0_ADDRESS, &[0, 0, 0]);

        e.u64(0, key::SYS_TIMER_OFFSET_NS, self.sys_timer_offset_ns);
        e.u32(0, key::GRID_BUILD_CSP, 0);

        let flag = |on: bool, bit: u64| if on { bit } else { 0 };
        let pcie_flags = flag(self.behind_bridge, pcie::GPU_BEHIND_BRIDGE)
            | flag(self.flr_supported, pcie::FLR_SUPPORTED)
            | flag(self.bar0_64bit, pcie::BAR0_64B_SUPPORTED)
            | flag(self.upstream_l0s_unsupported, pcie::UPSTREAM_L0S_UNSUPPORTED)
            | flag(self.upstream_l1_unsupported, pcie::UPSTREAM_L1_UNSUPPORTED)
            | flag(self.upstream_l1_por_supported, pcie::UPSTREAM_L1_POR_SUPPORTED)
            | flag(self.upstream_l1_por_mobile_only, pcie::UPSTREAM_L1_POR_MOBILE_ONLY)
            | flag(self.upstream_address_valid, pcie::UPSTREAM_ADDRESS_VALID);
        let platform_flags = flag(self.is_primary, PLATFORM_IS_PRIMARY);
        // No HDMI FRL compliance mode, no vidmem preserved over suspend, no
        // TDR event, no stretched vblank, no clock boost (not asked of the
        // FSP), window channels mapped on demand; no VF BARs
        e.seq64(0, key::PCIE_FLAGS, &[pcie_flags, platform_flags, 0, 0]);

        e.string(0, key::KERNEL_DRIVER_NAME, DRIVER_NAME);
        e.string(0, key::KERNEL_DRIVER_VERSION_STRING, super::GSP_RM_VERSION);
        e.u32(0, key::KERNEL_DRIVER_CHANGELIST, DRIVER_CHANGELIST);
        e.u32(0, key::UNSTABLE_INTERFACE_SUPPORT, UNSTABLE_RPC_AND_EVENTS);

        for (name, value) in registry {
            e.string(0, key::REGKEY_NAME, name);
            e.u32(0, key::REGKEY_VALUE_U32, *value);
        }
        e.to_bytes()
    }
}
