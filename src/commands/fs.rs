use crate::console;
use crate::shell::SESSION;
use crate::vfs::{
    self, with_vfs, with_vfs_ro, FileMode, OpenFlags, VNodeKind, VfsError, VfsResult,
    MAX_VNODES, InodeId, FsType, NAME_LEN,
};
use crate::{cprintln, print, print_error, print_success, println, serial_println};

fn print_ls_vnode(name: &str, kind: VNodeKind) {
    match kind {
        VNodeKind::Directory => cprintln!(0, 220, 220, "  {}/", name),
        VNodeKind::Symlink => cprintln!(128, 222, 217, "  {}@", name),
        VNodeKind::CharDevice | VNodeKind::BlockDevice => {
            cprintln!(220, 220, 100, "  {}*", name)
        }
        VNodeKind::Pipe | VNodeKind::Fifo | VNodeKind::Socket => {
            cprintln!(220, 220, 100, "  {}|", name)
        }
        _ => cprintln!(230, 240, 240, "  {}", name),
    }
}

fn print_ls_ext(name: &str, file_type: u8) {
    use crate::miku_extfs::structs::{FT_BLKDEV, FT_CHRDEV, FT_DIR, FT_FIFO, FT_SOCK, FT_SYMLINK};

    match file_type {
        FT_DIR => cprintln!(0, 220, 220, "  {}/", name),
        FT_SYMLINK => cprintln!(128, 222, 217, "  {}@", name),
        FT_CHRDEV | FT_BLKDEV => cprintln!(220, 220, 100, "  {}*", name),
        FT_FIFO | FT_SOCK => cprintln!(220, 220, 100, "  {}|", name),
        _ => cprintln!(230, 240, 240, "  {}", name),
    }
}

fn ls_ext_dir(dir_ino: u32) -> VfsResult<()> {
    use crate::miku_extfs::structs::DirEntry as ExtDirEntry;

    let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| -> VfsResult<()> {
        let inode = fs.read_inode(dir_ino).map_err(|_| VfsError::IoError)?;
        if !inode.is_directory() {
            return Err(VfsError::NotDirectory);
        }

        let mut entries = [const { ExtDirEntry::empty() }; 256];
        let count = fs.read_dir(&inode, &mut entries).map_err(|_| VfsError::IoError)?;

        serial_println!("[ls_ext] ino={} size={} bs={} entries={}", dir_ino, inode.size(), fs.block_size, count);

        cprintln!(120, 140, 140, "  ./");
        cprintln!(120, 140, 140, "  ../");

        for i in 0..count {
            let e = &entries[i];
            let name = e.name_str();
            if name == "." || name == ".." {
                continue;
            }
            print_ls_ext(name, e.file_type);
        }

        Ok(())
    });

    match result {
        Some(Ok(())) => Ok(()),
        Some(Err(e)) => Err(e),
        None => Err(VfsError::NotMounted),
    }
}

pub fn cmd_ls(path: &str) {
    let cwd = { SESSION.lock().cwd };
    let mut overlay_names = [[0u8; NAME_LEN]; 32];
    let mut overlay_lens = [0u8; 32];
    let mut overlay_kinds = [VNodeKind::Regular; 32];

    let vfs_info = with_vfs(|vfs| -> VfsResult<(FsType, u32, usize)> {
        let dir_id = vfs.resolve_path(cwd, path)?;
        if !vfs.nodes[dir_id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        let eff = vfs.xm(dir_id);
        let mut overlay_count = 0usize;
        for (_, child_id) in vfs.nodes[eff].children.iter() {
            if overlay_count >= overlay_names.len() {
                break;
            }
            let cid = child_id as usize;
            if cid >= MAX_VNODES || !vfs.nodes[cid].active || vfs.nodes[cid].ext2_ino != 0 {
                continue;
            }
            let name = vfs.nodes[cid].get_name().as_bytes();
            let len = name.len().min(NAME_LEN);
            overlay_names[overlay_count][..len].copy_from_slice(&name[..len]);
            overlay_lens[overlay_count] = len as u8;
            overlay_kinds[overlay_count] = vfs.nodes[cid].kind;
            overlay_count += 1;
        }
        Ok((vfs.nodes[dir_id].fs_type, vfs.nodes[dir_id].ext2_ino, overlay_count))
    });

    let mut overlay_count = 0usize;
    let ext2_ready = crate::commands::ext2_cmds::is_ext2_ready();

    let ext_lookup = match vfs_info {
        Ok((fs_type, ext2_ino, n)) => {
            overlay_count = n;
            serial_println!("[ls] vfs ok: fs={:?} ext2_ino={} overlay={} ext2_ready={}", fs_type, ext2_ino, n, ext2_ready);
            if fs_type.is_ext_family() && ext2_ino != 0 {
                serial_println!("[ls] path A: ext-backed vnode ext2_ino={}", ext2_ino);
                Some(Ok(ext2_ino))
            } else if ext2_ready {
                let abs = make_abs_path(path);
                let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
                serial_println!("[ls] path B: ext2 resolve '{}'", abs_str);
                crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    let ino = fs.resolve_path(abs_str).map_err(|e| {
                        serial_println!("[ls] ext2 resolve err: {:?}", e);
                        VfsError::NotFound
                    })?;
                    let inode = fs.read_inode(ino).map_err(|e| {
                        serial_println!("[ls] ext2 read_inode err: {:?}", e);
                        VfsError::IoError
                    })?;
                    if !inode.is_directory() {
                        return Err(VfsError::NotDirectory);
                    }
                    serial_println!("[ls] ext2 resolved ino={}", ino);
                    Ok(ino)
                })
            } else {
                serial_println!("[ls] path C: ext2 not ready, vfs-only");
                None
            }
        }
        Err(VfsError::NotFound) => {
            serial_println!("[ls] vfs NotFound, trying ext2 (ready={})", ext2_ready);
            if ext2_ready {
                let abs = make_abs_path(path);
                let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
                crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    let ino = fs.resolve_path(abs_str).map_err(|_| VfsError::NotFound)?;
                    let inode = fs.read_inode(ino).map_err(|_| VfsError::IoError)?;
                    if !inode.is_directory() {
                        return Err(VfsError::NotDirectory);
                    }
                    Ok(ino)
                })
            } else {
                None
            }
        }
        Err(VfsError::NotDirectory) => {
            print_error!("ls: not a directory");
            return;
        }
        Err(e) => {
            print_error!("ls: {:?}", e);
            return;
        }
    };

    if let Some(ext_lookup) = ext_lookup {
        serial_println!("[ls] ext_lookup={:?}", ext_lookup);
        match ext_lookup.and_then(ls_ext_dir) {
            Ok(()) => {
                serial_println!("[ls] ext ls ok, overlay={}", overlay_count);
                for i in 0..overlay_count {
                    let name =
                        unsafe { core::str::from_utf8_unchecked(&overlay_names[i][..overlay_lens[i] as usize]) };
                    if name != "." && name != ".." {
                        print_ls_vnode(name, overlay_kinds[i]);
                    }
                }
                return;
            }
            Err(VfsError::NotFound) => {
                serial_println!("[ls] ext ls NotFound, fallback to vfs");
            }
            Err(e) => {
                print_error!("ls: {:?}", e);
                serial_println!("[ls] ext ls err: {:?}", e);
                return;
            }
        }
    }

    serial_println!("[ls] vfs-only readdir");
    let mut entries = [vfs::DirEntry::empty(); 256];

    let count = match with_vfs(|vfs| -> VfsResult<usize> {
        let dir_id = vfs.resolve_path(cwd, path)?;
        if !vfs.nodes[dir_id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        vfs.readdir(dir_id, &mut entries)
    }) {
        Ok(n) => n,
        Err(VfsError::NotFound) => {
            print_error!("ls: not found");
            return;
        }
        Err(VfsError::NotDirectory) => {
            print_error!("ls: not a directory");
            return;
        }
        Err(e) => {
            print_error!("ls: {:?}", e);
            return;
        }
    };

    serial_println!("[ls] vfs readdir: {} entries", count);
    for i in 0..count {
        print_ls_vnode(entries[i].get_name(), entries[i].kind);
    }
}

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
    if !crate::commands::ext2_cmds::is_ext2_ready() {
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
    let ext2_info = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
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
        let ts: crate::vfs::Timestamp = 0;
        vfs.nodes[id].init(
            id as InodeId,
            cwd as InodeId,
            dirname,
            VNodeKind::Directory,
            crate::commands::ext2_cmds::active_fs_type(),
            FileMode::new(0o755),
            0, 0, ts,
        );
        vfs.nodes[id].ext2_ino = ext2_ino;
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

pub fn cmd_mkdir(name: &str) {
    let cwd = SESSION.lock().cwd;
    match with_vfs(|v| v.mkdir(cwd, name, FileMode::default_dir())) {
        Ok(_) => {}
        Err(e) => print_error!("mkdir: {:?}", e),
    }
}

pub fn cmd_touch(name: &str) {
    let cwd = SESSION.lock().cwd;

    with_vfs(|v| match v.resolve_path(cwd, name) {
        Ok(_) => {}
        Err(VfsError::NotFound) => {
            match v.open(
                cwd,
                name,
                OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE),
                FileMode::default_file(),
            ) {
                Ok(fd) => {
                    let _ = v.close(fd);
                }
                Err(e) => print_error!("touch: {:?}", e),
            }
        }
        Err(e) => print_error!("touch: {:?}", e),
    });
}

pub fn cmd_cat(name: &str) {
    let cwd = SESSION.lock().cwd;
    let is_dev = name.starts_with("/dev/") || name.starts_with("dev/");
    let ext2_ino: Option<u32> = with_vfs(|v| {
        match v.resolve_path(cwd, name) {
            Ok(id) if v.nodes[id].is_ext2_backed() => Some(v.nodes[id].ext2_ino),
            _ => None,
        }
    });
    let ext2_ino = if ext2_ino.is_none() && crate::commands::ext2_cmds::is_ext2_ready() {
        let abs = make_abs_path(name);
        let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
        crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            fs.resolve_path(abs_str).ok()
        }).and_then(|x| x)
    } else {
        ext2_ino
    };

    if let Some(ino) = ext2_ino {
        use crate::commands::ext2_cmds::with_ext2_pub;
        let result = with_ext2_pub(|fs| -> Result<(), crate::miku_extfs::FsError> {
            let inode = fs.read_inode(ino)?;
            let size = inode.size() as usize;
            if size == 0 { println!(""); return Ok(()); }
            let read_size = size.min(4096);
            let mut buf = [0u8; 4096];
            let n = fs.read_file(&inode, 0, &mut buf[..read_size])?;
            console::set_color(230, 240, 240);
            let s = unsafe { core::str::from_utf8_unchecked(&buf[..n]) };
            println!("{}", s);
            console::reset_color();
            Ok(())
        });
        match result {
            Some(Ok(())) => {}
            Some(Err(e)) => print_error!("cat: {:?}", e),
            None => print_error!("cat: ext not mounted"),
        }
        return;
    }

    with_vfs(|v| {
        match v.open(cwd, name, OpenFlags(OpenFlags::READ), FileMode::default_file()) {
            Ok(fd) => {
                let mut buf = [0u8; 64];
                let mut total = 0usize;
                let max_read: usize = if is_dev { 256 } else { 4096 };
                console::set_color(230, 240, 240);
                loop {
                    match v.read(fd, &mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if is_dev {
                                for i in 0..n {
                                    let b = buf[i];
                                    let hi = b >> 4;
                                    let lo = b & 0x0F;
                                    let hex_hi = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
                                    let hex_lo = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
                                    print!("{}{} ", hex_hi as char, hex_lo as char);
                                    if (total + i + 1) % 16 == 0 { println!(""); }
                                }
                            } else {
                                let s = unsafe { core::str::from_utf8_unchecked(&buf[..n]) };
                                print!("{}", s);
                            }
                            total += n;
                            if total >= max_read { break; }
                        }
                        Err(e) => { print_error!("read: {:?}", e); break; }
                    }
                }
                console::reset_color();
                if is_dev && total > 0 {
                    println!("");
                    cprintln!(120, 140, 140, "  ({} bytes)", total);
                } else {
                    println!("");
                }
                let _ = v.close(fd);
            }
            Err(e) => print_error!("cat: {:?}", e),
        }
    });
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

pub fn cmd_stat(path: &str) {
    let cwd = SESSION.lock().cwd;

    let vfs_info = with_vfs(|v| {
        let id = v.resolve_path(cwd, path)?;
        Ok((v.nodes[id].fs_type, v.nodes[id].ext2_ino, v.nodes[id].stat()))
    });

    let (fs_type, ext2_ino, st) = match vfs_info {
        Ok(t) => t,
        Err(VfsError::NotFound) if crate::commands::ext2_cmds::is_ext2_ready() => {
            let abs = make_abs_path(path);
            let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
            let ino = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.resolve_path(abs_str).ok()
            }).flatten();
            match ino {
                Some(ino) => (
                    crate::commands::ext2_cmds::active_fs_type(),
                    ino,
                    unsafe { core::mem::zeroed() },
                ),
                None => { print_error!("stat: not found"); return; }
            }
        }
        Err(e) => { print_error!("stat: {:?}", e); return; }
    };

    let fs_name: &str = if fs_type.is_ext_family() && crate::commands::ext2_cmds::is_ext2_ready() {
        ext_version()
    } else {
        match fs_type {
            FsType::TmpFS  => "tmpfs",
            FsType::DevFS  => "devfs",
            FsType::ProcFS => "procfs",
            _              => "ext",
        }
    };

    if fs_type.is_ext_family() && ext2_ino != 0 && crate::commands::ext2_cmds::is_ext2_ready() {
        let disk = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            fs.read_inode(ext2_ino).ok()
        }).flatten();

        if let Some(inode) = disk {
            let kind_str = if inode.is_directory() { "Directory" }
                else if inode.is_symlink() { "Symlink" }
                else { "Regular" };
            cprintln!(128, 222, 217, "  File: {}", path);
            cprintln!(230, 240, 240, "  Type: {}", kind_str);
            cprintln!(230, 240, 240, "  Size: {} bytes", inode.size());
            cprintln!(230, 240, 240, "  Mode: 0o{:o}", inode.mode() & 0o7777);
            cprintln!(230, 240, 240, "  Owner: {}:{}", inode.uid_full(), inode.gid_full());
            cprintln!(230, 240, 240, "  Links: {}", inode.links_count());
            cprintln!(230, 240, 240, "  Inode: {}", ext2_ino);
            cprintln!(120, 140, 140, "  atime: {}", inode.atime());
            cprintln!(120, 140, 140, "  mtime: {}", inode.mtime());
            cprintln!(120, 140, 140, "  ctime: {}", inode.ctime());
            cprintln!(120, 140, 140, "  FS:    {}", fs_name);
            return;
        }
    }

    cprintln!(128, 222, 217, "  File: {}", path);
    cprintln!(230, 240, 240, "  Type: {:?}", st.kind);
    cprintln!(230, 240, 240, "  Size: {} bytes", st.size);
    cprintln!(230, 240, 240, "  Mode: 0o{:o}", st.mode.0);
    cprintln!(230, 240, 240, "  Owner: {}:{}", st.uid, st.gid);
    cprintln!(230, 240, 240, "  Links: {}", st.nlinks);
    cprintln!(120, 140, 140, "  FS:    {}", fs_name);
}

pub fn cmd_rm(path: &str) {
    let cwd = SESSION.lock().cwd;
    let node_info = with_vfs(|v| match v.resolve_path(cwd, path) {
        Ok(id) => {
            let ft = v.nodes[id].fs_type;
            let ext_ino = v.nodes[id].ext2_ino;
            Ok((id, ft, ext_ino))
        }
        Err(e) => Err(e),
    });

    match node_info {
        Ok((_, FsType::ProcFS, _)) | Ok((_, FsType::DevFS, _)) => {
            print_error!("rm: permission denied (read-only fs)");
            return;
        }
        Ok((_, ft, ext_ino)) if ft.is_ext_family() => {
            if ext_ino != 0 {
                ext_rm(path);
            }
            let _ = with_vfs(|v| v.unlink(cwd, path));
        }
        Ok(_) => {

            match with_vfs(|v| v.unlink(cwd, path)) {
                Ok(_) => {}
                Err(e) => print_error!("rm: {:?}", e),
            }
        }
        Err(VfsError::NotFound) => {

            if crate::commands::ext2_cmds::is_ext2_ready() {
                ext_rm(path);
            } else {
                print_error!("rm: not found");
            }
        }
        Err(e) => print_error!("rm: {:?}", e),
    }
}

fn ext_rm(path: &str) {
    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    match ext_version() {
        "ext4" => crate::commands::ext4_cmds::cmd_ext4_rm(abs_str),
        "ext3" => crate::commands::ext3_cmds::cmd_ext3_rm(abs_str),
        _      => crate::commands::ext2_cmds::cmd_ext2_rm(abs_str),
    }
}

pub fn cmd_rm_rf(path: &str) {
    let cwd = SESSION.lock().cwd;

    let info = with_vfs(|v| match v.resolve_path(cwd, path) {
        Ok(id) => Ok((id, v.nodes[id].kind, v.nodes[id].fs_type)),
        Err(e) => Err(e),
    });

    match info {
        Ok((_, _, FsType::ProcFS)) | Ok((_, _, FsType::DevFS)) => {
            print_error!("rm -rf: permission denied (read-only fs)");
        }
        Ok((_, VNodeKind::Directory, ft)) if ft.is_ext_family() => {
            crate::commands::ext2_cmds::cmd_ext2_rm_rf(path);
            let _ = with_vfs(|v| v.rmdir(cwd, path));
        }
        Ok((_, VNodeKind::Directory, _)) => {
            recursive_rm(cwd, path);
        }
        Ok((_, _, ft)) if ft.is_ext_family() => {
            ext_rm(path);
            let _ = with_vfs(|v| v.unlink(cwd, path));
        }
        Ok(_) => {
            match with_vfs(|v| v.unlink(cwd, path)) {
                Ok(_) => {}
                Err(e) => print_error!("rm -rf: {:?}", e),
            }
        }
        Err(VfsError::NotFound) => {
            if crate::commands::ext2_cmds::is_ext2_ready() {
                crate::commands::ext2_cmds::cmd_ext2_rm_rf(path);
            }
        }
        Err(e) => print_error!("rm -rf: {:?}", e),
    }
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
        for (_, child_id) in vfs.nodes[eff].children.iter() {
            if child_count >= 16 {
                break;
            }
            let cid = child_id as usize;
            if cid >= MAX_VNODES || !vfs.nodes[cid].active {
                continue;
            }
            let nm = vfs.nodes[cid].get_name().as_bytes();
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

pub fn cmd_rmdir(path: &str) {
    if path.is_empty() { println!("Usage: rmdir <path>"); return; }

    let cwd = SESSION.lock().cwd;

    let ext_info = with_vfs(|v| {
        let id = v.resolve_path(cwd, path).ok()?;
        if v.nodes[id].fs_type.is_ext_family() && v.nodes[id].ext2_ino != 0 {
            Some(v.nodes[id].ext2_ino)
        } else {
            None
        }
    });

    if let Some(_ext2_ino) = ext_info {
        let abs = make_abs_path(path);
        let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
        use crate::commands::ext_cmds_common::impl_rmdir;
        let tag = crate::commands::ext2_cmds::ext_fs_version_tag();
        impl_rmdir(abs_str, tag);
    }

    match with_vfs(|v| v.rmdir(cwd, path)) {
        Ok(_) => { if ext_info.is_none() { print_success!("  removed"); } }
        Err(VfsError::NotFound) => { if ext_info.is_some() { } else { print_error!("rmdir: not found"); } }
        Err(e) => print_error!("rmdir: {:?}", e),
    }
}

pub fn cmd_mv(old: &str, new: &str) {
    let cwd = SESSION.lock().cwd;

    let node_info = with_vfs(|v| match v.resolve_path(cwd, old) {
        Ok(id) => Ok((v.nodes[id].fs_type, v.nodes[id].ext2_ino)),
        Err(e) => Err(e),
    });

    match node_info {
        Ok((ft, _)) if ft.is_ext_family() => {
            let abs_old = make_abs_path(old);
            let abs_old_str = unsafe { core::str::from_utf8_unchecked(&abs_old.0[..abs_old.1]) };
            let new_name = new.rsplit('/').next().unwrap_or(new);
            crate::commands::ext2_cmds::cmd_ext2_rename(abs_old_str, new_name);
            let _ = with_vfs(|v| v.rename(cwd, old, new_name));
        }
        Ok(_) => {
            match with_vfs(|v| v.rename(cwd, old, new)) {
                Ok(_) => {}
                Err(e) => print_error!("mv: {:?}", e),
            }
        }
        Err(VfsError::NotFound) => {
            if crate::commands::ext2_cmds::is_ext2_ready() {
                let abs_old = make_abs_path(old);
                let abs_old_str = unsafe { core::str::from_utf8_unchecked(&abs_old.0[..abs_old.1]) };
                let new_name = new.rsplit('/').next().unwrap_or(new);
                crate::commands::ext2_cmds::cmd_ext2_rename(abs_old_str, new_name);
            } else {
                print_error!("mv: not found");
            }
        }
        Err(e) => print_error!("mv: {:?}", e),
    }
}

pub fn cmd_symlink(target: &str, linkname: &str) {
    serial_println!("[symlink] target='{}' linkname='{}'", target, linkname);
    let cwd = SESSION.lock().cwd;
    match with_vfs(|v| v.symlink(cwd, linkname, target)) {
        Ok(_) => print_success!("  {} -> {}", linkname, target),
        Err(e) => print_error!("ln -s: {:?}", e),
    }
}

pub fn cmd_link(existing: &str, new_name: &str) {
    serial_println!("[link] existing='{}' new_name='{}'", existing, new_name);
    let cwd = SESSION.lock().cwd;
    match with_vfs(|v| v.link(cwd, existing, cwd, new_name)) {
        Ok(_) => print_success!("  {} => {}", new_name, existing),
        Err(e) => print_error!("ln: {:?}", e),
    }
}

pub fn cmd_readlink(path: &str) {
    let cwd = SESSION.lock().cwd;
    match with_vfs(|v| v.readlink(cwd, path)) {
        Ok(target) => cprintln!(230, 240, 240, "  {}", target.as_str()),
        Err(e) => print_error!("readlink: {:?}", e),
    }
}

pub fn cmd_chmod(mode_str: &str, path: &str) {
    let mode = parse_octal(mode_str);
    if mode.is_none() {
        print_error!("chmod: invalid mode '{}'", mode_str);
        return;
    }
    let cwd = SESSION.lock().cwd;
    match with_vfs(|v| v.chmod(cwd, path, FileMode::new(mode.unwrap()))) {
        Ok(_) => {}
        Err(e) => print_error!("chmod: {:?}", e),
    }
}

pub fn cmd_df() {
    let cwd = SESSION.lock().cwd;
    let result = with_vfs_ro(|v| v.statfs(cwd, "/"));
    match result {
        Ok(st) => {
            cprintln!(
                128,
                222,
                217,
                "  Filesystem  Type    Blocks  Free  Inodes  IFree"
            );
            cprintln!(
                230,
                240,
                240,
                "  /            {:?}    {:>6}  {:>4}  {:>6}  {:>5}",
                st.fs_type,
                st.total_blocks,
                st.free_blocks,
                st.total_inodes,
                st.free_inodes
            );
        }
        Err(e) => print_error!("df: {:?}", e),
    }
}

fn parse_octal(s: &str) -> Option<u16> {
    let mut result: u16 = 0;
    for &b in s.as_bytes() {
        if b < b'0' || b > b'7' {
            return None;
        }
        result = result.checked_mul(8)?.checked_add((b - b'0') as u16)?;
    }
    if result > 0o7777 {
        return None;
    }
    Some(result)
}

pub fn cmd_mount_list() {
    cprintln!(128, 222, 217, "  Filesystem    Mountpoint    Type");
    cprintln!(230, 240, 240, "  tmpfs         /             tmpfs");
    cprintln!(230, 240, 240, "  devfs         /dev          devfs");
    cprintln!(230, 240, 240, "  procfs        /proc         procfs");

    let mounts = crate::commands::ext2_cmds::mounted_slots_snapshot();

    with_vfs_ro(|vfs| {
        for entry in mounts.iter().flatten() {
            let (_slot, vnode, fs_name) = *entry;
            let path = vnode_path(vfs, vnode as usize);
            let path_str = path.as_str();
            cprintln!(230, 240, 240, "  {:<14}{:<14}{}", fs_name, path_str, fs_name);
        }
    });
}

/// Build the absolute path of a vnode by walking parent links
/// Returns "/" for the root and a [u8; 256]-backed buffer otherwise
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
        _ => {
            print_error!("mount: unknown filesystem '{}'", fstype);
            println!("  Supported: ext2, ext3, ext4");
        }
    }
}

pub fn cmd_umount(path: &str) {
    crate::commands::ext2_cmds::with_ext2_pub(|fs| {
        let _ = fs.flush_all_dirty_metadata();
    });

    let cwd = SESSION.lock().cwd;
    // Resolve path to a vnode and verify it's actually a registered ext
    // mount root. Returns the slot index alongside the vnode id so the
    // caller can tear down the matching slot  not just whichever slot
    // happens to be active
    let result = with_vfs(|vfs| -> VfsResult<(usize, usize)> {
        let id = vfs.resolve_path(cwd, path)?;

        if !vfs.nodes[id].fs_type.is_ext_family() {
            return Err(VfsError::InvalidArgument);
        }

        let slot = crate::commands::ext2_cmds::slot_for_vnode(id as u16)
            .ok_or(VfsError::InvalidArgument)?;

        vfs.evict_children_recursive(id);
        vfs.nodes[id].fs_type = FsType::TmpFS;
        vfs.nodes[id].ext2_ino = 0;
        vfs.nodes[id].children_loaded = false;
        Ok((id, slot))
    });

    match result {
        Ok((_id, slot)) => {
            crate::commands::ext2_cmds::unmount_slot(slot);
            // Keep the legacy "any ext mounted?" flag in sync. Set it from
            // a fresh snapshot rather than blanket-clearing - other slots
            // may still be mounted on different paths
            let any_left = crate::commands::ext2_cmds::mounted_slots_snapshot()
                .iter()
                .any(|e| e.is_some());
            with_vfs(|vfs| { vfs.ext2_mount_active = any_left; });
            print_success!("  unmounted {}", path);
        }
        Err(e) => print_error!("umount: {:?}", e),
    }
}

fn mount_ext2_to_vfs(mountpoint: &str) {
    use crate::commands::ext2_cmds;
    use crate::miku_extfs::structs::EXT2_ROOT_INO;

    if !ext2_cmds::is_ext2_ready() {
        print_error!("  ext2 not mounted. Run ext2mount first");
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
                if !vfs.nodes[id].children.is_empty() || vfs.nodes[id].fs_type.is_ext_family() {
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

        vfs.nodes[mount_id].fs_type = crate::commands::ext2_cmds::active_fs_type();
        vfs.nodes[mount_id].ext2_ino = EXT2_ROOT_INO;
        vfs.nodes[mount_id].children_loaded = false;
        vfs.ext2_mount_active = true;

        Ok(mount_id)
    });

    match result {
        Ok(id) => {
            // Track which VFS vnode now belongs to the active ext slot so
            // a later umount <path> can find the right slot to drop
            let active = crate::commands::ext2_cmds::active_slot_index();
            crate::commands::ext2_cmds::register_mount_vnode(active, id as u16);
            print_success!("  ext2 mounted at {} (on-demand)", mountpoint);
        }
        Err(e) => print_error!("mount: {:?}", e),
    }
}
pub fn ext_version() -> &'static str {
    crate::commands::ext2_cmds::ext_fs_version_tag()
}

pub fn cmd_extwrite(path: &str, text: &str) {
    if path.is_empty() || text.is_empty() {
        println!("Usage: extwrite <path> <text>");
        return;
    }
    if !crate::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }

    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    match ext_version() {
        "ext4" => crate::commands::ext4_cmds::cmd_ext4_write(abs_str, text),
        "ext3" => crate::commands::ext3_cmds::cmd_ext3_write(abs_str, text),
        _      => crate::commands::ext2_cmds::cmd_ext2_write(abs_str, text),
    }
}

pub fn cmd_extcp(src: &str, dst: &str) {
    if src.is_empty() || dst.is_empty() { println!("Usage: extcp <src> <dst>"); return; }
    if !crate::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }

    let abs_src = make_abs_path(src);
    let abs_dst = make_abs_path(dst);
    let s = unsafe { core::str::from_utf8_unchecked(&abs_src.0[..abs_src.1]) };
    let d = unsafe { core::str::from_utf8_unchecked(&abs_dst.0[..abs_dst.1]) };

    use crate::commands::ext2_cmds::with_ext2_pub;
    use crate::commands::ext_cmds_common::resolve_parent_and_name;
    let result = with_ext2_pub(|fs| -> Result<u32, crate::miku_extfs::FsError> {
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
    if !crate::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }

    let abs_old = make_abs_path(old);
    let s = unsafe { core::str::from_utf8_unchecked(&abs_old.0[..abs_old.1]) };

    let actual_new = match new_name.rfind('/') {
        Some(p) => &new_name[p+1..],
        None    => new_name,
    };
    if actual_new.is_empty() { print_error!("  extmv: invalid new name"); return; }

    use crate::commands::ext2_cmds::with_ext2_pub;
    use crate::commands::ext_cmds_common::resolve_parent_and_name;
    let result = with_ext2_pub(|fs| -> Result<(), crate::miku_extfs::FsError> {
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
    if !crate::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }
    let abs = make_abs_path(path);
    let s   = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    crate::commands::ext2_cmds::cmd_ext2_chmod(mode_str, s);
}

pub fn cmd_extchown(uid_str: &str, gid_str: &str, path: &str) {
    if uid_str.is_empty() || path.is_empty() { println!("Usage: extchown <uid> <gid> <path>"); return; }
    if !crate::commands::ext2_cmds::is_ext2_ready() { print_error!("  ext not mounted"); return; }
    let abs = make_abs_path(path);
    let s   = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    crate::commands::ext2_cmds::cmd_ext2_chown(uid_str, gid_str, s);
}


pub fn cmd_extls(path: &str) {
    if !crate::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }
    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    match ext_version() {
        "ext4" => crate::commands::ext4_cmds::cmd_ext4_ls(abs_str),
        "ext3" => crate::commands::ext3_cmds::cmd_ext3_ls(abs_str),
        _      => crate::commands::ext2_cmds::cmd_ext2_ls(abs_str),
    }
}

pub fn cmd_extmkdir(path: &str) {
    if path.is_empty() { println!("Usage: extmkdir <path>"); return; }
    if !crate::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }
    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    match ext_version() {
        "ext4" => crate::commands::ext4_cmds::cmd_ext4_mkdir(abs_str),
        "ext3" => crate::commands::ext3_cmds::cmd_ext3_mkdir(abs_str),
        _      => crate::commands::ext2_cmds::cmd_ext2_mkdir(abs_str),
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
    if !crate::commands::ext2_cmds::is_ext2_ready() {
        print_error!("  ext not mounted");
        return;
    }

    let abs = make_abs_path(path);
    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
    use crate::commands::ext2_cmds::with_ext2_pub;
    use crate::commands::ext_cmds_common::resolve_parent_and_name;
    let result = with_ext2_pub(|fs| -> Result<u32, crate::miku_extfs::FsError> {
        let (parent_ino, filename) = resolve_parent_and_name(fs, abs_str)?;
 
        if let Some(ino) = fs.ext2_lookup_in_dir(parent_ino, filename)? {
            return Ok(ino);
        }
        fs.ext3_create_file(parent_ino, filename, 0o644)
    });
    match result {
        Some(Ok(ino)) => print_success!("  created inode {}", ino),
        Some(Err(e))  => print_error!("  exttouch: {:?}", e),
        None          => print_error!("  ext not mounted"),
    }
}
