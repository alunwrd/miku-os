use crate::io::console;
use crate::shell::SESSION;
use crate::fs::vfs::{
    self, with_vfs, with_vfs_ro, FileMode, OpenFlags, SeekFrom, VNodeKind, VfsError, VfsResult,
    InodeId, FsType, NAME_LEN,
};
use crate::{cprintln, print, print_error, print_success, print_warn, println, serial_println};





pub fn cmd_cd(arg: &str) {
    if arg.is_empty() {
        let mut s = SESSION.lock();
        s.cwd = 0;
        s.path[0] = b'/';
        s.plen = 1;
        return;
    }

    let cwd = SESSION.lock().cwd;
    let vfs_result = with_vfs(|vfs| match vfs.resolve_path(cwd, arg) {
        Ok(id) if vfs.nodes[id].is_dir() => Ok(id),
        Ok(_)  => Err(vfs::VfsError::NotDirectory),
        Err(e) => Err(e),
    });

    if let Ok(new_id) = vfs_result {
        let mut s = SESSION.lock();
        s.cwd = new_id;
        crate::shell::update_path(&mut s, arg);
        return;
    }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() {
        print_error!("cd: {:?}", vfs_result.unwrap_err());
        return;
    }
    let ext2_path = {
        let s = SESSION.lock();
        let base = unsafe { core::str::from_utf8_unchecked(&s.path[..s.plen]) };
        let mut buf = [0u8; 256];
        let n = if arg.starts_with('/') {
            let b = arg.as_bytes();
            let l = b.len().min(255);
            buf[..l].copy_from_slice(&b[..l]);
            l
        } else {
            let base_b = base.as_bytes();
            let bl = base_b.len().min(255);
            buf[..bl].copy_from_slice(&base_b[..bl]);
            let mut l = bl;
            if l > 0 && buf[l - 1] != b'/' && l < 255 {
                buf[l] = b'/';
                l += 1;
            }
            let ab = arg.as_bytes();
            let al = ab.len().min(255 - l);
            buf[l..l + al].copy_from_slice(&ab[..al]);
            l + al
        };
        (buf, n)
    };
    let path_str = unsafe { core::str::from_utf8_unchecked(&ext2_path.0[..ext2_path.1]) };
    let ext2_info = crate::shell::commands::ext2_cmds::with_ext2_pub(|fs| {
        let ino = fs.resolve_path(path_str).map_err(|_| vfs::VfsError::NotFound)?;
        let inode = fs.read_inode(ino).map_err(|_| vfs::VfsError::IoError)?;
        if !inode.is_directory() {
            return Err(vfs::VfsError::NotDirectory);
        }
        Ok(ino)
    });

    let ext2_ino = match ext2_info {
        Some(Ok(ino)) => ino,
        Some(Err(e))  => { print_error!("cd: {:?}", e); return; }
        None          => { print_error!("cd: ext not mounted"); return; }
    };
    let dirname = arg.trim_end_matches('/').rsplit('/').next().unwrap_or(arg);
    let new_id = with_vfs(|vfs| -> VfsResult<usize> {

        if let Ok(id) = vfs.resolve_path(cwd, dirname) {
            if vfs.nodes[id].is_dir() { return Ok(id); }
        }
        let id = vfs.alloc_vnode()?;
        let ts: crate::fs::vfs::Timestamp = 0;
        // The flavour of the filesystem this directory is on, from the
        // directory itself.
        //
        // Every mount table numbers its slots from zero, so
        // `ext::mount::fs_type_of(slot)` with a slot from the vnode would
        // answer for whatever ext has in that slot when the `cwd` is on btrfs
        // or memfs. As `dev/fs-architecture.md` states: "which filesystem" is
        // answered from the node. A child is on its parent's filesystem
        let slot = vfs.nodes[cwd].ext2_slot;
        let fs_type = vfs.nodes[cwd].fs_type;
        vfs.nodes[id].init(
            id as InodeId,
            cwd as InodeId,
            dirname,
            VNodeKind::Directory,
            fs_type,
            FileMode::new(0o755),
            0, 0, ts,
        );
        vfs.nodes[id].ext2_ino = ext2_ino;
        vfs.nodes[id].ext2_slot = slot;
        vfs.nodes[id].children_loaded = false;
        vfs.nodes[cwd].children.insert(dirname, id as InodeId);
        Ok(id)
    });

    match new_id {
        Ok(id) => {
            let mut s = SESSION.lock();
            s.cwd = id;
            crate::shell::update_path(&mut s, arg);
        }
        Err(e) => print_error!("cd: {:?}", e),
    }
}

pub fn cmd_pwd() {
    let s = SESSION.lock();
    let p = unsafe { core::str::from_utf8_unchecked(&s.path[..s.plen]) };
    cprintln!(0, 220, 220, "{}", p);
}





/// Prints an error and returns true when the session may not read this path.
///
/// For builtins that resolve a path and read the bytes themselves rather than
/// going through `open`, whose permission check does not happen for them.
///
/// Metadata commands need no check here: POSIX requires search permission on
/// the path to stat a file, and path resolution enforces that
fn deny_read(path: &str) -> bool {
    let cwd = SESSION.lock().cwd;
    match crate::fs::vfs::core::may_read_path(cwd, path) {
        Ok(()) => false,
        Err(crate::fs::vfs::types::VfsError::PermissionDenied) => {
            print_error!("  {}: permission denied", path);
            true
        }
        // Anything else (not found, not a directory) is left to the command,
        // which has a better message
        Err(_) => false,
    }
}


/// Parse a dd-style size: a decimal count with an optional K/M/G suffix
/// (powers of 1024, like coreutils dd)
fn dd_parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult): (&str, u64) = match s.as_bytes().last() {
        Some(b'K') | Some(b'k') => (&s[..s.len() - 1], 1024),
        Some(b'M') | Some(b'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G') | Some(b'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.parse::<u64>().ok().and_then(|v| v.checked_mul(mult))
}

/// dd(1): copy blocks between any two VFS objects - regular/ext files,
/// /dev/zero, and the raw /dev/blkN block-device nodes. Supports
/// if= of= bs= count= skip= seek= conv=notrunc,fsync
pub fn cmd_dd(args: &str) {
    /// Cap a single block (and the heap buffer) at 8 MiB
    const MAX_BS: u64 = 8 * 1024 * 1024;

    let mut if_path = "";
    let mut of_path = "";
    let mut bs: u64 = 512;
    let mut count: Option<u64> = None;
    let mut skip: u64 = 0;
    let mut seek: u64 = 0;
    let mut conv_notrunc = false;
    let mut conv_fsync = false;

    for tok in args.split_whitespace() {
        if let Some(v) = tok.strip_prefix("if=") {
            if_path = v;
        } else if let Some(v) = tok.strip_prefix("of=") {
            of_path = v;
        } else if let Some(v) = tok.strip_prefix("bs=") {
            match dd_parse_size(v) { Some(x) => bs = x, None => { print_error!("dd: bad bs"); return; } }
        } else if let Some(v) = tok.strip_prefix("count=") {
            match dd_parse_size(v) { Some(x) => count = Some(x), None => { print_error!("dd: bad count"); return; } }
        } else if let Some(v) = tok.strip_prefix("skip=") {
            match dd_parse_size(v) { Some(x) => skip = x, None => { print_error!("dd: bad skip"); return; } }
        } else if let Some(v) = tok.strip_prefix("seek=") {
            match dd_parse_size(v) { Some(x) => seek = x, None => { print_error!("dd: bad seek"); return; } }
        } else if let Some(v) = tok.strip_prefix("conv=") {
            for c in v.split(',') {
                match c {
                    "notrunc" => conv_notrunc = true,
                    "fsync" | "sync" => conv_fsync = true,
                    other => { print_error!("dd: unknown conv '{}'", other); return; }
                }
            }
        } else {
            print_error!("dd: unknown operand '{}'", tok);
            return;
        }
    }

    if if_path.is_empty() || of_path.is_empty() {
        println!("Usage: dd if=<src> of=<dst> [bs=N] [count=N] [skip=N] [seek=N] [conv=notrunc,fsync]");
        return;
    }
    if bs == 0 || bs > MAX_BS {
        print_error!("dd: bs must be 1..={} bytes", MAX_BS);
        return;
    }

    let cwd = SESSION.lock().cwd;

    // Open both ends and apply skip/seek up front (offsets persist on the fd)
    let fds = with_vfs(|v| -> VfsResult<(usize, usize)> {
        let src = v.open(cwd, if_path, OpenFlags(OpenFlags::READ), FileMode::default_file())?;
        let mut dflags = OpenFlags::WRITE | OpenFlags::CREATE;
        if !conv_notrunc {
            dflags |= OpenFlags::TRUNCATE;
        }
        let dst = match v.open(cwd, of_path, OpenFlags(dflags), FileMode::default_file()) {
            Ok(d) => d,
            Err(e) => { let _ = v.close(src); return Err(e); }
        };
        if skip > 0 {
            if let Err(e) = v.seek(src, SeekFrom::Start(skip * bs)) {
                let _ = v.close(src); let _ = v.close(dst); return Err(e);
            }
        }
        if seek > 0 {
            if let Err(e) = v.seek(dst, SeekFrom::Start(seek * bs)) {
                let _ = v.close(src); let _ = v.close(dst); return Err(e);
            }
        }
        Ok((src, dst))
    });
    let (src, dst) = match fds {
        Ok(x) => x,
        Err(e) => { print_error!("dd: {:?}", e); return; }
    };

    let mut buf = alloc::vec![0u8; bs as usize];
    let mut records_in = 0u64;
    let mut records_out = 0u64;
    let mut total = 0u64;
    let mut short = false;

    // One block per VFS-lock acquisition, so a large copy never holds the
    // global lock for the whole transfer
    loop {
        if let Some(c) = count {
            if records_in >= c { break; }
        }
        let step = with_vfs(|v| -> VfsResult<(usize, usize)> {
            let n = v.read(src, &mut buf)?;
            if n == 0 {
                return Ok((0, 0));
            }
            let mut w = 0usize;
            while w < n {
                let x = v.write(dst, &buf[w..n])?;
                if x == 0 { break; }
                w += x;
            }
            Ok((n, w))
        });
        match step {
            Ok((0, _)) => break,
            Ok((n, w)) => {
                records_in += 1;
                records_out += 1;
                total += w as u64;
                if w < n { short = true; break; }
            }
            Err(e) => { print_error!("dd: {:?}", e); break; }
        }
    }

    with_vfs(|v| {
        if conv_fsync {
            let _ = v.fsync(dst);
        }
        let _ = v.close(src);
        let _ = v.close(dst);
    });

    if short {
        print_warn!("dd: short write (destination full?)");
    }
    print_success!(
        "  {}+0 records in, {}+0 records out, {} bytes ({} KB) copied",
        records_in, records_out, total, total / 1024
    );
}

pub fn cmd_write(name: &str, text: &str) {
    let cwd = SESSION.lock().cwd;
    with_vfs(|v| {
        let fl = OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE);
        match v.open(cwd, name, fl, FileMode::default_file()) {
            Ok(fd) => {
                match v.write(fd, text.as_bytes()) {
                    Ok(n) => print_success!("\nWrote {} bytes", n),
                    Err(e) => print_error!("write: {:?}", e),
                }
                let _ = v.close(fd);
            }
            Err(e) => print_error!("write: {:?}", e),
        }
    });
}




fn recursive_rm(cwd: usize, path: &str) {
    let mut child_names: [[u8; 23]; 16] = [[0; 23]; 16];
    let mut child_lens: [u8; 16] = [0; 16];
    let mut child_kinds: [u8; 16] = [0; 16];
    let mut child_count: usize = 0;

    with_vfs(|vfs| {
        let did = match vfs.resolve_path(cwd, path) {
            Ok(v) => v,
            Err(_) => return,
        };
        let eff = vfs.xm(did);
        for (name, child_id) in vfs.nodes[eff].children.iter_named() {
            if child_count >= 16 {
                break;
            }
            let cid = child_id as usize;
            if !vfs.nodes.contains(cid) || !vfs.nodes[cid].active {
                continue;
            }
            let nm = name.as_bytes();
            let l = nm.len().min(23);
            child_names[child_count][..l].copy_from_slice(&nm[..l]);
            child_lens[child_count] = l as u8;
            child_kinds[child_count] = vfs.nodes[cid].kind as u8;
            child_count += 1;
        }
    });

    for i in 0..child_count {
        let name =
            unsafe { core::str::from_utf8_unchecked(&child_names[i][..child_lens[i] as usize]) };

        let mut child_path = [0u8; 64];
        let mut cp_len = 0;
        for &b in path.as_bytes() {
            if cp_len < 63 {
                child_path[cp_len] = b;
                cp_len += 1;
            }
        }
        if cp_len < 63 && cp_len > 0 && child_path[cp_len - 1] != b'/' {
            child_path[cp_len] = b'/';
            cp_len += 1;
        }
        for &b in name.as_bytes() {
            if cp_len < 63 {
                child_path[cp_len] = b;
                cp_len += 1;
            }
        }
        let child_path_str = unsafe { core::str::from_utf8_unchecked(&child_path[..cp_len]) };

        if child_kinds[i] == 1 {
            recursive_rm(cwd, child_path_str);
        } else {
            let _ = with_vfs(|v| v.unlink(cwd, child_path_str));
        }
    }

    match with_vfs(|v| v.rmdir(cwd, path)) {
        Ok(_) => {}
        Err(e) => print_error!("rm -rf {}: {:?}", path, e),
    }
}









pub fn cmd_mount_list() {
    cprintln!(128, 222, 217, "  Filesystem    Mountpoint    Type");
    cprintln!(230, 240, 240, "  tmpfs         /             tmpfs");
    cprintln!(230, 240, 240, "  devfs         /dev          devfs");
    cprintln!(230, 240, 240, "  procfs        /proc         procfs");

    let mounts = crate::shell::commands::ext2_cmds::mounted_slots_snapshot();

    with_vfs_ro(|vfs| {
        for entry in mounts.iter().flatten() {
            let (slot, vnode, fs_name) = *entry;
            let path = vnode_path(vfs, vnode as usize);
            let path_str = path.as_str();
            // The device column names the disk rather than repeating the
            // filesystem type
            let drive = crate::fs::ext::mount::slot_drive(slot).unwrap_or(0);
            let ro = if crate::fs::ext::mount::slot_read_only(slot) { " (ro)" } else { "" };
            cprintln!(
                230, 240, 240,
                "  blk{:<11}{:<14}{}{}",
                drive, path_str, fs_name, ro
            );
        }
    });
}

/// Builds the absolute path of a vnode by walking parent links. Returns "/" for
/// the root and a [u8; 256]-backed buffer otherwise
fn vnode_path(vfs: &vfs::core::MikuVFS, mut id: usize) -> PathBuf {
    let mut buf = PathBuf::new();
    if id == 0 || !vfs.nodes[id].active {
        buf.push_byte(b'/');
        return buf;
    }
    // Collect names from leaf upward, then write them out in reverse
    let mut names: [&str; 32] = [""; 32];
    let mut depth = 0usize;
    while id != 0 && depth < names.len() {
        names[depth] = vfs.nodes[id].name.as_str();
        depth += 1;
        let p = vfs.nodes[id].parent as usize;
        if p == id { break; }
        id = p;
    }
    for i in (0..depth).rev() {
        buf.push_byte(b'/');
        buf.push_str(names[i]);
    }
    if depth == 0 { buf.push_byte(b'/'); }
    buf
}

struct PathBuf {
    data: [u8; 256],
    len:  usize,
}

impl PathBuf {
    fn new() -> Self { Self { data: [0; 256], len: 0 } }
    fn push_byte(&mut self, b: u8) {
        if self.len < self.data.len() { self.data[self.len] = b; self.len += 1; }
    }
    fn push_str(&mut self, s: &str) {
        for &b in s.as_bytes() { self.push_byte(b); }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.data[..self.len]).unwrap_or("/")
    }
}

pub fn cmd_mount(fstype: &str, target: &str) {
    match fstype {
        "ext2" | "ext3" | "ext4" => {
            let mountpoint = if target.is_empty() { "/mnt" } else { target };
            mount_ext2_to_vfs(mountpoint);
        }
        "btrfs" => {
            let mountpoint = if target.is_empty() { "/mnt" } else { target };
            mount_btrfs_to_vfs(mountpoint);
        }
        _ => {
            print_error!("mount: unknown filesystem '{}'", fstype);
            println!("  Supported: ext2, ext3, ext4, btrfs");
        }
    }
}

/// Finds a btrfs volume and grafts it onto the tree.
///
/// Unlike the ext path this does its own probing: the volume is read-only, so
/// there is no `rw` to ask for and no `btrfsmount` to run first. The registry
/// is asked for every device and partition, and the first btrfs found is
/// mounted.
fn mount_btrfs_to_vfs(mountpoint: &str) {
    use crate::fs::btrfs::mount as bmount;

    // Already mounted somewhere? That slot is used rather than a second view of
    // the same disk
    let slot = (0..bmount::MAX_MOUNTS as u8)
        .find(|&s| bmount::is_ready(s))
        .or_else(|| {
            let found = crate::fs::ext::mount::scan_volumes();
            let p = found.iter().find(|p| p.fs_type == FsType::Btrfs)?;
            let dev = crate::fs::ext::mount::dev_for_idx(p.drive);
            bmount::try_mount_on(dev, p.drive, p.start_lba)
        });

    let Some(slot) = slot else {
        print_error!("  no btrfs filesystem found on any drive");
        return;
    };

    serial_println!("[mount] mounting btrfs slot {} at {}", slot, mountpoint);
    let cwd = SESSION.lock().cwd;

    let result = with_vfs(|vfs| {
        let mount_id = match vfs.resolve_path(cwd, mountpoint) {
            Ok(id) => {
                if !vfs.nodes[id].is_dir() {
                    return Err(VfsError::NotDirectory);
                }
                if !vfs.nodes[id].children.is_empty() || vfs.nodes[id].fs_type.has_mounted_fs() {
                    vfs.evict_children_recursive(id);
                }
                id
            }
            Err(VfsError::NotFound) => {
                let (parent_path, dirname) = vfs::path::PathWalker::split_last(mountpoint);
                let parent_id = vfs.resolve_path(cwd, parent_path)?;
                vfs.mkdir(parent_id, dirname, FileMode::default_dir())?
            }
            Err(e) => return Err(e),
        };

        vfs.nodes[mount_id].fs_type = FsType::Btrfs;
        // The subvolume's own top directory. Not zero: zero is
        // `is_ext_backed`'s "nothing behind this"
        vfs.nodes[mount_id].ext2_ino = 256;
        vfs.nodes[mount_id].ext2_slot = slot;
        vfs.nodes[mount_id].children_loaded = false;
        // What the directory under the mount did not have says nothing
        // about the mounted one
        vfs.nodes[mount_id].children.drop_negatives();
        vfs.ext2_mount_active = true;
        Ok(mount_id)
    });

    match result {
        Ok(id) => {
            bmount::set_mount_vnode(slot, id as InodeId);
            print_success!("  btrfs mounted at {} (read-only)", mountpoint);
        }
        Err(e) => print_error!("mount: {:?}", e),
    }
}

pub fn cmd_umount(path: &str) {
    // btrfs first, the cheap case: nothing was written, so nothing to flush and
    // no slot bookkeeping
    {
        let cwd = SESSION.lock().cwd;
        let found = with_vfs(|vfs| -> VfsResult<(usize, u8)> {
            let id = vfs.resolve_path(cwd, path)?;
            if vfs.nodes[id].fs_type != FsType::Btrfs {
                return Err(VfsError::InvalidArgument);
            }
            let slot = crate::fs::btrfs::mount::slot_at_vnode(id as InodeId)
                .ok_or(VfsError::InvalidArgument)?;
            vfs.evict_children_recursive(id);
            vfs.nodes[id].fs_type = FsType::TmpFS;
            vfs.nodes[id].ext2_ino = 0;
            vfs.nodes[id].ext2_slot = crate::fs::vfs::types::INVALID_U8;
            vfs.nodes[id].children_loaded = false;
            Ok((id, slot))
        });
        if let Ok((_, slot)) = found {
            crate::fs::btrfs::mount::unmount_slot(slot);
            print_success!("  unmounted {}", path);
            return;
        }
    }

    // Files' pages not written out yet go before the mount does
    crate::fs::vfs::core::writeback::writeback_all();
    crate::shell::commands::ext2_cmds::with_ext2_pub(|fs| {
        let _ = fs.flush_all_dirty_metadata();
    });

    let cwd = SESSION.lock().cwd;
    // Resolves path to a vnode and verifies it is a registered ext mount root.
    // Returns the slot index with the vnode id so the caller tears down the
    // matching slot, not whichever is active
    let result = with_vfs(|vfs| -> VfsResult<(usize, usize)> {
        let id = vfs.resolve_path(cwd, path)?;

        if !vfs.nodes[id].fs_type.is_ext_family() {
            return Err(VfsError::InvalidArgument);
        }

        let slot = crate::shell::commands::ext2_cmds::slot_for_vnode(id as InodeId)
            .ok_or(VfsError::InvalidArgument)?;

        vfs.evict_children_recursive(id);
        vfs.nodes[id].fs_type = FsType::TmpFS;
        vfs.nodes[id].ext2_ino = 0;
        vfs.nodes[id].ext2_slot = crate::fs::vfs::types::INVALID_U8;
        vfs.nodes[id].children_loaded = false;
        Ok((id, slot))
    });

    match result {
        Ok((_id, slot)) => {
            crate::shell::commands::ext2_cmds::unmount_slot(slot);
            // Keeps the "any ext mounted?" flag in sync, from a fresh snapshot:
            // other slots may still be mounted on different paths
            let any_left = crate::shell::commands::ext2_cmds::mounted_slots_snapshot()
                .iter()
                .any(|e| e.is_some());
            with_vfs(|vfs| { vfs.ext2_mount_active = any_left; });
            print_success!("  unmounted {}", path);
        }
        Err(e) => print_error!("umount: {:?}", e),
    }
}

fn mount_ext2_to_vfs(mountpoint: &str) {
    use crate::shell::commands::ext2_cmds;
    use crate::fs::ext::structs::EXT2_ROOT_INO;

    if !ext2_cmds::is_ext2_ready() {
        print_error!("  no ext2/3/4 filesystem mounted. Run ext2mount first");
        return;
    }

    serial_println!("[mount] mounting ext2 at {} (lazy)", mountpoint);

    let cwd = SESSION.lock().cwd;

    let result = with_vfs(|vfs| {
        let mount_id = match vfs.resolve_path(cwd, mountpoint) {
            Ok(id) => {
                if !vfs.nodes[id].is_dir() {
                    return Err(VfsError::NotDirectory);
                }
                // `has_mounted_fs`, not `is_ext_family`: mounting over a point
                // with a filesystem behind it has to drop the children either
                // way, as the umount path beside this asks
                if !vfs.nodes[id].children.is_empty() || vfs.nodes[id].fs_type.has_mounted_fs() {
                    vfs.evict_children_recursive(id);
                }
                id
            }
            Err(VfsError::NotFound) => {
                let (parent_path, dirname) = vfs::path::PathWalker::split_last(mountpoint);
                let parent_id = vfs.resolve_path(cwd, parent_path)?;
                vfs.mkdir(parent_id, dirname, FileMode::default_dir())?
            }
            Err(e) => return Err(e),
        };

        let slot = crate::shell::commands::ext2_cmds::active_slot_index() as u8;
        vfs.nodes[mount_id].fs_type = crate::fs::ext::mount::fs_type_of(slot)
            .unwrap_or(crate::fs::vfs::types::FsType::Ext2);
        vfs.nodes[mount_id].ext2_ino = EXT2_ROOT_INO;
        // The mount root is where a slot enters the VFS tree; everything
        // under it inherits this on the way down
        vfs.nodes[mount_id].ext2_slot = slot;
        vfs.nodes[mount_id].children_loaded = false;
        vfs.nodes[mount_id].children.drop_negatives();
        vfs.ext2_mount_active = true;

        Ok(mount_id)
    });

    match result {
        Ok(id) => {
            // Tracks which VFS vnode now belongs to the active ext slot so a
            // later umount <path> can find the right slot
            let active = crate::shell::commands::ext2_cmds::active_slot_index();
            crate::shell::commands::ext2_cmds::register_mount_vnode(active, id as InodeId);
            print_success!("  {} mounted at {} (on-demand)",
                           crate::fs::ext::mount::ext_fs_version_tag(), mountpoint);
        }
        Err(e) => print_error!("mount: {:?}", e),
    }
}
pub fn ext_version() -> &'static str {
    crate::shell::commands::ext2_cmds::ext_fs_version_tag()
}

pub fn cmd_extwrite(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() {
        println!("Usage: extwrite <path> <text>");
        return;
    }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }

    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    match ext_version() {
        "ext4" => crate::shell::commands::ext4_cmds::cmd_ext4_write(abs_str, text),
        "ext3" => crate::shell::commands::ext3_cmds::cmd_ext3_write(abs_str, text),
        _      => crate::shell::commands::ext2_cmds::cmd_ext2_write(abs_str, text),
    }
}

pub fn cmd_extcp(src: &str, dst: &str) {
    if src.is_empty() || dst.is_empty() { println!("Usage: extcp <src> <dst>"); return; }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }

    let abs_src = make_abs_path(src);
    let abs_dst = make_abs_path(dst);
    let s = unsafe { core::str::from_utf8_unchecked(&abs_src.0[..abs_src.1]) };
    let d = unsafe { core::str::from_utf8_unchecked(&abs_dst.0[..abs_dst.1]) };

    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    use crate::shell::commands::ext_cmds_common::resolve_parent_and_name;
    let result = with_ext2_pub(|fs| -> Result<u32, crate::fs::ext::FsError> {
        let src_ino = fs.resolve_path(s)?;
        let (dst_parent_ino, dst_name) = resolve_parent_and_name(fs, d)?;
        fs.ext4_copy_file(src_ino, dst_parent_ino, dst_name)
    });
    match result {
        Some(Ok(ino)) => print_success!("  copied to inode {}", ino),
        Some(Err(e))  => print_error!("  extcp: {:?}", e),
        None          => print_error!("  ext not mounted"),
    }
}

pub fn cmd_extmv(old: &str, new_name: &str) {
    if old.is_empty() || new_name.is_empty() { println!("Usage: extmv <path> <newname>"); return; }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }

    let abs_old = make_abs_path(old);
    let s = unsafe { core::str::from_utf8_unchecked(&abs_old.0[..abs_old.1]) };

    let actual_new = match new_name.rfind('/') {
        Some(p) => &new_name[p+1..],
        None    => new_name,
    };
    if actual_new.is_empty() { print_error!("  extmv: invalid new name"); return; }

    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    use crate::shell::commands::ext_cmds_common::resolve_parent_and_name;
    let result = with_ext2_pub(|fs| -> Result<(), crate::fs::ext::FsError> {
        let (parent_ino, old_nm) = resolve_parent_and_name(fs, s)?;
        fs.ext2_rename(parent_ino, old_nm, actual_new)
    });
    match result {
        Some(Ok(())) => print_success!("  renamed to {}", actual_new),
        Some(Err(e)) => print_error!("  extmv: {:?}", e),
        None         => print_error!("  ext not mounted"),
    }
}


pub fn cmd_extchmod(mode_str: &str, path: &str) {
    if mode_str.is_empty() || path.is_empty() { println!("Usage: extchmod <mode> <path>"); return; }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }
    let abs = make_abs_path(path);
    let s   = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    crate::shell::commands::ext2_cmds::cmd_ext2_chmod(mode_str, s);
}

pub fn cmd_extchown(uid_str: &str, gid_str: &str, path: &str) {
    if uid_str.is_empty() || path.is_empty() { println!("Usage: extchown <uid> <gid> <path>"); return; }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }
    let abs = make_abs_path(path);
    let s   = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    crate::shell::commands::ext2_cmds::cmd_ext2_chown(uid_str, gid_str, s);
}


pub fn cmd_extls(path: &str) {
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }
    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    match ext_version() {
        "ext4" => crate::shell::commands::ext4_cmds::cmd_ext4_ls(abs_str),
        "ext3" => crate::shell::commands::ext3_cmds::cmd_ext3_ls(abs_str),
        _      => crate::shell::commands::ext2_cmds::cmd_ext2_ls(abs_str),
    }
}

pub fn cmd_extmkdir(path: &str) {
    if path.is_empty() { println!("Usage: extmkdir <path>"); return; }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }
    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    match ext_version() {
        "ext4" => crate::shell::commands::ext4_cmds::cmd_ext4_mkdir(abs_str),
        "ext3" => crate::shell::commands::ext3_cmds::cmd_ext3_mkdir(abs_str),
        _      => crate::shell::commands::ext2_cmds::cmd_ext2_mkdir(abs_str),
    }
}

pub fn make_abs_path_pub(rel: &str) -> ([u8; 256], usize) { make_abs_path(rel) }

fn make_abs_path(rel: &str) -> ([u8; 256], usize) {
    let mut buf = [0u8; 256];
    if rel.starts_with('/') {
        let n = rel.as_bytes().len().min(255);
        buf[..n].copy_from_slice(&rel.as_bytes()[..n]);
        return (buf, n);
    }
    let s = SESSION.lock();
    let base = &s.path[..s.plen];
    let bl = base.len().min(255);
    buf[..bl].copy_from_slice(&base[..bl]);
    let mut l = bl;
    if rel.is_empty() || rel == "." {
        return (buf, l);
    }
    if l > 0 && buf[l-1] != b'/' && l < 255 { buf[l] = b'/'; l += 1; }
    let rb = rel.as_bytes();
    let rl = rb.len().min(255 - l);
    buf[l..l+rl].copy_from_slice(&rb[..rl]);
    (buf, l + rl)
}

pub fn cmd_exttouch(path: &str) {
    if path.is_empty() { println!("Usage: exttouch <path>"); return; }
    if !crate::shell::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }

    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    use crate::shell::commands::ext2_cmds::with_ext2_pub;
    use crate::shell::commands::ext_cmds_common::resolve_parent_and_name;
    let result = with_ext2_pub(|fs| -> Result<u32, crate::fs::ext::FsError> {
        let (parent_ino, filename) = resolve_parent_and_name(fs, abs_str)?;
 
        if let Some(ino) = fs.ext2_lookup_in_dir(parent_ino, filename)? {
            return Ok(ino);
        }
        fs.ext3_create_file(parent_ino, filename, 0o644)
    });
    // Changed past the VFS, by inode number
    crate::fs::vfs::core::changed_below_vfs();
    match result {
        Some(Ok(ino)) => print_success!("  created inode {}", ino),
        Some(Err(e))  => print_error!("  exttouch: {:?}", e),
        None          => print_error!("  ext not mounted"),
    }
}
