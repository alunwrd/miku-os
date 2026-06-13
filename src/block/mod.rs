//  Block layer - the single routing point between filesystems and storage
//  drivers, modelled on Linux's generic block layer
// 
//  Concrete drivers (ATA today) are registered once and live here behind a
//  stable 'BlockDevId'; nobody above this layer holds a driver directly.
//  Every read/write is expressed as a 'BioRequest' (see 'crate::vfs::bio'),
//  pushed through a real, serviced 'BioQueue', then dispatched to the driver.
//  The queue is now on the live I/O path (previously it was a dead struct),
//  which gives us per-device accounting and a seam for an I/O scheduler / IRQ completion later

extern crate alloc;

pub mod ahci;
pub mod cache;
pub mod driver;
pub mod nvme;
pub mod virtio_blk;

use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;

use driver::{BlkError, BlockDevInfo, BlockDriver};
use crate::ata::AtaDrive;
use crate::vfs::bio::{BioDirection, BioQueue};
use crate::vfs::types::{BlockDevId, INVALID_ID, INVALID_U8, MAX_BLOCK_DEVICES};

/// Device ids 0..=3 are reserved for the four legacy ATA slots; ids from 'FIRST_DYNAMIC_DEV' up are handed out to probed drivers (virtio-blk, ...)
pub const FIRST_DYNAMIC_DEV: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevKind {
    Ata,
    VirtioBlk,
    Ahci,
    Nvme,
}

struct DeviceSlot {
    driver: Box<dyn BlockDriver>,
    info:   BlockDevInfo,
    kind:   DevKind,
    /// Bytes read / written / discarded, for 'iostat'-style reporting
    sectors_read:      u64,
    sectors_written:   u64,
    sectors_discarded: u64,
    /// Failed requests (after retries) and retry attempts that recovered
    io_errors: u64,
    retries:   u64,
    /// Accumulated device time for average-latency reporting ('await')
    total_io_us: u64,
    io_count:    u64,
}

type SlotRef = Arc<Mutex<DeviceSlot>>;

/// Device registry. Held only for slot lookup/insertion - never across an
/// I/O operation, so I/O to different devices proceeds in parallel, each
/// serialized by its own slot mutex (Linux's per-queue locking)
static REGISTRY: Mutex<[Option<SlotRef>; MAX_BLOCK_DEVICES]> =
    Mutex::new([None, None, None, None, None, None, None, None]);

/// Bio accounting queue, independent of any device lock
static BIO: Mutex<BioQueue> = Mutex::new(BioQueue::new());

/// The four legacy ATA slots share controller I/O ports per channel, so ATA
/// operations additionally serialize on this bus lock; PCI devices
/// (AHCI/NVMe/virtio) have fully independent register files and skip it
static ATA_BUS: Mutex<()> = Mutex::new(());

fn slot_ref(dev_id: BlockDevId) -> Option<SlotRef> {
    REGISTRY.lock().get(dev_id as usize)?.clone()
}

/// Shared buffer cache (see 'cache.rs'). Initialized in 'probe()', once the
/// heap is up; reads issued before that bypass the cache
static CACHE: Mutex<Option<cache::BufferCache>> = Mutex::new(None);

/// Register an ATA drive into the block layer, returning its stable id
///
/// Idempotent: the device id is derived from the drive's controller/role index
/// (0..=3), so registering the same physical drive twice reuses one slot and
/// one driver instance - exactly what a freely-copied 'AtaDrive' needs
pub fn register_ata(drive: AtaDrive) -> BlockDevId {
    let id = drive.idx() as usize;
    if id >= MAX_BLOCK_DEVICES {
        return INVALID_U8;
    }

    if REGISTRY.lock()[id].is_some() {
        return id as BlockDevId;
    }

    // IDENTIFY outside any registry lock; the bus lock covers the ports
    let mut boxed: Box<dyn BlockDriver> = Box::new(drive);
    let info = {
        let _bus = ATA_BUS.lock();
        boxed.info()
    };
    crate::serial_println!(
        "[block] register dev {} (ata{}) model='{}' sectors={} lba48={}",
        id, id, info.model_str(), info.total_sectors, info.lba48
    );

    let mut reg = REGISTRY.lock();
    if reg[id].is_none() {
        reg[id] = Some(Arc::new(Mutex::new(DeviceSlot {
            driver: boxed,
            info,
            kind: DevKind::Ata,
            sectors_read: 0,
            sectors_written: 0,
            sectors_discarded: 0,
            io_errors: 0,
            retries: 0,
            total_io_us: 0,
            io_count: 0,
        })));
    }
    id as BlockDevId
}

/// One step of an I/O completion wait: spin briefly, but once the scheduler
/// is up, periodically yield the CPU so other threads run while the device
/// works. Call with an incrementing iteration counter
#[inline]
pub fn io_relax(iter: u64) {
    if iter % 1024 == 1023 && crate::scheduler::started() {
        crate::scheduler::yield_now();
    } else {
        core::hint::spin_loop();
    }
}

/// Register a probed (non-ATA) driver into the dynamic id range and run a
/// first test read that doubles as the partition-table peek
fn register_dynamic(mut boxed: Box<dyn BlockDriver>, kind: DevKind, label: &str) {
    let info = boxed.info();

    let mut reg = REGISTRY.lock();
    let Some(id) = (FIRST_DYNAMIC_DEV..MAX_BLOCK_DEVICES)
        .find(|&i| reg[i].is_none())
    else {
        crate::serial_println!("[block] no free slot for {} device", label);
        return;
    };

    crate::serial_println!(
        "[block] register dev {} ({}) model='{}' sectors={} ({} MB)",
        id, label, info.model_str(), info.total_sectors,
        info.total_sectors * 512 / (1024 * 1024)
    );
    reg[id] = Some(Arc::new(Mutex::new(DeviceSlot {
        driver: boxed,
        info,
        kind,
        sectors_read: 0,
        sectors_written: 0,
        sectors_discarded: 0,
        io_errors: 0,
        retries: 0,
        total_io_us: 0,
        io_count: 0,
    })));
    drop(reg);

    let mut sec0 = [0u8; 512];
    match read(id as BlockDevId, 0, 1, &mut sec0) {
        Ok(()) => {
            let mbr = sec0[510] == 0x55 && sec0[511] == 0xAA;
            crate::serial_println!(
                "[block] dev {}: sector 0 read ok, {}",
                id, if mbr { "MBR/GPT signature present" } else { "no partition table" }
            );
        }
        Err(e) => {
            crate::serial_println!("[block] dev {}: test read failed: {:?}", id, e);
        }
    }
}

/// Probe the PCI bus for storage controllers (IDE bus-master DMA, AHCI, virtio-blk) and register what is found into the dynamic id range.
/// Idempotent: runs the bus walk only once per boot
pub fn probe() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static PROBED: AtomicBool = AtomicBool::new(false);
    if PROBED.swap(true, Ordering::SeqCst) {
        return;
    }

    *CACHE.lock() = Some(cache::BufferCache::new());

    crate::ata::dma_init();

    let mut ahci_ports: [Option<ahci::AhciPort>; 4] = [None, None, None, None];
    let n = ahci::find_ports(&mut ahci_ports);
    for slot in ahci_ports.iter_mut().take(n) {
        if let Some(port) = slot.take() {
            register_dynamic(Box::new(port), DevKind::Ahci, "ahci");
        }
    }

    let mut found = [crate::net::pci::PciDevice::empty(); 4];
    let n = virtio_blk::find_devices(&mut found);
    for pci in found.iter().take(n) {
        let Some(drv) = virtio_blk::VirtioBlk::new(pci) else { continue };
        register_dynamic(Box::new(drv), DevKind::VirtioBlk, "virtio-blk");
    }

    if let Some(drv) = nvme::find_controller() {
        register_dynamic(Box::new(drv), DevKind::Nvme, "nvme");
    }
}

/// GPT partition ranges per device - '(start_lba, sector_count)' - filled
/// when the /dev block nodes are registered. Indexed by partition number
/// minus one; 15 partitions per device match the /dev minor encoding
/// ('minor = dev * 16 + part')
static PARTITIONS: Mutex<[[Option<(u64, u64)>; 15]; MAX_BLOCK_DEVICES]> =
    Mutex::new([[None; 15]; MAX_BLOCK_DEVICES]);

pub fn set_partition(dev_id: BlockDevId, part_idx: usize, start_lba: u64, sectors: u64) {
    if (dev_id as usize) < MAX_BLOCK_DEVICES && part_idx < 15 {
        PARTITIONS.lock()[dev_id as usize][part_idx] = Some((start_lba, sectors));
    }
}

/// Sector range behind a /dev block node: part 0 is the whole disk,
/// parts 1-15 are GPT partitions registered via 'set_partition'
pub fn node_range(dev_id: BlockDevId, part: u8) -> Option<(u64, u64)> {
    if part == 0 {
        let i = info(dev_id)?;
        if i.total_sectors == 0 {
            return None;
        }
        return Some((0, i.total_sectors));
    }
    *PARTITIONS.lock().get(dev_id as usize)?.get(part as usize - 1)?
}

/// Geometry/identity for a registered device, if present
pub fn info(dev_id: BlockDevId) -> Option<BlockDevInfo> {
    let slot = slot_ref(dev_id)?;
    let guard = slot.lock();
    Some(guard.info)
}

/// '(submitted, completed, errors)' counters from the live bio queue
pub fn io_stats() -> (u64, u64, u64) {
    let q = BIO.lock();
    (q.total_submitted, q.total_completed, q.total_errors)
}

/// Read 'count' 512-byte sectors starting at 'lba' into 'buf', served from
/// the buffer cache where possible. Misses are fetched in whole 4 KiB chunks
/// (read-around) and cached; reads touching the tail of the device or issued
/// before the cache exists go straight to the driver
pub fn read(dev_id: BlockDevId, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
    if count == 0 {
        return Ok(());
    }
    if (count as usize) * 512 > buf.len() {
        return Err(BlkError::BufferTooSmall);
    }

    let cache_ready = CACHE.lock().is_some();
    let total = info(dev_id).map(|i| i.total_sectors).unwrap_or(0);
    if !cache_ready || total == 0 {
        return driver_read(dev_id, lba, count, buf);
    }

    let end = lba + count as u64;
    let first_chunk = lba / cache::CHUNK_SECTORS;
    let last_chunk  = (end - 1) / cache::CHUNK_SECTORS;

    let mut tmp = [0u8; cache::CHUNK_BYTES];
    for chunk in first_chunk..=last_chunk {
        let chunk_start = chunk * cache::CHUNK_SECTORS;
        let chunk_end   = chunk_start + cache::CHUNK_SECTORS;
        let s = lba.max(chunk_start);
        let e = end.min(chunk_end);
        let buf_off = ((s - lba) * 512) as usize;
        let len     = ((e - s) * 512) as usize;
        let in_off  = ((s - chunk_start) * 512) as usize;

        let (hit, streak) = match CACHE.lock().as_mut() {
            Some(c) => (c.get(dev_id, chunk, &mut tmp), c.advance(dev_id, chunk)),
            None => (false, 0),
        };
        if hit {
            buf[buf_off..buf_off + len].copy_from_slice(&tmp[in_off..in_off + len]);
            continue;
        }

        if chunk_end <= total {
            // Sequential misses pull a readahead window in one driver
            // command; random misses fetch just their own chunk
            let avail = (total - chunk_start) / cache::CHUNK_SECTORS;
            // Adaptive window: a fresh sequential stream gets 32 KiB; a
            // sustained one (4+ chunks in a row) ramps to 64 KiB - the
            // same ramp-up idea as Linux readahead
            let want = match streak {
                0 => 1,
                1..=3 => RA_CHUNKS,
                _ => RA_MAX_CHUNKS,
            };
            let n = (want as u64).min(avail).max(1) as usize;

            let mut window = [0u8; RA_MAX_CHUNKS * cache::CHUNK_BYTES];
            let sectors = (n as u64 * cache::CHUNK_SECTORS) as u32;
            driver_read(dev_id, chunk_start, sectors, &mut window[..n * cache::CHUNK_BYTES])?;

            let mut evicted = [0u8; cache::CHUNK_BYTES];
            for i in 0..n {
                let off = i * cache::CHUNK_BYTES;
                let victim = CACHE.lock().as_mut().and_then(|c| {
                    c.insert(dev_id, chunk + i as u64, &window[off..off + cache::CHUNK_BYTES], &mut evicted)
                });
                // Inserting clean data may displace someone's dirty chunk;
                // it must reach the disk before the cache forgets it
                if let Some((vdev, vchunk)) = victim {
                    let vlba = vchunk * cache::CHUNK_SECTORS;
                    dispatch(vdev, BioDirection::Write, vlba, cache::CHUNK_SECTORS as u32, |drv| {
                        drv.write_blocks(vlba, cache::CHUNK_SECTORS as u32, &evicted)
                    })?;
                }
            }
            if n > 1 {
                if let Some(c) = CACHE.lock().as_mut() {
                    c.readaheads += 1;
                }
            }
            buf[buf_off..buf_off + len].copy_from_slice(&window[in_off..in_off + len]);
        } else {
            // Tail of the device (or probing past it): exact-range read,
            // uncached, preserving the driver's error semantics
            driver_read(dev_id, s, (e - s) as u32, &mut buf[buf_off..buf_off + len])?;
        }
    }
    Ok(())
}

/// Readahead window: up to 8 chunks (32 KiB) fetched per sequential miss
const RA_CHUNKS: usize = 8;

/// Ramped-up readahead window for sustained sequential streams (64 KiB)
const RA_MAX_CHUNKS: usize = 16;

/// Write 'count' 512-byte sectors starting at 'lba' from 'buf' - write-back:
/// the data lands in the buffer cache and the call returns; the device write
/// happens on 'flush', on dirty-pressure, or when the chunk gets evicted.
/// Sub-chunk writes read-modify-write their chunk so the cache always holds
/// whole 4 KiB units. Anything the cache can't take (device tail, cache not up yet) is written through
pub fn write(dev_id: BlockDevId, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
    if count == 0 {
        return Ok(());
    }
    if (count as usize) * 512 > buf.len() {
        return Err(BlkError::BufferTooSmall);
    }

    let cache_ready = CACHE.lock().is_some();
    let total = info(dev_id).map(|i| i.total_sectors).unwrap_or(0);
    if !cache_ready || total == 0 {
        return write_sync(dev_id, lba, count, buf);
    }

    let end = lba + count as u64;
    let first_chunk = lba / cache::CHUNK_SECTORS;
    let last_chunk  = (end - 1) / cache::CHUNK_SECTORS;

    let mut evicted = [0u8; cache::CHUNK_BYTES];
    for chunk in first_chunk..=last_chunk {
        let chunk_start = chunk * cache::CHUNK_SECTORS;
        let chunk_end   = chunk_start + cache::CHUNK_SECTORS;
        let s = lba.max(chunk_start);
        let e = end.min(chunk_end);
        let buf_off = ((s - lba) * 512) as usize;
        let len     = ((e - s) * 512) as usize;

        if chunk_end > total {
            // Device tail: bypass the cache entirely
            write_sync(dev_id, s, (e - s) as u32, &buf[buf_off..buf_off + len])?;
            continue;
        }

        let full_chunk = s == chunk_start && e == chunk_end;
        let victim = if full_chunk {
            CACHE.lock().as_mut().and_then(|c| {
                c.insert_dirty(dev_id, chunk, &buf[buf_off..buf_off + len], &mut evicted)
            })
        } else {
            // Partial chunk: merge into the resident copy, or read-modify - write to make it resident
            let merged = CACHE.lock().as_mut().map_or(false, |c| {
                c.merge_dirty(dev_id, chunk, s - chunk_start, &buf[buf_off..buf_off + len])
            });
            if merged {
                None
            } else {
                let mut tmp = [0u8; cache::CHUNK_BYTES];
                driver_read(dev_id, chunk_start, cache::CHUNK_SECTORS as u32, &mut tmp)?;
                let in_off = ((s - chunk_start) * 512) as usize;
                tmp[in_off..in_off + len].copy_from_slice(&buf[buf_off..buf_off + len]);
                CACHE.lock().as_mut().and_then(|c| {
                    c.insert_dirty(dev_id, chunk, &tmp, &mut evicted)
                })
            }
        };

        // A dirty chunk got evicted to make room - write it out now
        if let Some((vdev, vchunk)) = victim {
            let vlba = vchunk * cache::CHUNK_SECTORS;
            dispatch(vdev, BioDirection::Write, vlba, cache::CHUNK_SECTORS as u32, |drv| {
                drv.write_blocks(vlba, cache::CHUNK_SECTORS as u32, &evicted)
            })?;
        }
    }

    // Backpressure: don't let dirty data swamp the cache
    let dirty = CACHE.lock().as_ref().map_or(0, |c| c.dirty_count());
    if dirty > DIRTY_HIGH_WATER {
        flush_dirty_only(dev_id)?;
    }
    Ok(())
}

/// Ordered write-through, used where on-disk ordering is the point: ext3
/// journal records, GPT tables, swap headers. The device write completes
/// before this returns; resident cache chunks are updated in place (or
/// dropped if the write failed, since the on-disk state is then unknown)
pub fn write_sync(dev_id: BlockDevId, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlkError> {
    if count == 0 {
        return Ok(());
    }
    let result = dispatch(dev_id, BioDirection::Write, lba, count, |drv| {
        drv.write_blocks(lba, count, buf)
    });
    if let Some(c) = CACHE.lock().as_mut() {
        match result {
            Ok(()) => c.update_on_write(dev_id, lba, count, buf),
            Err(_) => c.invalidate_range(dev_id, lba, count),
        }
    }
    result
}

/// Write out every dirty chunk of 'dev_id' in ascending LBA order - a
/// single elevator sweep across the disk. Adjacent dirty chunks coalesce
/// into one driver command of up to 64 KiB (the block layer's request
/// merging). Returns how many chunks went out
fn flush_dirty_only(dev_id: BlockDevId) -> Result<usize, BlkError> {
    /// Merge window: 16 chunks = 64 KiB = one full driver transfer
    const MERGE_CHUNKS: usize = 16;
    let mut tmp = [0u8; MERGE_CHUNKS * cache::CHUNK_BYTES];
    let mut after = 0u64;
    let mut flushed = 0usize;
    loop {
        let (chunk, n) = match CACHE.lock().as_mut().and_then(|c| {
            c.pop_dirty_run(dev_id, after, &mut tmp, MERGE_CHUNKS)
        }) {
            Some(run) => run,
            None => break,
        };
        let lba = chunk * cache::CHUNK_SECTORS;
        let sectors = (n as u64 * cache::CHUNK_SECTORS) as u32;
        dispatch(dev_id, BioDirection::Write, lba, sectors, |drv| {
            drv.write_blocks(lba, sectors, &tmp[..n * cache::CHUNK_BYTES])
        })?;
        after = chunk + n as u64;
        flushed += n;
    }
    Ok(flushed)
}

/// Background writeback daemon - MikuOS's flusher thread. Wakes every
/// couple of seconds and sweeps each device's dirty chunks out to disk, so
/// write-back data never lingers in RAM indefinitely even when nobody
/// calls 'sync'. Registered with mikuD as the 'bdflush' service
pub fn writeback_thread() -> ! {
    // 500 ticks @ 250 Hz = 2 s between sweeps
    const INTERVAL_TICKS: u64 = 500;
    crate::serial_println!("[bdflush] thread started");
    loop {
        crate::scheduler::sleep(INTERVAL_TICKS);
        if CACHE.lock().as_ref().map_or(0, |c| c.dirty_count()) == 0 {
            continue;
        }
        let mut total = 0usize;
        for dev in 0..MAX_BLOCK_DEVICES as u8 {
            match flush_dirty_only(dev) {
                Ok(n) => total += n,
                Err(e) => {
                    crate::serial_println!("[bdflush] dev {}: writeback failed: {:?}", dev, e);
                }
            }
        }
        if total > 0 {
            crate::serial_println!("[bdflush] wrote back {} chunks", total);
        }
    }
}

/// Flush threshold: at half the cache dirty, writers start draining
const DIRTY_HIGH_WATER: usize = 256;

fn driver_read(dev_id: BlockDevId, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlkError> {
    dispatch(dev_id, BioDirection::Read, lba, count, |drv| drv.read_blocks(lba, count, buf))
}

/// '(hits, misses, readaheads, write_merges, dirty)' from the shared buffer cache
pub fn cache_stats() -> (u64, u64, u64, u64, u64) {
    match CACHE.lock().as_ref() {
        Some(c) => (c.hits, c.misses, c.readaheads, c.write_merges, c.dirty_count() as u64),
        None => (0, 0, 0, 0, 0),
    }
}

/// Per-device '(kind, sectors_read, sectors_written)' counters
pub fn dev_stats(dev_id: BlockDevId) -> Option<DevStats> {
    let slot = slot_ref(dev_id)?;
    let guard = slot.lock();
    Some(DevStats {
        kind:              guard.kind,
        sectors_read:      guard.sectors_read,
        sectors_written:   guard.sectors_written,
        sectors_discarded: guard.sectors_discarded,
        io_errors:         guard.io_errors,
        retries:           guard.retries,
        avg_io_us:         if guard.io_count > 0 { guard.total_io_us / guard.io_count } else { 0 },
        ios:               guard.io_count,
    })
}

/// Snapshot of a device's accounting counters for 'blkstat'
#[derive(Clone, Copy)]
pub struct DevStats {
    pub kind:              DevKind,
    pub sectors_read:      u64,
    pub sectors_written:   u64,
    pub sectors_discarded: u64,
    pub io_errors:         u64,
    pub retries:           u64,
    pub avg_io_us:         u64,
    pub ios:               u64,
}

/// SMART-style health report from the device (NVMe health log, ATA SMART).
/// None when the backend has no health source (virtio)
pub fn health(dev_id: BlockDevId) -> Option<driver::HealthInfo> {
    let slot = slot_ref(dev_id)?;
    let mut guard = slot.lock();
    if guard.kind == DevKind::Ata {
        let _bus = ATA_BUS.lock();
        guard.driver.health()
    } else {
        guard.driver.health()
    }
}

/// Tell the device a sector range no longer holds useful data (TRIM /
/// NVMe deallocate / virtio discard). The range's contents become
/// indeterminate. Cache chunks fully inside the range are dropped first -
/// dirty ones included, their data is dead by definition - so writeback
/// can't re-materialize discarded sectors; partially-covered edge chunks
/// stay resident, since their out-of-range sectors are still live
pub fn discard(dev_id: BlockDevId, lba: u64, count: u32) -> Result<(), BlkError> {
    let Some(inf) = info(dev_id) else { return Err(BlkError::NoDevice) };
    if !inf.discard {
        return Err(BlkError::Unsupported);
    }
    // Clamp to the device: discarding past the end is a no-op, not an error
    let end = (lba + count as u64).min(inf.total_sectors);
    if lba >= end {
        return Ok(());
    }
    let count = (end - lba) as u32;

    if let Some(c) = CACHE.lock().as_mut() {
        c.invalidate_covered(dev_id, lba, count);
    }
    dispatch(dev_id, BioDirection::Discard, lba, count, |drv| drv.discard(lba, count))
}

/// Zero a sector range. Unlike 'discard', the range is guaranteed to read
/// back as zeros. The chunk-aligned middle takes the device-side fast path
/// (NVMe Write Zeroes / virtio WRITE_ZEROES - no data transfer); its cache
/// chunks are dropped so later reads refetch zeros. The unaligned edges go
/// through the regular write path, which keeps partially covered cache
/// chunks coherent. Devices without a native command get zero-filled
/// buffer writes instead
pub fn write_zeroes(dev_id: BlockDevId, lba: u64, count: u32) -> Result<(), BlkError> {
    let Some(inf) = info(dev_id) else { return Err(BlkError::NoDevice) };
    if inf.read_only {
        return Err(BlkError::ReadOnly);
    }
    let end = (lba + count as u64).min(inf.total_sectors);
    if lba >= end {
        return Ok(());
    }

    let zeros = [0u8; cache::CHUNK_BYTES];
    let mid_start = (lba + cache::CHUNK_SECTORS - 1) & !(cache::CHUNK_SECTORS - 1);
    let mid_end   = end & !(cache::CHUNK_SECTORS - 1);

    if mid_start >= mid_end {
        // Range smaller than one aligned chunk: a plain zero write suffices
        return write(dev_id, lba, (end - lba) as u32, &zeros[..((end - lba) * 512) as usize]);
    }
    if lba < mid_start {
        write(dev_id, lba, (mid_start - lba) as u32, &zeros[..((mid_start - lba) * 512) as usize])?;
    }
    if mid_end < end {
        write(dev_id, mid_end, (end - mid_end) as u32, &zeros[..((end - mid_end) * 512) as usize])?;
    }

    if let Some(c) = CACHE.lock().as_mut() {
        c.invalidate_covered(dev_id, mid_start, (mid_end - mid_start) as u32);
    }

    let mid_count = (mid_end - mid_start) as u32;
    let r = dispatch(dev_id, BioDirection::Write, mid_start, mid_count, |drv| {
        drv.write_zeroes(mid_start, mid_count)
    });
    if !matches!(r, Err(BlkError::Unsupported)) {
        return r;
    }

    // Fallback: stream zero-filled buffers, one full transfer at a time
    let zbuf = [0u8; 16 * cache::CHUNK_BYTES];
    let mut at = mid_start;
    let mut left = mid_count;
    while left > 0 {
        let n = left.min((zbuf.len() / 512) as u32);
        dispatch(dev_id, BioDirection::Write, at, n, |drv| {
            drv.write_blocks(at, n, &zbuf[..n as usize * 512])
        })?;
        at += n as u64;
        left -= n;
    }
    Ok(())
}

/// Flush barrier: drain the device's dirty chunks (elevator-ordered), then
/// flush the device's own volatile write cache. After this returns,
/// everything previously written is durable
pub fn flush(dev_id: BlockDevId) -> Result<(), BlkError> {
    flush_dirty_only(dev_id)?;

    let Some(slot) = slot_ref(dev_id) else { return Err(BlkError::NoDevice) };
    let mut guard = slot.lock();
    if guard.kind == DevKind::Ata {
        let _bus = ATA_BUS.lock();
        guard.driver.flush()
    } else {
        guard.driver.flush()
    }
}

/// Core dispatch: push the op as a bio request onto the accounting queue,
/// lock only the target device's slot (plus the shared ATA bus lock for
/// legacy slots, whose channels share I/O ports) and run it. I/O to distinct
/// PCI devices proceeds fully in parallel
fn dispatch<F>(
    dev_id: BlockDevId,
    dir: BioDirection,
    lba: u64,
    count: u32,
    mut run: F,
) -> Result<(), BlkError>
where
    F: FnMut(&mut Box<dyn BlockDriver>) -> Result<(), BlkError>,
{
    /// Transient errors get this many extra attempts before giving up.
    /// Sector reads/writes are idempotent, so blind re-issue is safe.
    const MAX_RETRIES: u32 = 2;

    let Some(slot) = slot_ref(dev_id) else { return Err(BlkError::NoDevice) };

    // Enqueue for accounting / future scheduling. Buffer travels with the
    // closure, so the queued request carries no page id (synchronous path)
    let block_count = count.min(u16::MAX as u32) as u16;
    let ticket = BIO
        .lock()
        .submit(dir, dev_id, lba, block_count, INVALID_ID)
        .ok();

    let mut guard = slot.lock();
    let sw = crate::timing::Stopwatch::start();

    let mut attempt = 0u32;
    let result = loop {
        let r = if guard.kind == DevKind::Ata {
            let _bus = ATA_BUS.lock();
            run(&mut guard.driver)
        } else {
            run(&mut guard.driver)
        };
        match r {
            Ok(()) => break Ok(()),
            Err(e @ (BlkError::Timeout | BlkError::DeviceFault))
                if attempt < MAX_RETRIES =>
            {
                attempt += 1;
                guard.retries += 1;
                crate::serial_println!(
                    "[block] dev {} {:?} lba={} count={}: {:?} - retry {}/{}",
                    dev_id, dir, lba, count, e, attempt, MAX_RETRIES
                );
            }
            Err(e) => break Err(e),
        }
    };

    // Latency accounting once the TSC is calibrated (early-boot I/O would
    // otherwise record garbage).
    if crate::timing::tsc_khz() > 0 {
        guard.total_io_us += sw.elapsed_us();
        guard.io_count += 1;
    }

    match result {
        Ok(()) => match dir {
            BioDirection::Read => guard.sectors_read += count as u64,
            BioDirection::Write => guard.sectors_written += count as u64,
            BioDirection::Discard => guard.sectors_discarded += count as u64,
        },
        // Unsupported is a capability answer, not an I/O failure
        Err(BlkError::Unsupported) => {}
        Err(_) => guard.io_errors += 1,
    }
    drop(guard);

    if let Some(idx) = ticket {
        BIO.lock().complete(idx, result.is_ok());
    }

    result
}
