use crate::host::ExtHost;
use crate::FsError;
use super::layout::{FsLayout, group_has_sb};
use super::params::{FsType, MkfsParams};
use crate::structs::*;
use crate::ext3::journal::{
    JBD2_CRC32C_CHKSUM, JBD_INCOMPAT_CSUM_V3, JBD_MAGIC, JBD_SUPERBLOCK_V2,
};
use crate::ext4::crc32c;

#[derive(Debug)]
pub enum MkfsError {
    Io(FsError),
    DiskTooSmall,
    TooManyGroups,
    InvalidParams(&'static str),
}

impl From<FsError> for MkfsError {
    fn from(e: FsError) -> Self { MkfsError::Io(e) }
}

pub struct MkfsReport {
    pub fs_type:        &'static str,
    pub block_size:     u32,
    pub inode_size:     u32,
    pub total_blocks:   u32,
    pub total_inodes:   u32,
    pub group_count:    u32,
    pub journal_blocks: u32,
    pub free_blocks:    u32,
    pub free_inodes:    u32,
}

/// Where a fresh filesystem is written: what moves sectors, plus the partition
/// offset, as with `DiskReader`
struct Writer<H: ExtHost> {
    host:      H,
    start_lba: u32,
}

impl<H: ExtHost> Writer<H> {
    #[inline]
    fn abs(&self, lba: u32) -> u64 {
        self.start_lba as u64 + lba as u64
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), MkfsError> {
        let at = self.abs(lba);
        self.host.write_sectors(at, 1, buf).map_err(|e| MkfsError::Io(e.into()))
    }

    fn zero_sector(&mut self, lba: u32) -> Result<(), MkfsError> {
        self.write_sector(lba, &[0u8; 512])
    }

    fn write_block(
        &mut self,
        block: u32,
        data: &[u8],
        block_size: u32,
        total_blocks: u32,
    ) -> Result<(), MkfsError> {
        if block >= total_blocks {
            crate::extlog!(
                "[mkfs] skip write_block {} (>= total_blocks {})", block, total_blocks
            );
            return Ok(());
        }
        let spb      = block_size / 512;
        let base_lba = block * spb;
        let at = self.abs(base_lba);
        self.host
            .write_sectors(at, spb, &data[..block_size as usize])
            .map_err(|e| MkfsError::Io(e.into()))
    }

    fn zero_block(&mut self, block: u32, block_size: u32, total_blocks: u32)
        -> Result<(), MkfsError>
    {
        if block >= total_blocks { return Ok(()); }
        let spb      = block_size / 512;
        let base_lba = block * spb;
        self.zero_lba_range(base_lba, spb)
    }

    fn zero_lba_range(&mut self, start_lba: u32, count: u32) -> Result<(), MkfsError> {
        const CHUNK: u32 = 255;
        static ZEROS: [u8; 255 * 512] = [0u8; 255 * 512];
        let mut done = 0u32;
        while done < count {
            let n = (count - done).min(CHUNK);
            let at = self.abs(start_lba + done);
            self.host
                .write_sectors(at, n, &ZEROS[..n as usize * 512])
                .map_err(|e| MkfsError::Io(e.into()))?;
            done += n;
        }
        Ok(())
    }

    fn probe_sectors(&mut self) -> u32 {
        // IDENTIFY-reported capacity, clamped to the u32 sector arithmetic of
        // the layout code
        let reported = self.host.total_sectors();
        if reported > 0 {
            return reported.min(u32::MAX as u64) as u32;
        }

        // Fallback: binary-searches the highest readable LBA
        let mut buf = [0u8; 512];
        if self.host.read_sectors(0, 1, &mut buf).is_err() {
            return 0;
        }
        let mut lo: u32 = 1;
        let mut hi: u32 = u32::MAX / 2;
        while hi > lo && self.host.read_sectors((hi - 1) as u64, 1, &mut buf).is_err() {
            hi /= 2;
            if hi == 0 { return lo; }
        }
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if self.host.read_sectors(mid as u64, 1, &mut buf).is_ok() { lo = mid; }
            else { hi = mid; }
        }
        lo + 1
    }
}


/// Indirect blocks for a block-mapped file of `data_blocks` blocks, at `ptrs`
/// pointers per block. Only the journal is large enough to need any: 12 blocks
/// fit in the inode, the next `ptrs` need one indirect block, past that a
/// double-indirect block plus one indirect per `ptrs`. Triple indirection, at 4
/// GiB, is not built but is counted
fn indirect_blocks_for(data_blocks: u32, ptrs: u32) -> u32 {
    if data_blocks <= 12 || ptrs == 0 {
        return 0;
    }
    let mut rem = data_blocks - 12;
    let mut meta = 0u32;

    let single = rem.min(ptrs);
    meta += 1;
    rem -= single;
    if rem == 0 {
        return meta;
    }

    let double_cap = ptrs.saturating_mul(ptrs);
    let double = rem.min(double_cap);
    meta += 1 + double.div_ceil(ptrs);
    rem -= double;
    if rem == 0 {
        return meta;
    }

    let triple = rem;
    meta += 1 + triple.div_ceil(double_cap) + triple.div_ceil(ptrs);
    meta
}

fn make_uuid(seed: u32) -> [u8; 16] {
    let mut u = [0u8; 16];
    let a = seed.wrapping_mul(0x6C62272E).wrapping_add(0xC965ABB7);
    let b = a.wrapping_mul(0x9E3779B9).wrapping_add(seed);
    let c = b.wrapping_mul(0xD2A98B26).wrapping_add(a);
    let d = c.wrapping_mul(0x45678913).wrapping_add(b);
    u[ 0.. 4].copy_from_slice(&a.to_le_bytes());
    u[ 4.. 8].copy_from_slice(&b.to_le_bytes());
    u[ 8..12].copy_from_slice(&c.to_le_bytes());
    u[12..16].copy_from_slice(&d.to_le_bytes());
    u[6] = (u[6] & 0x0F) | 0x40;
    u[8] = (u[8] & 0x3F) | 0x80;
    u
}

#[inline] fn wu16(b: &mut [u8], o: usize, v: u16) { b[o..o+2].copy_from_slice(&v.to_le_bytes()); }
#[inline] fn wu32(b: &mut [u8], o: usize, v: u32) { b[o..o+4].copy_from_slice(&v.to_le_bytes()); }
#[inline] fn wu32be(b: &mut [u8], o: usize, v: u32){ b[o..o+4].copy_from_slice(&v.to_be_bytes()); }


/// Lays down the fake dirent with a directory block's checksum and stamps it.
/// `tail` is 0 without metadata_csum, and then this does nothing
fn finish_dir_block(dir: &mut [u8], bs: usize, tail: usize, seed: u32, ino: u32) {
    if tail == 0 {
        return;
    }
    let at = bs - 12;
    wu32(dir, at, 0);
    wu16(dir, at + 4, 12);
    dir[at + 6] = 0;
    dir[at + 7] = 0xDE;
    let c = crc32c::ext4_dirent_csum(seed, ino, 0, &dir[..at]);
    wu32(dir, at + 8, c);
}

fn bitmap_set_range(buf: &mut [u8], first: u32, last: u32) {
    for bit in first..=last {
        let idx = (bit / 8) as usize;
        if idx < buf.len() {
            buf[idx] |= 1 << (bit % 8);
        }
    }
}

fn bitmap_mark_unused_tail(buf: &mut [u8], first_invalid: u32, block_size: u32) {
    let total_bits = block_size * 8;
    for bit in first_invalid..total_bits {
        let idx = (bit / 8) as usize;
        if idx < buf.len() {
            buf[idx] |= 1 << (bit % 8);
        }
    }
}

pub fn mkfs<H: ExtHost>(host: H, params: &MkfsParams) -> Result<MkfsReport, MkfsError> {
    if params.block_size != 1024 && params.block_size != 4096 {
        return Err(MkfsError::InvalidParams("block_size must be 1024 or 4096"));
    }
    if params.inode_size != 128 && params.inode_size != 256 {
        return Err(MkfsError::InvalidParams("inode_size must be 128 or 256"));
    }
    if params.fs_type == FsType::Ext4 && params.inode_size < 256 {
        return Err(MkfsError::InvalidParams("ext4 requires inode_size=256"));
    }
    if params.fs_type.needs_journal()
        && params.journal_blocks != 0
        && params.journal_blocks < super::params::MIN_JOURNAL_BLOCKS
    {
        return Err(MkfsError::InvalidParams(
            "journal_blocks must be >= 1024 - JBD2 rejects anything smaller",
        ));
    }

    let mut w = Writer { host, start_lba: params.start_lba };

    let total_sectors = if params.total_sectors > 0 {
        params.total_sectors
    } else {
        let s = w.probe_sectors();
        crate::extlog!("[mkfs] probed {} sectors ({} MB)", s, s / 2048);
        s.saturating_sub(params.start_lba)
    };

    let min_sectors = (params.block_size / 512) * 64;
    if total_sectors < min_sectors {
        return Err(MkfsError::DiskTooSmall);
    }

    // Like mkfs.ext4: tells the device the target region is dead before layout,
    // so SSDs and thin images can unmap it. Best effort
    {
        crate::extlog!(
            "[mkfs] step 0: discarding {} sectors at LBA {}",
            total_sectors, params.start_lba
        );
        let mut done = 0u64;
        while done < total_sectors as u64 {
            let n = (total_sectors as u64 - done).min(u32::MAX as u64) as u32;
            let at = params.start_lba as u64 + done;
            if w.host.discard(at, n).is_err() {
                crate::extlog!("[mkfs] discard failed - continuing without");
                break;
            }
            done += n as u64;
        }
    }

    crate::extlog!("[mkfs] step 1: zeroing old SB at LBA 2-3 (base={})", params.start_lba);
    w.zero_sector(2)?;
    w.zero_sector(3)?;
    w.host.flush().map_err(|e| MkfsError::Io(e.into()))?;

    let lay = FsLayout::compute(params, total_sectors);
    if lay.group_count == 0 || lay.total_blocks < 64 {
        return Err(MkfsError::DiskTooSmall);
    }

    let bs = lay.block_size as usize;
    let tb = lay.total_blocks;

    crate::extlog!(
        "[mkfs] step 2: {} grps, {} total blks, journal={} blks",
        lay.group_count, tb, lay.journal_blocks
    );

    let now  = w.host.wall_clock().unwrap_or_else(|| {
        let hz = w.host.tick_hz().max(1) as u64;
        (w.host.ticks() / hz) as u32
    });
    let uuid = make_uuid(now ^ (params.drive_index as u32).wrapping_mul(0xDEADBEEF));

    // Everything on metadata_csum chains from this, so it is derived once and
    // threaded through
    let (_, _, ro_flags) = feature_flags(params.fs_type, lay.inode_size);
    let csum = (ro_flags & FEATURE_RO_COMPAT_METADATA_CSUM) != 0;
    let seed = crc32c::ext4_csum_seed(&uuid, None);
    // The tail every directory block reserves for its checksum record
    let dtail = if csum { 12usize } else { 0 };

    crate::extlog!("[mkfs] step 3: zeroing group 0 metadata (lazy init)");
    {
        let gl = &lay.groups[0];
        w.zero_block(gl.block_bitmap, lay.block_size, tb)?;
        w.zero_block(gl.inode_bitmap, lay.block_size, tb)?;
        let itab_blocks = (lay.inodes_per_group as u64
            * lay.inode_size as u64
            + lay.block_size as u64 - 1) / lay.block_size as u64;
        let zero_itab = itab_blocks.min(4) as u32;
        for b in 0..zero_itab {
            w.zero_block(gl.inode_table + b, lay.block_size, tb)?;
        }
    }
    for g in 1..lay.group_count as usize {
        let gl = &lay.groups[g];
        w.zero_block(gl.block_bitmap, lay.block_size, tb)?;
        w.zero_block(gl.inode_bitmap, lay.block_size, tb)?;
    }

    crate::extlog!("[mkfs] step 4: block bitmaps");
    let mut bb = [0u8; 4096];
    // Bitmap checksums are stored in the group descriptors, written in step 7,
    // so they are collected as bitmaps are laid down
    let mut block_bitmap_csum = alloc::vec![0u32; lay.group_count as usize];
    let mut inode_bitmap_csum = alloc::vec![0u32; lay.group_count as usize];

    let g0      = &lay.groups[0];
    let root_blk = g0.data_start;
    let ptrs_per_block = lay.block_size / 4;
    let j_meta = indirect_blocks_for(lay.journal_blocks, ptrs_per_block);
    let (j_first, j_last_plus_one) = if lay.journal_blocks > 0 {
        let first = root_blk + 1;
        // Data blocks, then the indirect blocks mapping them
        (first, first + lay.journal_blocks + j_meta)
    } else {
        (root_blk + 1, root_blk + 1)
    };
    let lf_blk = j_last_plus_one;

    for g in 0..lay.group_count as usize {
        let gl = &lay.groups[g];
        bb.fill(0);

        let overhead = gl.data_start - gl.start_block;
        bitmap_set_range(&mut bb, 0, overhead.saturating_sub(1));

        if g == 0 {
            let local_root = root_blk - gl.start_block;
            let local_lf   = lf_blk  - gl.start_block;
            bitmap_set_range(&mut bb, local_root, local_lf.min(lay.blocks_per_group - 1));
        }

        if gl.total_blocks_in_group < lay.blocks_per_group {
            bitmap_mark_unused_tail(&mut bb, gl.total_blocks_in_group, lay.block_size);
        }

        if csum {
            let bytes = (lay.blocks_per_group as usize).div_ceil(8).min(bs);
            block_bitmap_csum[g] = crc32c::ext4_bitmap_csum(seed, &bb[..bytes]);
        }
        w.write_block(gl.block_bitmap, &bb[..bs], lay.block_size, tb)?;
    }

    crate::extlog!("[mkfs] step 5: inode bitmaps");
    let mut ib = [0u8; 4096];
    for g in 0..lay.group_count as usize {
        let gl = &lay.groups[g];
        ib.fill(0);
        if g == 0 {
            bitmap_set_range(&mut ib, 0, 10);
        }
        bitmap_mark_unused_tail(&mut ib, lay.inodes_per_group, lay.block_size);
        if csum {
            let bytes = (lay.inodes_per_group as usize).div_ceil(8).min(bs);
            inode_bitmap_csum[g] = crc32c::ext4_bitmap_csum(seed, &ib[..bytes]);
        }
        w.write_block(gl.inode_bitmap, &ib[..bs], lay.block_size, tb)?;
    }

    crate::extlog!("[mkfs] step 6a: root inode + dir block");
    {
        let mut dir = [0u8; 4096];
        wu32(&mut dir, 0, EXT2_ROOT_INO);
        wu16(&mut dir, 4, 12);
        dir[6] = 1; dir[7] = FT_DIR; dir[8] = b'.';
        wu32(&mut dir, 12, EXT2_ROOT_INO);
        wu16(&mut dir, 16, 12);
        dir[18] = 2; dir[19] = FT_DIR; dir[20] = b'.'; dir[21] = b'.';
        let off3 = 24usize;
        wu32(&mut dir, off3, EXT2_FIRST_INO_OLD);
        wu16(&mut dir, off3 + 4, (bs - dtail - off3) as u16);
        dir[off3 + 6] = 10; dir[off3 + 7] = FT_DIR;
        dir[off3 + 8..off3 + 18].copy_from_slice(b"lost+found");
        finish_dir_block(&mut dir, bs, dtail, seed, EXT2_ROOT_INO);
        w.write_block(root_blk, &dir[..bs], lay.block_size, tb)?;
        let raw = build_dir_inode(EXT2_ROOT_INO, 3, root_blk, &lay, params.fs_type, now);
        write_raw_inode(&mut w, &lay, EXT2_ROOT_INO, &raw, tb, csum.then_some(seed))?;
    }

    if lay.journal_blocks > 0 {
        crate::extlog!("[mkfs] step 6b: journal inode, {} blocks", lay.journal_blocks);
        let jblks = lay.journal_blocks;

        w.zero_block(j_first, lay.block_size, tb)?;

        let mut jsb = [0u8; 4096];
        wu32be(&mut jsb, 0,  JBD_MAGIC);
        wu32be(&mut jsb, 4,  JBD_SUPERBLOCK_V2);
        wu32be(&mut jsb, 8,  0);
        wu32be(&mut jsb, 12, lay.block_size);
        wu32be(&mut jsb, 16, jblks);
        wu32be(&mut jsb, 20, 1);
        wu32be(&mut jsb, 24, 1);
        wu32be(&mut jsb, 28, 0);
        wu32be(&mut jsb, 32, 0);
        jsb[48..64].copy_from_slice(&uuid);
        wu32be(&mut jsb, 64, 1);
        // A journal with checksums, so a torn commit record is detected.
        // `s_checksum_type` must read 4: JBD2 numbers crc32c fourth, and
        // e2fsprogs refuses a CSUM_V3 journal otherwise
        if ro_flags & FEATURE_RO_COMPAT_METADATA_CSUM != 0 {
            wu32be(&mut jsb, 40, JBD_INCOMPAT_CSUM_V3);
            jsb[80] = JBD2_CRC32C_CHKSUM;
            let csum = crate::ext4::crc32c::jbd2_superblock_csum(&jsb[..1024]);
            wu32be(&mut jsb, crate::ext4::crc32c::JBD2_SB_CSUM_OFF, csum);
        }
        w.write_block(j_first, &jsb[..bs], lay.block_size, tb)?;

        let mut raw = [0u8; 256];
        let ptrs = ptrs_per_block;
        let meta = indirect_blocks_for(jblks, ptrs);
        let j_size = jblks * lay.block_size;
        // i_blocks counts indirect blocks too, or e2fsck reports "i_blocks is
        // N, should be N+k"
        let j_blks_val = (jblks + meta) * (lay.block_size / 512);

        wu16(&mut raw, 0,  S_IFREG | 0o600);
        wu32(&mut raw, 4,  j_size);
        wu32(&mut raw, 8,  now); wu32(&mut raw, 12, now); wu32(&mut raw, 16, now);
        wu16(&mut raw, 26, 1);
        wu32(&mut raw, 28, j_blks_val);
        wu32(&mut raw, 32, 0);
        if lay.inode_size >= 256 {
            wu16(&mut raw, 128, 32);
        }

        // Data blocks are j_first.., with the mapping indirect blocks right
        // after
        let data_of = |i: u32| j_first + i;
        let mut next_meta = j_first + jblks;
        let mut take_meta = || {
            let b = next_meta;
            next_meta += 1;
            b
        };

        let direct = jblks.min(12);
        for i in 0..direct {
            wu32(&mut raw, 40 + i as usize * 4, data_of(i));
        }

        if jblks > 12 {
            let mut mapped = 12u32;

            // Single indirect
            let single = (jblks - mapped).min(ptrs);
            let ind_blk = take_meta();
            let mut ind = alloc::vec![0u8; bs];
            for k in 0..single {
                wu32(&mut ind, k as usize * 4, data_of(mapped + k));
            }
            w.write_block(ind_blk, &ind[..bs], lay.block_size, tb)?;
            wu32(&mut raw, 40 + 12 * 4, ind_blk);
            mapped += single;

            // Double indirect: one block of pointers to indirect blocks
            if mapped < jblks {
                let dind_blk = take_meta();
                let mut dind = alloc::vec![0u8; bs];
                let mut slot = 0usize;
                while mapped < jblks && (slot as u32) < ptrs {
                    let chunk = (jblks - mapped).min(ptrs);
                    let child = take_meta();
                    let mut child_buf = alloc::vec![0u8; bs];
                    for k in 0..chunk {
                        wu32(&mut child_buf, k as usize * 4, data_of(mapped + k));
                    }
                    w.write_block(child, &child_buf[..bs], lay.block_size, tb)?;
                    wu32(&mut dind, slot * 4, child);
                    slot += 1;
                    mapped += chunk;
                }
                w.write_block(dind_blk, &dind[..bs], lay.block_size, tb)?;
                wu32(&mut raw, 40 + 13 * 4, dind_blk);
            }

            debug_assert_eq!(next_meta, j_first + jblks + meta);
        }
        write_raw_inode(&mut w, &lay, EXT2_JOURNAL_INO, &raw, tb, csum.then_some(seed))?;
    }

    crate::extlog!("[mkfs] step 6c: lost+found inode, block={}", lf_blk);
    if lf_blk < tb {
        let mut dir = [0u8; 4096];
        wu32(&mut dir, 0,  EXT2_FIRST_INO_OLD);
        wu16(&mut dir, 4,  12);
        dir[6] = 1; dir[7] = FT_DIR; dir[8] = b'.';
        wu32(&mut dir, 12, EXT2_ROOT_INO);
        wu16(&mut dir, 16, (bs - dtail - 12) as u16);
        dir[18] = 2; dir[19] = FT_DIR; dir[20] = b'.'; dir[21] = b'.';
        finish_dir_block(&mut dir, bs, dtail, seed, EXT2_FIRST_INO_OLD);
        w.write_block(lf_blk, &dir[..bs], lay.block_size, tb)?;
        let raw = build_dir_inode(EXT2_FIRST_INO_OLD, 2, lf_blk, &lay, params.fs_type, now);
        write_raw_inode(&mut w, &lay, EXT2_FIRST_INO_OLD, &raw, tb, csum.then_some(seed))?;
    }

    crate::extlog!("[mkfs] step 7: GDT, {} block(s)", lay.gdt_blocks);
    {
        let gd_size = super::layout::GROUP_DESC_SIZE as usize;
        // The whole table
        let mut gdt = alloc::vec![0u8; lay.gdt_blocks as usize * bs];

        let g0_extra_used = {
            let mut n = 1u32;
            if lay.journal_blocks > 0 {
                n += lay.journal_blocks + indirect_blocks_for(lay.journal_blocks, lay.block_size / 4);
            }
            if lf_blk < tb { n += 1; }
            n
        };

        for g in 0..lay.group_count as usize {
            let gl  = &lay.groups[g];
            let off = g * gd_size;
            let free_blk = if g == 0 {
                gl.free_blocks.saturating_sub(g0_extra_used)
            } else {
                gl.free_blocks
            };
            let used_dirs: u16 = if g == 0 { 2 } else { 0 };
            wu32(&mut gdt, off + 0,  gl.block_bitmap);
            wu32(&mut gdt, off + 4,  gl.inode_bitmap);
            wu32(&mut gdt, off + 8,  gl.inode_table);
            wu16(&mut gdt, off + 12, free_blk as u16);
            wu16(&mut gdt, off + 14, gl.free_inodes as u16);
            wu16(&mut gdt, off + 16, used_dirs);
            if csum {
                wu16(&mut gdt, off + 24, (block_bitmap_csum[g] & 0xFFFF) as u16);
                wu16(&mut gdt, off + 26, (inode_bitmap_csum[g] & 0xFFFF) as u16);
                // The descriptor's checksum goes last, over everything above
                // with its two bytes read as zero
                const ZERO: [u8; 2] = [0, 0];
                let c = crc32c::crc32c_raw(seed, &(g as u32).to_le_bytes());
                let c = crc32c::crc32c_raw(c, &gdt[off..off + 30]);
                let c = crc32c::crc32c_raw(c, &ZERO);
                let c = crc32c::crc32c_raw(c, &gdt[off + 32..off + gd_size]);
                wu16(&mut gdt, off + 30, (c & 0xFFFF) as u16);
            }
        }

        let write_table = |w: &mut Writer<H>, first_block: u32| -> Result<(), MkfsError> {
            for i in 0..lay.gdt_blocks {
                let off = i as usize * bs;
                w.write_block(first_block + i, &gdt[off..off + bs], lay.block_size, tb)?;
            }
            Ok(())
        };

        write_table(&mut w, lay.gdt_block)?;

        for g in 1..lay.group_count as usize {
            if group_has_sb(g as u32) {
                let gl = &lay.groups[g];
                if gl.start_block < tb {
                    write_table(&mut w, gl.start_block + 1)?;
                }
            }
        }
    }

    crate::extlog!("[mkfs] step 8: superblock");
    let mut sb = [0u8; 1024];

    let g0_extra_used = {
        let mut n = 1u32;
        if lay.journal_blocks > 0 {
            n += lay.journal_blocks + indirect_blocks_for(lay.journal_blocks, lay.block_size / 4);
        }
        if lf_blk < tb { n += 1; }
        n
    };
    let free_blocks = lay.total_free_blocks().saturating_sub(g0_extra_used);
    let free_inodes = lay.total_free_inodes();

    wu32(&mut sb,  0,  lay.total_inodes);
    wu32(&mut sb,  4,  lay.total_blocks);
    wu32(&mut sb,  8,  lay.reserved_blocks);
    wu32(&mut sb, 12,  free_blocks);
    wu32(&mut sb, 16,  free_inodes);
    wu32(&mut sb, 20,  lay.first_data_block);
    wu32(&mut sb, 24,  match lay.block_size { 1024=>0, 2048=>1, _=>2 });
    wu32(&mut sb, 28,  match lay.block_size { 1024=>0, 2048=>1, _=>2 });
    wu32(&mut sb, 32,  lay.blocks_per_group);
    wu32(&mut sb, 36,  lay.blocks_per_group);
    wu32(&mut sb, 40,  lay.inodes_per_group);
    wu32(&mut sb, 44,  0);
    wu32(&mut sb, 48,  now);
    wu16(&mut sb, 52,  0);
    wu16(&mut sb, 54,  20);
    wu16(&mut sb, 56,  EXT2_MAGIC);
    wu16(&mut sb, 58,  1);
    wu16(&mut sb, 60,  1);
    wu16(&mut sb, 62,  0);
    wu32(&mut sb, 64,  now);
    wu32(&mut sb, 68,  0);
    wu32(&mut sb, 72,  0);
    wu32(&mut sb, 76,  1);
    wu16(&mut sb, 80,  0);
    wu16(&mut sb, 82,  0);
    wu32(&mut sb, 84,  EXT2_FIRST_INO_OLD);
    wu16(&mut sb, 88,  lay.inode_size as u16);
    wu16(&mut sb, 90,  0);

    let (fc, fi, fr) = feature_flags(params.fs_type, lay.inode_size);
    wu32(&mut sb, 92,  fc);
    wu32(&mut sb, 96,  fi);
    wu32(&mut sb, 100, fr);

    sb[104..120].copy_from_slice(&uuid);
    sb[120..136].copy_from_slice(&params.label);
    sb[136] = b'/';

    if lay.journal_blocks > 0 {
        // s_journal_uuid stays zero: it names an external journal device, and a
        // checker consults it before s_journal_inum
        wu32(&mut sb, 224, EXT2_JOURNAL_INO);
        wu32(&mut sb, 228, 0);
    }

    let hs = make_uuid(now.wrapping_add(1));
    sb[236..252].copy_from_slice(&hs);
    sb[252] = 1;
    wu16(&mut sb, 254, 32);
    wu32(&mut sb, 256, 0x000C);
    wu32(&mut sb, 264, now);

    if lay.inode_size >= 256 {
        wu16(&mut sb, 276, 28);
        wu16(&mut sb, 278, 28);
    }

    if csum {
        // s_checksum_type: crc32c is the only defined value
        sb[373] = 1;
        // Last, over everything before it; the seed's UUID is inside the summed
        // range
        let c = crc32c::ext4_superblock_csum(&sb);
        wu32(&mut sb, 1020, c);
    }

    let mut s0 = [0u8; 512];
    let mut s1 = [0u8; 512];
    s0.copy_from_slice(&sb[0..512]);
    s1.copy_from_slice(&sb[512..1024]);
    w.write_sector(2, &s0)?;
    w.write_sector(3, &s1)?;

    for g in 1..lay.group_count as usize {
        if group_has_sb(g as u32) {
            let gl     = &lay.groups[g];
            let sb_lba = gl.start_block * lay.sectors_per_block
                + if lay.block_size > 1024 { 2 } else { 0 };
            if (sb_lba + 1) * 512 / lay.block_size < tb {
                w.write_sector(sb_lba, &s0)?;
                w.write_sector(sb_lba + 1, &s1)?;
            }
        }
    }

    w.host.flush().map_err(|e| MkfsError::Io(e.into()))?;

    crate::extlog!(
        "[mkfs] done: {} blks {} ino {} grps jblks={} start_lba={}",
        lay.total_blocks, lay.total_inodes, lay.group_count, lay.journal_blocks, params.start_lba
    );

    Ok(MkfsReport {
        fs_type:        params.fs_type.name(),
        block_size:     lay.block_size,
        inode_size:     lay.inode_size,
        total_blocks:   lay.total_blocks,
        total_inodes:   lay.total_inodes,
        group_count:    lay.group_count,
        journal_blocks: lay.journal_blocks,
        free_blocks,
        free_inodes,
    })
}

fn build_dir_inode(
    _ino:    u32,
    nlinks:  u16,
    blk:     u32,
    lay:     &FsLayout,
    fs_type: FsType,
    now:     u32,
) -> [u8; 256] {
    let mut raw = [0u8; 256];
    wu16(&mut raw, 0,  S_IFDIR | 0o755);
    wu32(&mut raw, 4,  lay.block_size);
    wu32(&mut raw, 8,  now); wu32(&mut raw, 12, now); wu32(&mut raw, 16, now);
    wu16(&mut raw, 26, nlinks);
    wu32(&mut raw, 28, lay.block_size / 512);
    if lay.inode_size >= 256 {
        wu16(&mut raw, 128, 32);
    }

    if fs_type.needs_extents() {
        wu32(&mut raw, 32, EXT4_EXTENTS_FL);
        wu16(&mut raw, 40, 0xF30A);
        wu16(&mut raw, 42, 1);
        wu16(&mut raw, 44, 4);
        wu16(&mut raw, 46, 0);
        wu32(&mut raw, 48, 0);
        wu32(&mut raw, 52, 0);
        wu16(&mut raw, 56, 1);
        wu16(&mut raw, 58, 0);
        wu32(&mut raw, 60, blk);
    } else {
        wu32(&mut raw, 40, blk);
    }
    raw
}

fn write_raw_inode<H: ExtHost>(
    w:            &mut Writer<H>,
    lay:          &FsLayout,
    ino_num:      u32,
    raw:          &[u8; 256],
    total_blocks: u32,
    seed:         Option<u32>,
) -> Result<(), MkfsError> {
    // The checksum covers the inode with its field zeroed, computed on a copy
    // with everything else in place
    let mut raw = *raw;
    if let Some(seed) = seed {
        let size = lay.inode_size as usize;
        let extra = u16::from_le_bytes([raw[128], raw[129]]) as usize;
        let c = crc32c::ext4_inode_csum(seed, ino_num, 0, &raw[..size]);
        wu16(&mut raw, 124, (c & 0xFFFF) as u16);
        // i_checksum_hi exists only when i_extra_isize reaches it: Linux's
        // EXT4_FITS_IN_INODE
        if size > 128 && 128 + extra >= 132 {
            wu16(&mut raw, 130, (c >> 16) as u16);
        }
    }
    let raw = &raw;
    let idx   = ino_num - 1;
    let group = (idx / lay.inodes_per_group) as usize;
    let local = idx % lay.inodes_per_group;

    if group >= lay.group_count as usize { return Ok(()); }

    let byte_off = local as u64 * lay.inode_size as u64;
    let it_byte  = lay.groups[group].inode_table as u64 * lay.block_size as u64;
    let abs_byte = it_byte + byte_off;
    let lba      = (abs_byte / 512) as u32;
    let off      = (abs_byte % 512) as usize;

    let write_size = lay.inode_size as usize;
    let mut rem    = write_size;
    let mut dpos   = 0usize;
    let mut cur_lba = lba;
    let mut cur_off = off;

    let it_block = lay.groups[group].inode_table;
    if it_block >= total_blocks { return Ok(()); }

    while rem > 0 {
        let chunk = (512 - cur_off).min(rem);
        let mut sec = [0u8; 512];
        let at = w.abs(cur_lba);
        w.host.read_sectors(at, 1, &mut sec).map_err(|e| MkfsError::Io(e.into()))?;
        sec[cur_off..cur_off + chunk].copy_from_slice(&raw[dpos..dpos + chunk]);
        w.write_sector(cur_lba, &sec)?;
        dpos    += chunk;
        rem     -= chunk;
        cur_lba += 1;
        cur_off  = 0;
    }
    Ok(())
}

fn feature_flags(fs_type: FsType, inode_size: u32) -> (u32, u32, u32) {
    match fs_type {
        FsType::Ext2 => (
            FEATURE_COMPAT_DIR_INDEX | FEATURE_COMPAT_EXT_ATTR,
            FEATURE_INCOMPAT_FILETYPE,
            FEATURE_RO_COMPAT_SPARSE_SUPER | FEATURE_RO_COMPAT_LARGE_FILE,
        ),
        FsType::Ext3 => (
            FEATURE_COMPAT_HAS_JOURNAL | FEATURE_COMPAT_DIR_INDEX | FEATURE_COMPAT_EXT_ATTR,
            FEATURE_INCOMPAT_FILETYPE,
            FEATURE_RO_COMPAT_SPARSE_SUPER | FEATURE_RO_COMPAT_LARGE_FILE,
        ),
        FsType::Ext4 => {
            let mut ro = FEATURE_RO_COMPAT_SPARSE_SUPER
                | FEATURE_RO_COMPAT_LARGE_FILE
                | FEATURE_RO_COMPAT_HUGE_FILE
                | FEATURE_RO_COMPAT_DIR_NLINK;
            if inode_size >= 256 {
                ro |= FEATURE_RO_COMPAT_EXTRA_ISIZE;
                // Checksums need i_checksum_hi, past the 128-byte inode
                ro |= FEATURE_RO_COMPAT_METADATA_CSUM;
            }
            (
                FEATURE_COMPAT_HAS_JOURNAL | FEATURE_COMPAT_DIR_INDEX | FEATURE_COMPAT_EXT_ATTR,
                FEATURE_INCOMPAT_FILETYPE | FEATURE_INCOMPAT_EXTENTS,
                ro,
            )
        }
    }
}
