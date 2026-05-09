// PCI helpers for NVIDIA GPUs
//
// net/pci.rs hardcodes class 0x02 (network) in its scan(), so we need a
// separate pass for display devices. The low-level pci_read/pci_write
// functions in net::pci are generic and reused here

extern crate alloc;
use alloc::vec::Vec;

use crate::net::pci::{pci_read32, pci_read8, pci_read16, pci_write16, pci_write32};

pub const CLASS_DISPLAY:      u8  = 0x03;
pub const SUBCLASS_VGA:       u8  = 0x00;
pub const SUBCLASS_3D:        u8  = 0x02;

// PCI config space offsets
pub const CFG_COMMAND:        u8  = 0x04;
pub const CFG_STATUS:         u8  = 0x06;
pub const CFG_BAR0:           u8  = 0x10;
pub const CFG_ROM_BASE:       u8  = 0x30;
pub const CFG_CAP_PTR:        u8  = 0x34;
pub const CFG_INTERRUPT_LINE: u8  = 0x3C;
pub const CFG_INTERRUPT_PIN:  u8  = 0x3D;

// Command register bits
pub const CMD_IO_SPACE:       u16 = 1 << 0;
pub const CMD_MEM_SPACE:      u16 = 1 << 1;
pub const CMD_BUS_MASTER:     u16 = 1 << 2;
pub const CMD_INTX_DISABLE:   u16 = 1 << 10;

// Status register bit 4: capability list present
pub const STATUS_CAP_LIST:    u16 = 1 << 4;

#[derive(Clone, Copy, Debug)]
pub struct Bar {
    pub phys: u64,
    pub size: u64,
    pub is_64bit: bool,
    pub is_prefetchable: bool,
    pub is_io: bool,
}

impl Bar {
    pub const fn empty() -> Self {
        Self { phys: 0, size: 0, is_64bit: false, is_prefetchable: false, is_io: false }
    }
}

#[derive(Clone, Debug)]
pub struct GpuDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
    pub revision: u8,
    pub class: u8,
    pub subclass: u8,
    pub bars: [Bar; 6],
    pub rom_phys: u64,
    pub rom_size: u64,
    pub irq_line: u8,
    pub irq_pin: u8,
}

/// Probe a BAR size by writing all ones and reading back. Restores the
/// original value afterwards
fn probe_bar_size(bus: u8, dev: u8, func: u8, bar_idx: u8, is_64bit: bool) -> (u32, u32, u64) {
    let offset = 0x10 + bar_idx * 4;
    let original_lo = pci_read32(bus, dev, func, offset);
    let original_hi = if is_64bit { pci_read32(bus, dev, func, offset + 4) } else { 0 };

    pci_write32(bus, dev, func, offset, 0xFFFF_FFFF);
    let size_lo = pci_read32(bus, dev, func, offset);
    let size_hi = if is_64bit {
        pci_write32(bus, dev, func, offset + 4, 0xFFFF_FFFF);
        pci_read32(bus, dev, func, offset + 4)
    } else { 0 };

    pci_write32(bus, dev, func, offset, original_lo);
    if is_64bit {
        pci_write32(bus, dev, func, offset + 4, original_hi);
    }

    let mask = if is_64bit {
        let low  = (size_lo & 0xFFFF_FFF0) as u64;
        let high = (size_hi as u64) << 32;
        low | high
    } else {
        (size_lo & 0xFFFF_FFF0) as u64 | 0xFFFF_FFFF_0000_0000
    };
    let size = if mask == 0 { 0 } else { (!mask).wrapping_add(1) };

    (original_lo, original_hi, size)
}

fn read_bars(bus: u8, dev: u8, func: u8) -> [Bar; 6] {
    let mut bars = [Bar::empty(); 6];
    let mut i = 0u8;
    while i < 6 {
        let raw = pci_read32(bus, dev, func, 0x10 + i * 4);
        if raw == 0 { i += 1; continue; }

        let is_io = raw & 1 != 0;
        if is_io {
            let (orig, _, size) = probe_bar_size(bus, dev, func, i, false);
            bars[i as usize] = Bar {
                phys: (orig & !3) as u64,
                size,
                is_64bit: false,
                is_prefetchable: false,
                is_io: true,
            };
            i += 1;
            continue;
        }

        let bar_type = (raw >> 1) & 3;
        let is_64bit = bar_type == 2;
        let prefetch = raw & (1 << 3) != 0;
        let (orig_lo, orig_hi, size) = probe_bar_size(bus, dev, func, i, is_64bit);
        let phys = if is_64bit {
            ((orig_lo & 0xFFFF_FFF0) as u64) | ((orig_hi as u64) << 32)
        } else {
            (orig_lo & 0xFFFF_FFF0) as u64
        };
        bars[i as usize] = Bar { phys, size, is_64bit, is_prefetchable: prefetch, is_io: false };
        i += if is_64bit { 2 } else { 1 };
    }
    bars
}

/// Probe the expansion ROM BAR (config offset 0x30). Returns (phys, size)
/// The enable bit (bit 0) is left cleared; the caller sets it when mapping
fn probe_rom(bus: u8, dev: u8, func: u8) -> (u64, u64) {
    let original = pci_read32(bus, dev, func, CFG_ROM_BASE);
    pci_write32(bus, dev, func, CFG_ROM_BASE, 0xFFFF_F800);
    let size_raw = pci_read32(bus, dev, func, CFG_ROM_BASE);
    pci_write32(bus, dev, func, CFG_ROM_BASE, original);

    let mask = size_raw & 0xFFFF_F800;
    let size = if mask == 0 { 0 } else { (!(mask as u64) & 0xFFFF_FFFF).wrapping_add(1) };
    let phys = (original as u64) & 0xFFFF_F800;
    (phys, size)
}

/// Scan the PCI bus and return every NVIDIA device of class 0x03 (display)
pub fn scan_nvidia() -> Vec<GpuDevice> {
    let mut out = Vec::new();
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = pci_read32(bus, dev, func, 0x00);
                let vendor = (id & 0xFFFF) as u16;
                if vendor == 0xFFFF {
                    if func == 0 { break; } else { continue; }
                }
                if vendor != super::VENDOR_NVIDIA {
                    if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 { break; }
                    continue;
                }

                let class_rev = pci_read32(bus, dev, func, 0x08);
                let class = (class_rev >> 24) as u8;
                if class != CLASS_DISPLAY {
                    continue;
                }

                let subclass = (class_rev >> 16) as u8;
                let revision = (class_rev & 0xFF) as u8;
                let device_id = (id >> 16) as u16;
                let bars = read_bars(bus, dev, func);
                let (rom_phys, rom_size) = probe_rom(bus, dev, func);
                let irq_line = pci_read8(bus, dev, func, CFG_INTERRUPT_LINE);
                let irq_pin  = pci_read8(bus, dev, func, CFG_INTERRUPT_PIN);
                let sub_id = pci_read32(bus, dev, func, 0x2C);

                out.push(GpuDevice {
                    bus, dev, func,
                    vendor,
                    device: device_id,
                    subsystem_vendor: (sub_id & 0xFFFF) as u16,
                    subsystem_device: (sub_id >> 16) as u16,
                    revision,
                    class,
                    subclass,
                    bars,
                    rom_phys,
                    rom_size,
                    irq_line,
                    irq_pin,
                });

                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 { break; }
            }
        }
    }
    out
}

/// Enable memory-space decoding and bus mastering in the command register
pub fn enable_memory_and_bus_master(gpu: &GpuDevice) {
    let cmd = pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND);
    let new = cmd | CMD_MEM_SPACE | CMD_BUS_MASTER;
    pci_write16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND, new);
}

/// Mask legacy INTx delivery (bit 10 of COMMAND). Call this when using MSI
pub fn disable_intx(gpu: &GpuDevice) {
    let cmd = pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND);
    pci_write16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND, cmd | CMD_INTX_DISABLE);
}

/// Read the PCI status register
pub fn read_status(gpu: &GpuDevice) -> u16 {
    pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_STATUS)
}
