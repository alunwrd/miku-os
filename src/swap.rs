use spin::Mutex;
use crate::block::driver::BlkError;
use crate::pmm;
use crate::vfs::types::BlockDevId;

const SWAP_PAGE_SIZE: u32    = 4096;
const SWAP_SECS_PER_PAGE: u32 = SWAP_PAGE_SIZE / 512;
const SWAP_MAGIC: &[u8; 10]  = b"SWAPSPACE2";
const SWAP_MAX_PAGES: usize  = 65536;
const SWAP_BITMAP_WORDS: usize = SWAP_MAX_PAGES / 64;

pub struct SwapHeader {
    pub version:     u32,
    pub last_page:   u32,
    pub nr_badpages: u32,
    pub uuid:        [u8; 16],
    pub label:       [u8; 16],
}

struct SwapState {
    pub active:        bool,
    pub dev_id:        BlockDevId,
    pub partition_lba: u32,
    pub total_pages:   u32,
    pub used_pages:    u32,
    bitmap:            [u64; SWAP_BITMAP_WORDS],
}

impl SwapState {
    const fn new() -> Self {
        Self {
            active:        false,
            dev_id:        0,
            partition_lba: 0,
            total_pages:   0,
            used_pages:    0,
            bitmap:        [0u64; SWAP_BITMAP_WORDS],
        }
    }

    fn alloc_slot(&mut self) -> Option<u32> {
        for word_idx in 0..SWAP_BITMAP_WORDS {
            let w = self.bitmap[word_idx];
            if w != u64::MAX {
                let bit = w.trailing_ones() as usize;
                let slot = word_idx * 64 + bit;
                if slot < self.total_pages as usize {
                    self.bitmap[word_idx] |= 1 << bit;
                    self.used_pages += 1;
                    return Some(slot as u32 + 1);
                }
            }
        }
        None
    }

    fn free_slot(&mut self, slot: u32) {
        if slot == 0 || slot > self.total_pages { return; }
        let idx = (slot as usize - 1) / 64;
        let bit = (slot as usize - 1) % 64;
        if self.bitmap[idx] & (1 << bit) != 0 {
            self.bitmap[idx] &= !(1 << bit);
            self.used_pages = self.used_pages.saturating_sub(1);
        }
    }

    fn is_used(&self, slot: u32) -> bool {
        if slot == 0 || slot > self.total_pages { return false; }
        let idx = (slot as usize - 1) / 64;
        let bit = (slot as usize - 1) % 64;
        self.bitmap[idx] & (1 << bit) != 0
    }
}

static SWAP: Mutex<SwapState> = Mutex::new(SwapState::new());

pub fn swap_total_pages() -> u32  { SWAP.lock().total_pages }
pub fn swap_used_pages()  -> u32  { SWAP.lock().used_pages }
pub fn swap_is_active()   -> bool { SWAP.lock().active }
pub fn swap_dev_id()      -> BlockDevId { SWAP.lock().dev_id }
pub fn swap_partition_lba() -> u32 { SWAP.lock().partition_lba }

pub fn swap_free_pages() -> u32 {
    let s = SWAP.lock();
    s.total_pages.saturating_sub(s.used_pages)
}

pub fn swap_total_kb() -> u32 { swap_total_pages() * (SWAP_PAGE_SIZE / 1024) }
pub fn swap_used_kb()  -> u32 { swap_used_pages()  * (SWAP_PAGE_SIZE / 1024) }
pub fn swap_free_kb()  -> u32 { swap_free_pages()  * (SWAP_PAGE_SIZE / 1024) }

fn read_page(dev: BlockDevId, lba_base: u32, dst: *mut u8) -> Result<(), BlkError> {
    let buf = unsafe {
        core::slice::from_raw_parts_mut(dst, SWAP_PAGE_SIZE as usize)
    };
    crate::block::read(dev, lba_base as u64, SWAP_SECS_PER_PAGE, buf)
}

fn write_page(dev: BlockDevId, lba_base: u32, src: *const u8) -> Result<(), BlkError> {
    let buf = unsafe {
        core::slice::from_raw_parts(src, SWAP_PAGE_SIZE as usize)
    };
    crate::block::write(dev, lba_base as u64, SWAP_SECS_PER_PAGE, buf)
}

fn phys_to_virt(phys: u64) -> u64 {
    let hhdm = crate::net::HHDM_OFFSET.load(core::sync::atomic::Ordering::Relaxed);
    phys + hhdm
}

fn lba_for_slot(slot: u32) -> u32 {
    SWAP.lock().partition_lba + slot * SWAP_SECS_PER_PAGE
}

pub fn swap_out_internal(phys_addr: u64) -> Result<u32, SwapError> {
    let (slot, dev) = {
        let mut s = SWAP.lock();
        if !s.active { return Err(SwapError::NotActive); }
        let slot = s.alloc_slot().ok_or(SwapError::NoSpace)?;
        (slot, s.dev_id)
    };

    let lba_base = lba_for_slot(slot);
    write_page(dev, lba_base, phys_to_virt(phys_addr) as *const u8)
        .map_err(SwapError::Io)?;

    crate::serial_println!("[swap] swap_out: phys={:#x} -> slot={}", phys_addr, slot);
    Ok(slot)
}

pub fn swap_in_internal(slot: u32, phys_addr: u64) -> Result<(), SwapError> {
    let dev = {
        let s = SWAP.lock();
        if !s.active { return Err(SwapError::NotActive); }
        if !s.is_used(slot) { return Err(SwapError::InvalidSlot); }
        s.dev_id
    };

    let lba_base = lba_for_slot(slot);
    read_page(dev, lba_base, phys_to_virt(phys_addr) as *mut u8)
        .map_err(SwapError::Io)?;

    SWAP.lock().free_slot(slot);
    crate::serial_println!("[swap] swap_in: slot={} -> phys={:#x}", slot, phys_addr);
    Ok(())
}

pub fn swap_out(phys_addr: u64) -> Result<u32, SwapError> {
    swap_out_internal(phys_addr)
}

pub fn swap_in(slot: u32, phys_addr: u64) -> Result<(), SwapError> {
    swap_in_internal(slot, phys_addr)
}

pub fn free_swap_slot(slot: u32) {
    if slot == 0 { return; }
	SWAP.lock().free_slot(slot);
}

pub fn try_reclaim_page() -> Option<u64> {
    crate::swap_map::alloc_or_evict()
}

pub fn mkswap(
    dev: BlockDevId,
    partition_lba: u32,
    partition_sectors: u32,
    label: &str,
) -> Result<(), BlkError> {
    let total_pages = partition_sectors / SWAP_SECS_PER_PAGE;
    if total_pages < 10 {
        crate::serial_println!("[swap] partition too small: {} pages", total_pages);
        return Err(BlkError::DeviceFault);
    }

    let mut page0 = [0u8; 4096];

    page0[0..4].copy_from_slice(&1u32.to_le_bytes());
    let last_page = total_pages - 1;
    page0[4..8].copy_from_slice(&last_page.to_le_bytes());
    page0[8..12].copy_from_slice(&0u32.to_le_bytes());

    let uuid = guid_pseudo_swap(partition_lba);
    page0[12..28].copy_from_slice(&uuid);

    let lbytes = label.as_bytes();
    let llen = lbytes.len().min(15);
    page0[28..28 + llen].copy_from_slice(&lbytes[..llen]);

    page0[4086..4096].copy_from_slice(SWAP_MAGIC);

    write_page(dev, partition_lba, page0.as_ptr())?;
    crate::block::flush(dev)?;

    crate::serial_println!("[swap] mkswap: lba={} pages={} label='{}'", partition_lba, total_pages, label);
    Ok(())
}

pub fn swapon(
    dev: BlockDevId,
    partition_lba: u32,
    partition_sectors: u32,
) -> Result<u32, SwapError> {
    if SWAP.lock().active {
        return Err(SwapError::AlreadyActive);
    }

    let mut page0 = [0u8; 4096];
    read_page(dev, partition_lba, page0.as_mut_ptr())
        .map_err(SwapError::Io)?;

    if &page0[4086..4096] != SWAP_MAGIC {
        return Err(SwapError::InvalidMagic);
    }

    let version   = u32::from_le_bytes(page0[0..4].try_into().unwrap_or([0; 4]));
    let last_page = u32::from_le_bytes(page0[4..8].try_into().unwrap_or([0; 4]));

    if version != 1 {
        return Err(SwapError::UnsupportedVersion);
    }

    let total_pages = last_page + 1;

    {
        let mut s = SWAP.lock();
        s.active        = true;
        s.dev_id        = dev;
        s.partition_lba = partition_lba;
        s.total_pages   = total_pages.saturating_sub(1).min(SWAP_MAX_PAGES as u32);
        s.used_pages    = 0;
        s.bitmap        = [0u64; SWAP_BITMAP_WORDS];
    }

    crate::serial_println!(
        "[swap] swapon: lba={} pages={} ({} MB)",
        partition_lba, total_pages, total_pages * SWAP_PAGE_SIZE / (1024 * 1024)
    );

    crate::pmm::refill_emergency_pool();
    crate::serial_println!(
        "[swap] emergency pool filled: {} frames ready",
        crate::pmm::emergency_frames_available()
    );

    Ok(total_pages)
}

pub fn swapoff() -> Result<(), SwapError> {
    let mut s = SWAP.lock();
    if !s.active {
        return Err(SwapError::NotActive);
    }
    if s.used_pages > 0 {
        return Err(SwapError::SwapInUse);
    }
    s.active        = false;
    s.total_pages   = 0;
    s.used_pages    = 0;
    s.partition_lba = 0;
    s.bitmap        = [0u64; SWAP_BITMAP_WORDS];
    crate::serial_println!("[swap] swapoff ok");
    Ok(())
}

fn guid_pseudo_swap(seed: u32) -> [u8; 16] {
    let mut g = [0u8; 16];
    let a = seed.wrapping_mul(0xDEAD_BEEF).wrapping_add(0x1234_5678);
    let b = a.wrapping_mul(0xCAFE_BABE).wrapping_add(0xABCD_EF01);
    g[0..4].copy_from_slice(&a.to_le_bytes());
    g[4..8].copy_from_slice(&b.to_le_bytes());
    g[8..12].copy_from_slice(&a.wrapping_add(b).to_le_bytes());
    g[12..16].copy_from_slice(&b.wrapping_sub(a).to_le_bytes());
    g
}

#[derive(Debug)]
pub enum SwapError {
    Io(BlkError),
    AlreadyActive,
    NotActive,
    InvalidMagic,
    UnsupportedVersion,
    NoSpace,
    InvalidSlot,
    SwapInUse,
}
