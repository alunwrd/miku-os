use x86_64::instructions::port::Port;

pub const PCI_ADDR: u16 = 0xCF8;
pub const PCI_DATA: u16 = 0xCFC;

pub const VENDOR_INTEL: u16 = 0x8086;
pub const VENDOR_REALTEK: u16 = 0x10EC;
pub const VENDOR_VIRTIO: u16 = 0x1AF4;

pub const DEV_E1000_82540EM: u16 = 0x100E;
pub const DEV_E1000_82545EM: u16 = 0x100F;
pub const DEV_E1000_82574L: u16 = 0x10D3;
pub const DEV_E1000_82579LM: u16 = 0x1502;
pub const DEV_E1000_I217: u16 = 0x153A;
pub const DEV_RTL8139: u16 = 0x8139;
pub const DEV_RTL8168: u16 = 0x8168;
pub const DEV_RTL8169: u16 = 0x8169;
pub const DEV_VIRTIO_NET: u16 = 0x1000;
pub const DEV_VIRTIO_NET_MODERN: u16 = 0x1041;

#[derive(Clone, Copy, Debug)]
pub struct PciDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub bars: [u32; 6],
    pub irq: u8,
}

impl PciDevice {
    pub const fn empty() -> Self {
        Self {
            bus: 0,
            dev: 0,
            func: 0,
            vendor: 0xFFFF,
            device: 0xFFFF,
            class: 0,
            subclass: 0,
            bars: [0; 6],
            irq: 0,
        }
    }

    pub fn io_bar(&self, idx: usize) -> Option<u16> {
        if idx >= 6 {
            return None;
        }
        let bar = self.bars[idx];
        if bar & 1 != 0 {
            Some((bar & !3) as u16)
        } else {
            None
        }
    }

    pub fn mem_bar(&self, idx: usize) -> Option<u64> {
        if idx >= 6 {
            return None;
        }
        let bar = self.bars[idx];
        if bar & 1 == 0 && bar != 0 {
            let bar_type = (bar >> 1) & 3;
            if bar_type == 2 && idx + 1 < 6 {
                let lower = (bar & 0xFFFFFFF0) as u64;
                let upper = (self.bars[idx + 1] as u64) << 32;
                Some(lower | upper)
            } else {
                Some((bar & 0xFFFFFFF0) as u64)
            }
        } else {
            None
        }
    }

    pub fn enable_bus_mastering(&self) {
        let cmd = pci_read16(self.bus, self.dev, self.func, 0x04);
        pci_write16(
            self.bus,
            self.dev,
            self.func,
            0x04,
            cmd | 0x0004 | 0x0001 | 0x0002,
        );
    }
}

pub fn pci_addr(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC)
}

pub fn pci_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    unsafe {
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, offset));
        Port::<u32>::new(PCI_DATA).read()
    }
}

pub fn pci_read16(bus: u8, dev: u8, func: u8, offset: u8) -> u16 {
    (pci_read32(bus, dev, func, offset & !3) >> ((offset & 2) * 8)) as u16
}

pub fn pci_read8(bus: u8, dev: u8, func: u8, offset: u8) -> u8 {
    (pci_read32(bus, dev, func, offset & !3) >> ((offset & 3) * 8)) as u8
}

pub fn pci_write32(bus: u8, dev: u8, func: u8, offset: u8, val: u32) {
    unsafe {
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, offset));
        Port::<u32>::new(PCI_DATA).write(val);
    }
}

pub fn pci_write16(bus: u8, dev: u8, func: u8, offset: u8, val: u16) {
    let old = pci_read32(bus, dev, func, offset & !3);
    let shift = (offset & 2) * 8;
    pci_write32(
        bus,
        dev,
        func,
        offset & !3,
        (old & !(0xFFFF << shift)) | ((val as u32) << shift),
    );
}

pub fn scan() -> ([PciDevice; 32], usize) {
    let mut devices = [PciDevice::empty(); 32];
    let mut count = 0;
    for bus in 0..=255 {
        for dev in 0..32 {
            for func in 0..8 {
                let id = pci_read32(bus, dev, func, 0x00);
                if (id & 0xFFFF) as u16 == 0xFFFF {
                    if func == 0 {
                        break;
                    }
                    continue;
                }
                let class_rev = pci_read32(bus, dev, func, 0x08);
                if (class_rev >> 24) as u8 == 0x02 && count < 32 {
                    let mut bars = [0; 6];
                    for i in 0..6 {
                        bars[i] = pci_read32(bus, dev, func, 0x10 + (i as u8) * 4);
                    }
                    devices[count] = PciDevice {
                        bus,
                        dev,
                        func,
                        vendor: (id & 0xFFFF) as u16,
                        device: (id >> 16) as u16,
                        class: (class_rev >> 24) as u8,
                        subclass: (class_rev >> 16) as u8,
                        bars,
                        irq: pci_read8(bus, dev, func, 0x3C),
                    };
                    count += 1;
                }
                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 {
                    break;
                }
            }
        }
    }
    (devices, count)
}

pub fn find_nic() -> Option<PciDevice> {
    let (devs, n) = scan();
    devs.into_iter().take(n).find(|d| {
        d.vendor == VENDOR_INTEL || d.vendor == VENDOR_REALTEK || d.vendor == VENDOR_VIRTIO
    })
}

pub fn device_name(vendor: u16, device: u16) -> &'static str {
    match (vendor, device) {
        (VENDOR_REALTEK, DEV_RTL8168) => "RTL8168 Gigabit (r8168)",
        (VENDOR_REALTEK, DEV_RTL8169) => "RTL8169 Gigabit",
        (VENDOR_REALTEK, DEV_RTL8139) => "RTL8139 Fast Ethernet",
        (VENDOR_INTEL, DEV_E1000_82540EM) => "Intel e1000 82540EM",
        (VENDOR_INTEL, DEV_E1000_82545EM) => "Intel e1000 82545EM",
        (VENDOR_INTEL, DEV_E1000_82574L) => "Intel 82574L",
        (VENDOR_INTEL, DEV_E1000_82579LM) => "Intel 82579LM",
        (VENDOR_INTEL, DEV_E1000_I217) => "Intel I217",
        (VENDOR_VIRTIO, DEV_VIRTIO_NET) => "VirtIO Network",
        (VENDOR_VIRTIO, DEV_VIRTIO_NET_MODERN) => "VirtIO Network (modern, unsupported)",
        _ => "Unknown NIC",
    }
}
