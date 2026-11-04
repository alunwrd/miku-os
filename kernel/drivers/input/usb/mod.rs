// USB stack
//
// Layout:
//   usb/mod.rs      PCI scan for xHCI controllers, usbd service thread
//   usb/xhci.rs     xHCI host controller driver (polling, no interrupts)
//   usb/hid_kbd.rs  HID boot-protocol keyboard -> Set-1 scancodes -> stdin
//
// usb_handoff.rs takes every USB controller away from the firmware at boot to
// stop SMIs. That also ends the BIOS "USB Legacy Support" PS/2 emulation, so a
// USB keyboard needs this stack. It drives the xHCI controller natively and
// feeds keystrokes into the stdin ring the PS/2 IRQ uses.
//
// Scope: xHCI only (every board since ~2012 routes all ports, USB2 and USB3
// alike, through xHCI; companion UHCI/OHCI/EHCI controllers ended with Sandy
// Bridge). Root ports only: a keyboard behind an external hub or a dock is not
// enumerated.
//
// No interrupts. The usbd thread pumps every controller's event ring at the
// scheduler tick rate (250 Hz), as netd does. Worst-case input latency is 4 ms

pub mod xhci;
pub mod hid_kbd;
pub mod hid_report;

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
    crate::drivers::bus::pci::with_devices(|devs| {
        devs.iter()
            .filter(|d| {
                d.class == PCI_CLASS_SERIAL_BUS
                    && d.subclass == PCI_SUBCLASS_USB
                    && d.prog_if == PROGIF_XHCI
            })
            .map(|d| XhciPciDev {
                bus: d.bus, dev: d.dev, func: d.func,
                vendor: d.vendor, device: d.device,
            })
            .collect()
    })
}

/// Human-readable status lines for the 'usb' shell command. Written by the usbd
/// thread, read by the shell; strings, so the shell never touches controller
/// state
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
        // Claim this controller from the firmware before touching it. xHCI 1.2
        // sec 4.22.1: ownership changes hands before HCRST; until USBLEGSUP
        // says OS-owned, SMM services the controller on every legacy access and
        // both sides drive the same registers.
        //
        // Only the controllers usbd is about to drive. The full-bus
        // usb_handoff::run() also maps OHCI/EHCI BAR0 and rewrites UHCI legacy
        // config on hardware this driver never touches
        crate::serial_println!(
            "[usb] xHCI at {:02x}:{:02x}.{} vendor={:04x} device={:04x}",
            pci.bus, pci.dev, pci.func, pci.vendor, pci.device
        );
        // Memory decode and bus mastering first: the handoff reads the
        // controller's MMIO capability list, and Xhci::init starts handing it
        // DMA addresses
        let cmd = pci_read32(pci.bus, pci.dev, pci.func, 0x04);
        pci_write32(pci.bus, pci.dev, pci.func, 0x04, cmd | 0x6);
        crate::drivers::input::usb_handoff::xhci_handoff(pci.bus, pci.dev, pci.func);
        match xhci::Xhci::init(pci) {
            Ok(hc) => controllers.push(hc),
            Err(e) => crate::serial_println!("[usb] xHCI init failed: {}", e),
        }
    }

    for hc in controllers.iter_mut() {
        hc.enumerate_ports();
    }

    // Claiming the controller ended the firmware's legacy PS/2 emulation, so
    // nothing enumerated means no keyboard at all. Reported in the serial log
    // and 'usb' output, with the way out
    let kbds: usize = controllers.iter().map(|hc| hc.keyboard_count()).sum();
    if kbds == 0 {
        crate::serial_println!(
            "[usb] WARNING: no USB keyboard enumerated. The firmware's legacy \
             PS/2 emulation is gone with the controller, so USB input is dead. \
             Boot with 'nousb' on the multiboot2 line to leave USB alone."
        );
        status_push(alloc::string::String::from(
            "no HID keyboard found - boot with 'nousb' to keep legacy PS/2 emulation",
        ));
    } else {
        crate::serial_println!("[usb] {} HID keyboard(s) online", kbds);
    }
    let mice: usize = controllers.iter().map(|hc| hc.mouse_count()).sum();
    if mice > 0 {
        crate::serial_println!("[usb] {} HID mouse(s) online", mice);
    }

    loop {
        for hc in controllers.iter_mut() {
            hc.poll();
        }
        crate::sched::sleep(1);
    }
}
