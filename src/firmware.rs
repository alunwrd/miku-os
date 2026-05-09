// Firmware hand-off helpers: ACPI mode transition + chipset SMI disable
//
//  Even after USB legacy handoff (see src/usb_handoff.rs), several other SMM
//  sources can keep the firmware busy and wedge the OS on 'sti':
//
//   ACPI events delivered as SMI. Until the OS issues the ACPI
//   "enable" command (FADT.SMI_CMD <- FADT.ACPI_ENABLE), platform
//   power-management events (lid, button, thermal, GPE) are routed to
//   SMM. After the transition they fire as SCI through the IOAPIC
//
//   Intel chipset GLOBAL SMI. The ICH/PCH PMBASE+0x30 register
//   (SMI_EN) holds individual enables for TCO watchdog, periodic SMI,
//   APMC SMI, USB SMI, etc. We clear them all (except keeping bit 0
//   GBL_SMI_EN as-is unless safe to clear), then ACK any latched
//   status bits in SMI_STS at PMBASE+0x34
//
//   This module is best-effort: each step prints what it touched and
//   returns even if a particular controller/chipset is missing

use x86_64::instructions::port::Port;

use crate::grub;

const PCI_ADDR: u16 = 0xCF8;
const PCI_DATA: u16 = 0xCFC;

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

fn pci_read16(bus: u8, dev: u8, func: u8, off: u8) -> u16 {
    (pci_read32(bus, dev, func, off & !3) >> ((off & 2) * 8)) as u16
}

fn pci_read8(bus: u8, dev: u8, func: u8, off: u8) -> u8 {
    (pci_read32(bus, dev, func, off & !3) >> ((off & 3) * 8)) as u8
}

// ACPI enable

#[repr(C, packed)]
struct Rsdp10 {
    signature: [u8; 8],
    checksum:  u8,
    oem_id:    [u8; 6],
    revision:  u8,
    rsdt_addr: u32,
}

#[repr(C, packed)]
struct Rsdp20 {
    v10:          Rsdp10,
    length:       u32,
    xsdt_addr:    u64,
    ext_checksum: u8,
    _reserved:    [u8; 3],
}

#[repr(C, packed)]
struct SdtHeader {
    signature:      [u8; 4],
    length:         u32,
    revision:       u8,
    checksum:       u8,
    oem_id:         [u8; 6],
    oem_table_id:   [u8; 8],
    oem_revision:   u32,
    creator_id:     u32,
    creator_rev:    u32,
}

unsafe fn find_rsdp() -> Option<u64> {
    let hhdm = grub::hhdm();
    // EBDA segment (KB at 0x40E) shifted left 4
    let ebda_seg = unsafe { *((0x40E + hhdm) as *const u16) };
    let ebda_phys = (ebda_seg as u64) << 4;
    let ranges: [(u64, u64); 2] = [(ebda_phys, ebda_phys + 1024), (0xE0000, 0x100000)];
    for (start, end) in ranges {
        let mut a = start & !0xF;
        while a + 20 < end {
            let virt = (a + hhdm) as *const u8;
            let sig = unsafe { core::slice::from_raw_parts(virt, 8) };
            if sig == b"RSD PTR " {
                return Some(a);
            }
            a += 16;
        }
    }
    None
}

fn find_fadt(hhdm: u64) -> Option<u64> {
    let rsdp_phys = unsafe { find_rsdp() }?;
    let rsdp10 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp10) };
    let revision = rsdp10.revision;

    let (sdt_phys, is_xsdt) = if revision >= 2 {
        let rsdp20 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp20) };
        let xsdt = unsafe { core::ptr::addr_of!(rsdp20.xsdt_addr).read_unaligned() };
        if xsdt != 0 { (xsdt, true) } else { (rsdp10.rsdt_addr as u64, false) }
    } else {
        (rsdp10.rsdt_addr as u64, false)
    };

    let sdt_hdr = unsafe { &*((sdt_phys + hhdm) as *const SdtHeader) };
    let total_len = unsafe { core::ptr::addr_of!(sdt_hdr.length).read_unaligned() } as usize;
    if total_len < 36 { return None; }

    let entry_size = if is_xsdt { 8usize } else { 4usize };
    let entry_count = (total_len - 36) / entry_size;
    let entries_base = sdt_phys + hhdm + 36;

    for i in 0..entry_count {
        let addr = entries_base + (i * entry_size) as u64;
        let ent_phys = if is_xsdt {
            unsafe { (addr as *const u64).read_unaligned() }
        } else {
            unsafe { (addr as *const u32).read_unaligned() as u64 }
        };
        let hdr = unsafe { &*((ent_phys + hhdm) as *const SdtHeader) };
        if &hdr.signature == b"FACP" {
            return Some(ent_phys);
        }
    }
    None
}

/// Issue the ACPI mode enable command. After success, SCI_EN=1 and
/// platform power events fire as SCI rather than SMI
pub fn acpi_enable() {
    let hhdm = grub::hhdm();
    let fadt_phys = match find_fadt(hhdm) {
        Some(p) => p,
        None => {
            crate::serial_println!("[firmware] FADT not found, skipping ACPI enable");
            return;
        }
    };
    let fadt_virt = fadt_phys + hhdm;
    crate::serial_println!("[firmware] FADT @ {:#x}", fadt_phys);

    // FADT field layout (ACPI 1.0 fixed offsets):
    //   48: SMI_CMD            u32
    //   52: ACPI_ENABLE        u8
    //   53: ACPI_DISABLE       u8
    //   64: PM1A_EVT_BLK       u32
    //   68: PM1B_EVT_BLK       u32
    //   72: PM1A_CNT_BLK       u32
    //   76: PM1B_CNT_BLK       u32
    //   89: PM1_CNT_LEN        u8
    let smi_cmd: u32 = unsafe { core::ptr::read_unaligned((fadt_virt + 48) as *const u32) };
    let acpi_enable_val: u8 = unsafe { core::ptr::read_unaligned((fadt_virt + 52) as *const u8) };
    let pm1a_cnt: u32 = unsafe { core::ptr::read_unaligned((fadt_virt + 72) as *const u32) };
    let pm1b_cnt: u32 = unsafe { core::ptr::read_unaligned((fadt_virt + 76) as *const u32) };

    crate::serial_println!(
        "[firmware] SMI_CMD={:#x} ACPI_ENABLE={:#x} PM1A_CNT={:#x} PM1B_CNT={:#x}",
        smi_cmd, acpi_enable_val, pm1a_cnt, pm1b_cnt
    );

    if smi_cmd == 0 || acpi_enable_val == 0 || pm1a_cnt == 0 {
        crate::serial_println!("[firmware] ACPI enable not required (HW-reduced or already on)");
        return;
    }

    // Read PM1A_CNT, check SCI_EN (bit 0)
    let pm1a_port = (pm1a_cnt & 0xFFFF) as u16;
    let cnt0: u16 = unsafe { Port::<u16>::new(pm1a_port).read() };
    if cnt0 & 1 != 0 {
        crate::serial_println!("[firmware] SCI_EN already 1 (PM1A_CNT={:#x})", cnt0);
        return;
    }

    // Issue the enable command and poll. Per ACPI spec the firmware can
    // take several seconds; we cap at ~3 seconds-ish via spin count
    crate::serial_println!("[firmware] writing {:#x} to SMI_CMD ({:#x})", acpi_enable_val, smi_cmd);
    unsafe { Port::<u8>::new((smi_cmd & 0xFFFF) as u16).write(acpi_enable_val); }

    let mut spins = 0u32;
    loop {
        let cnt: u16 = unsafe { Port::<u16>::new(pm1a_port).read() };
        if cnt & 1 != 0 {
            crate::serial_println!("[firmware] ACPI enabled after {} spins (PM1A_CNT={:#x})", spins, cnt);
            return;
        }
        spins += 1;
        if spins > 10_000_000 {
            crate::serial_println!("[firmware] ACPI enable timed out (PM1A_CNT={:#x})", cnt);
            return;
        }
        core::hint::spin_loop();
    }
}

// GPE0 SMI DISABLE

/// Disable every bit in GPE0_EN (FADT-described general purpose event
/// enable register). Until the OS has entered ACPI mode (SCI_EN=1),
/// every enabled GPE bit fires SMI - and the BIOS keeps firing as long
/// as enable bits are set, even if the chipset SMI block enables
/// (FCH 0x210..) are zero. GPE0 routes through a SEPARATE chipset path
///
/// Probe data on Ryzen 5 2600 / B450M-HDV showed
///   [probe.fadt] PM1A_CNT live=0x0 SCI_EN=0
///   [probe.fadt] GPE0 sts=0x23 en=0xff
/// after firmware silence + ACPI enable - i.e. the OS never managed to
/// transition into ACPI mode AND all 8 GPE sources are still enabled
/// with 3 of them already latched. Writing the SMI_CMD trampoline did
/// not flip SCI_EN on this platform, so we must take the OTHER route:
/// silence GPE at the source by clearing its own enable register
pub fn disable_gpe0() {
    let hhdm = grub::hhdm();
    let fadt_phys = match find_fadt(hhdm) {
        Some(p) => p,
        None => {
            crate::serial_println!("[firmware] FADT not found, cannot disable GPE0");
            return;
        }
    };
    let v = fadt_phys + hhdm;
    let gpe0_blk: u32 = unsafe { core::ptr::read_unaligned((v + 80) as *const u32) };
    let gpe0_len: u8  = unsafe { core::ptr::read_unaligned((v + 92) as *const u8) };
    if gpe0_blk == 0 || gpe0_len < 2 {
        crate::serial_println!("[firmware] no GPE0 block in FADT");
        return;
    }
    // GPE0_LEN is total block size (status+enable). First half is status,
    // second half is enable. Each half is gpe0_len/2 bytes wide
    let half = (gpe0_len / 2) as u16;
    let port_sts = (gpe0_blk & 0xFFFF) as u16;
    let port_en  = port_sts + half;
    unsafe {
        let mut total_en  = 0u32;
        let mut total_sts = 0u32;
        for i in 0..half {
            total_sts |= (Port::<u8>::new(port_sts + i).read() as u32) << (8 * i);
            total_en  |= (Port::<u8>::new(port_en  + i).read() as u32) << (8 * i);
        }
        // Clear every enable bit and ACK every status bit. RWC: writing
        // 1 to a status bit clears it
        for i in 0..half {
            Port::<u8>::new(port_en  + i).write(0x00);
            Port::<u8>::new(port_sts + i).write(0xFF);
        }
        crate::cprintln!(180, 180, 180,
            "[firmware] GPE0 disabled (was sts={:#x} en={:#x})",
            total_sts, total_en);
    }
}

// intel chipset smi disable

/// Find the LPC bridge (Intel ICH/PCH) and clear all SMI enables in
/// PMBASE+SMI_EN. Idempotent. AMD or non-Intel platforms: noop
pub fn disable_intel_chipset_smi() {
    // Walk PCI bus 0 looking for an Intel ISA/LPC bridge
    //   class=0x06 (bridge), subclass=0x01 (ISA bridge) is LPC on Intel
    //   vendor must be 0x8086
    for dev in 0..32u8 {
        for func in 0..8u8 {
            let id = pci_read32(0, dev, func, 0x00);
            if (id & 0xFFFF) as u16 != 0x8086 {
                if func == 0 { break; }
                continue;
            }
            let class_rev = pci_read32(0, dev, func, 0x08);
            let class    = (class_rev >> 24) as u8;
            let subclass = (class_rev >> 16) as u8;
            if class == 0x06 && subclass == 0x01 {
                let device = (id >> 16) as u16;
                crate::serial_println!(
                    "[firmware] Intel LPC at 0:{:02x}.{} device={:04x}",
                    dev, func, device
                );
                clear_smi_for_lpc(dev, func);
                return;
            }
            if func == 0 && (pci_read8(0, dev, func, 0x0E) & 0x80) == 0 { break; }
        }
    }
    crate::serial_println!("[firmware] no Intel LPC found, skipping chipset SMI clear");
}

fn clear_smi_for_lpc(dev: u8, func: u8) {
    // PMBASE register location varies by chipset:
    //     ICH (older): config offset 0x40, 16-bit, low 7 bits zero
    //     PCH (modern): config offset 0x40, 16-bit, low 7 bits zero
    //                   (ABASE - ACPI Base Address)
    // Either way: read 16 bits at 0x40, mask off low 7 bits, that is the
    // I/O base for ACPI block. SMI_EN at base+0x30, SMI_STS at base+0x34
    let pmbase_raw = pci_read16(0, dev, func, 0x40);
    let pmbase = pmbase_raw & 0xFF80;
    if pmbase == 0 {
        crate::serial_println!("[firmware] LPC PMBASE not set, skipping SMI clear");
        return;
    }
    let smi_en_port  = pmbase + 0x30;
    let smi_sts_port = pmbase + 0x34;
    crate::serial_println!(
        "[firmware] PMBASE={:#x} SMI_EN={:#x} SMI_STS={:#x}",
        pmbase, smi_en_port, smi_sts_port
    );

    unsafe {
        let mut en_port: Port<u32> = Port::new(smi_en_port);
        let mut sts_port: Port<u32> = Port::new(smi_sts_port);

        let en0 = en_port.read();
        let sts0 = sts_port.read();
        crate::serial_println!(
            "[firmware] SMI_EN before={:#x} SMI_STS before={:#x}", en0, sts0
        );

        // Clear every SMI enable except bit 0 (GBL_SMI_EN). On most
        // chipsets the global enable just gates the rest; clearing all
        // sub-enables makes it harmless. Touching bit 0 is risky on some
        // platforms (BIOS may need it for CPU thermal throttling)
        en_port.write(en0 & 1);
        // ACK every latched status (RWC: write 1s clears)
        sts_port.write(0xFFFF_FFFF);

        let en1 = en_port.read();
        let sts1 = sts_port.read();
        crate::serial_println!(
            "[firmware] SMI_EN after={:#x} SMI_STS after={:#x}", en1, sts1
        );
    }
}


// HIGH-LEVEL ENTRY
// AMD CHIPSET SMI DISABLE (FCH / SP5100 / Promontory)

/// AMD platforms (FCH / SP5100 / Promontory) route platform SMI through
/// the AcpiMmio block at fixed physical address 0xFED8_0000. Layout:
///   +0x000 SMBus              (we don't touch)
///   +0x100 GPIO               (we don't touch)
///   +0x200 SMI block          (we clear EVERY register here)
///   +0x300 PMIO
///   +0x400 PMIO2
///   ...
///   +0xB00 Watchdog Timer     (we disable this too)
pub fn disable_amd_chipset_smi() {
    let mut found = false;
    let mut sample_vendor = 0u16;
    let mut sample_device = 0u16;
    for dev in 0..32u8 {
        for func in 0..8u8 {
            let id = pci_read32(0, dev, func, 0x00);
            let vendor = (id & 0xFFFF) as u16;
            if vendor == 0x1022 || vendor == 0x1002 {
                found = true;
                sample_vendor = vendor;
                sample_device = (id >> 16) as u16;
                break;
            }
            if func == 0 && (pci_read8(0, dev, func, 0x0E) & 0x80) == 0 { break; }
        }
        if found { break; }
    }
    if !found {
        crate::serial_println!("[firmware] no AMD chipset, skipping AMD SMI clear");
        return;
    }
    crate::cprintln!(180, 180, 180,
        "[firmware] AMD chipset detected (sample vendor={:04x} device={:04x})",
        sample_vendor, sample_device);

    let acpimmio_phys: u64 = 0xFED8_0000;
    crate::vmm::map_mmio_uc(acpimmio_phys, 0x1000);
    let mmio_virt = acpimmio_phys + grub::hhdm();

    // Conservative AMD FCH SMI clear. We touch ONLY the event enable
    // and status banks (0x200..0x21F), leaving the control/timer bits
    // (0x80..0xBF) alone. The AMD FCH layout from BKDG/PPR:
    //   0x200..0x20F: SMI_EVENT_STATUS_0..3 (RWC)
    //   0x210..0x21F: SMI_EVENT_ENABLE_0..3 (RW)
    //
    // Earlier we cleared the entire 0x200..0x300 region and the next
    // LAPIC/IOAPIC access wedged - the FCH evidently takes critical
    // control bits in 0x80..0xBF that we must not touch.
    unsafe {
        let event_status = (mmio_virt + 0x200) as *mut u32;
        let event_enable = (mmio_virt + 0x210) as *mut u32;

        let mut nonzero = 0u32;
        for i in 0..4 {
            let v = core::ptr::read_volatile(event_enable.add(i));
            if v != 0 { nonzero += 1; }
            core::ptr::write_volatile(event_enable.add(i), 0);
            // RWC: writing 1s clears status bits.
            core::ptr::write_volatile(event_status.add(i), 0xFFFF_FFFF);
        }
        crate::cprintln!(180, 180, 180,
            "[firmware] AMD SMI_EVENT_ENABLE: disabled {} non-zero banks", nonzero);
    }
    crate::serial_println!("[firmware] AMD event-side SMI cleared (conservative)");
}

// Disable any HPET timer that the firmware programmed for SMI delivery.
// HPET sits at the standard MMIO address 0xFED0_0000 (per IA-PC HPET
// spec; rare to be elsewhere on consumer hardware)
//
// HPET layout:
//   0x000 GCAP_ID        - capability id
//   0x010 GEN_CONF       - bit 0 ENABLE_CNF, bit 1 LEG_RT_CNF
//   0x020 GINTR_STA      - per-timer interrupt status (RWC)
//   0x100 + 0x20*N       - per-timer N config:
//                          bit 1 INT_TYPE (level)
//                          bit 2 INT_ENB
//                          bit 14 FSB_INT (MSI mode)
//                          bits 9..13 INT_ROUTE (IOAPIC GSI)
// Timer configs sometimes have bits set that route interrupts as SMI on
// real silicon even though the spec calls them GSI. We disable every timer entirely
pub fn disable_hpet_smi() {
    let hpet_phys: u64 = 0xFED0_0000;
    crate::vmm::map_mmio_uc(hpet_phys, 0x1000);
    let mmio_virt = hpet_phys + grub::hhdm();

    unsafe {
        let cap = (mmio_virt + 0x000) as *const u64;
        let conf = (mmio_virt + 0x010) as *mut u64;
        let cap_v = core::ptr::read_volatile(cap);
        let conf0 = core::ptr::read_volatile(conf);
        // Sanity: cap[15:0] should be 0x8086 / 0x4353 / etc, not 0xFFFF
        if cap_v == 0 || cap_v == !0u64 {
            crate::cprintln!(180, 180, 180,
                "[firmware] HPET not present at {:#x}", hpet_phys);
            return;
        }
        let num_timers = ((cap_v >> 8) & 0x1F) as usize + 1;
        crate::cprintln!(180, 180, 180,
            "[firmware] HPET cap={:#x} conf={:#x} timers={}",
            cap_v as u32, conf0 as u32, num_timers);

        // Disable every per-timer interrupt.
        for n in 0..num_timers.min(8) {
            let cfg_addr = mmio_virt + 0x100 + (0x20 * n as u64);
            let cfg = cfg_addr as *mut u64;
            let v = core::ptr::read_volatile(cfg);
            // Clear INT_ENB (bit 2), TYPE (bit 1), FSB_INT (bit 14), and
            // make routing 0 (bits 9..13). Preserve cap bits
            let new = v & !((1 << 1) | (1 << 2) | (1 << 14) | (0x1F << 9));
            if v != new {
                core::ptr::write_volatile(cfg, new);
            }
        }
        // Disable HPET counter and legacy replacement
        core::ptr::write_volatile(conf, conf0 & !0b11u64);
        // Ack any latched status.
        let gsta = (mmio_virt + 0x020) as *mut u64;
        core::ptr::write_volatile(gsta, !0u64);
    }
    crate::serial_println!("[firmware] HPET disabled");
}

// madt-driven lvt conf

fn find_madt(hhdm: u64) -> Option<u64> {
    let rsdp_phys = unsafe { find_rsdp() }?;
    let rsdp10 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp10) };
    let revision = rsdp10.revision;
    let (sdt_phys, is_xsdt) = if revision >= 2 {
        let r2 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp20) };
        let xsdt = unsafe { core::ptr::addr_of!(r2.xsdt_addr).read_unaligned() };
        if xsdt != 0 { (xsdt, true) } else { (rsdp10.rsdt_addr as u64, false) }
    } else {
        (rsdp10.rsdt_addr as u64, false)
    };
    let hdr = unsafe { &*((sdt_phys + hhdm) as *const SdtHeader) };
    let total = unsafe { core::ptr::addr_of!(hdr.length).read_unaligned() } as usize;
    if total < 36 { return None; }
    let esz = if is_xsdt { 8 } else { 4 };
    let count = (total - 36) / esz;
    let base = sdt_phys + hhdm + 36;
    for i in 0..count {
        let a = base + (i * esz) as u64;
        let p = if is_xsdt {
            unsafe { (a as *const u64).read_unaligned() }
        } else {
            unsafe { (a as *const u32).read_unaligned() as u64 }
        };
        let h = unsafe { &*((p + hhdm) as *const SdtHeader) };
        if &h.signature == b"APIC" { return Some(p); }
    }
    None
}

// Honor ACPI MADT "LAPIC NMI" (type 4) entries by programming
// LVT_LINT0/LINT1 with delivery mode = NMI on the BSP. Without this
// step the BIOS keeps asserting LINT1 (it expects NMI delivery per
// MADT), our LVT in FIX mode translates the signal to vector 0xff
// (spurious), our spurious handler returns WITHOUT EOI as required
// by SDM 10.9, and the BIOS just asserts LINT1 again - infinite
// spurious-vector loop in which 'sti' never effectively returns to user code
//
// Probe data on Ryzen 5 2600 / B450M-HDV showed
//   [probe.madt] LAPIC NMI: cpu=0xff lint=1 flags=0x5
// meaning all CPUs, LINT1, edge-triggered, active-high. With this
// function honoring the entry, our nmi_handler runs, paints slot 9
// white, and we get out of the loop
//
// LINT pins NOT covered by an MADT entry are masked defensively
// (vector=0xff, FIX mode, mask bit set) so a stray hardware signal  cannot deliver garbage
pub fn configure_lapic_per_madt() {
    let hhdm = grub::hhdm();
    let madt_phys = match find_madt(hhdm) {
        Some(p) => p,
        None => {
            crate::serial_println!("[firmware] MADT not found, LVT defaults retained");
            return;
        }
    };
    let total = unsafe {
        let h = &*((madt_phys + hhdm) as *const SdtHeader);
        core::ptr::addr_of!(h.length).read_unaligned()
    } as u64;

    // Defaults: both pins fully masked
    let mut lint0_val: u32 = (1 << 16) | 0xFF;
    let mut lint1_val: u32 = (1 << 16) | 0xFF;
    let mut lint0_nmi = false;
    let mut lint1_nmi = false;

    let bsp_id = crate::apic::lapic_id() as u8;
    let mut off = 36u64 + 8;
    while off + 2 < total {
        let etype = unsafe { core::ptr::read_unaligned(
            (madt_phys + hhdm + off) as *const u8) };
        let elen  = unsafe { core::ptr::read_unaligned(
            (madt_phys + hhdm + off + 1) as *const u8) };
        if elen < 2 { break; }
        if etype == 4 {
            let cpu  = unsafe { core::ptr::read_unaligned(
                (madt_phys + hhdm + off + 2) as *const u8) };
            let flags = unsafe { core::ptr::read_unaligned(
                (madt_phys + hhdm + off + 3) as *const u16) };
            let lint = unsafe { core::ptr::read_unaligned(
                (madt_phys + hhdm + off + 5) as *const u8) };
            // cpu == 0xFF means "all processors" per ACPI spec
            if cpu == 0xFF || cpu == bsp_id {
                // LVT bits:
                //  8..10  delivery mode = 100b (NMI)
                //  13     polarity (0=active high, 1=active low)
                //  15     trigger mode (0=edge, 1=level)
                //  16     mask = 0 (mask is ignored for NMI delivery)
                let polarity = flags & 0x3;
                let trigger  = (flags >> 2) & 0x3;
                let pol_bit:  u32 = if polarity == 0x3 { 1 << 13 } else { 0 };
                let trig_bit: u32 = if trigger  == 0x3 { 1 << 15 } else { 0 };
                let val: u32 = (4 << 8) | pol_bit | trig_bit;
                match lint {
                    0 => { lint0_val = val; lint0_nmi = true; }
                    1 => { lint1_val = val; lint1_nmi = true; }
                    _ => {}
                }
            }
        }
        off += elen as u64;
    }

    unsafe {
        crate::apic::lapic_write(crate::apic::LAPIC_LVT_LINT0, lint0_val);
        crate::apic::lapic_write(crate::apic::LAPIC_LVT_LINT1, lint1_val);
    }
    crate::cprintln!(180, 180, 180,
        "[firmware] LVT LINT0={:#x}({}) LINT1={:#x}({})",
        lint0_val, if lint0_nmi { "NMI" } else { "masked" },
        lint1_val, if lint1_nmi { "NMI" } else { "masked" });
}

// Run all firmware-handoff steps in the right order. Prints a one-line
// summary on the framebuffer so the user can see what actually fired without needing serial
pub fn run() {
    let cpu_vendor = detect_cpu_vendor();
    crate::cprintln!(180, 180, 180, "[firmware] cpu vendor: {}", cpu_vendor);

    acpi_enable();
    disable_intel_chipset_smi();
    disable_amd_chipset_smi();
    disable_hpet_smi();
    disable_gpe0();
    configure_lapic_per_madt();
}

fn detect_cpu_vendor() -> &'static str {
    let ebx: u32;
    let edx: u32;
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            inout("eax") 0u32 => _,
            out("edx") edx,
            out("ecx") ecx,
            options(preserves_flags),
        );
    }
    let bytes: [u8; 12] = unsafe { core::mem::transmute([ebx, edx, ecx]) };
    if &bytes == b"GenuineIntel" { "Intel" }
    else if &bytes == b"AuthenticAMD" { "AMD" }
    else { "Unknown" }
}
