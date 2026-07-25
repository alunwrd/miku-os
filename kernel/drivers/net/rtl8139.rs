use crate::drivers::bus::pci::PciDevice;
use crate::net::NetworkDriver;
use alloc::boxed::Box;
use x86_64::instructions::port::Port;

const TX_BUF_SIZE: usize = 1536;
const RX_BUF_SIZE: usize = 8192 + 16 + 1500;

#[repr(align(4))]
struct TxBuf([u8; TX_BUF_SIZE]);
#[repr(align(4096))]
struct RxBuf([u8; RX_BUF_SIZE]);

pub struct Rtl8139 {
    base: u16,
    pub mac: [u8; 6],
    tx_slot: usize,
    rx_offset: usize,
    tx_bufs: Box<[TxBuf; 4]>,
    rx_buf: Box<RxBuf>,
}

impl Rtl8139 {
    pub fn new(pci: &PciDevice) -> Option<Self> {
        pci.enable_bus_mastering();
        let mut drv = Self {
            base: pci.io_bar(0)?,
            mac: [0; 6],
            tx_slot: 0,
            rx_offset: 0,
            tx_bufs: Box::new([
                TxBuf([0; TX_BUF_SIZE]),
                TxBuf([0; TX_BUF_SIZE]),
                TxBuf([0; TX_BUF_SIZE]),
                TxBuf([0; TX_BUF_SIZE]),
            ]),
            rx_buf: Box::new(RxBuf([0; RX_BUF_SIZE])),
        };
        drv.init();
        Some(drv)
    }

    fn write8(&self, r: u8, v: u8) {
        unsafe {
            Port::<u8>::new(self.base + r as u16).write(v);
        }
    }
    fn write16(&self, r: u8, v: u16) {
        unsafe {
            Port::<u16>::new(self.base + r as u16).write(v);
        }
    }
    fn write32(&self, r: u8, v: u32) {
        unsafe {
            Port::<u32>::new(self.base + r as u16).write(v);
        }
    }
    fn read8(&self, r: u8) -> u8 {
        unsafe { Port::<u8>::new(self.base + r as u16).read() }
    }
    fn read16(&self, r: u8) -> u16 {
        unsafe { Port::<u16>::new(self.base + r as u16).read() }
    }
    fn read32(&self, r: u8) -> u32 {
        unsafe { Port::<u32>::new(self.base + r as u16).read() }
    }

    fn init(&mut self) {
        self.write8(0x52, 0x00);
        self.write8(0x37, 0x10);
        for _ in 0..100_000 {
            if self.read8(0x37) & 0x10 == 0 {
                break;
            }
        }
        for i in 0..6 {
            self.mac[i] = self.read8(0x00 + i as u8);
        }
        let rx_phys = crate::net::virt_to_phys(self.rx_buf.0.as_ptr() as u64);
        self.write32(0x30, rx_phys as u32);
        self.write32(
            0x44,
            (7 << 8) | (0 << 11) | (1 << 7) | (1 << 3) | (1 << 1) | (1 << 0),
        );
        self.write32(0x40, 0x0300);
        self.write16(0x3C, 0x0001 | 0x0004 | 0x0008 | 0x0002);
        self.write8(0x37, 0x04 | 0x08);
    }
}

impl NetworkDriver for Rtl8139 {
    fn send(&mut self, data: &[u8]) -> bool {
        if data.len() > TX_BUF_SIZE {
            return false;
        }
        let slot = self.tx_slot;
        self.tx_bufs[slot].0[..data.len()].copy_from_slice(data);
        let tx_phys = crate::net::virt_to_phys(self.tx_bufs[slot].0.as_ptr() as u64);
        self.write32(0x20 + slot as u8 * 4, tx_phys as u32);
        self.write32(0x10 + slot as u8 * 4, data.len() as u32);
        self.tx_slot = (slot + 1) % 4;
        true
    }

    fn recv(&mut self, handler: &mut dyn FnMut(&[u8])) {
        loop {
            if self.read8(0x37) & 0x01 != 0 {
                break;
            }
            let off = self.rx_offset;
            let status = u16::from_le_bytes([self.rx_buf.0[off], self.rx_buf.0[off + 1]]);
            let pkt_len =
                u16::from_le_bytes([self.rx_buf.0[off + 2], self.rx_buf.0[off + 3]]) as usize;
            if status & 0x0001 == 0 || pkt_len < 4 || pkt_len > 1522 {
                self.rx_offset = 0;
                self.write16(0x38, 0xFFF0);
                break;
            }
            if off + 4 + pkt_len - 4 <= RX_BUF_SIZE {
                handler(&self.rx_buf.0[off + 4..off + pkt_len]);
            }
            self.rx_offset = (off + 4 + pkt_len + 3) & !3;
            self.rx_offset %= 8192 + 16;
            self.write16(0x38, (self.rx_offset as u16).wrapping_sub(0x10));
        }
        self.write16(0x3E, 0xFFFF);
    }

    fn has_packet(&self) -> bool {
        self.read8(0x37) & 0x01 == 0
    }
    fn link_up(&self) -> bool {
        true
    }
    fn get_mac(&self) -> [u8; 6] {
        self.mac
    }
}
