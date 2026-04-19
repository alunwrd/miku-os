use core::hint::spin_loop;
use x86_64::instructions::port::Port;

const STATUS_BSY: u8 = 0x80;
const STATUS_DRQ: u8 = 0x08;
const STATUS_ERR: u8 = 0x01;
const STATUS_DF:  u8 = 0x20;

const CMD_READ_PIO:    u8 = 0x20;
const CMD_WRITE_PIO:   u8 = 0x30;
const CMD_CACHE_FLUSH: u8 = 0xE7;

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
}

impl AtaDrive {
    pub const EMPTY: Self = Self { base_port: 0, role: AtaRole::Master };

    pub const fn new(base_port: u16, role: AtaRole) -> Self {
        Self { base_port, role }
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
}
