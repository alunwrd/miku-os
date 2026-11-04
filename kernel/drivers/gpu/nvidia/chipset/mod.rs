// The chipset and the ports above the GPU, as OpenRM's OBJCL knows them
//
// `dev/gsp-foundation.md`, step 1.17. OpenRM runs `clUpdatePcieConfig`
// (`chipset_pcie.c:933`) right after it maps the registers: the Linux driver
// calls it from `osInitScalability` (`osinit.c:1808`), ahead of the FSP and
// GSP-RM. Three things:
// 1. Works out what it is plugged into: the chipset by OpenRM's tables, the
//    first host bridge, the upstream and root ports, whether extended config
//    space is reachable, LTR, L1 substates and PCIe atomics (`discover`).
// 2. Sets the GPU's Device Control to match: Extended Tag, Relaxed Ordering, No
//    Snoop, and the atomics requester (`init_device_control`). "This should
//    happen before any DMA."
// 3. Keeps what it learned for GSP_INIT's system info (`fill`), how GSP-RM
//    hears about the machine (`clSyncWithGsp`, `chipset.c:779`).

pub mod table;

use crate::drivers::bus::pci;
use super::gsp_common::sysinfo::{Businfo, SystemInfo};
use super::pci::GpuDevice;

/// OpenRM's chipset properties, in `CL_SYNC_PDB` order (`chipset.c:788`). The
/// discriminant is the bit `clSyncWithGsp` gives the property in
/// `clPdbProperties`, so the order is GSP-RM's ABI. Some are set by nothing on
/// x86 (registry keys, Tegra, hypervisors) and hold the later bits in place
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Prop {
    PcieConfigAccessible = 0,
    ForceSnoopReadsAndWritesWarBug410390,
    DisableBr03FlowControl,
    AslmSupportsNvLinkUpgrade,
    AslmSupportsFastLinkUpgrade,
    AslmSupportsHotReset,
    AslmSupportsGen2LinkUpgrade,
    IsChipsetInAspmPorList,
    AspmL0sChipsetDisabled,
    AspmL1ChipsetDisabled,
    War4802761Enabled,
    AspmL0sChipsetEnabledMobileOnly,
    AspmL1ChipsetEnabledMobileOnly,
    AspmL1UpstreamPortSupported,
    PcieGen1Gen2SwitchChipsetDisabled,
    PcieGen1Gen2SwitchChipsetDisabledGeforce,
    ExtendedTagFieldNotCapable,
    NosnoopNotCapable,
    RelaxedOrderingNotCapable,
    PcieForceGen2Enable,
    PcieGen2AtLessThanX16Disabled,
    RootportNeedsNosnoopWar,
    IntelCpuRootport1NeedsH57War,
    PcieConfigSkipMcfgRead,
    OnPcieGen3Patsburg,
    AllowPcieGen3OnPatsburgWithIvbeCpu,
    Bug999673P2pArbitrarySplitWar,
    UpstreamLtrSupported,
    Bug1340801DisableGen3OnGigabyteSniper3,
    Bug1681803WarDisableMscg,
    PcieNonCoherentUseTc0Only,
    UnsupportedChipset,
    IsChipsetIoCoherent,
    DisableIomapWc,
    HasResizableBarIssue,
    Bug3751839GenSpeedWar,
    Bug3562968WarAllowPcieAtomics,
    /// 615 on: bit 37 of `NVGMC_SI_CL_FLAGS`
    WarAmd5107271,
}

/// The properties' OpenRM names, by bit, for `nvidia pcie` and the selftest
pub const PROP_NAMES: [&str; 38] = [
    "PCIE_CONFIG_ACCESSIBLE",
    "FORCE_SNOOP_READS_AND_WRITES_WAR_BUG_410390",
    "DISABLE_BR03_FLOW_CONTROL",
    "ASLM_SUPPORTS_NV_LINK_UPGRADE",
    "ASLM_SUPPORTS_FAST_LINK_UPGRADE",
    "ASLM_SUPPORTS_HOT_RESET",
    "ASLM_SUPPORTS_GEN2_LINK_UPGRADE",
    "IS_CHIPSET_IN_ASPM_POR_LIST",
    "ASPM_L0S_CHIPSET_DISABLED",
    "ASPM_L1_CHIPSET_DISABLED",
    "WAR_4802761_ENABLED",
    "ASPM_L0S_CHIPSET_ENABLED_MOBILE_ONLY",
    "ASPM_L1_CHIPSET_ENABLED_MOBILE_ONLY",
    "ASPM_L1_UPSTREAM_PORT_SUPPORTED",
    "PCIE_GEN1_GEN2_SWITCH_CHIPSET_DISABLED",
    "PCIE_GEN1_GEN2_SWITCH_CHIPSET_DISABLED_GEFORCE",
    "EXTENDED_TAG_FIELD_NOT_CAPABLE",
    "NOSNOOP_NOT_CAPABLE",
    "RELAXED_ORDERING_NOT_CAPABLE",
    "PCIE_FORCE_GEN2_ENABLE",
    "PCIE_GEN2_AT_LESS_THAN_X16_DISABLED",
    "ROOTPORT_NEEDS_NOSNOOP_WAR",
    "INTEL_CPU_ROOTPORT1_NEEDS_H57_WAR",
    "PCIE_CONFIG_SKIP_MCFG_READ",
    "ON_PCIE_GEN3_PATSBURG",
    "ALLOW_PCIE_GEN3_ON_PATSBURG_WITH_IVBE_CPU",
    "BUG_999673_P2P_ARBITRARY_SPLIT_WAR",
    "UPSTREAM_LTR_SUPPORTED",
    "BUG_1340801_DISABLE_GEN3_ON_GIGABYTE_SNIPER_3",
    "BUG_1681803_WAR_DISABLE_MSCG",
    "PCIE_NON_COHERENT_USE_TC0_ONLY",
    "UNSUPPORTED_CHIPSET",
    "IS_CHIPSET_IO_COHERENT",
    "DISABLE_IOMAP_WC",
    "HAS_RESIZABLE_BAR_ISSUE",
    "BUG_3751839_GEN_SPEED_WAR",
    "BUG_3562968_WAR_ALLOW_PCIE_ATOMICS",
    "WAR_AMD_5107271",
];

/// One function on the bus, as OpenRM's `BUSINFO` and address together
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub subvendor: u16,
    pub subdevice: u16,
    pub revision: u8,
}

impl Device {
    fn of(d: &pci::PciDevice) -> Self {
        let (subvendor, subdevice) = pci::subsystem(d.bus, d.dev, d.func);
        Device {
            bus: d.bus, dev: d.dev, func: d.func,
            vendor: d.vendor, device: d.device,
            subvendor, subdevice,
            revision: pci::revision(d.bus, d.dev, d.func),
        }
    }

    fn same_place(&self, other: &Device) -> bool {
        self.bus == other.bus && self.dev == other.dev && self.func == other.func
    }
}

/// Everything OpenRM's OBJCL and the GPU's `gpuClData` hold that GSP-RM is
/// told, plus what the Device Control step did
#[derive(Clone, Debug, Default)]
pub struct Chipset {
    /// `clPdbProperties`: one bit per `Prop`
    pub props: u64,
    /// `CS_*` (`nvdevid.h:541`) and the table's name for it
    pub chipset: u32,
    pub name: &'static str,
    /// The first host bridge, and the device the chipset was recognised by: the
    /// same one when the table's match is a host bridge
    pub fhb: Option<Device>,
    pub chipset_id: Option<Device>,
    /// The bridge the GPU hangs off, and the root port above it, the same one
    /// unless a switch is in between
    pub upstream: Option<Device>,
    pub root: Option<Device>,
    pub behind_bridge: bool,
    pub upstream_l0s_unsupported: bool,
    pub upstream_l1_unsupported: bool,
    pub upstream_l1_por_supported: bool,
    pub upstream_l1_por_mobile_only: bool,
    /// The upstream port's L1 PM Substates Control 1, as read; 0 without the
    /// capability or without ECAM (`kbifCacheChipsetL1SubstatesEnable`)
    pub l1ss_control1: u32,
    /// `osPcieAtomicsOpMask` (`OS_PCIE_CAP_MASK_REQ_ATOMICS_*`) and
    /// `pcieAtomicsCplDeviceCapMask` (`BIF_PCIE_CPL_ATOMICS_*`)
    pub atomics_op_mask: u64,
    pub atomics_cpl_mask: u32,
    /// The GPU's Device Control before and after `init_device_control`; both 0
    /// until it ran, `after == before` under `pciebase`
    pub devctl_before: u16,
    pub devctl_after: u16,
}

impl Chipset {
    pub fn set(&mut self, p: Prop) { self.props |= 1u64 << p as u8; }
    pub fn clear(&mut self, p: Prop) { self.props &= !(1u64 << p as u8); }
    pub fn has(&self, p: Prop) -> bool { self.props & (1u64 << p as u8) != 0 }

    /// The names of every property set, in bit order
    pub fn prop_names(&self) -> alloc::vec::Vec<&'static str> {
        (0..PROP_NAMES.len())
            .filter(|i| self.props & (1u64 << i) != 0)
            .map(|i| PROP_NAMES[i])
            .collect()
    }
}

/// What the last `discover` found and `init_device_control` did, for
/// `nvidia pcie`
static LAST: spin::Mutex<Option<Chipset>> = spin::Mutex::new(None);

pub fn last() -> Option<Chipset> {
    LAST.lock().clone()
}

/// `pciebase` on the boot line: Device Control stays what the firmware left
pub fn base_devctl() -> bool {
    crate::arch::x86_64::grub::cmdline_flag("pciebase")
}

// PCIe capability registers, from the capability's start (PCIe Base 6.0, 7.5.3)
const DEV_CAP: u8 = 0x04;
const DEV_CTL: u8 = 0x08;
const LINK_CAP: u8 = 0x0C;
const DEV_CAP2: u8 = 0x24;
const DEV_CTL2: u8 = 0x28;

const DEVCAP_EXT_TAG: u32 = 1 << 5;
const DEVCTL_RELAXED_ORDERING: u16 = 1 << 4;
const DEVCTL_EXT_TAG: u16 = 1 << 8;
const DEVCTL_NO_SNOOP: u16 = 1 << 11;
/// Link Capabilities ASPM Support, bits 11:10 (`CL_PCIE_LINK_CAP_ASPM_*_BIT`)
const LINKCAP_ASPM_L0S: u32 = 1 << 10;
const LINKCAP_ASPM_L1: u32 = 1 << 11;
const DEVCAP2_ATOMIC_ROUTE: u32 = 1 << 6;
const DEVCAP2_ATOMIC_COMP32: u32 = 1 << 7;
const DEVCAP2_ATOMIC_COMP64: u32 = 1 << 8;
const DEVCAP2_ATOMIC_COMP128: u32 = 1 << 9;
const DEVCAP2_LTR: u32 = 1 << 11;
const DEVCTL2_ATOMIC_REQ: u32 = 1 << 6;
const DEVCTL2_ATOMIC_EGRESS_BLOCK: u32 = 1 << 7;
const DEVCTL2_LTR: u32 = 1 << 10;

/// One dword of a function's PCIe capability, through the legacy mechanism
/// (the capability is always in the first 256 bytes)
fn pcie_reg(d: &Device, off: u8) -> Option<u32> {
    let cap = pci::find_cap(d.bus, d.dev, d.func, pci::CAP_ID_PCIE)?;
    Some(pci::pci_read32(d.bus, d.dev, d.func, cap + off))
}

/// `objClBuildPcieAtomicsAllowList` (`chipset_pcie.c:206`): on x86 the
/// processor decides whether the GPU may issue PCIe atomics at all
fn atomics_allowed_cpu() -> bool {
    let r = core::arch::x86_64::__cpuid(1);
    let base_family = (r.eax >> 8) & 0xF;
    let family = if base_family == 0xF { base_family + ((r.eax >> 20) & 0xFF) } else { base_family };
    let model = ((r.eax >> 4) & 0xF) | if base_family == 0x6 || base_family == 0xF {
        ((r.eax >> 16) & 0xF) << 4
    } else {
        0
    };
    let stepping = r.eax & 0xF;
    // Intel Ice Lake, Sapphire Rapids; AMD Milan, Genoa
    (family == 0x6 && model == 0x6a && stepping == 0x6)
        || (family == 0x6 && model == 0x8f)
        || (family == 0x19 && model == 0x1 && stepping == 0x1)
        || (family == 0x19 && model == 0x11)
}

/// Work out what the GPU is plugged into, in `objClInitPcieChipset`'s order.
/// Reads only
pub fn discover(gpu: &GpuDevice) -> Chipset {
    let mut c = Chipset { chipset: table::CS_UNKNOWN, name: "Unknown", ..Default::default() };
    // `clConstruct_IMPL`: the one property set before anything is known
    c.set(Prop::IsChipsetIoCoherent);
    // `clUpdatePcieConfig` builds the atomics allow list first
    if atomics_allowed_cpu() {
        c.set(Prop::Bug3562968WarAllowPcieAtomics);
    }

    // `clFindFHBAndGetChipsetInfoIndex` (`chipset.c:283`): the first table row
    // with a device on the bus. A host bridge is both the FHB and the chipset's
    // id; anything else is the id, and the FHB is found by class below
    let devs = pci::devices();
    let mut matched: Option<&'static table::Row> = None;
    'rows: for row in table::CHIPSETS {
        for d in &devs {
            if d.vendor == row.vendor && d.device == row.device {
                matched = Some(row);
                let dev = Device::of(d);
                c.chipset_id = Some(dev);
                if (d.class == 0x06 && d.subclass == 0x00)
                    || (d.bus % 0x40 == 0 && d.dev == 0 && d.func == 0)
                {
                    c.fhb = Some(dev);
                }
                break 'rows;
            }
        }
    }
    if c.fhb.is_none() {
        // Host bridge, then PCI-to-PCI bridge, then 3D controller
        for (class, sub) in [(0x06u8, 0x00u8), (0x06, 0x04), (0x03, 0x02)] {
            if let Some(d) = devs.iter().find(|d| d.class == class && d.subclass == sub) {
                c.fhb = Some(Device::of(d));
                if matched.is_none() {
                    c.chipset_id = c.fhb;
                }
                break;
            }
        }
    }
    if let Some(row) = matched {
        c.chipset = row.chipset;
        c.name = row.name;
        if let Some(setup) = row.setup {
            setup(&mut c);
        }
    }

    // Extended config space (`chipset_pcie.c:349-375`): from `MCFG` unless a
    // setup function found it and said to skip `MCFG`, which no setup function
    // here does, and only if reading the first host bridge through it says what
    // reading it the legacy way does
    if !(c.has(Prop::PcieConfigAccessible) && c.has(Prop::PcieConfigSkipMcfgRead))
        && pci::has_ecam()
    {
        c.set(Prop::PcieConfigAccessible);
    }
    if c.has(Prop::PcieConfigAccessible) {
        if let Some(f) = c.fhb {
            let want = f.vendor as u32 | (f.device as u32) << 16;
            if pci::read32_ext(f.bus, f.dev, f.func, 0) != Some(want) {
                crate::kwarn!("[nvidia/bus] ECAM does not agree with config space about the host bridge - not using it");
                c.clear(Prop::PcieConfigAccessible);
            }
        }
    }

    // `objClInitGpuPortData` (`chipset_pcie.c:1139`)
    c.upstream = pci::upstream_bridge(gpu.bus).map(|d| Device::of(&d));
    c.root = pci::root_port_of(gpu.bus).map(|d| Device::of(&d));
    if let (Some(up), Some(root)) = (c.upstream, c.root) {
        c.behind_bridge = !up.same_place(&root);
    }
    if c.behind_bridge {
        // Behind a switch, ASPM follows the bridge the GPU is plugged into
        // (bug 540109)
        match c.upstream.and_then(|u| pcie_reg(&u, LINK_CAP)) {
            Some(link_cap) => {
                c.upstream_l0s_unsupported = link_cap & LINKCAP_ASPM_L0S == 0;
                c.upstream_l1_unsupported = link_cap & LINKCAP_ASPM_L1 == 0;
            }
            None => {
                c.upstream_l0s_unsupported = true;
                c.upstream_l1_unsupported = true;
            }
        }
    }

    // The port tables: the root port's row, then the upstream port's
    if let Some(root) = c.root {
        if let Some(setup) = table::port_row(table::ROOT_PORTS, root.vendor, root.device).and_then(|r| r.setup) {
            setup(&mut c);
        }
    }
    if let Some(up) = c.upstream {
        if let Some(setup) = table::port_row(table::UPSTREAM_PORTS, up.vendor, up.device).and_then(|r| r.setup) {
            setup(&mut c);
        }
    }

    // The root port through ECAM, the same check as the host bridge
    if c.has(Prop::PcieConfigAccessible) {
        if let Some(r) = c.root {
            let want = r.vendor as u32 | (r.device as u32) << 16;
            if pci::read32_ext(r.bus, r.dev, r.func, 0) != Some(want) {
                crate::kwarn!("[nvidia/bus] ECAM does not agree with config space about the root port - not using it");
                c.clear(Prop::PcieConfigAccessible);
            }
        }
    }

    // LTR: the root port supports and enables it, and so does every bridge
    // between it and the GPU (`clCheckUpstreamLtrSupport`, which needs config
    // space accessible)
    let ltr = |d: &Device| {
        matches!((pcie_reg(d, DEV_CAP2), pcie_reg(d, DEV_CTL2)),
                 (Some(cap2), Some(ctl2)) if cap2 & DEVCAP2_LTR != 0 && ctl2 & DEVCTL2_LTR != 0)
    };
    if c.root.as_ref().map(ltr).unwrap_or(false) && c.has(Prop::PcieConfigAccessible) {
        let mut bus = gpu.bus;
        let mut all = true;
        for _ in 0..16 {
            let Some(up) = pci::upstream_bridge(bus) else { all = false; break };
            let up = Device::of(&up);
            if !ltr(&up) {
                all = false;
                break;
            }
            if pci::pcie_port_type(up.bus, up.dev, up.func) == Some(pci::PCIE_TYPE_ROOT_PORT) {
                break;
            }
            bus = up.bus;
        }
        if all {
            c.set(Prop::UpstreamLtrSupported);
        }
    }

    // `kbifCacheChipsetL1SubstatesEnable`: the upstream port's L1 PM
    // Substates Control 1, which lives in extended config space
    if let Some(up) = c.upstream {
        if let Some(at) = pci::find_ext_cap(up.bus, up.dev, up.func, pci::EXT_CAP_L1SS) {
            c.l1ss_control1 = pci::read32_ext(up.bus, up.dev, up.func, at + 8).unwrap_or(0);
        }
    }

    // `kbifProbePcieCplAtomicCaps_GB100`: what the GPU itself completes
    let gpu_dev = Device {
        bus: gpu.bus, dev: gpu.dev, func: gpu.func,
        vendor: gpu.vendor, device: gpu.device, ..Default::default()
    };
    if let Some(cap2) = pcie_reg(&gpu_dev, DEV_CAP2) {
        let mut m = 0u32;
        if cap2 & DEVCAP2_ATOMIC_COMP32 != 0 {
            m |= (1 << 0) | (1 << 2) | (1 << 4); // FETCHADD_32, SWAP_32, CAS_32
        }
        if cap2 & DEVCAP2_ATOMIC_COMP64 != 0 {
            m |= (1 << 1) | (1 << 3) | (1 << 5); // FETCHADD_64, SWAP_64, CAS_64
        }
        if cap2 & DEVCAP2_ATOMIC_COMP128 != 0 {
            m |= 1 << 6; // CAS_128
        }
        c.atomics_cpl_mask = m;
    }

    *LAST.lock() = Some(c.clone());
    c
}

/// `pci_enable_atomic_ops_to_root` (Linux `drivers/pci/pci.c`), what the
/// driver's `osConfigurePcieReqAtomics` asks the OS: can a request of this size
/// get from the GPU to the root port. Every switch port on the way routes
/// atomics, an upstream port does not block them, and the root port completes
/// them
fn atomics_reach_root(gpu_bus: u8, completer: u32) -> bool {
    let mut bus = gpu_bus;
    for _ in 0..16 {
        let Some(up) = pci::upstream_bridge(bus) else { return false };
        let d = Device::of(&up);
        let Some(cap2) = pcie_reg(&d, DEV_CAP2) else { return false };
        match pci::pcie_port_type(d.bus, d.dev, d.func) {
            Some(pci::PCIE_TYPE_ROOT_PORT) => return cap2 & completer == completer,
            Some(pci::PCIE_TYPE_UPSTREAM) => {
                if cap2 & DEVCAP2_ATOMIC_ROUTE == 0 {
                    return false;
                }
                if pcie_reg(&d, DEV_CTL2).map(|c| c & DEVCTL2_ATOMIC_EGRESS_BLOCK != 0).unwrap_or(true) {
                    return false;
                }
            }
            Some(pci::PCIE_TYPE_DOWNSTREAM) => {
                if cap2 & DEVCAP2_ATOMIC_ROUTE == 0 {
                    return false;
                }
            }
            _ => {}
        }
        bus = d.bus;
    }
    false
}

/// `kbifInitPcieDeviceControlStatus` (`kernel_bif.c:439`) and the requester
/// half of the atomics probe, before any DMA. Under `pciebase` nothing is
/// written and the record says so
pub fn init_device_control(gpu: &GpuDevice, c: &mut Chipset) {
    let Some(cap) = pci::find_cap(gpu.bus, gpu.dev, gpu.func, pci::CAP_ID_PCIE) else { return };
    let rd = |off: u8| pci::pci_read32(gpu.bus, gpu.dev, gpu.func, cap + off);
    let before = rd(DEV_CTL) as u16;
    c.devctl_before = before;
    c.devctl_after = before;
    if base_devctl() {
        *LAST.lock() = Some(c.clone());
        return;
    }

    let mut ctl = before;
    // `kbifEnableExtendedTagSupport_GB100`
    if rd(DEV_CAP) & DEVCAP_EXT_TAG != 0 && !c.has(Prop::ExtendedTagFieldNotCapable) {
        ctl |= DEVCTL_EXT_TAG;
    }
    // "Many SBIOSes default to disabling relaxed ordering on GPUs, we want to
    // always force it back on unless the upstream root port is known to be
    // broken with respect to this feature" (bugs 382675, 482867)
    if c.has(Prop::RelaxedOrderingNotCapable) {
        ctl &= !DEVCTL_RELAXED_ORDERING;
    } else {
        ctl |= DEVCTL_RELAXED_ORDERING;
    }
    // Bug 393398, and bugs 332764 / 1511622 for the other way
    if !c.has(Prop::NosnoopNotCapable) && !c.has(Prop::RootportNeedsNosnoopWar) {
        ctl |= DEVCTL_NO_SNOOP;
    } else {
        ctl &= !DEVCTL_NO_SNOOP;
    }
    // A dword with Device Status as zero: its error bits are
    // write-one-to-clear, and writing back what was read wipes the errors
    // `nvidia pcie` shows
    if ctl != before {
        pci::pci_write32(gpu.bus, gpu.dev, gpu.func, cap + DEV_CTL, ctl as u32);
    }
    c.devctl_after = rd(DEV_CTL) as u16;

    // `kbifProbePcieReqAtomicCaps_GH100`: only on an allowed processor, only
    // the sizes that reach the root port, and then the requester is enabled
    if c.has(Prop::Bug3562968WarAllowPcieAtomics) {
        let mut mask = 0u64;
        for (bit, completer) in [(0u64, DEVCAP2_ATOMIC_COMP32), (1, DEVCAP2_ATOMIC_COMP64), (2, DEVCAP2_ATOMIC_COMP128)] {
            if atomics_reach_root(gpu.bus, completer) {
                mask |= 1 << bit;
            }
        }
        if mask != 0 {
            let ctl2 = rd(DEV_CTL2);
            pci::pci_write32(gpu.bus, gpu.dev, gpu.func, cap + DEV_CTL2, ctl2 | DEVCTL2_ATOMIC_REQ);
        }
        c.atomics_op_mask = mask;
    }

    *LAST.lock() = Some(c.clone());
}

fn businfo(d: Option<Device>, invalid: bool) -> Businfo {
    match d {
        Some(d) => Businfo {
            device_id: d.device, vendor_id: d.vendor,
            subdevice_id: d.subdevice, subvendor_id: d.subvendor,
            revision_id: d.revision,
        },
        // `PCI_INVALID_*` is what OpenRM initialises the chipset's id to
        None if invalid => Businfo {
            device_id: 0xFFFF, vendor_id: 0xFFFF, subdevice_id: 0xFFFF, subvendor_id: 0xFFFF,
            revision_id: 0,
        },
        None => Businfo { device_id: 0, vendor_id: 0, subdevice_id: 0, subvendor_id: 0, revision_id: 0 },
    }
}

/// `clSyncWithGsp` (`chipset.c`) and the GPU's port flags
/// (`_kgspEncodeSystemInfo`). The property bits are `NVGMC_SI_CL_FLAGS_*`, the
/// order of `Prop`
pub fn fill(c: &Chipset, info: &mut SystemInfo) {
    info.cl_flags = c.props;
    info.chipset = c.chipset;
    info.fhb = businfo(c.fhb, false);
    info.chipset_id = businfo(c.chipset_id, true);
    info.behind_bridge = c.behind_bridge;
    info.upstream_l0s_unsupported = c.upstream_l0s_unsupported;
    info.upstream_l1_unsupported = c.upstream_l1_unsupported;
    info.upstream_l1_por_supported = c.upstream_l1_por_supported;
    info.upstream_l1_por_mobile_only = c.upstream_l1_por_mobile_only;
    info.upstream_address_valid = c.upstream.is_some();
    info.chipset_l1ss_enable = c.l1ss_control1;
    info.pcie_atomics_op_mask = c.atomics_op_mask;
    info.pcie_atomics_cpl_device_cap_mask = c.atomics_cpl_mask;
}
