use crate::drivers::bus::pci::PciDevice;
use crate::net::NetworkDriver;
use alloc::boxed::Box;
use core::sync::atomic::{fence, Ordering};

const E1000_CTRL: u32 = 0x0000;
const E1000_STATUS: u32 = 0x0008;
const E1000_EERD: u32 = 0x0014;
const E1000_ICR: u32 = 0x00C0;
const E1000_IMS: u32 = 0x00D0;
const E1000_IMC: u32 = 0x00D8;
const E1000_RCTL: u32 = 0x0100;
const E1000_TCTL: u32 = 0x0400;
const E1000_TIPG: u32 = 0x0410;
const E1000_RDBAL: u32 = 0x2800;
const E1000_RDBAH: u32 = 0x2804;
const E1000_RDLEN: u32 = 0x2808;
const E1000_RDH: u32 = 0x2810;
const E1000_RDT: u32 = 0x2818;
const E1000_TDBAL: u32 = 0x3800;
const E1000_TDBAH: u32 = 0x3804;
const E1000_TDLEN: u32 = 0x3808;
const E1000_TDH: u32 = 0x3810;
const E1000_TDT: u32 = 0x3818;
const E1000_MTA: u32 = 0x5200;
const E1000_RAL0: u32 = 0x5400;
const E1000_RAH0: u32 = 0x5404;

const RX_DESC_N: usize = 16;
const TX_DESC_N: usize = 16;
const BUF_SIZE: usize = 2048;

#[repr(C)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,
    length: u16,
    csum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

#[repr(C, align(128))]
struct RxRing([RxDesc; RX_DESC_N]);
#[repr(C, align(128))]
struct TxRing([TxDesc; TX_DESC_N]);
#[repr(C, align(4096))]
struct RxBufs([[u8; BUF_SIZE]; RX_DESC_N]);
#[repr(C, align(4096))]
struct TxBufs([[u8; BUF_SIZE]; TX_DESC_N]);

unsafe fn alloc_dma_zeroed<T>() -> *mut T {
    let size = core::mem::size_of::<T>();
    let frames = (size + 4095) / 4096;
    let phys = crate::mm::pmm::alloc_frames(frames).expect("PMM OOM during DMA alloc");
    
    crate::net::map_mmio(phys, (frames * 4096) as u64);
    
    let virt = crate::arch::x86_64::grub::phys_to_virt(phys);
    crate::serial_println!(
        "[e1000] alloc_dma: phys={:#x} virt={:#x} size={:#x} frames={}",
        phys, virt, size, frames
    );
    core::ptr::write_bytes(virt as *mut u8, 0, frames * 4096);
    virt as *mut T
}

#[inline]
fn virt_to_dma_phys(virt: u64) -> u64 {
    let hhdm = crate::arch::x86_64::grub::hhdm();
    debug_assert!(virt >= hhdm, "virt_to_dma_phys: address not in HHDM");
    virt - hhdm
}

pub struct E1000 {
    mmio: u64,
    pub mac: [u8; 6],
    rx_tail: usize,
    tx_tail: usize,
    rx_ring: *mut RxRing,
    tx_ring: *mut TxRing,
    rx_bufs: *mut RxBufs,
    tx_bufs: *mut TxBufs,
}

unsafe impl Send for E1000 {}

impl E1000 {
    pub fn new(pci: &PciDevice) -> Option<Box<Self>> {
        let mem_phys = pci.mem_bar(0)?;
        pci.enable_bus_mastering();

        let rx_ring = unsafe { alloc_dma_zeroed::<RxRing>() };
        let rx_bufs = unsafe { alloc_dma_zeroed::<RxBufs>() };
        let tx_bufs = unsafe { alloc_dma_zeroed::<TxBufs>() };
        let tx_ring = unsafe { alloc_dma_zeroed::<TxRing>() };

        unsafe {
            for i in 0..TX_DESC_N {
                (*tx_ring).0[i].status = 1;
            }
        }

        let mut drv = Box::new(Self {
            mmio: crate::arch::x86_64::grub::phys_to_virt(mem_phys),
            mac: [0; 6],
            rx_tail: 0,
            tx_tail: 0,
            rx_ring,
            tx_ring,
            rx_bufs,
            tx_bufs,
        });
        drv.init()?;
        Some(drv)
    }

    fn read32(&self, reg: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio + reg as u64) as *const u32) }
    }
    fn write32(&self, reg: u32, val: u32) {
        unsafe {
            core::ptr::write_volatile((self.mmio + reg as u64) as *mut u32, val);
        }
    }

    fn init(&mut self) -> Option<()> {
        let rx_ring = self.rx_ring;
        let tx_ring = self.tx_ring;
        let rx_bufs = self.rx_bufs;
        let tx_bufs = self.tx_bufs;

        crate::serial_println!("[e1000] init: 1 IMC");
        self.write32(E1000_IMC, 0xFFFFFFFF);
        let _ = self.read32(E1000_ICR);

        crate::serial_println!("[e1000] init: 2 CTRL reset");
        self.write32(E1000_CTRL, self.read32(E1000_CTRL) | (1 << 26));
        for _ in 0..1_000_000 {
            if self.read32(E1000_CTRL) & (1 << 26) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        crate::serial_println!("[e1000] init: 3 IMC+SLU");
        self.write32(E1000_IMC, 0xFFFFFFFF);
        let _ = self.read32(E1000_ICR);
        self.write32(E1000_CTRL, self.read32(E1000_CTRL) | (1 << 6));

        for _ in 0..100_000 {
            core::hint::spin_loop();
        }

        crate::serial_println!("[e1000] init: 4 EEPROM");
        for i in 0u32..3 {
            self.write32(E1000_EERD, 1 | (i << 8));
            for _ in 0..100_000 {
                let v = self.read32(E1000_EERD);
                if v & (1 << 4) != 0 {
                    let w = (v >> 16) as u16;
                    self.mac[(i * 2) as usize] = w as u8;
                    self.mac[(i * 2 + 1) as usize] = (w >> 8) as u8;
                    break;
                }
                core::hint::spin_loop();
            }
        }
        if self.mac == [0; 6] || self.mac == [0xFF; 6] {
            let lo = self.read32(E1000_RAL0);
            let hi = self.read32(E1000_RAH0);
            self.mac = [
                lo as u8,
                (lo >> 8) as u8,
                (lo >> 16) as u8,
                (lo >> 24) as u8,
                hi as u8,
                (hi >> 8) as u8,
            ];
        }
        crate::serial_println!(
            "[e1000] init: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.mac[0], self.mac[1], self.mac[2],
            self.mac[3], self.mac[4], self.mac[5]
        );

        crate::serial_println!("[e1000] init: 5 MTA");
        for i in 0..128u32 {
            self.write32(E1000_MTA + i * 4, 0);
        }

        crate::serial_println!("[e1000] init: 6 RX");
        unsafe {
            for i in 0..RX_DESC_N {
                let buf_virt = (*rx_bufs).0[i].as_ptr() as u64;
                let buf_phys = virt_to_dma_phys(buf_virt);
                (*rx_ring).0[i] = RxDesc {
                    addr: buf_phys,
                    length: 0,
                    csum: 0,
                    status: 0,
                    errors: 0,
                    special: 0,
                };
            }
        }

        let rphys = virt_to_dma_phys(rx_ring as u64);
        crate::serial_println!("[e1000] rx_ring phys=0x{:x}", rphys);
        self.write32(E1000_RDBAL, rphys as u32);
        self.write32(E1000_RDBAH, (rphys >> 32) as u32);
        self.write32(E1000_RDLEN, (RX_DESC_N * core::mem::size_of::<RxDesc>()) as u32);
        self.write32(E1000_RDH, 0);
        self.rx_tail = RX_DESC_N - 1;
        self.write32(E1000_RDT, self.rx_tail as u32);
        self.write32(E1000_RCTL, (1 << 1) | (1 << 15) | (1 << 26) | (1 << 4));

        crate::serial_println!("[e1000] init: 7 TX");
        unsafe {
            for i in 0..TX_DESC_N {
                let buf_virt = (*tx_bufs).0[i].as_ptr() as u64;
                let buf_phys = virt_to_dma_phys(buf_virt);
                (*tx_ring).0[i] = TxDesc {
                    addr: buf_phys,
                    length: 0,
                    cso: 0,
                    cmd: 0,
                    status: 1,
                    css: 0,
                    special: 0,
                };
            }
        }

        let tphys = virt_to_dma_phys(tx_ring as u64);
        crate::serial_println!("[e1000] tx_ring phys=0x{:x}", tphys);
        self.write32(E1000_TDBAL, tphys as u32);
        self.write32(E1000_TDBAH, (tphys >> 32) as u32);
        self.write32(E1000_TDLEN, (TX_DESC_N * core::mem::size_of::<TxDesc>()) as u32);
        self.write32(E1000_TDH, 0);
        self.tx_tail = 0;
        self.write32(E1000_TDT, 0);
        self.write32(E1000_TCTL, (1 << 1) | (1 << 3) | (0x10 << 4) | (0x40 << 12));
        self.write32(E1000_TIPG, 0x0060200A);

        self.write32(E1000_IMC, 0xFFFFFFFF);
        let _ = self.read32(E1000_ICR);

        fence(Ordering::SeqCst);
        crate::serial_println!("[e1000] init: done");
        Some(())
    }
}

impl NetworkDriver for E1000 {
    fn send(&mut self, data: &[u8]) -> bool {
        if data.len() > BUF_SIZE {
            return false;
        }
        let tail = self.tx_tail;

        fence(Ordering::SeqCst);
        let status = unsafe {
            core::ptr::read_volatile(&(*self.tx_ring).0[tail].status)
        };
        if status & 1 == 0 {
            return false;
        }

        unsafe {
            (&mut (*self.tx_bufs).0[tail])[..data.len()].copy_from_slice(data);
            (*self.tx_ring).0[tail].length = data.len() as u16;
            (*self.tx_ring).0[tail].cmd = 1 | 2 | 8;
            (*self.tx_ring).0[tail].status = 0;
        }

        fence(Ordering::SeqCst);
        self.tx_tail = (tail + 1) % TX_DESC_N;
        self.write32(E1000_TDT, self.tx_tail as u32);
        true
    }

    fn recv(&mut self, handler: &mut dyn FnMut(&[u8])) {
        loop {
            let next = (self.rx_tail + 1) % RX_DESC_N;
            fence(Ordering::SeqCst);

            let status = unsafe {
                core::ptr::read_volatile(&(*self.rx_ring).0[next].status)
            };
            if status & 1 == 0 {
                break;
            }

            let len = unsafe {
                core::ptr::read_volatile(&(*self.rx_ring).0[next].length) as usize
            };
            if len > 0 && len <= BUF_SIZE {
                let buf = unsafe { &(&(*self.rx_bufs).0[next])[..len] };
                handler(buf);
            }

            unsafe {
                (*self.rx_ring).0[next].status = 0;
            }
            self.rx_tail = next;
            self.write32(E1000_RDT, self.rx_tail as u32);
        }
        let _ = self.read32(E1000_ICR);
    }

    fn has_packet(&self) -> bool {
        fence(Ordering::SeqCst);
        let status = unsafe {
            core::ptr::read_volatile(
                &(*self.rx_ring).0[(self.rx_tail + 1) % RX_DESC_N].status,
            )
        };
        status & 1 != 0
    }

    fn link_up(&self) -> bool {
        self.read32(E1000_STATUS) & (1 << 1) != 0
    }

    fn get_mac(&self) -> [u8; 6] {
        self.mac
    }
}
