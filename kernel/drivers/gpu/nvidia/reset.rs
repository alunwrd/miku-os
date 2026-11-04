// PCIe Function Level Reset (FLR) for NVIDIA GPUs
//
// On Hopper/Blackwell the GPU firmware (GFW / VBIOS devinit) runs on the on-die
// FSP at power-on and posts its progress into
// NV_PGC6_AON_SECURE_SCRATCH_GROUP_05(0). GSP-RM asserts on GFW_BOOT_PROGRESS
// unless that word reads 0xFF. After a half-finished boot, a warm reboot does
// not clear it: the PGC6 AON scratch domain lives across warm resets. A PCIe
// FLR re-runs the on-die init sequence, what OpenRM means by "the GPU may be in
// a bad state and may need to be reset".
//
// Reference: PCI Express Base Spec 4.0, section 6.6.2 (Function Level Reset)
// and section 7.5.3 (PCI Express Capability, Device
// Capabilities/Control/Status)

use crate::drivers::bus::pci::{pci_read16, pci_read32, pci_write16, pci_write32};
use crate::serial_println;
use crate::kcore::time::udelay;

use super::pci::{GpuDevice, CFG_COMMAND, CMD_MEM_SPACE, CMD_BUS_MASTER};

use crate::drivers::bus::pci::CAP_ID_PCIE;

// Offsets within the PCI Express capability structure
const PCIE_DEV_CAP:    u8 = 0x04; // Device Capabilities  (u32)
const PCIE_DEV_CTRL:   u8 = 0x08; // Device Control       (u16)
const PCIE_DEV_STATUS: u8 = 0x0A; // Device Status        (u16)
const PCIE_LINK_CAP:   u8 = 0x0C; // Link Capabilities    (u32)
const PCIE_DEV_CTRL2:  u8 = 0x28; // Device Control 2     (u16, read as the dword)

/// Device Capabilities bit 28: Function Level Reset Capable
const DEVCAP_FLR_CAPABLE: u32 = 1 << 28;
/// Device Control bit 15: Initiate Function Level Reset
const DEVCTRL_INITIATE_FLR: u16 = 1 << 15;
/// Device Status bit 5: Transactions Pending
const DEVSTATUS_TRANS_PENDING: u16 = 1 << 5;

#[derive(Debug)]
pub enum ResetError {
    NoPcieCap,
    NotFlrCapable,
    /// Device did not come back after the reset (config reads 0xFFFF_FFFF)
    DeviceGone,
}

/// Locate the PCI Express capability (id 0x10) in the config-space cap list
fn find_pcie_cap(gpu: &GpuDevice) -> Option<u8> {
    crate::drivers::bus::pci::find_cap(gpu.bus, gpu.dev, gpu.func, CAP_ID_PCIE)
}

/// The PCI Express capability fields GSP-RM is told at GSP_INIT
#[derive(Clone, Copy, Debug, Default)]
pub struct PcieCaps {
    /// Device Capabilities bit 28 (`PDB_PROP_KBIF_FLR_SUPPORTED`)
    pub flr: bool,
    /// The Link Capabilities dword (`pcieConfigReg.linkCap`)
    pub link_cap: u32,
    /// The Device Control 2 dword (`pf0DeviceControl2Reg`,
    /// `kbifCacheDeviceControlStatus2Reg_GB100`)
    pub dev_ctrl2: u32,
}

/// Read them. None when the device has no PCIe capability
pub fn pcie_caps(gpu: &GpuDevice) -> Option<PcieCaps> {
    let cap = find_pcie_cap(gpu)?;
    let dev_cap = pci_read32(gpu.bus, gpu.dev, gpu.func, cap + PCIE_DEV_CAP);
    Some(PcieCaps {
        flr: dev_cap & DEVCAP_FLR_CAPABLE != 0,
        link_cap: pci_read32(gpu.bus, gpu.dev, gpu.func, cap + PCIE_LINK_CAP),
        dev_ctrl2: pci_read32(gpu.bus, gpu.dev, gpu.func, cap + PCIE_DEV_CTRL2),
    })
}

/// Perform a Function Level Reset on the GPU.
///
/// The reset clears the type-0 config header (command, BARs, ROM BAR, interrupt
/// line), so dwords 0x04..0x40 are saved first and written back afterwards. On
/// success the GPU has re-run its power-on init and GFW_BOOT_PROGRESS should
/// march to 0xFF again
pub fn function_level_reset(gpu: &GpuDevice) -> Result<(), ResetError> {
    let cap = find_pcie_cap(gpu).ok_or(ResetError::NoPcieCap)?;

    let dev_cap = pci_read32(gpu.bus, gpu.dev, gpu.func, cap + PCIE_DEV_CAP);
    if dev_cap & DEVCAP_FLR_CAPABLE == 0 {
        return Err(ResetError::NotFlrCapable);
    }

    serial_println!(
        "[nvidia] FLR: pcie-cap@{:#x} on {:02x}:{:02x}.{}",
        cap, gpu.bus, gpu.dev, gpu.func
    );

    // Snapshot the standard header (dwords 0x04..0x40). 0x00 is read-only
    // vendor/device, so we start at 0x04
    let mut saved = [0u32; 15];
    for (i, slot) in saved.iter_mut().enumerate() {
        *slot = pci_read32(gpu.bus, gpu.dev, gpu.func, 0x04 + (i as u8) * 4);
    }

    // Quiesce: drop bus-mastering so no new DMA is issued, then wait for any
    // in-flight transactions to drain (Device Status, Transactions Pending)
    let cmd = pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND);
    pci_write16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND, cmd & !CMD_BUS_MASTER);

    for _ in 0..1000 {
        let st = pci_read16(gpu.bus, gpu.dev, gpu.func, cap + PCIE_DEV_STATUS);
        if st & DEVSTATUS_TRANS_PENDING == 0 {
            break;
        }
        udelay(1000); // up to ~1 s total
    }

    // Fire the reset.
    let ctrl = pci_read16(gpu.bus, gpu.dev, gpu.func, cap + PCIE_DEV_CTRL);
    pci_write16(gpu.bus, gpu.dev, gpu.func, cap + PCIE_DEV_CTRL,
                ctrl | DEVCTRL_INITIATE_FLR);

    // PCIe spec mandates at least 100 ms before touching the function again.
    // NVIDIA's own reset path waits longer for the on-die devinit; 250 ms here.
    udelay(250_000);

    // Wait for config space to come back (vendor id no longer all-ones)
    let mut back = false;
    for _ in 0..1000 {
        let vid = pci_read16(gpu.bus, gpu.dev, gpu.func, 0x00);
        if vid != 0xFFFF {
            back = true;
            break;
        }
        udelay(1000); // up to ~1 s more
    }
    if !back {
        return Err(ResetError::DeviceGone);
    }

    // Restore the saved header so BAR addresses / command bits are valid again.
    for (i, val) in saved.iter().enumerate() {
        pci_write32(gpu.bus, gpu.dev, gpu.func, 0x04 + (i as u8) * 4, *val);
    }

    // Make sure memory decode + bus mastering are on for the driver
    let cmd = pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND);
    pci_write16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND,
                cmd | CMD_MEM_SPACE | CMD_BUS_MASTER);

    serial_println!("[nvidia] FLR: complete, device back online");
    Ok(())
}
