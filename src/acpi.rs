////////////////////////////////////////////////////////////////////////
//                          ACPI parser                               //
//    read RSDP/RSDT/XSDT to locate MADT, enumerate LAPIC/IOAPIC      //
////////////////////////////////////////////////////////////////////////

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::grub;

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

#[repr(C, packed)]
struct MadtHeader {
    sdt:            SdtHeader,
    lapic_addr:     u32,
    flags:          u32,
}

// MADT entry types
const MADT_LAPIC:      u8 = 0;
const MADT_IOAPIC:     u8 = 1;
const MADT_ISO:        u8 = 2;  // Interrupt Source Override
const MADT_NMI_SRC:    u8 = 3;
const MADT_LAPIC_NMI:  u8 = 4;
const MADT_LAPIC_ADDR: u8 = 5;
const MADT_X2APIC:     u8 = 9;

const LAPIC_FLAG_ENABLED:        u32 = 1;
const LAPIC_FLAG_ONLINE_CAPABLE: u32 = 2;

#[derive(Debug, Clone, Copy)]
pub struct CpuInfo {
    pub acpi_uid:  u8,
    pub lapic_id:  u32,
    pub enabled:   bool,
    pub is_x2apic: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct IoApicInfo {
    pub id:       u8,
    pub addr:     u64,
    pub gsi_base: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct IsoInfo {
    pub bus:   u8,
    pub irq:   u8,     // legacy IRQ source
    pub gsi:   u32,    // global system interrupt
    pub flags: u16,    // polarity/trigger
}

pub struct AcpiTopology {
    pub lapic_phys:     u64,
    pub cpus:           Vec<CpuInfo>,
    pub ioapics:        Vec<IoApicInfo>,
    pub overrides:      Vec<IsoInfo>,
}

impl AcpiTopology {
    pub const fn empty() -> Self {
        Self {
            lapic_phys: 0xFEE00000,
            cpus:       Vec::new(),
            ioapics:    Vec::new(),
            overrides:  Vec::new(),
        }
    }

    /// Map legacy ISA irq to GSI (applies Interrupt Source Override if any)
    pub fn irq_to_gsi(&self, irq: u8) -> (u32, u16) {
        for iso in &self.overrides {
            if iso.bus == 0 && iso.irq == irq {
                return (iso.gsi, iso.flags);
            }
        }
        (irq as u32, 0)
    }

    /// Find the IOAPIC that owns the given GSI
    pub fn ioapic_for_gsi(&self, gsi: u32) -> Option<&IoApicInfo> {
        self.ioapics.iter().find(|ia| gsi >= ia.gsi_base && gsi < ia.gsi_base + 24)
    }
}

static TOPOLOGY: Mutex<Option<AcpiTopology>> = Mutex::new(None);
static INIT_DONE: AtomicBool = AtomicBool::new(false);

pub fn topology() -> spin::MutexGuard<'static, Option<AcpiTopology>> {
    TOPOLOGY.lock()
}

pub fn is_ready() -> bool {
    INIT_DONE.load(Ordering::Acquire)
}

// Scan physical memory region for RSDP signature on 16-byte boundaries
unsafe fn scan_for_rsdp(phys_start: u64, phys_end: u64) -> Option<u64> {
    let hhdm = grub::hhdm();
    let mut p = phys_start & !0xF;
    while p + 20 <= phys_end {
        let virt = (p + hhdm) as *const u8;
        let sig = core::slice::from_raw_parts(virt, 8);
        if sig == b"RSD PTR " {
            // verify checksum v1
            let mut sum: u8 = 0;
            for i in 0..20 {
                sum = sum.wrapping_add(*virt.add(i));
            }
            if sum == 0 {
                return Some(p);
            }
        }
        p += 16;
    }
    None
}

unsafe fn find_rsdp() -> Option<u64> {
    let hhdm = grub::hhdm();
    // EBDA pointer is at 0x40E (two bytes, shifted left by 4)
    let ebda_ptr_virt = (0x40E + hhdm) as *const u16;
    let ebda_seg = core::ptr::read_unaligned(ebda_ptr_virt) as u64;
    if ebda_seg != 0 {
        let ebda = ebda_seg << 4;
        if let Some(addr) = scan_for_rsdp(ebda, ebda + 1024) {
            return Some(addr);
        }
    }
    scan_for_rsdp(0xE0000, 0x100000)
}

unsafe fn checksum(ptr: *const u8, len: usize) -> bool {
    let mut sum: u8 = 0;
    for i in 0..len {
        sum = sum.wrapping_add(*ptr.add(i));
    }
    sum == 0
}

pub fn init() -> Result<(), &'static str> {
    let hhdm = grub::hhdm();
    let rsdp_phys = unsafe { find_rsdp() }.ok_or("RSDP not found")?;
    crate::serial_println!("[acpi] rsdp @ {:#x}", rsdp_phys);

    let rsdp10 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp10) };
    let revision = rsdp10.revision;
    crate::serial_println!("[acpi] rsdp revision={}", revision);

    // Prefer XSDT (ACPI 2.0+) if available
    let (sdt_phys, is_xsdt) = if revision >= 2 {
        let rsdp20 = unsafe { &*((rsdp_phys + hhdm) as *const Rsdp20) };
        let len = unsafe { core::ptr::addr_of!(rsdp20.length).read_unaligned() };
        if len >= 36 {
            unsafe {
                if !checksum((rsdp_phys + hhdm) as *const u8, len as usize) {
                    return Err("RSDP v2 bad checksum");
                }
            }
            let xsdt = unsafe { core::ptr::addr_of!(rsdp20.xsdt_addr).read_unaligned() };
            (xsdt, true)
        } else {
            (rsdp10.rsdt_addr as u64, false)
        }
    } else {
        (rsdp10.rsdt_addr as u64, false)
    };

    crate::serial_println!("[acpi] {} @ {:#x}", if is_xsdt { "xsdt" } else { "rsdt" }, sdt_phys);

    let sdt_hdr = unsafe { &*((sdt_phys + hhdm) as *const SdtHeader) };
    let total_len = unsafe { core::ptr::addr_of!(sdt_hdr.length).read_unaligned() } as usize;
    if total_len < 36 {
        return Err("SDT too short");
    }
    unsafe {
        if !checksum((sdt_phys + hhdm) as *const u8, total_len) {
            return Err("SDT bad checksum");
        }
    }

    let entry_size = if is_xsdt { 8usize } else { 4usize };
    let entry_count = (total_len - 36) / entry_size;
    let entries_base = sdt_phys + hhdm + 36;

    let mut madt_phys: u64 = 0;

    for i in 0..entry_count {
        let addr = entries_base + (i * entry_size) as u64;
        let ent_phys = if is_xsdt {
            unsafe { (addr as *const u64).read_unaligned() }
        } else {
            unsafe { (addr as *const u32).read_unaligned() as u64 }
        };
        let hdr = unsafe { &*((ent_phys + hhdm) as *const SdtHeader) };
        let sig = hdr.signature;
        if &sig == b"APIC" {
            madt_phys = ent_phys;
            break;
        }
    }

    if madt_phys == 0 {
        return Err("MADT not found");
    }

    crate::serial_println!("[acpi] madt @ {:#x}", madt_phys);

    let mut topo = AcpiTopology::empty();
    unsafe { parse_madt(madt_phys, &mut topo)?; }

    crate::serial_println!(
        "[acpi] cpus={} ioapics={} overrides={} lapic_phys={:#x}",
        topo.cpus.len(), topo.ioapics.len(), topo.overrides.len(), topo.lapic_phys,
    );
    for cpu in &topo.cpus {
        crate::serial_println!(
            "[acpi]   cpu uid={} lapic_id={} enabled={} x2apic={}",
            cpu.acpi_uid, cpu.lapic_id, cpu.enabled, cpu.is_x2apic
        );
    }
    for ia in &topo.ioapics {
        crate::serial_println!(
            "[acpi]   ioapic id={} addr={:#x} gsi_base={}",
            ia.id, ia.addr, ia.gsi_base
        );
    }
    for iso in &topo.overrides {
        crate::serial_println!(
            "[acpi]   iso bus={} irq={} gsi={} flags={:#x}",
            iso.bus, iso.irq, iso.gsi, iso.flags
        );
    }

    *TOPOLOGY.lock() = Some(topo);
    INIT_DONE.store(true, Ordering::Release);
    Ok(())
}

unsafe fn parse_madt(madt_phys: u64, topo: &mut AcpiTopology) -> Result<(), &'static str> {
    let hhdm = grub::hhdm();
    let mh = &*((madt_phys + hhdm) as *const MadtHeader);
    let total = core::ptr::addr_of!(mh.sdt.length).read_unaligned() as usize;
    if total < core::mem::size_of::<MadtHeader>() {
        return Err("MADT too short");
    }
    if !checksum((madt_phys + hhdm) as *const u8, total) {
        return Err("MADT bad checksum");
    }

    topo.lapic_phys = core::ptr::addr_of!(mh.lapic_addr).read_unaligned() as u64;

    let entries_start = madt_phys + hhdm + core::mem::size_of::<MadtHeader>() as u64;
    let entries_end   = madt_phys + hhdm + total as u64;
    let mut p = entries_start;

    while p + 2 <= entries_end {
        let etype = *(p as *const u8);
        let elen  = *(p.wrapping_add(1) as *const u8) as u64;
        if elen < 2 || p + elen > entries_end { break; }

        match etype {
            MADT_LAPIC => {
                let acpi_uid = *(p.wrapping_add(2) as *const u8);
                let lapic_id = *(p.wrapping_add(3) as *const u8) as u32;
                let flags    = (p.wrapping_add(4) as *const u32).read_unaligned();
                let enabled  = (flags & LAPIC_FLAG_ENABLED) != 0
                            || (flags & LAPIC_FLAG_ONLINE_CAPABLE) != 0;
                topo.cpus.push(CpuInfo {
                    acpi_uid, lapic_id, enabled, is_x2apic: false,
                });
            }
            MADT_IOAPIC => {
                let id       = *(p.wrapping_add(2) as *const u8);
                let addr     = (p.wrapping_add(4) as *const u32).read_unaligned() as u64;
                let gsi_base = (p.wrapping_add(8) as *const u32).read_unaligned();
                topo.ioapics.push(IoApicInfo { id, addr, gsi_base });
            }
            MADT_ISO => {
                let bus   = *(p.wrapping_add(2) as *const u8);
                let irq   = *(p.wrapping_add(3) as *const u8);
                let gsi   = (p.wrapping_add(4) as *const u32).read_unaligned();
                let flags = (p.wrapping_add(8) as *const u16).read_unaligned();
                topo.overrides.push(IsoInfo { bus, irq, gsi, flags });
            }
            MADT_LAPIC_ADDR => {
                // 64-bit override of LAPIC address
                let addr = (p.wrapping_add(4) as *const u64).read_unaligned();
                topo.lapic_phys = addr;
            }
            MADT_X2APIC => {
                let x2id     = (p.wrapping_add(4) as *const u32).read_unaligned();
                let flags    = (p.wrapping_add(8) as *const u32).read_unaligned();
                let acpi_uid = (p.wrapping_add(12) as *const u32).read_unaligned() as u8;
                let enabled  = (flags & LAPIC_FLAG_ENABLED) != 0
                            || (flags & LAPIC_FLAG_ONLINE_CAPABLE) != 0;
                topo.cpus.push(CpuInfo {
                    acpi_uid, lapic_id: x2id, enabled, is_x2apic: true,
                });
            }
            _ => {}
        }
        p = p.wrapping_add(elen);
    }
    Ok(())
}
