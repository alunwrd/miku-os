// Legacy (port-I/O) virtio-net
//
// The device dictates the vring layout: in legacy virtio the queue size is
// read-only, and every offset inside the ring (available ring start, used ring
// start) is computed from it, so everything here is sized from `queue_size` at
// probe time. QEMU reports 256.

use crate::drivers::bus::pci::PciDevice;
use crate::net::{NetworkDriver, RxMeta};
use core::sync::atomic::{fence, Ordering};
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
/// With MSI-X on, the legacy header grows by these two and the device's own
/// configuration (the MAC, the status) moves four bytes on
const REG_MSI_CONFIG_VECTOR: u16 = 0x14;
const REG_MSI_QUEUE_VECTOR:  u16 = 0x16;
const MSIX_CONFIG_SHIFT:     u16 = 4;
const VIRTIO_MSI_NO_VECTOR:  u16 = 0xFFFF;

const STATUS_ACKNOWLEDGE: u8 = 0x01;
const STATUS_DRIVER:      u8 = 0x02;
const STATUS_DRIVER_OK:   u8 = 0x04;

const FEATURE_MAC:      u32 = 1 << 5;
const FEATURE_STATUS:   u32 = 1 << 16;

const VRING_DESC_F_WRITE: u16 = 0x0002;

const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 0x01;
const VIRTIO_NET_HDR_F_DATA_VALID: u8 = 0x02;

/// The virtio net header that prefixes every buffer in both directions
const HDR_LEN: usize = 10;
/// Header plus a full ethernet frame with VLAN tag
const BUF_SIZE: usize = HDR_LEN + 1516;

/// Legacy vring alignment: the used ring starts on a page boundary
const VRING_ALIGN: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr:  u64,
    len:   u32,
    flags: u16,
    next:  u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElem {
    id:  u32,
    len: u32,
}

macro_rules! ior8  { ($base:expr, $off:expr) => { unsafe { Port::<u8>::new($base + $off).read() } } }
macro_rules! ior16 { ($base:expr, $off:expr) => { unsafe { Port::<u16>::new($base + $off).read() } } }
macro_rules! ior32 { ($base:expr, $off:expr) => { unsafe { Port::<u32>::new($base + $off).read() } } }
macro_rules! iow8  { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u8>::new($base + $off).write($v) } } }
macro_rules! iow16 { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u16>::new($base + $off).write($v) } } }
macro_rules! iow32 { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u32>::new($base + $off).write($v) } } }

/// Contiguous physical memory, addressed through the HHDM. The device reads
/// descriptor rings and packet buffers, so both are one physical run
fn alloc_dma(bytes: usize) -> Option<(u64, u64)> {
    let frames = (bytes + 4095) / 4096;
    let phys = crate::mm::pmm::alloc_frames(frames)?;
    let virt = crate::arch::x86_64::grub::phys_to_virt(phys);
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, frames * 4096) };
    Some((virt, phys))
}

/// One split virtqueue, laid out for the size the device reported
struct Vq {
    size: usize,
    base: u64,
    base_phys: u64,
    /// Byte offset of the used ring from `base`
    used_off: usize,
    /// Packet buffers, `size` of them, each BUF_SIZE bytes
    bufs: u64,
    bufs_phys: u64,
    /// How far we have consumed the used ring
    last_used: u16,
}

impl Vq {
    /// The legacy split-vring layout, in specification order: descriptors, the
    /// available ring, padding to the next page, the used ring
    fn new(size: usize) -> Option<Self> {
        let desc_bytes  = size * core::mem::size_of::<Desc>();
        let avail_bytes = 4 + 2 * size + 2;      // flags, idx, ring, used_event
        let used_off    = (desc_bytes + avail_bytes + VRING_ALIGN - 1) & !(VRING_ALIGN - 1);
        let used_bytes  = 4 + 8 * size + 2;      // flags, idx, ring, avail_event

        let (base, base_phys) = alloc_dma(used_off + used_bytes)?;
        let (bufs, bufs_phys) = alloc_dma(size * BUF_SIZE)?;

        Some(Vq { size, base, base_phys, used_off, bufs, bufs_phys, last_used: 0 })
    }

    #[inline] fn desc(&self, i: usize) -> *mut Desc {
        (self.base as *mut Desc).wrapping_add(i)
    }
    #[inline] fn avail_base(&self) -> u64 {
        self.base + (self.size * core::mem::size_of::<Desc>()) as u64
    }
    #[inline] fn avail_flags(&self) -> *mut u16 { self.avail_base() as *mut u16 }
    #[inline] fn avail_idx(&self) -> *mut u16 { (self.avail_base() + 2) as *mut u16 }
    #[inline] fn avail_ring(&self, i: usize) -> *mut u16 {
        ((self.avail_base() + 4) as *mut u16).wrapping_add(i)
    }
    #[inline] fn used_base(&self) -> u64 { self.base + self.used_off as u64 }
    #[inline] fn used_idx(&self) -> *const u16 { (self.used_base() + 2) as *const u16 }
    #[inline] fn used_elem(&self, i: usize) -> *const UsedElem {
        ((self.used_base() + 4) as *const UsedElem).wrapping_add(i)
    }

    #[inline] fn buf(&self, i: usize) -> *mut u8 {
        (self.bufs as *mut u8).wrapping_add(i * BUF_SIZE)
    }
    #[inline] fn buf_phys(&self, i: usize) -> u64 {
        self.bufs_phys + (i * BUF_SIZE) as u64
    }

    /// Publish one descriptor index in the available ring
    fn publish(&mut self, desc_idx: usize) {
        unsafe {
            let idx = core::ptr::read_volatile(self.avail_idx());
            core::ptr::write_volatile(self.avail_ring(idx as usize % self.size), desc_idx as u16);
            // The ring entry must be visible before the index that covers it
            fence(Ordering::Release);
            core::ptr::write_volatile(self.avail_idx(), idx.wrapping_add(1));
            fence(Ordering::SeqCst);
        }
    }
}

pub struct VirtioNet {
    io_base: u16,
    mac:     [u8; 6],
    rx:      Vq,
    tx:      Vq,
    /// MSI-X is on, and with it the shifted configuration layout
    msix:    bool,
}

unsafe impl Send for VirtioNet {}

impl VirtioNet {
    pub fn new(pci: &PciDevice) -> Option<Self> {
        pci.enable_bus_mastering();

        let io_base = pci.io_bar(0)?;
        crate::log!("virtio-net: io_base=0x{:04X}", io_base);

        iow8!(io_base, REG_DEVICE_STATUS, 0);
        for _ in 0..100_000 { core::hint::spin_loop(); }

        iow8!(io_base, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        let dev_features = ior32!(io_base, REG_DEVICE_FEATURES);
        // MAC and link status only, no GUEST_CSUM or other offloads: the device
        // then hands over fully checksummed frames, as the demux's software
        // verification expects
        let our_features = dev_features & (FEATURE_MAC | FEATURE_STATUS);
        iow32!(io_base, REG_GUEST_FEATURES, our_features);
        crate::log!("virtio-net: features dev=0x{:08X} guest=0x{:08X}", dev_features, our_features);

        let mut mac = [0u8; 6];
        for i in 0..6 {
            mac[i] = ior8!(io_base, REG_MAC_0 + i as u16);
        }
        crate::log!("virtio-net: mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

        let rx = Self::setup_queue(io_base, 0, true)?;
        let tx = Self::setup_queue(io_base, 1, false)?;

        iow8!(io_base, REG_DEVICE_STATUS,
              STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);

        // The device is live, so the receive buffers can be offered
        let mut drv = VirtioNet { io_base, mac, rx, tx, msix: false };
        drv.fill_rx();
        Some(drv)
    }

    /// Read the queue's size from the device, build a ring that shape, and hand
    /// the device its address
    fn setup_queue(io_base: u16, index: u16, device_writes: bool) -> Option<Vq> {
        iow16!(io_base, REG_QUEUE_SELECT, index);
        let size = ior16!(io_base, REG_QUEUE_SIZE) as usize;
        crate::log!("virtio-net: queue {} size = {}", index, size);
        if size == 0 || !size.is_power_of_two() || size > 1024 {
            crate::log_err!("virtio-net: queue {} has an unusable size {}", index, size);
            return None;
        }

        let vq = Vq::new(size)?;

        // Every descriptor points at its own buffer for the life of the queue;
        // only the length and flags change
        for i in 0..size {
            unsafe {
                core::ptr::write_volatile(vq.desc(i), Desc {
                    addr:  vq.buf_phys(i),
                    len:   BUF_SIZE as u32,
                    flags: if device_writes { VRING_DESC_F_WRITE } else { 0 },
                    next:  0,
                });
            }
        }
        unsafe { core::ptr::write_volatile(vq.avail_flags(), 0) };

        iow16!(io_base, REG_QUEUE_SELECT, index);
        iow32!(io_base, REG_QUEUE_ADDRESS, (vq.base_phys / 4096) as u32);
        Some(vq)
    }

    /// Offer every receive descriptor to the device
    fn fill_rx(&mut self) {
        for i in 0..self.rx.size {
            self.rx.publish(i);
        }
        iow16!(self.io_base, REG_QUEUE_NOTIFY, 0);
    }
}

impl NetworkDriver for VirtioNet {
    fn send(&mut self, data: &[u8]) -> bool {
        if data.len() + HDR_LEN > BUF_SIZE { return false; }

        let avail_idx = unsafe { core::ptr::read_volatile(self.tx.avail_idx()) };
        let used_idx  = unsafe { core::ptr::read_volatile(self.tx.used_idx()) };
        // Both are free-running u16 counters, so the distance is measured in
        // u16. Widened to usize it reads ~65535 when avail wraps past used, and
        // the queue looks permanently full
        if avail_idx.wrapping_sub(used_idx) >= self.tx.size as u16 {
            return false;
        }

        let i = avail_idx as usize % self.tx.size;
        unsafe {
            let buf = self.tx.buf(i);
            core::ptr::write_bytes(buf, 0, HDR_LEN);
            core::ptr::copy_nonoverlapping(data.as_ptr(), buf.add(HDR_LEN), data.len());
            core::ptr::write_volatile(self.tx.desc(i), Desc {
                addr:  self.tx.buf_phys(i),
                len:   (HDR_LEN + data.len()) as u32,
                flags: 0,
                next:  0,
            });
        }
        self.tx.publish(i);
        iow16!(self.io_base, REG_QUEUE_NOTIFY, 1);
        true
    }

    fn recv(&mut self, handler: &mut dyn FnMut(&[u8], RxMeta) -> bool) {
        loop {
            let used_idx = unsafe { core::ptr::read_volatile(self.rx.used_idx()) };
            if self.rx.last_used == used_idx { break; }

            // used.idx is published before the ring entry it covers is read
            fence(Ordering::Acquire);

            let elem = unsafe {
                core::ptr::read_volatile(self.rx.used_elem(self.rx.last_used as usize % self.rx.size))
            };
            let id = elem.id as usize % self.rx.size;
            // elem.len is the device's word, clamped so a broken or hostile
            // backend cannot slice past the end of the buffer and panic ring 0
            let len = (elem.len as usize).min(BUF_SIZE);

            let mut go_on = true;
            if len > HDR_LEN {
                let buf = unsafe { core::slice::from_raw_parts(self.rx.buf(id), len) };
                // The header's flags say whether the transport checksum can be
                // trusted. NEEDS_CSUM: the backend left it incomplete;
                // DATA_VALID: it has checked. Software verification is wrong in
                // both cases
                let meta = RxMeta {
                    skip_l4_csum: buf[0] & (VIRTIO_NET_HDR_F_NEEDS_CSUM
                                          | VIRTIO_NET_HDR_F_DATA_VALID) != 0,
                };
                go_on = handler(&buf[HDR_LEN..], meta);
            }

            // Hand the buffer back to the device
            unsafe {
                core::ptr::write_volatile(self.rx.desc(id), Desc {
                    addr:  self.rx.buf_phys(id),
                    len:   BUF_SIZE as u32,
                    flags: VRING_DESC_F_WRITE,
                    next:  0,
                });
            }
            self.rx.publish(id);
            self.rx.last_used = self.rx.last_used.wrapping_add(1);

            // Frame taken and the descriptor recycled; the rest of the used
            // ring waits for the next call
            if !go_on { break; }
        }
        iow16!(self.io_base, REG_QUEUE_NOTIFY, 0);
        let _ = ior8!(self.io_base, REG_ISR_STATUS);
    }

    fn has_packet(&self) -> bool {
        let used_idx = unsafe { core::ptr::read_volatile(self.rx.used_idx()) };
        self.rx.last_used != used_idx
    }

    fn link_up(&self) -> bool {
        let at = REG_NET_STATUS + if self.msix { MSIX_CONFIG_SHIFT } else { 0 };
        ior16!(self.io_base, at) & 0x01 != 0
    }

    /// MSI-X entry 0 for the receive queue, nothing for the transmit queue or a
    /// configuration change. MSI-X goes on first: in the legacy layout the
    /// vector registers exist only once it is (virtio 1.0 4.1.4.8)
    fn enable_irq(&mut self, f: crate::drivers::bus::msi::PciFn) -> bool {
        if !crate::net::irq::has_msix(f) {
            return false;
        }
        if !crate::net::irq::enable(f, Some(0), None) {
            return false;
        }
        self.msix = true;
        iow16!(self.io_base, REG_MSI_CONFIG_VECTOR, VIRTIO_MSI_NO_VECTOR);
        iow16!(self.io_base, REG_QUEUE_SELECT, 1);
        iow16!(self.io_base, REG_MSI_QUEUE_VECTOR, VIRTIO_MSI_NO_VECTOR);
        iow16!(self.io_base, REG_QUEUE_SELECT, 0);
        iow16!(self.io_base, REG_MSI_QUEUE_VECTOR, 0);
        // The device says no by reading back no vector
        if ior16!(self.io_base, REG_MSI_QUEUE_VECTOR) != 0 {
            crate::serial_println!("[virtio-net] the device refused an MSI-X vector - polling");
            crate::net::irq::disable();
            return false;
        }
        true
    }

    fn get_mac(&self) -> [u8; 6] {
        self.mac
    }
}
