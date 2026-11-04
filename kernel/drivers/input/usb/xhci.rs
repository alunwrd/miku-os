// xHCI host controller driver (polling mode)
//
// Covers the path from PCI BAR to a running HID keyboard:
//     controller halt / reset / start (xHCI 1.2 sec 4.2)
//     DCBAA, scratchpad buffers, command ring, one event ring (interrupter 0,
//     interrupts left disabled: the usbd thread polls at 250 Hz)
//     root-port reset and device enumeration (USB2 ports need an explicit
//     reset; USB3 ports come up enabled on their own)
//     Enable Slot / Address Device / Configure Endpoint command flow with
//     both 32- and 64-byte context layouts (HCCPARAMS1.CSZ)
//     control transfers on EP0 (descriptor reads, SET_CONFIGURATION,
//     HID SET_PROTOCOL) and one interrupt-IN endpoint per HID keyboard
//
// Not covered: external hubs (a keyboard behind a hub or dock does not
// enumerate), isoch/bulk, bandwidth accounting, USB3 link management.
//
// Spec references: xHCI 1.2 (Intel doc 625472), USB 2.0 sec 9 (device
// framework), HID 1.11 appendix B (boot protocol)

use alloc::format;
use alloc::vec::Vec;

use crate::mm::dma::DmaBuffer;
use crate::sched;
use crate::serial_println;

use super::hid_kbd;
use super::hid_report;
use super::XhciPciDev;

// registers

// Capability registers (offsets from BAR0)
const CAP_CAPLENGTH:  u64 = 0x00; // u8
const CAP_HCSPARAMS1: u64 = 0x04;
const CAP_HCSPARAMS2: u64 = 0x08;
const CAP_HCCPARAMS1: u64 = 0x10;
const CAP_DBOFF:      u64 = 0x14;
const CAP_RTSOFF:     u64 = 0x18;

// Operational registers (offsets from BAR0 + CAPLENGTH)
const OP_USBCMD:  u64 = 0x00;
const OP_USBSTS:  u64 = 0x04;
const OP_CRCR:    u64 = 0x18;
const OP_DCBAAP:  u64 = 0x30;
const OP_CONFIG:  u64 = 0x38;
const OP_PORTSC:  u64 = 0x400; // + 0x10 * (port - 1)

const USBCMD_RUN:   u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBSTS_HCH:   u32 = 1 << 0;
const USBSTS_CNR:   u32 = 1 << 11;

// HCCPARAMS1 bits
const HCC_PPC: u32 = 1 << 3;     // port power control implemented

// PORTSC bits
const PORTSC_CCS: u32 = 1 << 0;  // current connect status
const PORTSC_PED: u32 = 1 << 1;  // port enabled (RW1C: writing 1 DISABLES)
const PORTSC_PR:  u32 = 1 << 4;  // port reset
const PORTSC_PP:  u32 = 1 << 9;  // port power
// change bits, all RW1C
const PORTSC_CHANGE_BITS: u32 = (1 << 17) | (1 << 18) | (1 << 19)
                              | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);
// Bits safe to write back as-is (power + wake enables); everything else in a
// PORTSC write must be 0, to avoid acking or toggling RW1C bits
const PORTSC_PRESERVE: u32 = PORTSC_PP | (0x7 << 25);

// Interrupter 0 registers (offsets from BAR0 + RTSOFF)
const IR0_IMAN:   u64 = 0x20;
const IR0_ERSTSZ: u64 = 0x28;
const IR0_ERSTBA: u64 = 0x30;
const IR0_ERDP:   u64 = 0x38;

const ERDP_EHB: u64 = 1 << 3;

// TRB types
const TRB_NORMAL:        u32 = 1;
const TRB_SETUP:         u32 = 2;
const TRB_DATA:          u32 = 3;
const TRB_STATUS:        u32 = 4;
const TRB_LINK:          u32 = 6;
const TRB_ENABLE_SLOT:   u32 = 9;
const TRB_ADDRESS_DEV:   u32 = 11;
const TRB_CONFIGURE_EP:  u32 = 12;
const TRB_EVALUATE_CTX:  u32 = 13;
const TRB_RESET_EP:      u32 = 14;
const TRB_SET_TR_DEQ:    u32 = 16;
const TRB_EVT_TRANSFER:  u32 = 32;
const TRB_EVT_CMD_DONE:  u32 = 33;
const TRB_EVT_PORT_CHG:  u32 = 34;

// TRB control-word flags
const TRB_C:   u32 = 1 << 0; // cycle
const TRB_TC:  u32 = 1 << 1; // toggle cycle (link TRB)
const TRB_ISP: u32 = 1 << 2; // interrupt on short packet
const TRB_IOC: u32 = 1 << 5; // interrupt on completion
const TRB_IDT: u32 = 1 << 6; // immediate data (setup stage)

// completion codes (event TRB dword2 bits 31:24)
const CC_SUCCESS:      u32 = 1;
const CC_SHORT_PACKET: u32 = 13;

const RING_TRBS: usize = 256; // one 4 KiB page, 16-byte TRBs

// Scheduler ticks (the usbd thread's only clock) at 250 Hz. The delays below
// come from spec millisecond figures
const MS_PER_TICK: u64 = 4;
const fn ms(n: u64) -> u64 { (n + MS_PER_TICK - 1) / MS_PER_TICK }

/// How long to keep sweeping the root ports after the controller starts. A
/// just-powered port has to debounce a USB2 connect (100 ms, USB 2.0 sec
/// 7.1.7.3) or finish training a USB3 link, so the first sweep after HCRST can
/// see an empty bus
const SETTLE_ROUNDS: u32 = 40;   // 40 * 50 ms = 2 s
const SETTLE_TICKS:  u64 = ms(50);

/// Give up on a port after this many failed enumeration attempts, so one dead
/// device cannot use the whole settle window
const MAX_PORT_ATTEMPTS: u8 = 3;

// port speeds (PORTSC bits 13:10)
const SPEED_FULL:  u8 = 1;
const SPEED_LOW:   u8 = 2;
const SPEED_HIGH:  u8 = 3;
const SPEED_SUPER: u8 = 4;

fn mmio_read32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn mmio_write32(addr: u64, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

fn mmio_write64(addr: u64, val: u64) {
    // xHCI 64-bit registers must be written low dword first
    mmio_write32(addr, val as u32);
    mmio_write32(addr + 4, (val >> 32) as u32);
}

// rings

/// TRB producer ring (command ring or transfer ring): one page of TRBs, the
/// last a Link TRB pointing back to the start with Toggle Cycle set
struct Ring {
    buf: DmaBuffer,
    idx: usize,
    cycle: bool,
}

impl Ring {
    fn new() -> Result<Self, &'static str> {
        let mut buf = DmaBuffer::alloc(1).map_err(|_| "ring alloc failed")?;
        buf.zero();
        let ring = Ring { buf, idx: 0, cycle: true };
        // Pre-plant the link TRB; its cycle bit stays 0 (invalid for the first
        // lap) until the producer wraps
        let link = ring.trb_addr(RING_TRBS - 1);
        mmio_write32(link,      ring.buf.phys() as u32);
        mmio_write32(link + 4, (ring.buf.phys() >> 32) as u32);
        mmio_write32(link + 8,  0);
        mmio_write32(link + 12, (TRB_LINK << 10) | TRB_TC);
        Ok(ring)
    }

    fn trb_addr(&self, i: usize) -> u64 {
        self.buf.virt_base() as u64 + (i as u64) * 16
    }

    fn trb_phys(&self, i: usize) -> u64 {
        self.buf.phys() + (i as u64) * 16
    }

    /// Enqueue one TRB (control word without the cycle bit, added here).
    /// Returns the TRB's physical address, which command-completion events echo
    /// back
    fn push(&mut self, d0: u32, d1: u32, d2: u32, ctrl: u32) -> u64 {
        let i = self.idx;
        let a = self.trb_addr(i);
        mmio_write32(a,     d0);
        mmio_write32(a + 4, d1);
        mmio_write32(a + 8, d2);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let cycle = if self.cycle { TRB_C } else { 0 };
        mmio_write32(a + 12, (ctrl & !TRB_C) | cycle);

        self.idx += 1;
        if self.idx == RING_TRBS - 1 {
            // Hand the link TRB to the controller with the current cycle, then
            // wrap with the opposite polarity
            let link = self.trb_addr(RING_TRBS - 1);
            let lctrl = (TRB_LINK << 10) | TRB_TC | cycle;
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            mmio_write32(link + 12, lctrl);
            self.cycle = !self.cycle;
            self.idx = 0;
        }
        self.trb_phys(i)
    }
}

/// Event ring consumer: single segment, single-entry ERST
struct EventRing {
    seg: DmaBuffer,
    _erst: DmaBuffer,
    idx: usize,
    ccs: bool,
}

impl EventRing {
    fn new() -> Result<Self, &'static str> {
        let mut seg = DmaBuffer::alloc(1).map_err(|_| "event ring alloc failed")?;
        seg.zero();
        let mut erst = DmaBuffer::alloc(1).map_err(|_| "erst alloc failed")?;
        erst.zero();
        // ERST entry 0: {base (8B), size in TRBs (4B), rsvd}
        let e = erst.virt_base() as u64;
        mmio_write32(e,      seg.phys() as u32);
        mmio_write32(e + 4, (seg.phys() >> 32) as u32);
        mmio_write32(e + 8,  RING_TRBS as u32);
        Ok(EventRing { seg, _erst: erst, idx: 0, ccs: true })
    }

    fn erst_phys(&self) -> u64 { self._erst.phys() }

    fn dequeue_phys(&self) -> u64 {
        self.seg.phys() + (self.idx as u64) * 16
    }

    /// Pop the next event TRB if the controller has produced one
    fn pop(&mut self) -> Option<[u32; 4]> {
        let a = self.seg.virt_base() as u64 + (self.idx as u64) * 16;
        let d3 = mmio_read32(a + 12);
        if (d3 & TRB_C != 0) != self.ccs {
            return None;
        }
        let trb = [
            mmio_read32(a),
            mmio_read32(a + 4),
            mmio_read32(a + 8),
            d3,
        ];
        self.idx += 1;
        if self.idx == RING_TRBS {
            self.idx = 0;
            self.ccs = !self.ccs;
        }
        Some(trb)
    }
}

// keyboard

/// What a configured interface is. Both boot protocols use the same transfer
/// machinery, an interrupt-IN endpoint delivering a small fixed report, and
/// differ only in how the bytes are read: one device list, one recovery path
enum HidKind {
    Keyboard(hid_kbd::KbdState),
    Mouse,
    /// A pointer with a described report: a device with no boot protocol, or
    /// one reporting a position
    Pointer(hid_report::Pointer),
}

/// A configured HID boot-protocol device on its interrupt-IN endpoint
struct Hid {
    slot: u8,
    dci: u8,
    ring: Ring,
    report: DmaBuffer,
    /// Bytes to ask for per interrupt-IN transfer: the endpoint's
    /// wMaxPacketSize, not the 8 bytes of a boot report. A TRB shorter than the
    /// packet the device sends is a babble error, which halts the endpoint
    xfer_len: u32,
    kind: HidKind,
    /// Whether anything has ever arrived from this device. Tells a mouse that
    /// enumerated and said nothing from one that never enumerated
    reported: bool,
}

// controller

pub struct Xhci {
    pci: XhciPciDev,
    bar: u64,   // virt base (HHDM) of BAR0
    op: u64,    // operational registers
    run: u64,   // runtime registers
    db: u64,    // doorbell array
    csz64: bool,
    ppc: bool,
    max_ports: u8,
    dcbaa: DmaBuffer,
    cmd: Ring,
    evt: EventRing,
    // Scratchpad index page + the pages it points to; controller-owned, held so
    // Drop returns them
    _scratch: Vec<DmaBuffer>,
    // Per-device DMA blocks (input ctx, device ctx, EP0 ring), alive for the
    // lifetime of the controller
    _dev_mem: Vec<DmaBuffer>,
    hids: Vec<Hid>,
    port_enumerated: [bool; 64],
    port_attempts: [u8; 64],
    last_cmd_event: Option<[u32; 4]>,
    // Ports with unhandled connect/disconnect changes. poll() only queues here;
    // enumeration happens at the usbd top level, so a port-change event during
    // setup_device (its own reset raises one) cannot recurse into a second
    // enumeration of the same port
    pending_ports: Vec<u8>,
    // (slot, dci) of halted endpoints. Recovery needs two commands, re-entering
    // poll(), so it is queued out of the event drain as port changes are
    pending_halts: Vec<(u8, u8)>,
    in_enumeration: bool,
}

impl Xhci {
    pub fn init(pci: XhciPciDev) -> Result<Self, &'static str> {
        let (bus, dev, func) = (pci.bus, pci.dev, pci.func);

        // BAR0 (memory, possibly 64-bit)
        let bar_lo = super::pci_read32(bus, dev, func, 0x10);
        if bar_lo & 1 != 0 { return Err("BAR0 is I/O"); }
        let mut phys = (bar_lo & 0xFFFF_FFF0) as u64;
        if (bar_lo >> 1) & 3 == 2 {
            phys |= (super::pci_read32(bus, dev, func, 0x14) as u64) << 32;
        }
        if phys == 0 { return Err("BAR0 not assigned"); }

        // memory decode + bus mastering
        let cmd_reg = super::pci_read32(bus, dev, func, 0x04);
        super::pci_write32(bus, dev, func, 0x04, cmd_reg | 0x6);

        crate::mm::vmm::map_mmio_uc(phys, 0x1_0000);
        let bar = phys + crate::arch::x86_64::grub::hhdm();

        let caplen  = (mmio_read32(bar + CAP_CAPLENGTH) & 0xFF) as u64;
        let hcs1    = mmio_read32(bar + CAP_HCSPARAMS1);
        let hcs2    = mmio_read32(bar + CAP_HCSPARAMS2);
        let hcc1    = mmio_read32(bar + CAP_HCCPARAMS1);
        let dboff   = (mmio_read32(bar + CAP_DBOFF)  & !0x3) as u64;
        let rtsoff  = (mmio_read32(bar + CAP_RTSOFF) & !0x1F) as u64;

        let max_slots = (hcs1 & 0xFF) as u8;
        let max_ports = (hcs1 >> 24) as u8;
        let csz64 = hcc1 & (1 << 2) != 0;
        let ppc = hcc1 & HCC_PPC != 0;

        // Doorbells / runtime regs can sit past the first 64 KiB on some
        // controllers; the UC mapping is extended to cover them
        let span = (rtsoff + 0x8000).max(dboff + 0x1000).max(0x1_0000);
        crate::mm::vmm::map_mmio_uc(phys, span);

        serial_println!(
            "[usb] xhci caplen={} slots={} ports={} csz={} ppc={} dboff={:#x} rtsoff={:#x}",
            caplen, max_slots, max_ports, if csz64 { 64 } else { 32 }, ppc, dboff, rtsoff
        );

        let op = bar + caplen;
        let run = bar + rtsoff;
        let db = bar + dboff;

        // Halt (if running), then reset
        let cmd0 = mmio_read32(op + OP_USBCMD);
        if cmd0 & USBCMD_RUN != 0 {
            mmio_write32(op + OP_USBCMD, cmd0 & !USBCMD_RUN);
        }
        if !wait_reg(op + OP_USBSTS, USBSTS_HCH, USBSTS_HCH, 100) {
            return Err("controller would not halt");
        }
        mmio_write32(op + OP_USBCMD, USBCMD_HCRST);
        if !wait_reg(op + OP_USBCMD, USBCMD_HCRST, 0, 500) {
            return Err("controller reset stuck");
        }
        if !wait_reg(op + OP_USBSTS, USBSTS_CNR, 0, 500) {
            return Err("controller not ready after reset");
        }

        // DCBAA
        let mut dcbaa = DmaBuffer::alloc(1).map_err(|_| "dcbaa alloc failed")?;
        dcbaa.zero();

        // Scratchpad buffers (DCBAA[0] -> array of page pointers)
        let mut scratch: Vec<DmaBuffer> = Vec::new();
        let n_scratch = (((hcs2 >> 21) & 0x1F) << 5 | (hcs2 >> 27)) as usize;
        if n_scratch > 0 {
            let mut arr = DmaBuffer::alloc(1).map_err(|_| "scratch array alloc failed")?;
            arr.zero();
            for i in 0..n_scratch.min(512) {
                let mut page = DmaBuffer::alloc(1).map_err(|_| "scratch page alloc failed")?;
                page.zero();
                let slot = arr.virt_base() as u64 + (i as u64) * 8;
                unsafe { core::ptr::write_volatile(slot as *mut u64, page.phys()); }
                scratch.push(page);
            }
            unsafe {
                core::ptr::write_volatile(dcbaa.virt_base() as *mut u64, arr.phys());
            }
            scratch.push(arr);
        }

        let cmd = Ring::new()?;
        let evt = EventRing::new()?;

        mmio_write32(op + OP_CONFIG, max_slots as u32);
        mmio_write64(op + OP_DCBAAP, dcbaa.phys());
        mmio_write64(op + OP_CRCR, cmd.buf.phys() | 1); // RCS = 1

        // Interrupter 0: event ring wired up, interrupts left disabled (polled)
        mmio_write32(run + IR0_ERSTSZ, 1);
        mmio_write64(run + IR0_ERDP, evt.dequeue_phys() | ERDP_EHB);
        mmio_write64(run + IR0_ERSTBA, evt.erst_phys());
        mmio_write32(run + IR0_IMAN, mmio_read32(run + IR0_IMAN) & !(1 << 1)); // IE off

        DmaBuffer::write_barrier();
        mmio_write32(op + OP_USBCMD, mmio_read32(op + OP_USBCMD) | USBCMD_RUN);
        if !wait_reg(op + OP_USBSTS, USBSTS_HCH, 0, 100) {
            return Err("controller did not start");
        }
        // Root ports come out of a chip reset unpowered on every controller
        // with Port Power Control, and nothing else turns them back on. An
        // unpowered port never raises a connect
        if ppc {
            for port in 1..=max_ports {
                let a = op + OP_PORTSC + 0x10 * (port as u64 - 1);
                let sc = mmio_read32(a);
                if sc & PORTSC_PP == 0 {
                    mmio_write32(a, (sc & PORTSC_PRESERVE) | PORTSC_PP);
                }
            }
            // USB 2.0 sec 7.2.4.1: 20 ms for power to become good
            // (bPwrOn2PwrGood)
            sched::sleep(ms(20));
        }

        serial_println!("[usb] xhci running ({} scratchpads)", n_scratch);
        super::status_push(format!(
            "xHCI {:02x}:{:02x}.{} {:04x}:{:04x} running, {} ports",
            pci.bus, pci.dev, pci.func, pci.vendor, pci.device, max_ports
        ));

        Ok(Xhci {
            pci,
            bar,
            op,
            run,
            db,
            csz64,
            ppc,
            max_ports,
            dcbaa,
            cmd,
            evt,
            _scratch: scratch,
            _dev_mem: Vec::new(),
            hids: Vec::new(),
            port_enumerated: [false; 64],
            port_attempts: [0; 64],
            last_cmd_event: None,
            pending_ports: Vec::new(),
            pending_halts: Vec::new(),
            in_enumeration: false,
        })
    }

    fn ctx_size(&self) -> u64 {
        if self.csz64 { 64 } else { 32 }
    }

    fn portsc_addr(&self, port: u8) -> u64 {
        self.op + OP_PORTSC + 0x10 * (port as u64 - 1)
    }

    fn ring_doorbell(&self, slot: u8, target: u32) {
        DmaBuffer::write_barrier();
        mmio_write32(self.db + (slot as u64) * 4, target);
    }

    //events

    /// Drain the event ring. Command completions are stashed for the issuer;
    /// transfer events go to the owning keyboard; port changes trigger
    /// (re-)enumeration attempts
    pub fn poll(&mut self) {
        let mut popped = false;
        while let Some(trb) = self.evt.pop() {
            popped = true;
            match (trb[3] >> 10) & 0x3F {
                TRB_EVT_CMD_DONE => self.last_cmd_event = Some(trb),
                TRB_EVT_TRANSFER => self.handle_transfer_event(trb),
                TRB_EVT_PORT_CHG => self.note_port_change(&trb),
                _ => {}
            }
        }
        if popped {
            mmio_write64(self.run + IR0_ERDP, self.evt.dequeue_phys() | ERDP_EHB);
        }
        // Enumeration re-enters poll() through run_cmd/wait_transfer; only the
        // outermost caller drains the pending lists
        if !self.in_enumeration {
            // Recovery issues commands, which re-enter poll() through run_cmd.
            // The enumeration flag stops the nested call from starting a second
            // recovery or an enumeration
            self.in_enumeration = true;
            while let Some((slot, dci)) = self.pending_halts.pop() {
                self.recover_endpoint(slot, dci);
            }
            self.in_enumeration = false;

            while let Some(port) = self.pending_ports.pop() {
                self.handle_port_change(port);
            }
        }
    }

    /// Queue a port for (re-)enumeration. Both event drains go through here:
    /// unacked PORTSC change bits stop the controller from raising a second
    /// event, and the port stays invisible for the rest of the boot
    fn note_port_change(&mut self, trb: &[u32; 4]) {
        let port = (trb[0] >> 24) as u8;
        if port >= 1 && port <= self.max_ports && !self.pending_ports.contains(&port) {
            self.pending_ports.push(port);
        }
    }

    fn handle_port_change(&mut self, port: u8) {
        let sc = mmio_read32(self.portsc_addr(port));
        // Ack the change bits so the event does not refire
        mmio_write32(
            self.portsc_addr(port),
            (sc & PORTSC_PRESERVE) | (sc & PORTSC_CHANGE_BITS),
        );
        let idx = (port - 1) as usize;
        if sc & PORTSC_CCS != 0 && !self.port_enumerated[idx] {
            serial_println!("[usb] port {} hotplug (portsc={:#x})", port, sc);
            self.try_enumerate_port(port);
        } else if sc & PORTSC_CCS == 0 && self.port_enumerated[idx] {
            serial_println!("[usb] port {} disconnected", port);
            self.port_enumerated[idx] = false;
            self.port_attempts[idx] = 0;
            // The device's slot is dead; a keyboard bound to it is handled
            // lazily (its transfers stop)
        }
    }

    fn handle_transfer_event(&mut self, trb: [u32; 4]) {
        let slot = (trb[3] >> 24) as u8;
        let cc = trb[2] >> 24;
        let db = self.db;
        let mut halted: Option<(u8, u8)> = None;
        for kbd in self.hids.iter_mut() {
            if kbd.slot != slot {
                continue;
            }
            if cc != CC_SUCCESS && cc != CC_SHORT_PACKET {
                // Anything else halted the endpoint. A TRB pushed at a halted
                // endpoint does nothing, so it goes to the recovery path
                serial_println!(
                    "[usb] slot {} ep {} transfer cc={} - halted, recovering",
                    slot, kbd.dci, cc
                );
                halted = Some((slot, kbd.dci));
                break;
            }

            // DMA-written memory: a volatile read, so the compiler cannot hoist
            // or reuse a load from before the controller filled it in
            let base = kbd.report.virt_base();
            // Sixteen, not the eight of a boot report: a described pointer's
            // report is whatever its descriptor says, and a tablet's is six or
            // seven
            let mut buf = [0u8; 16];
            for (i, b) in buf.iter_mut().enumerate() {
                *b = unsafe { core::ptr::read_volatile(base.add(i)) };
            }
            let boot: &[u8; 8] = &buf[..8].try_into().unwrap();
            match &mut kbd.kind {
                HidKind::Keyboard(state) => state.process_report(boot),
                // The boot mouse report: buttons, then dx, dy and the wheel as
                // plain signed bytes. The report arrives whole or not at all,
                // with no resynchronisation
                HidKind::Mouse => {
                    // A three-byte mouse has no wheel byte, and the slot holds
                    // the last report's value. The event says how much of the
                    // TRB went unused
                    let got = kbd.xfer_len.saturating_sub(trb[2] & 0x00ff_ffff);
                    if !kbd.reported {
                        kbd.reported = true;
                        serial_println!(
                            "[usb] slot {} mouse: first report, {} bytes: {:02x?}",
                            slot, got, &buf[..got.min(8) as usize]);
                    }
                    crate::drivers::input::mouse::feed_hid(
                        buf[0],
                        buf[1] as i8 as i32,
                        buf[2] as i8 as i32,
                        if got >= 4 { buf[3] as i8 as i32 } else { 0 },
                    );
                }
                // The layout says which bits are the buttons and the axes and
                // whether the axes are a position; the decode lands in the same
                // two accumulators either way
                HidKind::Pointer(p) => {
                    let got = kbd.xfer_len.saturating_sub(trb[2] & 0x00ff_ffff);
                    let n = (got as usize).min(buf.len());
                    if !kbd.reported {
                        kbd.reported = true;
                        serial_println!(
                            "[usb] slot {} {}: first report, {} bytes: {:02x?}",
                            slot, if p.absolute { "tablet" } else { "mouse" }, got, &buf[..n]);
                    }
                    if let Some((btn, x, y, wheel)) = p.decode(&buf[..n]) {
                        if p.absolute {
                            crate::drivers::input::mouse::feed_abs(
                                btn, x as u32, y as u32, wheel);
                        } else {
                            crate::drivers::input::mouse::feed_hid(btn, x, y, wheel);
                        }
                    }
                }
            }

            // requeue the next report read
            kbd.ring.push(
                kbd.report.phys() as u32,
                (kbd.report.phys() >> 32) as u32,
                kbd.xfer_len,
                (TRB_NORMAL << 10) | TRB_IOC | TRB_ISP,
            );
            DmaBuffer::write_barrier();
            mmio_write32(db + (slot as u64) * 4, kbd.dci as u32);
            return;
        }
        if let Some(h) = halted {
            if !self.pending_halts.contains(&h) {
                self.pending_halts.push(h);
            }
        }
    }

    /// Clear a halted interrupt endpoint and restart its report stream: Reset
    /// Endpoint, point the transfer ring's dequeue pointer at the next enqueue,
    /// post a fresh read
    fn recover_endpoint(&mut self, slot: u8, dci: u8) {
        let Some(i) = self.hids.iter().position(|k| k.slot == slot && k.dci == dci) else {
            return;
        };
        let deq = {
            let k = &self.hids[i];
            k.ring.trb_phys(k.ring.idx) | if k.ring.cycle { 1 } else { 0 }
        };
        if let Err(e) = self.reset_endpoint(slot, dci, deq) {
            serial_println!("[usb] slot {} ep {} recovery failed: {}", slot, dci, e);
            return;
        }
        let k = &mut self.hids[i];
        k.ring.push(
            k.report.phys() as u32,
            (k.report.phys() >> 32) as u32,
            k.xfer_len,
            (TRB_NORMAL << 10) | TRB_IOC | TRB_ISP,
        );
        self.ring_doorbell(slot, dci as u32);
        serial_println!("[usb] slot {} ep {} recovered", slot, dci);
    }

    /// Reset Endpoint + Set TR Dequeue Pointer. 'deq' carries the Dequeue Cycle
    /// State in bit 0
    fn reset_endpoint(&mut self, slot: u8, dci: u8, deq: u64) -> Result<(), &'static str> {
        let ep = ((dci as u32) << 16) | ((slot as u32) << 24);
        let (cc, _) = self.run_cmd(0, 0, 0, (TRB_RESET_EP << 10) | ep)?;
        if cc != CC_SUCCESS {
            serial_println!("[usb] Reset Endpoint cc={}", cc);
        }
        let (cc, _) = self.run_cmd(
            deq as u32,
            (deq >> 32) as u32,
            0,
            (TRB_SET_TR_DEQ << 10) | ep,
        )?;
        if cc != CC_SUCCESS {
            serial_println!("[usb] Set TR Dequeue cc={}", cc);
            return Err("Set TR Dequeue failed");
        }
        Ok(())
    }

    // commands

    /// Issue one command TRB and sleep until its completion event. Returns
    /// (completion code, slot id from the event)
    fn run_cmd(&mut self, d0: u32, d1: u32, d2: u32, ctrl: u32) -> Result<(u32, u8), &'static str> {
        let trb_phys = self.cmd.push(d0, d1, d2, ctrl);
        self.ring_doorbell(0, 0);
        // 250 ticks = 1 s
        for _ in 0..250 {
            self.poll();
            if let Some(evt) = self.last_cmd_event.take() {
                let evt_ptr = (evt[0] as u64) | ((evt[1] as u64) << 32);
                if evt_ptr == trb_phys {
                    return Ok((evt[2] >> 24, (evt[3] >> 24) as u8));
                }
                // Completion for an older command; keep waiting
            }
            sched::sleep(1);
        }
        Err("command timed out")
    }

    // port bring-up

    /// Bring up every device already on a root port.
    ///
    /// Sweeps repeatedly. Straight after HCRST port power was only just
    /// reapplied, a USB2 connect has a 100 ms debounce ahead of it and a USB3
    /// port is training its link, so one immediate sweep sees nothing. Sweeping
    /// also covers a connect whose change bits were latched before the event
    /// ring was running, which raises no port-change event
    pub fn enumerate_ports(&mut self) {
        for port in 1..=self.max_ports {
            serial_println!(
                "[usb] port {} portsc={:#x} at start",
                port, mmio_read32(self.portsc_addr(port))
            );
        }

        for _ in 0..SETTLE_ROUNDS {
            self.poll();
            for port in 1..=self.max_ports {
                let idx = (port - 1) as usize;
                if self.port_enumerated[idx] || self.port_attempts[idx] >= MAX_PORT_ATTEMPTS {
                    continue;
                }
                if mmio_read32(self.portsc_addr(port)) & PORTSC_CCS != 0 {
                    self.try_enumerate_port(port);
                }
            }
            // A keyboard specifically: settling stops when the machine can be
            // typed on. A mouse found on the way is kept but does not end the
            // search
            if self.keyboard_count() > 0 {
                return;
            }
            sched::sleep(SETTLE_TICKS);
        }
        serial_println!("[usb] no HID keyboard on this controller after settle");
    }

    fn try_enumerate_port(&mut self, port: u8) {
        let idx = (port - 1) as usize;
        self.port_attempts[idx] = self.port_attempts[idx].saturating_add(1);
        self.in_enumeration = true;
        let result = self.setup_device(port);
        self.in_enumeration = false;
        match result {
            Ok(desc) => {
                self.port_enumerated[idx] = true;
                super::status_push(desc);
            }
            Err(e) => {
                serial_println!(
                    "[usb] port {}: {} (attempt {}/{})",
                    port, e, self.port_attempts[idx], MAX_PORT_ATTEMPTS
                );
                if self.port_attempts[idx] >= MAX_PORT_ATTEMPTS {
                    super::status_push(format!("port {}: {}", port, e));
                }
            }
        }
    }

    pub fn keyboard_count(&self) -> usize {
        self.hids.iter()
            .filter(|h| matches!(h.kind, HidKind::Keyboard(_)))
            .count()
    }

    pub fn mouse_count(&self) -> usize {
        self.hids.iter()
            .filter(|h| matches!(h.kind, HidKind::Mouse | HidKind::Pointer(_)))
            .count()
    }

    /// Reset (if needed) and fully enumerate the device on a root port. Returns
    /// a human-readable description on success
    fn setup_device(&mut self, port: u8) -> Result<alloc::string::String, &'static str> {
        let psc = self.portsc_addr(port);
        let mut sc = mmio_read32(psc);

        // A port the firmware or our HCRST left unpowered reports a connect
        // that goes nowhere; power it and wait out bPwrOn2PwrGood
        if self.ppc && sc & PORTSC_PP == 0 {
            mmio_write32(psc, (sc & PORTSC_PRESERVE) | PORTSC_PP);
            sched::sleep(ms(20));
            sc = mmio_read32(psc);
        }

        if sc & PORTSC_PED == 0 {
            // USB2 port: needs an explicit reset (USB3 ports self-enable)
            mmio_write32(psc, (sc & PORTSC_PRESERVE) | PORTSC_PR);
            let mut ok = false;
            for _ in 0..125 { // 500 ms
                sc = mmio_read32(psc);
                if sc & PORTSC_PED != 0 { ok = true; break; }
                if sc & PORTSC_CCS == 0 { return Err("device left during reset"); }
                sched::sleep(1);
            }
            if !ok {
                serial_println!("[usb] port {} reset gave up, portsc={:#x}", port, sc);
                return Err("port reset did not enable the port");
            }
        }
        // ack any pending change bits from the reset/connect
        sc = mmio_read32(psc);
        mmio_write32(psc, (sc & PORTSC_PRESERVE) | (sc & PORTSC_CHANGE_BITS));

        // USB 2.0 sec 7.1.7.5: the device gets 10 ms of reset recovery before
        // it has to answer on the default pipe. Inside that window a low-speed
        // keyboard times out enumeration on real hardware, though not under
        // emulation
        sched::sleep(ms(10));
        sc = mmio_read32(psc);

        let speed = ((sc >> 10) & 0xF) as u8;
        let ep0_mps: u16 = match speed {
            SPEED_LOW | SPEED_FULL => 8,
            SPEED_HIGH => 64,
            _ => 512, // SuperSpeed and above
        };
        serial_println!("[usb] port {} enabled, speed={} ep0_mps={}", port, speed, ep0_mps);

        // Enable Slot
        let (cc, slot) = self.run_cmd(0, 0, 0, TRB_ENABLE_SLOT << 10)?;
        if cc != CC_SUCCESS || slot == 0 {
            return Err("Enable Slot failed");
        }

        // DMA blocks for this device
        let mut input = DmaBuffer::alloc(1).map_err(|_| "input ctx alloc failed")?;
        input.zero();
        let mut devctx = DmaBuffer::alloc(1).map_err(|_| "device ctx alloc failed")?;
        devctx.zero();
        let ep0_ring = Ring::new()?;
        let mut data = DmaBuffer::alloc(1).map_err(|_| "xfer buf alloc failed")?;
        data.zero();

        unsafe {
            let entry = self.dcbaa.virt_base() as u64 + (slot as u64) * 8;
            core::ptr::write_volatile(entry as *mut u64, devctx.phys());
        }

        // Input context: add slot (A0) + EP0 (A1)
        let cs = self.ctx_size();
        let ictx = input.virt_base() as u64;
        mmio_write32(ictx + 4, 0b11); // add flags A0|A1
        let slot_ctx = ictx + cs;
        mmio_write32(slot_ctx, ((speed as u32) << 20) | (1 << 27)); // ctx entries = 1
        mmio_write32(slot_ctx + 4, (port as u32) << 16);            // root hub port
        let ep0_ctx = slot_ctx + cs;
        write_ep_ctx(ep0_ctx, 4 /* control */, ep0_mps, 0, ep0_ring.buf.phys());

        // Address Device
        let (cc, _) = self.run_cmd(
            input.phys() as u32,
            (input.phys() >> 32) as u32,
            0,
            (TRB_ADDRESS_DEV << 10) | ((slot as u32) << 24),
        )?;
        if cc != CC_SUCCESS {
            serial_println!("[usb] Address Device cc={}", cc);
            return Err("Address Device failed");
        }

        // SET_ADDRESS recovery, USB 2.0 sec 9.2.6.3
        sched::sleep(ms(10));

        // Wire the EP0 ring into a temporary holder for control()
        let mut ep0 = ep0_ring;

        // Device descriptor: first 8 bytes for bMaxPacketSize0. Retried: this
        // is the first request the device sees, and a NAK past the timeout
        // would cost the whole port
        self.control_in_retry(slot, &mut ep0, 0x80, 6, 0x0100, 0, &mut data, 8)?;
        let b_mps0 = data.as_slice()[7];
        let real_mps: u16 = if speed >= SPEED_SUPER {
            1u16 << b_mps0 // SS encodes as a power of two
        } else {
            b_mps0 as u16
        };
        if real_mps != ep0_mps && real_mps != 0 {
            // Evaluate Context with the corrected EP0 max packet size
            input.zero();
            let ictx = input.virt_base() as u64;
            mmio_write32(ictx + 4, 0b10); // add A1 only
            write_ep_ctx(ictx + 2 * cs, 4, real_mps, 0, 0 /* keep ring: deq not evaluated */);
            // Evaluate Context only looks at max packet size for EP0; the
            // dequeue pointer is kept valid anyway
            let deq = ep0.trb_phys(ep0.idx) | if ep0.cycle { 1 } else { 0 };
            mmio_write64_virt(ictx + 2 * cs + 8, deq);
            let (cc, _) = self.run_cmd(
                input.phys() as u32,
                (input.phys() >> 32) as u32,
                0,
                (TRB_EVALUATE_CTX << 10) | ((slot as u32) << 24),
            )?;
            if cc != CC_SUCCESS {
                serial_println!("[usb] Evaluate Context cc={} (mps {} -> {})", cc, ep0_mps, real_mps);
            }
        }

        // Full device descriptor (18 bytes)
        self.control_in(slot, &mut ep0, 0x80, 6, 0x0100, 0, &mut data, 18)?;
        let d = data.as_slice();
        let vid = u16::from_le_bytes([d[8], d[9]]);
        let pid = u16::from_le_bytes([d[10], d[11]]);
        let dev_class = d[4];
        serial_println!(
            "[usb] port {} slot {}: device {:04x}:{:04x} class={:#x}",
            port, slot, vid, pid, dev_class
        );

        // Configuration descriptor: header first for wTotalLength, then all
        self.control_in(slot, &mut ep0, 0x80, 6, 0x0200, 0, &mut data, 9)?;
        let total = u16::from_le_bytes([data.as_slice()[2], data.as_slice()[3]])
            .min(4096) as u16;
        self.control_in(slot, &mut ep0, 0x80, 6, 0x0200, 0, &mut data, total)?;

        // Walk descriptors: find a HID boot keyboard interface + its INT-IN ep
        let cfg = data.as_slice()[..total as usize].to_vec();
        let cfg_value = cfg[5];
        let mut kbd_iface: Option<u8> = None;
        let mut ep_addr: u8 = 0;
        let mut ep_mps: u16 = 8;
        let mut ep_interval: u8 = 10;
        // bInterfaceProtocol under the HID boot subclass: 1 keyboard, 2 mouse
        let mut hid_proto: u8 = 0;
        // Whether that interface offers the boot subclass. One that does not is
        // still usable (a tablet, a touch screen, QEMU's `usb-tablet`), but its
        // reports are defined by its report descriptor, whose length is picked
        // up here
        let mut hid_boot = false;
        let mut report_desc_len: u16 = 0;
        {
            let mut off = 0usize;
            let mut in_kbd_iface = false;
            while off + 2 <= cfg.len() {
                let len = cfg[off] as usize;
                let dtype = cfg[off + 1];
                if len < 2 || off + len > cfg.len() { break; }
                match dtype {
                    4 => { // interface
                        let class = cfg[off + 5];
                        let sub   = cfg[off + 6];
                        let proto = cfg[off + 7];
                        in_kbd_iface = class == 3;
                        if in_kbd_iface && kbd_iface.is_none() {
                            kbd_iface = Some(cfg[off + 2]);
                            hid_proto = proto;
                            hid_boot = sub == 1 && (proto == 1 || proto == 2);
                        }
                    }
                    0x21 => { // HID descriptor, subordinate to the interface
                        if in_kbd_iface && kbd_iface.is_some()
                            && report_desc_len == 0 && len >= 9
                            && cfg[off + 6] == 0x22
                        {
                            report_desc_len =
                                u16::from_le_bytes([cfg[off + 7], cfg[off + 8]]).min(1024);
                        }
                    }
                    5 => { // endpoint
                        if in_kbd_iface
                            && kbd_iface.is_some()
                            && ep_addr == 0
                            && cfg[off + 2] & 0x80 != 0        // IN
                            && cfg[off + 3] & 0x3 == 3          // interrupt
                        {
                            ep_addr = cfg[off + 2];
                            ep_mps = u16::from_le_bytes([cfg[off + 4], cfg[off + 5]]) & 0x7FF;
                            ep_interval = cfg[off + 6];
                        }
                    }
                    _ => {}
                }
                off += len;
            }
        }

        // keep the device's DMA blocks alive
        self._dev_mem.push(input);
        self._dev_mem.push(devctx);
        self._dev_mem.push(data);

        let iface = match kbd_iface {
            Some(i) if ep_addr != 0 => i,
            _ => {
                // Not a boot keyboard or mouse (hub, stick...). Left addressed;
                // still reports what it is
                self._dev_mem.push(ep0.buf);
                if dev_class == 9 {
                    // This driver does not descend into hubs, so a keyboard
                    // behind this one is invisible, and the way back to a
                    // working keyboard is 'nousb' on the boot line
                    serial_println!(
                        "[usb] port {}: USB hub {:04x}:{:04x} - devices behind it \
                         are not enumerated yet",
                        port, vid, pid
                    );
                    return Ok(format!(
                        "port {}: {:04x}:{:04x} hub (devices behind it not supported)",
                        port, vid, pid
                    ));
                }
                return Ok(format!(
                    "port {}: {:04x}:{:04x} class={:#x} (no boot HID interface, ignored)",
                    port, vid, pid, dev_class
                ));
            }
        };

        // SET_CONFIGURATION
        self.control_out(slot, &mut ep0, 0x00, 9, cfg_value as u16, 0)?;

        // Configure the interrupt-IN endpoint
        let ep_num = ep_addr & 0xF;
        let dci = ep_num * 2 + 1; // IN endpoint DCI
        let kbd_ring = Ring::new()?;

        let mut input2 = DmaBuffer::alloc(1).map_err(|_| "input ctx2 alloc failed")?;
        input2.zero();
        let ictx = input2.virt_base() as u64;
        mmio_write32(ictx + 4, 1 | (1u32 << dci)); // A0 + A(dci)
        let slot_ctx = ictx + cs;
        mmio_write32(slot_ctx, ((speed as u32) << 20) | ((dci as u32) << 27));
        mmio_write32(slot_ctx + 4, (port as u32) << 16);
        let interval = xhci_interval(speed, ep_interval);
        write_ep_ctx(
            ictx + (1 + dci as u64) * cs,
            7, // interrupt IN
            ep_mps,
            interval,
            kbd_ring.buf.phys(),
        );

        let (cc, _) = self.run_cmd(
            input2.phys() as u32,
            (input2.phys() >> 32) as u32,
            0,
            (TRB_CONFIGURE_EP << 10) | ((slot as u32) << 24),
        )?;
        if cc != CC_SUCCESS {
            serial_println!("[usb] Configure Endpoint cc={}", cc);
            return Err("Configure Endpoint failed");
        }

        // HID boot protocol + infinite idle. Some keyboards NAK SET_IDLE; the
        // error is ignored.
        //
        // Only for a device offering the boot subclass. Another device may
        // stall the request, and its report format stays the one it was sending
        if hid_boot {
            self.control_out(slot, &mut ep0, 0x21, 0x0B, 0, iface as u16)?; // SET_PROTOCOL(boot)
        }
        let _ = self.control_out(slot, &mut ep0, 0x21, 0x0A, 0, iface as u16); // SET_IDLE

        // A non-boot HID defines its reports in its report descriptor; read it
        // and look for a pointer. Nothing found means a HID with no use here (a
        // gamepad, a set of dials), left alone
        let mut layout: Option<hid_report::Pointer> = None;
        if !hid_boot && report_desc_len > 0 {
            let mut rd = DmaBuffer::alloc(1).map_err(|_| "report descriptor buf")?;
            rd.zero();
            let want = report_desc_len.min(rd.size() as u16);
            // GET_DESCRIPTOR(HID report) is addressed to the interface, not the
            // device: the request type differs
            if self.control_in(slot, &mut ep0, 0x81, 6, 0x2200, iface as u16, &mut rd, want).is_ok() {
                layout = hid_report::find_pointer(&rd.as_slice()[..want as usize]);
                match &layout {
                    Some(p) => serial_println!(
                        "[usb] slot {}: report descriptor {} bytes - {} pointer, \
                         {} bit axes, max {}, report {} bytes",
                        slot, want,
                        if p.absolute { "absolute" } else { "relative" },
                        p.x.size, p.logical_max, p.report_len),
                    None => serial_println!(
                        "[usb] slot {}: report descriptor {} bytes - no pointer in it",
                        slot, want),
                }
            }
            self._dev_mem.push(rd);
        }

        // Post the first report read for a whole packet, not the 8 bytes of a
        // boot report: a keyboard whose boot endpoint declares a larger
        // wMaxPacketSize babbles against a short TRB and halts the endpoint
        let mut report = DmaBuffer::alloc(1).map_err(|_| "report buf alloc failed")?;
        report.zero();
        let xfer_len = (ep_mps.max(8) as u32).min(report.size() as u32);
        // A HID with no boot protocol and no described pointer cannot be read.
        // It is configured and left quiet, since the keyboard decoder would
        // turn a gamepad into keystrokes
        if !hid_boot && layout.is_none() {
            self._dev_mem.push(input2);
            self._dev_mem.push(ep0.buf);
            self._dev_mem.push(report);
            serial_println!(
                "[usb] port {} slot {}: HID {:04x}:{:04x} is neither a boot device nor a \
                 pointer - left alone", port, slot, vid, pid);
            return Ok(format!("port {}: {:04x}:{:04x} HID, not readable here", port, vid, pid));
        }

        let kind = match layout {
            Some(p) => HidKind::Pointer(p),
            None if hid_proto == 2 => HidKind::Mouse,
            None => HidKind::Keyboard(hid_kbd::KbdState::new()),
        };
        let mouse = !matches!(kind, HidKind::Keyboard(_));
        let absolute = matches!(kind, HidKind::Pointer(p) if p.absolute);
        let mut kbd = Hid {
            slot,
            dci,
            ring: kbd_ring,
            report,
            xfer_len,
            kind,
            reported: false,
        };
        kbd.ring.push(
            kbd.report.phys() as u32,
            (kbd.report.phys() >> 32) as u32,
            kbd.xfer_len,
            (TRB_NORMAL << 10) | TRB_IOC | TRB_ISP,
        );
        self.ring_doorbell(slot, dci as u32);

        self._dev_mem.push(input2);
        self._dev_mem.push(ep0.buf);
        self.hids.push(kbd);
        let what = match (mouse, absolute) {
            (false, _) => "keyboard",
            (true, false) => "mouse",
            (true, true) => "tablet",
        };
        if mouse {
            // Recorded before anything moves, so "no mouse" and "a mouse nobody
            // touched" stay distinct in the status
            if absolute {
                crate::drivers::input::mouse::abs_attached();
            } else {
                crate::drivers::input::mouse::hid_attached();
            }
        }

        serial_println!(
            "[usb] port {} slot {}: HID {} online (ep {:#x} mps {} interval {})",
            port, slot, what, ep_addr, ep_mps, interval
        );
        Ok(format!(
            "port {}: {:04x}:{:04x} HID {} online",
            port, vid, pid, what
        ))
    }

    // control transfers

    /// Control IN transfer on EP0: setup + data-in + status-out
    fn control_in(
        &mut self,
        slot: u8,
        ep0: &mut Ring,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        buf: &mut DmaBuffer,
        len: u16,
    ) -> Result<usize, &'static str> {
        let setup_lo = (bm_request_type as u32)
            | ((b_request as u32) << 8)
            | ((w_value as u32) << 16);
        let setup_hi = (w_index as u32) | ((len as u32) << 16);
        // TRT=3 (IN data stage)
        ep0.push(setup_lo, setup_hi, 8, (TRB_SETUP << 10) | TRB_IDT | (3 << 16));
        ep0.push(
            buf.phys() as u32,
            (buf.phys() >> 32) as u32,
            len as u32,
            (TRB_DATA << 10) | (1 << 16), // DIR = IN
        );
        let status_phys = ep0.push(0, 0, 0, (TRB_STATUS << 10) | TRB_IOC); // status = OUT
        self.ring_doorbell(slot, 1);
        self.wait_transfer(slot, status_phys)?;
        Ok(len as usize)
    }

    /// control_in, retried after clearing a halted EP0. A failed control
    /// transfer leaves the default pipe halted, so a plain retry fails the same
    /// way
    #[allow(clippy::too_many_arguments)]
    fn control_in_retry(
        &mut self,
        slot: u8,
        ep0: &mut Ring,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        buf: &mut DmaBuffer,
        len: u16,
    ) -> Result<usize, &'static str> {
        // Two attempts: each failed one costs a transfer timeout plus two
        // command timeouts, and try_enumerate_port retries the whole port on
        // top of this
        let mut last = "control transfer failed";
        for attempt in 0..2 {
            match self.control_in(slot, ep0, bm_request_type, b_request, w_value, w_index, buf, len) {
                Ok(n) => return Ok(n),
                Err(e) => {
                    last = e;
                    serial_println!(
                        "[usb] slot {} control req={:#x} attempt {} failed: {}",
                        slot, b_request, attempt + 1, e
                    );
                    let deq = ep0.trb_phys(ep0.idx) | if ep0.cycle { 1 } else { 0 };
                    if self.reset_endpoint(slot, 1, deq).is_err() {
                        return Err(last);
                    }
                    sched::sleep(ms(20));
                }
            }
        }
        Err(last)
    }

    /// Control OUT transfer with no data stage (SET_*)
    fn control_out(
        &mut self,
        slot: u8,
        ep0: &mut Ring,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
    ) -> Result<(), &'static str> {
        let setup_lo = (bm_request_type as u32)
            | ((b_request as u32) << 8)
            | ((w_value as u32) << 16);
        let setup_hi = w_index as u32;
        ep0.push(setup_lo, setup_hi, 8, (TRB_SETUP << 10) | TRB_IDT); // TRT=0: no data
        let status_phys = ep0.push(0, 0, 0, (TRB_STATUS << 10) | TRB_IOC | (1 << 16)); // status = IN
        self.ring_doorbell(slot, 1);
        self.wait_transfer(slot, status_phys)
    }

    /// Wait for the transfer event whose TRB pointer matches 'trb_phys'
    fn wait_transfer(&mut self, slot: u8, trb_phys: u64) -> Result<(), &'static str> {
        for _ in 0..250 {
            let mut done: Option<u32> = None;
            let mut popped = false;
            while let Some(trb) = self.evt.pop() {
                popped = true;
                match (trb[3] >> 10) & 0x3F {
                    TRB_EVT_TRANSFER => {
                        let ptr = (trb[0] as u64) | ((trb[1] as u64) << 32);
                        let ev_slot = (trb[3] >> 24) as u8;
                        if ev_slot == slot && ptr == trb_phys {
                            done = Some(trb[2] >> 24);
                        } else {
                            self.handle_transfer_event(trb);
                        }
                    }
                    TRB_EVT_CMD_DONE => self.last_cmd_event = Some(trb),
                    TRB_EVT_PORT_CHG => self.note_port_change(&trb),
                    _ => {}
                }
            }
            if popped {
                mmio_write64(self.run + IR0_ERDP, self.evt.dequeue_phys() | ERDP_EHB);
            }
            if let Some(cc) = done {
                if cc == CC_SUCCESS || cc == CC_SHORT_PACKET {
                    return Ok(());
                }
                serial_println!("[usb] transfer cc={}", cc);
                return Err("transfer failed");
            }
            sched::sleep(1);
        }
        Err("transfer timed out")
    }

    pub fn describe(&self) -> alloc::string::String {
        format!(
            "xHCI {:02x}:{:02x}.{} - {} ports, {} keyboard(s), {} mouse(s)",
            self.pci.bus, self.pci.dev, self.pci.func,
            self.max_ports, self.keyboard_count(), self.mouse_count()
        )
    }
}

/// Poll 'addr' until (value & mask) == want, sleeping between reads. Timeout in
/// ms (4 ms granularity through the scheduler)
fn wait_reg(addr: u64, mask: u32, want: u32, timeout_ms: u32) -> bool {
    let ticks = (timeout_ms / 4).max(1);
    for _ in 0..ticks {
        if mmio_read32(addr) & mask == want {
            return true;
        }
        sched::sleep(1);
    }
    mmio_read32(addr) & mask == want
}

/// Fill an endpoint context: dword1 = type/CErr/MaxPacket, dwords 2-3 =
/// transfer-ring dequeue pointer with DCS=1, dword4 = average TRB length
fn write_ep_ctx(ctx: u64, ep_type: u32, max_packet: u16, interval: u8, ring_phys: u64) {
    mmio_write32(ctx, (interval as u32) << 16);
    mmio_write32(ctx + 4, (ep_type << 3) | (3 << 1) | ((max_packet as u32) << 16));
    if ring_phys != 0 {
        mmio_write64_virt(ctx + 8, ring_phys | 1); // DCS = 1
    }
    mmio_write32(ctx + 16, 8 | ((max_packet as u32) << 16)); // avg TRB len, max ESIT
}

fn mmio_write64_virt(addr: u64, val: u64) {
    mmio_write32(addr, val as u32);
    mmio_write32(addr + 4, (val >> 32) as u32);
}

/// Translate a USB bInterval into the xHCI EP-context interval exponent (period
/// = 2^interval * 125 us)
fn xhci_interval(speed: u8, b_interval: u8) -> u8 {
    match speed {
        SPEED_LOW | SPEED_FULL => {
            // bInterval is in milliseconds (1..255); floor(log2(ms * 8)). xHCI
            // 6.2.3.6 allows 3..=10 for full/low-speed interrupt endpoints
            let ms = b_interval.max(1) as u32;
            let exp = 31 - (ms * 8).leading_zeros();
            exp.clamp(3, 10) as u8
        }
        _ => {
            // HS/SS: bInterval is an exponent (period = 2^(b-1) * 125us)
            (b_interval.clamp(1, 16) - 1).min(15)
        }
    }
}
