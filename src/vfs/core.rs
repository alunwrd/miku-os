use crate::vfs::address_space::AddressSpace;
use crate::vfs::devfs;
use crate::vfs::fd::FdTable;
use crate::vfs::hash::name_hash;
use crate::vfs::mount::MountTable;
use crate::vfs::pages::PageCache;
use crate::vfs::path::PathWalker;
use crate::vfs::procfs;
use crate::vfs::types::*;
use crate::vfs::vnode::VNode;
use spin::Mutex;

mod syslibs {
    pub struct SysLib {
        pub dir: &'static str,
        pub name: &'static str,
        pub data: &'static [u8],
    }

    pub static LIBS: &[SysLib] = &[
        SysLib {
            dir: "lib",
            name: "libmiku.so",
            data: include_bytes!("../lib/libmiku/libmiku.so"),
        },
    ];
}

static VFS_LOCK: Mutex<()> = Mutex::new(());

#[repr(C, align(4096))]
struct VfsStorage {
    data: core::mem::MaybeUninit<MikuVFS>,
}

static mut VFS_STORAGE: VfsStorage = VfsStorage {
    data: core::mem::MaybeUninit::uninit(),
};

static VFS_INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn init_vfs() -> Result<(), &'static str> {
    unsafe {
        let ptr = VFS_STORAGE.data.as_mut_ptr();

        let nodes_ptr = core::ptr::addr_of_mut!((*ptr).nodes);
        for i in 0..MAX_VNODES {
            core::ptr::write(core::ptr::addr_of_mut!((*nodes_ptr)[i]), VNode::empty());
        }

        core::ptr::write(core::ptr::addr_of_mut!((*ptr).page_cache), PageCache::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).mounts), MountTable::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).fd_table), FdTable::new());
        core::ptr::write(
            core::ptr::addr_of_mut!((*ptr).ctx),
            ProcessContext::root_context(),
        );
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).ext2_mount_active), false);
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).vnode_free_hint), 1);

        let vfs = &mut *ptr;
        vfs.bootstrap();

        VFS_INITIALIZED.store(true, core::sync::atomic::Ordering::Release);
    }

    crate::serial_println!("[vfs] init done");
    Ok(())
}

fn get_vfs() -> &'static mut MikuVFS {
    if !VFS_INITIALIZED.load(core::sync::atomic::Ordering::Acquire) {
        panic!("[vfs] accessed before init");
    }
    unsafe { &mut *VFS_STORAGE.data.as_mut_ptr() }
}

pub fn with_vfs<F, R>(f: F) -> R
where
    F: FnOnce(&mut MikuVFS) -> R,
{
    let _guard = VFS_LOCK.lock();
    f(get_vfs())
}

pub fn with_vfs_ro<F, R>(f: F) -> R
where
    F: FnOnce(&MikuVFS) -> R,
{
    let _guard = VFS_LOCK.lock();
    f(get_vfs())
}

pub struct MikuVFS {
    pub nodes: [VNode; MAX_VNODES],
    pub page_cache: PageCache,
    pub mounts: MountTable,
    pub fd_table: FdTable,
    pub ctx: ProcessContext,
    pub ext2_mount_active: bool,
    pub(crate) vnode_free_hint: usize,
}

impl MikuVFS {
    fn bootstrap(&mut self) {
        self.nodes[0].init(
            0,
            INVALID_ID,
            "/",
            VNodeKind::Directory,
            FsType::TmpFS,
            FileMode::default_dir(),
            0,
            0,
            self.now(),
        );
        self.nodes[0].parent = 0;
        let _ = self.mounts.add(FsType::TmpFS, 0, INVALID_ID);

        self.mount_devfs();
        self.mount_procfs();
        self.mount_syslibs();
        self.create_mnt();
    }

    fn mount_syslibs(&mut self) {
        crate::serial_println!("[vfs] mounting syslibs");

        let mut files_created = 0u8;

        for lib in syslibs::LIBS {
            let dir_id = {
                let found = self.nodes[0].children.find_by_name(lib.dir)
                    .and_then(|id| {
                        let c = id as usize;
                        if c < MAX_VNODES && self.nodes[c].active { Some(c) } else { None }
                    });
                match found {
                    Some(id) => id,
                    None => {
                        let id = match self.alloc_vnode() {
                            Ok(id) => id,
                            Err(_) => continue,
                        };
                        let ts = self.now();
                        self.nodes[id].init(
                            id as InodeId, 0, lib.dir,
                            VNodeKind::Directory, FsType::TmpFS,
                            FileMode::new(0o755), 0, 0, ts,
                        );
                        self.nodes[id].flags.immutable = true;
                        if !self.nodes[0].children.insert(lib.dir, id as InodeId) {
                            self.nodes[id].active = false;
                            continue;
                        }
                        id
                    }
                }
            };

            let file_id = match self.alloc_vnode() {
                Ok(id) => id,
                Err(_) => continue,
            };

            let ts = self.now();
            self.nodes[file_id].init(
                file_id as InodeId, dir_id as InodeId, lib.name,
                VNodeKind::Regular, FsType::TmpFS,
                FileMode::new(0o555), 0, 0, ts,
            );

            let mut offset = 0usize;
            let mut ok = true;
            while offset < lib.data.len() {
                let page_num = offset / PAGE_SIZE;
                let page_off = offset % PAGE_SIZE;
                let chunk = (PAGE_SIZE - page_off).min(lib.data.len() - offset);

                let pid = match self.nodes[file_id].addr_space.get_page(page_num) {
                    Some(pid) => pid,
                    None => match self.page_cache.alloc_page() {
                        Ok(pid) => {
                            if self.nodes[file_id].addr_space.set_page(page_num, pid).is_err() {
                                ok = false; break;
                            }
                            pid
                        }
                        Err(_) => { ok = false; break; }
                    }
                };

                if let Some(page) = self.page_cache.get_page_data_mut(pid) {
                    page[page_off..page_off + chunk]
                        .copy_from_slice(&lib.data[offset..offset + chunk]);
                } else {
                    ok = false; break;
                }
                offset += chunk;
            }

            if !ok {
                crate::serial_println!("[vfs] syslib write failed: {}", lib.name);
                self.nodes[file_id].active = false;
                continue;
            }

            self.nodes[file_id].size = lib.data.len() as u64;
            self.nodes[file_id].flags.immutable = true;

            if !self.nodes[dir_id].children.insert(lib.name, file_id as InodeId) {
                self.nodes[file_id].active = false;
                continue;
            }

            files_created += 1;
            crate::serial_println!(
                "[vfs] syslib: /{}/{} vnode={} {} bytes (immutable)",
                lib.dir, lib.name, file_id, lib.data.len()
            );
        }

        crate::serial_println!("[vfs] syslibs: {} files", files_created);
    }

    fn create_mnt(&mut self) {
        if let Ok(mnt_id) = self.alloc_vnode() {
            self.nodes[mnt_id].init(
                mnt_id as InodeId,
                0,
                "mnt",
                VNodeKind::Directory,
                FsType::TmpFS,
                FileMode::default_dir(),
                0,
                0,
                self.now(),
            );
            if self.nodes[0].children.insert("mnt", mnt_id as InodeId) {
                crate::serial_println!("[vfs] /mnt created");
            } else {
                self.nodes[mnt_id].active = false;
            }
        }
    }

    fn mount_devfs(&mut self) {
        crate::serial_println!("[vfs] mounting devfs");

        let dev_id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(e) => {
                crate::serial_println!("[vfs] devfs alloc failed: {:?}", e);
                return;
            }
        };

        self.nodes[dev_id].init(
            dev_id as InodeId,
            0,
            "dev",
            VNodeKind::Directory,
            FsType::DevFS,
            FileMode::default_dir(),
            0,
            0,
            self.now(),
        );

        if !self.nodes[0].children.insert("dev", dev_id as InodeId) {
            self.nodes[dev_id].active = false;
            return;
        }

        let _ = self.mounts.add(FsType::DevFS, dev_id as InodeId, 0);

        let mut count = 0u8;
        for &(name, dev_type) in devfs::DEV_ENTRIES {
            if let Ok(id) = self.alloc_vnode() {
                self.nodes[id].init(
                    id as InodeId,
                    dev_id as InodeId,
                    name,
                    VNodeKind::CharDevice,
                    FsType::DevFS,
                    FileMode::default_dev(),
                    0,
                    0,
                    self.now(),
                );
                self.nodes[id].dev_major = dev_type.major();
                self.nodes[id].dev_minor = dev_type.minor();

                if self.nodes[dev_id].children.insert(name, id as InodeId) {
                    count += 1;
                } else {
                    self.nodes[id].active = false;
                }
            }
        }

        crate::serial_println!("[vfs] devfs: {} devices mounted at /dev", count);
    }

    fn mount_procfs(&mut self) {
        crate::serial_println!("[vfs] mounting procfs");

        let proc_id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(e) => {
                crate::serial_println!("[vfs] procfs alloc failed: {:?}", e);
                return;
            }
        };

        self.nodes[proc_id].init(
            proc_id as InodeId,
            0,
            "proc",
            VNodeKind::Directory,
            FsType::ProcFS,
            FileMode::default_dir(),
            0,
            0,
            self.now(),
        );

        if !self.nodes[0].children.insert("proc", proc_id as InodeId) {
            self.nodes[proc_id].active = false;
            return;
        }

        let _ = self.mounts.add(FsType::ProcFS, proc_id as InodeId, 0);

        let mut count = 0u8;
        for &name in procfs::PROC_ENTRIES {
            if let Ok(id) = self.alloc_vnode() {
                self.nodes[id].init(
                    id as InodeId,
                    proc_id as InodeId,
                    name,
                    VNodeKind::Regular,
                    FsType::ProcFS,
                    FileMode(0o444),
                    0,
                    0,
                    self.now(),
                );
                if self.nodes[proc_id].children.insert(name, id as InodeId) {
                    count += 1;
                } else {
                    self.nodes[id].active = false;
                }
            }
        }

        crate::serial_println!("[vfs] procfs: {} entries mounted at /proc", count);
    }

    #[inline]
    fn now(&self) -> Timestamp {
        procfs::uptime_ticks()
    }

    pub fn alloc_vnode(&mut self) -> VfsResult<usize> {
        let hint = self.vnode_free_hint;
        for i in hint..MAX_VNODES {
            if !self.nodes[i].active {
                self.vnode_free_hint = i + 1;
                return Ok(i);
            }
        }
        for i in 1..hint {
            if !self.nodes[i].active {
                self.vnode_free_hint = i + 1;
                return Ok(i);
            }
        }
        Err(VfsError::NoSpace)
    }

    #[inline]
    pub fn valid_vnode(&self, id: usize) -> bool {
        id < MAX_VNODES && self.nodes[id].active
    }

    pub fn resolve_path(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        let path = path.trim();
        if path.is_empty() {
            return Ok(cwd);
        }

        let mut current = if path.starts_with('/') { 0 } else { cwd };
        let mut depth = 0u8;

        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = self.nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }

            depth += 1;
            if depth as usize > MAX_PATH_DEPTH {
                return Err(VfsError::InvalidPath);
            }

            if !self.nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }

            let eff = self.xm(current);
            current = self.lookup_child_or_load(eff, component)?;

            if self.nodes[current].is_symlink() {
                current = self.follow_symlink(current, 0)?;
            }
        }
        Ok(current)
    }

    pub fn resolve_path_lstat(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        let path = path.trim();
        if path.is_empty() {
            return Ok(cwd);
        }

        let total = path.split('/').filter(|c| !c.is_empty() && *c != "." && *c != "..").count();
        let mut idx = 0usize;
        let mut current = if path.starts_with('/') { 0 } else { cwd };
        let mut depth = 0u8;

        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = self.nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }

            idx += 1;
            depth += 1;
            if depth as usize > MAX_PATH_DEPTH {
                return Err(VfsError::InvalidPath);
            }

            if !self.nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }

            let eff = self.xm(current);
            current = self.lookup_child_or_load(eff, component)?;

            // follow symlinks for intermediate components, not the last one
            if idx < total && self.nodes[current].is_symlink() {
                current = self.follow_symlink(current, 0)?;
            }
        }
        Ok(current)
    }

    fn follow_symlink(&mut self, link_id: usize, depth: usize) -> VfsResult<usize> {
        if depth >= MAX_SYMLINK_DEPTH {
            return Err(VfsError::TooManySymlinks);
        }
        if !self.nodes[link_id].is_symlink() {
            return Ok(link_id);
        }

        let mut target_buf = [0u8; NAME_LEN];
        let target_len = self.nodes[link_id].symlink_target.len as usize;
        target_buf[..target_len]
            .copy_from_slice(&self.nodes[link_id].symlink_target.data[..target_len]);

        let target_str = match core::str::from_utf8(&target_buf[..target_len]) {
            Ok(s) => s,
            Err(_) => return Err(VfsError::InvalidPath),
        };

        if target_str.is_empty() {
            return Err(VfsError::InvalidPath);
        }

        let parent = self.nodes[link_id].parent as usize;
        let start = if target_str.starts_with('/') { 0 } else { parent };

        let mut current = start;
        for component in target_str.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = self.nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }
            if !self.nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }
            let eff = self.xm(current);
            current = self.lookup_child_or_load(eff, component)?;
            if self.nodes[current].is_symlink() {
                current = self.follow_symlink(current, depth + 1)?;
            }
        }
        Ok(current)
    }

    fn lookup_child_or_load(&mut self, parent: usize, name: &str) -> VfsResult<usize> {
        if let Some(id) = self.nodes[parent].children.find_by_name(name) {
            let cid = id as usize;
            if cid < MAX_VNODES && self.nodes[cid].active {
                return Ok(cid);
            }
        }

        if self.nodes[parent].fs_type.is_ext_family() && self.nodes[parent].ext2_ino != 0 {
            return self.ext2_lazy_lookup(parent, name);
        }

        Err(VfsError::NotFound)
    }

    fn ext2_lazy_lookup(&mut self, parent_vnode: usize, name: &str) -> VfsResult<usize> {
        if !self.ext2_mount_active || !crate::commands::ext2_cmds::is_ext2_ready() {
            return Err(VfsError::NotFound);
        }

        let parent_ext2_ino = self.nodes[parent_vnode].ext2_ino;
        if parent_ext2_ino == 0 {
            return Err(VfsError::NotFound);
        }

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            let child_ino = match fs.ext2_lookup_in_dir(parent_ext2_ino, name) {
                Ok(Some(ino)) => ino,
                Ok(None) => return Err(VfsError::NotFound),
                Err(_) => return Err(VfsError::IoError),
            };

            let inode = match fs.read_inode(child_ino) {
                Ok(i) => i,
                Err(_) => return Err(VfsError::IoError),
            };

            let kind = if inode.is_directory() {
                VNodeKind::Directory
            } else if inode.is_symlink() {
                VNodeKind::Symlink
            } else {
                VNodeKind::Regular
            };

            let perm = inode.permissions();
            let size = inode.size() as u64;
            let uid = inode.uid();
            let gid = inode.gid();
            let nlinks = inode.links_count();

            let mut symlink_target = [0u8; NAME_LEN];
            let mut symlink_len = 0u8;
            if inode.is_symlink() {
                if inode.is_fast_symlink() {
                    let target = inode.fast_symlink_target();
                    let l = target.len().min(NAME_LEN);
                    symlink_target[..l].copy_from_slice(&target[..l]);
                    symlink_len = l as u8;
                } else {
                    let read_len = (size as usize).min(NAME_LEN);
                    let n = fs
                        .read_file(&inode, 0, &mut symlink_target[..read_len])
                        .unwrap_or(0);
                    symlink_len = n as u8;
                }
            }

            Ok((child_ino, kind, perm, size, uid, gid, nlinks, symlink_target, symlink_len))
        });

        let info = match result {
            Some(Ok(info)) => info,
            Some(Err(e)) => return Err(e),
            None => return Err(VfsError::NotFound),
        };

        let (child_ino, kind, perm, size, uid, gid, nlinks, symlink_target, symlink_len) = info;

        let id = self.alloc_vnode()?;
        let ts = self.now();

        self.nodes[id].init(
            id as InodeId,
            parent_vnode as InodeId,
            name,
            kind,
            crate::commands::ext2_cmds::active_fs_type(),
            FileMode::new(perm),
            uid,
            gid,
            ts,
        );
        self.nodes[id].ext2_ino = child_ino;
        self.nodes[id].size = size;
        self.nodes[id].nlinks = nlinks;
        self.nodes[id].children_loaded = false;

        if kind == VNodeKind::Symlink && symlink_len > 0 {
            self.nodes[id].symlink_target.data[..symlink_len as usize]
                .copy_from_slice(&symlink_target[..symlink_len as usize]);
            self.nodes[id].symlink_target.len = symlink_len;
        }

        if !self.nodes[parent_vnode].children.insert(name, id as InodeId) {
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        Ok(id)
    }

    pub fn ext2_ensure_children_loaded(&mut self, dir_vnode: usize) -> VfsResult<()> {
        if !self.nodes[dir_vnode].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if !self.nodes[dir_vnode].fs_type.is_ext_family() {
            return Ok(());
        }
        if self.nodes[dir_vnode].children_loaded {
            return Ok(());
        }
        if self.nodes[dir_vnode].ext2_ino == 0 {
            return Ok(());
        }

        let ext2_ino = self.nodes[dir_vnode].ext2_ino;

        const BATCH: usize = 256;
        struct ChildInfo {
            name: [u8; 255],
            name_len: u8,
            ino: u32,
            file_type: u8,
            mode: u16,
            uid: u16,
            gid: u16,
            size: u64,
            nlinks: u16,
            symlink_target: [u8; 64],
            symlink_len: u8,
        }

        let mut child_infos: [ChildInfo; BATCH] = unsafe { core::mem::zeroed() };
        let mut child_count = 0usize;

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            use crate::miku_extfs::structs::{FT_DIR, FT_SYMLINK};

            let inode = fs.read_inode(ext2_ino).map_err(|_| VfsError::IoError)?;
            let mut entries = [const { crate::miku_extfs::structs::DirEntry::empty() }; 256];
            let count = fs
                .read_dir(&inode, &mut entries)
                .map_err(|_| VfsError::IoError)?;

            for i in 0..count {
                if child_count >= BATCH {
                    break;
                }
                let e = &entries[i];
                let n = e.name_str();
                if n == "." || n == ".." || n == "lost+found" {
                    continue;
                }
                let child_inode = match fs.read_inode(e.inode) {
                    Ok(ino) => ino,
                    Err(_) => continue,
                };
                let nb = n.as_bytes();
                let l = nb.len().min(255);
                child_infos[child_count].name[..l].copy_from_slice(&nb[..l]);
                child_infos[child_count].name_len = l as u8;
                child_infos[child_count].ino = e.inode;
                child_infos[child_count].file_type = e.file_type;
                child_infos[child_count].mode = child_inode.permissions();
                child_infos[child_count].uid = child_inode.uid();
                child_infos[child_count].gid = child_inode.gid();
                child_infos[child_count].size = child_inode.size();
                child_infos[child_count].nlinks = child_inode.links_count();

                if e.file_type == FT_SYMLINK {
                    if child_inode.is_fast_symlink() {
                        let target = child_inode.fast_symlink_target();
                        let tl = target.len().min(64);
                        child_infos[child_count].symlink_target[..tl]
                            .copy_from_slice(&target[..tl]);
                        child_infos[child_count].symlink_len = tl as u8;
                    } else {
                        let sz = child_inode.size() as usize;
                        let read_len = sz.min(64);
                        let n = fs
                            .read_file(&child_inode, 0, &mut child_infos[child_count].symlink_target[..read_len])
                            .unwrap_or(0);
                        child_infos[child_count].symlink_len = n as u8;
                    }
                }

                child_count += 1;
            }
            Ok::<(), VfsError>(())
        });

        match result {
            Some(Ok(())) => {}
            Some(Err(_)) => return Err(VfsError::IoError),
            None => return Err(VfsError::NotFound),
        }

        for i in 0..child_count {
            let ci = &child_infos[i];
            let name_str = match core::str::from_utf8(&ci.name[..ci.name_len as usize]) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let already = self.nodes[dir_vnode].children.find_by_name(name_str)
                .map(|id| {
                    let c = id as usize;
                    c < MAX_VNODES && self.nodes[c].active
                })
                .unwrap_or(false);
            if already {
                continue;
            }

            use crate::miku_extfs::structs::{FT_DIR, FT_SYMLINK};
            let kind = match ci.file_type {
                FT_DIR => VNodeKind::Directory,
                FT_SYMLINK => VNodeKind::Symlink,
                _ => VNodeKind::Regular,
            };

            if let Ok(id) = self.alloc_vnode() {
                let ts = self.now();
                self.nodes[id].init(
                    id as InodeId,
                    dir_vnode as InodeId,
                    name_str,
                    kind,
                    crate::commands::ext2_cmds::active_fs_type(),
                    FileMode::new(ci.mode),
                    ci.uid,
                    ci.gid,
                    ts,
                );
                self.nodes[id].ext2_ino = ci.ino;
                self.nodes[id].size = ci.size;
                self.nodes[id].nlinks = ci.nlinks;
                self.nodes[id].children_loaded = false;

                if kind == VNodeKind::Symlink && ci.symlink_len > 0 {
                    let sl = ci.symlink_len as usize;
                    self.nodes[id].symlink_target.data[..sl]
                        .copy_from_slice(&ci.symlink_target[..sl]);
                    self.nodes[id].symlink_target.len = ci.symlink_len;
                }

                if !self.nodes[dir_vnode].children.insert(name_str, id as InodeId) {
                    self.nodes[id].active = false;
                }
            }
        }

        if child_count < BATCH {
            self.nodes[dir_vnode].children_loaded = true;
        }
        Ok(())
    }

    pub fn resolve_path_follow(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        let mut id = self.resolve_path(cwd, path)?;
        let mut depth = 0;
        while self.nodes[id].is_symlink() {
            if depth >= MAX_SYMLINK_DEPTH {
                return Err(VfsError::TooManySymlinks);
            }
            let mut target_buf = [0u8; NAME_LEN];
            let tlen = self.nodes[id].symlink_target.len as usize;
            target_buf[..tlen].copy_from_slice(&self.nodes[id].symlink_target.data[..tlen]);
            let target = unsafe { core::str::from_utf8_unchecked(&target_buf[..tlen]) };
            if target.is_empty() {
                return Err(VfsError::InvalidPath);
            }
            let parent = self.nodes[id].parent as usize;
            id = self.resolve_path(parent, target)?;
            depth += 1;
        }
        Ok(id)
    }

    #[inline]
    pub fn effective_node(&self, id: usize) -> usize {
        id
    }

    pub fn xm(&self, id: usize) -> usize {
        self.effective_node(id)
    }

    #[inline]
    fn is_readonly_fs(&self, id: usize) -> bool {
        matches!(self.nodes[id].fs_type, FsType::DevFS | FsType::ProcFS)
    }

    fn get_dev_type(&self, id: usize) -> Option<devfs::DevType> {
        if self.nodes[id].is_device() {
            devfs::dev_type_from_node(self.nodes[id].dev_major, self.nodes[id].dev_minor)
        } else {
            None
        }
    }

    fn split_path<'a>(&mut self, cwd: usize, path: &'a str) -> VfsResult<(usize, &'a str)> {
        match path.rfind('/') {
            Some(pos) => {
                let name = &path[pos + 1..];
                if name.is_empty() {
                    return Err(VfsError::InvalidPath);
                }
                let dir_part = &path[..pos];
                let parent = if dir_part.is_empty() {
                    self.resolve_path(cwd, "/")?
                } else {
                    self.resolve_path(cwd, dir_part)?
                };
                Ok((parent, name))
            }
            None => Ok((cwd, path)),
        }
    }

    fn check_access(&self, id: usize, flags: OpenFlags) -> VfsResult<()> {
        if self.ctx.cred.is_root() {
            return Ok(());
        }
        let node = &self.nodes[id];
        let who = if self.ctx.cred.euid == node.uid {
            PermWho::Owner
        } else if self.ctx.cred.in_group(node.gid) {
            PermWho::Group
        } else {
            PermWho::Other
        };
        let bits = node.mode.perm_bits_for(who);

        if flags.readable() && (bits & 0o4) == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if flags.writable() && (bits & 0o2) == 0 {
            return Err(VfsError::PermissionDenied);
        }
        Ok(())
    }

    fn check_dir_write(&self, dir_id: usize) -> VfsResult<()> {
        self.check_access(dir_id, OpenFlags(OpenFlags::WRITE))
    }

    pub fn free_file_pages(&mut self, id: usize) {
        let mut to_free = [INVALID_ID; DIRECT_BLOCKS];
        let mut free_count = 0;
        for (_, pid) in self.nodes[id].addr_space.iter_pages() {
            if free_count < DIRECT_BLOCKS {
                to_free[free_count] = pid;
                free_count += 1;
            }
        }
        for i in 0..free_count {
            if to_free[i] != INVALID_ID {
                self.page_cache.free_page(to_free[i]);
            }
        }
    }

    fn truncate_file(&mut self, id: usize) {
        if self.nodes[id].is_ext_backed() {
            let ino = self.nodes[id].ext2_ino;
            let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_truncate(ino)
            });
        }
        self.free_file_pages(id);
        self.nodes[id].size = 0;
        self.nodes[id].addr_space = AddressSpace::new();
        self.nodes[id].touch_mtime(self.now());
    }

    pub fn truncate_to(&mut self, id: usize, new_size: u64) {
        let old_size = self.nodes[id].size;
        if new_size >= old_size {
            self.nodes[id].size = new_size;
            return;
        }

        if self.nodes[id].is_ext_backed() {
            if new_size == 0 {
                let ino = self.nodes[id].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_truncate(ino)
                });
            }
        }

        let keep_pages = if new_size == 0 {
            0
        } else {
            ((new_size as usize - 1) / PAGE_SIZE) + 1
        };

        let mut to_free = [INVALID_ID; DIRECT_BLOCKS];
        let mut free_count = 0;
        for page_num in keep_pages..DIRECT_BLOCKS {
            if let Some(pid) = self.nodes[id].addr_space.get_page(page_num) {
                to_free[free_count] = pid;
                free_count += 1;
                self.nodes[id].addr_space.clear_page(page_num);
            }
        }
        for i in 0..free_count {
            self.page_cache.free_page(to_free[i]);
        }

        if new_size > 0 {
            let last_page = (new_size as usize - 1) / PAGE_SIZE;
            let zero_from = new_size as usize % PAGE_SIZE;
            if zero_from > 0 {
                if let Some(pid) = self.nodes[id].addr_space.get_page(last_page) {
                    if let Some(data) = self.page_cache.get_page_data_mut(pid) {
                        for i in zero_from..PAGE_SIZE {
                            data[i] = 0;
                        }
                        self.page_cache.mark_dirty(pid);
                    }
                }
            }
        }

        self.nodes[id].size = new_size;
        let ts = self.now();
        self.nodes[id].touch_mtime(ts);
        self.nodes[id].touch_ctime(ts);
    }

    #[inline]
    fn validate_name(name: &str) -> VfsResult<()> {
        if name.is_empty() || name.len() > NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        if name.contains('/') || name.contains('\0') {
            return Err(VfsError::InvalidArgument);
        }
        if name == "." || name == ".." {
            return Err(VfsError::InvalidArgument);
        }
        Ok(())
    }

    fn ensure_no_duplicate(&self, parent: usize, name: &str) -> VfsResult<()> {
        let eff = self.effective_node(parent);
        if let Some(id) = self.nodes[eff].children.find_by_name(name) {
            let c = id as usize;
            if c < MAX_VNODES && self.nodes[c].active {
                return Err(VfsError::AlreadyExists);
            }
        }
        if self.nodes[eff].is_ext_backed() {
            let parent_ino = self.nodes[eff].ext2_ino;
            let exists = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext2_lookup_in_dir(parent_ino, name)
                    .map(|r| r.is_some())
                    .unwrap_or(false)
            });
            if exists == Some(true) {
                return Err(VfsError::AlreadyExists);
            }
        }
        Ok(())
    }

    fn is_dir_empty(&self, id: usize) -> bool {
        let eff = self.effective_node(id);
        self.nodes[eff].children.is_empty()
    }

    pub(crate) fn evict_ext2_children(&mut self, dir_id: usize) {
        let mut to_evict: alloc::vec::Vec<InodeId> = alloc::vec::Vec::new();

        for (_, child_id) in self.nodes[dir_id].children.iter() {
            let cid = child_id as usize;
            if cid >= MAX_VNODES || !self.nodes[cid].active { continue; }
            if !self.nodes[cid].fs_type.is_ext_family()   { continue; }
            if self.nodes[cid].refcount > 0               { continue; }
            to_evict.push(child_id);
        }

        for child_id in to_evict {
            let cid = child_id as usize;
            if cid < MAX_VNODES && self.nodes[cid].active {
                if self.nodes[cid].is_dir() {
                    self.evict_ext2_children(cid);
                }
                let h = name_hash(self.nodes[cid].get_name());
                self.nodes[dir_id].children.remove(h, cid as InodeId);
                self.nodes[cid].active = false;
            }
        }

        self.nodes[dir_id].children_loaded = false;
    }

    pub(crate) fn evict_children_recursive(&mut self, dir_id: usize) {
        let mut to_evict: alloc::vec::Vec<InodeId> = alloc::vec::Vec::new();

        for (_, child_id) in self.nodes[dir_id].children.iter() {
            let cid = child_id as usize;
            if cid >= MAX_VNODES || !self.nodes[cid].active {
                continue;
            }
            if self.nodes[cid].refcount > 0 {
                continue;
            }
            to_evict.push(child_id);
        }

        for child_id in to_evict {
            let cid = child_id as usize;
            if cid < MAX_VNODES && self.nodes[cid].active {
                if self.nodes[cid].is_dir() {
                    self.evict_children_recursive(cid);
                }
                self.free_file_pages(cid);
                let h = name_hash(self.nodes[cid].get_name());
                self.nodes[dir_id].children.remove(h, cid as InodeId);
                self.nodes[cid].active = false;
            }
        }

        self.nodes[dir_id].children_loaded = false;
    }

    pub fn mkdir(&mut self, parent: usize, name: &str, mode: FileMode) -> VfsResult<usize> {
        Self::validate_name(name)?;
        let pid = self.effective_node(parent);

        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.is_readonly_fs(pid) {
            return Err(VfsError::ReadOnly);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, name)?;

        let id = self.alloc_vnode()?;
        let ts = self.now();
        let applied_mode = mode.apply_umask(self.ctx.umask);

        self.nodes[id].init(
            id as InodeId,
            pid as InodeId,
            name,
            VNodeKind::Directory,
            self.nodes[pid].fs_type,
            applied_mode,
            self.ctx.cred.euid,
            self.ctx.cred.egid,
            ts,
        );

        if self.nodes[pid].is_ext_backed() {
            let parent_ino = self.nodes[pid].ext2_ino;
            let disk_mode = applied_mode.0;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_create_dir(parent_ino, name, disk_mode)
            });
            match result {
                Some(Ok(new_ino)) => {
                    self.nodes[id].ext2_ino = new_ino;
                }
                Some(Err(_)) => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
                None => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
            }
        }

        if !self.nodes[pid].children.insert(name, id as InodeId) {
            if self.nodes[id].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_dir(parent_ino, name)
                });
            }
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].nlinks += 1;
        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!("[vfs] mkdir '{}' id={} parent={} ext2_ino={}", name, id, pid, self.nodes[id].ext2_ino);
        Ok(id)
    }

    pub fn rmdir(&mut self, cwd: usize, path: &str) -> VfsResult<()> {
        let id = self.resolve_path(cwd, path)?;

        if !self.nodes[id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if id == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        if self.nodes[id].is_ext_backed() {
            if !self.nodes[id].children_loaded {
                self.ext2_ensure_children_loaded(id)?;
            }
        }

        if !self.is_dir_empty(id) {
            return Err(VfsError::NotEmpty);
        }

        let pid = self.nodes[id].parent as usize;
        self.check_dir_write(pid)?;

        if self.nodes[id].is_ext_backed() && self.nodes[pid].ext2_ino != 0 {
            let parent_ino = self.nodes[pid].ext2_ino;
            let dir_name = self.nodes[id].get_name();
            let mut name_buf = [0u8; NAME_LEN];
            let nlen = dir_name.len().min(NAME_LEN);
            name_buf[..nlen].copy_from_slice(dir_name.as_bytes());
            let name_str = unsafe { core::str::from_utf8_unchecked(&name_buf[..nlen]) };
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_delete_dir(parent_ino, name_str)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) => return Err(VfsError::IoError),
                None => return Err(VfsError::IoError),
            }
        }

        let h = name_hash(self.nodes[id].get_name());
        self.nodes[pid].children.remove(h, id as InodeId);

        if self.nodes[pid].nlinks > 0 {
            self.nodes[pid].nlinks -= 1;
        }

        let ts = self.now();
        self.nodes[pid].touch_mtime(ts);
        self.nodes[id].nlinks = 0;
        self.nodes[id].active = false;
        if id < self.vnode_free_hint {
            self.vnode_free_hint = id;
        }

        crate::serial_println!("[vfs] rmdir '{}' id={}", path, id);
        Ok(())
    }

    pub fn readdir(&mut self, dir_id: usize, entries: &mut [DirEntry]) -> VfsResult<usize> {
        if !self.valid_vnode(dir_id) {
            return Err(VfsError::NotFound);
        }
        if !self.nodes[dir_id].is_dir() {
            return Err(VfsError::NotDirectory);
        }

        if self.nodes[dir_id].fs_type.is_ext_family() && !self.nodes[dir_id].children_loaded {
            self.ext2_ensure_children_loaded(dir_id)?;
        }

        let eff = self.effective_node(dir_id);
        let mut count = 0usize;

        if count < entries.len() {
            entries[count] = DirEntry::from_name(".", dir_id as InodeId, VNodeKind::Directory);
            entries[count].offset = 0;
            count += 1;
        }
        if count < entries.len() {
            let parent_id = self.nodes[dir_id].parent as usize;
            let par = if self.valid_vnode(parent_id) { parent_id } else { dir_id };
            entries[count] = DirEntry::from_name("..", par as InodeId, VNodeKind::Directory);
            entries[count].offset = 1;
            count += 1;
        }

        for (_, child_id) in self.nodes[eff].children.iter() {
            if count >= entries.len() {
                break;
            }
            let cid = child_id as usize;
            if !self.valid_vnode(cid) {
                continue;
            }
            entries[count] = DirEntry::from_name(
                self.nodes[cid].get_name(),
                cid as InodeId,
                self.nodes[cid].kind,
            );
            entries[count].offset = count as u32;
            count += 1;
        }

        Ok(count)
    }

    pub fn create_file(&mut self, parent: usize, name: &str, mode: FileMode) -> VfsResult<usize> {
        Self::validate_name(name)?;
        let pid = self.effective_node(parent);

        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.is_readonly_fs(pid) {
            return Err(VfsError::ReadOnly);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, name)?;

        let id = self.alloc_vnode()?;
        let ts = self.now();
        let applied_mode = mode.apply_umask(self.ctx.umask);

        self.nodes[id].init(
            id as InodeId,
            pid as InodeId,
            name,
            VNodeKind::Regular,
            self.nodes[pid].fs_type,
            applied_mode,
            self.ctx.cred.euid,
            self.ctx.cred.egid,
            ts,
        );

        if self.nodes[pid].is_ext_backed() {
            let parent_ino = self.nodes[pid].ext2_ino;
            let disk_mode = applied_mode.0;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_create_file(parent_ino, name, disk_mode)
            });
            match result {
                Some(Ok(new_ino)) => {
                    self.nodes[id].ext2_ino = new_ino;
                }
                Some(Err(_)) => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
                None => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
            }
        }

        if !self.nodes[pid].children.insert(name, id as InodeId) {
            if self.nodes[id].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let fname = self.nodes[id].get_name();
                let mut fname_buf = [0u8; NAME_LEN];
                let flen = fname.len().min(NAME_LEN);
                fname_buf[..flen].copy_from_slice(&fname.as_bytes()[..flen]);
                let fname_str = unsafe { core::str::from_utf8_unchecked(&fname_buf[..flen]) };
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_file(parent_ino, fname_str)
                });
            }
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!("[vfs] create '{}' id={} parent={} ext2_ino={}", name, id, pid, self.nodes[id].ext2_ino);
        Ok(id)
    }

    pub fn symlink(&mut self, parent: usize, linkname: &str, target: &str) -> VfsResult<usize> {
        Self::validate_name(linkname)?;
        if target.is_empty() || target.len() > NAME_LEN {
            return Err(VfsError::NameTooLong);
        }

        let pid = self.effective_node(parent);
        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.is_readonly_fs(pid) {
            return Err(VfsError::ReadOnly);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, linkname)?;

        let id = self.alloc_vnode()?;
        let ts = self.now();

        self.nodes[id].init(
            id as InodeId,
            pid as InodeId,
            linkname,
            VNodeKind::Symlink,
            self.nodes[pid].fs_type,
            FileMode::default_symlink(),
            self.ctx.cred.euid,
            self.ctx.cred.egid,
            ts,
        );
        self.nodes[id].symlink_target = NameBuf::from_str(target);
        self.nodes[id].size = target.len() as u64;

        if self.nodes[pid].is_ext_backed() {
            let parent_ino = self.nodes[pid].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_create_symlink(parent_ino, linkname, target)
            });
            match result {
                Some(Ok(new_ino)) => {
                    self.nodes[id].ext2_ino = new_ino;
                }
                Some(Err(_)) => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
                None => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
            }
        }

        if !self.nodes[pid].children.insert(linkname, id as InodeId) {
            if self.nodes[id].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_file(parent_ino, linkname)
                });
            }
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!(
            "[vfs] symlink '{}' -> '{}' id={} ext2_ino={}",
            linkname, target, id, self.nodes[id].ext2_ino
        );
        Ok(id)
    }

    pub fn readlink(&mut self, cwd: usize, path: &str) -> VfsResult<NameBuf> {
        let id = self.resolve_path_lstat(cwd, path)?;
        if !self.nodes[id].is_symlink() {
            return Err(VfsError::NotSymlink);
        }
        Ok(self.nodes[id].symlink_target)
    }

    pub fn link(
        &mut self,
        cwd: usize,
        existing_path: &str,
        new_parent: usize,
        new_name: &str,
    ) -> VfsResult<()> {
        Self::validate_name(new_name)?;

        let target_id = self.resolve_path_follow(cwd, existing_path)?;

        if self.nodes[target_id].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.is_readonly_fs(target_id) {
            return Err(VfsError::ReadOnly);
        }

        let pid = self.effective_node(new_parent);
        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.nodes[pid].fs_type != self.nodes[target_id].fs_type {
            return Err(VfsError::CrossDevice);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, new_name)?;

        if self.nodes[pid].is_ext_backed()
            && self.nodes[pid].ext2_ino != 0
            && self.nodes[target_id].ext2_ino != 0
        {
            let parent_ino = self.nodes[pid].ext2_ino;
            let target_ino = self.nodes[target_id].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_hardlink(parent_ino, new_name, target_ino)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) => return Err(VfsError::IoError),
                None => return Err(VfsError::IoError),
            }
        }

        if !self.nodes[pid].children.insert(new_name, target_id as InodeId) {
            if self.nodes[pid].is_ext_backed() && self.nodes[pid].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_file(parent_ino, new_name)
                });
            }
            return Err(VfsError::NoSpace);
        }

        self.nodes[target_id].nlinks += 1;
        let ts = self.now();
        self.nodes[target_id].touch_ctime(ts);
        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!(
            "[vfs] hardlink '{}' id={} nlinks={}",
            new_name,
            target_id,
            self.nodes[target_id].nlinks
        );
        Ok(())
    }

    pub fn open(
        &mut self,
        cwd: usize,
        path: &str,
        flags: OpenFlags,
        mode: FileMode,
    ) -> VfsResult<usize> {
        crate::serial_println!("[vfs] open '{}' flags=0x{:x}", path, flags.0);

        let nofollow = flags.has(OpenFlags::NOFOLLOW);

        let id = if nofollow {
            match self.resolve_path(cwd, path) {
                Ok(id) => {
                    if self.nodes[id].is_symlink() {
                        return Err(VfsError::Loop);
                    }
                    if flags.has(OpenFlags::DIRECTORY) && !self.nodes[id].is_dir() {
                        return Err(VfsError::NotDirectory);
                    }
                    if flags.has(OpenFlags::CREATE) && flags.has(OpenFlags::EXCLUSIVE) {
                        return Err(VfsError::AlreadyExists);
                    }
                    self.check_access(id, flags)?;
                    if flags.has(OpenFlags::TRUNCATE) && flags.writable() && self.nodes[id].is_regular() {
                        self.truncate_file(id);
                    }
                    id
                }
                Err(VfsError::NotFound) if flags.has(OpenFlags::CREATE) => {
                    let (parent, name) = self.split_path(cwd, path)?;
                    self.create_file(parent, name, mode)?
                }
                Err(e) => return Err(e),
            }
        } else {
            match self.resolve_path_follow(cwd, path) {
                Ok(id) => {
                    if flags.has(OpenFlags::DIRECTORY) && !self.nodes[id].is_dir() {
                        return Err(VfsError::NotDirectory);
                    }
                    if flags.has(OpenFlags::CREATE) && flags.has(OpenFlags::EXCLUSIVE) {
                        return Err(VfsError::AlreadyExists);
                    }
                    self.check_access(id, flags)?;
                    if flags.has(OpenFlags::TRUNCATE) && flags.writable() && self.nodes[id].is_regular() {
                        self.truncate_file(id);
                    }
                    id
                }
                Err(VfsError::NotFound) if flags.has(OpenFlags::CREATE) => {
                    let (parent, name) = self.split_path(cwd, path)?;
                    self.create_file(parent, name, mode)?
                }
                Err(e) => return Err(e),
            }
        };

        if flags.writable() && self.is_readonly_fs(id) && self.get_dev_type(id).is_none() {
            return Err(VfsError::ReadOnly);
        }

        let fd = self.fd_table.alloc(id as InodeId, flags)?;
        self.nodes[id].inc_ref();

        crate::serial_println!(
            "[vfs] opened fd={} vnode={} refs={}",
            fd,
            id,
            self.nodes[id].refcount
        );
        Ok(fd)
    }

    pub fn close(&mut self, fd: usize) -> VfsResult<()> {
        let vid = self.fd_table.get(fd)?.vnode_id as usize;
        self.fd_table.close(fd)?;

        if self.valid_vnode(vid) && self.nodes[vid].refcount > 0 {
            self.nodes[vid].dec_ref();

            if self.nodes[vid].nlinks == 0 && self.nodes[vid].refcount == 0 {
                if self.nodes[vid].is_ext_backed() {
                    let pid = self.nodes[vid].parent as usize;
                    if self.valid_vnode(pid) && self.nodes[pid].ext2_ino != 0 {
                        let parent_ino = self.nodes[pid].ext2_ino;
                        let file_name = self.nodes[vid].get_name();
                        let mut name_buf = [0u8; NAME_LEN];
                        let nlen = file_name.len().min(NAME_LEN);
                        name_buf[..nlen].copy_from_slice(file_name.as_bytes());
                        let name_str = unsafe { core::str::from_utf8_unchecked(&name_buf[..nlen]) };
                        let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                            fs.ext3_delete_file(parent_ino, name_str)
                        });
                    }
                }

                crate::serial_println!("[vfs] deferred free vnode {}", vid);
                self.free_file_pages(vid);
                self.nodes[vid].active = false;
                if vid < self.vnode_free_hint {
                    self.vnode_free_hint = vid;
                }
            }
        }

        crate::serial_println!("[vfs] close fd={} vnode={}", fd, vid);
        Ok(())
    }

    pub fn dup(&mut self, old_fd: usize) -> VfsResult<usize> {
        let file = *self.fd_table.get(old_fd)?;
        let new_fd = self.fd_table.alloc(file.vnode_id, file.flags)?;

        let vid = file.vnode_id as usize;
        if self.valid_vnode(vid) {
            self.nodes[vid].inc_ref();
        }

        let offset = file.offset;
        self.fd_table.get_mut(new_fd)?.offset = offset;

        Ok(new_fd)
    }

    pub fn dup_to(&mut self, old_fd: usize, new_fd: usize) -> VfsResult<usize> {
        let file = *self.fd_table.get(old_fd)?;

        // close new_fd if open, decrement refcount
        if self.fd_table.get(new_fd).is_ok() {
            let _ = self.close(new_fd);
        }

        self.fd_table.alloc_at(new_fd, file.vnode_id, file.flags)?;

        let vid = file.vnode_id as usize;
        if self.valid_vnode(vid) {
            self.nodes[vid].inc_ref();
        }

        self.fd_table.get_mut(new_fd)?.offset = file.offset;

        Ok(new_fd)
    }

    pub fn read(&mut self, fd: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let file = self.fd_table.get(fd)?;
        if !file.flags.readable() {
            return Err(VfsError::PermissionDenied);
        }

        let vid = file.vnode_id as usize;
        let offset = file.offset;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }

        if buf.is_empty() {
            return Ok(0);
        }

        if self.nodes[vid].fs_type == FsType::ProcFS {
            return self.read_procfs(fd, vid, offset, buf);
        }

        if let Some(dt) = self.get_dev_type(vid) {
            let n = devfs::dev_read(dt, buf, offset)?;
            self.fd_table.get_mut(fd)?.offset += n as u64;
            return Ok(n);
        }

        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }

        if self.nodes[vid].is_ext_backed() {
            return self.read_ext2_file(fd, vid, offset, buf);
        }

        let size = self.nodes[vid].size;
        if offset >= size {
            return Ok(0);
        }

        let avail = (size - offset) as usize;
        let to_read = buf.len().min(avail);
        let mut done = 0;

        while done < to_read {
            let file_off = offset as usize + done;
            let page_num = file_off / PAGE_SIZE;
            let page_off = file_off % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(to_read - done);

            match self.nodes[vid].addr_space.get_page(page_num) {
                Some(pid) => {
                    if let Some(data) = self.page_cache.get_page_data(pid) {
                        buf[done..done + chunk].copy_from_slice(&data[page_off..page_off + chunk]);
                    } else {
                        buf[done..done + chunk].fill(0);
                    }
                }
                None => {
                    buf[done..done + chunk].fill(0);
                }
            }
            done += chunk;
        }

        if !self.nodes[vid].flags.no_atime {
            let ts = self.now();
            self.nodes[vid].touch_atime(ts);
        }

        self.fd_table.get_mut(fd)?.offset += done as u64;
        Ok(done)
    }

    fn read_ext2_file(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        buf: &mut [u8],
    ) -> VfsResult<usize> {
        let ext2_ino = self.nodes[vid].ext2_ino;

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            let inode = fs.read_inode(ext2_ino).map_err(|_| VfsError::IoError)?;
            let size = inode.size() as u64;
            if offset >= size {
                return Ok((0usize, size));
            }
            let avail = (size - offset) as usize;
            let to_read = buf.len().min(avail);
            let n = fs
                .read_file(&inode, offset, &mut buf[..to_read])
                .map_err(|_| VfsError::IoError)?;
            // update atime on read (relatime: only if atime < mtime or atime > 1 day old :)
            let _ = fs.touch_atime(ext2_ino);
            Ok((n, size))
        });

        let (n, disk_size) = match result {
            Some(Ok(pair)) => pair,
            Some(Err(e)) => return Err(e),
            None => return Err(VfsError::IoError),
        };

        if disk_size > 0 {
            self.nodes[vid].size = disk_size;
        }

        self.fd_table.get_mut(fd)?.offset += n as u64;
        Ok(n)
    }

    fn read_procfs(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        buf: &mut [u8],
    ) -> VfsResult<usize> {
        let mut name_copy = [0u8; NAME_LEN];
        let name_bytes = self.nodes[vid].get_name().as_bytes();
        let name_len = name_bytes.len().min(NAME_LEN);
        name_copy[..name_len].copy_from_slice(&name_bytes[..name_len]);
        let name_str = match core::str::from_utf8(&name_copy[..name_len]) {
            Ok(s) => s,
            Err(_) => return Err(VfsError::NotFound),
        };

        let vnode_used = self.total_vnodes();
        let mut proc_buf = [0u8; 192];

        match procfs::proc_read(name_str, &mut proc_buf, vnode_used) {
            Ok(total) => {
                let off = offset as usize;
                if off >= total {
                    return Ok(0);
                }
                let avail = total - off;
                let to_copy = buf.len().min(avail);
                buf[..to_copy].copy_from_slice(&proc_buf[off..off + to_copy]);
                self.fd_table.get_mut(fd)?.offset += to_copy as u64;
                Ok(to_copy)
            }
            Err(e) => Err(e),
        }
    }

    pub fn write(&mut self, fd: usize, data: &[u8]) -> VfsResult<usize> {
        let file = self.fd_table.get(fd)?;
        if !file.flags.writable() {
            return Err(VfsError::PermissionDenied);
        }

        let vid = file.vnode_id as usize;
        let is_append = file.flags.has(OpenFlags::APPEND);
        let is_sync = file.flags.has(OpenFlags::SYNC);
        let mut offset = file.offset as usize;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }

        if data.is_empty() {
            return Ok(0);
        }

        if self.nodes[vid].fs_type == FsType::ProcFS {
            return Err(VfsError::ReadOnly);
        }

        if let Some(dt) = self.get_dev_type(vid) {
            let n = devfs::dev_write(dt, data, offset as u64)?;
            self.fd_table.get_mut(fd)?.offset += n as u64;
            return Ok(n);
        }

        if self.nodes[vid].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.nodes[vid].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }
        if self.nodes[vid].flags.append_only && !is_append {
            offset = self.nodes[vid].size as usize;
        }

        if is_append {
            if self.nodes[vid].is_ext2_backed() {
                let ino = self.nodes[vid].ext2_ino;
                let disk_size = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.read_inode(ino).map(|i| i.size()).unwrap_or(0)
                });
                if let Some(sz) = disk_size {
                    self.nodes[vid].size = sz;
                }
            }
            offset = self.nodes[vid].size as usize;
        }

        if self.nodes[vid].is_ext_backed() {
            return self.write_ext2_file(fd, vid, offset as u64, data);
        }

        // partial write if data exceeds max file size (POSIX semantics)
        let max = AddressSpace::max_size() as usize;
        if offset >= max {
            return Err(VfsError::FileTooLarge);
        }
        let available = max - offset;
        let to_write = data.len().min(available);

        let mut done = 0;
        while done < to_write {
            let file_off = offset + done;
            let page_num = file_off / PAGE_SIZE;
            let page_off = file_off % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(to_write - done);

            let pid = match self.nodes[vid].addr_space.get_page(page_num) {
                Some(pid) => pid,
                None => {
                    let pid = self.page_cache.alloc_page()?;
                    self.nodes[vid].addr_space.set_page(page_num, pid)?;
                    pid
                }
            };

            if let Some(page_data) = self.page_cache.get_page_data_mut(pid) {
                page_data[page_off..page_off + chunk].copy_from_slice(&data[done..done + chunk]);
                self.page_cache.mark_dirty(pid);
            } else {
                return Err(VfsError::IoError);
            }

            done += chunk;
        }

        let new_end = offset + done;
        if new_end as u64 > self.nodes[vid].size {
            self.nodes[vid].size = new_end as u64;
        }

        let ts = self.now();
        self.nodes[vid].touch_mtime(ts);
        self.nodes[vid].flags.dirty = true;

        self.fd_table.get_mut(fd)?.offset = new_end as u64;

        if is_sync {
            self.nodes[vid].flags.dirty = false;
        }

        Ok(done)
    }

    fn write_ext2_file(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        data: &[u8],
    ) -> VfsResult<usize> {
        let ext2_ino = self.nodes[vid].ext2_ino;
        let is_sync = self.fd_table.get(fd).map(|f| f.flags.has(OpenFlags::SYNC)).unwrap_or(false);

        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            fs.ext3_write_file(ext2_ino, data, offset)
                .map_err(|_| VfsError::IoError)
        });

        let n = match result {
            Some(Ok(n)) => n,
            Some(Err(e)) => return Err(e),
            None => return Err(VfsError::IoError),
        };

        let new_end = offset + n as u64;
        if new_end > self.nodes[vid].size {
            self.nodes[vid].size = new_end;
        }

        let ts = self.now();
        self.nodes[vid].touch_mtime(ts);
        self.nodes[vid].flags.dirty = true;

        if is_sync {
            let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.sync());
            self.nodes[vid].flags.dirty = false;
        }

        self.fd_table.get_mut(fd)?.offset = new_end;
        Ok(n)
    }

    pub fn seek(&mut self, fd: usize, whence: SeekFrom) -> VfsResult<u64> {
        let file = self.fd_table.get(fd)?;
        let vid = file.vnode_id as usize;
        let current = file.offset;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }

        let size = self.nodes[vid].size;

        let new_offset: i64 = match whence {
            SeekFrom::Start(pos) => pos as i64,
            SeekFrom::Current(delta) => current as i64 + delta,
            SeekFrom::End(delta) => size as i64 + delta,
        };

        if new_offset < 0 {
            return Err(VfsError::SeekError);
        }

        self.fd_table.get_mut(fd)?.offset = new_offset as u64;
        Ok(new_offset as u64)
    }

    pub fn fsync(&mut self, fd: usize) -> VfsResult<()> {
        let file = self.fd_table.get(fd)?;
        let vid = file.vnode_id as usize;

        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }

        if self.nodes[vid].fs_type.is_ext_family() {
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.sync().map_err(|_| VfsError::IoError)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(e)) => return Err(e),
                None => return Err(VfsError::IoError),
            }
        }

        self.nodes[vid].flags.dirty = false;
        Ok(())
    }

    pub fn unlink(&mut self, cwd: usize, path: &str) -> VfsResult<()> {
        let id = self.resolve_path_lstat(cwd, path)?;

        if self.nodes[id].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        // extract the basename from path for correct hardlink removal
        let entry_name = match path.rfind('/') {
            Some(pos) => &path[pos + 1..],
            None => path,
        };

        let pid = self.nodes[id].parent as usize;
        self.check_dir_write(pid)?;

        if self.nodes[id].is_ext_backed()
            && self.nodes[pid].ext2_ino != 0
            && self.nodes[id].refcount == 0
        {
            let parent_ino = self.nodes[pid].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_delete_file(parent_ino, entry_name)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) => return Err(VfsError::IoError),
                None => return Err(VfsError::IoError),
            }
        }

        self.nodes[pid].children.remove_by_name(entry_name);

        let ts = self.now();
        self.nodes[pid].touch_mtime(ts);

        self.nodes[id].nlinks = self.nodes[id].nlinks.saturating_sub(1);

        crate::serial_println!(
            "[vfs] unlink '{}' id={} nlinks={} refs={}",
            path,
            id,
            self.nodes[id].nlinks,
            self.nodes[id].refcount
        );

        if self.nodes[id].nlinks == 0 && self.nodes[id].refcount == 0 {
            self.free_file_pages(id);
            self.nodes[id].active = false;
            if id < self.vnode_free_hint {
                self.vnode_free_hint = id;
            }
        } else {
            self.nodes[id].touch_ctime(ts);
        }

        Ok(())
    }

    pub fn rename(&mut self, cwd: usize, old: &str, new_path: &str) -> VfsResult<()> {
        let id = self.resolve_path(cwd, old)?;
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        let old_pid = self.nodes[id].parent as usize;

        let (new_pid, new_base) = self.split_path(cwd, new_path)?;
        Self::validate_name(new_base)?;

        if !self.nodes[new_pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.nodes[new_pid].fs_type != self.nodes[id].fs_type {
            return Err(VfsError::CrossDevice);
        }

        self.check_dir_write(old_pid)?;
        if new_pid != old_pid {
            self.check_dir_write(new_pid)?;
        }

        if self.nodes[id].is_dir() {
            let mut cur = new_pid;
            loop {
                if cur == id {
                    return Err(VfsError::InvalidArgument);
                }
                let p = self.nodes[cur].parent as usize;
                if p == cur || cur == 0 {
                    break;
                }
                cur = p;
            }
        }

        if let Some(existing) = self.nodes[new_pid].children.find_by_name(new_base) {
            let c = existing as usize;
            if c < MAX_VNODES && self.nodes[c].active && c != id {
                return Err(VfsError::AlreadyExists);
            }
        }

        if self.nodes[id].is_ext_backed()
            && self.nodes[old_pid].ext2_ino != 0
            && self.nodes[id].ext2_ino != 0
        {
            if new_pid != old_pid {
                return Err(VfsError::CrossDevice);
            }
            let parent_ino = self.nodes[old_pid].ext2_ino;
            let old_name = self.nodes[id].get_name();
            let mut old_buf = [0u8; NAME_LEN];
            let olen = old_name.len().min(NAME_LEN);
            old_buf[..olen].copy_from_slice(old_name.as_bytes());
            let old_str = unsafe { core::str::from_utf8_unchecked(&old_buf[..olen]) };
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_rename(parent_ino, old_str, new_base)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) => return Err(VfsError::IoError),
                None => return Err(VfsError::IoError),
            }
        }

        let mut old_name_buf = [0u8; NAME_LEN];
        let old_name_len = self.nodes[id].name.len as usize;
        old_name_buf[..old_name_len].copy_from_slice(&self.nodes[id].name.data[..old_name_len]);
        let old_name_str = unsafe { core::str::from_utf8_unchecked(&old_name_buf[..old_name_len]) };

        self.nodes[old_pid].children.remove_by_name(old_name_str);
        self.nodes[id].name = NameBuf::from_str(new_base);
        self.nodes[id].parent = new_pid as InodeId;

        if !self.nodes[new_pid].children.insert(new_base, id as InodeId) {
            let rollback_name = match core::str::from_utf8(&old_name_buf[..old_name_len]) {
                Ok(s) => s,
                Err(_) => {
                    self.nodes[id].active = false;
                    return Err(VfsError::Corrupted);
                }
            };
            self.nodes[id].name = NameBuf::from_str(rollback_name);
            self.nodes[id].parent = old_pid as InodeId;
            let _ = self.nodes[old_pid].children.insert(rollback_name, id as InodeId);
            if self.nodes[id].is_ext_backed() && self.nodes[old_pid].ext2_ino != 0 {
                let parent_ino = self.nodes[old_pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_rename(parent_ino, new_base, rollback_name)
                });
            }
            return Err(VfsError::NoSpace);
        }

        let ts = self.now();

        if new_pid != old_pid && self.nodes[id].is_dir() {
            if self.nodes[old_pid].nlinks > 0 {
                self.nodes[old_pid].nlinks -= 1;
            }
            self.nodes[new_pid].nlinks = self.nodes[new_pid].nlinks.saturating_add(1);
            self.nodes[old_pid].touch_mtime(ts);
        }

        self.nodes[id].touch_ctime(ts);
        self.nodes[new_pid].touch_mtime(ts);

        Ok(())
    }

    fn ext2_refresh_size(&mut self, id: usize) {
        if !self.nodes[id].is_ext_backed() {
            return;
        }
        let ino = self.nodes[id].ext2_ino;
        let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            fs.read_inode(ino).map(|inode| (inode.size(), inode.blocks()))
        });
        if let Some(Ok((size, blocks))) = result {
            self.nodes[id].size = size;
            self.nodes[id].addr_space.nr_pages = blocks;
        }
    }

    pub fn stat(&mut self, cwd: usize, path: &str) -> VfsResult<VNodeStat> {
        let id = self.resolve_path_follow(cwd, path)?;
        self.ext2_refresh_size(id);
        Ok(self.nodes[id].stat())
    }

    pub fn lstat(&mut self, cwd: usize, path: &str) -> VfsResult<VNodeStat> {
        let id = self.resolve_path(cwd, path)?;
        self.ext2_refresh_size(id);
        Ok(self.nodes[id].stat())
    }

    pub fn fstat(&mut self, fd: usize) -> VfsResult<VNodeStat> {
        let vid = self.fd_table.get(fd)?.vnode_id as usize;
        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }
        self.ext2_refresh_size(vid);
        Ok(self.nodes[vid].stat())
    }

    pub fn chmod(&mut self, cwd: usize, path: &str, mode: FileMode) -> VfsResult<()> {
        let id = self.resolve_path_follow(cwd, path)?;

        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if !self.ctx.cred.is_root() && self.ctx.cred.euid != self.nodes[id].uid {
            return Err(VfsError::PermissionDenied);
        }

        if self.nodes[id].is_ext_backed() {
            let ino = self.nodes[id].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext2_chmod(ino, mode.0)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) => return Err(VfsError::IoError),
                None => return Err(VfsError::IoError),
            }
        }

        self.nodes[id].mode = mode;
        self.nodes[id].touch_ctime(self.now());
        Ok(())
    }

    pub fn chown(
        &mut self,
        cwd: usize,
        path: &str,
        uid: Option<u16>,
        gid: Option<u16>,
    ) -> VfsResult<()> {
        let id = self.resolve_path_follow(cwd, path)?;

        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if !self.ctx.cred.is_root() {
            return Err(VfsError::PermissionDenied);
        }

        let new_uid = uid.unwrap_or(self.nodes[id].uid);
        let new_gid = gid.unwrap_or(self.nodes[id].gid);

        if self.nodes[id].is_ext_backed() {
            let ino = self.nodes[id].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext2_chown(ino, new_uid, new_gid)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) => return Err(VfsError::IoError),
                None => return Err(VfsError::IoError),
            }
        }

        self.nodes[id].uid = new_uid;
        self.nodes[id].gid = new_gid;
        self.nodes[id].touch_ctime(self.now());
        Ok(())
    }

    pub fn setattr(&mut self, cwd: usize, path: &str, attr: SetAttr) -> VfsResult<()> {
        let id = self.resolve_path_follow(cwd, path)?;

        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        if let Some(mode) = attr.mode {
            if !self.ctx.cred.is_root() && self.ctx.cred.euid != self.nodes[id].uid {
                return Err(VfsError::PermissionDenied);
            }
            if self.nodes[id].is_ext_backed() {
                let ino = self.nodes[id].ext2_ino;
                let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext2_chmod(ino, mode.0)
                });
                if matches!(result, Some(Err(_)) | None) {
                    return Err(VfsError::IoError);
                }
            }
            self.nodes[id].mode = mode;
        }

        if attr.uid.is_some() || attr.gid.is_some() {
            if !self.ctx.cred.is_root() {
                return Err(VfsError::PermissionDenied);
            }
            let new_uid = attr.uid.unwrap_or(self.nodes[id].uid);
            let new_gid = attr.gid.unwrap_or(self.nodes[id].gid);
            if self.nodes[id].is_ext_backed() {
                let ino = self.nodes[id].ext2_ino;
                let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext2_chown(ino, new_uid, new_gid)
                });
                if matches!(result, Some(Err(_)) | None) {
                    return Err(VfsError::IoError);
                }
            }
            self.nodes[id].uid = new_uid;
            self.nodes[id].gid = new_gid;
        }

        if let Some(new_size) = attr.size {
            if !self.nodes[id].is_regular() {
                return Err(VfsError::InvalidArgument);
            }
            self.truncate_to(id, new_size);
        }

        if let Some(atime) = attr.atime {
            self.nodes[id].atime = atime;
        }
        if let Some(mtime) = attr.mtime {
            self.nodes[id].mtime = mtime;
        }

        self.nodes[id].touch_ctime(self.now());
        Ok(())
    }

    pub fn statfs(&self, cwd: usize, path: &str) -> VfsResult<StatFs> {
        let id = PathWalker::resolve(&self.nodes, cwd, path)?;
        let fs_type = self.nodes[id].fs_type;

        if fs_type.is_ext_family() {
            let info = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.fs_info());
            if let Some(info) = info {
                return Ok(StatFs {
                    fs_type,
                    block_size: info.block_size,
                    total_blocks: info.total_blocks as u64,
                    free_blocks: info.free_blocks as u64,
                    total_inodes: info.total_inodes as u64,
                    free_inodes: info.free_inodes as u64,
                    max_name_len: 255,
                    flags: 0,
                });
            }
        }

        let total_inodes = MAX_VNODES as u64;
        let used_inodes = self.total_vnodes() as u64;

        Ok(StatFs {
            fs_type,
            block_size: PAGE_SIZE as u32,
            total_blocks: MAX_DATA_PAGES as u64,
            free_blocks: self.page_cache.slab.free_count() as u64,
            total_inodes,
            free_inodes: total_inodes.saturating_sub(used_inodes),
            max_name_len: NAME_LEN as u32,
            flags: if self.is_readonly_fs(id) { 1 } else { 0 },
        })
    }

    pub fn total_vnodes(&self) -> usize {
        self.nodes.iter().filter(|v| v.active).count()
    }

    pub fn total_mounts(&self) -> usize {
        self.mounts.count as usize
    }

    pub fn is_vnode_open(&self, vid: usize) -> bool {
        for i in 0..MAX_OPEN_FILES {
            if self.fd_table.files[i].active && self.fd_table.files[i].vnode_id as usize == vid {
                return true;
            }
        }
        false
    }
}
