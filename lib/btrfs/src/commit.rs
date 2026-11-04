// Putting a transaction on the medium
//
// The first half writes new tree blocks at unused addresses, elsewhere. Here,
// one superblock write names them and the transaction is the filesystem.
//
// From `write_all_supers` in `fs/btrfs/disk-io.c`:
//
//   1. `barrier_all_devices`: an empty flush on every device, waited for.
//      Everything from the
//      first half is stable on return.
//   2. `write_dev_supers`: each superblock copy, primary first with `REQ_FUA`,
//      backups without.
//      The kernel accepts "a short window where the on-disk copies might still contain the older version".
//   3. `wait_dev_supers`: waits for those writes to be accepted, not stable. No
//      flush after the supers.
//
// Step 1 is the only ordering constraint. What a crash costs at each point is
// above `commit_superblocks`.
//
// Write methods of `fshost::BlockHost`:
//
//   `flush`                  step 1, the barrier
//   `write_sectors_barrier`  step 2's primary copy: `REQ_FUA`
//   `write_sectors`          step 2's backups, and every tree block
//   `write_sectors_sync`     not used: write-through ordering is for ext's journal; btrfs separates
//                            two groups of writes with a flush

use crate::chunk::Physical;
use crate::error::BtrfsError;
use crate::mount::{check_superblock, Superblock};
use crate::node::{header, parse_node};
use crate::structs::*;
use crate::volume::Volume;
use alloc::vec::Vec;
use fshost::BlockHost;

/// `dev_item.total_bytes`, relative to the superblock's start: the device's
/// size as recorded here, `device->commit_total_bytes` in the reference, which
/// bounds a superblock mirror. A device that grew has room the filesystem does
/// not know of
const DEV_ITEM_TOTAL_BYTES: usize = sb::DEV_ITEM + 8;

/// `BTRFS_HEADER_FLAG_WRITTEN`. `write_all_supers` sets it on each superblock,
/// and `check_superblock` counts it among allowed flags
const SUPER_FLAG_WRITTEN: u64 = 1 << 0;

/// What the caller wants from the device before return, the reference's two.
/// The barrier is a separate `flush`
#[derive(Clone, Copy, PartialEq, Eq)]
enum Durability {
    /// May sit in the device's cache; a crash may lose it
    Lazy,
    /// On stable media at return: `REQ_FUA`
    Stable,
}

/// Superblock mirrors fitting on a device of this size. The reference's test: a
/// mirror is skipped when
/// `bytenr + BTRFS_SUPER_INFO_SIZE >= device->commit_total_bytes`, stopping at
/// the first that does not fit, as offsets ascend. The kernel refuses a mirror
/// ending at the device's last byte, hence `>=`. Pure: a 512 MiB volume has
/// mirrors 0 and 1, and mirror 2 at 256 GiB must not be written
pub fn mirror_count(device_bytes: u64) -> usize {
    let mut n = 0;
    for offset in SUPER_MIRRORS {
        if offset.saturating_add(SUPER_INFO_SIZE as u64) >= device_bytes {
            break;
        }
        n += 1;
    }
    n
}

/// Writes bytes at a volume byte offset through the host's sectors. Unlike
/// `read_raw` in `volume.rs`, refuses an unaligned start, as read-modify-write
/// could destroy bytes on a failed read. Each offset here, the four mirror
/// addresses and a chunk stripe offset, is sector-aligned by format
fn write_raw<H: BlockHost>(
    host: &mut H,
    start_lba: u64,
    offset: u64,
    data: &[u8],
    how: Durability,
) -> Result<(), BtrfsError> {
    if data.is_empty() {
        return Ok(());
    }
    if offset % 512 != 0 || data.len() % 512 != 0 {
        return Err(BtrfsError::InvalidArg);
    }
    let lba = start_lba + offset / 512;
    let count = (data.len() / 512) as u32;
    match how {
        Durability::Lazy => host.write_sectors(lba, count, data)?,
        Durability::Stable => host.write_sectors_barrier(lba, count, data)?,
    }
    Ok(())
}

/// Where this volume starts on the host in 512-byte sectors, established by
/// reading. `Volume` keeps `start_lba` private, and guessing zero on a
/// partitioned disk would put a superblock inside another partition.
///
/// The primary superblock is at byte 65536 of the volume; if the volume starts
/// at sector zero those host bytes are its superblock, same location field and
/// fsid. Otherwise it is refused
fn volume_base<H: BlockHost>(vol: &mut Volume<H>) -> Result<u64, BtrfsError> {
    let mut found = Superblock::zeroed();
    let sectors = (SUPER_INFO_SIZE / 512) as u32;
    vol.host_mut()
        .read_sectors(SUPER_INFO_OFFSET / 512, sectors, &mut found.data)?;

    // `Some(SUPER_INFO_OFFSET)`: the read asks if the primary mirror is where
    // expected, the location check's question
    check_superblock(&found, Some(SUPER_INFO_OFFSET)).map_err(|_| BtrfsError::Unsupported)?;
    if found.fsid() != vol.superblock.fsid() {
        return Err(BtrfsError::Unsupported);
    }
    Ok(0)
}

/// This volume's device as btrfs numbers it, from the chunk map, as
/// `Volume::open` chooses. Reads and writes must agree
fn this_device<H: BlockHost>(vol: &Volume<H>) -> Result<u64, BtrfsError> {
    vol.chunks
        .iter()
        .next()
        .and_then(|c| c.stripes.first())
        .map(|s| s.devid)
        .ok_or(BtrfsError::Corrupt)
}

fn put_u64(buf: &mut [u8], off: usize, v: u64) -> Result<(), BtrfsError> {
    let slot = buf.get_mut(off..off + 8).ok_or(BtrfsError::InvalidArg)?;
    slot.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

fn put_u32(buf: &mut [u8], off: usize, v: u32) -> Result<(), BtrfsError> {
    let slot = buf.get_mut(off..off + 4).ok_or(BtrfsError::InvalidArg)?;
    slot.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

/// Stamps a superblock copy with its write address and the checksum covering it
/// there, in that order: the checksum covers byte 32 onward and `bytenr` is at
/// byte 48, as in `write_dev_supers`.
///
/// Each mirror has its own address. The kernel (`btrfs_validate_super`) and
/// `check_superblock` (`MountReject::WrongLocation`) refuse a mismatch, so a
/// stale superblock elsewhere on a disk cannot pass as a backup
fn seal_super(sb: &mut Superblock, at: u64) -> Result<(), BtrfsError> {
    put_u64(&mut sb.data, sb::BYTENR, at)?;
    let csum = sb.computed_csum();
    put_u32(&mut sb.data, sb::CSUM, csum)
}

/// Stamps a tree block with its address and checksum, so `write_block` accepts
/// it. Separate because `write_block` refuses a wrong `bytenr` or checksum
/// instead of fixing it, catching a block built for another address.
///
/// Sets only the address and sum. `HEADER_FLAG_WRITTEN` is the caller's
/// statement that the block is filled
pub fn seal_block(block: &mut [u8], logical: u64) -> Result<(), BtrfsError> {
    if block.len() < HEADER_SIZE {
        return Err(BtrfsError::InvalidArg);
    }
    put_u64(block, header::BYTENR, logical)?;
    let csum = fshost::crc32c::crc32c(0, &block[CSUM_SIZE..]);
    put_u32(block, header::CSUM, csum)
}

/// Writes a tree block at a logical address to every copy. A DUP chunk is two
/// copies on one device, and a reader takes the first passing its checksum, so
/// all copies must match; RAID1 likewise.
///
/// Ordinary `write_sectors`, as in `btrfs_write_and_wait_transaction`: the
/// commit's flush makes the block durable before anything names it.
///
/// The block is parsed as `read_node` would against its target address, so
/// nothing is written that this crate would refuse to read
pub fn write_block<H: BlockHost>(
    vol: &mut Volume<H>,
    logical: u64,
    data: &[u8],
) -> Result<(), BtrfsError> {
    let base = volume_base(vol)?;

    let nodesize = vol.geometry.nodesize as usize;
    if data.len() != nodesize {
        return Err(BtrfsError::InvalidArg);
    }

    // The tree checker, on the way in, at the cost of one node copy
    let fsid = vol.superblock.fsid();
    let mut owned = Vec::new();
    owned
        .try_reserve_exact(nodesize)
        .map_err(|_| BtrfsError::OutOfMemory)?;
    owned.extend_from_slice(data);
    parse_node(owned, logical, fsid, vol.geometry.sectorsize)?;

    // Copies resolved before writing: the map borrows the volume, the host
    // wants it mutably, and a partly mappable block is refused unwritten
    let copies = vol
        .chunks
        .find(logical)
        .ok_or(BtrfsError::NotFound)?
        .profile
        .copies();
    let devid = this_device(vol)?;
    let mut places = [Physical { devid: 0, offset: 0, len: 0 }; 4];
    let n = copies as usize;
    if n == 0 || n > places.len() {
        return Err(BtrfsError::Corrupt);
    }
    for (i, place) in places.iter_mut().enumerate().take(n) {
        let phys = vol.chunks.map(logical, i as u16)?;
        // One device. A stripe elsewhere is a chunk that cannot be fully
        // written, and partial copies would disagree
        if phys.devid != devid {
            return Err(BtrfsError::NotFound);
        }
        if phys.len < nodesize as u64 {
            return Err(BtrfsError::Corrupt);
        }
        *place = phys;
    }

    for place in places.iter().take(n) {
        write_raw(vol.host_mut(), base, place.offset, data, Durability::Lazy)?;
    }
    Ok(())
}

/// Commits a transaction's superblocks: the second half of
/// `btrfs_commit_transaction`.
///
/// The caller has written the new tree blocks and set `vol.superblock`'s root
/// fields. This moves the generation and writes the superblock in
/// `write_all_supers`' order.
///
/// # What a crash leaves
///
/// "Old volume" means every byte as before this call.
///
/// *Before the flush*: the new blocks are in space nothing references. Every
/// mirror has the old generation. Old volume.
///
/// *Between the flush and the primary*: the new blocks are stable,
/// unreferenced. Old volume.
///
/// *Between the primary and the first backup*: the primary, stable, names the
/// new generation, while backups name the old. This 4 KiB write is the
/// transaction; step 1 made its blocks durable. Stale backups are an older
/// transaction by definition.
///
/// *Between backups or after the last*: the same. Backups are lazy, and the
/// next commit rewrites them.
///
/// A superblock cannot name a generation whose blocks are still in a volatile
/// cache; step 1 forbids it.
///
/// # What this refuses
///
///   * the 4096 bytes must pass `check_superblock`, the reference's
///     `btrfs_validate_write_super`;
///   * the generation must exceed the volume's. A repeat or regression fails
///     each new block's
///     parent-transid check at next mount;
///   * the generation must not be below `chunk_root_generation`;
///   * `log_root` must be zero. This driver neither writes nor replays a tree
///     log, and replaying
///     records from the previous generation over this one would be wrong.
///
/// # What this does not touch
///
/// `update_super_roots` in `transaction.c` moves several fields, and only
/// `generation` is this function's. `root`, `root_level`, `chunk_root`,
/// `chunk_root_generation`, and `chunk_root_level` name blocks written by the
/// first half, so the writer sets them. `cache_generation` belongs to the
/// unmaintained v1 free space cache. `uuid_tree_generation` moves only with the
/// uuid tree. `backup_super_roots` rotates tree roots, as with the above.
/// `dev_item` stays, with no size or usage change.
///
/// `SUPER_FLAG_WRITTEN` is set, as `write_all_supers` does
pub fn commit_superblocks<H: BlockHost>(
    vol: &mut Volume<H>,
    generation: u64,
) -> Result<(), BtrfsError> {
    let base = volume_base(vol)?;

    // Checksum first, since the one in hand cannot be right. The caller has
    // written the new root, level, and `bytes_used`, so the arriving checksum
    // covers previous contents. `seal_super` recomputes it per mirror below, as
    // each copy's checksum covers its own `bytenr`. The rest of
    // `check_superblock` runs below: magic, fsid, feature masks, root
    // alignment, tree levels, `num_devices`, and `dev_item.fsid == fsid`
    let csum = vol.superblock.computed_csum();
    put_u32(&mut vol.superblock.data, sb::CSUM, csum)?;

    // `None`: this copy gets each mirror's address in turn, so its `bytenr` is
    // not a fact yet. The reference passes `mirror_num = -1`
    check_superblock(&vol.superblock, None).map_err(|_| BtrfsError::Corrupt)?;

    if generation <= vol.superblock.generation() {
        return Err(BtrfsError::InvalidArg);
    }
    if generation < u64_at(&vol.superblock.data, sb::CHUNK_ROOT_GENERATION)? {
        return Err(BtrfsError::InvalidArg);
    }
    if vol.superblock.log_root() != 0 {
        return Err(BtrfsError::Unsupported);
    }

    // The size as recorded, bounded by what the host says. A filesystem on a
    // later-truncated file records more than the disk; a grown disk has more
    // than the record. A mirror fits inside both
    let recorded = u64_at(&vol.superblock.data, DEV_ITEM_TOTAL_BYTES)?;
    let host_bytes = vol.host_mut().total_sectors().saturating_mul(512);
    let device_bytes = if host_bytes == 0 { recorded } else { recorded.min(host_bytes) };
    let mirrors = mirror_count(device_bytes);
    if mirrors == 0 {
        // Not even the primary fits, so the volume cannot be what it says
        return Err(BtrfsError::Corrupt);
    }

    put_u64(&mut vol.superblock.data, sb::GENERATION, generation)?;
    let flags = vol.superblock.flags() | SUPER_FLAG_WRITTEN;
    put_u64(&mut vol.superblock.data, sb::FLAGS, flags)?;

    // Step 1: `barrier_all_devices`, one flush on one device. The first half's
    // writes are stable on return
    vol.host_mut().flush()?;

    // Step 2, primary first and FUA. `vol.superblock` keeps this copy, sealed
    // for the primary, matching the address it was opened from
    seal_super(&mut vol.superblock, SUPER_MIRRORS[0])?;
    let primary = vol.superblock.data;
    write_raw(vol.host_mut(), base, SUPER_MIRRORS[0], &primary, Durability::Stable)?;

    // Step 2, backups, each sealed for its address, lazily. A clone keeps the
    // volume's copy valid for where it lives
    for &at in SUPER_MIRRORS.iter().take(mirrors).skip(1) {
        let mut copy = vol.superblock.clone();
        seal_super(&mut copy, at)?;
        write_raw(vol.host_mut(), base, at, &copy.data, Durability::Lazy)?;
    }

    // Step 3, `wait_dev_supers`: each write on a synchronous host has been
    // waited for. No flush here, as in the reference
    Ok(())
}
