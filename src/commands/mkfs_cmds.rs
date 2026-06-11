use crate::ata::AtaDrive;
use crate::mkfs::{mkfs, FsType, MkfsError, MkfsParams};
use crate::mkfs::layout::FsLayout;
use crate::vfs::types::BlockDevId;
use crate::{cprintln, print_error, print_success, print_warn, println};

fn dev_from_index(i: usize) -> Option<BlockDevId> {
    match i {
        0..=3 => Some(crate::block::register_ata(AtaDrive::from_idx(i))),
        4..=7 => {
            crate::block::probe();
            Some(i as BlockDevId)
        }
        _ => None,
    }
}

fn parse_drive(s: &str) -> Option<usize> {
    match s.parse::<usize>() {
        Ok(n) if n <= 7 => Some(n),
        _ => None,
    }
}

pub fn cmd_mkfs_dry(drive_str: &str, type_str: &str) {
    let drive_idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => {
            print_error!("  usage: mkfs.dry <drive 0-3> <ext2|ext3|ext4>");
            return;
        }
    };
    let fs_type = match type_str {
        "ext2" => FsType::Ext2,
        "ext3" => FsType::Ext3,
        "ext4" => FsType::Ext4,
        _ => {
            print_error!("  type must be ext2, ext3, or ext4");
            return;
        }
    };

    let params = MkfsParams::new(fs_type, drive_idx);

    let dev = match dev_from_index(drive_idx) {
        Some(d) => d,
        None => { print_error!("  invalid drive index"); return; }
    };

    let total_sectors = probe_sectors(dev);
    if total_sectors < 2048 {
        print_error!("  drive {} appears empty or too small", drive_idx);
        return;
    }

    let layout = FsLayout::compute(&params, total_sectors);

    cprintln!(57, 197, 187, "  mkfs dry-run: {} on drive {}", fs_type.name(), drive_idx);
    println!("  Disk:          {} sectors ({} MB)", total_sectors, total_sectors / 2048);
    println!("  Block size:    {} bytes", layout.block_size);
    println!("  Inode size:    {} bytes", layout.inode_size);
    println!("  Total blocks:  {}", layout.total_blocks);
    println!("  Total inodes:  {}", layout.total_inodes);
    println!("  Groups:        {}", layout.group_count);
    println!("  Inodes/group:  {}", layout.inodes_per_group);
    println!("  IT blocks/grp: {}", layout.inode_table_blocks);
    if fs_type.needs_journal() {
        println!("  Journal:       {} blocks ({} KB)",
            layout.journal_blocks, layout.journal_blocks * layout.block_size / 1024);
    }
    println!("  Free blocks:   {} ({} MB)",
        layout.total_free_blocks(),
        layout.total_free_blocks() * layout.block_size / (1024 * 1024));
    println!("  Free inodes:   {}", layout.total_free_inodes());
    println!("  Reserved (5%): {}", layout.reserved_blocks);
    println!();
    println!("  Group layout:");
    for g in 0..layout.group_count as usize {
        let gl = &layout.groups[g];
        println!("  [{}] start={} bitmap={} imap={} itable={} data_start={} free={}",
            g, gl.start_block, gl.block_bitmap, gl.inode_bitmap,
            gl.inode_table, gl.data_start, gl.free_blocks);
    }
    println!();
    print_warn!("  (dry-run: nothing was written)");
}

pub fn cmd_mkfs_ext2(args: &str) { do_mkfs(args, FsType::Ext2); }
pub fn cmd_mkfs_ext3(args: &str) { do_mkfs(args, FsType::Ext3); }
pub fn cmd_mkfs_ext4(args: &str) { do_mkfs(args, FsType::Ext4); }

fn do_mkfs(args: &str, fs_type: FsType) {
    let mut parts       = args.split_whitespace();
    let drive_str       = parts.next().unwrap_or("");
    let second_str      = parts.next().unwrap_or("0");

    let drive_idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => {
            print_error!("  usage: mkfs.{} <drive 0-3> [partition|sectors]", fs_type.name());
            println!("    drive 0 = primary master");
            println!("    drive 1 = primary slave");
            println!("    drive 2 = secondary master");
            println!("    drive 3 = secondary slave");
            println!("    example (whole disk):  mkfs.ext4 1");
            println!("    example (partition 2): mkfs.ext4 1 2");
            return;
        }
    };

    let dev = match dev_from_index(drive_idx) {
        Some(d) => d,
        None => { print_error!("  invalid drive index"); return; }
    };

    let mut params = MkfsParams::new(fs_type, drive_idx);

    let second_val: u32 = second_str.parse().unwrap_or(0);

    if second_val >= 1 && second_val <= 128 {
        let part_num = second_val as usize;
        match crate::gpt::gpt_read(dev) {
            Ok(tbl) => {
                let entry = &tbl.entries[part_num - 1];
                if !entry.is_used() {
                    print_error!("  partition {} does not exist", part_num);
                    return;
                }
                if entry.is_swap() {
                    print_error!("  partition {} is swap type - use mkswap instead", part_num);
                    return;
                }
                params.start_lba     = entry.start_lba as u32;
                params.total_sectors = entry.size_sectors() as u32;
                cprintln!(255, 80, 80,
                    "  !! warning: partition {} on drive {} will be erased!!",
                    part_num, drive_idx
                );
                cprintln!(128, 222, 217,
                    "  start_lba={} sectors={}",
                    params.start_lba, params.total_sectors
                );
            }
            Err(_) => {
                print_error!("  could not read GPT on drive {} - treating as manual sectors", drive_idx);
                params.total_sectors = second_val;
            }
        }
    } else if second_val > 128 {
        params.total_sectors = second_val;
        cprintln!(255, 80, 80, "  !! warning: drive {} will be erased (manual {} sectors)!!", drive_idx, second_val);
    } else {
        cprintln!(255, 80, 80, "  !! warning: drive {} will be fully erased!!", drive_idx);
    }

    crate::commands::ext2_cmds::invalidate_drive_mounts(drive_idx, params.start_lba);

    println!();
    cprintln!(57, 197, 187, "  mkfs.{} on drive {}...", fs_type.name(), drive_idx);

    match mkfs(dev, &params) {
        Ok(report) => {
            println!();
            print_success!("  {} filesystem created successfully", report.fs_type);
            println!("  Block size:    {} bytes",  report.block_size);
            println!("  Inode size:    {} bytes",  report.inode_size);
            println!("  Total blocks:  {}",        report.total_blocks);
            println!("  Total inodes:  {}",        report.total_inodes);
            println!("  Groups:        {}",        report.group_count);
            if report.journal_blocks > 0 {
                println!("  Journal:       {} blocks ({} KB)",
                    report.journal_blocks,
                    report.journal_blocks * report.block_size / 1024);
            }
            println!("  Free blocks:   {} ({} MB)",
                report.free_blocks,
                report.free_blocks * report.block_size / (1024 * 1024));
            println!("  Free inodes:   {}", report.free_inodes);
            println!();
            if params.start_lba > 0 {
                cprintln!(128, 222, 217,
                    "  Mount with: {}mount {} (partition)",
                    report.fs_type, drive_idx
                );
            } else {
                cprintln!(128, 222, 217, "  Mount with: {}mount", report.fs_type);
            }
        }
        Err(MkfsError::DiskTooSmall)        => print_error!("  error: disk too small for {}", fs_type.name()),
        Err(MkfsError::InvalidParams(msg))  => print_error!("  error: invalid params: {}", msg),
        Err(MkfsError::TooManyGroups)       => print_error!("  error: too many block groups (max 32)"),
        Err(MkfsError::Io(e)) => {
            print_error!("  I/O error during format: {:?}", e);
            print_error!("  The disk may be in an inconsistent state.");
            print_error!("  Do NOT attempt to mount it.");
        }
    }
}

fn probe_sectors(dev: BlockDevId) -> u32 {
    crate::gpt::gpt_probe_sectors(dev).min(u32::MAX as u64) as u32
}
