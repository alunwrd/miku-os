// PCIe Function Level Reset (FLR) for NVIDIA GPUs
//
// Why this exists: on Hopper/Blackwell the GPU firmware (GFW / VBIOS devinit)
// runs on the on-die FSP at power-on and posts its progress into
// NV_PGC6_AON_SECURE_SCRATCH_GROUP_05(0). GSP-RM refuses to finish init
// (it asserts on GFW_BOOT_PROGRESS) unless that word reads 0xFF. If a prior
// boot attempt left the GPU half-initialised, a *warm* reboot does not clear
// it - the PGC6 AON scratch domain is kept alive across warm resets. A PCIe
// FLR re-runs the on-die init sequence from scratch, which is exactly what
// OpenRM means when it logs "the GPU may be in a bad state and may need to
// be reset"
//
// Reference: PCI Express Base Spec 4.0, section 6.6.2 (Function Level Reset)
// and section 7.5.3 (PCI Express Capability, Device Capabilities/Control/Status)

use crate::drivers::bus::pci::{pci_read8, pci_read16, pci_read32, pci_write16, pci_write32};
use crate::serial_println;
use crate::kcore::time::udelay;

use super::pci::{GpuDevice, CFG_CAP_PTR, CFG_COMMAND, CFG_STATUS, STATUS_CAP_LIST,
                 CMD_MEM_SPACE, CMD_BUS_MASTER};

/// PCI Express capability id
const CAP_ID_PCIE: u8 = 0x10;

// Offsets within the PCI Express capability structure
const PCIE_DEV_CAP:    u8 = 0x04; // Device Capabilities  (u32)
const PCIE_DEV_CTRL:   u8 = 0x08; // Device Control       (u16)
const PCIE_DEV_STATUS: u8 = 0x0A; // Device Status        (u16)

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
    let status = pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_STATUS);
    if status & STATUS_CAP_LIST == 0 {
        return None;
    }
    let mut ptr = pci_read8(gpu.bus, gpu.dev, gpu.func, CFG_CAP_PTR) & 0xFC;
    for _ in 0..48 {
        if ptr == 0 {
            break;
        }
        let cap_id = pci_read8(gpu.bus, gpu.dev, gpu.func, ptr);
        if cap_id == CAP_ID_PCIE {
            return Some(ptr);
        }
        let next = pci_read8(gpu.bus, gpu.dev, gpu.func, ptr + 1) & 0xFC;
        if next == ptr {
            break;
        }
        ptr = next;
    }
    None
}

/// Perform a Function Level Reset on the GPU
///
/// The type-0 config header (command, BARs, ROM BAR, interrupt line) is
/// cleared by the reset, so we snapshot dwords 0x04..0x40 first and write
/// them back afterwards. On success the GPU has re-run its power-on init
/// and GFW_BOOT_PROGRESS should march to 0xFF again
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
    // NVIDIA's own reset path waits longer to let the on-die devinit re-run;
    // give it 250 ms.
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
