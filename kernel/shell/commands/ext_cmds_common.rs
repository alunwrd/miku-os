use alloc::vec::Vec;
use crate::fs::ext::structs::*;
use crate::fs::ext::FsError;
use crate::{cprint, cprintln, print_error, print_success, println};
use crate::fs::ext::ext2::write::TreeResult;
use crate::fs::vfs::path::split_parent_name;

pub fn resolve_parent_and_name<'a>(
    fs: &mut crate::fs::ext::MikuFS,
    path: &'a str,
) -> Result<(u32, &'a str), FsError> {
    let (parent_path, name) = split_parent_name(path);
    if name.is_empty() { return Err(FsError::InvalidInode); }
    let parent_ino = fs.resolve_path(parent_path)?;
    Ok((parent_ino, name))
}

pub fn impl_ls(
    path: &str,
    prefix: &'static str,
) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    let path = if path.is_empty() { "/" } else { path };
    // Collected on the heap: a `[DirEntry; 256]` on the stack is about 67 KiB
    // and stops at the 256th entry
    let result = with_ext2_pub(|fs| -> Result<(Vec<DirEntry>, &'static str), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        if !inode.is_directory() { return Err(FsError::NotDirectory); }
        let mut entries: Vec<DirEntry> = Vec::new();
        fs.read_dir_each(&inode, |e| {
            entries.push(*e);
            true
        })?;
        // The heading names the filesystem mounted, not the command typed:
        // `ext2ls` on an ext4 root says ext4
        Ok((entries, fs.superblock.fs_version_str()))
    });
    match result {
        Some(Ok((entries, fs_name))) => {
            println!("  {}:{} ({} entries)", fs_name, path, entries.len());
            for e in entries.iter() {
                let name = e.name_str();
                match e.file_type {
                    FT_DIR     => cprintln!(0, 220, 220, "  d {}/", name),
                    FT_SYMLINK => cprintln!(128, 222, 217, "  l {}@", name),
                    _          => println!("  - {} (ino={})", name, e.inode),
                }
            }
        }
        Some(Err(e)) => print_error!("  {}ls: {:?}", prefix, e),
        None         => print_error!("  {} not mounted (run {}mount first)", prefix, prefix),
    }
}

pub fn impl_cat(path: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if path.is_empty() { println!("Usage: {}cat <path>", prefix); return; }
    let result = with_ext2_pub(|fs| -> Result<([u8; 512], usize, u64), FsError> {
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
        Some(Err(e)) => print_error!("  {}cat: {:?}", prefix, e),
        None         => print_error!("  {} not mounted (run {}mount first)", prefix, prefix),
    }
}

pub fn impl_stat(path: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if path.is_empty() { println!("Usage: {}stat <path>", prefix); return; }
    let result = with_ext2_pub(|fs| -> Result<(u32, Inode, Option<bool>), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        // Whether there is a journal to report is the filesystem's answer, not
        // the command name's
        let journal = fs.superblock.has_journal().then_some(fs.journal_active);
        Ok((ino, inode, journal))
    });
    match result {
        Some(Ok((ino, inode, journal))) => {
            println!("  Inode:  {}", ino);
            println!("  Type:   {:?}", inode.file_type());
            println!("  Mode:   0o{:o}", inode.permissions());
            println!("  Size:   {} bytes", inode.size());
            println!("  Links:  {}", inode.links_count());
            println!("  Blocks: {}", inode.blocks());
            println!("  UID:    {}", inode.uid_full());
            println!("  GID:    {}", inode.gid_full());
            if inode.uses_extents()   { println!("  Extents: yes"); }
            if inode.has_inline_data(){ println!("  Inline:  yes"); }
            if let Some(j) = journal  { println!("  Journal: {}", if j { "active" } else { "inactive" }); }
            if inode.is_fast_symlink() {
                let target = inode.fast_symlink_target();
                if let Ok(t) = core::str::from_utf8(target) { println!("  Target: {}", t); }
            }
        }
        Some(Err(e)) => print_error!("  {}stat: {:?}", prefix, e),
        None         => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_write(path: &str, text: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if path.is_empty() || text.is_empty() {
        println!("Usage: {}write <path> <text>", prefix);
        return;
    }
    let sw = crate::kcore::time::Stopwatch::start();
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        fs.reader.reset_io();
        let (parent_ino, filename) = resolve_parent_and_name(fs, path)?;
        let data = text.as_bytes();
        let ino = fs.ext3_write_file_create_or_overwrite(parent_ino, filename, 0o644, data)?;
        crate::serial_println!("[io] ata_commands={}", fs.reader.io_count);
        Ok(ino)
    });
    let ms = sw.elapsed_ms();
    // Written by inode, past the VFS: no cached page of it may outlive this
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(ino)) => print_success!("  written to inode {}  [disk {}ms]", ino, ms),
        Some(Err(e))  => print_error!("  {}write: {:?}", prefix, e),
        None          => print_error!("  {} not mounted", prefix),
    }
    crate::serial_println!("[timing] {}write disk={}ms", prefix, ms);
}

pub fn impl_mkdir(path: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if path.is_empty() { println!("Usage: {}mkdir <path>", prefix); return; }
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let (parent_ino, dirname) = resolve_parent_and_name(fs, path)?;
        if fs.superblock.has_extents() {
            fs.ext4_create_dir(parent_ino, dirname, 0o755)
        } else {
            fs.ext2_create_dir(parent_ino, dirname, 0o755)
        }
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(ino)) => print_success!("  created dir inode {}", ino),
        Some(Err(e))  => print_error!("  {}mkdir: {:?}", prefix, e),
        None          => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_rm(path: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if path.is_empty() { println!("Usage: {}rm <path>", prefix); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        let inode = {
            let ino = fs.resolve_path(path)?;
            fs.read_inode(ino)?
        };
        if inode.uses_extents() {
            fs.ext4_delete_file(parent_ino, name)
        } else {
            fs.ext2_delete_file(parent_ino, name)
        }
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(())) => print_success!("  deleted"),
        Some(Err(e)) => print_error!("  {}rm: {:?}", prefix, e),
        None         => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_rmdir(path: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if path.is_empty() { println!("Usage: {}rmdir <path>", prefix); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let (parent_ino, name) = resolve_parent_and_name(fs, path)?;
        let inode = {
            let ino = fs.resolve_path(path)?;
            fs.read_inode(ino)?
        };
        if inode.uses_extents() {
            fs.ext4_delete_dir(parent_ino, name)
        } else {
            fs.ext2_delete_dir(parent_ino, name)
        }
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(())) => print_success!("  removed dir"),
        Some(Err(e)) => print_error!("  {}rmdir: {:?}", prefix, e),
        None         => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_append(path: &str, text: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if path.is_empty() || text.is_empty() {
        println!("Usage: {}append <path> <text>", prefix);
        return;
    }
    let result = with_ext2_pub(|fs| -> Result<usize, FsError> {
        let ino = fs.resolve_path(path)?;
        if fs.superblock.has_extents() {
            fs.ext4_append_file(ino, text.as_bytes())
        } else {
            fs.ext2_append_file(ino, text.as_bytes())
        }
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(n))  => print_success!("  appended {} bytes", n),
        Some(Err(e)) => print_error!("  {}append: {:?}", prefix, e),
        None         => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_tree(path: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    let path = if path.is_empty() { "/" } else { path };
    let mut tree = TreeResult::new();
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
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
                if e.is_dir            { cprintln!(0, 220, 220, "{}/", e.name_str()); }
                else if e.is_symlink   { cprintln!(128, 222, 217, "{}@", e.name_str()); }
                else                   { cprintln!(230, 240, 240, "{} ({}b)", e.name_str(), e.size); }
            }
            println!("  {} entries", tree.count);
        }
        Some(Err(e)) => print_error!("  {}tree: {:?}", prefix, e),
        None         => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_du(path: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    let path = if path.is_empty() { "/" } else { path };
    let result = with_ext2_pub(|fs| -> Result<(u32, u64), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.ext2_dir_size(ino)
    });
    match result {
        Some(Ok((files, bytes))) => {
            println!("  {} files, {} bytes total", files, bytes);
            if bytes >= 1024 { println!("  ({} KB)", bytes / 1024); }
        }
        Some(Err(e)) => print_error!("  {}du: {:?}", prefix, e),
        None         => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_cp(src: &str, dst: &str, prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    if src.is_empty() || dst.is_empty() { println!("Usage: {}cp <src> <dst>", prefix); return; }
    let result = with_ext2_pub(|fs| -> Result<u32, FsError> {
        let src_ino = fs.resolve_path(src)?;
        let (dst_parent_ino, dst_name) = resolve_parent_and_name(fs, dst)?;
        fs.ext4_copy_file(src_ino, dst_parent_ino, dst_name)
    });
    match result {
        Some(Ok(ino)) => print_success!("  copied to inode {}", ino),
        Some(Err(e))  => print_error!("  {}cp: {:?}", prefix, e),
        None          => print_error!("  {} not mounted", prefix),
    }
}

pub fn impl_info(prefix: &'static str) {
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    let result = with_ext2_pub(|fs| fs.fs_info());
    match result {
        Some(info) => {
            cprintln!(57, 197, 187, "  {} Filesystem Info", info.version.to_ascii_uppercase());
            println!("  Version:    {}", info.version);
            println!("  Block size: {} bytes", info.block_size);
            println!("  Blocks:     {} / {} used",
                info.total_blocks - info.free_blocks, info.total_blocks);
            println!("  Inodes:     {} / {} used",
                info.total_inodes - info.free_inodes, info.total_inodes);
            println!("  Groups:     {}", info.groups);
            println!("  Journal:    {}", if info.has_journal { "yes" } else { "no" });
            if info.has_extents { println!("  Extents:    yes"); }
        }
        None => print_error!("  {} not mounted", prefix),
    }
}

/// `sync`: makes every mounted filesystem durable.
///
/// `MikuFS::sync` commits the running transaction before it checkpoints, the
/// ordering the journal exists to keep
pub fn impl_sync(prefix: &'static str) {
    let sw = crate::kcore::time::Stopwatch::start();
    // The page cache's dirty pages first: the filesystems' sync makes durable
    // only what they have been given
    crate::fs::vfs::core::writeback::writeback_all();
    let result = crate::fs::ext::mount::sync_all_mounts();
    let ms = sw.elapsed_ms();
    match result {
        Ok(0) => print_error!("  {} not mounted", prefix),
        Ok(n) => {
            print_success!("  synced {} filesystem(s) [{}ms]", n, ms);
            crate::serial_println!("[timing] sync {} mounts in {}ms", n, ms);
        }
        Err(e) => print_error!("  sync: {:?}", e),
    }
}

/// `extfsck`, and `extfsck -y` to let it write.
///
/// One implementation for all three flavours: the check does not care which is
/// mounted
pub fn impl_fsck(repair: bool) {
    use crate::fs::ext::fsck::FsckOptions;
    use crate::shell::commands::ext2_cmds::with_ext2_pub;

    let opts = if repair { FsckOptions::repair() } else { FsckOptions::check() };
    let result = with_ext2_pub(|fs| (fs.fsck(opts), fs.superblock.fs_version_str()));

    let (report, fs_name) = match result {
        Some(v) => v,
        None => { print_error!("  no ext2/3/4 filesystem mounted"); return; }
    };
    let r = match report {
        Ok(r) => r,
        Err(e) => { print_error!("  fsck could not run: {:?}", e); return; }
    };
    if !r.checked {
        print_error!("  fsck stopped: the superblock is not usable");
        return;
    }

    cprintln!(57, 197, 187, "  {} filesystem check", fs_name);
    println!("  Block size:   {} bytes", r.block_size);
    println!("  Blocks:       {} used / {} total", r.used_blocks, r.total_blocks);
    println!("  Inodes:       {} used / {} total, {} directories",
             r.used_inodes, r.total_inodes, r.used_dirs);

    for f in r.examples() {
        if f.fixed {
            print_success!("  fixed: {}", f.problem);
        } else {
            print_error!("  {}", f.problem);
        }
    }
    let hidden = r.findings.truncated();
    if hidden > 0 {
        println!("  ... and {} more", hidden);
    }

    if r.errors() == 0 {
        print_success!("  filesystem ok");
    } else if r.left == 0 {
        print_success!("  {} problems, all repaired", r.errors());
    } else if r.repairing {
        print_error!("  {} problems, {} repaired, {} left", r.errors(), r.fixed, r.left);
    } else {
        print_error!("  {} problems found - `extfsck -y` to repair", r.errors());
    }
}

