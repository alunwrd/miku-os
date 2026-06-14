// virtio-blk driver (legacy/transitional virtio-pci, port I/O transport)
//
// Same transport style as 'net/virtio.rs', but the ring layout is computed at
// runtime from the device-reported queue size, as the legacy spec requires -
// the device derives the avail/used ring offsets from *its* queue size, so a
// driver-side fixed layout only works by accident.
//
// I/O model: one synchronous request at a time through a driver-owned DMA
// bounce buffer (64 KiB max per request), polled to completion. The block layer above chunks larger transfers

extern crate alloc;

use alloc::boxed::Box;
use spin::Mutex as SpinMutex;
use x86_64::instructions::port::Port;

use super::driver::{BlkError, BlockDevInfo, BlockDriver};
use crate::net::pci::{pci_read32, pci_read8, PciDevice, VENDOR_VIRTIO};

pub const DEV_VIRTIO_BLK: u16 = 0x1001;

// Legacy virtio-pci register layout (BAR0, port I/O)
const REG_DEVICE_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES:  u16 = 0x04;
const REG_QUEUE_ADDRESS:   u16 = 0x08;
const REG_QUEUE_SIZE:      u16 = 0x0C;
const REG_QUEUE_SELECT:    u16 = 0x0E;
const REG_QUEUE_NOTIFY:    u16 = 0x10;
const REG_DEVICE_STATUS:   u16 = 0x12;
// Device-specific config (no MSI-X): virtio-blk capacity in 512-byte sectors
const REG_CONFIG_CAPACITY: u16 = 0x14;

const STATUS_ACKNOWLEDGE: u8 = 0x01;
const STATUS_DRIVER:      u8 = 0x02;
const STATUS_DRIVER_OK:   u8 = 0x04;
const STATUS_FAILED:      u8 = 0x80;

const FEATURE_BLK_FLUSH:        u32 = 1 << 9;
const FEATURE_BLK_DISCARD:      u32 = 1 << 13;
const FEATURE_BLK_WRITE_ZEROES: u32 = 1 << 14;

const VRING_DESC_F_NEXT:  u16 = 0x0001;
const VRING_DESC_F_WRITE: u16 = 0x0002;

// Request types (virtio-blk header 'type_' field)
const VIRTIO_BLK_T_IN:      u32 = 0; // device -> driver (read)
const VIRTIO_BLK_T_OUT:     u32 = 1; // driver -> device (write)
const VIRTIO_BLK_T_FLUSH:   u32 = 4;
const VIRTIO_BLK_T_DISCARD:      u32 = 11; // payload: discard segments, no data
const VIRTIO_BLK_T_WRITE_ZEROES: u32 = 13; // same segment payload as discard

// Request status byte written by the device
const VIRTIO_BLK_S_OK: u8 = 0;

/// Largest queue size we lay the ring out for (QEMU default is 256)
const MAX_QUEUE: usize = 256;

/// Max payload per request; larger transfers are chunked by the trait impl
pub const MAX_XFER_SECTORS: u32 = 128;
const BOUNCE_SIZE: usize = MAX_XFER_SECTORS as usize * 512;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr:  u64,
    len:   u32,
    flags: u16,
    next:  u16,
}

/// Ring storage: 3 pages cover desc/avail/used for queue sizes up to 256.
/// Offsets within it are computed from the device's queue size at init
#[repr(C, align(4096))]
struct RingMem([u8; 3 * 4096]);

/// DMA-reachable scratch: request header, status byte and the data bounce
/// buffer, all in one physically-contiguous allocation
#[repr(C, align(4096))]
struct DmaMem {
    data:   [u8; BOUNCE_SIZE],
    hdr:    [u8; 16],
    status: u8,
}

pub struct VirtioBlk {
    io_base:   u16,
    queue_num: usize,
    avail_off: usize,
    used_off:  usize,
    ring:      Box<RingMem>,
    dma:       Box<DmaMem>,
    avail_idx: u16,
    last_used: u16,
    capacity:  u64,
    has_flush: bool,
    /// VIRTIO_BLK_F_DISCARD negotiated; 'max_discard' is the per-request
    /// sector cap from device config (0 = device stated no limit)
    has_discard: bool,
    max_discard: u32,
    /// VIRTIO_BLK_F_WRITE_ZEROES negotiated, with its own sector cap
    has_wz: bool,
    max_wz: u32,
}

macro_rules! ior16 { ($base:expr, $off:expr) => { unsafe { Port::<u16>::new($base + $off).read() } } }
macro_rules! ior32 { ($base:expr, $off:expr) => { unsafe { Port::<u32>::new($base + $off).read() } } }
macro_rules! iow8  { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u8>::new($base + $off).write($v) } } }
macro_rules! iow16 { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u16>::new($base + $off).write($v) } } }
macro_rules! iow32 { ($base:expr, $off:expr, $v:expr) => { unsafe { Port::<u32>::new($base + $off).write($v) } } }

#[inline]
fn mfence() {
    unsafe { core::arch::asm!("mfence", options(nostack, nomem)); }
}

/// Scan the PCI bus for legacy/transitional virtio-blk functions.
/// 'pci::scan()' only collects network-class devices, so walk the bus here
pub fn find_devices(out: &mut [PciDevice; 4]) -> usize {
    let mut count = 0usize;
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = pci_read32(bus, dev, func, 0x00);
                if (id & 0xFFFF) as u16 == 0xFFFF {
                    if func == 0 { break; }
                    continue;
                }
                let vendor = (id & 0xFFFF) as u16;
                let device = (id >> 16) as u16;
                if vendor == VENDOR_VIRTIO && device == DEV_VIRTIO_BLK && count < 4 {
                    let mut bars = [0u32; 6];
                    for i in 0..6 {
                        bars[i] = pci_read32(bus, dev, func, 0x10 + (i as u8) * 4);
                    }
                    let class_rev = pci_read32(bus, dev, func, 0x08);
                    out[count] = PciDevice {
                        bus, dev, func,
                        vendor, device,
                        class: (class_rev >> 24) as u8,
                        subclass: (class_rev >> 16) as u8,
                        bars,
                        irq: pci_read8(bus, dev, func, 0x3C),
                    };
                    count += 1;
                }
                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 {
                    break;
                }
            }
        }
    }
    count
}

impl VirtioBlk {
    pub fn new(pci: &PciDevice) -> Option<Self> {
        pci.enable_bus_mastering();
        let io_base = pci.io_bar(0)?;

        // Reset, then announce ourselves
        iow8!(io_base, REG_DEVICE_STATUS, 0);
        for _ in 0..100_000 { core::hint::spin_loop(); }
        iow8!(io_base, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        let dev_features = ior32!(io_base, REG_DEVICE_FEATURES);
        let has_flush   = dev_features & FEATURE_BLK_FLUSH != 0;
        let has_discard = dev_features & FEATURE_BLK_DISCARD != 0;
        let has_wz      = dev_features & FEATURE_BLK_WRITE_ZEROES != 0;
        iow32!(io_base, REG_GUEST_FEATURES,
            dev_features & (FEATURE_BLK_FLUSH | FEATURE_BLK_DISCARD | FEATURE_BLK_WRITE_ZEROES));

        // Queue 0 is the only virtio-blk request queue
        iow16!(io_base, REG_QUEUE_SELECT, 0);
        let queue_num = ior16!(io_base, REG_QUEUE_SIZE) as usize;
        if queue_num == 0 || queue_num > MAX_QUEUE {
            crate::serial_println!("[virtio-blk] unsupported queue size {}", queue_num);
            iow8!(io_base, REG_DEVICE_STATUS, STATUS_FAILED);
            return None;
        }

        // Legacy ring layout, derived from the device's queue size:
        //   desc table  at 0          (16 * num)
        //   avail ring  right after   (6 + 2 * num)
        //   used ring   page-aligned  (6 + 8 * num)
        let avail_off = 16 * queue_num;
        let used_off  = (avail_off + 6 + 2 * queue_num + 4095) & !4095;

        let mut drv = VirtioBlk {
            io_base,
            queue_num,
            avail_off,
            used_off,
            ring: Box::new(RingMem([0u8; 3 * 4096])),
            dma:  Box::new(DmaMem { data: [0u8; BOUNCE_SIZE], hdr: [0u8; 16], status: 0 }),
            avail_idx: 0,
            last_used: 0,
            capacity:  0,
            has_flush,
            has_discard,
            max_discard: 0,
            has_wz,
            max_wz: 0,
        };

        let ring_phys = crate::net::virt_to_phys(drv.ring.0.as_ptr() as u64);
        iow32!(io_base, REG_QUEUE_ADDRESS, (ring_phys / 4096) as u32);

        iow8!(io_base, REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);

        // Capacity (in 512-byte sectors) from device config space
        let cap_lo = ior32!(io_base, REG_CONFIG_CAPACITY) as u64;
        let cap_hi = ior32!(io_base, REG_CONFIG_CAPACITY + 4) as u64;
        drv.capacity = cap_lo | (cap_hi << 32);

        // Per-request caps from the blk config block: max_discard_sectors at
        // offset 36, max_write_zeroes_sectors at offset 48
        if has_discard {
            drv.max_discard = ior32!(io_base, REG_CONFIG_CAPACITY + 36);
        }
        if has_wz {
            drv.max_wz = ior32!(io_base, REG_CONFIG_CAPACITY + 48);
        }

        crate::serial_println!(
            "[virtio-blk] io=0x{:04X} queue={} capacity={} sectors ({} MB) flush={} discard={} wz={}",
            io_base, queue_num, drv.capacity, drv.capacity * 512 / (1024 * 1024),
            has_flush, has_discard, has_wz
        );
        Some(drv)
    }

    #[inline]
    fn desc_table(&mut self) -> *mut Desc {
        self.ring.0.as_mut_ptr() as *mut Desc
    }

    /// Submit one 3-descriptor request (header / data / status) and poll the
    /// used ring until the device completes it
    fn request(&mut self, req_type: u32, sector: u64, data_len: usize) -> Result<(), BlkError> {
        let device_writes_data = req_type == VIRTIO_BLK_T_IN;

        self.dma.hdr[0..4].copy_from_slice(&req_type.to_le_bytes());
        self.dma.hdr[4..8].copy_from_slice(&0u32.to_le_bytes());
        self.dma.hdr[8..16].copy_from_slice(&sector.to_le_bytes());
        self.dma.status = 0xFF;

        let hdr_phys    = crate::net::virt_to_phys(self.dma.hdr.as_ptr() as u64);
        let data_phys   = crate::net::virt_to_phys(self.dma.data.as_ptr() as u64);
        let status_phys = crate::net::virt_to_phys((&self.dma.status as *const u8) as u64);

        let with_data = data_len > 0;
        unsafe {
            let desc = self.desc_table();
            *desc.add(0) = Desc {
                addr: hdr_phys, len: 16,
                flags: VRING_DESC_F_NEXT,
                next: if with_data { 1 } else { 2 },
            };
            if with_data {
                *desc.add(1) = Desc {
                    addr: data_phys, len: data_len as u32,
                    flags: VRING_DESC_F_NEXT
                        | if device_writes_data { VRING_DESC_F_WRITE } else { 0 },
                    next: 2,
                };
            }
            *desc.add(2) = Desc {
                addr: status_phys, len: 1,
                flags: VRING_DESC_F_WRITE,
                next: 0,
            };

            // avail.ring[avail_idx % num] = 0 (head of chain), then bump idx
            let avail = self.ring.0.as_mut_ptr().add(self.avail_off) as *mut u16;
            let slot = self.avail_idx as usize % self.queue_num;
            core::ptr::write_volatile(avail.add(2 + slot), 0u16);
            mfence();
            self.avail_idx = self.avail_idx.wrapping_add(1);
            core::ptr::write_volatile(avail.add(1), self.avail_idx);
            mfence();
        }

        iow16!(self.io_base, REG_QUEUE_NOTIFY, 0);

        // Poll the used ring; ~10^8 spins is generous for a local hypervisor
        let used_idx_ptr = unsafe {
            (self.ring.0.as_ptr().add(self.used_off) as *const u16).add(1)
        };
        let mut spins = 0u64;
        loop {
            let used = unsafe { core::ptr::read_volatile(used_idx_ptr) };
            if used != self.last_used {
                self.last_used = used;
                break;
            }
            super::io_relax(spins);
            spins += 1;
            if spins > 100_000_000 {
                return Err(BlkError::Timeout);
            }
        }

        let status = unsafe { core::ptr::read_volatile(&self.dma.status) };
        if status == VIRTIO_BLK_S_OK {
            Ok(())
        } else {
            Err(BlkError::DeviceFault)
        }
    }
}

/// Block-layer wrapper: virtio-blk uses one request queue and one bounce
/// buffer, so concurrent '&self' dispatch serializes on this internal mutex
pub struct VirtioBlockDev(SpinMutex<VirtioBlk>);

impl VirtioBlockDev {
    pub fn new(dev: VirtioBlk) -> Self {
        Self(SpinMutex::new(dev))
    }
}

impl BlockDriver for VirtioBlockDev {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        self.0.lock().bd_read_blocks(lba, count, buf)
    }
    fn write_blocks(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        self.0.lock().bd_write_blocks(lba, count, buf)
    }
    fn flush(&self) -> Result<(), BlkError> {
        self.0.lock().bd_flush()
    }
    fn discard(&self, lba: u64, count: u32) -> Result<(), BlkError> {
        self.0.lock().bd_discard(lba, count)
    }
    fn write_zeroes(&self, lba: u64, count: u32) -> Result<(), BlkError> {
        self.0.lock().bd_write_zeroes(lba, count)
    }
    fn info(&self) -> BlockDevInfo {
        self.0.lock().bd_info()
    }
}

impl VirtioBlk {
    fn bd_read_blocks(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            self.request(VIRTIO_BLK_T_IN, lba + done as u64, bytes)?;
            let off = done as usize * 512;
            buf[off..off + bytes].copy_from_slice(&self.dma.data[..bytes]);
            done += chunk;
        }
        Ok(())
    }

    fn bd_write_blocks(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let mut done = 0u32;
        while done < count {
            let chunk = (count - done).min(MAX_XFER_SECTORS);
            let bytes = chunk as usize * 512;
            let off = done as usize * 512;
            self.dma.data[..bytes].copy_from_slice(&buf[off..off + bytes]);
            self.request(VIRTIO_BLK_T_OUT, lba + done as u64, bytes)?;
            done += chunk;
        }
        Ok(())
    }

    fn bd_flush(&mut self) -> Result<(), BlkError> {
        if self.has_flush {
            self.request(VIRTIO_BLK_T_FLUSH, 0, 0)
        } else {
            Ok(())
        }
    }

    /// Discard: one 16-byte segment (sector, num_sectors, flags) per request
    /// in the data descriptor, capped at the device's max_discard_sectors
    fn bd_discard(&mut self, lba: u64, count: u32) -> Result<(), BlkError> {
        if !self.has_discard {
            return Err(BlkError::Unsupported);
        }
        let cap = if self.max_discard == 0 { u32::MAX } else { self.max_discard };
        let mut done = 0u32;
        while done < count {
            let n = (count - done).min(cap);
            let seg_lba = lba + done as u64;
            self.dma.data[0..8].copy_from_slice(&seg_lba.to_le_bytes());
            self.dma.data[8..12].copy_from_slice(&n.to_le_bytes());
            self.dma.data[12..16].copy_from_slice(&0u32.to_le_bytes());
            self.request(VIRTIO_BLK_T_DISCARD, 0, 16)?;
            done += n;
        }
        Ok(())
    }

    /// Write zeroes: same 16-byte segment as discard, request type 13.
    /// flags = 0 (no unmap hint), capped at the device's per-request limit
    fn bd_write_zeroes(&mut self, lba: u64, count: u32) -> Result<(), BlkError> {
        if !self.has_wz {
            return Err(BlkError::Unsupported);
        }
        let cap = if self.max_wz == 0 { u32::MAX } else { self.max_wz };
        let mut done = 0u32;
        while done < count {
            let n = (count - done).min(cap);
            let seg_lba = lba + done as u64;
            self.dma.data[0..8].copy_from_slice(&seg_lba.to_le_bytes());
            self.dma.data[8..12].copy_from_slice(&n.to_le_bytes());
            self.dma.data[12..16].copy_from_slice(&0u32.to_le_bytes());
            self.request(VIRTIO_BLK_T_WRITE_ZEROES, 0, 16)?;
            done += n;
        }
        Ok(())
    }

    fn bd_info(&mut self) -> BlockDevInfo {
        let mut out = BlockDevInfo::unknown();
        out.total_sectors = self.capacity;
        out.lba48 = true;
        let model = b"virtio-blk";
        out.model[..model.len()].copy_from_slice(model);
        out.model_len = model.len() as u8;
        out.discard = self.has_discard;
        out
    }
}
