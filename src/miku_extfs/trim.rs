// FITRIM analogue - the kernel side of fstrim(8)
//
// Walks every block group's bitmap of the mounted filesystem and tells the
// device that runs of free blocks no longer hold useful data, via
// 'crate::block::discard' (NVMe DSM deallocate / ATA TRIM / virtio discard).
// The journal and all metadata are naturally skipped: their blocks are
// marked allocated in the bitmaps

use crate::miku_extfs::{FsError, MikuFS};

/// What a trim pass accomplished, for the shell report
pub struct TrimReport {
    pub trimmed_blocks: u64,
    pub trimmed_bytes:  u64,
}

impl MikuFS {
    /// Discard every run of at least 'minlen' contiguous free blocks.
    /// The filesystem is synced first, so what gets trimmed is free in the
    /// on-disk bitmaps too - not just in memory - and a crash right after
    /// cannot resurrect a discarded block as allocated
    pub fn trim_free_blocks(&mut self, minlen: u32) -> Result<TrimReport, FsError> {
        self.periodic_sync()?;

        let bs  = self.block_size as usize;
        let spb = self.sectors_per_block() as u64;
        let dev = self.reader.dev_id;
        let base_lba = self.reader.start_lba as u64;
        let total = self.superblock.blocks_count();
        let first = self.superblock.first_data_block();
        let minlen = minlen.max(1);

        let mut report = TrimReport { trimmed_blocks: 0, trimmed_bytes: 0 };
        let mut bitmap = [0u8; 4096];

        for group in 0..(self.group_count as usize).min(32) {
            let group_first = first + group as u32 * self.blocks_per_group;
            if group_first >= total {
                break;
            }
            let blocks_in_group = self.blocks_per_group.min(total - group_first);

            let bitmap_block = self.groups[group].block_bitmap();
            self.read_block_into(bitmap_block, &mut bitmap[..bs])?;

            let mut bit = 0u32;
            while bit < blocks_in_group {
                if bitmap[(bit / 8) as usize] & (1 << (bit % 8)) != 0 {
                    bit += 1;
                    continue;
                }
                let run_start = bit;
                while bit < blocks_in_group
                    && bitmap[(bit / 8) as usize] & (1 << (bit % 8)) == 0
                {
                    bit += 1;
                }
                let run_len = bit - run_start;
                if run_len < minlen {
                    continue;
                }

                // Bitmap bit b of group g is absolute fs block
                // 'first_data_block + g * blocks_per_group + b'
                let mut lba = base_lba + (group_first + run_start) as u64 * spb;
                let mut sectors = run_len as u64 * spb;
                while sectors > 0 {
                    let n = sectors.min(u32::MAX as u64) as u32;
                    crate::block::discard(dev, lba, n).map_err(|_| FsError::IoError)?;
                    lba += n as u64;
                    sectors -= n as u64;
                }
                report.trimmed_blocks += run_len as u64;
                report.trimmed_bytes  += run_len as u64 * bs as u64;
            }
        }
        Ok(report)
    }
}
