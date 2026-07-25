use super::params::{FsType, MkfsParams};

pub struct FsLayout {
    pub fs_type:            FsType,
    pub block_size:         u32,
    pub inode_size:         u32,
    pub sectors_per_block:  u32,
    pub first_data_block:   u32,
    pub gdt_block:          u32,
    pub group_count:        u32,
    pub blocks_per_group:   u32,
    pub inodes_per_group:   u32,
    pub inode_table_blocks: u32,
    pub total_blocks:       u32,
    pub total_inodes:       u32,
    pub reserved_blocks:    u32,
    pub journal_blocks:     u32,
    pub groups:             [GroupLayout; 32],
}

#[derive(Clone, Copy, Default)]
pub struct GroupLayout {
    pub start_block:           u32,
    pub has_sb_copy:           bool,
    pub sb_overhead:           u32,
    pub block_bitmap:          u32,
    pub inode_bitmap:          u32,
    pub inode_table:           u32,
    pub data_start:            u32,
    pub total_blocks_in_group: u32,
    pub free_blocks:           u32,
    pub free_inodes:           u32,
}

fn is_power_of(mut n: u32, base: u32) -> bool {
    if n == 0 { return false; }
    while n % base == 0 { n /= base; }
    n == 1
}

pub fn group_has_sb(group: u32) -> bool {
    group == 0
        || group == 1
        || is_power_of(group, 3)
        || is_power_of(group, 5)
        || is_power_of(group, 7)
}

impl FsLayout {
    pub fn compute(params: &MkfsParams, total_sectors: u32) -> Self {
        let block_size        = params.block_size;
        let inode_size        = params.inode_size;
        let sectors_per_block = block_size / 512;
        let first_data_block  = if block_size == 1024 { 1u32 } else { 0u32 };

        let total_blocks_real = total_sectors / sectors_per_block;

        let gdt_block    = first_data_block + 1;
        let gdt_blocks   = 1u32;

        let blocks_per_group = block_size * 8;

        let (inodes_per_group, inode_table_blocks) = {
            let target = blocks_per_group / 4;
            let it_b   = (target * inode_size + block_size - 1) / block_size;
            let max_it = blocks_per_group.saturating_sub(2 + 2 + 64);
            if it_b > max_it {
                let clamped_ino = max_it * block_size / inode_size;
                (clamped_ino, max_it)
            } else {
                (target, it_b)
            }
        };

        let usable      = total_blocks_real.saturating_sub(first_data_block);
        let group_count = ((usable + blocks_per_group - 1) / blocks_per_group)
            .min(32)
            .max(1);

        let total_blocks = (first_data_block + group_count * blocks_per_group)
            .min(total_blocks_real);
        let total_inodes   = group_count * inodes_per_group;
        let reserved_blocks = total_blocks / 20;

        let g0_overhead = 1 + gdt_blocks + 1 + 1 + inode_table_blocks;
        let g0_real     = total_blocks.saturating_sub(first_data_block)
            .min(blocks_per_group);
        let g0_for_journal = g0_real.saturating_sub(g0_overhead + 2 + 4);
        let journal_blocks = if params.fs_type.needs_journal() {
            params.journal_blocks.min(g0_for_journal)
        } else {
            0
        };

        crate::serial_println!(
            "[mkfs] disk={} blks, bs={}, groups={}, ino/g={}, jblks={}",
            total_blocks, block_size, group_count, inodes_per_group, journal_blocks
        );

        let mut groups = [GroupLayout::default(); 32];

        for g in 0..group_count as usize {
            let g_start  = first_data_block + g as u32 * blocks_per_group;
            let has_sb   = group_has_sb(g as u32);
            let sb_over  = if has_sb { 1 + gdt_blocks } else { 0 };

            let block_bitmap = g_start + sb_over;
            let inode_bitmap = block_bitmap + 1;
            let inode_table  = inode_bitmap + 1;
            let data_start   = inode_table + inode_table_blocks;

            let g_total = if g as u32 == group_count - 1 {
                total_blocks.saturating_sub(g_start)
            } else {
                blocks_per_group
            };

            let overhead = sb_over + 1 + 1 + inode_table_blocks;
            let free_blk = g_total.saturating_sub(overhead);

            let used_ino = if g == 0 { 11u32 } else { 0u32 };
            let free_ino = inodes_per_group.saturating_sub(used_ino);

            groups[g] = GroupLayout {
                start_block:           g_start,
                has_sb_copy:           has_sb,
                sb_overhead:           sb_over,
                block_bitmap,
                inode_bitmap,
                inode_table,
                data_start,
                total_blocks_in_group: g_total,
                free_blocks:           free_blk,
                free_inodes:           free_ino,
            };
        }

        Self {
            fs_type: params.fs_type,
            block_size,
            inode_size,
            sectors_per_block,
            first_data_block,
            gdt_block,
            group_count,
            blocks_per_group,
            inodes_per_group,
            inode_table_blocks,
            total_blocks,
            total_inodes,
            reserved_blocks,
            journal_blocks,
            groups,
        }
    }

    pub fn total_free_blocks(&self) -> u32 {
        self.groups[..self.group_count as usize]
            .iter()
            .map(|g| g.free_blocks)
            .sum()
    }

    pub fn total_free_inodes(&self) -> u32 {
        self.groups[..self.group_count as usize]
            .iter()
            .map(|g| g.free_inodes)
            .sum()
    }

    pub fn block_to_group(&self, block: u32) -> (usize, u32) {
        let rel   = block.saturating_sub(self.first_data_block);
        let g     = (rel / self.blocks_per_group) as usize;
        let local = rel % self.blocks_per_group;
        (g, local)
    }
}

