use super::pci::PciDevice;
use super::NetworkDriver;
use alloc::boxed::Box;

const TX_DESC_COUNT: usize = 4;
const RX_DESC_COUNT: usize = 4;
const BUF_SIZE: usize = 1536;

// ERIAR (0xD0) / ERIDR (0xD4) - External Register Interface.
// Bit 31 = command flag: set=1 for write (chip clears when done),
// set=0 for read (chip sets when data is ready in ERIDR).
// Bits [27:26] = type (0 = EXGMAC). Bits [25:24] = byte-enable mask
// Bits [19:0] = ERI register address
const ERIAR: usize = 0xD0;
const ERIDR: usize = 0xD4;
const ERI_WRITE:  u32 = 1 << 31;
const ERI_MASK16: u32 = 0x03 << 24; // bytes 0,1

// TxConfig bit 29: enables Ethernet MAC clock on RTL8168G/H/EP/FP.
// Without it TX descriptors are processed (OWN cleared) but frames
// are silently dropped before leaving the PHY - the exact symptom of
// tx:N rx:0 on boards with these chip revisions
const TXCFG_ETHER_CLKEN: u32 = 1 << 29;

const DESC_OWN: u32 = 1 << 31;
const DESC_EOR: u32 = 1 << 30;
const DESC_FS: u32 = 1 << 29;
const DESC_LS: u32 = 1 << 28;

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
/// bit). This matters because a plain d = Desc { flags: OWN|..., .. }`
/// lets the compiler emit the field stores in any order it likes - if
/// flags lands before buf_lo/buf_hi, the NIC sees OWN=1 with stale or
/// zero buffer pointers and silently drops the frame. That is the failure
/// mode that produces "link: up" + "tx: 0" on real hardware
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

        // RTL8111/8168 register MMIO lives in BAR2 on real hardware; BAR0 is I/O,
        // BAR1 is unused, BAR4 is the 16KB MSI-X region. QEMU's rtl8139 uses BAR1
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
        super::map_mmio(mem_phys, 0x1000);

        let hhdm = super::HHDM_OFFSET.load(core::sync::atomic::Ordering::Relaxed);
        let mmio_base = mem_phys + hhdm;

        let mut drv = Self {
            mmio_base,
            mac: [0; 6],
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

    // ERI (External Register Interface)
    // Accesses the RTL8168's embedded EXGMAC controller registers
    // Must be called AFTER CR=TE|RE (chip clock must be running)

    fn eri_write(&self, addr: u16, byte_en: u32, val: u32) {
        self.write32(ERIDR, val);
        self.write32(ERIAR, ERI_WRITE | byte_en | (addr as u32));
        for _ in 0..1_000_000 {
            if self.read32(ERIAR) & ERI_WRITE == 0 { return; }
            core::hint::spin_loop();
        }
    }

    fn eri_read(&self, addr: u16, byte_en: u32) -> u32 {
        // For reads: bit 31 starts at 0; chip sets it when data is ready in ERIDR
        self.write32(ERIAR, byte_en | (addr as u32));
        for _ in 0..1_000_000 {
            if self.read32(ERIAR) & ERI_WRITE != 0 {
                return self.read32(ERIDR);
            }
            core::hint::spin_loop();
        }
        0xFFFF_FFFF
    }

    fn eri_set_bits16(&self, addr: u16, bits: u16) {
        let v = self.eri_read(addr, ERI_MASK16) as u16;
        self.eri_write(addr, ERI_MASK16, (v | bits) as u32);
    }

    fn eri_clr_bits16(&self, addr: u16, bits: u16) {
        let v = self.eri_read(addr, ERI_MASK16) as u16;
        self.eri_write(addr, ERI_MASK16, (v & !bits) as u32);
    }

    // Minimal ERI init for RTL8168G/H without a firmware blob
    // is safe to apply unconditionally and does not depend on firmware state
    fn init_g_h_eri(&self) {
        // Clear packet-buffer control registers to a known state
        self.eri_write(0xc0, ERI_MASK16, 0x0000);
        self.eri_write(0xb8, ERI_MASK16, 0x0000);
        // Enable descriptor-engine flow-control and DMA arbitration bits
        self.eri_set_bits16(0xd4, 0x1f00);
        self.eri_set_bits16(0xdc, 0x001f);
        self.eri_set_bits16(0xe8, 0x001f);
        // Reset the MCU packet filter: toggling bit 0 of 0xDC flushes
        // any stale filter state left by BIOS/PXE and lets ARP/DHCP frames
        // through the descriptor pipe.
        self.eri_clr_bits16(0xdc, 0x0001);
        self.eri_set_bits16(0xdc, 0x0001);
    }

    // Print RTL8168 register state to the console (for net diag)
    pub fn diag(&self) {
        let cr  = self.read8(0x37);
        let txc = self.read32(0x40);
        let rxc = self.read32(0x44);
        let cp  = self.read32(0xE0) & 0xFFFF;
        let phy = self.read8(0x6C);
        let hwver = (txc & 0x7CF0_0000) >> 20;
        let isr = unsafe {
            core::ptr::read_volatile((self.mmio_base + 0x3E) as *const u16)
        };
        crate::cprintln!(57, 197, 187, "[rtl8168 diag]");
        crate::cprintln!(230, 240, 240,
            "  hw_ver={:#x}  CR={:#04x}  ISR={:#06x}", hwver, cr, isr);
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
        let tx_phys = super::virt_to_phys(self.tx_ring.0.as_ptr() as u64);
        let rx_phys = super::virt_to_phys(self.rx_ring.0.as_ptr() as u64);
        let tnpds_lo = self.read32(0x20);
        let tnpds_hi = self.read32(0x24);
        let rdsar_lo = self.read32(0xE4);
        let rdsar_hi = self.read32(0xE8);
        crate::cprintln!(230, 240, 240,
            "  TX ring: want={:#010x}  chip={:#010x}",
            tx_phys as u32, tnpds_lo);
        crate::cprintln!(230, 240, 240,
            "  RX ring: want={:#010x}  chip={:#010x}",
            rx_phys as u32, rdsar_lo);
        // Mismatched addresses mean virt_to_phys returned wrong result
        if tnpds_hi != 0 || rdsar_hi != 0 {
            crate::cprintln!(255, 180, 50,
                "  WARNING: 64-bit DMA addr (hi TX={:#x} RX={:#x}) - verify IOMMU",
                tnpds_hi, rdsar_hi);
        }
        // Check first RX descriptor OWN bit
        let rx0_flags = unsafe {
            core::ptr::read_volatile(
                core::ptr::addr_of!(self.rx_ring.0[self.rx_idx].flags)
            )
        };
        crate::cprintln!(230, 240, 240,
            "  RX desc[{}] flags={:#010x}  OWN={}",
            self.rx_idx, rx0_flags,
            if rx0_flags & DESC_OWN != 0 { "chip (empty)" } else { "driver (pkt!)" }
        );
    }

    fn init(&mut self) {
        // 1) Mask interrupts and clear any latched status before touching anything else.
        //    IMR is 0x3C, ISR (write-1-to-clear) is 0x3E
        self.write16(0x3C, 0x0000);
        self.write16(0x3E, 0xFFFF);

        // 2) Soft reset (CR=0x37, RST bit). Self-clears when the chip is ready.
        self.write8(0x37, 0x10);
        for _ in 0..1_000_000 {
            if self.read8(0x37) & 0x10 == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 3) Read MAC from IDR0..5. The chip latches it from the EEPROM on reset.
        for i in 0..6 {
            self.mac[i] = self.read8(i);
        }

        // 4) Unlock Config0..Config5 via 9346CR (offset 0x50). On real boards the
        //    BIOS or PXE loader frequently leaves WoL bits set, which keeps the
        //    PHY parked in a low-power state and gives "link: down" forever even
        //    when the cable is fine. We need to clear those bits, but they live
        //    in locked registers
        self.write8(0x50, 0xC0);

        // 5) Clear WoL latches: Config3 (0x54) bits LinkUp(0x20)/MagicPkt(0x40)/
        //    LanWake(0x01); Config5 (0x56) wake-frame bits BWF/MWF/UWF/LanWake PMEStatus
        let cfg3 = self.read8(0x54);
        self.write8(0x54, cfg3 & !0x61);
        let cfg5 = self.read8(0x56);
        self.write8(0x56, cfg5 & !0x1F);

        // 6) C+ Command register (0xE0). Read-modify-write so we keep whatever
        //    chip-revision-specific bits the EEPROM/BIOS already set, and just
        //    add RxChkSum (0x20) | RxVlan (0x40), and clear PktCntrDisable
        //    (0x80) so packet counters increment normally
        let cur_cp = (self.read32(0xE0) & 0xFFFF) as u16;
        let new_cp = (cur_cp | 0x0060) & !0x0080;
        self.write16(0xE0, new_cp);

        // 7) Build descriptor rings using ordered volatile writes so the chip
        //    never sees a half-published descriptor (see write_desc above)
        let tx_phys = super::virt_to_phys(self.tx_ring.0.as_ptr() as u64);
        let rx_phys = super::virt_to_phys(self.rx_ring.0.as_ptr() as u64);

        for i in 0..TX_DESC_COUNT {
            let buf_phys = super::virt_to_phys(self.tx_bufs.0[i].as_ptr() as u64);
            let eor = if i == TX_DESC_COUNT - 1 { DESC_EOR } else { 0 };
            unsafe {
                write_desc(&mut self.tx_ring.0[i] as *mut Desc, eor, 0, buf_phys);
            }
        }

        for i in 0..RX_DESC_COUNT {
            let buf_phys = super::virt_to_phys(self.rx_bufs.0[i].as_ptr() as u64);
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

        // 8) Descriptor base addresses: TNPDS at 0x20 (TX), RDSAR at 0xE4 (RX)
        self.write32(0x20, tx_phys as u32);
        self.write32(0x24, (tx_phys >> 32) as u32);
        self.write32(0xE4, rx_phys as u32);
        self.write32(0xE8, (rx_phys >> 32) as u32);

        // 9) RX max size (0xDA) and Max-Tx-Packet-Size in 128B units (0xEC)
        //    MTPS=0x3F gives ~8 KiB which comfortably covers our 1536B buffers
        self.write16(0xDA, BUF_SIZE as u16);
        self.write8(0xEC, 0x3F);

        // 10) Multicast filter: accept-all so DHCP/ARP-request broadcasts land
        self.write32(0x08, 0xFFFF_FFFF);
        self.write32(0x0C, 0xFFFF_FFFF);

        // 11) Read chip hw_ver from TxConfig before writing it. The hw_ver bits
        //     ([30:26]+[23:20] masked by 0x7CF00000) are read-only, so reading
        //     here gives the silicon revision regardless of any prior write
        //     We need this before writing TxConfig so we can set TXCFG_ETHER_CLKEN
        //     (bit 29) for G/H variants - without it the Ethernet MAC clock is
        //     gated and TX frames never reach the wire even though the descriptor
        //     OWN bit is cleared by the chip
        let hwver = (self.read32(0x40) & 0x7CF0_0000) >> 20;
        // hwver thresholds after masking + >>20:
        //   RTL8168B/C/D: 0x100..0x27F
        //   RTL8168E/EV:  0x280..0x47F
        //   RTL8168F:     0x480..0x4FF
        //   RTL8168G/GU:  0x500..0x53F
        //   RTL8168H/EP:  0x540..0x7CF
        let txconfig = if hwver >= 0x500 {
            0x0300_0700 | TXCFG_ETHER_CLKEN
        } else {
            0x0300_0700
        };
        // TxConfig: IFG=3, MaxDMA=7 (1024B) + version-specific clock bits
        // RxConfig is deliberately deferred until AFTER CR=TE/RE per Linux r8169
        // ordering - on several 8168 revisions a pre-enable RxConfig is silently
        // ignored and RX comes up in a half-configured state
        self.write32(0x40, txconfig);

        // 12) Re-lock config registers
        self.write8(0x50, 0x00);

        // 13) Make every descriptor write above visible before we let the chip
        //     start chasing the rings.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 14) Enable TX/RX engines (CR = TE|RE) BEFORE poking the PHY: the moment
        //     auto-negotiation finishes we want the descriptor pipe already running.
        self.write8(0x37, 0x04 | 0x08);

        // 15) Now program RxConfig so the engine is enabled when the filter rules
        //     land. AB/AM/APM/AAP filter, MaxDMA=7, RXFTH=no-threshold
        self.write32(0x44, 0x0000_E70F);

        // 15b) For RTL8168G/H, apply ERI (External Register Interface) patches
        //      that configure the chip's embedded MCU packet filter and FIFO
        //      without loading a full firmware blob. Without these writes the MCU
        //      stays in reset state and never fills RX descriptors - the second
        //      root cause of "rx: 0" on real G/H boards (the first being the
        //      missing TXCFG_ETHER_CLKEN above)
        if hwver >= 0x500 {
            self.init_g_h_eri();
        }

        // 16) Ack any ISR bits that latched during the bring-up traffic.
        self.write16(0x3E, 0xFFFF);

        // 17) Log key registers and chip family. hwver was already computed in
        //     step 11 from the read-only TxConfig bits
        let cr  = self.read8(0x37);
        let cp  = self.read32(0xE0) & 0xFFFF;
        let txc = self.read32(0x40);
        let rxc = self.read32(0x44);
        crate::serial_println!(
            "[rtl8168] CR={:#04x} CPlusCmd={:#06x} TxConfig={:#010x} RxConfig={:#010x}",
            cr, cp, txc, rxc
        );
        // Correct thresholds: after (txc & 0x7CF00000) >> 20 the max possible
        // value is 0x7CF. Old code compared against 0xB00/0x900 (unreachable)
        let family = if hwver >= 0x540 {
            "RTL8168H/EP/FP (ERI patches applied, firmware blob not loaded)"
        } else if hwver >= 0x500 {
            "RTL8168G/GU (TXCFG_ETHER_CLKEN + ERI patches applied)"
        } else if hwver >= 0x480 {
            "RTL8168F (no firmware needed)"
        } else if hwver >= 0x280 {
            "RTL8168E/EV (no firmware needed)"
        } else if hwver >= 0x100 {
            "RTL8168B/C/D (no firmware needed)"
        } else {
            "RTL8169 / pre-8168B"
        };
        crate::serial_println!(
            "[rtl8168] chip hw_ver={:#x} family: {}",
            hwver, family
        );

        // 18) Bring the PHY out of any BIOS-induced power-down/isolation and
        //     restart 802.3 auto-negotiation. This is the actual fix for
        //     "link: down" on real hardware - without it the MAC is fully alive
        //     but the PHY never lights up
        self.phy_init();
    }

    // Write a 16-bit value to MII register reg of the internal PHY through
    // PHYAR (offset 0x60). Bit 31 is the start flag; the chip clears it on completion
    fn mdio_write(&self, reg: u8, val: u16) {
        let v = (1u32 << 31) | ((reg as u32 & 0x1F) << 16) | (val as u32);
        self.write32(0x60, v);
        for _ in 0..1_000_000 {
            if self.read32(0x60) & (1 << 31) == 0 { return; }
            core::hint::spin_loop();
        }
    }

    // Read MII register reg. Returns 0xFFFF on timeout, which is the normal
    // "no PHY" sentinel and will surface as link-down rather than a silent stall
    fn mdio_read(&self, reg: u8) -> u16 {
        let v = (reg as u32 & 0x1F) << 16;
        self.write32(0x60, v);
        for _ in 0..1_000_000 {
            let r = self.read32(0x60);
            if r & (1 << 31) != 0 { return (r & 0xFFFF) as u16; }
            core::hint::spin_loop();
        }
        0xFFFF
    }

    /// Reset the PHY, advertise 10/100/1000 full+half duplex and restart
    /// auto-neg. Polls PHYStatus (0x6C) for a short while so we can log the
    /// negotiated mode; upper layers keep calling link_up() so we don't have
    /// to block until success
    fn phy_init(&self) {
        // BMCR (MII reg 0): bit 15 = soft reset, self-clears when complete
        self.mdio_write(0, 0x8000);
        for _ in 0..1000 {
            if self.mdio_read(0) & 0x8000 == 0 { break; }
            for _ in 0..10_000 { core::hint::spin_loop(); }
        }

        // ANAR (reg 4): advertise 10/100 full+half plus the 802.3 selector
        self.mdio_write(4, 0x01E1);
        // 1000-T Control (reg 9): advertise 1000BASE-T full+half
        self.mdio_write(9, 0x0300);
        // BMCR: AutoNegEnable | RestartAutoNeg
        self.mdio_write(0, 0x1200);

        // Give the PHY a moment to converge so the boot log shows a useful
        // state. We deliberately don't wait the full +-3s of auto-neg here -
        // the dhcp/arp layers already poll link_up() and will pick up the
        // link as soon as it comes up
        for _ in 0..200 {
            if self.read8(0x6C) & 0x02 != 0 { break; }
            for _ in 0..200_000 { core::hint::spin_loop(); }
        }

        let s = self.read8(0x6C);
        let speed = if s & 0x10 != 0 { "1000M" }
            else if s & 0x08 != 0 { "100M" }
            else if s & 0x04 != 0 { "10M" }
            else { "?" };
        crate::serial_println!(
            "[rtl8168] PHYStatus={:#04x} link={} duplex={} speed={}",
            s,
            s & 0x02 != 0,
            if s & 0x01 != 0 { "full" } else { "half" },
            speed,
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
        // Volatile read so we always pick up the chip's most recent OWN-clear,
        // not a cached value from the last time we touched this descriptor
        let cur_flags = unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(self.tx_ring.0[i].flags))
        };
        if cur_flags & DESC_OWN != 0 {
            return false;
        }

        let eor = if i == TX_DESC_COUNT - 1 { DESC_EOR } else { 0 };
        let buf_phys = super::virt_to_phys(self.tx_bufs.0[i].as_ptr() as u64);
        self.tx_bufs.0[i][..data.len()].copy_from_slice(data);

        unsafe {
            write_desc(
                &mut self.tx_ring.0[i] as *mut Desc,
                DESC_OWN | DESC_FS | DESC_LS | eor | (data.len() as u32),
                0,
                buf_phys,
            );
        }

        // Make sure the descriptor + buffer are globally visible before we
        // tell the chip to look at the ring
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.write8(0x38, 0x40); // TPPoll: NPQ

        // Bring-up diagnostic: for the first few frames, sample the OWN bit a
        // moment after kicking TPPoll. If the chip is processing the ring it
        // will have cleared OWN by the time the busy-loop ends. If OWN is
        // still set, the chip is silently ignoring our descriptors - that is
        // the signal you are looking at on a board that needs OOB firmware,
        // or where TNPDS / TxConfig is misprogrammed
        if self.send_log_count < 3 {
            for _ in 0..200_000 { core::hint::spin_loop(); }
            let post = unsafe {
                core::ptr::read_volatile(core::ptr::addr_of!(self.tx_ring.0[i].flags))
            };
            let cr_now = self.read8(0x37);
            let isr_now = unsafe {
                core::ptr::read_volatile((self.mmio_base + 0x3E) as *const u16)
            };
            crate::serial_println!(
                "[rtl8168] tx#{} idx={} len={} buf_phys={:#x} desc_flags_post={:#010x} CR={:#04x} ISR={:#06x}",
                self.send_log_count, i, data.len(), buf_phys, post, cr_now, isr_now
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
            let buf_phys = super::virt_to_phys(self.rx_bufs.0[i].as_ptr() as u64);
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
        self.write16(0x3E, 0xFFFF);
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
        self.read8(0x6C) & 0x02 != 0
    }

    fn get_mac(&self) -> [u8; 6] {
        self.mac
    }

    fn diag(&self) {
        Rtl8168::diag(self);
    }
}
