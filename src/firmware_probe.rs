//! Read-only dump of firmware-controlled SMI/NMI/IRQ sources.
//!
//! Goal: photograph the output once and know exactly which register the
//! BIOS left armed. Replaces the guess-and-build cycle: until now we
//! were clearing AMD FCH SMI banks blind. With this module the user
//! sees ALL the candidate sources in one screen and we can pick the
//! exact bit to clear.
//!
//! Sections (in print order, ~25 lines fit on one framebuffer page):
//!   1. CPU vendor + family/model + SMM MSRs (HWCR, SMM_BASE/ADDR/MASK)
//!   2. LAPIC current state (SVR/TPR/ESR + every LVT entry + IRR/ISR)
//!   3. HPET (cap, conf, per-timer cfg with routing)
//!   4. AMD FCH AcpiMmio: SMI bank 0x200..0x21F, watchdog 0xB00,
//!      first 4 nonzero offsets in PMIO 0x300..0x400
//!   5. ACPI MADT: NMI Source + LAPIC NMI entries (LIKELY culprit
//!      for unmaskable LINT1 NMI on AMD legacy boot - per Intel SDM
//!      11.5.1, mask bit is IGNORED when LVT delivery mode is NMI)
//!   6. ACPI FADT: SMI_CMD, ACPI_EN/DIS, PM1A_CNT live read (SCI_EN
//!      bit), GPE0 enable/status live read
//!   7. PCI bus 0: USB controllers + LPC bridge + SMBus (FCH PMBASE)

use core::arch::asm;
use x86_64::instructions::port::Port;

use crate::{cprintln, grub, serial_println};

const PCI_ADDR: u16 = 0xCF8;
const PCI_DATA: u16 = 0xCFC;

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, preserves_flags),
    );
    ((hi as u64) << 32) | (lo as u64)
}

fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx_out,
            inout("eax") leaf => eax_out,
            inout("ecx") sub => ecx_out,
            out("edx") edx_out,
            options(preserves_flags),
        );
    }
    (eax_out, ebx_out, ecx_out, edx_out)
}

fn cpu_vendor() -> [u8; 12] {
    let (_, ebx, ecx, edx) = cpuid(0, 0);
    unsafe { core::mem::transmute([ebx, edx, ecx]) }
}

fn is_amd() -> bool { &cpu_vendor() == b"AuthenticAMD" }

fn pci_addr_word(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    0x8000_0000
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) << 8)
        | ((off  as u32) & 0xFC)
}

fn pci_read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    unsafe {
        Port::<u32>::new(PCI_ADDR).write(pci_addr_word(bus, dev, func, off));
        Port::<u32>::new(PCI_DATA).read()
    }
}

fn pci_read8(bus: u8, dev: u8, func: u8, off: u8) -> u8 {
    (pci_read32(bus, dev, func, off & !3) >> ((off & 3) * 8)) as u8
}

pub fn dump() {
    cprintln!(120, 200, 255, "==== firmware probe ====");
    dump_cpu();
    dump_lapic();
    dump_hpet();
    dump_amd_fch();
    dump_acpi();
    dump_pci_bus0();
    cprintln!(120, 200, 255, "========================");
}

fn dump_cpu() {
    let v = cpu_vendor();
    let (eax, _, _, _) = cpuid(1, 0);
    let stepping = eax & 0xF;
    let model    = ((eax >> 4) & 0xF) | ((eax >> 12) & 0xF0);
    let family   = ((eax >> 8) & 0xF) + ((eax >> 20) & 0xFF);
    let vendor_str = core::str::from_utf8(&v).unwrap_or("?");
    cprintln!(200, 200, 200,
        "[probe.cpu] {} fam={:#x} model={:#x} step={}",
        vendor_str, family, model, stepping);

    if is_amd() {
        unsafe {
            let hwcr = read_msr(0xC001_0015);
            let smm_lock = (hwcr >> 0) & 1;
            let smm_dis  = (hwcr >> 31) & 1;
            cprintln!(200, 200, 200,
                "[probe.cpu] HWCR={:#018x} SmmLock={} SmmDisable={}",
                hwcr, smm_lock, smm_dis);
            let smm_base = read_msr(0xC001_0111);
            let smm_addr = read_msr(0xC001_0112);
            let smm_mask = read_msr(0xC001_0113);
            cprintln!(200, 200, 200,
                "[probe.cpu] SMM_BASE={:#x} TSEG_ADDR={:#x} TSEG_MASK={:#x}",
                smm_base, smm_addr, smm_mask);
        }
    }
}

fn dump_lapic() {
    use crate::apic;
    unsafe {
        let svr    = apic::lapic_read(apic::LAPIC_SVR);
        let tpr    = apic::lapic_read(apic::LAPIC_TPR);
        let esr    = apic::lapic_read(apic::LAPIC_ESR);
        let lvt_t  = apic::lapic_read(apic::LAPIC_LVT_TIMER);
        let lvt_l0 = apic::lapic_read(apic::LAPIC_LVT_LINT0);
        let lvt_l1 = apic::lapic_read(apic::LAPIC_LVT_LINT1);
        let lvt_e  = apic::lapic_read(apic::LAPIC_LVT_ERROR);
        let lvt_pf = apic::lapic_read(apic::LAPIC_LVT_PERF);
        let lvt_th = apic::lapic_read(apic::LAPIC_LVT_THERMAL);
        let lvt_cm = apic::lapic_read(apic::LAPIC_LVT_CMCI);
        cprintln!(200, 200, 200,
            "[probe.lapic] SVR={:#x} TPR={:#x} ESR={:#x}", svr, tpr, esr);
        let mode = |v: u32| -> &'static str {
            match (v >> 8) & 7 {
                0 => "FIX", 2 => "SMI", 4 => "NMI", 5 => "INI",
                7 => "EXT", _ => "?",
            }
        };
        let masked = |v: u32| if (v >> 16) & 1 != 0 { 'M' } else { 'U' };
        cprintln!(200, 200, 200,
            "[probe.lapic] tim={:#x}({}{}) l0={:#x}({}{}) l1={:#x}({}{})",
            lvt_t, masked(lvt_t), mode(lvt_t),
            lvt_l0, masked(lvt_l0), mode(lvt_l0),
            lvt_l1, masked(lvt_l1), mode(lvt_l1));
        cprintln!(200, 200, 200,
            "[probe.lapic] err={:#x}({}) prf={:#x}({}) thr={:#x}({}) cmc={:#x}({})",
            lvt_e,  masked(lvt_e),
            lvt_pf, masked(lvt_pf),
            lvt_th, masked(lvt_th),
            lvt_cm, masked(lvt_cm));

        let irr = apic::read_irr();
        let mut irr_pending: i32 = -1;
        for i in 0..8 {
            if irr[i] != 0 {
                irr_pending = (i as i32) * 32 + irr[i].trailing_zeros() as i32;
                break;
            }
        }
        let mut isr_pending: i32 = -1;
        for i in 0..8 {
            let w = apic::lapic_read(apic::LAPIC_ISR_BASE + (i as u32) * 0x10);
            if w != 0 {
                isr_pending = (i as i32) * 32 + w.trailing_zeros() as i32;
                break;
            }
        }
        if irr_pending >= 0 || isr_pending >= 0 {
            cprintln!(255, 100, 100,
                "[probe.lapic] PENDING irr_vec={} isr_vec={} <SUSPECT>",
                irr_pending, isr_pending);
        } else {
            cprintln!(150, 200, 150, "[probe.lapic] IRR/ISR clean");
        }
    }
}

fn dump_hpet() {
    let phys: u64 = 0xFED0_0000;
    crate::vmm::map_mmio_uc(phys, 0x1000);
    let virt = phys + grub::hhdm();
    unsafe {
        let cap_v = core::ptr::read_volatile(virt as *const u64);
        if cap_v == 0 || cap_v == !0u64 {
            cprintln!(200, 200, 200, "[probe.hpet] not present");
            return;
        }
        let conf    = core::ptr::read_volatile((virt + 0x010) as *const u64);
        let timers  = ((cap_v >> 8) & 0x1F) + 1;
        let leg_cap = (cap_v >> 15) & 1;
        cprintln!(200, 200, 200,
            "[probe.hpet] cap={:#x} timers={} leg_cap={} conf={:#x} en={} legrt={}",
            cap_v as u32, timers, leg_cap, conf, conf & 1, (conf >> 1) & 1);
        for n in 0..timers.min(4) {
            let cfg = core::ptr::read_volatile(
                (virt + 0x100 + 0x20 * n) as *const u64);
            let route = (cfg >> 9) & 0x1F;
            cprintln!(200, 200, 200,
                "[probe.hpet]  T{} cfg={:#x} en={} type={} fsb={} gsi={}",
                n, cfg as u32, (cfg >> 2) & 1, (cfg >> 1) & 1,
                (cfg >> 14) & 1, route);
        }
    }
}

fn dump_amd_fch() {
    if !is_amd() { return; }
    let phys: u64 = 0xFED8_0000;
    crate::vmm::map_mmio_uc(phys, 0x1000);
    let virt = phys + grub::hhdm();
    unsafe {
        let st: [u32; 4] = [
            core::ptr::read_volatile((virt + 0x200) as *const u32),
            core::ptr::read_volatile((virt + 0x204) as *const u32),
            core::ptr::read_volatile((virt + 0x208) as *const u32),
            core::ptr::read_volatile((virt + 0x20C) as *const u32),
        ];
        let en: [u32; 4] = [
            core::ptr::read_volatile((virt + 0x210) as *const u32),
            core::ptr::read_volatile((virt + 0x214) as *const u32),
            core::ptr::read_volatile((virt + 0x218) as *const u32),
            core::ptr::read_volatile((virt + 0x21C) as *const u32),
        ];
        let any = st[0] | st[1] | st[2] | st[3] | en[0] | en[1] | en[2] | en[3];
        let (cr, cg, cb) = if any != 0 { (255, 100, 100) } else { (150, 200, 150) };
        cprintln!(cr, cg, cb,
            "[probe.fch] SMI sts {:08x} {:08x} {:08x} {:08x}",
            st[0], st[1], st[2], st[3]);
        cprintln!(cr, cg, cb,
            "[probe.fch] SMI en  {:08x} {:08x} {:08x} {:08x}",
            en[0], en[1], en[2], en[3]);

        let mut found = 0u32;
        for off in (0x300u64..0x400).step_by(4) {
            let v = core::ptr::read_volatile((virt + off) as *const u32);
            if v != 0 && v != 0xFFFF_FFFF && found < 4 {
                cprintln!(220, 200, 150,
                    "[probe.fch] PMIO[{:#05x}]={:#010x}", off, v);
                found += 1;
            }
        }
        let wdt_b00 = core::ptr::read_volatile((virt + 0xB00) as *const u32);
        cprintln!(200, 200, 200,
            "[probe.fch] WDT[0xB00]={:#010x}", wdt_b00);
    }
    serial_println!("[probe.fch] full AcpiMmio[0..0x300] dump on serial");
    dump_mmio_serial(phys + grub::hhdm(), 0x300);
}

fn dump_mmio_serial(virt: u64, len: u64) {
    for row in 0..(len / 16) {
        let mut b = [0u8; 16];
        for i in 0..16 {
            unsafe {
                b[i] = core::ptr::read_volatile(
                    (virt + row * 16 + i as u64) as *const u8);
            }
        }
        serial_println!(
            "[probe] {:04x}: {:02x}{:02x}{:02x}{:02x} {:02x}{:02x}{:02x}{:02x} {:02x}{:02x}{:02x}{:02x} {:02x}{:02x}{:02x}{:02x}",
            row * 16,
            b[0],  b[1],  b[2],  b[3],
            b[4],  b[5],  b[6],  b[7],
            b[8],  b[9],  b[10], b[11],
            b[12], b[13], b[14], b[15]);
    }
}

#[repr(C, packed)]
struct Rsdp10 {
    sig: [u8; 8], cksum: u8, oem: [u8; 6], rev: u8, rsdt: u32,
}
#[repr(C, packed)]
struct Rsdp20 {
    v10: Rsdp10, length: u32, xsdt: u64, ext_cksum: u8, rsv: [u8; 3],
}
#[repr(C, packed)]
struct SdtHdr {
    sig: [u8; 4], length: u32, rev: u8, cksum: u8, oem: [u8; 6],
    table_id: [u8; 8], oem_rev: u32, creator: u32, creator_rev: u32,
}

unsafe fn find_rsdp() -> Option<u64> {
    let hhdm = grub::hhdm();
    let ebda_seg = unsafe { *((0x40E + hhdm) as *const u16) };
    let ebda_phys = (ebda_seg as u64) << 4;
    let ranges: [(u64, u64); 2] = [(ebda_phys, ebda_phys + 1024), (0xE0000, 0x100000)];
    for (s, e) in ranges {
        let mut a = s & !0xF;
        while a + 20 < e {
            let p = (a + hhdm) as *const u8;
            let sig = unsafe { core::slice::from_raw_parts(p, 8) };
            if sig == b"RSD PTR " { return Some(a); }
            a += 16;
        }
    }
    None
}

fn find_table(sig: &[u8; 4]) -> Option<u64> {
    let hhdm = grub::hhdm();
    let rsdp_phys = unsafe { find_rsdp() }?;
    let rsdp10 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp10) };
    let revision = rsdp10.rev;
    let (sdt_phys, is_xsdt) = if revision >= 2 {
        let r2 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp20) };
        let xsdt = unsafe { core::ptr::addr_of!(r2.xsdt).read_unaligned() };
        if xsdt != 0 { (xsdt, true) } else { (rsdp10.rsdt as u64, false) }
    } else {
        (rsdp10.rsdt as u64, false)
    };
    let hdr = unsafe { &*((sdt_phys + hhdm) as *const SdtHdr) };
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
        let h = unsafe { &*((p + hhdm) as *const SdtHdr) };
        if &h.sig == sig { return Some(p); }
    }
    None
}

fn dump_acpi() {
    let hhdm = grub::hhdm();

    if let Some(madt_phys) = find_table(b"APIC") {
        let hdr = unsafe { &*((madt_phys + hhdm) as *const SdtHdr) };
        let total = unsafe { core::ptr::addr_of!(hdr.length).read_unaligned() } as u64;
        let mut off = 36u64 + 8;
        let mut suspect = 0u32;
        while off + 2 < total {
            // MADT entries are byte-packed: type/length at off+0..1 then
            // a payload that can land on any alignment. read_unaligned
            // is mandatory for the multi-byte fields.
            let etype = unsafe { core::ptr::read_unaligned(
                (madt_phys + hhdm + off) as *const u8) };
            let elen  = unsafe { core::ptr::read_unaligned(
                (madt_phys + hhdm + off + 1) as *const u8) };
            if elen < 2 { break; }
            match etype {
                3 => {
                    let flags = unsafe { core::ptr::read_unaligned(
                        (madt_phys + hhdm + off + 2) as *const u16) };
                    let gsi = unsafe { core::ptr::read_unaligned(
                        (madt_phys + hhdm + off + 4) as *const u32) };
                    cprintln!(255, 120, 120,
                        "[probe.madt] NMI Source: gsi={} flags={:#x} <SUSPECT>",
                        gsi, flags);
                    suspect += 1;
                }
                4 => {
                    let cpu  = unsafe { core::ptr::read_unaligned(
                        (madt_phys + hhdm + off + 2) as *const u8) };
                    let flags = unsafe { core::ptr::read_unaligned(
                        (madt_phys + hhdm + off + 3) as *const u16) };
                    let lint = unsafe { core::ptr::read_unaligned(
                        (madt_phys + hhdm + off + 5) as *const u8) };
                    cprintln!(255, 120, 120,
                        "[probe.madt] LAPIC NMI: cpu={:#x} lint={} flags={:#x} <SUSPECT>",
                        cpu, lint, flags);
                    suspect += 1;
                }
                _ => {}
            }
            off += elen as u64;
        }
        if suspect == 0 {
            cprintln!(150, 200, 150, "[probe.madt] no NMI source entries");
        }
    } else {
        cprintln!(255, 120, 120, "[probe.madt] not found");
    }

    if let Some(fadt_phys) = find_table(b"FACP") {
        let v = fadt_phys + hhdm;
        unsafe {
            let smi_cmd: u32  = core::ptr::read_unaligned((v + 48) as *const u32);
            let acpi_en: u8   = core::ptr::read_unaligned((v + 52) as *const u8);
            let acpi_di: u8   = core::ptr::read_unaligned((v + 53) as *const u8);
            let pm1a_evt: u32 = core::ptr::read_unaligned((v + 56) as *const u32);
            let pm1a_cnt: u32 = core::ptr::read_unaligned((v + 64) as *const u32);
            let gpe0_blk: u32 = core::ptr::read_unaligned((v + 80) as *const u32);
            let gpe0_len: u8  = core::ptr::read_unaligned((v + 92) as *const u8);
            cprintln!(200, 200, 200,
                "[probe.fadt] SMI_CMD={:#x} EN={:#x} DIS={:#x}",
                smi_cmd, acpi_en, acpi_di);
            cprintln!(200, 200, 200,
                "[probe.fadt] PM1A_EVT={:#x} PM1A_CNT={:#x} GPE0={:#x}/{}",
                pm1a_evt, pm1a_cnt, gpe0_blk, gpe0_len);
            if pm1a_cnt != 0 {
                let live = Port::<u16>::new((pm1a_cnt & 0xFFFF) as u16).read();
                cprintln!(200, 200, 200,
                    "[probe.fadt] PM1A_CNT live={:#x} SCI_EN={}",
                    live, live & 1);
            }
            if gpe0_blk != 0 && gpe0_len >= 2 {
                let half = (gpe0_len / 2) as u16;
                let port_sts = (gpe0_blk & 0xFFFF) as u16;
                let port_en  = port_sts + half;
                let sts = Port::<u8>::new(port_sts).read();
                let en  = Port::<u8>::new(port_en).read();
                let (r, g, b) = if sts != 0 || en != 0 { (255, 200, 100) } else { (200, 200, 200) };
                cprintln!(r, g, b,
                    "[probe.fadt] GPE0 sts={:#x} en={:#x}", sts, en);
            }
        }
    }
}

fn dump_pci_bus0() {
    let mut shown = 0u32;
    for dev in 0..32u8 {
        for func in 0..8u8 {
            let id = pci_read32(0, dev, func, 0);
            let vendor = (id & 0xFFFF) as u16;
            if vendor == 0xFFFF { if func == 0 { break; } else { continue; } }
            let device = (id >> 16) as u16;
            let class_rev = pci_read32(0, dev, func, 0x08);
            let class   = (class_rev >> 24) as u8;
            let sub     = (class_rev >> 16) as u8;
            let prog_if = (class_rev >> 8)  as u8;

            if class == 0x0C && sub == 0x03 {
                let bar0 = pci_read32(0, dev, func, 0x10);
                let kind = match prog_if {
                    0x00 => "UHCI", 0x10 => "OHCI", 0x20 => "EHCI",
                    0x30 => "xHCI", _ => "USB?",
                };
                cprintln!(200, 200, 255,
                    "[probe.pci] {:02x}:{:02x}.{} {:04x}:{:04x} {} bar0={:#x}",
                    0, dev, func, vendor, device, kind, bar0);
                shown += 1;
            }
            if class == 0x06 && sub == 0x01 {
                cprintln!(200, 200, 255,
                    "[probe.pci] {:02x}:{:02x}.{} {:04x}:{:04x} LPC bridge",
                    0, dev, func, vendor, device);
                shown += 1;
            }
            if class == 0x0C && sub == 0x05 {
                let pmbase = pci_read32(0, dev, func, 0x90);
                cprintln!(200, 200, 255,
                    "[probe.pci] {:02x}:{:02x}.{} {:04x}:{:04x} SMBus PMBASE={:#x}",
                    0, dev, func, vendor, device, pmbase);
                shown += 1;
            }

            if func == 0 && (pci_read8(0, dev, func, 0x0E) & 0x80) == 0 { break; }
        }
    }
    if shown == 0 {
        cprintln!(200, 200, 200, "[probe.pci] no USB/LPC/SMBus on bus 0");
    }
}
