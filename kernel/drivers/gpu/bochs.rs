// The Bochs display, a virtual machine's screen
//
// QEMU's `-vga std`, its `bochs-display`, and the VGA compatibility face of
// virtio-vga and QXL are the same programming interface: a pair of index and
// data registers, the "DISPI" registers, through which a resolution is written,
// plus a linear framebuffer at BAR 0. These are the three writes Linux's
// `bochs-drm` makes.
//
// Firmware hands over whatever it chose: 800x600 under QEMU, GRUB's default
// when a multiboot2 kernel asks for "any" framebuffer.
//
// Where the registers are
//
//   * BAR 2, a memory window. The DISPI registers are at +0x500, two bytes
//     each, and the monitor's EDID is at +0x000. QEMU offers this one.
//   * I/O ports 0x01CE (index) and 0x01CF (data), for a machine without the
//     MMIO BAR.
//
// The EDID exists only on the memory window, so a device found through the
// ports is left alone.
//
// Reference: the Bochs VBE specification, and QEMU hw/display/vga.c

use x86_64::instructions::port::Port;

use crate::drivers::bus::pci::{self, PciDevice};

/// QEMU and Bochs both use this pair for the standard VGA and for
/// `bochs-display`
const VENDOR: u16 = 0x1234;
const DEVICE: u16 = 0x1111;

/// Where in BAR 2 the two halves live
const MMIO_EDID: u64 = 0x0000;
const MMIO_DISPI: u64 = 0x0500;

const PORT_INDEX: u16 = 0x01CE;
const PORT_DATA: u16 = 0x01CF;

// Register indices
const IDX_ID: u16 = 0;
const IDX_XRES: u16 = 1;
const IDX_YRES: u16 = 2;
const IDX_BPP: u16 = 3;
const IDX_ENABLE: u16 = 4;
const IDX_VIRT_WIDTH: u16 = 6;
const IDX_VIRT_HEIGHT: u16 = 7;
const IDX_X_OFFSET: u16 = 8;
const IDX_Y_OFFSET: u16 = 9;

const ENABLE_DISABLED: u16 = 0x00;
const ENABLE_ENABLED: u16 = 0x01;
const ENABLE_LFB: u16 = 0x40;
/// Keep what is in the framebuffer across the mode set: the console has the
/// boot log in there
const ENABLE_NOCLEAR: u16 = 0x80;

/// The lowest interface version that accepts a resolution at all
const ID0: u16 = 0xB0C0;
const ID5: u16 = 0xB0C5;

pub struct Bochs {
    /// The linear framebuffer, physical
    pub fb_phys: u64,
    /// The register window, virtual through the direct map, when there is one
    mmio: Option<u64>,
    /// Version reported by the ID register
    pub id: u16,
}

impl Bochs {
    /// Find the display, if this machine has one
    pub fn find() -> Option<Bochs> {
        let dev: PciDevice = pci::find_by_id(VENDOR, DEVICE)?;
        let fb_phys = dev.mem_bar(0)?;
        let mmio = dev
            .mem_bar(2)
            .map(crate::arch::x86_64::grub::phys_to_virt);

        // Memory decoding may be off if firmware never used the device; the
        // registers answer as 0xFFFF until it is on
        dev.enable_bus_mastering();

        let mut b = Bochs { fb_phys, mmio, id: 0 };
        b.id = b.read(IDX_ID);
        if !(ID0..=ID5).contains(&b.id) {
            crate::serial_println!(
                "[display] a Bochs display at {:04x}:{:04x} answered id {:#06x} - not one we know",
                VENDOR, DEVICE, b.id);
            return None;
        }
        Some(b)
    }

    fn read(&self, index: u16) -> u16 {
        match self.mmio {
            Some(base) => unsafe {
                core::ptr::read_volatile((base + MMIO_DISPI + index as u64 * 2) as *const u16)
            },
            None => unsafe {
                Port::<u16>::new(PORT_INDEX).write(index);
                Port::<u16>::new(PORT_DATA).read()
            },
        }
    }

    fn write(&self, index: u16, value: u16) {
        match self.mmio {
            Some(base) => unsafe {
                core::ptr::write_volatile(
                    (base + MMIO_DISPI + index as u64 * 2) as *mut u16, value)
            },
            None => unsafe {
                Port::<u16>::new(PORT_INDEX).write(index);
                Port::<u16>::new(PORT_DATA).write(value);
            },
        }
    }

    /// The monitor's EDID, when the device has the memory window that carries
    /// it. 128 bytes, the base block
    pub fn edid(&self) -> Option<[u8; 128]> {
        let base = self.mmio?;
        let mut out = [0u8; 128];
        for (i, b) in out.iter_mut().enumerate() {
            *b = unsafe {
                core::ptr::read_volatile((base + MMIO_EDID + i as u64) as *const u8)
            };
        }
        // An absent EDID reads as zeros or as 0xFF; either way it is not a
        // block, and `edid::parse` is what says so
        Some(out)
    }

    /// Put the display into a mode. Returns the stride in bytes.
    ///
    /// Everything is written with the display disabled, and enabling it last
    /// latches the lot
    pub fn set_mode(&self, width: u32, height: u32, bpp: u16) -> u32 {
        self.write(IDX_ENABLE, ENABLE_DISABLED);
        self.write(IDX_BPP, bpp);
        self.write(IDX_XRES, width as u16);
        self.write(IDX_YRES, height as u16);
        // No panning and no scrolling: the framebuffer is exactly the screen,
        // which the stride arithmetic below relies on
        self.write(IDX_VIRT_WIDTH, width as u16);
        self.write(IDX_VIRT_HEIGHT, height as u16);
        self.write(IDX_X_OFFSET, 0);
        self.write(IDX_Y_OFFSET, 0);
        self.write(IDX_ENABLE, ENABLE_ENABLED | ENABLE_LFB | ENABLE_NOCLEAR);

        // Asked back: a device that could not do what it was told reports what
        // it did, and the console is built on that number
        let got_w = self.read(IDX_VIRT_WIDTH).max(self.read(IDX_XRES)) as u32;
        got_w * (bpp as u32 / 8)
    }

    /// What the display is showing now
    pub fn current(&self) -> (u32, u32, u16) {
        (
            self.read(IDX_XRES) as u32,
            self.read(IDX_YRES) as u32,
            self.read(IDX_BPP),
        )
    }
}
