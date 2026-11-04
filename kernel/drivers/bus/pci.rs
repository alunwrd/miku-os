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
    /// Register 0x09. Class and subclass cannot tell a USB controller
    /// generation apart, or an NVMe controller from an AHCI one: both are in
    /// class 01 and differ only here
    pub prog_if: u8,
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
            prog_if: 0,
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

/// Read one device's header, if anything answers at that address
fn probe(bus: u8, dev: u8, func: u8) -> Option<PciDevice> {
    let id = pci_read32(bus, dev, func, 0x00);
    if (id & 0xFFFF) as u16 == 0xFFFF {
        return None;
    }
    let class_rev = pci_read32(bus, dev, func, 0x08);
    let mut bars = [0; 6];
    for (i, b) in bars.iter_mut().enumerate() {
        *b = pci_read32(bus, dev, func, 0x10 + (i as u8) * 4);
    }
    Some(PciDevice {
        bus,
        dev,
        func,
        vendor: (id & 0xFFFF) as u16,
        device: (id >> 16) as u16,
        class: (class_rev >> 24) as u8,
        subclass: (class_rev >> 16) as u8,
        prog_if: (class_rev >> 8) as u8,
        bars,
        irq: pci_read8(bus, dev, func, 0x3C),
    })
}

// One enumeration, kept
//
// A sweep of 0..=255 x 0..32 x 0..8 is 8192 config-space reads before
// multi-function expansion, and the bus does not change between callers. So
// sweep once and let everyone filter the result. A header-type without bit 7
// has no functions 1..7

static DEVICES: spin::Mutex<alloc::vec::Vec<PciDevice>> = spin::Mutex::new(alloc::vec::Vec::new());
static ENUMERATED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Header type register, with the multi-function bit stripped off
fn header_type(bus: u8, dev: u8, func: u8) -> u8 {
    pci_read8(bus, dev, func, 0x0E)
}

/// Every function answering on one bus, appended to `out`. Bridges found on
/// the way have their secondary bus numbers pushed onto `queue`; a bridge
/// whose secondary bus number was never programmed sets `unprogrammed`
fn sweep_bus(
    bus: u8,
    out: &mut alloc::vec::Vec<PciDevice>,
    queue: &mut alloc::vec::Vec<u8>,
    unprogrammed: &mut bool,
) {
    for dev in 0..32u8 {
        for func in 0..8u8 {
            let Some(d) = probe(bus, dev, func) else {
                // Nothing at function 0 means nothing at this device at all
                if func == 0 { break; }
                continue;
            };
            let hdr = header_type(bus, dev, func);
            // Header type 1 is a PCI-to-PCI bridge; register 0x19 is the bus
            // number on its far side, how the buses past 0 are reached
            if hdr & 0x7F == 0x01 {
                let secondary = pci_read8(bus, dev, func, 0x19);
                if secondary != 0 && secondary != 0xFF && secondary != bus {
                    queue.push(secondary);
                } else if secondary == 0 || secondary == 0xFF {
                    *unprogrammed = true;
                }
            }
            out.push(d);
            // A device that is not multi-function has no function 1..7
            if func == 0 && hdr & 0x80 == 0 {
                break;
            }
        }
    }
}

/// Walk every bus reachable from the host bridge.
///
/// A PC has two or three buses, and the host bridge answers for the other 253
/// with all-ones, so following the bridges visits only the buses that exist.
///
/// Firmware that never programmed the bridge bus numbers leaves a bridge
/// pointing at bus 0 or 0xFF. That is detected and falls back to a full sweep
fn sweep() -> alloc::vec::Vec<PciDevice> {
    let mut out = alloc::vec::Vec::new();
    let mut visited = [false; 256];
    let mut queue = alloc::vec::Vec::new();
    let mut unprogrammed_bridge = false;

    // A multi-function device at 00:00 is several host bridges, one per
    // function, and function f owns bus f
    if header_type(0, 0, 0) & 0x80 != 0 {
        for f in 0..8u8 {
            if probe(0, 0, f).is_some() {
                queue.push(f);
            }
        }
    } else {
        queue.push(0);
    }

    while let Some(bus) = queue.pop() {
        if visited[bus as usize] { continue; }
        visited[bus as usize] = true;
        sweep_bus(bus, &mut out, &mut queue, &mut unprogrammed_bridge);
    }

    if !unprogrammed_bridge {
        return out;
    }

    crate::serial_println!(
        "[pci] a bridge has no secondary bus number, falling back to a full 0..255 sweep");
    out.clear();
    let mut ignored = alloc::vec::Vec::new();
    let mut ignored_flag = false;
    for bus in 0..=255u8 {
        sweep_bus(bus, &mut out, &mut ignored, &mut ignored_flag);
    }
    out
}

/// Walk the bus now, so the cost lands in a boot step of its own
pub fn enumerate() -> usize {
    let mut g = DEVICES.lock();
    *g = sweep();
    ENUMERATED.store(true, core::sync::atomic::Ordering::Release);
    g.len()
}

/// Run a closure over every enumerated function, sweeping the bus first if
/// nobody has yet.
///
/// The closure must not call back in here: the list is behind one lock
pub fn with_devices<R>(f: impl FnOnce(&[PciDevice]) -> R) -> R {
    let mut g = DEVICES.lock();
    if !ENUMERATED.load(core::sync::atomic::Ordering::Acquire) {
        *g = sweep();
        ENUMERATED.store(true, core::sync::atomic::Ordering::Release);
    }
    f(g.as_slice())
}

/// The first function matching `pred`, in bus order
pub fn find(pred: impl Fn(&PciDevice) -> bool) -> Option<PciDevice> {
    with_devices(|devs| devs.iter().find(|d| pred(d)).copied())
}

/// Every function matching `pred`, up to `out.len()`. Returns how many
/// landed in `out`
pub fn find_all(pred: impl Fn(&PciDevice) -> bool, out: &mut [PciDevice]) -> usize {
    with_devices(|devs| {
        let mut n = 0;
        for d in devs.iter().filter(|d| pred(d)) {
            if n == out.len() { break; }
            out[n] = *d;
            n += 1;
        }
        n
    })
}

/// Every enumerated function, copied out
pub fn devices() -> alloc::vec::Vec<PciDevice> {
    with_devices(|d| d.to_vec())
}

/// The first device with this vendor and device id, whatever class it is.
///
/// `scan` below keeps only class 0x02, network controllers; a display is class
/// 0x03
pub fn find_by_id(vendor: u16, device: u16) -> Option<PciDevice> {
    find(|d| d.vendor == vendor && d.device == device)
}

/// Every network controller on the bus. Class 0x02 only; see `find_by_id` for
/// anything else
pub fn scan() -> ([PciDevice; 32], usize) {
    let mut devices = [PciDevice::empty(); 32];
    let n = find_all(|d| d.class == 0x02, &mut devices);
    (devices, n)
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

// Capabilities, extended config space, and where a device hangs
//
// Extended config space, everything past 256 bytes (AER, L1 PM substates, the
// PCIe 4.0/5.0 physical layer), is reachable only through ECAM, whose windows
// the ACPI `MCFG` table gives (`acpi::ecam_regions`). Without that table the
// legacy 0xCF8 mechanism is all there is, and the `_ext` calls say `None`

/// Capability ids (PCI Local Bus 3.0, appendix H)
pub const CAP_ID_PM: u8 = 0x01;
pub const CAP_ID_MSI: u8 = 0x05;
pub const CAP_ID_PCIE: u8 = 0x10;
pub const CAP_ID_MSIX: u8 = 0x11;

/// Extended capability ids (PCIe Base 6.0, 7.6)
pub const EXT_CAP_AER: u16 = 0x0001;
pub const EXT_CAP_LTR: u16 = 0x0018;
pub const EXT_CAP_L1SS: u16 = 0x001E;

/// Status register bit 4: the capability list exists
const STATUS_CAP_LIST: u16 = 1 << 4;

/// Every capability in a function's standard list, as `(id, offset)`, in list
/// order. At most 48: a list that loops is a broken device
pub fn caps(bus: u8, dev: u8, func: u8) -> alloc::vec::Vec<(u8, u8)> {
    let mut out = alloc::vec::Vec::new();
    if pci_read16(bus, dev, func, 0x06) & STATUS_CAP_LIST == 0 {
        return out;
    }
    let mut ptr = pci_read8(bus, dev, func, 0x34) & 0xFC;
    for _ in 0..48 {
        if ptr == 0 {
            break;
        }
        let id = pci_read8(bus, dev, func, ptr);
        out.push((id, ptr));
        let next = pci_read8(bus, dev, func, ptr + 1) & 0xFC;
        if next == ptr {
            break;
        }
        ptr = next;
    }
    out
}

/// Where capability `id` starts, if the function has it
pub fn find_cap(bus: u8, dev: u8, func: u8, id: u8) -> Option<u8> {
    caps(bus, dev, func).into_iter().find(|(c, _)| *c == id).map(|(_, p)| p)
}

/// Buses whose ECAM window has been mapped, one bit each. A bus is mapped the
/// first time anything asks about it, 1 MiB, uncached
static ECAM_MAPPED: [core::sync::atomic::AtomicU64; 4] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];

/// The virtual address of one function's 4 KiB of ECAM, segment 0
fn ecam_at(bus: u8, dev: u8, func: u8) -> Option<u64> {
    if dev >= 32 || func >= 8 {
        return None;
    }
    let region = crate::arch::x86_64::acpi::ecam_regions()
        .into_iter()
        .find(|r| r.segment == 0 && bus >= r.start_bus && bus <= r.end_bus)?;
    let bus_phys = region.base + (((bus - region.start_bus) as u64) << 20);
    let word = &ECAM_MAPPED[bus as usize / 64];
    let bit = 1u64 << (bus % 64);
    if word.load(core::sync::atomic::Ordering::Acquire) & bit == 0 {
        crate::mm::vmm::map_mmio_uc(bus_phys, 1 << 20);
        word.fetch_or(bit, core::sync::atomic::Ordering::AcqRel);
    }
    let phys = bus_phys + ((dev as u64) << 15) + ((func as u64) << 12);
    Some(crate::arch::x86_64::grub::phys_to_virt(phys))
}

/// Whether extended config space can be reached at all
pub fn has_ecam() -> bool {
    !crate::arch::x86_64::acpi::ecam_regions().is_empty()
}

/// A dword of config space through ECAM, `off` up to 4 KiB. `None` without an
/// `MCFG` window for this bus
pub fn read32_ext(bus: u8, dev: u8, func: u8, off: u16) -> Option<u32> {
    if off as usize + 4 > 4096 || off & 3 != 0 {
        return None;
    }
    let at = ecam_at(bus, dev, func)?;
    Some(unsafe { core::ptr::read_volatile((at + off as u64) as *const u32) })
}

/// The write half of `read32_ext`. False when there is no window
pub fn write32_ext(bus: u8, dev: u8, func: u8, off: u16, val: u32) -> bool {
    if off as usize + 4 > 4096 || off & 3 != 0 {
        return false;
    }
    let Some(at) = ecam_at(bus, dev, func) else { return false };
    unsafe { core::ptr::write_volatile((at + off as u64) as *mut u32, val) };
    true
}

/// Where extended capability `id` starts (at 0x100 or later), through ECAM.
/// At most 64 hops, for the same reason as `caps`
pub fn find_ext_cap(bus: u8, dev: u8, func: u8, id: u16) -> Option<u16> {
    let mut off = 0x100u16;
    for _ in 0..64 {
        let hdr = read32_ext(bus, dev, func, off)?;
        if hdr == 0 || hdr == u32::MAX {
            return None;
        }
        if (hdr & 0xFFFF) as u16 == id {
            return Some(off);
        }
        let next = ((hdr >> 20) & 0xFFC) as u16;
        if next < 0x100 || next == off {
            return None;
        }
        off = next;
    }
    None
}

/// PCIe capability +0x02, bits 7:4: what kind of port or endpoint this is
/// (PCIe Base 6.0, 7.5.3.2)
pub const PCIE_TYPE_ENDPOINT: u8 = 0x0;
pub const PCIE_TYPE_LEGACY_ENDPOINT: u8 = 0x1;
pub const PCIE_TYPE_ROOT_PORT: u8 = 0x4;
pub const PCIE_TYPE_UPSTREAM: u8 = 0x5;
pub const PCIE_TYPE_DOWNSTREAM: u8 = 0x6;

pub fn pcie_port_type(bus: u8, dev: u8, func: u8) -> Option<u8> {
    let cap = find_cap(bus, dev, func, CAP_ID_PCIE)?;
    Some(((pci_read16(bus, dev, func, cap + 2) >> 4) & 0xF) as u8)
}

/// The bridge whose far side is `bus`: the port a device on that bus is
/// plugged into. `None` for bus 0, which hangs off the host bridge itself
pub fn upstream_bridge(bus: u8) -> Option<PciDevice> {
    if bus == 0 {
        return None;
    }
    find(|d| {
        d.class == 0x06 && d.subclass == 0x04
            && header_type(d.bus, d.dev, d.func) & 0x7F == 0x01
            && pci_read8(d.bus, d.dev, d.func, 0x19) == bus
    })
}

/// The root port above a device on `bus`: the bridges are climbed until one
/// says it is a root port. `None` when the climb ends without one: a device on
/// bus 0, or a PCI bridge with no PCIe capability on the way
pub fn root_port_of(bus: u8) -> Option<PciDevice> {
    let mut at = bus;
    for _ in 0..16 {
        let up = upstream_bridge(at)?;
        if pcie_port_type(up.bus, up.dev, up.func) == Some(PCIE_TYPE_ROOT_PORT) {
            return Some(up);
        }
        at = up.bus;
    }
    None
}

/// Subsystem vendor and device of a type-0 function (0x2C, 0x2E). A bridge
/// keeps its subsystem in a capability (id 0x0D), which OpenRM's
/// `getSubsystemFromPCIECapabilities` reads
pub fn subsystem(bus: u8, dev: u8, func: u8) -> (u16, u16) {
    if header_type(bus, dev, func) & 0x7F == 0x00 {
        let v = pci_read32(bus, dev, func, 0x2C);
        return (v as u16, (v >> 16) as u16);
    }
    match find_cap(bus, dev, func, 0x0D) {
        Some(p) => {
            let v = pci_read32(bus, dev, func, p + 4);
            (v as u16, (v >> 16) as u16)
        }
        None => (0, 0),
    }
}

/// The revision id, register 0x08
pub fn revision(bus: u8, dev: u8, func: u8) -> u8 {
    pci_read8(bus, dev, func, 0x08)
}
