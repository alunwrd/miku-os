use crate::drivers::bus::pci::PciDevice;
use crate::net::NetworkDriver;
use x86_64::instructions::port::Port;

pub const VENDOR_VIRTIO: u16  = 0x1AF4;
pub const DEV_VIRTIO_NET: u16 = 0x1000;

const REG_DEVICE_FEATURES:  u16 = 0x00;
const REG_GUEST_FEATURES:   u16 = 0x04;
const REG_QUEUE_ADDRESS:    u16 = 0x08;
const REG_QUEUE_SIZE:       u16 = 0x0C;
const REG_QUEUE_SELECT:     u16 = 0x0E;
const REG_QUEUE_NOTIFY:     u16 = 0x10;
const REG_DEVICE_STATUS:    u16 = 0x12;
const REG_ISR_STATUS:       u16 = 0x13;
const REG_MAC_0:            u16 = 0x14;
const REG_NET_STATUS:       u16 = 0x1A;

const STATUS_ACKNOWLEDGE: u8 = 0x01;
const STATUS_DRIVER:      u8 = 0x02;
const STATUS_DRIVER_OK:   u8 = 0x04;
const STATUS_FAILED:      u8 = 0x80;

const FEATURE_MAC:      u32 = 1 << 5;
const FEATURE_STATUS:   u32 = 1 << 16;

const VRING_DESC_F_WRITE: u16 = 0x0002;
const VRING_DESC_F_NEXT:  u16 = 0x0001;

const QUEUE_SIZE: usize = 16;

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct VirtioNetHdr {
    flags:       u8,
    gso_type:    u8,
    hdr_len:     u16,
    gso_size:    u16,
    csum_start:  u16,
    csum_offset: u16,
}

impl VirtioNetHdr {
    const fn zero() -> Self {
        Self { flags: 0, gso_type: 0, hdr_len: 0, gso_size: 0, csum_start: 0, csum_offset: 0 }
    }
}

const BUF_SIZE: usize = 1526;

#[repr(C)]
struct Desc {
    addr:  u64,
    len:   u32,
    flags: u16,
    next:  u16,
}

#[repr(C)]
struct AvailRing {
    flags: u16,
    idx:   u16,
    ring:  [u16; QUEUE_SIZE],
}

#[repr(C)]
struct UsedElem {
    id:  u32,
    len: u32,
}

#[repr(C)]
struct UsedRing {
    flags: u16,
    idx:   u16,
    ring:  [UsedElem; QUEUE_SIZE],
}

#[repr(C, align(4096))]
struct Virtqueue {
    desc:  [Desc; 16],
    avail: AvailRing,
    _pad:  [u8; 3804],
    used:  UsedRing,
}

#[repr(align(4096))]
struct RxBufs([[u8; BUF_SIZE]; QUEUE_SIZE]);

#[repr(align(4096))]
struct TxBufs([[u8; BUF_SIZE]; QUEUE_SIZE]);

extern crate alloc;
use alloc::boxed::Box;

pub struct VirtioNet {
    io_base:      u16,
    mac:          [u8; 6],
    rx_queue:     Box<Virtqueue>,
    tx_queue:     Box<Virtqueue>,
    rx_bufs:      Box<RxBufs>,
    tx_bufs:      Box<TxBufs>,
    rx_last_used: u16,
    tx_free_idx:  usize,
}

macro_rules! ior8  { ($base:expr, $off:expr) => { unsafe { Port::<u8>::new($base + $off).read() } } }
macro_rules! ior16 { ($base:expr, $off:expr) => { unsafe { Port::<u16>::new($base + $off).read() } } }
macro_rules! ior32 { ($base:expr, $off:expr) => { unsafe { Port::<u32>::new($base + $off).read() } } }
macro_rules! iow8  { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u8>::new($base + $off).write($v) } } }
macro_rules! iow16 { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u16>::new($base + $off).write($v) } } }
macro_rules! iow32 { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u32>::new($base + $off).write($v) } } }

impl VirtioNet {
    pub fn new(pci: &PciDevice) -> Option<Self> {
        pci.enable_bus_mastering();

        let io_base = pci.io_bar(0)?;
        crate::log!("virtio-net: io_base=0x{:04X}", io_base);

        iow8!(io_base, REG_DEVICE_STATUS, 0);
        for _ in 0..100_000 { core::hint::spin_loop(); }

        iow8!(io_base, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        let dev_features = ior32!(io_base, REG_DEVICE_FEATURES);
        let our_features = dev_features & (FEATURE_MAC | FEATURE_STATUS);
        iow32!(io_base, REG_GUEST_FEATURES, our_features);
        crate::log!("virtio-net: features dev=0x{:08X} guest=0x{:08X}", dev_features, our_features);

        let mut mac = [0u8; 6];
        for i in 0..6 {
            mac[i] = ior8!(io_base, REG_MAC_0 + i as u16);
        }
        crate::log!("virtio-net: mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

        iow16!(io_base, REG_QUEUE_SELECT, 0);
        let rx_qsize = ior16!(io_base, REG_QUEUE_SIZE) as usize;
        crate::log!("virtio-net: rx queue size = {}", rx_qsize);

        let mut drv = VirtioNet {
            io_base,
            mac,
            rx_queue:     Box::new(unsafe { core::mem::zeroed() }),
            tx_queue:     Box::new(unsafe { core::mem::zeroed() }),
            rx_bufs:      Box::new(RxBufs([[0u8; BUF_SIZE]; QUEUE_SIZE])),
            tx_bufs:      Box::new(TxBufs([[0u8; BUF_SIZE]; QUEUE_SIZE])),
            rx_last_used: 0,
            tx_free_idx:  0,
        };

        drv.setup_rx();
        drv.setup_tx();

        iow8!(io_base, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);

        Some(drv)
    }

    fn setup_rx(&mut self) {
        let io = self.io_base;
        iow16!(io, REG_QUEUE_SELECT, 0);

        let q = &mut *self.rx_queue;
        for i in 0..QUEUE_SIZE {
            let phys = crate::net::virt_to_phys(self.rx_bufs.0[i].as_ptr() as u64);
            q.desc[i] = Desc {
                addr:  phys,
                len:   BUF_SIZE as u32,
                flags: VRING_DESC_F_WRITE,
                next:  0,
            };
            q.avail.ring[i] = i as u16;
        }
        q.avail.idx = QUEUE_SIZE as u16;
        q.avail.flags = 0;

        let q_phys = crate::net::virt_to_phys(q as *const Virtqueue as u64);
        iow32!(io, REG_QUEUE_ADDRESS, (q_phys / 4096) as u32);
        iow16!(io, REG_QUEUE_NOTIFY, 0);
    }

    fn setup_tx(&mut self) {
        let io = self.io_base;
        iow16!(io, REG_QUEUE_SELECT, 1);

        let q = &mut *self.tx_queue;
        for i in 0..QUEUE_SIZE {
            let phys = crate::net::virt_to_phys(self.tx_bufs.0[i].as_ptr() as u64);
            q.desc[i] = Desc {
                addr:  phys,
                len:   BUF_SIZE as u32,
                flags: 0,
                next:  0,
            };
        }
        q.avail.flags = 0;
        q.avail.idx   = 0;

        let q_phys = crate::net::virt_to_phys(q as *const Virtqueue as u64);
        iow32!(io, REG_QUEUE_ADDRESS, (q_phys / 4096) as u32);
    }
}

impl NetworkDriver for VirtioNet {
    fn send(&mut self, data: &[u8]) -> bool {
        if data.len() + 10 > BUF_SIZE { return false; }

        let q = &mut *self.tx_queue;
        let avail_idx = q.avail.idx as usize;
        let desc_idx  = avail_idx % QUEUE_SIZE;

        let used_idx = unsafe {
            core::ptr::read_volatile(&q.used.idx) as usize
        };
        if (avail_idx.wrapping_sub(used_idx)) >= QUEUE_SIZE {
            return false;
        }

        let hdr = VirtioNetHdr::zero();
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const _ as *const u8, 10)
        };
        let total = 10 + data.len();
        self.tx_bufs.0[desc_idx][..10].copy_from_slice(hdr_bytes);
        self.tx_bufs.0[desc_idx][10..total].copy_from_slice(data);

        let phys = crate::net::virt_to_phys(self.tx_bufs.0[desc_idx].as_ptr() as u64);
        q.desc[desc_idx] = Desc {
            addr:  phys,
            len:   total as u32,
            flags: 0,
            next:  0,
        };

        q.avail.ring[avail_idx % QUEUE_SIZE] = desc_idx as u16;
        unsafe { core::arch::asm!("mfence", options(nostack, nomem)); }
        q.avail.idx = q.avail.idx.wrapping_add(1);
        unsafe { core::arch::asm!("mfence", options(nostack, nomem)); }

        iow16!(self.io_base, REG_QUEUE_NOTIFY, 1);
        true
    }

    fn recv(&mut self, handler: &mut dyn FnMut(&[u8])) {
        let q = &mut *self.rx_queue;

        loop {
            let used_idx = unsafe {
                core::ptr::read_volatile(&q.used.idx)
            };

            if self.rx_last_used == used_idx { break; }

            let elem = &q.used.ring[self.rx_last_used as usize % QUEUE_SIZE];
            let desc_id = elem.id as usize % QUEUE_SIZE;
            let pkt_len = elem.len as usize;

            if pkt_len > 10 {
                handler(&self.rx_bufs.0[desc_id][10..pkt_len]);
            }

            q.desc[desc_id].len   = BUF_SIZE as u32;
            q.desc[desc_id].flags = VRING_DESC_F_WRITE;

            let avail_idx = q.avail.idx as usize;
            q.avail.ring[avail_idx % QUEUE_SIZE] = desc_id as u16;
            unsafe { core::arch::asm!("mfence", options(nostack, nomem)); }
            q.avail.idx = q.avail.idx.wrapping_add(1);
            unsafe { core::arch::asm!("mfence", options(nostack, nomem)); }

            self.rx_last_used = self.rx_last_used.wrapping_add(1);
        }

            iow16!(self.io_base, REG_QUEUE_NOTIFY, 0);
    }

    fn has_packet(&self) -> bool {
        let used_idx = unsafe {
            core::ptr::read_volatile(&(*self.rx_queue).used.idx)
        };
        self.rx_last_used != used_idx
    }

    fn link_up(&self) -> bool {
        ior16!(self.io_base, REG_NET_STATUS) & 0x01 != 0
    }

    fn get_mac(&self) -> [u8; 6] {
        self.mac
    }
}
