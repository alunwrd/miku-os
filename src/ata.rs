use core::hint::spin_loop;
use x86_64::instructions::port::Port;

const STATUS_BSY: u8 = 0x80;
const STATUS_DRQ: u8 = 0x08;
const STATUS_ERR: u8 = 0x01;
const STATUS_DF:  u8 = 0x20;

const CMD_READ_PIO:      u8 = 0x20;
const CMD_WRITE_PIO:     u8 = 0x30;
const CMD_READ_PIO_EXT:  u8 = 0x24; // LBA48
const CMD_WRITE_PIO_EXT: u8 = 0x34; // LBA48
const CMD_CACHE_FLUSH:   u8 = 0xE7;
const CMD_CACHE_FLUSH_EXT: u8 = 0xEA; // LBA48
const CMD_IDENTIFY:      u8 = 0xEC;

/// Highest LBA addressable with 28-bit (LBA28) addressing
const LBA28_MAX: u64 = (1 << 28) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaError {
    DeviceFault,
    ErrorBitSet,
    BufferTooSmall,
    NoDevice,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaRole {
    Master,
    Slave,
}

pub struct AtaDrive {
    base_port: u16,
    role:      AtaRole,
    /// Bus-master DMA capability cache: 0 = unknown, 1 = usable, 2 = PIO only
    dma_state: u8,
}

impl AtaDrive {
    pub const EMPTY: Self = Self { base_port: 0, role: AtaRole::Master, dma_state: 0 };

    pub const fn new(base_port: u16, role: AtaRole) -> Self {
        Self { base_port, role, dma_state: 0 }
    }

    pub fn primary()         -> Self { Self::new(0x1F0, AtaRole::Master) }
    pub fn primary_slave()   -> Self { Self::new(0x1F0, AtaRole::Slave)  }
    pub fn secondary()       -> Self { Self::new(0x170, AtaRole::Master) }
    pub fn secondary_slave() -> Self { Self::new(0x170, AtaRole::Slave)  }

    pub fn from_idx(idx: usize) -> Self {
        match idx {
            0 => Self::primary(),
            1 => Self::primary_slave(),
            2 => Self::secondary(),
            _ => Self::secondary_slave(),
        }
    }

    /// Inverse of `from_idx`: a stable 0..=3 index used by the block layer as
    /// the device id, so registering the same physical drive is idempotent
    pub fn idx(&self) -> usize {
        match (self.base_port, self.role) {
            (0x1F0, AtaRole::Master) => 0,
            (0x1F0, AtaRole::Slave)  => 1,
            (0x170, AtaRole::Master) => 2,
            _                        => 3,
        }
    }

    #[inline]
    fn device_select_byte(&self, lba_top: u8) -> u8 {
        // bit7=1, bit6=1 (LBA), bit5=1, bit4=DEV, bits3-0=LBA27-24
        let dev = if self.role == AtaRole::Slave { 1u8 << 4 } else { 0 };
        0xE0 | dev | (lba_top & 0x0F)
    }

    #[inline]
    fn control_port(&self) -> u16 {
        // primary: 0x1F0 + 0x206 = 0x3F6
        // secondary: 0x170 + 0x206 = 0x376
        self.base_port + 0x206
    }

    #[inline]
    unsafe fn delay_400ns(&self) {
        let mut alt = Port::<u8>::new(self.control_port());
        for _ in 0..4 {
            let _ = alt.read();
        }
    }

    unsafe fn wait_not_busy(&self) -> Result<u8, AtaError> {
        let mut status_port = Port::<u8>::new(self.base_port + 7);
        for _ in 0..50_000 {
            let s = status_port.read();
            if s & STATUS_BSY == 0 {
                return Ok(s);
            }
            spin_loop();
        }
        Err(AtaError::Timeout)
    }

    unsafe fn wait_drq(&self) -> Result<(), AtaError> {
        let mut status_port = Port::<u8>::new(self.base_port + 7);
        for _ in 0..50_000 {
            let s = status_port.read();
            if s & STATUS_BSY == 0 {
                if s & STATUS_DF  != 0 { return Err(AtaError::DeviceFault); }
                if s & STATUS_ERR != 0 { return Err(AtaError::ErrorBitSet); }
                if s & STATUS_DRQ != 0 { return Ok(()); }
            }
            spin_loop();
        }
        Err(AtaError::Timeout)
    }

    #[inline]
    fn check_status(status: u8) -> Result<(), AtaError> {
        if status & STATUS_DF  != 0 { return Err(AtaError::DeviceFault); }
        if status & STATUS_ERR != 0 { return Err(AtaError::ErrorBitSet); }
        Ok(())
    }

    unsafe fn prepare_pio(&mut self, lba: u32, count: u8, cmd: u8) -> Result<(), AtaError> {
        let bp = self.base_port;
        
        if Port::<u8>::new(bp + 7).read() == 0xFF {
            return Err(AtaError::NoDevice);
        }

        Port::<u8>::new(self.control_port()).write(0x02);
        self.wait_not_busy()?;
        Port::<u8>::new(bp + 6).write(self.device_select_byte((lba >> 24) as u8));
        self.delay_400ns();
        Port::<u8>::new(bp + 2).write(count);
        Port::<u8>::new(bp + 3).write(lba as u8);
        Port::<u8>::new(bp + 4).write((lba >>  8) as u8);
        Port::<u8>::new(bp + 5).write((lba >> 16) as u8);
        Port::<u8>::new(bp + 7).write(cmd);

        self.delay_400ns();
        Ok(())
    }

    pub fn read_sector(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), AtaError> {
        if self.base_port == 0 { return Err(AtaError::NoDevice); }
        if buf.len() < 512     { return Err(AtaError::BufferTooSmall); }
        unsafe {
            self.prepare_pio(lba, 1, CMD_READ_PIO)?;
            self.wait_drq()?;
            self.pio_read_words(buf, 0, 256);
            Port::<u8>::new(self.control_port()).write(0x00);
        }
        Ok(())
    }

    pub fn write_sector(&mut self, lba: u32, data: &[u8]) -> Result<(), AtaError> {
        if self.base_port == 0 { return Err(AtaError::NoDevice); }
        if data.len() < 512    { return Err(AtaError::BufferTooSmall); }
        unsafe {
            self.prepare_pio(lba, 1, CMD_WRITE_PIO)?;
            self.wait_drq()?;
            self.pio_write_words(data, 0, 256);
            let status = self.wait_not_busy()?;
            Port::<u8>::new(self.control_port()).write(0x00);
            Self::check_status(status)?;
        }
        Ok(())
    }

    pub fn read_sectors(&mut self, lba: u32, buf: &mut [u8], count: u8) -> Result<(), AtaError> {
        if self.base_port == 0 { return Err(AtaError::NoDevice); }
        if count == 0          { return Ok(()); }
        if buf.len() < count as usize * 512 { return Err(AtaError::BufferTooSmall); }

        if count == 1 {
            return self.read_sector(lba, buf);
        }

        unsafe {
            self.prepare_pio(lba, count, CMD_READ_PIO)?;
            for s in 0..count as usize {
                self.wait_drq()?;
                self.pio_read_words(buf, s * 512, 256);
            }
            Port::<u8>::new(self.control_port()).write(0x00);
        }
        Ok(())
    }

    pub fn write_sectors(&mut self, lba: u32, buf: &[u8], count: u8) -> Result<(), AtaError> {
        if self.base_port == 0 { return Err(AtaError::NoDevice); }
        if count == 0          { return Ok(()); }
        if buf.len() < count as usize * 512 { return Err(AtaError::BufferTooSmall); }

        if count == 1 {
            return self.write_sector(lba, buf);
        }

        unsafe {
            self.prepare_pio(lba, count, CMD_WRITE_PIO)?;
            for s in 0..count as usize {
                self.wait_drq()?;
                self.pio_write_words(buf, s * 512, 256);
            }
            let status = self.wait_not_busy()?;
            Port::<u8>::new(self.control_port()).write(0x00);
            Self::check_status(status)?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), AtaError> {
        if self.base_port == 0 { return Ok(()); }

        unsafe {
            let bp = self.base_port;

            if Port::<u8>::new(bp + 7).read() == 0xFF {
                return Err(AtaError::NoDevice);
            }

            Port::<u8>::new(self.control_port()).write(0x02);
            self.wait_not_busy()?;
            Port::<u8>::new(bp + 6).write(self.device_select_byte(0));
            self.delay_400ns();
            Port::<u8>::new(bp + 7).write(CMD_CACHE_FLUSH);
            self.delay_400ns();

            let status = self.wait_not_busy()?;
            Port::<u8>::new(self.control_port()).write(0x00);
            Self::check_status(status)?;
        }
        Ok(())
    }

    #[inline]
    unsafe fn pio_read_words(&self, buf: &mut [u8], offset: usize, words: usize) {
        let mut data_port = Port::<u16>::new(self.base_port);
        for w in 0..words {
            let word = data_port.read();
            buf[offset + w * 2]     = word as u8;
            buf[offset + w * 2 + 1] = (word >> 8) as u8;
        }
    }

    #[inline]
    unsafe fn pio_write_words(&self, buf: &[u8], offset: usize, words: usize) {
        let mut data_port = Port::<u16>::new(self.base_port);
        for w in 0..words {
            let word = (buf[offset + w * 2] as u16)
                     | ((buf[offset + w * 2 + 1] as u16) << 8);
            data_port.write(word);
        }
    }

    //                              LBA48 
    // Program the task-file registers for a 48-bit LBA PIO transfer. The high
    // bytes are written first, then the low bytes, into the same registers
    // (the controller keeps a 2-deep FIFO per register)
    unsafe fn prepare_pio48(&mut self, lba: u64, count: u16, cmd: u8) -> Result<(), AtaError> {
        let bp = self.base_port;

        if Port::<u8>::new(bp + 7).read() == 0xFF {
            return Err(AtaError::NoDevice);
        }

        Port::<u8>::new(self.control_port()).write(0x02);
        self.wait_not_busy()?;

        // Device register: LBA mode, master/slave; LBA bits live in the LBA regs
        let dev = if self.role == AtaRole::Slave { 1u8 << 4 } else { 0 };
        Port::<u8>::new(bp + 6).write(0x40 | dev);
        self.delay_400ns();

        // High bytes first
        Port::<u8>::new(bp + 2).write((count >> 8) as u8);
        Port::<u8>::new(bp + 3).write((lba >> 24) as u8);
        Port::<u8>::new(bp + 4).write((lba >> 32) as u8);
        Port::<u8>::new(bp + 5).write((lba >> 40) as u8);
        // Low bytes
        Port::<u8>::new(bp + 2).write((count & 0xFF) as u8);
        Port::<u8>::new(bp + 3).write(lba as u8);
        Port::<u8>::new(bp + 4).write((lba >> 8) as u8);
        Port::<u8>::new(bp + 5).write((lba >> 16) as u8);

        Port::<u8>::new(bp + 7).write(cmd);
        self.delay_400ns();
        Ok(())
    }

    /// Read `count` sectors (1..=65536, 0 means 65536) using LBA48 PIO
    pub fn read_sectors_ext(&mut self, lba: u64, buf: &mut [u8], count: u16) -> Result<(), AtaError> {
        if self.base_port == 0 { return Err(AtaError::NoDevice); }
        let n = if count == 0 { 65536usize } else { count as usize };
        if buf.len() < n * 512 { return Err(AtaError::BufferTooSmall); }

        unsafe {
            self.prepare_pio48(lba, count, CMD_READ_PIO_EXT)?;
            for s in 0..n {
                self.wait_drq()?;
                self.pio_read_words(buf, s * 512, 256);
            }
            Port::<u8>::new(self.control_port()).write(0x00);
        }
        Ok(())
    }

    /// Write `count` sectors (1..=65536, 0 means 65536) using LBA48 PIO
    pub fn write_sectors_ext(&mut self, lba: u64, buf: &[u8], count: u16) -> Result<(), AtaError> {
        if self.base_port == 0 { return Err(AtaError::NoDevice); }
        let n = if count == 0 { 65536usize } else { count as usize };
        if buf.len() < n * 512 { return Err(AtaError::BufferTooSmall); }

        unsafe {
            self.prepare_pio48(lba, count, CMD_WRITE_PIO_EXT)?;
            for s in 0..n {
                self.wait_drq()?;
                self.pio_write_words(buf, s * 512, 256);
            }
            let status = self.wait_not_busy()?;
            Port::<u8>::new(self.control_port()).write(0x00);
            Self::check_status(status)?;
        }
        Ok(())
    }

    pub fn flush_ext(&mut self) -> Result<(), AtaError> {
        if self.base_port == 0 { return Ok(()); }
        unsafe {
            let bp = self.base_port;
            if Port::<u8>::new(bp + 7).read() == 0xFF {
                return Err(AtaError::NoDevice);
            }
            Port::<u8>::new(self.control_port()).write(0x02);
            self.wait_not_busy()?;
            let dev = if self.role == AtaRole::Slave { 1u8 << 4 } else { 0 };
            Port::<u8>::new(bp + 6).write(0x40 | dev);
            self.delay_400ns();
            Port::<u8>::new(bp + 7).write(CMD_CACHE_FLUSH_EXT);
            self.delay_400ns();
            let status = self.wait_not_busy()?;
            Port::<u8>::new(self.control_port()).write(0x00);
            Self::check_status(status)?;
        }
        Ok(())
    }

    /// Issue IDENTIFY DEVICE and return the raw 256-word response
    pub fn identify(&mut self) -> Result<[u16; 256], AtaError> {
        if self.base_port == 0 { return Err(AtaError::NoDevice); }
        let bp = self.base_port;
        unsafe {
            if Port::<u8>::new(bp + 7).read() == 0xFF {
                return Err(AtaError::NoDevice);
            }
            Port::<u8>::new(self.control_port()).write(0x02);
            self.wait_not_busy()?;
            let dev = if self.role == AtaRole::Slave { 1u8 << 4 } else { 0 };
            Port::<u8>::new(bp + 6).write(0xA0 | dev);
            self.delay_400ns();
            // Zero the LBA/count registers, then send IDENTIFY
            Port::<u8>::new(bp + 2).write(0);
            Port::<u8>::new(bp + 3).write(0);
            Port::<u8>::new(bp + 4).write(0);
            Port::<u8>::new(bp + 5).write(0);
            Port::<u8>::new(bp + 7).write(CMD_IDENTIFY);
            self.delay_400ns();

            // Status 0 => no device
            if Port::<u8>::new(bp + 7).read() == 0 {
                return Err(AtaError::NoDevice);
            }
            self.wait_drq()?;

            let mut words = [0u16; 256];
            let mut data_port = Port::<u16>::new(bp);
            for w in words.iter_mut() {
                *w = data_port.read();
            }
            Port::<u8>::new(self.control_port()).write(0x00);
            Ok(words)
        }
    }
}

// Bus-master IDE DMA
//
// PCI IDE controllers expose a Bus Master register block via BAR4: 8 bytes per
// channel (command, status, PRDT pointer). One PRD entry covering a 64 KiB
// bounce buffer moves up to 128 sectors per command, replacing the per-word
// port I/O of PIO; under KVM that is thousands of VM exits saved per request.
// Completion is detected via the IRQ14/15 flags ('interrupts::ATA_*_IRQ')
// backed up by polling the BM status register, so it works with or without
// interrupts enabled

extern crate alloc;
use alloc::boxed::Box;
use spin::Mutex;

const CMD_READ_DMA:      u8 = 0xC8;
const CMD_WRITE_DMA:     u8 = 0xCA;
const CMD_READ_DMA_EXT:  u8 = 0x25;
const CMD_WRITE_DMA_EXT: u8 = 0x35;

const BM_CMD_START: u8 = 0x01;
const BM_CMD_READ:  u8 = 0x08; // direction: device -> memory
const BM_ST_ACTIVE: u8 = 0x01;
const BM_ST_ERROR:  u8 = 0x02;
const BM_ST_IRQ:    u8 = 0x04;

/// One PRD entry = one 64 KiB buffer = 128 sectors per DMA command
pub const DMA_MAX_SECTORS: u32 = 128;
const BOUNCE_BYTES: usize = DMA_MAX_SECTORS as usize * 512;

/// 64 KiB-aligned so the bounce never crosses a 64 KiB boundary (a PRD entry  must not), with the 8-byte PRDT right behind it
#[repr(C, align(65536))]
struct ChannelDma {
    bounce: [u8; BOUNCE_BYTES],
    prdt:   [u8; 8],
}

struct BmChannel {
    bm_base: u16,
    mem:     Box<ChannelDma>,
}

static BM_PRIMARY:   Mutex<Option<BmChannel>> = Mutex::new(None);
static BM_SECONDARY: Mutex<Option<BmChannel>> = Mutex::new(None);

fn bm_channel(base_port: u16) -> &'static Mutex<Option<BmChannel>> {
    if base_port == 0x1F0 { &BM_PRIMARY } else { &BM_SECONDARY }
}

fn irq_flag(base_port: u16) -> &'static core::sync::atomic::AtomicBool {
    if base_port == 0x1F0 {
        &crate::interrupts::ATA_PRIMARY_IRQ
    } else {
        &crate::interrupts::ATA_SECONDARY_IRQ
    }
}

/// Locate the PCI IDE controller, enable bus mastering and allocate the
/// per-channel DMA contexts. Called once from 'block::probe()'; ATA falls
/// back to PIO when nothing is found
pub fn dma_init() {
    use crate::net::pci::{pci_read32, pci_read8};

    let mut bm_base: Option<u16> = None;
    'scan: for bus in 0..=255u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = pci_read32(bus, dev, func, 0x00);
                if (id & 0xFFFF) as u16 == 0xFFFF {
                    if func == 0 { break; }
                    continue;
                }
                let class_rev = pci_read32(bus, dev, func, 0x08);
                let class    = (class_rev >> 24) as u8;
                let subclass = (class_rev >> 16) as u8;
                if class == 0x01 && subclass == 0x01 {
                    let bar4 = pci_read32(bus, dev, func, 0x10 + 4 * 4);
                    if bar4 & 1 != 0 {
                        // Enable I/O + memory + bus mastering
                        let cmd_off = 0x04u8;
                        let cmd = pci_read32(bus, dev, func, cmd_off);
                        unsafe {
                            Port::<u32>::new(crate::net::pci::PCI_ADDR)
                                .write(crate::net::pci::pci_addr(bus, dev, func, cmd_off));
                            Port::<u32>::new(crate::net::pci::PCI_DATA)
                                .write(cmd | 0x0007);
                        }
                        bm_base = Some((bar4 & !3) as u16);
                        crate::serial_println!(
                            "[ata] IDE controller {:02x}:{:02x}.{} bus-master base=0x{:04X}",
                            bus, dev, func, bm_base.unwrap()
                        );
                        break 'scan;
                    }
                }
                if func == 0 && (pci_read8(bus, dev, func, 0x0E) & 0x80) == 0 {
                    break;
                }
            }
        }
    }

    let Some(base) = bm_base else {
        crate::serial_println!("[ata] no bus-master IDE controller - PIO only");
        return;
    };

    let mk = || Box::new(ChannelDma { bounce: [0u8; BOUNCE_BYTES], prdt: [0u8; 8] });
    *BM_PRIMARY.lock()   = Some(BmChannel { bm_base: base,     mem: mk() });
    *BM_SECONDARY.lock() = Some(BmChannel { bm_base: base + 8, mem: mk() });
    crate::serial_println!("[ata] DMA channels ready (primary/secondary, 64 KiB bounce each)");
}

impl AtaDrive {
    /// Program the task file for a DMA command with device interrupts enabled
    /// (nIEN=0) so the bus-master IRQ status latches on completion
    unsafe fn prepare_dma_taskfile(&mut self, lba: u64, count: u16, lba48: bool, cmd: u8)
        -> Result<(), AtaError>
    {
        let bp = self.base_port;
        if Port::<u8>::new(bp + 7).read() == 0xFF {
            return Err(AtaError::NoDevice);
        }

        Port::<u8>::new(self.control_port()).write(0x00);
        self.wait_not_busy()?;

        if lba48 {
            let dev = if self.role == AtaRole::Slave { 1u8 << 4 } else { 0 };
            Port::<u8>::new(bp + 6).write(0x40 | dev);
            self.delay_400ns();
            Port::<u8>::new(bp + 2).write((count >> 8) as u8);
            Port::<u8>::new(bp + 3).write((lba >> 24) as u8);
            Port::<u8>::new(bp + 4).write((lba >> 32) as u8);
            Port::<u8>::new(bp + 5).write((lba >> 40) as u8);
            Port::<u8>::new(bp + 2).write((count & 0xFF) as u8);
            Port::<u8>::new(bp + 3).write(lba as u8);
            Port::<u8>::new(bp + 4).write((lba >> 8) as u8);
            Port::<u8>::new(bp + 5).write((lba >> 16) as u8);
        } else {
            Port::<u8>::new(bp + 6).write(self.device_select_byte((lba >> 24) as u8));
            self.delay_400ns();
            Port::<u8>::new(bp + 2).write(count as u8);
            Port::<u8>::new(bp + 3).write(lba as u8);
            Port::<u8>::new(bp + 4).write((lba >> 8) as u8);
            Port::<u8>::new(bp + 5).write((lba >> 16) as u8);
        }

        Port::<u8>::new(bp + 7).write(cmd);
        self.delay_400ns();
        Ok(())
    }

    /// One bus-master DMA transfer of up to 'DMA_MAX_SECTORS' sectors through the channel bounce buffer
    fn dma_transfer(&mut self, lba: u64, sectors: u32, write: bool, buf_in: &[u8], buf_out: &mut [u8])
        -> Result<(), AtaError>
    {
        debug_assert!(sectors >= 1 && sectors <= DMA_MAX_SECTORS);
        let bytes = sectors as usize * 512;

        let mut guard = bm_channel(self.base_port).lock();
        let ch = guard.as_mut().ok_or(AtaError::NoDevice)?;
        let bm = ch.bm_base;

        if write {
            ch.mem.bounce[..bytes].copy_from_slice(&buf_in[..bytes]);
        }

        // PRDT must live below 4 GiB for the 32-bit bus-master engine
        let bounce_phys = crate::net::virt_to_phys(ch.mem.bounce.as_ptr() as u64);
        let prdt_phys   = crate::net::virt_to_phys(ch.mem.prdt.as_ptr() as u64);
        if bounce_phys + bytes as u64 > u32::MAX as u64 || prdt_phys > u32::MAX as u64 {
            return Err(AtaError::DeviceFault);
        }

        let count_field: u16 = if bytes == 65536 { 0 } else { bytes as u16 };
        ch.mem.prdt[0..4].copy_from_slice(&(bounce_phys as u32).to_le_bytes());
        ch.mem.prdt[4..6].copy_from_slice(&count_field.to_le_bytes());
        ch.mem.prdt[6..8].copy_from_slice(&0x8000u16.to_le_bytes()); // EOT

        let lba48 = lba + sectors as u64 - 1 > LBA28_MAX || sectors > 256;
        let cmd = match (write, lba48) {
            (false, false) => CMD_READ_DMA,
            (false, true)  => CMD_READ_DMA_EXT,
            (true,  false) => CMD_WRITE_DMA,
            (true,  true)  => CMD_WRITE_DMA_EXT,
        };

        unsafe {
            // Reset the engine, point it at the PRDT, clear stale status
            Port::<u8>::new(bm).write(0);
            Port::<u32>::new(bm + 4).write(prdt_phys as u32);
            Port::<u8>::new(bm + 2).write(BM_ST_ERROR | BM_ST_IRQ);

            irq_flag(self.base_port).store(false, core::sync::atomic::Ordering::Release);

            self.prepare_dma_taskfile(lba, sectors as u16, lba48, cmd)?;

            let dir = if write { 0 } else { BM_CMD_READ };
            Port::<u8>::new(bm).write(BM_CMD_START | dir);

            // Wait for completion: IRQ flag (vector 0x2E/0x2F), BM IRQ status  bit, or the engine going idle - whichever lands first
            let mut status_port = Port::<u8>::new(bm + 2);
            let mut spins = 0u64;
            let ok = loop {
                let st = status_port.read();
                if st & BM_ST_ERROR != 0 {
                    break false;
                }
                let done = irq_flag(self.base_port).load(core::sync::atomic::Ordering::Acquire)
                    || st & BM_ST_IRQ != 0
                    || st & BM_ST_ACTIVE == 0;
                if done {
                    break true;
                }
                crate::block::io_relax(spins);
                spins += 1;
                if spins > 200_000_000 {
                    break false;
                }
            };

            // Stop the engine and clear latched status either way
            Port::<u8>::new(bm).write(0);
            Port::<u8>::new(bm + 2).write(BM_ST_ERROR | BM_ST_IRQ);

            if !ok {
                return Err(AtaError::Timeout);
            }

            let status = self.wait_not_busy()?;
            Self::check_status(status)?;
        }

        if !write {
            buf_out[..bytes].copy_from_slice(&ch.mem.bounce[..bytes]);
        }
        Ok(())
    }

    /// Resolve (once) whether this drive can use bus-master DMA: the
    /// controller must expose a BM block and IDENTIFY word 49 bit 8 must be
    /// set.
    fn dma_capable(&mut self) -> bool {
        if self.dma_state == 0 {
            let chan = bm_channel(self.base_port).lock().is_some();
            let drive = match self.identify() {
                Ok(w) => w[49] & (1 << 8) != 0,
                Err(_) => false,
            };
            self.dma_state = if chan && drive { 1 } else { 2 };
            crate::serial_println!(
                "[ata] drive {}: {}",
                self.idx(),
                if self.dma_state == 1 { "bus-master DMA enabled" } else { "PIO mode" }
            );
        }
        self.dma_state == 1
    }
}

// Block layer integration

use crate::block::driver::{BlkError, BlockDevInfo, BlockDriver};

impl From<AtaError> for BlkError {
    fn from(e: AtaError) -> Self {
        match e {
            AtaError::NoDevice       => BlkError::NoDevice,
            AtaError::BufferTooSmall => BlkError::BufferTooSmall,
            AtaError::DeviceFault    => BlkError::DeviceFault,
            AtaError::Timeout        => BlkError::Timeout,
            AtaError::ErrorBitSet    => BlkError::DeviceFault,
        }
    }
}

impl BlockDriver for AtaDrive {
    fn read_blocks(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let mut use_dma = self.dma_capable();
        let mut done = 0u32;
        while done < count {
            let cur_lba = lba + done as u64;
            let remaining = count - done;
            let off = done as usize * 512;

            if use_dma {
                let chunk = remaining.min(DMA_MAX_SECTORS);
                match self.dma_transfer(cur_lba, chunk, false, &[], &mut buf[off..off + chunk as usize * 512]) {
                    Ok(()) => { done += chunk; continue; }
                    Err(e) => {
                        crate::serial_println!("[ata] DMA read failed ({:?}) - falling back to PIO", e);
                        self.dma_state = 2;
                        use_dma = false;
                    }
                }
            }

            // PIO path: LBA48 past the 28-bit limit, otherwise LBA28 capped
            // at 256 sectors per command
            if cur_lba > LBA28_MAX || remaining > 256 {
                let chunk = remaining.min(32) as u16; // keep PIO bursts modest
                self.read_sectors_ext(cur_lba, &mut buf[off..off + chunk as usize * 512], chunk)?;
                done += chunk as u32;
            } else {
                let chunk = remaining.min(256);
                let cnt8 = if chunk == 256 { 0u8 } else { chunk as u8 };
                self.read_sectors(cur_lba as u32, &mut buf[off..off + chunk as usize * 512], cnt8)?;
                done += chunk;
            }
        }
        Ok(())
    }

    fn write_blocks(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
        if (count as usize) * 512 > buf.len() {
            return Err(BlkError::BufferTooSmall);
        }
        let mut use_dma = self.dma_capable();
        let mut done = 0u32;
        while done < count {
            let cur_lba = lba + done as u64;
            let remaining = count - done;
            let off = done as usize * 512;

            if use_dma {
                let chunk = remaining.min(DMA_MAX_SECTORS);
                match self.dma_transfer(cur_lba, chunk, true, &buf[off..off + chunk as usize * 512], &mut []) {
                    Ok(()) => { done += chunk; continue; }
                    Err(e) => {
                        crate::serial_println!("[ata] DMA write failed ({:?}) - falling back to PIO", e);
                        self.dma_state = 2;
                        use_dma = false;
                    }
                }
            }

            if cur_lba > LBA28_MAX || remaining > 256 {
                let chunk = remaining.min(32) as u16;
                self.write_sectors_ext(cur_lba, &buf[off..off + chunk as usize * 512], chunk)?;
                done += chunk as u32;
            } else {
                let chunk = remaining.min(256);
                let cnt8 = if chunk == 256 { 0u8 } else { chunk as u8 };
                self.write_sectors(cur_lba as u32, &buf[off..off + chunk as usize * 512], cnt8)?;
                done += chunk;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), BlkError> {
        AtaDrive::flush(self).map_err(Into::into)
    }

    fn info(&mut self) -> BlockDevInfo {
        let mut out = BlockDevInfo::unknown();
        if let Ok(words) = self.identify() {
            out.lba48 = words[83] & (1 << 10) != 0;
            let lba28 = (words[60] as u64) | ((words[61] as u64) << 16);
            let lba48 = (words[100] as u64)
                | ((words[101] as u64) << 16)
                | ((words[102] as u64) << 32)
                | ((words[103] as u64) << 48);
            out.total_sectors = if out.lba48 && lba48 != 0 { lba48 } else { lba28 };

            // Model string lives in words 27..=46, ASCII, byte-swapped per word
            let mut n = 0usize;
            for i in 27..=46 {
                let w = words[i];
                let hi = (w >> 8) as u8;
                let lo = (w & 0xFF) as u8;
                if n < out.model.len() { out.model[n] = hi; n += 1; }
                if n < out.model.len() { out.model[n] = lo; n += 1; }
            }
            // Trim trailing spaces / NULs
            while n > 0 && (out.model[n - 1] == b' ' || out.model[n - 1] == 0) {
                n -= 1;
            }
            out.model_len = n as u8;
        }
        out
    }
}
