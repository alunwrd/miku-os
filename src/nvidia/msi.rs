// PCI MSI / MSI-X capability reader
//
// This module only discovers and parses the MSI/MSI-X capability of a PCI
// device. Actually routing the interrupt still requires a platform-side
// allocator that hands out a (vector, LAPIC-id, address) triple, which our
// apic module does not yet expose. Once that exists, program_msi() below
// will be completed so GPU interrupts can fire.
//
// References:
//   PCI Local Bus Specification 3.0, section 6.8 (Message Signaled Interrupts) and 6.8.2 (MSI-X)
//   Intel SDM Volume 3A, section 10.11 (Message Signalled Interrupts)

use crate::net::pci::{pci_read8, pci_read16, pci_read32, pci_write16, pci_write32};
use crate::serial_println;

use super::pci::{GpuDevice, CFG_CAP_PTR, CFG_STATUS, STATUS_CAP_LIST};

// Capability IDs
pub const CAP_ID_MSI:   u8 = 0x05;
pub const CAP_ID_MSIX:  u8 = 0x11;

// MSI control register bits
const MSI_CTRL_ENABLE:       u16 = 1 << 0;
const MSI_CTRL_64BIT:        u16 = 1 << 7;
const MSI_CTRL_PERVECT_MASK: u16 = 1 << 8;
// Bits [3:1] = multi-message capable, [6:4] = multi-message enable (log2)

// MSI-X control register bits
const MSIX_CTRL_ENABLE:     u16 = 1 << 15;
const MSIX_CTRL_MASK_ALL:   u16 = 1 << 14;
// Table size (N-1) lives in bits [10:0] of the control word

#[derive(Clone, Copy, Debug)]
pub struct MsiCapability {
    pub cap_offset: u8,
    pub is_64bit: bool,
    pub per_vector_masking: bool,
    pub multi_message_capable: u8, // log2 of max vectors
}

#[derive(Clone, Copy, Debug)]
pub struct MsixCapability {
    pub cap_offset: u8,
    pub table_size: u16,     // total vectors (already +1)
    pub table_bir: u8,       // which BAR holds the table
    pub table_offset: u32,
    pub pba_bir: u8,
    pub pba_offset: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    pub msi: Option<MsiCapability>,
    pub msix: Option<MsixCapability>,
}

/// Walk the PCI capability list and pick out MSI and MSI-X. Returns an
/// empty struct if the device has no capabilities or neither MSI variant
pub fn read_caps(gpu: &GpuDevice) -> Capabilities {
    let mut out = Capabilities::default();
    let status = pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_STATUS);
    if status & STATUS_CAP_LIST == 0 {
        return out;
    }

    let mut ptr = pci_read8(gpu.bus, gpu.dev, gpu.func, CFG_CAP_PTR) & 0xFC;
    // Defensive cap: at most 48 chained capabilities
    for _ in 0..48 {
        if ptr == 0 { break; }
        let cap_id  = pci_read8(gpu.bus, gpu.dev, gpu.func, ptr);
        let next    = pci_read8(gpu.bus, gpu.dev, gpu.func, ptr + 1) & 0xFC;
        match cap_id {
            CAP_ID_MSI => {
                let ctrl = pci_read16(gpu.bus, gpu.dev, gpu.func, ptr + 2);
                out.msi = Some(MsiCapability {
                    cap_offset: ptr,
                    is_64bit: ctrl & MSI_CTRL_64BIT != 0,
                    per_vector_masking: ctrl & MSI_CTRL_PERVECT_MASK != 0,
                    multi_message_capable: ((ctrl >> 1) & 0x7) as u8,
                });
            }
            CAP_ID_MSIX => {
                let ctrl  = pci_read16(gpu.bus, gpu.dev, gpu.func, ptr + 2);
                let table = pci_read32(gpu.bus, gpu.dev, gpu.func, ptr + 4);
                let pba   = pci_read32(gpu.bus, gpu.dev, gpu.func, ptr + 8);
                out.msix = Some(MsixCapability {
                    cap_offset: ptr,
                    table_size: (ctrl & 0x7FF) + 1,
                    table_bir:     (table & 0x7) as u8,
                    table_offset:   table & !0x7,
                    pba_bir:       (pba & 0x7) as u8,
                    pba_offset:     pba & !0x7,
                });
            }
            _ => {}
        }
        if next == ptr { break; }
        ptr = next;
    }
    out
}

pub fn log_capabilities(caps: &Capabilities) {
    if let Some(m) = caps.msi {
        let max = 1u32 << m.multi_message_capable;
        serial_println!(
            "[nvidia] MSI: cap@{:#x} 64bit={} maskable={} max_vectors={}",
            m.cap_offset, m.is_64bit, m.per_vector_masking, max
        );
    }
    if let Some(x) = caps.msix {
        serial_println!(
            "[nvidia] MSI-X: cap@{:#x} table_size={} table_bar={} table_off={:#x} pba_bar={} pba_off={:#x}",
            x.cap_offset, x.table_size,
            x.table_bir, x.table_offset,
            x.pba_bir, x.pba_offset
        );
    }
    if caps.msi.is_none() && caps.msix.is_none() {
        serial_println!("[nvidia] no MSI or MSI-X capability present");
    }
}

/// Program a single-vector MSI to deliver to (address, data) as returned
/// from apic::alloc_msi_vector. Writes ADDR_LO / ADDR_HI / DATA into the
/// capability, clears pending bits if per-vector masking is present,
/// forces multi-message-enable to 0 (one vector), and sets ENABLE 
///
/// MSI capability layout (PCI 3.0, section 6.8.1):
///   cap+0x00  [u8]  capability id
///   cap+0x01  [u8]  next pointer
///   cap+0x02  [u16] message control
///   cap+0x04  [u32] message address low
///   cap+0x08  [u32] message address high     (only if 64-bit)
///   cap+0x08  [u16] message data             (if 32-bit)
///   cap+0x0C  [u16] message data             (if 64-bit)
///   cap+0x10  [u32] mask bits                (if per-vector-masking)
///   cap+0x14  [u32] pending bits             (if per-vector-masking)
pub fn program_msi(
    gpu: &GpuDevice,
    cap: &MsiCapability,
    address: u64,
    data: u32,
) -> Result<(), &'static str> {
    let off = cap.cap_offset;

    let mut ctrl = pci_read16(gpu.bus, gpu.dev, gpu.func, off + 2);
    // Disable while we reprogram it
    ctrl &= !MSI_CTRL_ENABLE;
    pci_write16(gpu.bus, gpu.dev, gpu.func, off + 2, ctrl);

    pci_write32(gpu.bus, gpu.dev, gpu.func, off + 4, (address & 0xFFFF_FFFF) as u32);
    if cap.is_64bit {
        pci_write32(gpu.bus, gpu.dev, gpu.func, off + 8, (address >> 32) as u32);
        // data word sits at +0x0C as a 16-bit register; the high half is reserved
        pci_write16(gpu.bus, gpu.dev, gpu.func, off + 0x0C, (data & 0xFFFF) as u16);
        if cap.per_vector_masking {
            // Unmask vector 0, clear pending
            pci_write32(gpu.bus, gpu.dev, gpu.func, off + 0x10, 0);
            pci_write32(gpu.bus, gpu.dev, gpu.func, off + 0x14, 0);
        }
    } else {
        pci_write16(gpu.bus, gpu.dev, gpu.func, off + 8, (data & 0xFFFF) as u16);
        if cap.per_vector_masking {
            pci_write32(gpu.bus, gpu.dev, gpu.func, off + 0x0C, 0);
            pci_write32(gpu.bus, gpu.dev, gpu.func, off + 0x10, 0);
        }
    }

    // Clear multi-message-enable ([6:4]) to 0 (one vector) and enable
    ctrl &= !(0x7 << 4);
    ctrl |= MSI_CTRL_ENABLE;
    pci_write16(gpu.bus, gpu.dev, gpu.func, off + 2, ctrl);
    Ok(())
}

/// Clear the MSI enable bit if it is currently set
pub fn disable_msi(gpu: &GpuDevice, cap: &MsiCapability) {
    let ctrl = pci_read16(gpu.bus, gpu.dev, gpu.func, cap.cap_offset + 2);
    if ctrl & MSI_CTRL_ENABLE != 0 {
        pci_write16(gpu.bus, gpu.dev, gpu.func, cap.cap_offset + 2, ctrl & !MSI_CTRL_ENABLE);
    }
}

/// Clear the MSI-X enable bit
pub fn disable_msix(gpu: &GpuDevice, cap: &MsixCapability) {
    let ctrl = pci_read16(gpu.bus, gpu.dev, gpu.func, cap.cap_offset + 2);
    if ctrl & MSIX_CTRL_ENABLE != 0 {
        let new = (ctrl & !MSIX_CTRL_ENABLE) | MSIX_CTRL_MASK_ALL;
        pci_write16(gpu.bus, gpu.dev, gpu.func, cap.cap_offset + 2, new);
    }
}
