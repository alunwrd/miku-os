// Passes 4 and 5: the counters against what the first three counted
//
// The filesystem has been read twice, as inodes and blocks, then as directories
// and names, so each counter can be compared with a real count.
//
// A link count too high leaks a file; too low frees one still open. A clear
// block bitmap bit under a live file gives that block to the next writer.

use super::maps::block_buf;
use super::{Fsck, Problem};
use crate::error::FsError;
use crate::ext2::bitmap::mark_tail;
use crate::fs::MikuFS;
use crate::host::ExtHost;
use crate::structs::*;

impl<H: ExtHost> MikuFS<H> {
    // pass 4

    pub(crate) fn fsck_pass4(&mut self, st: &mut Fsck) -> Result<(), FsError> {
        // Inodes nothing names. Directories went through pass 3; here a file's
        // name is gone
        for ino in st.first_ino..=st.inodes_count {
            if !st.inode_in_use(ino) || st.links.get(ino) != 0 {
                continue;
            }
            // The journal, orphan file, and quota files are named only by the
            // superblock, and must not go to lost+found
            if self.superblock.is_hidden_inode(ino) {
                continue;
            }
            let fixed = self.fsck_reattach(st, ino)?;
            st.note(Problem::InodeUnattached { ino }, fixed);
        }

        // Linking into lost+found may have grown it, after pass 1
        if st.reconnected {
            if let Some(lf) = self.fsck_lost_found()? {
                self.fsck_reclaim(st, lf)?;
            }
        }

        for ino in 1..=st.inodes_count {
            if !st.inode_in_use(ino) {
                continue;
            }
            // A reserved inode has no name, so counting names says nothing
            // about its link count
            if (super::is_reserved(ino, st.first_ino) && ino != EXT2_ROOT_INO)
                || self.superblock.is_hidden_inode(ino)
            {
                continue;
            }
            let counted = st.links.get(ino);
            let Ok((mut inode, _)) = self.read_inode_unchecked(ino) else { continue };
            let on_disk = inode.links_count();
            if on_disk as u32 == counted {
                continue;
            }
            let fixed = if st.opts.repair && counted > 0 && counted <= EXT2_LINK_MAX as u32 {
                inode.set_links_count(counted as u16);
                self.write_inode(ino, &inode).is_ok()
            } else {
                false
            };
            st.note(Problem::LinkCountWrong { ino, on_disk, counted }, fixed);
        }
        Ok(())
    }

    /// Give an inode nothing names a name in `/lost+found`
    fn fsck_reattach(&mut self, st: &mut Fsck, ino: u32) -> Result<bool, FsError> {
        if !st.opts.repair {
            return Ok(false);
        }
        let Some(lf) = self.fsck_lost_found()? else { return Ok(false) };
        let Ok((inode, _)) = self.read_inode_unchecked(ino) else { return Ok(false) };
        let Some(ft) = super::ft_for_mode(inode.mode()) else { return Ok(false) };

        let (bytes, len) = super::orphan_name(ino);
        let name = core::str::from_utf8(&bytes[..len]).unwrap_or("");
        if name.is_empty() || self.add_dir_entry(lf, name, ino, ft).is_err() {
            return Ok(false);
        }
        st.reconnected = true;
        st.links.inc(ino);
        if ft == FT_DIR {
            self.fsck_set_dotdot(st, ino, lf)?;
            st.links.inc(lf);
        }
        Ok(true)
    }

    /// Marks each block an inode reaches as claimed, without judging. Used
    /// after a repair allocated blocks: pass 5 rebuilds the bitmap from claims
    /// and frees the rest
    fn fsck_reclaim(&mut self, st: &mut Fsck, ino: u32) -> Result<(), FsError> {
        let Ok((inode, _)) = self.read_inode_unchecked(ino) else { return Ok(()) };
        let mut logical = 0u32;
        let bs = self.block_size as u64;
        let blocks = inode.size().div_ceil(bs) as u32;
        while logical < blocks {
            if let Ok(phys) = self.get_file_block(&inode, logical) {
                if let Some(bit) = st.block_bit(phys) {
                    st.blocks.set(bit);
                }
            }
            logical += 1;
        }
        // And the indirect blocks holding that map, not reached by logical
        // block
        if !inode.uses_extents() {
            for slot in 12..15 {
                let b = inode.block(slot);
                if let Some(bit) = st.block_bit(b) {
                    st.blocks.set(bit);
                }
            }
        }
        Ok(())
    }

    // pass 5

    pub(crate) fn fsck_pass5(&mut self, st: &mut Fsck) -> Result<(), FsError> {
        let bs = self.block_size;
        let bpg = self.blocks_per_group;
        let ipg = self.inodes_per_group;
        let groups = self.groups.len();

        let mut want = block_buf(bs);
        let mut have = block_buf(bs);
        let mut free_blocks_total = 0u64;
        let mut free_inodes_total = 0u32;
        let mut used_blocks_total = 0u64;

        for g in 0..groups {
            let g64 = g as u64;
            let start = st.first_block as u64 + g64 * bpg as u64;
            let real = (st.blocks_count as u64).saturating_sub(start).min(bpg as u64) as u32;

            // What the bitmap should say: the filesystem's structures,
            // everything an inode claimed, and the tail past this group's last
            // block
            want.fill(0);
            self.mark_group_metadata(g, &mut want);
            for (i, b) in st.blocks.byte_range(g64 * bpg as u64, real as u64).iter().enumerate() {
                want[i] |= b;
            }

            let in_use = count_bits(&want, real);
            free_blocks_total += (real - in_use) as u64;
            used_blocks_total += in_use as u64;

            let bitmap_block = self.groups[g].block_bitmap();
            let uninit = self.groups[g].flags_gd() & EXT4_BG_BLOCK_UNINIT != 0;
            // An impossible descriptor was reported in pass 0. Reading it would
            // end the check with an I/O error
            let readable = self.read_block_into(bitmap_block, &mut have).is_ok();
            // An uninitialized group's bitmap block holds old disk contents;
            // the descriptor flag says what is in the group
            let diffs = if uninit || !readable { 0 } else { differences(&want, &have) };
            if diffs > 0 {
                let fixed = if st.opts.repair {
                    self.fsck_write_bitmap(g, bitmap_block, &want, false)?
                } else {
                    false
                };
                st.note(Problem::BlockBitmapWrong { group: g as u32, differences: diffs }, fixed);
            } else if !uninit
                && readable
                && self.superblock.has_metadata_csum()
                && !self.verify_block_bitmap_csum(g)
            {
                let fixed = if st.opts.repair {
                    self.set_block_bitmap_csum(g, &have);
                    self.flush_group_desc(g).is_ok()
                } else {
                    false
                };
                st.note(Problem::BitmapChecksumWrong { group: g as u32, inodes: false }, fixed);
            }

            // inode bitmap
            want.fill(0);
            for (i, b) in st.inodes.byte_range(g64 * ipg as u64, ipg as u64).iter().enumerate() {
                want[i] |= b;
            }
            mark_tail(&mut want, ipg);

            let used_inodes = count_bits(&want, ipg);
            free_inodes_total += ipg - used_inodes;

            let ibitmap = self.groups[g].inode_bitmap();
            let iuninit = self.groups[g].flags_gd() & EXT4_BG_INODE_UNINIT != 0;
            let ireadable = self.read_block_into(ibitmap, &mut have).is_ok();
            let idiffs = if iuninit || !ireadable { 0 } else { differences(&want, &have) };
            if idiffs > 0 {
                let fixed = if st.opts.repair {
                    self.fsck_write_bitmap(g, ibitmap, &want, true)?
                } else {
                    false
                };
                st.note(Problem::InodeBitmapWrong { group: g as u32, differences: idiffs }, fixed);
            } else if !iuninit
                && ireadable
                && self.superblock.has_metadata_csum()
                && !self.verify_inode_bitmap_csum(g)
            {
                let fixed = if st.opts.repair {
                    self.set_inode_bitmap_csum(g, &have);
                    self.flush_group_desc(g).is_ok()
                } else {
                    false
                };
                st.note(Problem::BitmapChecksumWrong { group: g as u32, inodes: true }, fixed);
            }

            // counters
            let dirs = count_bits_in(&st.dirs, g64 * ipg as u64, ipg as u64);
            let unused = trailing_free(&st.inodes, g64 * ipg as u64, ipg as u64);
            self.fsck_check_group_counts(
                st, g, real - in_use, ipg - used_inodes, dirs, unused,
            )?;
        }

        let sb_free_blocks = self.superblock.free_blocks_count_full();
        if sb_free_blocks != free_blocks_total {
            let fixed = if st.opts.repair {
                self.superblock.write_u32(12, free_blocks_total as u32);
                if self.superblock.has_64bit() {
                    self.superblock.write_u32(0x158, (free_blocks_total >> 32) as u32);
                }
                self.superblock_dirty = true;
                self.flush_superblock().is_ok()
            } else {
                false
            };
            st.note(
                Problem::SuperFreeBlocksWrong {
                    on_disk: sb_free_blocks,
                    counted: free_blocks_total,
                },
                fixed,
            );
        }

        let sb_free_inodes = self.superblock.free_inodes_count();
        if sb_free_inodes != free_inodes_total {
            let fixed = if st.opts.repair {
                self.superblock.write_u32(16, free_inodes_total);
                self.superblock_dirty = true;
                self.flush_superblock().is_ok()
            } else {
                false
            };
            st.note(
                Problem::SuperFreeInodesWrong {
                    on_disk: sb_free_inodes,
                    counted: free_inodes_total,
                },
                fixed,
            );
        }

        st.report.free_blocks = self.superblock.free_blocks_count_full();
        st.report.free_inodes = self.superblock.free_inodes_count();
        st.report.used_blocks = used_blocks_total;
        Ok(())
    }

    fn fsck_check_group_counts(
        &mut self, st: &mut Fsck, g: usize,
        free_blocks: u32, free_inodes: u32, dirs: u32, unused: u32,
    ) -> Result<(), FsError> {
        let d = &self.groups[g];
        let (fb, fi, ud) = (d.free_blocks_full(), d.free_inodes_full(), d.used_dirs() as u32);
        let iu = d.itable_unused();
        let has_unused = self.superblock.has_gdt_csum();

        let mut wrong = false;
        if fb != free_blocks {
            wrong = true;
        }
        if fi != free_inodes {
            wrong = true;
        }
        if ud != dirs {
            wrong = true;
        }
        if has_unused && iu != unused {
            wrong = true;
        }
        if !wrong {
            return Ok(());
        }

        let fixed = if st.opts.repair {
            self.set_group_counts(g, free_blocks, free_inodes, dirs);
            if has_unused {
                self.groups[g].set_itable_unused(unused);
            }
            self.update_group_desc_csum(g);
            self.flush_group_desc(g).is_ok()
        } else {
            false
        };

        let group = g as u32;
        if fb != free_blocks {
            st.note(
                Problem::GroupFreeBlocksWrong { group, on_disk: fb, counted: free_blocks },
                fixed,
            );
        }
        if fi != free_inodes {
            st.note(
                Problem::GroupFreeInodesWrong { group, on_disk: fi, counted: free_inodes },
                fixed,
            );
        }
        if ud != dirs {
            st.note(Problem::GroupUsedDirsWrong { group, on_disk: ud, counted: dirs }, fixed);
        }
        if has_unused && iu != unused {
            st.note(
                Problem::GroupItableUnusedWrong { group, on_disk: iu, counted: unused },
                fixed,
            );
        }
        Ok(())
    }

    /// Writes a rebuilt bitmap, refreshes its checksum, and takes the group out
    /// of the uninitialized state, since the block is now accurate
    fn fsck_write_bitmap(
        &mut self, g: usize, block: u32, data: &[u8], inodes: bool,
    ) -> Result<bool, FsError> {
        self.write_meta_block(block, data)?;
        if inodes {
            self.set_inode_bitmap_csum(g, data);
            let flags = self.groups[g].flags_gd();
            self.groups[g].set_flags_gd(flags & !EXT4_BG_INODE_UNINIT);
        } else {
            self.set_block_bitmap_csum(g, data);
            let flags = self.groups[g].flags_gd();
            self.groups[g].set_flags_gd(flags & !EXT4_BG_BLOCK_UNINIT);
        }
        self.update_group_desc_csum(g);
        self.flush_group_desc(g)?;
        Ok(true)
    }

    /// The three counters of a group descriptor, written to whichever halves
    /// this filesystem's descriptors have
    fn set_group_counts(&mut self, g: usize, free_blocks: u32, free_inodes: u32, dirs: u32) {
        let wide = self.superblock.group_desc_size() >= 64;
        let d = &mut self.groups[g];
        d.write_u16(12, free_blocks as u16);
        d.write_u16(14, free_inodes as u16);
        d.write_u16(16, dirs as u16);
        if wide {
            d.write_u16(44, (free_blocks >> 16) as u16);
            d.write_u16(46, (free_inodes >> 16) as u16);
            d.write_u16(48, (dirs >> 16) as u16);
        }
    }
}

/// How many of the first `bits` bits are set
fn count_bits(buf: &[u8], bits: u32) -> u32 {
    let mut n = 0u32;
    let whole = (bits / 8) as usize;
    for &b in buf[..whole.min(buf.len())].iter() {
        n += b.count_ones();
    }
    let rest = bits % 8;
    if rest != 0 && whole < buf.len() {
        n += (buf[whole] & ((1u16 << rest) - 1) as u8).count_ones();
    }
    n
}

fn count_bits_in(map: &super::maps::BitMap, from: u64, len: u64) -> u32 {
    map.count_range(from, len) as u32
}

/// Inodes at the end of the group that nothing has ever used
fn trailing_free(map: &super::maps::BitMap, from: u64, len: u64) -> u32 {
    let mut n = 0u32;
    for i in (0..len).rev() {
        if map.get(from + i) {
            break;
        }
        n += 1;
    }
    n
}

/// How many bits differ between what should be there and what is
fn differences(want: &[u8], have: &[u8]) -> u32 {
    let mut n = 0u32;
    for i in 0..want.len().min(have.len()) {
        n += (want[i] ^ have[i]).count_ones();
    }
    n
}
