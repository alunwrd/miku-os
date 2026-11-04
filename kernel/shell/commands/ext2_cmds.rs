use crate::drivers::block::ata::AtaDrive;
use crate::fs::ext::ext2::write::TreeResult;
use crate::fs::ext::structs::*;
use crate::fs::ext::{FsError, MikuFS};
use crate::{cprint, cprintln, print_error, print_success, println, serial_println};
use crate::fs::vfs::path::split_parent_name;

// Mount table lives in fs::ext::mount; re-exported here so shell call sites
// can keep using `ext2_cmds::`
pub use crate::fs::ext::mount::{
    active_fs_type, active_slot_index, dev_for_idx, ext_fs_version_tag, force_unmount,
    invalidate_drive_mounts, is_ext2_ready, mount_dev, mount_root_disk,
    mounted_slots_snapshot, register_mount_vnode, remount_ro, remount_rw, slot_for_vnode,
    try_mount, try_mount_ro, unmount_slot, with_ext2_pub, MAX_MOUNTS,
};
use crate::fs::ext::mount::{
    invalidate_vfs_ext_mounts, scan_volumes, FsProbe, INVALID_VNODE, STATE,
};

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
            let mode    = if state.slots[slot].reader.read_only { "ro" } else { "rw" };
            println!(
                "  [{}] {} {} drive={} lba={} free={}/{} ({} MB){}",
                slot, version, mode, drive, lba,
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
        _ => { print_error!("  usage: fs.select <0-{}>", MAX_MOUNTS - 1); return; }
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

/// fs.rw [slot] / fs.ro [slot]: flips a mount between read-only and read-write.
/// Mounts come up read-only, so this is where the user says "yes, write to this
/// disk"
pub fn cmd_fs_remount(args: &str, want_rw: bool) {
    let arg = args.trim();
    let slot: usize = if arg.is_empty() {
        active_slot_index()
    } else {
        match arg.parse() {
            Ok(n) if n < MAX_MOUNTS => n,
            _ => {
                print_error!("  usage: fs.{} [0-{}]", if want_rw { "rw" } else { "ro" }, MAX_MOUNTS - 1);
                return;
            }
        }
    };
    let result = if want_rw { remount_rw(slot) } else { remount_ro(slot) };
    match result {
        Ok(())   => print_success!("  slot {} is now {}", slot, if want_rw { "read-write" } else { "read-only" }),
        Err(msg) => print_error!("  fs.{}: slot {}: {}", if want_rw { "rw" } else { "ro" }, slot, msg),
    }
}

pub fn cmd_fs_umount(args: &str) {
    let mut state = STATE.lock();
    let slot: usize = if args.trim().is_empty() {
        state.active_slot
    } else {
        match args.trim().parse() {
            Ok(n) if n < MAX_MOUNTS => n,
            _ => { print_error!("  usage: fs.umount [0-{}]", MAX_MOUNTS - 1); return; }
        }
    };
    if !state.ready[slot] {
        crate::print_warn!("  slot {} is already empty", slot);
        return;
    }
    // Read-only slot never got dirty, nothing to flush
    if !state.slots[slot].reader.read_only {
        let _ = state.slots[slot].finish_unmount();
    }
    state.ready[slot] = false;
    state.slots[slot].block_cache = None;
    state.slots[slot].journal_inode_cached = None;
    state.mount_vnode[slot] = INVALID_VNODE;
    print_success!("  slot {} unmounted", slot);
    // Hands the active role to another mounted slot, if there is one
    if state.active_slot == slot {
        if let Some(other) = (0..MAX_MOUNTS).find(|&i| i != slot && state.ready[i]) {
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

/// An id as typed. `u32`, the width an id has on disk: ext4 keeps the high half
/// in `l_i_uid_high`
fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() { return None; }
    let mut result: u32 = 0;
    for &b in s.as_bytes() {
        if b < b'0' || b > b'9' { return None; }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(result)
}

/// Mounts an ext filesystem. Indices 0-3 are the legacy ATA slots; 4-7 address
/// PCI block devices (virtio-blk) the boot-time probe registered.
pub fn cmd_ext2_mount(args: &str) {
    // rw must be spelled out explicitly: a read-write mount stamps the
    // superblock, replays the journal, and starts writing atimes
    let want_rw = args.split_whitespace().any(|t| t == "rw");
    let mut parts = args.split_whitespace().filter(|t| *t != "rw");
    let drive_str = parts.next().unwrap_or("");
    let part_str  = parts.next().unwrap_or("");
    let mount = |drive: usize, lba: u32| -> bool {
        if want_rw { try_mount(drive, lba) } else { try_mount_ro(drive, lba) }
    };

    if drive_str.is_empty() {
        serial_println!("[miku_extfs] scanning all drives...");

        // The shared scan walks GPT partitions too, not just LBA 0: an
        // installed root always lives in a partition
        let mut candidates: alloc::vec::Vec<FsProbe> = alloc::vec::Vec::new();
        let mut already_mounted: Option<usize> = None;

        for probe in scan_volumes() {
            if STATE.lock().is_already_mounted(probe.drive, probe.start_lba) {
                already_mounted = Some(probe.drive);
                continue;
            }
            serial_println!(
                "[miku_extfs] drive {}:{} - {} candidate (block={}, label '{}')",
                probe.drive, probe.start_lba, probe.version, probe.block_size,
                probe.label_str()
            );
            candidates.push(probe);
        }

        match candidates.len() {
            0 => {
                if let Some(d) = already_mounted {
                    print_success!("  drive {} already mounted (use fs.list)", d);
                } else {
                    print_error!("  no extfs found on any drive");
                }
            }
            1 => {
                let (d, lba) = (candidates[0].drive, candidates[0].start_lba);
                if !mount(d, lba) {
                    print_error!("  failed to mount ext on drive {}:{}", d, lba);
                }
            }
            _ => {
                print_error!("  multiple ext filesystems found:");
                for c in &candidates {
                    println!(
                        "    drive {}:{}  {} ({} byte blocks) labelled '{}'",
                        c.drive, c.start_lba, c.version, c.block_size, c.label_str()
                    );
                }
                println!("  specify explicitly: ext4mount <drive> <lba>");
            }
        }
        return;
    }

    let drive_idx = match drive_str.parse::<usize>() {
        Ok(n) if n <= 7 => n,
        _ => { print_error!("  usage: ext2mount [drive 0-7] [partition] [rw]"); return; }
    };

    let start_lba: u32 = if !part_str.is_empty() {
        let part_num: usize = match part_str.parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => { print_error!("  invalid partition number"); return; }
        };
        let dev = dev_for_idx(drive_idx);
        match crate::fs::gpt::gpt_read(dev) {
            Ok(tbl) => {
                // part_num is 1-based user input, not an array index
                let Some(entry) = tbl.entries.get(part_num - 1) else {
                    print_error!(
                        "  drive {} has {} partition slots, not {}",
                        drive_idx, tbl.entries.len(), part_num);
                    return;
                };
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

    if !mount(drive_idx, start_lba) {
        print_error!("  no extfs found on drive {} (start_lba={})", drive_idx, start_lba);
    }
}

// ls/cat/stat/info share one implementation across ext2/3/4; output reflects
// what the superblock actually reports, not which command name was typed
pub fn cmd_ext2_ls(path: &str) {
    super::ext_cmds_common::impl_ls(path, "ext2");
}

pub fn cmd_ext2_cat(path: &str) {
    super::ext_cmds_common::impl_cat(path, "ext2");
}

pub fn cmd_ext2_stat(path: &str) {
    super::ext_cmds_common::impl_stat(path, "ext2");
}

pub fn cmd_ext2_info() {
    super::ext_cmds_common::impl_info("ext2");
}

pub fn cmd_ext2_write(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() { println!("Usage: ext2write <path> <text>"); return; }
    let disk_sw = crate::kcore::time::Stopwatch::start();
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let (parent_ino, filename) = resolve_parent_and_name(fs, path)?;
        let data = text.as_bytes();
        fs.ext3_write_file_create_or_overwrite(parent_ino, filename, 0o644, data)
    });
    let disk_ms = disk_sw.elapsed_ms();
    // Written by inode, past the VFS: no cached page of it may outlive this
    crate::fs::vfs::core::changed_below_vfs();
    let render_sw = crate::kcore::time::Stopwatch::start();
    match result {
        Some(Ok(ino)) => print_success!("  written to inode {}  [disk {}ms]", ino, disk_ms),
        Some(Err(e))  => print_error!("  ext2write: {:?}", e),
        None          => print_error!("  no ext2/3/4 filesystem mounted"),
    }
    let render_us = render_sw.elapsed_us();
    crate::serial_println!("[timing] ext2write disk={}ms render={}us", disk_ms, render_us);
}

pub fn cmd_ext2_mkdir(path: &str) {
    if path.is_empty() { println!("Usage: ext2mkdir <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let (parent_ino, dirname) = resolve_parent_and_name(fs, path)?;
        fs.ext3_create_dir(parent_ino, dirname, 0o755)
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(ino)) => print_success!("  created dir inode {}", ino),
        Some(Err(e))  => print_error!("  ext2mkdir: {:?}", e),
        None          => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_rm(path: &str) {
    if path.is_empty() { println!("Usage: ext2rm <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        fs.ext3_delete_file(parent_ino, name)
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(())) => print_success!("  deleted"),
        Some(Err(e)) => print_error!("  ext2rm: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_rmdir(path: &str) {
    if path.is_empty() { println!("Usage: ext2rmdir <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        fs.ext3_delete_dir(parent_ino, name)
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(())) => print_success!("  removed dir"),
        Some(Err(e)) => print_error!("  ext2rmdir: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_rm_rf(path: &str) {
    if path.is_empty() { println!("Usage: ext2rm -rf <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        fs.ext2_delete_recursive(parent_ino, name)
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(n))  => print_success!("  removed {} entries", n),
        Some(Err(e)) => print_error!("  ext2rm -rf: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_symlink(target: &str, linkname: &str) {
    if target.is_empty() || linkname.is_empty() { println!("Usage: ext2ln -s <target> <linkname>"); return; }
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, linkname)?;
        fs.ext2_create_symlink(parent_ino, name, target)
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(ino)) => print_success!("  symlink inode {} -> {}", ino, target),
        Some(Err(e))  => print_error!("  ext2ln: {:?}", e),
        None          => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_rename(old_path: &str, new_name: &str) {
    if old_path.is_empty() || new_name.is_empty() { println!("Usage: ext2mv <path> <newname>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let (parent_ino, old_name) = resolve_parent_and_name(fs, old_path)?;
        let actual_new_name = match new_name.rfind('/') {
            Some(pos) => &new_name[pos + 1..],
            None => new_name,
        };
        if actual_new_name.is_empty() { return Err(FsError::InvalidInode); }
        fs.ext2_rename(parent_ino, old_name, actual_new_name)
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(())) => print_success!("  renamed"),
        Some(Err(e)) => print_error!("  ext2mv: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_chmod(mode_str: &str, path: &str) {
    if mode_str.is_empty() || path.is_empty() { println!("Usage: ext2chmod <mode> <path>"); return; }
    let mode = parse_ext2_octal(mode_str);
    if mode.is_none() { print_error!("  invalid mode '{}'", mode_str); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_chmod(ino, mode.unwrap())
    });
    match result {
        Some(Ok(())) => print_success!("  mode set to 0o{}", mode_str),
        Some(Err(e)) => print_error!("  ext2chmod: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_chown(uid_str: &str, gid_str: &str, path: &str) {
    if uid_str.is_empty() || path.is_empty() { println!("Usage: ext2chown <uid> <gid> <path>"); return; }
    let uid = match parse_u32(uid_str) { Some(v) => v, None => { print_error!("  invalid uid"); return; } };
    let gid = if gid_str.is_empty() { uid } else {
        match parse_u32(gid_str) { Some(v) => v, None => { print_error!("  invalid gid"); return; } }
    };
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_chown(ino, uid, gid)
    });
    match result {
        Some(Ok(())) => print_success!("  owner set to {}:{}", uid, gid),
        Some(Err(e)) => print_error!("  ext2chown: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_cp(src: &str, dst: &str) {
    if src.is_empty() || dst.is_empty() { println!("Usage: ext2cp <src> <dst>"); return; }
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let src_ino = fs.resolve_path(src)?;
        let (dst_parent_ino, dst_name) = resolve_parent_and_name(fs, dst)?;
        fs.ext4_copy_file(src_ino, dst_parent_ino, dst_name)
    });
    match result {
        Some(Ok(ino)) => print_success!("  copied to inode {}", ino),
        Some(Err(e))  => print_error!("  ext2cp: {:?}", e),
        None          => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_du(path: &str) {
    super::ext_cmds_common::impl_du(path, "ext2");
}

pub fn cmd_ext2_tree(path: &str) {
    super::ext_cmds_common::impl_tree(path, "ext2");
}

pub fn cmd_ext2_fsck(repair: bool) {
    super::ext_cmds_common::impl_fsck(repair);
}

pub fn cmd_ext2_append(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() { println!("Usage: ext2append <path> <text>"); return; }
    let result = with_ext2_pub(|fs| -> Result<usize, FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_append_file(ino, text.as_bytes())
    });
    match result {
        Some(Ok(n))  => print_success!("  appended {} bytes", n),
        Some(Err(e)) => print_error!("  ext2append: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_hardlink(existing: &str, linkname: &str) {
    if existing.is_empty() || linkname.is_empty() { println!("Usage: ext2link <existing> <linkname>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let target_ino = fs.resolve_path(existing)?;
        let (parent_ino, name) = resolve_parent_and_name(fs, linkname)?;
        fs.ext2_hardlink(parent_ino, name, target_ino)
    });
    match result {
        Some(Ok(())) => print_success!("  hardlink created"),
        Some(Err(e)) => print_error!("  ext2link: {:?}", e),
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
    }
}

pub fn cmd_ext2_cache() {
    let result = with_ext2_pub(|fs| match &fs.block_cache {
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
    if result.is_none() { print_error!("  no ext2/3/4 filesystem mounted"); }
}

pub fn cmd_ext2_cache_flush() {
    let result = with_ext2_pub(|fs| {
        if let Some(ref mut c) = fs.block_cache {
            c.clear();
            print_success!("  cache flushed");
        } else {
            print_error!("  cache not initialized");
        }
    });
    if result.is_none() { print_error!("  no ext2/3/4 filesystem mounted"); }
}

