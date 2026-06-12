use crate::ata::AtaDrive;
use crate::gpt::{
    self, GptReadError, GptWriteError,
    GUID_LINUX_FS, GUID_LINUX_SWAP,
};
use crate::swap;
use crate::{cprintln, print_error, print_success, print_warn, print_info, println};

fn parse_drive(s: &str) -> Option<usize> {
    match s.parse::<usize>() {
        Ok(n) if n <= 7 => Some(n),
        _ => None,
    }
}

/// Resolve a drive index to a block-layer device id: 0-3 are the legacy ATA
/// slots (registered on first use), 4-7 are PCI block devices (virtio-blk).
fn blk_dev(idx: usize) -> crate::vfs::types::BlockDevId {
    if idx < 4 {
        crate::block::register_ata(AtaDrive::from_idx(idx))
    } else {
        crate::block::probe();
        idx as crate::vfs::types::BlockDevId
    }
}

/// Probe all four ATA slots into the block layer and report their identity
/// (from IDENTIFY) plus the live bio-queue counters.
pub fn cmd_blkstat() {
    cprintln!(57, 197, 187, "  Block layer devices");

    // Make sure both the ATA slots and any PCI block devices are registered.
    for idx in 0..4 {
        let _ = crate::block::register_ata(AtaDrive::from_idx(idx));
    }
    crate::block::probe();

    let mut any = false;
    for id in 0..crate::vfs::types::MAX_BLOCK_DEVICES as u8 {
        if let Some(info) = crate::block::info(id) {
            if info.total_sectors == 0 && info.model_len == 0 {
                continue;
            }
            any = true;
            let mb = info.total_sectors * info.sector_size as u64 / (1024 * 1024);
            let Some(st) = crate::block::dev_stats(id) else { continue };
            let kind_str = match st.kind {
                crate::block::DevKind::Ata       => "ata",
                crate::block::DevKind::VirtioBlk => "virtio",
                crate::block::DevKind::Ahci      => "ahci",
                crate::block::DevKind::Nvme      => "nvme",
            };
            println!(
                "  blk{} [{}]: '{}' {} sectors ({} MB) lba48={} ro={} discard={}",
                id, kind_str, info.model_str(), info.total_sectors, mb,
                info.lba48, info.read_only, info.discard
            );
            println!(
                "        io: read {} KB, written {} KB, discarded {} KB, avg latency {} us",
                st.sectors_read * 512 / 1024,
                st.sectors_written * 512 / 1024,
                st.sectors_discarded * 512 / 1024,
                st.avg_io_us
            );
            if st.io_errors > 0 || st.retries > 0 {
                print_warn!(
                    "        errors: {} failed, {} retries",
                    st.io_errors, st.retries
                );
            }
            if let Ok(tbl) = gpt::gpt_read(id) {
                for (i, e) in tbl.entries.iter().enumerate() {
                    if !e.is_used() { continue; }
                    let mut nbuf = [0u8; 36];
                    let nlen = e.name_str(&mut nbuf);
                    let name = core::str::from_utf8(&nbuf[..nlen]).unwrap_or("?");
                    let start = { e.start_lba };
                    let end   = { e.end_lba };
                    println!(
                        "        |- blk{}p{} {} {} MB lba {}..{} '{}'",
                        id, i + 1, e.type_name(), e.size_mb(), start, end, name
                    );
                }
            }
        }
    }
    if !any {
        print_warn!("  no block devices detected");
    }

    let (sub, done, err) = crate::block::io_stats();
    println!("  bio queue: submitted={} completed={} errors={}", sub, done, err);

    let (hits, misses, ra, merges, dirty) = crate::block::cache_stats();
    let total = hits + misses;
    let pct = if total > 0 { hits * 100 / total } else { 0 };
    println!(
        "  buffer cache: hits={} misses={} ({}% hit rate), readaheads={}, write merges={}, dirty={}",
        hits, misses, pct, ra, merges, dirty
    );
}

/// fstrim(8) analogue: discard every free block of the active mounted ext
/// filesystem, walking the group bitmaps, so the device can unmap the space
pub fn cmd_fstrim() {
    enum Outcome {
        Unsupported(crate::vfs::types::BlockDevId),
        Failed(crate::vfs::types::BlockDevId, crate::miku_extfs::FsError),
        Done(crate::vfs::types::BlockDevId, crate::miku_extfs::trim::TrimReport),
    }

    let res = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        let dev = fs.reader.dev_id;
        if !crate::block::info(dev).map_or(false, |i| i.discard) {
            return Outcome::Unsupported(dev);
        }
        match fs.trim_free_blocks(1) {
            Ok(r)  => Outcome::Done(dev, r),
            Err(e) => Outcome::Failed(dev, e),
        }
    });

    match res {
        None => print_error!("  fstrim: no mounted ext filesystem (use ext2/3/4mount first)"),
        Some(Outcome::Unsupported(dev)) => {
            print_warn!("  fstrim: blk{} does not support discard/TRIM", dev);
        }
        Some(Outcome::Failed(dev, e)) => {
            print_error!("  fstrim: blk{}: {:?}", dev, e);
        }
        Some(Outcome::Done(dev, r)) => {
            print_success!(
                "  blk{}: {} MB ({} blocks) trimmed",
                dev, r.trimmed_bytes / (1024 * 1024), r.trimmed_blocks
            );
        }
    }
}

/// Discard (TRIM) a sector range, or the whole device when no range is
/// given - MikuOS's blkdiscard(8)
pub fn cmd_blkdiscard(args: &str) {
    let mut it = args.split_whitespace();
    let Some(idx) = it.next().and_then(parse_drive) else {
        print_error!("  usage: blkdiscard <drive 0-7> [lba count]");
        return;
    };
    let dev = blk_dev(idx);
    let Some(info) = crate::block::info(dev) else {
        print_error!("  no device blk{}", idx);
        return;
    };
    if !info.discard {
        print_warn!("  blk{}: device does not support discard/TRIM", idx);
        return;
    }

    let (lba, sectors) = match (it.next(), it.next()) {
        (Some(l), Some(c)) => {
            let (Ok(l), Ok(c)) = (l.parse::<u64>(), c.parse::<u64>()) else {
                print_error!("  usage: blkdiscard <drive 0-7> [lba count]");
                return;
            };
            (l, c)
        }
        (None, _) => (0, info.total_sectors),
        _ => {
            print_error!("  usage: blkdiscard <drive 0-7> [lba count]");
            return;
        }
    };

    let mut done = 0u64;
    while done < sectors {
        let n = (sectors - done).min(u32::MAX as u64) as u32;
        match crate::block::discard(dev, lba + done, n) {
            Ok(()) => done += n as u64,
            Err(e) => {
                print_error!("  blk{}: discard failed at lba {}: {:?}", idx, lba + done, e);
                return;
            }
        }
    }
    print_success!(
        "  blk{}: discarded {} sectors ({} MB) at lba {}",
        idx, sectors, sectors * 512 / (1024 * 1024), lba
    );
}

/// SMART / health report for a block device
pub fn cmd_smart(drive_str: &str) {
    let idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: smart <drive 0-7>"); return; }
    };
    let dev = blk_dev(idx);

    let Some(info) = crate::block::info(dev) else {
        print_error!("  no device blk{}", idx);
        return;
    };
    cprintln!(57, 197, 187, "  SMART / health: blk{} '{}'", dev, info.model_str());

    match crate::block::health(dev) {
        None => print_warn!("  device reports no health data"),
        Some(h) => {
            if h.healthy {
                print_success!("  overall status: healty");
            } else {
                print_error!("  overall status: failing - back up your data!");
            }
            if h.temp_c != i16::MIN {
                println!("  temperature:    {} C", h.temp_c);
            }
            if h.percent_used != 0xFF {
                println!("  wear used:      {}%", h.percent_used);
            }
            if h.power_on_hours != u64::MAX {
                println!("  power-on hours: {}", h.power_on_hours);
            }
            if h.data_read_mb != u64::MAX {
                println!("  lifetime read:  {} MB", h.data_read_mb);
            }
            if h.data_written_mb != u64::MAX {
                println!("  lifetime write: {} MB", h.data_written_mb);
            }
        }
    }
}

pub fn cmd_gpt_show(drive_str: &str) {
    let idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: gpt <drive 0-3>"); return; }
    };
    let dev = blk_dev(idx);

    let tbl = match gpt::gpt_read(dev) {
        Ok(t)  => t,
        Err(GptReadError::NotGpt)       => { print_error!("  disk {} has no GPT", idx); return; }
        Err(GptReadError::InvalidFormat) => { print_error!("  corrupted GPT header"); return; }
        Err(GptReadError::Io(e))        => { print_error!("  I/O error: {:?}", e); return; }
    };

    cprintln!(57, 197, 187, "  GPT partition table -- disk {}", idx);
    let total_mb = tbl.total_sectors * 512 / (1024 * 1024);
    println!("  Disk size: {} sectors ({} MB)", tbl.total_sectors, total_mb);
    let first_lba  = { tbl.header.first_usable_lba };
    let last_lba   = { tbl.header.last_usable_lba  };
    println!("  First LBA: {}", first_lba);
    println!("  Last  LBA: {}", last_lba);
    println!();

    let mut found = false;
    for (i, e) in tbl.entries.iter().enumerate() {
        if !e.is_used() { continue; }
        found = true;
        let mut nbuf = [0u8; 36];
        let nlen = e.name_str(&mut nbuf);
        let name = core::str::from_utf8(&nbuf[..nlen]).unwrap_or("???");
        let size_mb   = e.size_mb();
        let start_lba = { e.start_lba };
        let end_lba   = { e.end_lba   };
        let color = if e.is_swap() { (255u8, 200u8, 80u8) } else { (128u8, 222u8, 217u8) };
        cprintln!(color.0, color.1, color.2,
            "  [{:3}]  {:10}  {:>9}-{:<9}  {:4} MB  {}",
            i + 1, e.type_name(), start_lba, end_lba, size_mb, name);
    }

    if !found {
        print_info!("  (no partitions)");
    }
}

pub fn cmd_gpt_init(drive_str: &str) {
    let idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: gpt.init <drive 0-3>"); return; }
    };
    let dev = blk_dev(idx);

    let total_sectors = gpt::gpt_probe_sectors(dev);
    if total_sectors < 2048 {
        print_error!("  disk {} is too small or not available", idx);
        return;
    }

    cprintln!(255, 80, 80, "warning: GPT will overwrite LBA 0-33 and tail sectors");
    cprintln!(255, 80, 80, "data in those sectors will be destroyed");
    println!();

    match gpt::gpt_init(dev, total_sectors) {
        Ok(()) => {
            print_success!("  GPT initialized on disk {}", idx);
            println!("  Total sectors: {}", total_sectors);
            println!("  Usable:  {} MB", total_sectors * 512 / (1024 * 1024));
        }
        Err(e) => { print_error!("  I/O error: {:?}", e); }
    }
}

pub fn cmd_gpt_add(args: &str) {
    let mut parts = args.split_whitespace();
    let drive_str = parts.next().unwrap_or("");
    let type_str  = parts.next().unwrap_or("");
    let size_str  = parts.next().unwrap_or("");
    let name      = parts.next().unwrap_or("partition");

    let idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => {
            print_error!("  usage: gpt.add <drive> <fs|swap> <size_mb> [name]");
            return;
        }
    };

    let (type_guid, type_label) = match type_str {
        "fs"   | "linux" => (GUID_LINUX_FS,   "Linux FS"),
        "swap"           => (GUID_LINUX_SWAP, "Linux Swap"),
        _ => {
            print_error!("  type must be: fs or swap");
            return;
        }
    };

    let size_mb: u64 = match size_str.parse() {
        Ok(n) if n > 0 => n,
        _ => { print_error!("  invalid size: '{}'", size_str); return; }
    };

    let size_sectors = size_mb * 1024 * 1024 / 512;

    match gpt::gpt_add_partition(blk_dev(idx), type_guid, size_sectors, name, 0xABCD1234) {
        Ok(slot) => {
            print_success!("  partition added: slot {}", slot);
            println!("  Type:  {}", type_label);
            println!("  Size:  {} MB", size_mb);
            println!("  Name:  {}", name);
            if type_guid == GUID_LINUX_SWAP {
                print_info!("  Next run: mkswap {} {}", idx, slot);
            } else {
                print_info!("  Next run: mkfs.ext4 {} [sectors]", idx);
            }
        }
        Err(GptWriteError::NotEnoughSpace) => print_error!("  not enough space on disk"),
        Err(GptWriteError::NoFreeSlot)     => print_error!("  no free GPT slots (max 128)"),
        Err(GptWriteError::ReadFailed)     => print_error!("  GPT read failed - run gpt.init first"),
        Err(GptWriteError::InvalidIndex)   => print_error!("  invalid partition index"),
        Err(GptWriteError::Io(e))          => print_error!("  I/O error: {:?}", e),
    }
}

pub fn cmd_gpt_del(drive_str: &str, index_str: &str) {
    let idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: gpt.del <drive 0-3> <partition>"); return; }
    };
    let part_num: usize = match index_str.parse() {
        Ok(n) if n >= 1 => n,
        _ => { print_error!("  invalid partition number (must be >= 1)"); return; }
    };
    let part_idx = part_num - 1;

    match gpt::gpt_del_partition(blk_dev(idx), part_idx) {
        Ok(()) => print_success!("  partition {} deleted from disk {}", part_num, idx),
        Err(GptWriteError::InvalidIndex) => print_error!("  partition {} does not exist", part_num),
        Err(GptWriteError::ReadFailed)   => print_error!("  GPT read failed"),
        Err(GptWriteError::Io(e))        => print_error!("  I/O error: {:?}", e),
        Err(e) => print_error!("  error: {:?}", e),
    }
}

pub fn cmd_mkswap(drive_str: &str, part_str: &str) {
    let drive_idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: mkswap <drive 0-3> <partition>"); return; }
    };
    let part_num: usize = match part_str.parse() {
        Ok(n) if n >= 1 => n,
        _ => { print_error!("  invalid partition number (must be >= 1)"); return; }
    };
    let part_idx = part_num - 1;

    let dev = blk_dev(drive_idx);

    let tbl = match gpt::gpt_read(dev) {
        Ok(t)  => t,
        Err(_) => { print_error!("  could not read GPT - run gpt.init first"); return; }
    };

    if part_idx >= 128 || !tbl.entries[part_idx].is_used() {
        print_error!("  partition {} does not exist", part_num);
        return;
    }

    let entry = &tbl.entries[part_idx];
    if entry.type_guid != GUID_LINUX_SWAP {
        print_error!("  partition {} is not a swap partition (type: {})", part_num, entry.type_name());
        print_error!("  add a swap partition with: gpt.add {} swap <size_mb> [name]", drive_idx);
        return;
    }

    let partition_lba     = entry.start_lba as u32;
    let partition_sectors = entry.size_sectors() as u32;

    match swap::mkswap(dev, partition_lba, partition_sectors, "miku-swap") {
        Ok(()) => {
            print_success!("  swap formatted: partition {} on disk {}", part_num, drive_idx);
            println!("  Size:  {} MB", partition_sectors as u64 * 512 / (1024 * 1024));
            println!("  Pages:  {}", partition_sectors / 8);
            print_info!("  Activate: swapon {} {}", drive_idx, part_num);
        }
        Err(e) => { print_error!("  mkswap error: {:?}", e); }
    }
}

pub fn cmd_swapon(drive_str: &str, part_str: &str) {
    let drive_idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: swapon <drive 0-3> <partition>"); return; }
    };
    let part_num: usize = match part_str.parse() {
        Ok(n) if n >= 1 => n,
        _ => { print_error!("  invalid partition number (must be >= 1)"); return; }
    };
    let part_idx = part_num - 1;

    let dev = blk_dev(drive_idx);

    let tbl = match gpt::gpt_read(dev) {
        Ok(t)  => t,
        Err(_) => { print_error!("  could not read GPT"); return; }
    };

    if part_idx >= 128 || !tbl.entries[part_idx].is_used() {
        print_error!("  partition {} does not exist", part_num);
        return;
    }

    let entry = &tbl.entries[part_idx];
    if entry.type_guid != GUID_LINUX_SWAP {
        print_error!("  partition {} is not swap type (type: {})", part_num, entry.type_name());
        return;
    }

    let partition_lba     = entry.start_lba as u32;
    let partition_sectors = entry.size_sectors() as u32;

    match swap::swapon(dev, partition_lba, partition_sectors) {
        Ok(pages) => {
            print_success!("  swap activated");
            println!("  Drive:   {}", drive_idx);
            println!("  Part:    {}", part_num);
            println!("  Pages:  {}", pages);
            println!("  Size:    {} MB", pages as u64 * 4096 / (1024 * 1024));
        }
        Err(swap::SwapError::AlreadyActive)      => print_error!("  swap already active - run swapoff first"),
        Err(swap::SwapError::InvalidMagic)       => print_error!("  no swap signature - run mkswap {} {} first", drive_idx, part_idx),
        Err(swap::SwapError::UnsupportedVersion) => print_error!("  unsupported swap header version"),
        Err(swap::SwapError::Io(e))              => print_error!("  I/O error: {:?}", e),
        Err(e) => print_error!("  swapon error: {:?}", e),
    }
}

pub fn cmd_swapoff() {
    match swap::swapoff() {
        Ok(()) => print_success!("  swap deactivated"),
        Err(swap::SwapError::NotActive) => print_error!("  swap is not active"),
        Err(swap::SwapError::SwapInUse) => {
            print_error!("  swap is in use - cannot deactivate");
            print_warn!("  free all swap pages before disabling");
        }
        Err(e) => print_error!("  swapoff error: {:?}", e),
    }
}

pub fn cmd_swapinfo() {
    if !swap::swap_is_active() {
        print_warn!("  swap is not active");
        return;
    }
    cprintln!(57, 197, 187, "  Swap info");
    println!("  Total:  {} KB ({} MB)", swap::swap_total_kb(), swap::swap_total_kb() / 1024);
    println!("  Used:   {} KB", swap::swap_used_kb());
    println!("  Free:   {} KB", swap::swap_free_kb());
    let total = swap::swap_total_pages();
    let used  = swap::swap_used_pages();
    let pct   = if total > 0 { used * 100 / total } else { 0 };
    println!("  Pages:   {}/{} ({}%)", used, total, pct);
    if pct > 80 {
        print_warn!("  warning: swap is more than 80% full");
    }
}

pub fn cmd_mkswap_raw(args: &str) {
    let mut parts = args.split_whitespace();
    let drive_str = parts.next().unwrap_or("");
    let lba_str   = parts.next().unwrap_or("");
    let size_str  = parts.next().unwrap_or("");

    let drive_idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: mkswap.raw <drive 0-3> <start_lba> <size_mb>"); return; }
    };
    let start_lba: u32 = match lba_str.parse() {
        Ok(n) => n,
        Err(_) => { print_error!("  invalid LBA: '{}'", lba_str); return; }
    };
    let size_mb: u32 = match size_str.parse::<u32>() {
        Ok(n) if n > 0 => n,
        _ => { print_error!("  invalid size: '{}'", size_str); return; }
    };

    let size_sectors = size_mb * 1024 * 1024 / 512;

    match swap::mkswap(blk_dev(drive_idx), start_lba, size_sectors, "miku-swap") {
        Ok(()) => {
            print_success!("  swap formatted on drive {} LBA {} size {} MB", drive_idx, start_lba, size_mb);
            print_info!("  Activate: swapon.raw {} {} {}", drive_idx, start_lba, size_sectors);
        }
        Err(e) => { print_error!("  mkswap.raw error: {:?}", e); }
    }
}

pub fn cmd_swapon_raw(args: &str) {
    let mut parts = args.split_whitespace();
    let drive_str   = parts.next().unwrap_or("");
    let lba_str     = parts.next().unwrap_or("");
    let sectors_str = parts.next().unwrap_or("");

    let drive_idx = match parse_drive(drive_str) {
        Some(i) => i,
        None => { print_error!("  usage: swapon.raw <drive 0-3> <start_lba> <size_sectors>"); return; }
    };
    let start_lba: u32 = match lba_str.parse() {
        Ok(n) => n,
        Err(_) => { print_error!("  invalid LBA"); return; }
    };
    let size_sectors: u32 = match sectors_str.parse() {
        Ok(n) if n > 0 => n,
        _ => { print_error!("  invalid size_sectors"); return; }
    };

    match swap::swapon(blk_dev(drive_idx), start_lba, size_sectors) {
        Ok(pages) => {
            print_success!("  swap activated  drive={} lba={}", drive_idx, start_lba);
            crate::println!("  Pages: {}  Size: {} MB", pages, pages as u64 * 4096 / (1024 * 1024));
        }
        Err(swap::SwapError::AlreadyActive)      => print_error!("  swap already active - run swapoff first"),
        Err(swap::SwapError::InvalidMagic)       => print_error!("  no swap signature - run mkswap.raw {} {} {} first", drive_idx, start_lba, size_sectors),
        Err(swap::SwapError::UnsupportedVersion) => print_error!("  unsupported swap header version"),
        Err(e) => print_error!("  swapon.raw error: {:?}", e),
    }
}

pub fn cmd_swapon_auto() {
    if swap::swap_is_active() {
        print_error!("  swap already active");
        return;
    }

    crate::println!("  Scanning drives for swap...");

    for drive_idx in 0..4usize {
        let dev = blk_dev(drive_idx);

        let mut probe = [0u8; 512];
        if crate::block::read(dev, 0, 1, &mut probe).is_err() {
            continue;
        }

        if let Ok(tbl) = gpt::gpt_read(dev) {
            for entry in tbl.entries.iter() {
                if !entry.is_used() || !entry.is_swap() { continue; }
                let lba      = entry.start_lba as u32;
                let sectors  = entry.size_sectors() as u32;
                match swap::swapon(dev, lba, sectors) {
                    Ok(pages) => {
                        print_success!("  swap found and activated on drive {}", drive_idx);
                        crate::println!("  Pages: {}  Size: {} MB", pages, pages as u64 * 4096 / (1024*1024));
                        return;
                    }
                    Err(swap::SwapError::InvalidMagic) => {}
                    Err(e) => { print_error!("  swapon error on drive {}: {:?}", drive_idx, e); }
                }
            }
        }

        let total = gpt::gpt_probe_sectors(dev);
        if total > 16 {
            if let Ok(pages) = swap::swapon(dev, 0, total.min(u32::MAX as u64) as u32) {
                print_success!("  whole-disk swap activated on drive {}", drive_idx);
                crate::println!("  Pages: {}  Size: {} MB", pages, pages as u64 * 4096 / (1024*1024));
                return;
            }
        }
    }

    print_error!("  no swap found on any drive");
    crate::println!("  Hint: mkswap <drive> <part>  or  mkswap.raw <drive> <lba> <mb>");
}
