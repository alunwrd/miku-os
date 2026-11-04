// Root filesystem initialization and standard mounts:
// devfs, procfs, /lib (system libraries), /mnt

use super::MikuVFS;
use crate::fs::devfs;
use crate::fs::procfs;
use crate::fs::vfs::types::*;

/// Format a raw block-node name into 'buf': "blkN" for part 0, "blkNpM"
/// otherwise. Returns the byte length written
fn fmt_block_name(buf: &mut [u8; 8], dev: u8, part: u8) -> usize {
    fn push_dec(buf: &mut [u8; 8], pos: &mut usize, mut v: u8) {
        if v >= 10 {
            buf[*pos] = b'0' + v / 10;
            *pos += 1;
            v %= 10;
        }
        buf[*pos] = b'0' + v;
        *pos += 1;
    }
    let mut pos = 0;
    buf[..3].copy_from_slice(b"blk");
    pos += 3;
    push_dec(buf, &mut pos, dev);
    if part > 0 {
        buf[pos] = b'p';
        pos += 1;
        push_dec(buf, &mut pos, part);
    }
    pos
}

mod syslibs {
    pub struct SysLib {
        pub dir: &'static str,
        pub name: &'static str,
        pub data: &'static [u8],
    }

    // System libraries installed into /lib; bytes shared with the solib preload
    // cache (crate::process::ldso::MIKU_LIBS)
    pub fn libs() -> impl Iterator<Item = SysLib> {
        crate::process::ldso::MIKU_LIBS.iter().map(|l| SysLib {
            dir: "lib",
            name: l.name,
            data: l.bytes,
        })
    }
}

/// A small unsigned number into a byte buffer. Returns how many digits
fn fmt_u8(out: &mut [u8], mut v: u8) -> usize {
    if v == 0 { out[0] = b'0'; return 1; }
    let mut tmp = [0u8; 3];
    let mut i = 0;
    while v > 0 { tmp[i] = b'0' + v % 10; v /= 10; i += 1; }
    for j in 0..i { out[j] = tmp[i - 1 - j]; }
    i
}

impl MikuVFS {
    pub(super) fn bootstrap(&mut self) {
        // The root is id 0: an absolute path starts there, and a working
        // directory of 0 is the root. It is the first vnode made
        let root = self.alloc_vnode();
        assert_eq!(root, Ok(0), "vfs: the root is not vnode 0");
        self.init_vnode(0, 
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
        self.create_tmp();
        self.create_run();
    }

    fn mount_syslibs(&mut self) {
        crate::trace_println!("[vfs] mounting syslibs");

        let mut files_created = 0u8;

        for lib in syslibs::libs() {
            let dir_id = match self.lookup_or_create_lib_dir(lib.dir) {
                Some(id) => id,
                None => continue,
            };

            if self.install_syslib_file(dir_id, &lib).is_some() {
                files_created += 1;
            }
        }

        crate::serial_println!("[vfs] syslibs: {} files", files_created);
    }

    fn lookup_or_create_lib_dir(&mut self, dir_name: &str) -> Option<usize> {
        let found = self.nodes[0]
            .children
            .find_by_name(dir_name)
            .and_then(|id| {
                let c = id as usize;
                if self.nodes.contains(c) && self.nodes[c].active {
                    Some(c)
                } else {
                    None
                }
            });
        if let Some(id) = found {
            return Some(id);
        }

        let id = self.alloc_vnode().ok()?;
        let ts = self.now();
        self.init_vnode(id, 
            id as InodeId,
            0,
            dir_name,
            VNodeKind::Directory,
            FsType::TmpFS,
            FileMode::new(0o755),
            0,
            0,
            ts,
        );
        self.nodes[id].flags.immutable = true;
        if !self.nodes[0].children.insert(dir_name, id as InodeId) {
            self.free_vnode(id);
            return None;
        }
        Some(id)
    }

    fn install_syslib_file(&mut self, dir_id: usize, lib: &syslibs::SysLib) -> Option<usize> {
        let file_id = self.alloc_vnode().ok()?;
        let ts = self.now();
        self.init_vnode(file_id, 
            file_id as InodeId,
            dir_id as InodeId,
            lib.name,
            VNodeKind::Regular,
            FsType::TmpFS,
            FileMode::new(0o555),
            0,
            0,
            ts,
        );

        // Embedded blobs are served from kernel image memory: no page-cache
        // copy, no size limit
        self.nodes[file_id].static_data = Some(lib.data);
        self.nodes[file_id].size = lib.data.len() as u64;
        self.nodes[file_id].flags.immutable = true;

        if !self.nodes[dir_id]
            .children
            .insert(lib.name, file_id as InodeId)
        {
            self.free_vnode(file_id);
            return None;
        }

        crate::trace_println!(
            "[vfs] syslib: /{}/{} vnode={} {} bytes (immutable)",
            lib.dir,
            lib.name,
            file_id,
            lib.data.len()
        );
        Some(file_id)
    }

    fn create_mnt(&mut self) {
        let mnt_id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(_) => return,
        };
        self.init_vnode(mnt_id, 
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
            self.free_vnode(mnt_id);
        }
    }

    /// /tmp. Core dumps land here; tmpfs pages are RAM, so `process::coredump`
    /// polices its own share of it
    fn create_tmp(&mut self) {
        let id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(_) => return,
        };
        self.init_vnode(id, 
            id as InodeId,
            0,
            "tmp",
            VNodeKind::Directory,
            FsType::TmpFS,
            FileMode::new(0o1777),
            0,
            0,
            self.now(),
        );
        if self.nodes[0].children.insert("tmp", id as InodeId) {
            crate::serial_println!("[vfs] /tmp created");
        } else {
            self.free_vnode(id);
        }
    }

    /// /run: where a service puts the socket it can be reached on.
    ///
    /// Mode 0755: only root may create here, which gives the name
    /// `/run/mikudo.sock` its meaning. In a world-writable /run an ordinary
    /// user could bind that name first and collect passwords from every program
    /// that connects
    fn create_run(&mut self) {
        let id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(_) => return,
        };
        self.init_vnode(id, 
            id as InodeId,
            0,
            "run",
            VNodeKind::Directory,
            FsType::TmpFS,
            FileMode::new(0o755),
            0,
            0,
            self.now(),
        );
        if self.nodes[0].children.insert("run", id as InodeId) {
            crate::serial_println!("[vfs] /run created");
        } else {
            self.free_vnode(id);
        }
    }

    fn mount_devfs(&mut self) {
        crate::trace_println!("[vfs] mounting devfs");

        let dev_id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(e) => {
                crate::serial_println!("[vfs] devfs alloc failed: {:?}", e);
                return;
            }
        };

        self.init_vnode(dev_id, 
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
            self.free_vnode(dev_id);
            return;
        }

        let _ = self.mounts.add(FsType::DevFS, dev_id as InodeId, 0);

        // One master node per pty slot, made here and never by userspace: devfs
        // is read-only from ring 3. Mode 0: the only route to one is
        // `open("/dev/ptmx")`, which substitutes it.
        //
        // And one slave node per slot beside it, `ptsN`, the end a program in
        // the terminal opens (`openpty`, `ptsname`). Mode 0o666: the permission
        // is `open`'s family rule
        let slots = (1..crate::io::tty::MAX_TTY).flat_map(|slot| {
            [
                (b"ptm", devfs::DevType::PtyMaster { slot: slot as u8 }, slot),
                (b"pts", devfs::DevType::PtySlave { slot: slot as u8 }, slot),
            ]
        });
        for (prefix, dt, slot) in slots {
            let mut nb = [0u8; 8];
            nb[..3].copy_from_slice(prefix);
            let n = 3 + fmt_u8(&mut nb[3..], slot as u8);
            let Ok(name) = core::str::from_utf8(&nb[..n]) else { continue };
            if let Ok(id) = self.alloc_vnode() {
                self.init_vnode(id, 
                    id as InodeId, dev_id as InodeId, name,
                    VNodeKind::CharDevice, FsType::DevFS,
                    FileMode::new(dt.default_mode()), 0, 0, self.now(),
                );
                self.nodes[id].dev_major = dt.major();
                self.nodes[id].dev_minor = dt.minor();
                if !self.nodes[dev_id].children.insert(name, id as InodeId) {
                    self.free_vnode(id);
                }
            }
        }

        let mut count = 0u8;
        for &(name, dev_type) in devfs::DEV_ENTRIES {
            if let Ok(id) = self.alloc_vnode() {
                self.init_vnode(id, 
                    id as InodeId,
                    dev_id as InodeId,
                    name,
                    VNodeKind::CharDevice,
                    FsType::DevFS,
                    // Per device; see `DevType::default_mode`
                    FileMode::new(dev_type.default_mode()),
                    0,
                    0,
                    self.now(),
                );
                self.nodes[id].dev_major = dev_type.major();
                self.nodes[id].dev_minor = dev_type.minor();

                if self.nodes[dev_id].children.insert(name, id as InodeId) {
                    count += 1;
                } else {
                    self.free_vnode(id);
                }
            }
        }

        crate::serial_println!("[vfs] devfs: {} devices mounted at /dev", count);
    }

    /// Populate /dev with raw block-device nodes once the block layer has
    /// probed its controllers. Run from 'kernel_main' after 'block::probe'
    /// (devfs is mounted earlier, before any device exists). Each registered
    /// device gets /dev/blkN for the whole disk, plus /dev/blkNpM for every
    /// used GPT partition
    pub fn register_block_nodes(&mut self) {
        use crate::fs::devfs::DevType;

        // The four legacy ATA slots register lazily on first use; forced here
        // so their disks appear in /dev too (PCI devices are already up)
        for idx in 0..4 {
            let _ = crate::io::block::register_ata(crate::drivers::block::ata::AtaDrive::from_idx(idx));
        }

        let Some(&dev_dir) = self.nodes[0].children.find_by_name("dev").map(|id| id as usize).as_ref()
        else {
            crate::serial_println!("[vfs] register_block_nodes: no /dev");
            return;
        };

        let mut count = 0u8;
        for dev in 0..crate::fs::vfs::types::MAX_BLOCK_DEVICES as u8 {
            let Some(info) = crate::io::block::info(dev) else { continue };
            if info.total_sectors == 0 && info.model_len == 0 {
                continue;
            }

            // Whole-disk node /dev/blkN (partition 0)
            let mut name = [0u8; 8];
            let nlen = fmt_block_name(&mut name, dev, 0);
            let nstr = core::str::from_utf8(&name[..nlen]).unwrap_or("blk");
            self.add_block_node(dev_dir, nstr, DevType::Block { dev, part: 0 },
                info.total_sectors * 512);
            count += 1;

            // GPT partitions -> /dev/blkNpM, recording each range so byte I/O
            // on the node is clamped to the partition
            if let Ok(tbl) = crate::fs::gpt::gpt_read(dev) {
                for (i, e) in tbl.entries.iter().enumerate() {
                    if !e.is_used() || i >= 15 { continue; }
                    let part = (i + 1) as u8;
                    let sectors = e.size_sectors();
                    crate::io::block::set_partition(dev, i, e.start_lba, sectors);

                    let plen = fmt_block_name(&mut name, dev, part);
                    let pstr = core::str::from_utf8(&name[..plen]).unwrap_or("blk");
                    self.add_block_node(dev_dir, pstr, DevType::Block { dev, part },
                        sectors * 512);
                    count += 1;
                }
            }
        }
        crate::serial_println!("[vfs] devfs: {} block nodes registered in /dev", count);
    }

    /// Re-read one device's GPT and bring its /dev/blkNpM nodes in line with
    /// it: the partprobe path. Existing partition nodes are dropped (unless
    /// held open) and recreated from the fresh table, so partitions added or
    /// removed with the 'gpt' commands appear without a reboot. Returns
    /// '(partitions_now, removed)'
    pub fn rescan_block_partitions(&mut self, dev: u8) -> (usize, usize) {
        use crate::fs::devfs::DevType;

        let Some(&dev_dir) = self.nodes[0].children.find_by_name("dev").map(|id| id as usize).as_ref()
        else {
            return (0, 0);
        };

        // Drop the device's current partition nodes (parts 1..=15); the
        // whole-disk blkN node stays. A node still open is left in place
        let mut removed = 0usize;
        let mut name = [0u8; 8];
        for part in 1..=15u8 {
            let plen = fmt_block_name(&mut name, dev, part);
            let pstr = core::str::from_utf8(&name[..plen]).unwrap_or("");
            if let Some(cid) = self.nodes[dev_dir].children.find_by_name(pstr) {
                let cid = cid as usize;
                if self.nodes[cid].refs() == 0 {
                    self.nodes[dev_dir].children.remove_by_name(pstr);
                    self.free_vnode(cid);
                    removed += 1;
                }
            }
        }
        crate::io::block::clear_partitions(dev);

        // Re-read the GPT and (re)create nodes for every used entry
        let mut now = 0usize;
        if let Ok(tbl) = crate::fs::gpt::gpt_read(dev) {
            for (i, e) in tbl.entries.iter().enumerate() {
                if !e.is_used() || i >= 15 { continue; }
                let part = (i + 1) as u8;
                let sectors = e.size_sectors();
                crate::io::block::set_partition(dev, i, e.start_lba, sectors);

                let plen = fmt_block_name(&mut name, dev, part);
                let pstr = core::str::from_utf8(&name[..plen]).unwrap_or("blk");
                self.add_block_node(dev_dir, pstr, DevType::Block { dev, part },
                    sectors * 512);
                now += 1;
            }
        }
        crate::serial_println!(
            "[vfs] partprobe dev {}: {} partitions ({} stale removed)", dev, now, removed
        );
        (now, removed)
    }

    fn add_block_node(&mut self, dev_dir: usize, name: &str,
        dev_type: crate::fs::devfs::DevType, size: u64)
    {
        // Idempotent: a re-probe must not double-insert
        if self.nodes[dev_dir].children.find_by_name(name).is_some() {
            return;
        }
        let Ok(id) = self.alloc_vnode() else { return };
        self.init_vnode(id, 
            id as InodeId,
            dev_dir as InodeId,
            name,
            VNodeKind::BlockDevice,
            FsType::DevFS,
            // The disk is root's; see `DevType::default_mode`
            FileMode::new(dev_type.default_mode()),
            0,
            0,
            self.now(),
        );
        self.nodes[id].dev_major = dev_type.major();
        self.nodes[id].dev_minor = dev_type.minor();
        self.nodes[id].size = size;
        if !self.nodes[dev_dir].children.insert(name, id as InodeId) {
            self.free_vnode(id);
        }
    }

    /// Every entry here has a size of zero: a procfs file does not exist until
    /// it is read, so `fstat` has no ProcFS branch beside its ext2 one, and
    /// Linux says zero too. A reader reads until EOF and does not size its
    /// buffer from `stat`
    fn mount_procfs(&mut self) {
        crate::trace_println!("[vfs] mounting procfs");

        let proc_id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(e) => {
                crate::serial_println!("[vfs] procfs alloc failed: {:?}", e);
                return;
            }
        };

        self.init_vnode(proc_id, 
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
            self.free_vnode(proc_id);
            return;
        }

        let _ = self.mounts.add(FsType::ProcFS, proc_id as InodeId, 0);

        let mut count = 0u8;
        for &name in procfs::PROC_ENTRIES {
            if let Ok(id) = self.alloc_vnode() {
                self.init_vnode(id, 
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
                    self.free_vnode(id);
                }
            }
        }

        crate::serial_println!("[vfs] procfs: {} entries mounted at /proc", count);
    }
}
