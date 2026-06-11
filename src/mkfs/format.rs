use crate::block::driver::BlkError;
use crate::vfs::types::BlockDevId;
use super::layout::{FsLayout, group_has_sb};
use super::params::{FsType, MkfsParams};
use crate::miku_extfs::structs::*;
use crate::miku_extfs::ext3::journal::{JBD_MAGIC, JBD_SUPERBLOCK_V2};

#[derive(Debug)]
pub enum MkfsError {
    Io(BlkError),
    DiskTooSmall,
    TooManyGroups,
    InvalidParams(&'static str),
}

impl From<BlkError> for MkfsError {
    fn from(e: BlkError) -> Self { MkfsError::Io(e) }
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

struct Writer {
    dev:       BlockDevId,
    start_lba: u32,
}

impl Writer {
    fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), MkfsError> {
        crate::block::write(self.dev, (self.start_lba + lba) as u64, 1, buf)
            .map_err(MkfsError::Io)
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
            crate::serial_println!(
                "[mkfs] skip write_block {} (>= total_blocks {})", block, total_blocks
            );
            return Ok(());
        }
        let spb      = block_size / 512;
        let base_lba = block * spb;
        crate::block::write(
            self.dev,
            (self.start_lba + base_lba) as u64,
            spb,
            &data[..block_size as usize],
        )
        .map_err(MkfsError::Io)
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
            crate::block::write(
                self.dev,
                (self.start_lba + start_lba + done) as u64,
                n,
                &ZEROS[..n as usize * 512],
            )
            .map_err(MkfsError::Io)?;
            done += n;
        }
        Ok(())
    }

    fn probe_sectors(&mut self) -> u32 {
        // IDENTIFY-reported capacity, clamped to the u32 sector arithmetic the
        // mkfs layout code works in.
        if let Some(info) = crate::block::info(self.dev) {
            if info.total_sectors > 0 {
                return info.total_sectors.min(u32::MAX as u64) as u32;
            }
        }

        // Fallback: binary-search the highest readable LBA.
        let mut buf = [0u8; 512];
        if crate::block::read(self.dev, 0, 1, &mut buf).is_err() {
            return 0;
        }
        let mut lo: u32 = 1;
        let mut hi: u32 = u32::MAX / 2;
        while hi > lo && crate::block::read(self.dev, (hi - 1) as u64, 1, &mut buf).is_err() {
            hi /= 2;
            if hi == 0 { return lo; }
        }
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if crate::block::read(self.dev, mid as u64, 1, &mut buf).is_ok() { lo = mid; }
            else { hi = mid; }
        }
        lo + 1
    }
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

fn bitmap_mark_used_range(buf: &mut [u8], last_bit: u32) {
    let full_bytes = (last_bit / 8) as usize;
    for i in 0..full_bytes {
        if i < buf.len() { buf[i] = 0xFF; }
    }
    let rem = last_bit % 8;
    if rem > 0 && full_bytes < buf.len() {
        buf[full_bytes] = (1u16 << rem).saturating_sub(1) as u8;
    }
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

pub fn mkfs(dev: BlockDevId, params: &MkfsParams) -> Result<MkfsReport, MkfsError> {
    if params.block_size != 1024 && params.block_size != 4096 {
        return Err(MkfsError::InvalidParams("block_size must be 1024 or 4096"));
    }
    if params.inode_size != 128 && params.inode_size != 256 {
        return Err(MkfsError::InvalidParams("inode_size must be 128 or 256"));
    }
    if params.fs_type == FsType::Ext4 && params.inode_size < 256 {
        return Err(MkfsError::InvalidParams("ext4 requires inode_size=256"));
    }
    if params.fs_type.needs_journal() && params.journal_blocks < 16
        && params.journal_blocks != 0
    {
        return Err(MkfsError::InvalidParams("journal_blocks must be >= 16"));
    }

    let mut w = Writer { dev, start_lba: params.start_lba };

    let total_sectors = if params.total_sectors > 0 {
        params.total_sectors
    } else {
        let s = w.probe_sectors();
        crate::serial_println!("[mkfs] probed {} sectors ({} MB)", s, s / 2048);
        s.saturating_sub(params.start_lba)
    };

    let min_sectors = (params.block_size / 512) * 64;
    if total_sectors < min_sectors {
        return Err(MkfsError::DiskTooSmall);
    }

    crate::serial_println!("[mkfs] step 1: zeroing old SB at LBA 2-3 (base={})", params.start_lba);
    w.zero_sector(2)?;
    w.zero_sector(3)?;
    crate::block::flush(w.dev).map_err(MkfsError::Io)?;

    let lay = FsLayout::compute(params, total_sectors);
    if lay.group_count == 0 || lay.total_blocks < 64 {
        return Err(MkfsError::DiskTooSmall);
    }

    let bs = lay.block_size as usize;
    let tb = lay.total_blocks;

    crate::serial_println!(
        "[mkfs] step 2: {} grps, {} total blks, journal={} blks",
        lay.group_count, tb, lay.journal_blocks
    );

    let now  = (crate::vfs::procfs::uptime_ticks() / 18) as u32;
    let uuid = make_uuid(now ^ (params.drive_index as u32).wrapping_mul(0xDEADBEEF));

    crate::serial_println!("[mkfs] step 3: zeroing group 0 metadata (lazy init)");
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

    crate::serial_println!("[mkfs] step 4: block bitmaps");
    let mut bb = [0u8; 4096];

    let g0      = &lay.groups[0];
    let root_blk = g0.data_start;
    let (j_first, j_last_plus_one) = if lay.journal_blocks > 0 {
        let first = root_blk + 1;
        let mut last_plus_one = first + lay.journal_blocks;
        if lay.journal_blocks > 12 {
            last_plus_one += 1;
        }
        (first, last_plus_one)
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

        w.write_block(gl.block_bitmap, &bb[..bs], lay.block_size, tb)?;
    }

    crate::serial_println!("[mkfs] step 5: inode bitmaps");
    let mut ib = [0u8; 4096];
    for g in 0..lay.group_count as usize {
        let gl = &lay.groups[g];
        ib.fill(0);
        if g == 0 {
            bitmap_set_range(&mut ib, 0, 10);
        }
        bitmap_mark_unused_tail(&mut ib, lay.inodes_per_group, lay.block_size);
        w.write_block(gl.inode_bitmap, &ib[..bs], lay.block_size, tb)?;
    }

    crate::serial_println!("[mkfs] step 6a: root inode + dir block");
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
        wu16(&mut dir, off3 + 4, (bs - off3) as u16);
        dir[off3 + 6] = 10; dir[off3 + 7] = FT_DIR;
        dir[off3 + 8..off3 + 18].copy_from_slice(b"lost+found");
        w.write_block(root_blk, &dir[..bs], lay.block_size, tb)?;
        let raw = build_dir_inode(EXT2_ROOT_INO, 3, root_blk, &lay, params.fs_type, now);
        write_raw_inode(&mut w, &lay, EXT2_ROOT_INO, &raw, tb)?;
    }

    if lay.journal_blocks > 0 {
        crate::serial_println!("[mkfs] step 6b: journal inode, {} blocks", lay.journal_blocks);
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
        w.write_block(j_first, &jsb[..bs], lay.block_size, tb)?;

        let mut raw = [0u8; 256];
        let j_size     = jblks * lay.block_size;
        let j_blks_val = jblks * (lay.block_size / 512);
        wu16(&mut raw, 0,  S_IFREG | 0o600);
        wu32(&mut raw, 4,  j_size);
        wu32(&mut raw, 8,  now); wu32(&mut raw, 12, now); wu32(&mut raw, 16, now);
        wu16(&mut raw, 26, 1);
        wu32(&mut raw, 28, j_blks_val);
        wu32(&mut raw, 32, 0);

        let direct = jblks.min(12) as usize;
        for i in 0..direct {
            wu32(&mut raw, 40 + i * 4, j_first + i as u32);
        }
        if jblks > 12 {
            let ind_blk = j_first + jblks;
            let mut ind = [0u8; 4096];
            for i in 12..jblks as usize {
                wu32(&mut ind, (i - 12) * 4, j_first + i as u32);
            }
            w.write_block(ind_blk, &ind[..bs], lay.block_size, tb)?;
            wu32(&mut raw, 40 + 12 * 4, ind_blk);
        }
        write_raw_inode(&mut w, &lay, EXT2_JOURNAL_INO, &raw, tb)?;
    }

    crate::serial_println!("[mkfs] step 6c: lost+found inode, block={}", lf_blk);
    if lf_blk < tb {
        let mut dir = [0u8; 4096];
        wu32(&mut dir, 0,  EXT2_FIRST_INO_OLD);
        wu16(&mut dir, 4,  12);
        dir[6] = 1; dir[7] = FT_DIR; dir[8] = b'.';
        wu32(&mut dir, 12, EXT2_ROOT_INO);
        wu16(&mut dir, 16, (bs - 12) as u16);
        dir[18] = 2; dir[19] = FT_DIR; dir[20] = b'.'; dir[21] = b'.';
        w.write_block(lf_blk, &dir[..bs], lay.block_size, tb)?;
        let raw = build_dir_inode(EXT2_FIRST_INO_OLD, 2, lf_blk, &lay, params.fs_type, now);
        write_raw_inode(&mut w, &lay, EXT2_FIRST_INO_OLD, &raw, tb)?;
    }

    crate::serial_println!("[mkfs] step 7: GDT");
    {
        let gd_size = 32usize;
        let mut gdt = [0u8; 4096];

        let g0_extra_used = {
            let mut n = 1u32;
            if lay.journal_blocks > 0 {
                n += lay.journal_blocks;
                if lay.journal_blocks > 12 { n += 1; }
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
        }

        w.write_block(lay.gdt_block, &gdt[..bs], lay.block_size, tb)?;

        for g in 1..lay.group_count as usize {
            if group_has_sb(g as u32) {
                let gl = &lay.groups[g];
                if gl.start_block < tb {
                    let gdt_copy = gl.start_block + 1;
                    w.write_block(gdt_copy, &gdt[..bs], lay.block_size, tb)?;
                }
            }
        }
    }

    crate::serial_println!("[mkfs] step 8: superblock");
    let mut sb = [0u8; 1024];

    let g0_extra_used = {
        let mut n = 1u32;
        if lay.journal_blocks > 0 {
            n += lay.journal_blocks;
            if lay.journal_blocks > 12 { n += 1; }
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
        sb[208..224].copy_from_slice(&uuid);
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

    crate::block::flush(w.dev).map_err(MkfsError::Io)?;

    crate::serial_println!(
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

fn write_raw_inode(
    w:            &mut Writer,
    lay:          &FsLayout,
    ino_num:      u32,
    raw:          &[u8; 256],
    total_blocks: u32,
) -> Result<(), MkfsError> {
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
        crate::block::read(w.dev, (w.start_lba + cur_lba) as u64, 1, &mut sec).map_err(MkfsError::Io)?;
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
            if inode_size >= 256 { ro |= FEATURE_RO_COMPAT_EXTRA_ISIZE; }
            (
                FEATURE_COMPAT_HAS_JOURNAL | FEATURE_COMPAT_DIR_INDEX | FEATURE_COMPAT_EXT_ATTR,
                FEATURE_INCOMPAT_FILETYPE | FEATURE_INCOMPAT_EXTENTS,
                ro,
            )
        }
    }
}
