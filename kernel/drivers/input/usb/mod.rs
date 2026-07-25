/////////////////////////////////////////////////////////////////////////////////////
//                            USB stack for MikuOS                                 //
//                                                                                 //
// Layout:                                                                         //
//   usb/mod.rs      PCI scan for xHCI controllers, usbd service thread            //
//   usb/xhci.rs     xHCI host controller driver (polling, no interrupts)          //
//   usb/hid_kbd.rs  HID boot-protocol keyboard -> Set-1 scancodes -> stdin        //
//                                                                                 //
// Why this exists: usb_handoff.rs takes every USB controller away from the        //
// firmware at boot to stop SMIs. That also kills the BIOS "USB Legacy             //
// Support" PS/2 emulation, so on real hardware a USB keyboard goes dead the       //
// moment the kernel starts. This stack drives the xHCI controller natively        //
// and feeds keystrokes into the same stdin ring the PS/2 IRQ uses, so the         //
// shell does not know or care where input comes from                              //
//                                                                                 //
// Scope (deliberate): xHCI only (every board since ~2012 routes all ports,        //
// USB2 and USB3 alike, through xHCI; companion UHCI/OHCI/EHCI controllers         //
// died with Sandy Bridge). Root ports only for now: a keyboard behind an          //
// external hub or a dock enumerates the hub, which we do not descend into yet     //
//                                                                                 //
// Model: no interrupts. The usbd thread pumps every controller's event ring       //
// at the scheduler tick rate (250 Hz), same pattern as netd. 4 ms worst-case      //
// input latency is invisible when typing                                          //
////////////////////////////////////////////////////////////////////////////////////

pub mod xhci;
pub mod hid_kbd;

use alloc::vec::Vec;
use spin::Mutex;

use x86_64::instructions::port::Port;

const PCI_ADDR: u16 = 0xCF8;
const PCI_DATA: u16 = 0xCFC;

const PCI_CLASS_SERIAL_BUS: u8 = 0x0C;
const PCI_SUBCLASS_USB:     u8 = 0x03;
const PROGIF_XHCI:          u8 = 0x30;

fn pci_addr(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((off as u32) & 0xFC)
}

pub(crate) fn pci_read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    unsafe {
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, off));
        Port::<u32>::new(PCI_DATA).read()
    }
}

pub(crate) fn pci_write32(bus: u8, dev: u8, func: u8, off: u8, val: u32) {
    unsafe {
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, off));
        Port::<u32>::new(PCI_DATA).write(val);
    }
}

fn pci_read8(bus: u8, dev: u8, func: u8, off: u8) -> u8 {
    (pci_read32(bus, dev, func, off & !3) >> ((off & 3) * 8)) as u8
}

/// One discovered xHCI controller (PCI location + ids), before bring-up
#[derive(Clone, Copy)]
pub struct XhciPciDev {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
}

fn scan_xhci() -> Vec<XhciPciDev> {
    let mut found = Vec::new();
    for bus in 0..=255u16 {
        let bus = bus as u8;
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = pci_read32(bus, dev, func, 0x00);
                if (id & 0xFFFF) as u16 == 0xFFFF {
                    if func == 0 { break; }
                    continue;
                }
                let class_rev = pci_read32(bus, dev, func, 0x08);
                let class    = (class_rev >> 24) as u8;
                let subclass = (class_rev >> 16) as u8;
                let progif   = (class_rev >> 8)  as u8;
                if class == PCI_CLASS_SERIAL_BUS
                    && subclass == PCI_SUBCLASS_USB
                    && progif == PROGIF_XHCI
                {
                    found.push(XhciPciDev {
                        bus, dev, func,
                        vendor: (id & 0xFFFF) as u16,
                        device: (id >> 16) as u16,
                    });
                }
                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 {
                    break;
                }
            }
        }
        if bus == 255 { break; }
    }
    found
}

/// Human-readable status lines for the 'usb' shell command. Written by the
/// usbd thread, read by the shell; strings so the shell never touches
/// controller state
static STATUS: Mutex<Vec<alloc::string::String>> = Mutex::new(Vec::new());

pub fn status_lines() -> Vec<alloc::string::String> {
    STATUS.lock().clone()
}

pub(crate) fn status_push(line: alloc::string::String) {
    STATUS.lock().push(line);
}

/// usbd: brings up every xHCI controller, enumerates root ports, then pumps
/// event rings forever. Registered with mikuD from kernel_main
pub fn usbd_thread() -> ! {
    let pcis = scan_xhci();
    if pcis.is_empty() {
        crate::serial_println!("[usb] no xHCI controller found");
        status_push(alloc::string::String::from("no xHCI controller found"));
        loop { crate::sched::sleep(2500); }
    }

    let mut controllers: Vec<xhci::Xhci> = Vec::new();
    for pci in pcis {
        crate::serial_println!(
            "[usb] xHCI at {:02x}:{:02x}.{} vendor={:04x} device={:04x}",
            pci.bus, pci.dev, pci.func, pci.vendor, pci.device
        );
        match xhci::Xhci::init(pci) {
            Ok(hc) => controllers.push(hc),
            Err(e) => crate::serial_println!("[usb] xHCI init failed: {}", e),
        }
    }

    for hc in controllers.iter_mut() {
        hc.enumerate_ports();
    }

    loop {
        for hc in controllers.iter_mut() {
            hc.poll();
        }
        crate::sched::sleep(1);
    }
}
