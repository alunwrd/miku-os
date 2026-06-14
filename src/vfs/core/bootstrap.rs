// Root filesystem initialization and standard mounts:
// devfs, procfs, /lib (system libraries), /mnt

use super::MikuVFS;
use crate::vfs::devfs;
use crate::vfs::procfs;
use crate::vfs::types::*;

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

    pub static LIBS: &[SysLib] = &[SysLib {
        dir: "lib",
        name: "libmiku.so",
        data: include_bytes!("../../lib/libmiku/libmiku.so"),
    }];
}

impl MikuVFS {
    pub(super) fn bootstrap(&mut self) {
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
            let dir_id = match self.lookup_or_create_lib_dir(lib.dir) {
                Some(id) => id,
                None => continue,
            };

            if self.install_syslib_file(dir_id, lib).is_some() {
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
                if c < MAX_VNODES && self.nodes[c].active {
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
        self.nodes[id].init(
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
            self.nodes[id].active = false;
            return None;
        }
        Some(id)
    }

    fn install_syslib_file(&mut self, dir_id: usize, lib: &syslibs::SysLib) -> Option<usize> {
        let file_id = self.alloc_vnode().ok()?;
        let ts = self.now();
        self.nodes[file_id].init(
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

        if !self.write_pages_from_slice(file_id, lib.data) {
            crate::serial_println!("[vfs] syslib write failed: {}", lib.name);
            self.nodes[file_id].active = false;
            return None;
        }

        self.nodes[file_id].size = lib.data.len() as u64;
        self.nodes[file_id].flags.immutable = true;

        if !self.nodes[dir_id]
            .children
            .insert(lib.name, file_id as InodeId)
        {
            self.nodes[file_id].active = false;
            return None;
        }

        crate::serial_println!(
            "[vfs] syslib: /{}/{} vnode={} {} bytes (immutable)",
            lib.dir,
            lib.name,
            file_id,
            lib.data.len()
        );
        Some(file_id)
    }

    fn write_pages_from_slice(&mut self, file_id: usize, data: &[u8]) -> bool {
        let mut offset = 0usize;
        while offset < data.len() {
            let page_num = offset / PAGE_SIZE;
            let page_off = offset % PAGE_SIZE;
            let chunk = (PAGE_SIZE - page_off).min(data.len() - offset);

            let pid = match self.nodes[file_id].addr_space.get_page(page_num) {
                Some(pid) => pid,
                None => {
                    let pid = match self.page_cache.alloc_page() {
                        Ok(pid) => pid,
                        Err(_) => return false,
                    };
                    if self.nodes[file_id]
                        .addr_space
                        .set_page(page_num, pid)
                        .is_err()
                    {
                        return false;
                    }
                    pid
                }
            };

            match self.page_cache.get_page_data_mut(pid) {
                Some(page) => page[page_off..page_off + chunk]
                    .copy_from_slice(&data[offset..offset + chunk]),
                None => return false,
            }
            offset += chunk;
        }
        true
    }

    fn create_mnt(&mut self) {
        let mnt_id = match self.alloc_vnode() {
            Ok(id) => id,
            Err(_) => return,
        };
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

    /// Populate /dev with raw block-device nodes once the block layer has
    /// probed its controllers. Run from 'kernel_main' after 'block::probe'
    /// (devfs itself is mounted earlier, before any device exists). Each
    /// registered device gets /dev/blkN for the whole disk, plus
    /// /dev/blkNpM for every used GPT partition
    pub fn register_block_nodes(&mut self) {
        use crate::vfs::devfs::DevType;

        // The four legacy ATA slots register lazily on first use; force them
        // in so their disks appear in /dev too (PCI devices are already up)
        for idx in 0..4 {
            let _ = crate::block::register_ata(crate::ata::AtaDrive::from_idx(idx));
        }

        let Some(&dev_dir) = self.nodes[0].children.find_by_name("dev").map(|id| id as usize).as_ref()
        else {
            crate::serial_println!("[vfs] register_block_nodes: no /dev");
            return;
        };

        let mut count = 0u8;
        for dev in 0..crate::vfs::types::MAX_BLOCK_DEVICES as u8 {
            let Some(info) = crate::block::info(dev) else { continue };
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

            // GPT partitions -> /dev/blkNpM, recording each range so byte
            // I/O on the node is clamped to the partition
            if let Ok(tbl) = crate::gpt::gpt_read(dev) {
                for (i, e) in tbl.entries.iter().enumerate() {
                    if !e.is_used() || i >= 15 { continue; }
                    let part = (i + 1) as u8;
                    let sectors = e.size_sectors();
                    crate::block::set_partition(dev, i, e.start_lba, sectors);

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
    /// it - the partprobe path. Existing partition nodes are dropped (unless
    /// held open) and recreated from the fresh table, so partitions added or
    /// removed with the 'gpt' commands appear without a reboot. Returns
    /// '(partitions_now, removed)'
    pub fn rescan_block_partitions(&mut self, dev: u8) -> (usize, usize) {
        use crate::vfs::devfs::DevType;

        let Some(&dev_dir) = self.nodes[0].children.find_by_name("dev").map(|id| id as usize).as_ref()
        else {
            return (0, 0);
        };

        // Drop the device's current partition nodes (parts 1..=15); the
        // whole-disk blkN node stays. A node still open by someone is left in
        // place to avoid pulling the rug out from under it
        let mut removed = 0usize;
        let mut name = [0u8; 8];
        for part in 1..=15u8 {
            let plen = fmt_block_name(&mut name, dev, part);
            let pstr = core::str::from_utf8(&name[..plen]).unwrap_or("");
            if let Some(cid) = self.nodes[dev_dir].children.find_by_name(pstr) {
                let cid = cid as usize;
                if self.nodes[cid].refcount == 0 {
                    self.nodes[dev_dir].children.remove_by_name(pstr);
                    self.nodes[cid].active = false;
                    removed += 1;
                }
            }
        }
        crate::block::clear_partitions(dev);

        // Re-read the GPT and (re)create nodes for every used entry
        let mut now = 0usize;
        if let Ok(tbl) = crate::gpt::gpt_read(dev) {
            for (i, e) in tbl.entries.iter().enumerate() {
                if !e.is_used() || i >= 15 { continue; }
                let part = (i + 1) as u8;
                let sectors = e.size_sectors();
                crate::block::set_partition(dev, i, e.start_lba, sectors);

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
        dev_type: crate::vfs::devfs::DevType, size: u64)
    {
        // Idempotent: a re-probe must not double-insert
        if self.nodes[dev_dir].children.find_by_name(name).is_some() {
            return;
        }
        let Ok(id) = self.alloc_vnode() else { return };
        self.nodes[id].init(
            id as InodeId,
            dev_dir as InodeId,
            name,
            VNodeKind::BlockDevice,
            FsType::DevFS,
            FileMode::default_dev(),
            0,
            0,
            self.now(),
        );
        self.nodes[id].dev_major = dev_type.major();
        self.nodes[id].dev_minor = dev_type.minor();
        self.nodes[id].size = size;
        if !self.nodes[dev_dir].children.insert(name, id as InodeId) {
            self.nodes[id].active = false;
        }
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
}
