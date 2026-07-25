// Realtek RTL8111/8118/8168 gigabit driver (the r8169 family)
//
// Primary target: RTL8118AS on the Gigabyte B550M K (PCI 10ec:8168,
// hw_ver in the RTL8168H range). The whole bring-up sequence is a port of
// Linux r8169_main.c (rtl_hw_start_8168h_1 and friends); register names
// and magic values reference that file.
//
// The G/H generations (hw_ver >= 0x500) are NOT just faster 8168Bs:
//   - the internal PHY is only reachable through GPHY OCP (0xB8); the
//     classic PHYAR (0x60) interface is dead silicon on these chips
//   - RX is gated by RXDV_GATED_EN in MISC (0xF0) out of reset; until it
//     is cleared the MAC processes descriptors but never delivers a frame
//   - the packet FIFO / flow control / filter live behind the ERI window
//     (ERIDR 0x70 / ERIAR 0x74, byte-enable mask in bits 15:12)
//   - assorted MCU tweaks go through MAC OCP (OCPDR 0xB0)
//
// Interrupts are not used: netd polls recv() at 250 Hz like every other
// NIC driver here.

use crate::drivers::bus::pci::PciDevice;
use crate::net::NetworkDriver;
use alloc::boxed::Box;

const TX_DESC_COUNT: usize = 16;
const RX_DESC_COUNT: usize = 32;
const BUF_SIZE: usize = 1536;

// ------------------------------------------------------------- registers

const IDR0:      usize = 0x00; // MAC address, 6 bytes
const MAR0:      usize = 0x08; // multicast filter, 8 bytes
const TNPDS_LO:  usize = 0x20; // TX normal-priority descriptor base
const TNPDS_HI:  usize = 0x24;
const CR:        usize = 0x37; // command: RST/TE/RE
const TPPOLL:    usize = 0x38; // TX poll doorbell
const IMR:       usize = 0x3C;
const ISR:       usize = 0x3E;
const TXCFG:     usize = 0x40;
const RXCFG:     usize = 0x44;
const CFG9346:   usize = 0x50; // config-register lock
const CONFIG3:   usize = 0x54;
const CONFIG5:   usize = 0x56;
const PHYAR:     usize = 0x60; // classic MDIO, pre-8168G only
const PHYSTATUS: usize = 0x6C;
const ERIDR:     usize = 0x70; // ERI data
const ERIAR:     usize = 0x74; // ERI address/command
const EPHYAR:    usize = 0x80; // PCIe ePHY access
const OCPDR:     usize = 0xB0; // MAC OCP data/command
const GPHY_OCP:  usize = 0xB8; // PHY OCP (8168G+)
const DLLPR:     usize = 0xD0;
const RXMAXSIZE: usize = 0xDA;
const CPLUSCMD:  usize = 0xE0;
const RDSAR_LO:  usize = 0xE4; // RX descriptor base
const RDSAR_HI:  usize = 0xE8;
const MTPS:      usize = 0xEC; // max TX packet size, 128 B units
const MISC:      usize = 0xF0;
const MISC1:     usize = 0xF2;

const CR_RST: u8 = 1 << 4;
const CR_RE:  u8 = 1 << 3;
const CR_TE:  u8 = 1 << 2;

// TxConfig: IFG = 3 (bits 25:24), MaxDMA = 7 = unlimited (bits 10:8).
// AUTO_FIFO (bit 7) is required from 8111E-VL on; without it the G/H FIFO
// sizing programmed over ERI is ignored
const TXCFG_BASE:      u32 = 0x0300_0700;
const TXCFG_AUTO_FIFO: u32 = 1 << 7;

// RxConfig for 8168G..8168H per rtl_init_rxcfg (VER_40..52)
const RXCFG_RX128_INT_EN: u32 = 1 << 15;
const RXCFG_RX_MULTI_EN:  u32 = 1 << 14;
const RXCFG_RX_EARLY_OFF: u32 = 1 << 11;
const RXCFG_RX_DMA_BURST: u32 = 7 << 8;
// filter: accept broadcast / multicast / phys-match / all-phys
const RXCFG_FILTER_ALL:   u32 = 0xF;

const MISC_RXDV_GATED_EN: u32 = 1 << 19;
const DLLPR_PFM_EN:       u8 = 1 << 6;
const DLLPR_TX_10M_PS_EN: u8 = 1 << 7;
const MISC1_PFM_D3COLD:   u8 = 1 << 6;
const CONFIG3_RDY_TO_L23: u8 = 1 << 1;

// ERI window. Byte-enable mask sits in bits 15:12, type (EXGMAC = 0) in
// bits 17:16, register address in bits 11:0. Bit 31 doubles as the write
// command flag and the read ready flag
const ERIAR_FLAG:      u32 = 1 << 31;
const ERI_MASK_0001:   u32 = 0x1 << 12;
const ERI_MASK_0011:   u32 = 0x3 << 12;
const ERI_MASK_1111:   u32 = 0xF << 12;

const OCPAR_FLAG: u32 = 1 << 31;
const OCP_STD_PHY_BASE: u32 = 0xA400;

const DESC_OWN: u32 = 1 << 31;
const DESC_EOR: u32 = 1 << 30;
const DESC_FS:  u32 = 1 << 29;
const DESC_LS:  u32 = 1 << 28;

#[repr(C, align(16))]
#[derive(Copy, Clone)]
struct Desc {
    flags: u32,
    vlan: u32,
    buf_lo: u32,
    buf_hi: u32,
}

impl Desc {
    const fn zero() -> Self {
        Self { flags: 0, vlan: 0, buf_lo: 0, buf_hi: 0 }
    }
}

/// Publish a descriptor to the chip in the correct order: buffer address +
/// VLAN first, then a full memory fence, then flags (which carries the OWN
/// bit). A plain struct assignment lets the compiler reorder the field
/// stores; if flags lands before buf_lo/buf_hi the NIC sees OWN=1 with a
/// stale buffer pointer and silently drops the frame
#[inline]
unsafe fn write_desc(d: *mut Desc, flags: u32, vlan: u32, buf_phys: u64) {
    use core::ptr::{addr_of_mut, write_volatile};
    write_volatile(addr_of_mut!((*d).buf_lo), buf_phys as u32);
    write_volatile(addr_of_mut!((*d).buf_hi), (buf_phys >> 32) as u32);
    write_volatile(addr_of_mut!((*d).vlan), vlan);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    write_volatile(addr_of_mut!((*d).flags), flags);
}

#[repr(align(256))]
struct TxRing([Desc; TX_DESC_COUNT]);
#[repr(align(256))]
struct RxRing([Desc; RX_DESC_COUNT]);

#[repr(align(16))]
struct TxBufs([[u8; BUF_SIZE]; TX_DESC_COUNT]);
#[repr(align(16))]
struct RxBufs([[u8; BUF_SIZE]; RX_DESC_COUNT]);

pub struct Rtl8168 {
    mmio_base: u64,
    pub mac: [u8; 6],
    hwver: u32,
    tx_ring: Box<TxRing>,
    rx_ring: Box<RxRing>,
    tx_bufs: Box<TxBufs>,
    rx_bufs: Box<RxBufs>,
    tx_idx: usize,
    rx_idx: usize,
    /// How many sends we have logged so far. Bring-up diagnostic only
    send_log_count: u32,
}

impl Rtl8168 {
    pub fn new(pci: &PciDevice) -> Option<Self> {
        pci.enable_bus_mastering();

        // Register MMIO lives in BAR2 on real 8168 hardware; BAR0 is I/O,
        // BAR4 is the MSI-X table
        let mem_phys = pci.mem_bar(2)
            .or_else(|| pci.mem_bar(1))
            .or_else(|| pci.mem_bar(0));
        crate::serial_println!("[rtl8168] mem_bar2={:?} mem_bar1={:?} mem_bar0={:?} picked={:?}",
            pci.mem_bar(2), pci.mem_bar(1), pci.mem_bar(0), mem_phys);
        let mem_phys = match mem_phys {
            Some(p) if p != 0 => p,
            _ => {
                crate::serial_println!("[rtl8168] abort: no usable memory BAR");
                return None;
            }
        };
        crate::net::map_mmio(mem_phys, 0x1000);

        let hhdm = crate::net::HHDM_OFFSET.load(core::sync::atomic::Ordering::Relaxed);
        let mmio_base = mem_phys + hhdm;

        let mut drv = Self {
            mmio_base,
            mac: [0; 6],
            hwver: 0,
            tx_ring: Box::new(TxRing([Desc::zero(); TX_DESC_COUNT])),
            rx_ring: Box::new(RxRing([Desc::zero(); RX_DESC_COUNT])),
            tx_bufs: Box::new(TxBufs([[0u8; BUF_SIZE]; TX_DESC_COUNT])),
            rx_bufs: Box::new(RxBufs([[0u8; BUF_SIZE]; RX_DESC_COUNT])),
            tx_idx: 0,
            rx_idx: 0,
            send_log_count: 0,
        };
        drv.init();
        Some(drv)
    }

    fn read8(&self, off: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.mmio_base + off as u64) as *const u8) }
    }
    fn read16(&self, off: usize) -> u16 {
        unsafe { core::ptr::read_volatile((self.mmio_base + off as u64) as *const u16) }
    }
    fn read32(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio_base + off as u64) as *const u32) }
    }
    fn write8(&self, off: usize, v: u8) {
        unsafe { core::ptr::write_volatile((self.mmio_base + off as u64) as *mut u8, v) }
    }
    fn write16(&self, off: usize, v: u16) {
        unsafe { core::ptr::write_volatile((self.mmio_base + off as u64) as *mut u16, v) }
    }
    fn write32(&self, off: usize, v: u32) {
        unsafe { core::ptr::write_volatile((self.mmio_base + off as u64) as *mut u32, v) }
    }

    /// True for RTL8168G and later (hw_ver 0x500+): OCP-based PHY access,
    /// ERI FIFO config, RXDV gate
    fn is_g_plus(&self) -> bool {
        self.hwver >= 0x500
    }

    // -------------------------------------------------- ERI (EXGMAC window)

    fn eri_write(&self, addr: u16, mask: u32, val: u32) {
        self.write32(ERIDR, val);
        self.write32(ERIAR, ERIAR_FLAG | mask | (addr as u32 & 0xFFF));
        for _ in 0..1_000_000 {
            if self.read32(ERIAR) & ERIAR_FLAG == 0 { return; }
            core::hint::spin_loop();
        }
    }

    fn eri_read(&self, addr: u16) -> u32 {
        // reads always return the full dword; the chip sets bit 31 when
        // ERIDR holds valid data
        self.write32(ERIAR, ERI_MASK_1111 | (addr as u32 & 0xFFF));
        for _ in 0..1_000_000 {
            if self.read32(ERIAR) & ERIAR_FLAG != 0 {
                return self.read32(ERIDR);
            }
            core::hint::spin_loop();
        }
        0xFFFF_FFFF
    }

    /// Read-modify-write: val = (val & !clear) | set, written back with the
    /// given byte-enable mask
    fn eri_w0w1(&self, addr: u16, mask: u32, clear: u32, set: u32) {
        let v = self.eri_read(addr);
        self.eri_write(addr, mask, (v & !clear) | set);
    }

    /// Toggle bit 0 of ERI 0xDC: flushes the MCU packet filter (stale
    /// BIOS/PXE filter state otherwise eats ARP/DHCP broadcasts)
    fn eri_reset_packet_filter(&self) {
        self.eri_w0w1(0xDC, ERI_MASK_0001, 1, 0);
        self.eri_w0w1(0xDC, ERI_MASK_0001, 0, 1);
    }

    // ----------------------------------------------------------- MAC OCP

    // MAC-side microcontroller registers. Encoding per r8169: bit 31 =
    // write flag, bits 30:16 = (even) register << 15, bits 15:0 = data

    fn mac_ocp_write(&self, reg: u16, val: u16) {
        debug_assert!(reg & 1 == 0);
        self.write32(OCPDR, OCPAR_FLAG | ((reg as u32) << 15) | val as u32);
    }

    fn mac_ocp_read(&self, reg: u16) -> u16 {
        self.write32(OCPDR, (reg as u32) << 15);
        (self.read32(OCPDR) & 0xFFFF) as u16
    }

    fn mac_ocp_modify(&self, reg: u16, mask: u16, set: u16) {
        let v = self.mac_ocp_read(reg);
        self.mac_ocp_write(reg, (v & !mask) | set);
    }

    // -------------------------------------------------------- PCIe ePHY

    fn ephy_write(&self, reg: u8, val: u16) {
        self.write32(EPHYAR, ERIAR_FLAG | ((reg as u32 & 0x3F) << 16) | val as u32);
        for _ in 0..100_000 {
            if self.read32(EPHYAR) & ERIAR_FLAG == 0 { return; }
            core::hint::spin_loop();
        }
    }

    fn ephy_read(&self, reg: u8) -> u16 {
        self.write32(EPHYAR, (reg as u32 & 0x3F) << 16);
        for _ in 0..100_000 {
            let v = self.read32(EPHYAR);
            if v & ERIAR_FLAG != 0 { return (v & 0xFFFF) as u16; }
            core::hint::spin_loop();
        }
        0xFFFF
    }

    fn ephy_init(&self, table: &[(u8, u16, u16)]) {
        for &(reg, clear, set) in table {
            let v = self.ephy_read(reg);
            self.ephy_write(reg, (v & !clear) | set);
        }
    }

    // ------------------------------------------------------------- MDIO

    // Classic MDIO through PHYAR: pre-8168G silicon only
    fn phyar_write(&self, reg: u8, val: u16) {
        self.write32(PHYAR, (1u32 << 31) | ((reg as u32 & 0x1F) << 16) | val as u32);
        for _ in 0..1_000_000 {
            if self.read32(PHYAR) & (1 << 31) == 0 { return; }
            core::hint::spin_loop();
        }
    }

    // 8168G+ PHY access through the GPHY OCP window. MII register N of the
    // standard page lives at OCP address 0xA400 + 2*N
    fn phy_ocp_write(&self, ocp_addr: u32, val: u16) {
        self.write32(GPHY_OCP, OCPAR_FLAG | ((ocp_addr & 0xFFFE) << 15) | val as u32);
        for _ in 0..1_000_000 {
            if self.read32(GPHY_OCP) & OCPAR_FLAG == 0 { return; }
            core::hint::spin_loop();
        }
    }

    fn phy_ocp_read(&self, ocp_addr: u32) -> u16 {
        self.write32(GPHY_OCP, (ocp_addr & 0xFFFE) << 15);
        for _ in 0..1_000_000 {
            let v = self.read32(GPHY_OCP);
            if v & OCPAR_FLAG != 0 { return (v & 0xFFFF) as u16; }
            core::hint::spin_loop();
        }
        0xFFFF
    }

    /// MII write that picks the right transport for the chip generation
    fn mdio_write(&self, reg: u8, val: u16) {
        if self.is_g_plus() {
            self.phy_ocp_write(OCP_STD_PHY_BASE + 2 * reg as u32, val);
        } else {
            self.phyar_write(reg, val);
        }
    }

    // -------------------------------------------------------- chip init

    fn init(&mut self) {
        // 1) Mask + ack interrupts before touching anything else
        self.write16(IMR, 0x0000);
        self.write16(ISR, 0xFFFF);

        // 2) Soft reset (self-clearing)
        self.write8(CR, CR_RST);
        for _ in 0..1_000_000 {
            if self.read8(CR) & CR_RST == 0 { break; }
            core::hint::spin_loop();
        }

        // 3) MAC address (latched from EFUSE/EEPROM on reset)
        for i in 0..6 {
            self.mac[i] = self.read8(IDR0 + i);
        }

        // 4) hw_ver from the read-only TxConfig bits, needed to pick the
        //    bring-up path. Thresholds after (txc & 0x7CF00000) >> 20:
        //      8168B/C/D: 0x100..0x27F   8168E: 0x280..0x47F
        //      8168F:     0x480..0x4FF   8168G: 0x500..0x53F
        //      8168H/EP + RTL8118AS: 0x540..0x7CF
        self.hwver = (self.read32(TXCFG) & 0x7CF0_0000) >> 20;

        // 5) Base RxConfig (no filter bits yet) per rtl_init_rxcfg
        let rxcfg_base = if self.is_g_plus() {
            RXCFG_RX128_INT_EN | RXCFG_RX_MULTI_EN | RXCFG_RX_DMA_BURST | RXCFG_RX_EARLY_OFF
        } else {
            RXCFG_RX128_INT_EN | RXCFG_RX_MULTI_EN | RXCFG_RX_DMA_BURST
        };
        self.write32(RXCFG, rxcfg_base);

        // 6) Unlock Config0..5, clear the WoL latches BIOS/PXE love to
        //    leave behind (they park the PHY in low-power = eternal
        //    link-down), and keep the config window open for the per-chip
        //    sequence below
        self.write8(CFG9346, 0xC0);
        let cfg3 = self.read8(CONFIG3);
        self.write8(CONFIG3, cfg3 & !0x61);
        let cfg5 = self.read8(CONFIG5);
        self.write8(CONFIG5, cfg5 & !0x1F);

        // 7) C+ command: keep revision bits, add RxChkSum|RxVlan, clear
        //    PktCntrDisable
        let cur_cp = (self.read32(CPLUSCMD) & 0xFFFF) as u16;
        self.write16(CPLUSCMD, (cur_cp | 0x0060) & !0x0080);

        // 8) Generation-specific MAC/FIFO/MCU setup
        if self.hwver >= 0x540 {
            self.hw_start_8168h();
        } else if self.hwver >= 0x500 {
            self.hw_start_8168g();
        }
        // pre-G chips need none of this: FIFO defaults are sane and RX is
        // not gated

        // 9) Descriptor rings
        let tx_phys = crate::net::virt_to_phys(self.tx_ring.0.as_ptr() as u64);
        let rx_phys = crate::net::virt_to_phys(self.rx_ring.0.as_ptr() as u64);

        for i in 0..TX_DESC_COUNT {
            let buf_phys = crate::net::virt_to_phys(self.tx_bufs.0[i].as_ptr() as u64);
            let eor = if i == TX_DESC_COUNT - 1 { DESC_EOR } else { 0 };
            unsafe { write_desc(&mut self.tx_ring.0[i] as *mut Desc, eor, 0, buf_phys); }
        }
        for i in 0..RX_DESC_COUNT {
            let buf_phys = crate::net::virt_to_phys(self.rx_bufs.0[i].as_ptr() as u64);
            let eor = if i == RX_DESC_COUNT - 1 { DESC_EOR } else { 0 };
            unsafe {
                write_desc(
                    &mut self.rx_ring.0[i] as *mut Desc,
                    DESC_OWN | eor | (BUF_SIZE as u32),
                    0,
                    buf_phys,
                );
            }
        }

        self.write32(TNPDS_LO, tx_phys as u32);
        self.write32(TNPDS_HI, (tx_phys >> 32) as u32);
        self.write32(RDSAR_LO, rx_phys as u32);
        self.write32(RDSAR_HI, (rx_phys >> 32) as u32);

        // 10) RX size limit and TX max packet size (128 B units)
        self.write16(RXMAXSIZE, BUF_SIZE as u16);
        self.write8(MTPS, 0x3F);

        // 11) Multicast filter wide open so DHCP/ARP broadcasts land
        self.write32(MAR0, 0xFFFF_FFFF);
        self.write32(MAR0 + 4, 0xFFFF_FFFF);

        // 12) Re-lock config space
        self.write8(CFG9346, 0x00);

        // 13) Everything above must be visible before the engines start
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 14) Enable TX/RX, then TxConfig + RxConfig filter bits (this
        //     ordering matches r8169: several revisions ignore TxConfig/
        //     RxConfig writes while the engines are disabled)
        self.write8(CR, CR_TE | CR_RE);
        let txcfg = if self.hwver >= 0x2C0 {
            // 8111E-VL and everything after wants AUTO_FIFO
            TXCFG_BASE | TXCFG_AUTO_FIFO
        } else {
            TXCFG_BASE
        };
        self.write32(TXCFG, txcfg);
        self.write32(RXCFG, rxcfg_base | RXCFG_FILTER_ALL);

        // 15) Ack whatever latched during bring-up
        self.write16(ISR, 0xFFFF);

        let family = if self.hwver >= 0x540 {
            "RTL8168H/8118AS (full r8169 h_1 sequence, no firmware blob)"
        } else if self.hwver >= 0x500 {
            "RTL8168G/GU (full r8169 g sequence, no firmware blob)"
        } else if self.hwver >= 0x480 {
            "RTL8168F"
        } else if self.hwver >= 0x280 {
            "RTL8168E/EV"
        } else if self.hwver >= 0x100 {
            "RTL8168B/C/D"
        } else {
            "RTL8169 / pre-8168B"
        };
        crate::serial_println!(
            "[rtl8168] hw_ver={:#x} {} CR={:#04x} TxCfg={:#010x} RxCfg={:#010x}",
            self.hwver, family, self.read8(CR), self.read32(TXCFG), self.read32(RXCFG)
        );

        // 16) Wake the PHY and restart auto-negotiation (through the right
        //     transport for this generation)
        self.phy_init();
    }

    /// rtl_hw_start_8168g: FIFO sizing, packet-filter reset, RXDV un-gate,
    /// power-management bits off, L2/L3 workaround
    fn hw_start_8168g(&self) {
        // FIFO sizes (static << 16 | dynamic), then pause thresholds
        self.eri_write(0xC8, ERI_MASK_1111, (0x08 << 16) | 0x02);
        self.eri_write(0xE8, ERI_MASK_1111, (0x10 << 16) | 0x06);
        self.eri_write(0xCC, ERI_MASK_0001, 0x38);
        self.eri_write(0xD0, ERI_MASK_0001, 0x48);

        self.eri_reset_packet_filter();
        self.eri_write(0x2F8, ERI_MASK_0011, 0x1D8F);

        self.disable_rxdv_gate();

        self.eri_write(0xC0, ERI_MASK_0011, 0x0000);
        self.eri_write(0xB8, ERI_MASK_0011, 0x0000);

        self.eri_w0w1(0x2FC, ERI_MASK_0001, 0x06, 0x01);
        self.eri_w0w1(0x1B0, ERI_MASK_1111, 1 << 12, 0);

        self.pcie_l2l3_disable();
    }

    /// rtl_hw_start_8168h_1: everything 8168G gets, plus the H-specific
    /// ePHY fixes and MCU (MAC OCP) tuning. This is the path the B550M K's
    /// RTL8118AS takes
    fn hw_start_8168h(&self) {
        const EPHY_8168H_1: [(u8, u16, u16); 6] = [
            (0x1E, 0x0800, 0x0001),
            (0x1D, 0x0000, 0x0800),
            (0x05, 0xFFFF, 0x2089),
            (0x06, 0xFFFF, 0x5881),
            (0x04, 0xFFFF, 0x854A),
            (0x01, 0xFFFF, 0x068B),
        ];
        self.ephy_init(&EPHY_8168H_1);

        self.eri_write(0xC8, ERI_MASK_1111, (0x08 << 16) | 0x02);
        self.eri_write(0xE8, ERI_MASK_1111, (0x10 << 16) | 0x06);
        self.eri_write(0xCC, ERI_MASK_0001, 0x38);
        self.eri_write(0xD0, ERI_MASK_0001, 0x48);

        self.eri_reset_packet_filter();

        self.eri_w0w1(0xDC, ERI_MASK_0001, 0, 0x001C);
        self.eri_write(0x5F0, ERI_MASK_0011, 0x4F87);

        self.disable_rxdv_gate();

        self.eri_write(0xC0, ERI_MASK_0011, 0x0000);
        self.eri_write(0xB8, ERI_MASK_0011, 0x0000);

        // power-management features off while we have no PM
        let d = self.read8(DLLPR);
        self.write8(DLLPR, d & !(DLLPR_PFM_EN | DLLPR_TX_10M_PS_EN));
        let m1 = self.read8(MISC1);
        self.write8(MISC1, m1 & !MISC1_PFM_D3COLD);

        self.eri_w0w1(0x1B0, ERI_MASK_1111, 1 << 12, 0);

        self.pcie_l2l3_disable();

        // MCU tuning per rtl_hw_start_8168h_1 (the rg_saw_cnt PHY
        // calibration is skipped: it only trims a green-ethernet timer)
        self.mac_ocp_modify(0xE056, 0x00F0, 0x0000);
        self.mac_ocp_modify(0xE052, 0x6000, 0x8008);
        self.mac_ocp_modify(0xE0D6, 0x01FF, 0x017F);
        self.mac_ocp_modify(0xD420, 0x0FFF, 0x047F);

        self.mac_ocp_write(0xE63E, 0x0001);
        self.mac_ocp_write(0xE63E, 0x0000);
        self.mac_ocp_write(0xC094, 0x0000);
        self.mac_ocp_write(0xC09E, 0x0000);
    }

    /// Clear RXDV_GATED_EN (MISC bit 19). 8168G+ comes out of reset with
    /// RX gated: descriptors are consumed but no frame is ever delivered
    /// until this bit goes away. THE classic "tx works, rx: 0" cause
    fn disable_rxdv_gate(&self) {
        let m = self.read32(MISC);
        self.write32(MISC, m & !MISC_RXDV_GATED_EN);
    }

    /// Work around PCI resets during L2/L3 link states (r8169
    /// rtl_pcie_state_l2l3_disable)
    fn pcie_l2l3_disable(&self) {
        let c3 = self.read8(CONFIG3);
        self.write8(CONFIG3, c3 & !CONFIG3_RDY_TO_L23);
    }

    /// Advertise 10/100/1000 and restart auto-negotiation. Does not wait
    /// for convergence (takes seconds); netd polls link_up() and picks the
    /// link up whenever it lands
    fn phy_init(&self) {
        // ANAR: 10/100 half+full, 802.3 selector; GBCR: 1000T half+full;
        // BMCR: auto-neg enable + restart (also clears isolate/power-down)
        self.mdio_write(4, 0x01E1);
        self.mdio_write(9, 0x0300);
        self.mdio_write(0, 0x1200);

        let s = self.read8(PHYSTATUS);
        crate::serial_println!(
            "[rtl8168] PHYStatus={:#04x} link={} speed={} (mdio via {})",
            s,
            s & 0x02 != 0,
            if s & 0x10 != 0 { "1000M" } else if s & 0x08 != 0 { "100M" }
            else if s & 0x04 != 0 { "10M" } else { "?" },
            if self.is_g_plus() { "GPHY-OCP" } else { "PHYAR" },
        );
    }

    // Print register state to the console (for `net diag`)
    pub fn diag(&self) {
        let cr  = self.read8(CR);
        let txc = self.read32(TXCFG);
        let rxc = self.read32(RXCFG);
        let cp  = self.read32(CPLUSCMD) & 0xFFFF;
        let phy = self.read8(PHYSTATUS);
        let isr = self.read16(ISR);
        let misc = self.read32(MISC);
        crate::cprintln!(57, 197, 187, "[rtl8168 diag]");
        crate::cprintln!(230, 240, 240,
            "  hw_ver={:#x}  CR={:#04x}  ISR={:#06x}  MISC={:#010x} (rxdv_gate={})",
            self.hwver, cr, isr, misc,
            if misc & MISC_RXDV_GATED_EN != 0 { "ON (rx blocked!)" } else { "off" });
        crate::cprintln!(230, 240, 240,
            "  TxCfg={:#010x}  RxCfg={:#010x}  CPlusCmd={:#06x}", txc, rxc, cp);
        crate::cprintln!(230, 240, 240,
            "  PHYStatus={:#04x}  link={}  speed={}",
            phy,
            if phy & 0x02 != 0 { "up" } else { "down" },
            if phy & 0x10 != 0 { "1000M" }
            else if phy & 0x08 != 0 { "100M" }
            else if phy & 0x04 != 0 { "10M" }
            else { "?" }
        );
        let tx_phys = crate::net::virt_to_phys(self.tx_ring.0.as_ptr() as u64);
        let rx_phys = crate::net::virt_to_phys(self.rx_ring.0.as_ptr() as u64);
        crate::cprintln!(230, 240, 240,
            "  TX ring: want={:#010x}  chip={:#010x}", tx_phys as u32, self.read32(TNPDS_LO));
        crate::cprintln!(230, 240, 240,
            "  RX ring: want={:#010x}  chip={:#010x}", rx_phys as u32, self.read32(RDSAR_LO));
        let rx0_flags = unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(self.rx_ring.0[self.rx_idx].flags))
        };
        crate::cprintln!(230, 240, 240,
            "  RX desc[{}] flags={:#010x}  OWN={}",
            self.rx_idx, rx0_flags,
            if rx0_flags & DESC_OWN != 0 { "chip (empty)" } else { "driver (pkt!)" }
        );
    }

    pub fn driver_name() -> &'static str {
        "RTL8168 (Realtek Gigabit)"
    }
}

impl NetworkDriver for Rtl8168 {
    fn send(&mut self, data: &[u8]) -> bool {
        if data.len() > BUF_SIZE {
            return false;
        }
        let i = self.tx_idx;
        // Volatile read so we always pick up the chip's most recent
        // OWN-clear, not a cached value
        let cur_flags = unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(self.tx_ring.0[i].flags))
        };
        if cur_flags & DESC_OWN != 0 {
            return false;
        }

        let eor = if i == TX_DESC_COUNT - 1 { DESC_EOR } else { 0 };
        let buf_phys = crate::net::virt_to_phys(self.tx_bufs.0[i].as_ptr() as u64);
        self.tx_bufs.0[i][..data.len()].copy_from_slice(data);

        unsafe {
            write_desc(
                &mut self.tx_ring.0[i] as *mut Desc,
                DESC_OWN | DESC_FS | DESC_LS | eor | (data.len() as u32),
                0,
                buf_phys,
            );
        }

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.write8(TPPOLL, 0x40); // NPQ

        // Bring-up diagnostic for the first frames: if OWN never clears,
        // the chip is ignoring the ring (bad TNPDS/TxConfig)
        if self.send_log_count < 3 {
            for _ in 0..200_000 { core::hint::spin_loop(); }
            let post = unsafe {
                core::ptr::read_volatile(core::ptr::addr_of!(self.tx_ring.0[i].flags))
            };
            crate::serial_println!(
                "[rtl8168] tx#{} idx={} len={} desc_flags_post={:#010x} CR={:#04x} ISR={:#06x}",
                self.send_log_count, i, data.len(), post, self.read8(CR), self.read16(ISR)
            );
            self.send_log_count += 1;
        }

        self.tx_idx = (i + 1) % TX_DESC_COUNT;
        true
    }

    fn recv(&mut self, handler: &mut dyn FnMut(&[u8])) {
        loop {
            let i = self.rx_idx;
            let flags = unsafe {
                core::ptr::read_volatile(core::ptr::addr_of!(self.rx_ring.0[i].flags))
            };
            if flags & DESC_OWN != 0 {
                break;
            }
            let len = (flags & 0x3FFF) as usize;
            if len > 4 && len <= BUF_SIZE {
                handler(&self.rx_bufs.0[i][..len - 4]);
            }
            let buf_phys = crate::net::virt_to_phys(self.rx_bufs.0[i].as_ptr() as u64);
            let eor = if i == RX_DESC_COUNT - 1 { DESC_EOR } else { 0 };
            unsafe {
                write_desc(
                    &mut self.rx_ring.0[i] as *mut Desc,
                    DESC_OWN | eor | (BUF_SIZE as u32),
                    0,
                    buf_phys,
                );
            }
            self.rx_idx = (i + 1) % RX_DESC_COUNT;
        }
        self.write16(ISR, 0xFFFF);
    }

    fn has_packet(&self) -> bool {
        let flags = unsafe {
            core::ptr::read_volatile(
                core::ptr::addr_of!(self.rx_ring.0[self.rx_idx].flags),
            )
        };
        flags & DESC_OWN == 0
    }

    fn link_up(&self) -> bool {
        self.read8(PHYSTATUS) & 0x02 != 0
    }

    fn get_mac(&self) -> [u8; 6] {
        self.mac
    }

    fn diag(&self) {
        Rtl8168::diag(self);
    }
}
