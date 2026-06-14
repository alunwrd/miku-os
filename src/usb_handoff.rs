//                           USB BIOS-to-OS handoff
//
// On legacy-BIOS boot from a USB stick, the firmware keeps USB Legacy
// Support active. Every USB poll the firmware does triggers an SMI,
// and the SMM handler runs invisibly to the OS 
//
// The fix is to claim ownership of every USB host controller via the
// BIOS-OS handoff protocol defined in the EHCI/XHCI specs, and to
// clear all SMI enables in UHCI legacy-support registers. Once that
// is done the firmware stops poking USB, no more SMIs fire, and 'sti'
// becomes safe
//
// Called from 'kernel_main' between 'apic::init_bsp()' and the final 'sti'
//
// Spec references:
//   xHCI 1.2 sec 7.1.1 (USBLEGSUP), sec 7.2 (xECP traversal)
//   EHCI 1.0 sec 2.1.7 (HCCPARAMS), sec 2.1.8 (EECP)
//   UHCI Design Guide Rev 1.1 (USBLEGSUP at PCI 0xC0)
//   OHCI 1.0a sec 5.1.1.3.5 (Ownership Change Request)

use x86_64::instructions::port::Port;

use crate::grub;

const PCI_ADDR: u16 = 0xCF8;
const PCI_DATA: u16 = 0xCFC;

const PCI_CLASS_SERIAL_BUS: u8 = 0x0C;
const PCI_SUBCLASS_USB:     u8 = 0x03;

const PROGIF_UHCI: u8 = 0x00;
const PROGIF_OHCI: u8 = 0x10;
const PROGIF_EHCI: u8 = 0x20;
const PROGIF_XHCI: u8 = 0x30;

fn pci_addr(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((off as u32) & 0xFC)
}

fn pci_read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    unsafe {
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, off));
        Port::<u32>::new(PCI_DATA).read()
    }
}

fn pci_write32(bus: u8, dev: u8, func: u8, off: u8, val: u32) {
    unsafe {
        Port::<u32>::new(PCI_ADDR).write(pci_addr(bus, dev, func, off));
        Port::<u32>::new(PCI_DATA).write(val);
    }
}

fn pci_read16(bus: u8, dev: u8, func: u8, off: u8) -> u16 {
    (pci_read32(bus, dev, func, off & !3) >> ((off & 2) * 8)) as u16
}

fn pci_write16(bus: u8, dev: u8, func: u8, off: u8, val: u16) {
    let old = pci_read32(bus, dev, func, off & !3);
    let shift = (off & 2) * 8;
    pci_write32(
        bus, dev, func, off & !3,
        (old & !(0xFFFF << shift)) | ((val as u32) << shift),
    );
}

fn pci_read8(bus: u8, dev: u8, func: u8, off: u8) -> u8 {
    (pci_read32(bus, dev, func, off & !3) >> ((off & 3) * 8)) as u8
}

fn bar_io(bus: u8, dev: u8, func: u8, idx: u8) -> Option<u16> {
    let bar = pci_read32(bus, dev, func, 0x10 + idx * 4);
    if bar & 1 != 0 { Some((bar & !3) as u16) } else { None }
}

fn bar_mem(bus: u8, dev: u8, func: u8, idx: u8) -> Option<u64> {
    let bar = pci_read32(bus, dev, func, 0x10 + idx * 4);
    if bar & 1 == 0 && bar != 0 {
        let bar_type = (bar >> 1) & 3;
        let lo = (bar & 0xFFFF_FFF0) as u64;
        if bar_type == 2 && idx + 1 < 6 {
            let hi = pci_read32(bus, dev, func, 0x10 + (idx + 1) * 4) as u64;
            Some(lo | (hi << 32))
        } else {
            Some(lo)
        }
    } else {
        None
    }
}

/// Top-level entry point. Scans every PCI function, dispatches to the
/// per-controller handler. Idempotent and safe to re-run
pub fn run() {
    let mut count = 0u32;
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

                if class == PCI_CLASS_SERIAL_BUS && subclass == PCI_SUBCLASS_USB {
                    handoff_one(bus, dev, func, progif, id);
                    count += 1;
                }

                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 {
                    break;
                }
            }
        }
        if bus == 255 { break; }
    }
    crate::serial_println!("[usb-handoff] processed {} usb controllers", count);
}

fn handoff_one(bus: u8, dev: u8, func: u8, progif: u8, id: u32) {
    let vendor = (id & 0xFFFF) as u16;
    let device = (id >> 16) as u16;
    let kind = match progif {
        PROGIF_UHCI => "UHCI",
        PROGIF_OHCI => "OHCI",
        PROGIF_EHCI => "EHCI",
        PROGIF_XHCI => "XHCI",
        _           => "USB?",
    };
    crate::serial_println!(
        "[usb-handoff] {} at {:02x}:{:02x}.{} vendor={:04x} device={:04x}",
        kind, bus, dev, func, vendor, device
    );
    match progif {
        PROGIF_UHCI => uhci_handoff(bus, dev, func),
        PROGIF_OHCI => ohci_handoff(bus, dev, func),
        PROGIF_EHCI => ehci_handoff(bus, dev, func),
        PROGIF_XHCI => xhci_handoff(bus, dev, func),
        _           => {}
    }
}

// UHCI: legacy-support register lives at PCI config offset 0xC0.
// Bits 13:0 are SMI enables/status. Writing 0x8F00 acknowledges all
// pending SMI status bits and clears every enable, which permanently
// stops the firmware from emulating PS/2 over UHCI
fn uhci_handoff(bus: u8, dev: u8, func: u8) {
    let before = pci_read16(bus, dev, func, 0xC0);
    pci_write16(bus, dev, func, 0xC0, 0x8F00);
    let after = pci_read16(bus, dev, func, 0xC0);
    crate::serial_println!(
        "[usb-handoff]   UHCI USBLEGSUP {:04x} -> {:04x}",
        before, after
    );
}

// OHCI: HcControl is at MMIO BAR0 offset 0x04. If bit 8 (IR =
// InterruptRouting) is set, the controller is BIOS-owned via SMI. We
// request ownership change by setting bit 3 (OwnershipChangeRequest)
// of HcCommandStatus (offset 0x08), then poll IR until it clears
fn ohci_handoff(bus: u8, dev: u8, func: u8) {
    let mmio_phys = match bar_mem(bus, dev, func, 0) {
        Some(p) if p != 0 => p,
        _ => {
            crate::serial_println!("[usb-handoff]   OHCI: no MMIO BAR0");
            return;
        }
    };
    crate::vmm::map_mmio_uc(mmio_phys, 0x1000);
    let mmio = (mmio_phys + grub::hhdm()) as *mut u32;

    unsafe {
        let hc_control = mmio.add(1);  // 0x04
        let hc_cmdstat = mmio.add(2);  // 0x08

        let ctrl0 = core::ptr::read_volatile(hc_control);
        crate::serial_println!("[usb-handoff]   OHCI HcControl={:#x}", ctrl0);

        if ctrl0 & (1 << 8) != 0 {
            // BIOS-owned via SMI. Request ownership change
            core::ptr::write_volatile(hc_cmdstat, 1 << 3);

            // Poll for IR=0. Per spec, BIOS responds within millisec
            // Cap at ~100ms equivalent (bounded busy-loop, no timer yet)
            let mut spins = 0u32;
            while core::ptr::read_volatile(hc_control) & (1 << 8) != 0 {
                spins += 1;
                if spins > 1_000_000 {
                    crate::serial_println!(
                        "[usb-handoff]   OHCI: BIOS handoff timed out, forcing"
                    );
                    // Force: just clear IR in HcControl directly. BIOS is broken; we take the device anyway
                    let v = core::ptr::read_volatile(hc_control);
                    core::ptr::write_volatile(hc_control, v & !(1 << 8));
                    break;
                }
                core::hint::spin_loop();
            }
        }

        // Disable all interrupts the BIOS may have left enabled (HcInterruptDisable @ 0x14, write all-1s)
        core::ptr::write_volatile(mmio.add(5), 0xFFFF_FFFF);
    }
}

// EHCI: legacy-support cap is in the PCI extended capability list
// reachable via EECP, an 8-bit field at HCCPARAMS bits [15:8]
// HCCPARAMS lives at MMIO BAR0 offset = CAPLENGTH(0x00 byte) + 0x08
// Actually HCCPARAMS is a fixed offset of 0x08 in capability registers
fn ehci_handoff(bus: u8, dev: u8, func: u8) {
    let mmio_phys = match bar_mem(bus, dev, func, 0) {
        Some(p) if p != 0 => p,
        _ => {
            crate::serial_println!("[usb-handoff]   EHCI: no MMIO BAR0");
            return;
        }
    };
    crate::vmm::map_mmio_uc(mmio_phys, 0x1000);
    let mmio_virt = mmio_phys + grub::hhdm();

    let hccparams = unsafe {
        core::ptr::read_volatile((mmio_virt + 0x08) as *const u32)
    };
    let eecp = ((hccparams >> 8) & 0xFF) as u8;
    crate::serial_println!(
        "[usb-handoff]   EHCI HCCPARAMS={:#x} EECP={:#x}",
        hccparams, eecp
    );

    if eecp < 0x40 {
        return; // no extended capability list
    }

    // Walk the EECP-rooted PCI capability chain. Each cap header is
    // 32 bits at PCI cfg offset 'cap': ID in [7:0], NEXT in [15:8]
    // We look for cap ID 0x01 (USB Legacy Support)
    let mut cap = eecp;
    let mut hops = 0u32;
    while cap != 0 && hops < 16 {
        let header = pci_read32(bus, dev, func, cap);
        let id   = (header & 0xFF) as u8;
        let next = ((header >> 8) & 0xFF) as u8;

        if id == 0x01 {
            do_legacy_handoff(bus, dev, func, cap, "EHCI");
            return;
        }
        cap = next;
        hops += 1;
    }
    crate::serial_println!("[usb-handoff]   EHCI: no Legacy Support cap");
}

// XHCI: extended capability list rooted at xECP, bits [31:16] of
// HCCPARAMS1 (offset 0x10 in MMIO capability registers), 
// expressed as 32-bit dword offsets from the start of capability registers
fn xhci_handoff(bus: u8, dev: u8, func: u8) {
    let mmio_phys = match bar_mem(bus, dev, func, 0) {
        Some(p) if p != 0 => p,
        _ => {
            crate::serial_println!("[usb-handoff]   XHCI: no MMIO BAR0");
            return;
        }
    };
    // xHCI MMIO can extend to many KiB; the cap+legacy region we touch
    // sits in the first 4KiB of capability space, so 64KiB is safe 
    crate::vmm::map_mmio_uc(mmio_phys, 0x1_0000);
    let mmio_virt = mmio_phys + grub::hhdm();

    let hccparams1 = unsafe {
        core::ptr::read_volatile((mmio_virt + 0x10) as *const u32)
    };
    let xecp_dwords = ((hccparams1 >> 16) & 0xFFFF) as u32;
    crate::serial_println!(
        "[usb-handoff]   XHCI HCCPARAMS1={:#x} xECP_dwords={:#x}",
        hccparams1, xecp_dwords
    );
    if xecp_dwords == 0 {
        return;
    }

    // Walk the xECP-rooted MMIO capability chain. Each header is at
    // mmio_virt + cap_off (in bytes). NEXT is in dwords from the cap
    // header; 0 terminates the chain
    let mut cap_off = (xecp_dwords as u64) * 4;
    let mut hops = 0u32;
    while cap_off != 0 && hops < 32 {
        let header_addr = mmio_virt + cap_off;
        let header = unsafe { core::ptr::read_volatile(header_addr as *const u32) };
        let id   = (header & 0xFF) as u8;
        let next = ((header >> 8) & 0xFF) as u8;

        if id == 0x01 {
            xhci_legacy_handoff(header_addr);
            return;
        }
        if next == 0 { break; }
        cap_off += (next as u64) * 4;
        hops += 1;
    }
    crate::serial_println!("[usb-handoff]   XHCI: no USBLEGSUP cap found");
}

// XHCI USBLEGSUP layout (cap header at offset 0):
//   +0x00 USBLEGSUP    - bit 16 = HC_BIOS_OWNED, bit 24 = HC_OS_OWNED
//   +0x04 USBLEGCTLSTS - SMI enables/status

fn xhci_legacy_handoff(usblegsup_virt: u64) {
    unsafe {
        let legsup     = usblegsup_virt as *mut u32;
        let legctlsts  = (usblegsup_virt + 4) as *mut u32;

        let v0 = core::ptr::read_volatile(legsup);
        crate::serial_println!(
            "[usb-handoff]   XHCI USBLEGSUP before={:#x}", v0
        );

        // Set OS-owned. Preserve the cap header in the low 16 bits
        core::ptr::write_volatile(legsup, v0 | (1 << 24));

        let mut spins = 0u32;
        loop {
            let v = core::ptr::read_volatile(legsup);
            // Done when BIOS-owned cleared and OS-owned set
            if v & (1 << 16) == 0 && v & (1 << 24) != 0 {
                crate::serial_println!(
                    "[usb-handoff]   XHCI handoff complete after {} spins (USBLEGSUP={:#x})",
                    spins, v
                );
                break;
            }
            spins += 1;
            if spins > 5_000_000 {
                crate::serial_println!(
                    "[usb-handoff]   XHCI handoff timed out, forcing (USBLEGSUP={:#x})",
                    v
                );
                // Force: clear BIOS-owned bit. Some BIOSes never release; we take the device
                core::ptr::write_volatile(legsup, (v & !(1 << 16)) | (1 << 24));
                break;
            }
            core::hint::spin_loop();
        }

        // Clear ALL SMI enables (bits 0,4,13,14,15,16) and acknowledge
        // any latched RWC status bits (bits 17..=21,29..=31). Writing
        // 1s to RWC bits clears them; writing 0s to RW bits disables
        // Easiest: clear bits 0,4,13..21 (SMI enables and statuses) by
        // writing 0x0000_E1F0 to status side and 0 to enable side
        // Simplest correct sequence: write 0 to clear all enables,
        // then write the status mask back to ack
        let cs0 = core::ptr::read_volatile(legctlsts);
        // Clear all enable bits (0, 4, 13..=15) by AND-NOT, then write
        // back the status bits as-is (they are RWC, write-1-to-clear)
        let enable_mask: u32 = (1 << 0) | (1 << 4) | (1 << 13) | (1 << 14) | (1 << 15);
        let status_mask: u32 = 0x0000_E1F0; // bits 4..8, 13..15
        let new = (cs0 & !enable_mask) | status_mask;
        core::ptr::write_volatile(legctlsts, new);
        crate::serial_println!(
            "[usb-handoff]   XHCI USBLEGCTLSTS {:#x} -> {:#x}",
            cs0, core::ptr::read_volatile(legctlsts)
        );
    }
}

// Shared handler for EHCI legacy-support cap (lives in PCI config
// space, not MMIO). cap_off is the PCI config offset of the cap
// header. USBLEGSUP layout matches xHCI: bit 16 = BIOS, bit 24 = OS
fn do_legacy_handoff(bus: u8, dev: u8, func: u8, cap_off: u8, label: &str) {
    let v0 = pci_read32(bus, dev, func, cap_off);
    crate::serial_println!(
        "[usb-handoff]   {} USBLEGSUP before={:#x}", label, v0
    );

    pci_write32(bus, dev, func, cap_off, v0 | (1 << 24));

    let mut spins = 0u32;
    loop {
        let v = pci_read32(bus, dev, func, cap_off);
        if v & (1 << 16) == 0 && v & (1 << 24) != 0 {
            crate::serial_println!(
                "[usb-handoff]   {} handoff complete after {} spins (USBLEGSUP={:#x})",
                label, spins, v
            );
            break;
        }
        spins += 1;
        if spins > 5_000_000 {
            crate::serial_println!(
                "[usb-handoff]   {} handoff timed out, forcing (USBLEGSUP={:#x})",
                label, v
            );
            pci_write32(bus, dev, func, cap_off, (v & !(1 << 16)) | (1 << 24));
            break;
        }
        core::hint::spin_loop();
    }

    // USBLEGCTLSTS at cap_off+4: kill SMI enables
    let cs0 = pci_read32(bus, dev, func, cap_off + 4);
    let enable_mask: u32 = (1 << 0) | (1 << 4) | (1 << 13) | (1 << 14) | (1 << 15);
    let status_mask: u32 = 0x0000_E1F0;
    pci_write32(bus, dev, func, cap_off + 4, (cs0 & !enable_mask) | status_mask);
}
