use crate::drivers::block::ata::AtaDrive;
use crate::fs::ext::ext2::write::TreeResult;
use crate::fs::ext::structs::*;
use crate::fs::ext::{FsError, MikuFS};
use crate::{cprint, cprintln, print_error, print_success, println, serial_println};
use crate::fs::vfs::path::split_parent_name;

// The mount table itself now lives in fs::ext::mount. These re-exports keep
// the existing `ext2_cmds::` call sites inside the shell working
pub use crate::fs::ext::mount::{
    active_fs_type, active_slot_index, dev_for_idx, ext_fs_version_tag, force_unmount,
    invalidate_drive_mounts, is_ext2_ready, mount_dev, mount_root_disk,
    mounted_slots_snapshot, register_mount_vnode, slot_for_vnode, try_mount,
    unmount_slot, with_ext2_pub, MAX_MOUNTS,
};
use crate::fs::ext::mount::{
    invalidate_vfs_ext_mounts, probe_drive, with_ext2, ExtProbe, INVALID_VNODE, STATE,
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
    state.mount_vnode[slot] = INVALID_VNODE;
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

/// Resolve a mount "drive index" to a block-layer device id. Indices 0-3 are
/// the legacy ATA slots; 4-7 address PCI block devices (virtio-blk) that the
/// boot-time probe registered.
pub fn cmd_ext2_mount(args: &str) {
    let mut parts = args.split_whitespace();
    let drive_str = parts.next().unwrap_or("");
    let part_str  = parts.next().unwrap_or("");

    if drive_str.is_empty() {
        serial_println!("[miku_extfs] scanning all drives...");

        let mut candidates: alloc::vec::Vec<ExtProbe> = alloc::vec::Vec::new();
        let mut already_mounted: Option<usize> = None;

        for i in 0..crate::fs::vfs::types::MAX_BLOCK_DEVICES {
            if STATE.lock().is_already_mounted(i, 0) {
                already_mounted = Some(i);
                continue;
            }
            if let Some(probe) = probe_drive(i, 0) {
                serial_println!(
                    "[miku_extfs] drive {} - {} candidate (block={})",
                    probe.drive, probe.fs_version, probe.block_size
                );
                candidates.push(probe);
            }
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
                let d = candidates[0].drive;
                if !try_mount(d, 0) {
                    print_error!("  failed to mount ext on drive {}", d);
                }
            }
            _ => {
                print_error!("  multiple ext filesystems found:");
                for c in &candidates {
                    println!(
                        "    drive {}: {} ({} byte blocks)",
                        c.drive, c.fs_version, c.block_size
                    );
                }
                println!("  specify explicitly: ext4mount <drive>");
            }
        }
        return;
    }

    let drive_idx = match drive_str.parse::<usize>() {
        Ok(n) if n <= 7 => n,
        _ => { print_error!("  usage: ext2mount [drive 0-7] [partition]"); return; }
    };

    let start_lba: u32 = if !part_str.is_empty() {
        let part_num: usize = match part_str.parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => { print_error!("  invalid partition number"); return; }
        };
        let dev = dev_for_idx(drive_idx);
        match crate::fs::gpt::gpt_read(dev) {
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

/// Mount the persistent root disk (disk.img = ATA drive 1, primary slave) at
/// the ext layer if it is not already mounted. Quiet, boot-safe wrapper around
/// try_mount used by the firmware loader to bring /lib/firmware online. Returns
/// true if an ext filesystem is mounted on drive 1 afterwards
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
        None => print_error!("  no ext2/3/4 filesystem mounted (run ext2mount first)"),
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
        None => print_error!("  no ext2/3/4 filesystem mounted (run ext2mount first)"),
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
        None => print_error!("  no ext2/3/4 filesystem mounted (run ext2mount first)"),
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
        None => print_error!("  no ext2/3/4 filesystem mounted (run ext2mount first)"),
    }
}

pub fn cmd_ext2_write(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() { println!("Usage: ext2write <path> <text>"); return; }
    let disk_sw = crate::kcore::time::Stopwatch::start();
    let result = with_ext2(|fs| -> Result<u32, FsError> {
        let (parent_ino, filename) = resolve_parent_and_name(fs, path)?;
        let data = text.as_bytes();
        fs.ext3_write_file_create_or_overwrite(parent_ino, filename, 0o644, data)
    });
    let disk_ms = disk_sw.elapsed_ms();
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
    let result = with_ext2(|fs| -> Result<u32, FsError> {
        let (parent_ino, dirname) = resolve_parent_and_name(fs, path)?;
        fs.ext3_create_dir(parent_ino, dirname, 0o755)
    });
    match result {
        Some(Ok(ino)) => print_success!("  created dir inode {}", ino),
        Some(Err(e))  => print_error!("  ext2mkdir: {:?}", e),
        None          => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None          => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None          => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
        None         => print_error!("  no ext2/3/4 filesystem mounted"),
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
    if result.is_none() { print_error!("  no ext2/3/4 filesystem mounted"); }
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
    if result.is_none() { print_error!("  no ext2/3/4 filesystem mounted"); }
}

