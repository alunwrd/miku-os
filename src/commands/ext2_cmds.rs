use crate::ata::AtaDrive;
use crate::miku_extfs::ext2::write::TreeResult;
use crate::miku_extfs::ext3::journal::{TxnTag, DEFAULT_JOURNAL_BLOCKS};
use crate::miku_extfs::reader::DiskReader;
use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};
use crate::{cprint, cprintln, print_error, print_success, println, serial_println};
use crate::vfs::path::split_parent_name;
use spin::Mutex;

const MAX_MOUNTS: usize = 4;

const EMPTY_FS: MikuFS = MikuFS {
    superblock:       Superblock { data: [0; 1024] },
    block_size:       0,
    inodes_per_group: 0,
    blocks_per_group: 0,
    group_count:      0,
    groups:           [GroupDesc { data: [0; 64] }; 32],
    reader: DiskReader {
        drive:     AtaDrive::EMPTY,
        start_lba: 0,
        io_count:  0,
    },
    journal_seq:      0,
    journal_pos:      0,
    journal_maxlen:   0,
    journal_first:    0,
    journal_active:   false,
    txn_active:       false,
    txn_desc_pos:     0,
    txn_tags:         [TxnTag { fs_block: 0, journal_pos: 0 }; 64],
    txn_tag_count:    0,
    txn_revokes:      [0; 128],
    txn_revoke_count: 0,
    block_cache:      None,
    superblock_dirty: false,
    groups_dirty:     [false; 32],
    last_sync_ticks:  0,
    journal_inode_cached: None,
    alloc_hint: [0u32; 32],
};

struct ExtFsState {
    slots:       [MikuFS; MAX_MOUNTS],
    ready:       [bool; MAX_MOUNTS],
    drive_idx:   [usize; MAX_MOUNTS],
    start_lba:   [u32; MAX_MOUNTS],
    active_slot: usize,
}

impl ExtFsState {
    const fn new() -> Self {
        Self {
            slots:       [EMPTY_FS; MAX_MOUNTS],
            ready:       [false; MAX_MOUNTS],
            drive_idx:   [0; MAX_MOUNTS],
            start_lba:   [0; MAX_MOUNTS],
            active_slot: 0,
        }
    }

    fn active_fs(&mut self) -> Option<&mut MikuFS> {
        let slot = self.active_slot;
        if self.ready[slot] { Some(&mut self.slots[slot]) } else { None }
    }

    fn find_free_slot(&self) -> Option<usize> {
        self.ready.iter().position(|&r| !r)
    }

    fn is_already_mounted(&self, drive: usize, lba: u32) -> bool {
        for i in 0..MAX_MOUNTS {
            if self.ready[i] && self.drive_idx[i] == drive && self.start_lba[i] == lba {
                return true;
            }
        }
        false
    }
}

static STATE: Mutex<ExtFsState> = Mutex::new(ExtFsState::new());

fn with_ext2<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut MikuFS) -> R,
{
    STATE.lock().active_fs().map(f)
}

pub fn is_ext2_ready() -> bool {
    let state = STATE.lock();
    state.ready[state.active_slot]
}

pub fn active_fs_type() -> crate::vfs::types::FsType {
    let state = STATE.lock();
    let slot = state.active_slot;
    if !state.ready[slot] {
        return crate::vfs::types::FsType::Ext2;
    }
    match state.slots[slot].superblock.fs_version_str() {
        "ext4" => crate::vfs::types::FsType::Ext4,
        "ext3" => crate::vfs::types::FsType::Ext3,
        _      => crate::vfs::types::FsType::Ext2,
    }
}

pub fn ext_fs_version_tag() -> &'static str {
    let state = STATE.lock();
    let slot = state.active_slot;
    if !state.ready[slot] { return "ext"; }
    state.slots[slot].superblock.fs_version_str()
}

pub fn with_ext2_pub<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut MikuFS) -> R,
{
    STATE.lock().active_fs().map(f)
}

fn invalidate_vfs_ext_mounts() {
    let mut dropped_any = false;

    crate::vfs::core::with_vfs(|vfs| {
        for id in 0..crate::vfs::MAX_VNODES {
            if !vfs.nodes[id].active {
                continue;
            }
            if !vfs.nodes[id].fs_type.is_ext_family() {
                continue;
            }
            if !vfs.nodes[id].is_dir() || vfs.nodes[id].ext2_ino != EXT2_ROOT_INO {
                continue;
            }

            vfs.evict_children_recursive(id);
            vfs.nodes[id].fs_type = crate::vfs::FsType::TmpFS;
            vfs.nodes[id].ext2_ino = 0;
            vfs.nodes[id].children_loaded = false;
            dropped_any = true;
        }

        if dropped_any {
            vfs.ext2_mount_active = false;
        }
    });

    if dropped_any {
        let mut s = crate::shell::SESSION.lock();
        s.cwd = 0;
        s.path[0] = b'/';
        s.plen = 1;
    }
}

pub fn force_unmount() {
    let mut state = STATE.lock();
    let slot = state.active_slot;
    state.ready[slot] = false;
    state.slots[slot].block_cache = None;
    state.slots[slot].journal_inode_cached = None;
    drop(state);
    invalidate_vfs_ext_mounts();
}

pub fn cmd_fs_list() {
    let state = STATE.lock();
    cprintln!(57, 197, 187, "  Mounted filesystems:");
    let mut any = false;
    for slot in 0..MAX_MOUNTS {
        if state.ready[slot] {
            any = true;
            let version = state.slots[slot].superblock.fs_version_str();
            let drive   = state.drive_idx[slot];
            let lba     = state.start_lba[slot];
            let free_b  = state.slots[slot].superblock.free_blocks_count();
            let total_b = state.slots[slot].superblock.blocks_count();
            let bs      = state.slots[slot].block_size;
            let marker  = if slot == state.active_slot { " <- active" } else { "" };
            println!(
                "  [{}] {} drive={} lba={} free={}/{} ({} MB){}",
                slot, version, drive, lba,
                free_b, total_b,
                free_b as u64 * bs as u64 / (1024 * 1024),
                marker
            );
        } else {
            let marker = if slot == state.active_slot { " <- active" } else { "" };
            println!("  [{}] <empty>{}", slot, marker);
        }
    }
    if !any {
        crate::print_warn!("  no filesystems mounted");
    }
}

pub fn cmd_fs_select(args: &str) {
    let slot: usize = match args.trim().parse() {
        Ok(n) if n < MAX_MOUNTS => n,
        _ => { print_error!("  usage: fs.select <0|1>"); return; }
    };
    let mut state = STATE.lock();
    if !state.ready[slot] {
        crate::print_warn!("  slot {} is empty - switching anyway", slot);
    }
    state.active_slot = slot;
    print_success!("  active slot = {}", slot);
    if state.ready[slot] {
        let version = state.slots[slot].superblock.fs_version_str();
        let drive   = state.drive_idx[slot];
        let lba     = state.start_lba[slot];
        println!("  {} on drive {} lba={}", version, drive, lba);
    }
}

pub fn cmd_fs_umount(args: &str) {
    let mut state = STATE.lock();
    let slot: usize = if args.trim().is_empty() {
        state.active_slot
    } else {
        match args.trim().parse() {
            Ok(n) if n < MAX_MOUNTS => n,
            _ => { print_error!("  usage: fs.umount [0|1]"); return; }
        }
    };
    if !state.ready[slot] {
        crate::print_warn!("  slot {} is already empty", slot);
        return;
    }
    let _ = state.slots[slot].sync();
    state.slots[slot].mark_clean_unmount();
    let _ = state.slots[slot].flush_all_dirty_metadata();
    state.ready[slot] = false;
    state.slots[slot].block_cache = None;
    state.slots[slot].journal_inode_cached = None;
    print_success!("  slot {} unmounted", slot);
    if state.active_slot == slot {
        let other = 1 - slot;
        if state.ready[other] {
            state.active_slot = other;
            println!("  active slot switched to {}", other);
        }
    }
    drop(state);
    invalidate_vfs_ext_mounts();
}

fn resolve_parent_and_name<'a>(fs: &mut MikuFS, path: &'a str) -> Result<(u32, &'a str), FsError> {
    let (parent_path, name) = split_parent_name(path);
    if name.is_empty() {
        return Err(FsError::InvalidInode);
    }
    let parent_ino = fs.resolve_path(parent_path)?;
    Ok((parent_ino, name))
}

fn parse_ext2_octal(s: &str) -> Option<u16> {
    let mut result: u16 = 0;
    for &b in s.as_bytes() {
        if b < b'0' || b > b'7' { return None; }
        result = result.checked_mul(8)?.checked_add((b - b'0') as u16)?;
    }
    if result > 0o7777 { return None; }
    Some(result)
}

fn parse_u16(s: &str) -> Option<u16> {
    let mut result: u16 = 0;
    for &b in s.as_bytes() {
        if b < b'0' || b > b'9' { return None; }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u16)?;
    }
    Some(result)
}

fn make_ata_drive(idx: usize) -> AtaDrive {
    match idx {
        0 => AtaDrive::primary(),
        1 => AtaDrive::primary_slave(),
        2 => AtaDrive::secondary(),
        _ => AtaDrive::secondary_slave(),
    }
}

pub fn invalidate_drive_mounts(drive_idx: usize, start_lba: u32) {
    let mut state = STATE.lock();
    let mut invalidated_any = false;
    for i in 0..MAX_MOUNTS {
        if state.ready[i] && state.drive_idx[i] == drive_idx && state.start_lba[i] == start_lba {
            let _ = state.slots[i].flush_all_dirty_metadata();
            state.ready[i] = false;
            state.slots[i].block_cache = None;
            state.slots[i].journal_inode_cached = None;
            invalidated_any = true;
            serial_println!(
                "[miku_extfs] slot {} invalidated (drive {} lba {} reformatted)",
                i, drive_idx, start_lba
            );
        }
    }
    drop(state);

    if invalidated_any {
        invalidate_vfs_ext_mounts();
    }
}

pub fn cmd_ext2_mount(args: &str) {
    let mut parts = args.split_whitespace();
    let drive_str = parts.next().unwrap_or("");
    let part_str  = parts.next().unwrap_or("");

    if drive_str.is_empty() {
        serial_println!("[miku_extfs] scanning all drives...");
        for &i in &[2usize, 1, 3, 0] {
            if STATE.lock().is_already_mounted(i, 0) {
                serial_println!("[miku_extfs] drive {} lba 0 - already mounted, skip", i);
                continue;
            }
            serial_println!("[miku_extfs] trying drive {} ...", i);
            if try_mount(i, 0) { return; }
        }
        print_error!("  no extfs found on any drive");
        return;
    }

    let drive_idx = match drive_str.parse::<usize>() {
        Ok(n) if n <= 3 => n,
        _ => { print_error!("  usage: ext2mount [drive 0-3] [partition]"); return; }
    };

    let start_lba: u32 = if !part_str.is_empty() {
        let part_num: usize = match part_str.parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => { print_error!("  invalid partition number"); return; }
        };
        let mut drive = make_ata_drive(drive_idx);
        match crate::gpt::gpt_read(&mut drive) {
            Ok(tbl) => {
                let entry = &tbl.entries[part_num - 1];
                if !entry.is_used() {
                    print_error!("  partition {} does not exist", part_num);
                    return;
                }
                entry.start_lba as u32
            }
            Err(_) => { print_error!("  could not read GPT on drive {}", drive_idx); return; }
        }
    } else {
        0u32
    };

    if !try_mount(drive_idx, start_lba) {
        print_error!("  no extfs found on drive {} (start_lba={})", drive_idx, start_lba);
    }
}

fn try_mount(drive_index: usize, start_lba: u32) -> bool {
    let mut state = STATE.lock();

    if state.is_already_mounted(drive_index, start_lba) {
        serial_println!("[miku_extfs] drive {} lba {} already mounted", drive_index, start_lba);
        return false;
    }

    let slot = match state.find_free_slot() {
        Some(s) => s,
        None => {
            print_error!("  all {} mount slots used - run fs.umount first", MAX_MOUNTS);
            return false;
        }
    };

    let drive = make_ata_drive(drive_index);
    state.ready[slot] = false;
    state.slots[slot].reader = DiskReader::new_partitioned(drive, start_lba);
    state.slots[slot].block_cache = None;
    state.slots[slot].journal_inode_cached = None;

    let mut sector = [0u8; 512];

    if state.slots[slot].reader.read_sector(2, &mut sector).is_err() {
        serial_println!(
            "[miku_extfs] drive {} lba {} - cannot read sector 2",
            drive_index, start_lba
        );
        return false;
    }
    state.slots[slot].superblock.data[0..512].copy_from_slice(&sector);

    let magic_lo = u16::from_le_bytes([sector[56], sector[57]]);
    if magic_lo != EXT2_MAGIC {
        serial_println!(
            "[miku_extfs] drive {} lba {} - bad magic 0x{:04X}, skip",
            drive_index, start_lba, magic_lo
        );
        return false;
    }

    if state.slots[slot].reader.read_sector(3, &mut sector).is_err() {
        serial_println!(
            "[miku_extfs] drive {} lba {} - cannot read sector 3",
            drive_index, start_lba
        );
        return false;
    }
    state.slots[slot].superblock.data[512..1024].copy_from_slice(&sector);

    serial_println!("[miku_extfs] slot {} drive {} lba {} - found!", slot, drive_index, start_lba);

    let block_size       = state.slots[slot].superblock.block_size();
    let inodes_per_group = state.slots[slot].superblock.inodes_per_group();
    let blocks_per_group = state.slots[slot].superblock.blocks_per_group();
    let blocks_count     = state.slots[slot].superblock.blocks_count();
    let first_data_block = state.slots[slot].superblock.first_data_block();
    let usable           = blocks_count.saturating_sub(first_data_block);
    let group_count      = if blocks_per_group == 0 { 0 }
        else { (usable + blocks_per_group - 1) / blocks_per_group };
    let gd_size          = state.slots[slot].superblock.group_desc_size() as usize;

    if group_count as usize > 32 {
        print_error!("  miku_extfs: too many block groups ({})", group_count);
        return false;
    }

    state.slots[slot].block_size       = block_size;
    state.slots[slot].inodes_per_group = inodes_per_group;
    state.slots[slot].blocks_per_group = blocks_per_group;
    state.slots[slot].group_count      = group_count;

    let gdt_block      = if block_size == 1024 { 2 } else { 1 };
    let spb            = block_size / 512;
    let gdt_start_lba  = gdt_block * spb;
    let total_gd_bytes = group_count as usize * gd_size;
    let total_sectors  = ((total_gd_bytes + 511) / 512) as u32;

    let mut carry     = [0u8; 64];
    let mut carry_len = 0usize;
    let mut gd_idx    = 0usize;

    for s in 0..total_sectors {
        if state.slots[slot].reader.read_sector(gdt_start_lba + s, &mut sector).is_err() {
            serial_println!("[miku_extfs] gdt read error at lba {}", gdt_start_lba + s);
            return false;
        }
        let mut pos = 0usize;
        if carry_len > 0 {
            let need = gd_size - carry_len;
            carry[carry_len..gd_size].copy_from_slice(&sector[..need]);
            if gd_idx < group_count as usize {
                state.slots[slot].groups[gd_idx].data[..gd_size]
                    .copy_from_slice(&carry[..gd_size]);
                gd_idx += 1;
            }
            pos = need;
            carry_len = 0;
        }
        while pos + gd_size <= 512 && gd_idx < group_count as usize {
            state.slots[slot].groups[gd_idx].data[..gd_size]
                .copy_from_slice(&sector[pos..pos + gd_size]);
            gd_idx += 1;
            pos    += gd_size;
        }
        if pos < 512 && gd_idx < group_count as usize {
            let remaining = 512 - pos;
            carry[..remaining].copy_from_slice(&sector[pos..]);
            carry_len = remaining;
        }
    }

    state.ready[slot]     = true;
    state.drive_idx[slot] = drive_index;
    state.start_lba[slot] = start_lba;
    state.active_slot     = slot;

    state.slots[slot].init_cache();
    let _ = state.slots[slot].init_journal();
    let _ = state.slots[slot].warm_cache();

    if state.slots[slot].journal_active
        && !state.slots[slot]
            .read_journal_superblock()
            .map(|j| j.is_clean())
            .unwrap_or(true)
    {
        match state.slots[slot].ext3_recover() {
            Ok(0) => {}
            Ok(n) => serial_println!("[ext3] slot {} recovery: replayed {} blocks", slot, n),
            Err(e) => serial_println!("[ext3] slot {} recovery failed: {:?}", slot, e),
        }
    }

    // cleanup orphan inodes left by unclean shutdown
    match state.slots[slot].cleanup_orphans() {
        Ok(0) => {}
        Ok(n) => serial_println!("[mount] cleaned {} orphan inodes", n),
        Err(e) => serial_println!("[mount] orphan cleanup failed: {:?}", e),
    }

    // update mount state in superblock
    state.slots[slot].update_mount_state();
    let _ = state.slots[slot].flush_all_dirty_metadata();

    let total_inodes = state.slots[slot].superblock.inodes_count();
    let free_blocks  = state.slots[slot].superblock.free_blocks_count();
    let free_inodes  = state.slots[slot].superblock.free_inodes_count();
    let version      = state.slots[slot].superblock.fs_version_str();

    print_success!("  {} mounted -> slot {} (drive {} lba={})", version, slot, drive_index, start_lba);
    println!("  Block:   {} bytes", block_size);
    println!("  Blocks:  {} total, {} free", blocks_count, free_blocks);
    println!("  Inodes:  {} total, {} free", total_inodes, free_inodes);
    println!("  Groups:  {}", group_count);
    println!("  Cache:   enabled");
    println!("  Use 'fs.select <0|1>' to switch slots");
    true
}

pub fn cmd_ext2_ls(path: &str) {
    let path = if path.is_empty() { "/" } else { path };
    let result = with_ext2(|fs| -> Result<([DirEntry; 256], usize), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        if !inode.is_directory() { return Err(FsError::NotDirectory); }
        let mut entries = [const { DirEntry::empty() }; 256];
        let count = fs.read_dir(&inode, &mut entries)?;
        Ok((entries, count))
    });
    match result {
        Some(Ok((entries, count))) => {
            println!("  ext2:{} ({} entries)", path, count);
            for i in 0..count {
                let e = &entries[i];
                let name = e.name_str();
                match e.file_type {
                    FT_DIR     => cprintln!(0, 220, 220, "  d {}/", name),
                    FT_SYMLINK => cprintln!(128, 222, 217, "  l {}@", name),
                    _          => println!("  - {} (ino={})", name, e.inode),
                }
            }
        }
        Some(Err(e)) => print_error!("  ext2ls: {:?}", e),
        None => print_error!("  ext2 not mounted (run ext2mount first)"),
    }
}

pub fn cmd_ext2_cat(path: &str) {
    if path.is_empty() { println!("Usage: ext2cat <path>"); return; }
    let result = with_ext2(|fs| -> Result<([u8; 512], usize, u64), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        if inode.is_directory() { return Err(FsError::IsDirectory); }
        if !inode.is_regular() && !inode.is_symlink() { return Err(FsError::NotRegularFile); }
        let size = inode.size();
        let read_size = (size as usize).min(512);
        let mut buf = [0u8; 512];
        let n = fs.read_file(&inode, 0, &mut buf[..read_size])?;
        Ok((buf, n, size))
    });
    match result {
        Some(Ok((buf, n, size))) => {
            if size > 512 { println!("  (showing first 512 of {} bytes)", size); }
            let s = core::str::from_utf8(&buf[..n]).unwrap_or("(binary data)");
            println!("{}", s);
        }
        Some(Err(e)) => print_error!("  ext2cat: {:?}", e),
        None => print_error!("  ext2 not mounted (run ext2mount first)"),
    }
}

pub fn cmd_ext2_stat(path: &str) {
    if path.is_empty() { println!("Usage: ext2stat <path>"); return; }
    let result = with_ext2(|fs| -> Result<(u32, Inode), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        Ok((ino, inode))
    });
    match result {
        Some(Ok((ino, inode))) => {
            println!("  Inode: {}", ino);
            println!("  Type:  {:?}", inode.file_type());
            println!("  Mode:  0o{:o}", inode.permissions());
            println!("  Size:  {} bytes", inode.size());
            println!("  Links: {}", inode.links_count());
            println!("  Blocks: {}", inode.blocks());
            println!("  UID:   {}", inode.uid_full());
            println!("  GID:   {}", inode.gid_full());
            if inode.uses_extents() { println!("  Extents: yes"); }
            if inode.has_inline_data() { println!("  Inline: yes"); }
            if inode.is_fast_symlink() {
                let target = inode.fast_symlink_target();
                if let Ok(t) = core::str::from_utf8(target) { println!("  Target: {}", t); }
            }
        }
        Some(Err(e)) => print_error!("  ext2stat: {:?}", e),
        None => print_error!("  ext2 not mounted (run ext2mount first)"),
    }
}

pub fn cmd_ext2_info() {
    let result = with_ext2(|fs| fs.fs_info());
    match result {
        Some(info) => {
            println!("  Version: {}", info.version);
            println!("  Block size: {} bytes", info.block_size);
            println!("  Blocks: {} / {} used", info.total_blocks - info.free_blocks, info.total_blocks);
            println!("  Inodes: {} / {} used", info.total_inodes - info.free_inodes, info.total_inodes);
            println!("  Groups: {}", info.groups);
            println!("  Inode size: {} bytes", info.inode_size);
            println!("  Journal: {}", if info.has_journal { "yes" } else { "no" });
            println!("  Extents: {}", if info.has_extents { "yes" } else { "no" });
        }
        None => print_error!("  ext2 not mounted (run ext2mount first)"),
    }
}

pub fn cmd_ext2_write(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() { println!("Usage: ext2write <path> <text>"); return; }
    let disk_sw = crate::timing::Stopwatch::start();
    let result = with_ext2(|fs| -> Result<u32, FsError> {
        let (parent_ino, filename) = resolve_parent_and_name(fs, path)?;
        let data = text.as_bytes();
        fs.ext3_write_file_create_or_overwrite(parent_ino, filename, 0o644, data)
    });
    let disk_ms = disk_sw.elapsed_ms();
    let render_sw = crate::timing::Stopwatch::start();
    match result {
        Some(Ok(ino)) => print_success!("  written to inode {}  [disk {}ms]", ino, disk_ms),
        Some(Err(e))  => print_error!("  ext2write: {:?}", e),
        None          => print_error!("  ext2 not mounted"),
    }
    let render_us = render_sw.elapsed_us();
    crate::serial_println!("[timing] ext2write disk={}ms render={}us", disk_ms, render_us);
}

pub fn cmd_ext2_mkdir(path: &str) {
    if path.is_empty() { println!("Usage: ext2mkdir <path>"); return; }
    let result = with_ext2(|fs| -> Result<u32, FsError> {
        let (parent_ino, dirname) = resolve_parent_and_name(fs, path)?;
        fs.ext3_create_dir(parent_ino, dirname, 0o755)
    });
    match result {
        Some(Ok(ino)) => print_success!("  created dir inode {}", ino),
        Some(Err(e))  => print_error!("  ext2mkdir: {:?}", e),
        None          => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_rm(path: &str) {
    if path.is_empty() { println!("Usage: ext2rm <path>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        fs.ext3_delete_file(parent_ino, name)
    });
    match result {
        Some(Ok(())) => print_success!("  deleted"),
        Some(Err(e)) => print_error!("  ext2rm: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_rmdir(path: &str) {
    if path.is_empty() { println!("Usage: ext2rmdir <path>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        fs.ext3_delete_dir(parent_ino, name)
    });
    match result {
        Some(Ok(())) => print_success!("  removed dir"),
        Some(Err(e)) => print_error!("  ext2rmdir: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_rm_rf(path: &str) {
    if path.is_empty() { println!("Usage: ext2rm -rf <path>"); return; }
    let result = with_ext2(|fs| -> Result<u32, FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        fs.ext2_delete_recursive(parent_ino, name)
    });
    match result {
        Some(Ok(n))  => print_success!("  removed {} entries", n),
        Some(Err(e)) => print_error!("  ext2rm -rf: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_symlink(target: &str, linkname: &str) {
    if target.is_empty() || linkname.is_empty() { println!("Usage: ext2ln -s <target> <linkname>"); return; }
    let result = with_ext2(|fs| -> Result<u32, FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, linkname)?;
        fs.ext2_create_symlink(parent_ino, name, target)
    });
    match result {
        Some(Ok(ino)) => print_success!("  symlink inode {} -> {}", ino, target),
        Some(Err(e))  => print_error!("  ext2ln: {:?}", e),
        None          => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_rename(old_path: &str, new_name: &str) {
    if old_path.is_empty() || new_name.is_empty() { println!("Usage: ext2mv <path> <newname>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let (parent_ino, old_name) = resolve_parent_and_name(fs, old_path)?;
        let actual_new_name = match new_name.rfind('/') {
            Some(pos) => &new_name[pos + 1..],
            None => new_name,
        };
        if actual_new_name.is_empty() { return Err(FsError::InvalidInode); }
        fs.ext2_rename(parent_ino, old_name, actual_new_name)
    });
    match result {
        Some(Ok(())) => print_success!("  renamed"),
        Some(Err(e)) => print_error!("  ext2mv: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_chmod(mode_str: &str, path: &str) {
    if mode_str.is_empty() || path.is_empty() { println!("Usage: ext2chmod <mode> <path>"); return; }
    let mode = parse_ext2_octal(mode_str);
    if mode.is_none() { print_error!("  invalid mode '{}'", mode_str); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_chmod(ino, mode.unwrap())
    });
    match result {
        Some(Ok(())) => print_success!("  mode set to 0o{}", mode_str),
        Some(Err(e)) => print_error!("  ext2chmod: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_chown(uid_str: &str, gid_str: &str, path: &str) {
    if uid_str.is_empty() || path.is_empty() { println!("Usage: ext2chown <uid> <gid> <path>"); return; }
    let uid = match parse_u16(uid_str) { Some(v) => v, None => { print_error!("  invalid uid"); return; } };
    let gid = if gid_str.is_empty() { uid } else {
        match parse_u16(gid_str) { Some(v) => v, None => { print_error!("  invalid gid"); return; } }
    };
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_chown(ino, uid, gid)
    });
    match result {
        Some(Ok(())) => print_success!("  owner set to {}:{}", uid, gid),
        Some(Err(e)) => print_error!("  ext2chown: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_cp(src: &str, dst: &str) {
    if src.is_empty() || dst.is_empty() { println!("Usage: ext2cp <src> <dst>"); return; }
    let result = with_ext2(|fs| -> Result<u32, FsError> {
        let src_ino = fs.resolve_path(src)?;
        let (dst_parent_ino, dst_name) = resolve_parent_and_name(fs, dst)?;
        fs.ext4_copy_file(src_ino, dst_parent_ino, dst_name)
    });
    match result {
        Some(Ok(ino)) => print_success!("  copied to inode {}", ino),
        Some(Err(e))  => print_error!("  ext2cp: {:?}", e),
        None          => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_du(path: &str) {
    let path = if path.is_empty() { "/" } else { path };
    let result = with_ext2(|fs| -> Result<(u32, u64), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_dir_size(ino)
    });
    match result {
        Some(Ok((files, bytes))) => {
            println!("  {} files, {} bytes total", files, bytes);
            if bytes >= 1024 { println!("  ({} KB)", bytes / 1024); }
        }
        Some(Err(e)) => print_error!("  ext2du: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_tree(path: &str) {
    let path = if path.is_empty() { "/" } else { path };
    let mut tree = TreeResult::new();
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_tree(ino, "", &mut tree)
    });
    match result {
        Some(Ok(())) => {
            cprintln!(0, 220, 220, "  {}", path);
            for i in 0..tree.count {
                let e = &tree.entries[i];
                let depth = e.depth as usize;
                for _ in 0..depth { cprint!(120, 140, 140, "    "); }
                if e.is_last { cprint!(120, 140, 140, "/ "); } else { cprint!(120, 140, 140, "--- "); }
                if e.is_dir { cprintln!(0, 220, 220, "{}/", e.name_str()); }
                else if e.is_symlink { cprintln!(128, 222, 217, "{}@", e.name_str()); }
                else { cprintln!(230, 240, 240, "{} ({}b)", e.name_str(), e.size); }
            }
            println!("  {} entries", tree.count);
        }
        Some(Err(e)) => print_error!("  ext2tree: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_fsck() {
    let result = with_ext2(|fs| fs.ext2_fsck());
    match result {
        Some(r) => {
            if !r.checked { print_error!("  fsck failed to run"); return; }
            cprintln!(57, 197, 187, "  ext2 filesystem check");
            println!("  Block size:   {} bytes", r.block_size);
            println!("  Total blocks: {}", r.total_blocks);
            println!("  Free blocks:  {}", r.free_blocks);
            println!("  Total inodes: {}", r.total_inodes);
            println!("  Free inodes:  {}", r.free_inodes);
            println!("  Used inodes:  {}", r.used_inodes);
            if r.bad_magic     { print_error!("  error: bad superblock magic"); }
            if !r.root_ok      { print_error!("  error: cannot read root inode"); }
            if r.root_not_dir  { print_error!("  error: root inode is not a directory"); }
            if r.bad_groups > 0 { print_error!("  error: {} bad group descriptors", r.bad_groups); }
            if r.orphan_inodes > 0 { cprintln!(220, 220, 100, "  warning: {} orphan inodes", r.orphan_inodes); }
            if r.errors == 0 { print_success!("  filesystem ok"); }
            else { print_error!("  {} errors found", r.errors); }
        }
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_append(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() { println!("Usage: ext2append <path> <text>"); return; }
    let result = with_ext2(|fs| -> Result<usize, FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_append_file(ino, text.as_bytes())
    });
    match result {
        Some(Ok(n))  => print_success!("  appended {} bytes", n),
        Some(Err(e)) => print_error!("  ext2append: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_hardlink(existing: &str, linkname: &str) {
    if existing.is_empty() || linkname.is_empty() { println!("Usage: ext2link <existing> <linkname>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let target_ino = fs.resolve_path(existing)?;
        let (parent_ino, name) = resolve_parent_and_name(fs, linkname)?;
        fs.ext2_hardlink(parent_ino, name, target_ino)
    });
    match result {
        Some(Ok(())) => print_success!("  hardlink created"),
        Some(Err(e)) => print_error!("  ext2link: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_info() {
    let result = with_ext2(|fs| fs.scan_journal());
    match result {
        Some(Ok(info)) => {
            if !info.valid { print_error!("  no journal found"); return; }
            cprintln!(57, 197, 187, "  ext3 Journal Info");
            println!("  Version:    {}", if info.version == 2 { "JBD2" } else { "JBD1" });
            println!("  Block size: {} bytes", info.block_size);
            println!("  Total:      {} blocks", info.total_blocks);
            println!("  Size:       {} KB", info.journal_size / 1024);
            println!("  First:      block {}", info.first_block);
            println!("  Sequence:   {}", info.sequence);
            println!("  Start:      {}", info.start);
            if info.clean { print_success!("  Status:     clean"); }
            else { print_error!("  Status:     dirty ({} transactions)", info.transaction_count); }
            if info.errno != 0 { print_error!("  Errno:      {}", info.errno); }
        }
        Some(Err(FsError::NoJournal)) => print_error!("  no journal"),
        Some(Err(e)) => print_error!("  ext3info: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_journal() {
    let result = with_ext2(|fs| fs.scan_journal());
    match result {
        Some(Ok(info)) => {
            if !info.valid { print_error!("  no journal found"); return; }
            if info.clean { print_success!("  journal is clean"); return; }
            cprintln!(57, 197, 187, "  Journal Transactions ({}):", info.transaction_count);
            for i in 0..info.transaction_count {
                let tx = &info.transactions[i];
                if !tx.active { continue; }
                if tx.committed {
                    cprintln!(100, 220, 150, "  {:>6}  {:>8}  {:>6}  committed", tx.sequence, tx.start_block, tx.data_blocks);
                } else {
                    cprintln!(255, 50, 50, "  {:>6}  {:>8}  {:>6}  incomplete", tx.sequence, tx.start_block, tx.data_blocks);
                }
            }
        }
        Some(Err(FsError::NoJournal)) => print_error!("  no journal"),
        Some(Err(e)) => print_error!("  ext3journal: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_mkjournal() {
    let result = with_ext2(|fs| -> Result<(), FsError> { fs.ext3_create_journal(DEFAULT_JOURNAL_BLOCKS) });
    match result {
        Some(Ok(())) => print_success!("  ext3 journal created ({} blocks)", DEFAULT_JOURNAL_BLOCKS),
        Some(Err(FsError::AlreadyExists)) => print_error!("  journal already exists"),
        Some(Err(e)) => print_error!("  ext3mkjournal: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_clean() {
    let result = with_ext2(|fs| fs.ext3_clean_journal());
    match result {
        Some(Ok(())) => print_success!("  journal marked clean"),
        Some(Err(FsError::NoJournal)) => print_error!("  no journal found"),
        Some(Err(e)) => print_error!("  ext3clean: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_recover() {
    let result = with_ext2(|fs| fs.ext3_recover());
    match result {
        Some(Ok(0)) => print_success!("  no recovery needed"),
        Some(Ok(n)) => print_success!("  recovered {} blocks", n),
        Some(Err(FsError::NoJournal)) => print_error!("  no journal found"),
        Some(Err(e)) => print_error!("  ext3recover: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext2_cache() {
    let result = with_ext2(|fs| match &fs.block_cache {
        Some(c) => {
            cprintln!(57, 197, 187, "  Block Cache");
            println!("  Entries:   {}/{}", c.cached_entries(), c.capacity());
            println!("  Memory:    {} KB", c.total_bytes() / 1024);
            println!("  Hits:      {}", c.hits);
            println!("  Misses:    {}", c.misses);
            println!("  Hit rate:  {}%", c.hit_rate());
            println!("  Evictions: {}", c.evictions);
        }
        None => print_error!("  cache not initialized"),
    });
    if result.is_none() { print_error!("  ext2 not mounted"); }
}

pub fn cmd_ext2_cache_flush() {
    let result = with_ext2(|fs| {
        if let Some(ref mut c) = fs.block_cache {
            c.clear();
            print_success!("  cache flushed");
        } else {
            print_error!("  cache not initialized");
        }
    });
    if result.is_none() { print_error!("  ext2 not mounted"); }
}

pub fn cmd_ext4_info() {
    let result = with_ext2(|fs| {
        let info = fs.fs_info();
        cprintln!(57, 197, 187, "  ext4 Filesystem Info");
        println!("  Version:   {}", info.version);
        println!("  Extents:   {}", if info.has_extents { "enabled" } else { "disabled" });
        println!("  Journal:   {}", if info.has_journal { "enabled" } else { "disabled" });
        println!("  64bit:     {}", if fs.superblock.has_64bit() { "yes" } else { "no" });
        println!("  Checksums: {}", if fs.superblock.has_metadata_csum() { "crc32c" } else { "none" });
        println!("  Flex BG:   {}", if fs.superblock.has_flex_bg() { "yes" } else { "no" });
        if fs.superblock.has_metadata_csum() {
            let sb_ok = fs.verify_superblock_csum();
            if sb_ok { print_success!("  SB csum:   valid"); }
            else { print_error!("  SB csum:   invalid"); }
        }
    });
    if result.is_none() { print_error!("  ext2 not mounted"); }
}

pub fn cmd_ext4_enable_extents() {
    let result = with_ext2(|fs| -> Result<(), FsError> { fs.enable_extents_feature() });
    match result {
        Some(Ok(())) => print_success!("  extents enabled"),
        Some(Err(e)) => print_error!("  ext4extents: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext4_checksums() {
    let result = with_ext2(|fs| {
        cprintln!(57, 197, 187, "  Checksum Verification");
        let sb_ok = fs.verify_superblock_csum();
        println!("  Superblock: {}", if sb_ok { "ok" } else { "fail" });
        let gc = fs.group_count as usize;
        let (mut gd_ok, mut gd_fail) = (0u32, 0u32);
        let (mut bb_ok, mut bb_fail) = (0u32, 0u32);
        let (mut ib_ok, mut ib_fail) = (0u32, 0u32);
        for g in 0..gc.min(32) {
            if fs.verify_group_desc_csum(g) { gd_ok += 1; } else { gd_fail += 1; }
            if fs.verify_block_bitmap_csum(g) { bb_ok += 1; } else { bb_fail += 1; }
            if fs.verify_inode_bitmap_csum(g) { ib_ok += 1; } else { ib_fail += 1; }
        }
        println!("  Group descs:   {} ok, {} fail", gd_ok, gd_fail);
        println!("  Block bitmaps: {} ok, {} fail", bb_ok, bb_fail);
        println!("  Inode bitmaps: {} ok, {} fail", ib_ok, ib_fail);
        let (mut ino_ok, mut ino_fail) = (0u32, 0u32);
        let max_check = fs.superblock.inodes_count().min(64);
        for ino in 1..=max_check {
            if let Ok(inode) = fs.read_inode(ino) {
                if inode.mode() != 0 {
                    if fs.verify_inode_csum(ino, &inode) { ino_ok += 1; } else { ino_fail += 1; }
                }
            }
        }
        println!("  Inodes (first {}): {} ok, {} fail", max_check, ino_ok, ino_fail);
    });
    if result.is_none() { print_error!("  ext2 not mounted"); }
}

pub fn cmd_ext4_extent_info(path: &str) {
    if path.is_empty() { println!("Usage: ext4extinfo <path>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        if !inode.uses_extents() {
            println!("  inode {} does not use extents (indirect blocks)", ino);
            return Ok(());
        }
        let header = inode.extent_header();
        println!("  Inode: {}", ino);
        println!("  Extent tree depth: {}", header.depth);
        println!("  Entries: {} / {}", header.entries, header.max);
        let count = fs.ext4_extent_count(&inode)?;
        println!("  Total extents: {}", count);
        if header.depth == 0 {
            for i in 0..header.entries as usize {
                let ext = inode.extent_at(i);
                println!("    [{}] logical={} len={} phys={}", i, ext.block, ext.actual_len(), ext.start());
            }
        }
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  ext4extinfo: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext4_write(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() { println!("Usage: ext4write <path> <text>"); return; }
    let disk_sw = crate::timing::Stopwatch::start();
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let (parent_ino, filename) = resolve_parent_and_name(fs, path)?;
        fs.ext3_write_file_create_or_overwrite(parent_ino, filename, 0o644, text.as_bytes())
    });
    let disk_ms = disk_sw.elapsed_ms();
    let render_sw = crate::timing::Stopwatch::start();
    match result {
        Some(Ok(ino)) => print_success!("  [ext4] written to inode {} (extents+journal)  [disk {}ms]", ino, disk_ms),
        Some(Err(e))  => print_error!("  ext4write: {:?}", e),
        None          => print_error!("  not mounted"),
    }
    let render_us = render_sw.elapsed_us();
    crate::serial_println!("[timing] ext4write disk={}ms render={}us", disk_ms, render_us);
}

pub fn cmd_fiemap(path: &str) {
    if path.is_empty() { println!("Usage: fiemap <path>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let mut extents = [(0u32, 0u32, 0u32); 64];
        let count = fs.ext4_fiemap(ino, &mut extents)?;
        println!("  File extent map for inode {}:", ino);
        if count == 0 {
            println!("  (no extents / empty file)");
        }
        let mut total_blocks = 0u32;
        for i in 0..count {
            let (logical, phys, len) = extents[i];
            println!("    [{:2}] logical={:<6} phys={:<8} len={}", i, logical, phys, len);
            total_blocks += len;
        }
        println!("  {} extents, {} blocks total", count, total_blocks);
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  fiemap: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_getxattr(path: &str, name: &str) {
    if path.is_empty() || name.is_empty() { println!("Usage: getxattr <path> <name>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let mut buf = [0u8; 256];
        let len = fs.get_xattr(ino, crate::miku_extfs::xattr::XATTR_INDEX_USER, name, &mut buf)?;
        if let Ok(s) = core::str::from_utf8(&buf[..len]) {
            println!("  {}=\"{}\"", name, s);
        } else {
            println!("  {} = [{} bytes]", name, len);
        }
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(FsError::NotFound)) => print_error!("  xattr '{}' not found", name),
        Some(Err(e)) => print_error!("  getxattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_setxattr(path: &str, name: &str, value: &str) {
    if path.is_empty() || name.is_empty() { println!("Usage: setxattr <path> <name> <value>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.set_xattr(ino, crate::miku_extfs::xattr::XATTR_INDEX_USER, name, value.as_bytes())
    });
    match result {
        Some(Ok(())) => print_success!("  xattr '{}' set", name),
        Some(Err(e)) => print_error!("  setxattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_listxattr(path: &str) {
    if path.is_empty() { println!("Usage: listxattr <path>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let list = fs.read_xattrs(ino)?;
        if list.count == 0 {
            println!("  (no extended attributes)");
        }
        for i in 0..list.count {
            let e = &list.entries[i];
            let ns = match e.name_index {
                1 => "user",
                2 => "system.posix_acl_access",
                3 => "system.posix_acl_default",
                4 => "trusted",
                6 => "security",
                7 => "system",
                _ => "unknown",
            };
            if let Ok(val) = core::str::from_utf8(&e.value[..e.value_len as usize]) {
                println!("  {}.{}=\"{}\"", ns, e.name_str(), val);
            } else {
                println!("  {}.{} = [{} bytes]", ns, e.name_str(), e.value_len);
            }
        }
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  listxattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_chattr(flags_str: &str, path: &str) {
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        let mut flags = inode.flags();
        let adding = flags_str.starts_with('+');
        let removing = flags_str.starts_with('-');
        if !adding && !removing {
            return Err(FsError::InvalidArg);
        }
        let chars = &flags_str[1..];
        for c in chars.bytes() {
            let flag = match c {
                b'i' => EXT4_IMMUTABLE_FL,
                b'a' => EXT4_APPEND_FL,
                b'd' => EXT4_NODUMP_FL,
                b'A' => EXT4_NOATIME_FL,
                _ => continue,
            };
            if adding { flags |= flag; } else { flags &= !flag; }
        }
        let mut inode = fs.read_inode(ino)?;
        inode.set_flags(flags);
        let now = fs.get_timestamp();
        inode.set_ctime(now);
        fs.write_inode(ino, &inode)
    });
    match result {
        Some(Ok(())) => print_success!("  flags updated"),
        Some(Err(e)) => print_error!("  chattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_lsattr(path: &str) {
    if path.is_empty() { println!("Usage: lsattr <path>"); return; }
    let result = with_ext2(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        let f = inode.flags();
        let mut attrs = [b'-'; 8];
        if f & EXT4_IMMUTABLE_FL != 0 { attrs[0] = b'i'; }
        if f & EXT4_APPEND_FL != 0 { attrs[1] = b'a'; }
        if f & EXT4_NODUMP_FL != 0 { attrs[2] = b'd'; }
        if f & EXT4_NOATIME_FL != 0 { attrs[3] = b'A'; }
        if f & EXT4_EXTENTS_FL != 0 { attrs[4] = b'e'; }
        if f & EXT4_INDEX_FL != 0 { attrs[5] = b'I'; }
        if f & EXT4_HUGE_FILE_FL != 0 { attrs[6] = b'h'; }
        if f & EXT4_INLINE_DATA_FL != 0 { attrs[7] = b'N'; }
        let s = core::str::from_utf8(&attrs).unwrap_or("--------");
        println!("  {} {}", s, path);
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  lsattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}
