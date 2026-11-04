// PCI MSI and MSI-X: finding the capabilities and programming them
//
// The platform half (a vector, the LAPIC it reaches, the (address, data) pair
// naming both) is `apic::alloc_msi_vector`. This is the device half, for any
// PCI function.
//
// - MSI: one message, written into the capability itself. The GPU uses it: one
//   vector carries its whole interrupt tree.
// - MSI-X: a table of messages in one of the device's BARs, one entry per
//   source. An NVMe controller uses it: an entry per completion queue.
//
// References:
//   PCI Local Bus Specification 3.0, 6.8 (MSI) and 6.8.2 (MSI-X)
//   Intel SDM Volume 3A, 10.11 (Message Signalled Interrupts)

use crate::drivers::bus::pci::{pci_read16, pci_read32, pci_write16, pci_write32};
use crate::serial_println;

pub use crate::drivers::bus::pci::{CAP_ID_MSI, CAP_ID_MSIX, CAP_ID_PCIE};

// MSI control register bits
const MSI_CTRL_ENABLE:       u16 = 1 << 0;
const MSI_CTRL_64BIT:        u16 = 1 << 7;
const MSI_CTRL_PERVECT_MASK: u16 = 1 << 8;
// Bits [3:1] = multi-message capable, [6:4] = multi-message enable (log2)

// MSI-X control register bits
const MSIX_CTRL_ENABLE:     u16 = 1 << 15;
const MSIX_CTRL_MASK_ALL:   u16 = 1 << 14;
// Table size (N-1) lives in bits [10:0] of the control word

/// One MSI-X table entry: address low, address high, data, vector control
const MSIX_ENTRY_BYTES: u64 = 16;
const MSIX_VECTOR_MASKED: u32 = 1;

/// Command register bit 10: the legacy INTx pin, off once messages are on
const PCI_CMD_INTX_DISABLE: u16 = 1 << 10;

/// A PCI function's address on the bus
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciFn {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}

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
    /// Offset of the PCI Express capability, if the device has one
    pub pcie: Option<u8>,
}

/// The PCI Express link as the device sees it now and at best: generation (1 =
/// 2.5 GT/s ... 5 = 32 GT/s) and lanes. Read live: a GPU lowers its own link at
/// idle and raises it under load, and Gen1 is a sixteenth of the bandwidth
/// every read of system memory has
#[derive(Clone, Copy, Debug)]
pub struct PcieLink {
    pub gen: u8,
    pub width: u8,
    pub max_gen: u8,
    pub max_width: u8,
}

/// Link Capabilities at +0x0C and Link Status at +0x12 of the PCI Express
/// capability (PCIe base spec 7.5.3.6 / 7.5.3.8): speed in 3:0, width in 9:4
pub fn pcie_link(f: PciFn, caps: &Capabilities) -> Option<PcieLink> {
    let at = caps.pcie?;
    let lnkcap = pci_read32(f.bus, f.dev, f.func, at + 0x0C);
    let lnksta = pci_read16(f.bus, f.dev, f.func, at + 0x12);
    Some(PcieLink {
        gen: (lnksta & 0xF) as u8,
        width: ((lnksta >> 4) & 0x3F) as u8,
        max_gen: (lnkcap & 0xF) as u8,
        max_width: ((lnkcap >> 4) & 0x3F) as u8,
    })
}

/// Walk the PCI capability list and pick out MSI, MSI-X and PCI Express.
/// Empty if the device has none of them
pub fn read_caps(f: PciFn) -> Capabilities {
    let mut out = Capabilities::default();
    for (cap_id, ptr) in crate::drivers::bus::pci::caps(f.bus, f.dev, f.func) {
        match cap_id {
            CAP_ID_MSI => {
                let ctrl = pci_read16(f.bus, f.dev, f.func, ptr + 2);
                out.msi = Some(MsiCapability {
                    cap_offset: ptr,
                    is_64bit: ctrl & MSI_CTRL_64BIT != 0,
                    per_vector_masking: ctrl & MSI_CTRL_PERVECT_MASK != 0,
                    multi_message_capable: ((ctrl >> 1) & 0x7) as u8,
                });
            }
            CAP_ID_MSIX => {
                let ctrl  = pci_read16(f.bus, f.dev, f.func, ptr + 2);
                let table = pci_read32(f.bus, f.dev, f.func, ptr + 4);
                let pba   = pci_read32(f.bus, f.dev, f.func, ptr + 8);
                out.msix = Some(MsixCapability {
                    cap_offset: ptr,
                    table_size: (ctrl & 0x7FF) + 1,
                    table_bir:     (table & 0x7) as u8,
                    table_offset:   table & !0x7,
                    pba_bir:       (pba & 0x7) as u8,
                    pba_offset:     pba & !0x7,
                });
            }
            CAP_ID_PCIE => out.pcie = Some(ptr),
            _ => {}
        }
    }
    out
}

/// One line per capability on the serial log, under the driver's `tag`
pub fn log_capabilities(tag: &str, caps: &Capabilities) {
    if let Some(m) = caps.msi {
        let max = 1u32 << m.multi_message_capable;
        serial_println!(
            "[{}] MSI: cap@{:#x} 64bit={} maskable={} max_vectors={}",
            tag, m.cap_offset, m.is_64bit, m.per_vector_masking, max
        );
    }
    if let Some(x) = caps.msix {
        serial_println!(
            "[{}] MSI-X: cap@{:#x} table_size={} table_bar={} table_off={:#x} pba_bar={} pba_off={:#x}",
            tag, x.cap_offset, x.table_size,
            x.table_bir, x.table_offset,
            x.pba_bir, x.pba_offset
        );
    }
    if caps.msi.is_none() && caps.msix.is_none() {
        serial_println!("[{}] no MSI or MSI-X capability present", tag);
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
    f: PciFn,
    cap: &MsiCapability,
    address: u64,
    data: u32,
) -> Result<(), &'static str> {
    let off = cap.cap_offset;

    let mut ctrl = pci_read16(f.bus, f.dev, f.func, off + 2);
    // Disable while we reprogram it
    ctrl &= !MSI_CTRL_ENABLE;
    pci_write16(f.bus, f.dev, f.func, off + 2, ctrl);

    pci_write32(f.bus, f.dev, f.func, off + 4, (address & 0xFFFF_FFFF) as u32);
    if cap.is_64bit {
        pci_write32(f.bus, f.dev, f.func, off + 8, (address >> 32) as u32);
        // data word sits at +0x0C as a 16-bit register; the high half is
        // reserved
        pci_write16(f.bus, f.dev, f.func, off + 0x0C, (data & 0xFFFF) as u16);
        if cap.per_vector_masking {
            // Unmask vector 0, clear pending
            pci_write32(f.bus, f.dev, f.func, off + 0x10, 0);
            pci_write32(f.bus, f.dev, f.func, off + 0x14, 0);
        }
    } else {
        pci_write16(f.bus, f.dev, f.func, off + 8, (data & 0xFFFF) as u16);
        if cap.per_vector_masking {
            pci_write32(f.bus, f.dev, f.func, off + 0x0C, 0);
            pci_write32(f.bus, f.dev, f.func, off + 0x10, 0);
        }
    }

    // Clear multi-message-enable ([6:4]) to 0 (one vector) and enable
    ctrl &= !(0x7 << 4);
    ctrl |= MSI_CTRL_ENABLE;
    pci_write16(f.bus, f.dev, f.func, off + 2, ctrl);
    Ok(())
}

/// Clear the MSI enable bit if it is currently set. What an unload does
/// before it gives the vector back
pub fn disable_msi(f: PciFn, cap: &MsiCapability) {
    let ctrl = pci_read16(f.bus, f.dev, f.func, cap.cap_offset + 2);
    if ctrl & MSI_CTRL_ENABLE != 0 {
        pci_write16(f.bus, f.dev, f.func, cap.cap_offset + 2, ctrl & !MSI_CTRL_ENABLE);
    }
}

/// An MSI-X table, mapped: entries are written through `set`
pub struct MsixTable {
    base: u64,
    entries: u16,
}

/// Map `cap`'s table out of the BAR it lives in. `None` when that BAR is
/// not a memory BAR
pub fn map_msix(f: PciFn, cap: &MsixCapability) -> Option<MsixTable> {
    let dev = crate::drivers::bus::pci::find(|d| d.bus == f.bus && d.dev == f.dev && d.func == f.func)?;
    let bar = dev.mem_bar(cap.table_bir as usize)?;
    let phys = bar + cap.table_offset as u64;
    let bytes = cap.table_size as u64 * MSIX_ENTRY_BYTES;
    crate::mm::vmm::map_mmio_uc(phys, bytes);
    Some(MsixTable { base: crate::arch::x86_64::grub::phys_to_virt(phys), entries: cap.table_size })
}

impl MsixTable {
    pub fn len(&self) -> u16 {
        self.entries
    }

    /// Point entry `i` at (address, data) and unmask it
    pub fn set(&self, i: u16, address: u64, data: u32) {
        if i >= self.entries {
            return;
        }
        let e = self.base + i as u64 * MSIX_ENTRY_BYTES;
        unsafe {
            core::ptr::write_volatile((e + 12) as *mut u32, MSIX_VECTOR_MASKED);
            core::ptr::write_volatile(e as *mut u32, address as u32);
            core::ptr::write_volatile((e + 4) as *mut u32, (address >> 32) as u32);
            core::ptr::write_volatile((e + 8) as *mut u32, data);
            core::ptr::write_volatile((e + 12) as *mut u32, 0);
        }
    }

    /// Mask entry `i`, for a source that is going away
    pub fn mask(&self, i: u16) {
        if i < self.entries {
            let e = self.base + i as u64 * MSIX_ENTRY_BYTES;
            unsafe { core::ptr::write_volatile((e + 12) as *mut u32, MSIX_VECTOR_MASKED) };
        }
    }
}

/// MSI-X on and the function-wide mask off, and the INTx pin off with it:
/// every entry set with `MsixTable::set` delivers from here on
pub fn enable_msix(f: PciFn, cap: &MsixCapability) {
    let cmd = pci_read16(f.bus, f.dev, f.func, 0x04);
    pci_write16(f.bus, f.dev, f.func, 0x04, cmd | PCI_CMD_INTX_DISABLE);
    let ctrl = pci_read16(f.bus, f.dev, f.func, cap.cap_offset + 2);
    pci_write16(f.bus, f.dev, f.func, cap.cap_offset + 2, (ctrl | MSIX_CTRL_ENABLE) & !MSIX_CTRL_MASK_ALL);
}

/// Clear the MSI-X enable bit, masking the function as it goes
pub fn disable_msix(f: PciFn, cap: &MsixCapability) {
    let ctrl = pci_read16(f.bus, f.dev, f.func, cap.cap_offset + 2);
    if ctrl & MSIX_CTRL_ENABLE != 0 {
        let new = (ctrl & !MSIX_CTRL_ENABLE) | MSIX_CTRL_MASK_ALL;
        pci_write16(f.bus, f.dev, f.func, cap.cap_offset + 2, new);
    }
}
