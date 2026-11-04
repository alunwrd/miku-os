// PCI helpers for NVIDIA GPUs
//
// The bus walk belongs to `drivers::bus::pci`. Here is what a GPU needs on top:
// BAR sizing, the expansion-ROM window, and a `GpuDevice` carrying all of it

extern crate alloc;
use alloc::vec::Vec;

use crate::drivers::bus::pci::{pci_read32, pci_read8, pci_read16, pci_write16, pci_write32};

// Config-space map, complete, including offsets and bits not touched today
pub const CLASS_DISPLAY:      u8  = 0x03;
pub const SUBCLASS_VGA:       u8  = 0x00;
#[allow(dead_code)]
pub const SUBCLASS_3D:        u8  = 0x02;

// PCI config space offsets
pub const CFG_COMMAND:        u8  = 0x04;
#[allow(dead_code)]
pub const CFG_STATUS:         u8  = 0x06;
#[allow(dead_code)]
pub const CFG_BAR0:           u8  = 0x10;
pub const CFG_ROM_BASE:       u8  = 0x30;
#[allow(dead_code)]
pub const CFG_CAP_PTR:        u8  = 0x34;
pub const CFG_INTERRUPT_LINE: u8  = 0x3C;
pub const CFG_INTERRUPT_PIN:  u8  = 0x3D;

// Command register bits
pub const CMD_IO_SPACE:       u16 = 1 << 0;
pub const CMD_MEM_SPACE:      u16 = 1 << 1;
pub const CMD_BUS_MASTER:     u16 = 1 << 2;
pub const CMD_INTX_DISABLE:   u16 = 1 << 10;

// Status register bit 4: capability list present
#[allow(dead_code)]
pub const STATUS_CAP_LIST:    u16 = 1 << 4;

/// One decoded BAR, with every field the BAR says about itself: a prefetchable
/// aperture and a non-prefetchable one are different windows
#[allow(dead_code)]
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

/// A GPU as config space describes it, its identity like `Bar`
#[allow(dead_code)]
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

/// Every NVIDIA device of class 0x03 (display) on the bus.
///
/// BAR sizing and the expansion-ROM probe, which write config space and put it
/// back, stay per device; the walk comes from the shared enumeration
impl GpuDevice {
    /// Its address on the bus, as the bus layer's MSI code takes it
    pub fn pci_fn(&self) -> crate::drivers::bus::msi::PciFn {
        crate::drivers::bus::msi::PciFn { bus: self.bus, dev: self.dev, func: self.func }
    }
}

pub fn scan_nvidia() -> Vec<GpuDevice> {
    let candidates: Vec<_> = crate::drivers::bus::pci::with_devices(|devs| {
        devs.iter()
            .filter(|d| d.vendor == super::VENDOR_NVIDIA && d.class == CLASS_DISPLAY)
            .map(|d| (d.bus, d.dev, d.func))
            .collect()
    });

    let mut out = Vec::new();
    for (bus, dev, func) in candidates {
        let id = pci_read32(bus, dev, func, 0x00);
        let class_rev = pci_read32(bus, dev, func, 0x08);
        let bars = read_bars(bus, dev, func);
        let (rom_phys, rom_size) = probe_rom(bus, dev, func);
        let sub_id = pci_read32(bus, dev, func, 0x2C);

        out.push(GpuDevice {
            bus, dev, func,
            vendor: (id & 0xFFFF) as u16,
            device: (id >> 16) as u16,
            subsystem_vendor: (sub_id & 0xFFFF) as u16,
            subsystem_device: (sub_id >> 16) as u16,
            revision: (class_rev & 0xFF) as u8,
            class: (class_rev >> 24) as u8,
            subclass: (class_rev >> 16) as u8,
            bars,
            rom_phys,
            rom_size,
            irq_line: pci_read8(bus, dev, func, CFG_INTERRUPT_LINE),
            irq_pin:  pci_read8(bus, dev, func, CFG_INTERRUPT_PIN),
        });
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

/// Read the command register, to report the INTx-disable and bus-master bits
pub fn read_command(gpu: &GpuDevice) -> u16 {
    pci_read16(gpu.bus, gpu.dev, gpu.func, CFG_COMMAND)
}

/// Bridge Control (type-1 header) and its VGA Enable bit
const CFG_BRIDGE_CONTROL: u8 = 0x3E;
const BRIDGE_CTL_VGA: u16 = 1 << 3;
/// Secondary Bus Number (type-1 header)
const CFG_SECONDARY_BUS: u8 = 0x19;

/// Is this the legacy VGA device the firmware left routed?
///
/// OpenRM's `primary_vga` is whether Linux marked the VGA ROM as shadowed
/// (`nv_set_primary_vga_status`, `nv.c:4851`), and Linux does that in
/// `pci_fixup_video` for a VGA-class function that every bridge above it
/// forwards VGA to, with I/O or memory decode on. The same test, here
pub fn is_boot_vga(gpu: &GpuDevice) -> bool {
    if gpu.class != CLASS_DISPLAY || gpu.subclass != SUBCLASS_VGA {
        return false;
    }
    if read_command(gpu) & (CMD_IO_SPACE | CMD_MEM_SPACE) == 0 {
        return false;
    }
    // Up the tree: the bridge whose secondary bus is ours, then its own bus,
    // until no bridge is above. The host bridge has no Bridge Control register
    let mut bus = gpu.bus;
    for _ in 0..32 {
        let above = crate::drivers::bus::pci::find(|d| {
            d.class == 0x06 && d.subclass == 0x04
                && pci_read8(d.bus, d.dev, d.func, CFG_SECONDARY_BUS) == bus
        });
        let Some(bridge) = above else { return true };
        let ctl = pci_read16(bridge.bus, bridge.dev, bridge.func, CFG_BRIDGE_CONTROL);
        if ctl & BRIDGE_CTL_VGA == 0 {
            return false;
        }
        if bridge.bus == bus {
            return true;
        }
        bus = bridge.bus;
    }
    true
}
