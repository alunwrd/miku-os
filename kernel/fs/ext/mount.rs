//  ext2/ext3/ext4 mount state
// 
//  This used to live in shell/commands/ext2_cmds.rs, which meant the VFS,
//  the syscall layer and the firmware loader all had to reach up into a shell
//  command module to read or write the mounted filesystem - 54 references
//  from 'fs', 'syscall', 'net' and 'kcore' into 'crate::shell'. The mount
//  table belongs to the filesystem layer; the shell is just one of its users

use crate::drivers::block::ata::AtaDrive;
use crate::fs::ext::ext3::journal::{TxnTag, DEFAULT_JOURNAL_BLOCKS};
use crate::fs::ext::reader::DiskReader;
use crate::fs::ext::structs::*;
use crate::fs::ext::MikuFS;
use crate::{print_error, print_success, println, serial_println};
use alloc::vec::Vec;
use spin::Mutex;

pub const MAX_MOUNTS: usize = 4;

const EMPTY_FS: MikuFS = MikuFS {
    superblock:       Superblock { data: [0; 1024] },
    block_size:       0,
    inodes_per_group: 0,
    blocks_per_group: 0,
    group_count:      0,
    groups:           Vec::new(),
    reader: DiskReader {
        dev_id:    crate::fs::vfs::types::INVALID_U8,
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
    groups_dirty:     Vec::new(),
    last_sync_ticks:  0,
    journal_inode_cached: None,
    alloc_hint: Vec::new(),
};

pub struct ExtFsState {
    pub slots:       [MikuFS; MAX_MOUNTS],
    pub ready:       [bool; MAX_MOUNTS],
    pub drive_idx:   [usize; MAX_MOUNTS],
    pub start_lba:   [u32; MAX_MOUNTS],
    pub active_slot: usize,
    /// VFS vnode id of each slot's mountpoint, or INVALID_VNODE when the
    /// slot is mounted at the disk layer but not yet attached to the VFS via
    /// mount. Lets us umount a specific path instead of always tearing
    /// down active_slot
    pub mount_vnode: [u16; MAX_MOUNTS],
}

pub const INVALID_VNODE: u16 = u16::MAX;

impl ExtFsState {
    const fn new() -> Self {
        Self {
            slots:       [EMPTY_FS; MAX_MOUNTS],
            ready:       [false; MAX_MOUNTS],
            drive_idx:   [0; MAX_MOUNTS],
            start_lba:   [0; MAX_MOUNTS],
            active_slot: 0,
            mount_vnode: [INVALID_VNODE; MAX_MOUNTS],
        }
    }

    pub fn active_fs(&mut self) -> Option<&mut MikuFS> {
        let slot = self.active_slot;
        if self.ready[slot] { Some(&mut self.slots[slot]) } else { None }
    }

    pub fn find_free_slot(&self) -> Option<usize> {
        self.ready.iter().position(|&r| !r)
    }

    pub fn is_already_mounted(&self, drive: usize, lba: u32) -> bool {
        for i in 0..MAX_MOUNTS {
            if self.ready[i] && self.drive_idx[i] == drive && self.start_lba[i] == lba {
                return true;
            }
        }
        false
    }
}

pub static STATE: Mutex<ExtFsState> = Mutex::new(ExtFsState::new());

pub fn with_ext2<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut MikuFS) -> R,
{
    STATE.lock().active_fs().map(f)
}

pub fn is_ext2_ready() -> bool {
    let state = STATE.lock();
    state.ready[state.active_slot]
}

pub fn active_slot_index() -> usize {
    STATE.lock().active_slot
}

pub fn active_fs_type() -> crate::fs::vfs::types::FsType {
    let state = STATE.lock();
    let slot = state.active_slot;
    if !state.ready[slot] {
        return crate::fs::vfs::types::FsType::Ext2;
    }
    match state.slots[slot].superblock.fs_version_str() {
        "ext4" => crate::fs::vfs::types::FsType::Ext4,
        "ext3" => crate::fs::vfs::types::FsType::Ext3,
        _      => crate::fs::vfs::types::FsType::Ext2,
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

pub fn invalidate_vfs_ext_mounts() {
    let mut dropped_any = false;

    crate::fs::vfs::core::with_vfs(|vfs| {
        for id in 0..crate::fs::vfs::MAX_VNODES {
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
            vfs.nodes[id].fs_type = crate::fs::vfs::FsType::TmpFS;
            vfs.nodes[id].ext2_ino = 0;
            vfs.nodes[id].children_loaded = false;
            dropped_any = true;
        }

        if dropped_any {
            vfs.ext2_mount_active = false;
        }
    });

    if dropped_any {
        // Anyone holding a cwd inside the vanished subtree has to be told.
        // The filesystem does not get to know that a shell exists, so it
        // publishes the event and interested layers subscribe
        if let Some(hook) = *UNMOUNT_HOOK.lock() {
            hook();
        }
    }
}

/// Called after an ext subtree is dropped from the VFS. Registered by
/// whoever caches directory state across calls (the shell keeps a cwd)
static UNMOUNT_HOOK: Mutex<Option<fn()>> = Mutex::new(None);

pub fn set_unmount_hook(hook: fn()) {
    *UNMOUNT_HOOK.lock() = Some(hook);
}

pub fn force_unmount() {
    let mut state = STATE.lock();
    let slot = state.active_slot;
    state.ready[slot] = false;
    state.slots[slot].block_cache = None;
    state.slots[slot].journal_inode_cached = None;
    state.mount_vnode[slot] = INVALID_VNODE;
    drop(state);
    invalidate_vfs_ext_mounts();
}

/// Record which VFS vnode is the root of slot's mount. Called by the
/// mount shell command after it grafts an ext-family slot onto a VFS
/// directory.
pub fn register_mount_vnode(slot: usize, vnode: u16) {
    if slot >= MAX_MOUNTS { return; }
    let mut state = STATE.lock();
    state.mount_vnode[slot] = vnode;
}

/// Reverse lookup: given a VFS vnode id, return the ext slot whose mount
/// root is that vnode, or None if the vnode is not a known mountpoint
pub fn slot_for_vnode(vnode: u16) -> Option<usize> {
    let state = STATE.lock();
    for s in 0..MAX_MOUNTS {
        if state.ready[s] && state.mount_vnode[s] == vnode {
            return Some(s);
        }
    }
    None
}

/// Tear down a specific ext slot. Unlike force_unmount (which always
/// targets active_slot), this lets umount <path> drop only the slot
/// owning that path, leaving any sibling mounts intact
///
/// Caller must already have evicted the corresponding VFS subtree
pub fn unmount_slot(slot: usize) {
    if slot >= MAX_MOUNTS { return; }
    let mut state = STATE.lock();
    if !state.ready[slot] { return; }

    let _ = state.slots[slot].sync();
    state.slots[slot].mark_clean_unmount();
    let _ = state.slots[slot].flush_all_dirty_metadata();
    state.ready[slot] = false;
    state.slots[slot].block_cache = None;
    state.slots[slot].journal_inode_cached = None;
    state.mount_vnode[slot] = INVALID_VNODE;

    // If we just dropped the active slot, fail over to any other ready slot
    // so subsequent ext commands keep working without an explicit fs.select
    if state.active_slot == slot {
        for s in 0..MAX_MOUNTS {
            if state.ready[s] {
                state.active_slot = s;
                break;
            }
        }
    }
}

/// Snapshot every ext slot that currently has a VFS mount attached
/// Returns up to MAX_MOUNTS (slot, vnode_id, fs_version_tag) tuples;
/// slots that are mounted at the disk layer but not yet grafted onto the
/// VFS are skipped
pub fn mounted_slots_snapshot() -> [Option<(usize, u16, &'static str)>; MAX_MOUNTS] {
    let mut out: [Option<(usize, u16, &'static str)>; MAX_MOUNTS] = [None; MAX_MOUNTS];
    let state = STATE.lock();
    for s in 0..MAX_MOUNTS {
        if state.ready[s] && state.mount_vnode[s] != INVALID_VNODE {
            out[s] = Some((
                s,
                state.mount_vnode[s],
                state.slots[s].superblock.fs_version_str(),
            ));
        }
    }
    out
}

pub fn dev_for_idx(idx: usize) -> crate::fs::vfs::types::BlockDevId {
    if idx < 4 {
        crate::io::block::register_ata(AtaDrive::from_idx(idx))
    } else {
        crate::io::block::probe();
        idx as crate::fs::vfs::types::BlockDevId
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
            state.mount_vnode[i] = INVALID_VNODE;
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

pub struct ExtProbe {
    pub drive: usize,
    pub block_size: u32,
    pub fs_version: &'static str,
}

pub fn probe_drive(drive_index: usize, start_lba: u32) -> Option<ExtProbe> {
    let mut reader = DiskReader::from_dev(dev_for_idx(drive_index), start_lba);
    let mut sector = [0u8; 512];
    if reader.read_sector(2, &mut sector).is_err() {
        return None;
    }
    let magic_lo = u16::from_le_bytes([sector[56], sector[57]]);
    if magic_lo != EXT2_MAGIC {
        return None;
    }
    let log_bs = u32::from_le_bytes([sector[24], sector[25], sector[26], sector[27]]);
    if log_bs > 6 {
        return None;
    }
    let block_size = 1024u32 << log_bs;

    let mut sector2 = [0u8; 512];
    if reader.read_sector(3, &mut sector2).is_err() {
        return None;
    }
    let mut sb = Superblock { data: [0u8; 1024] };
    sb.data[0..512].copy_from_slice(&sector);
    sb.data[512..1024].copy_from_slice(&sector2);

    Some(ExtProbe {
        drive: drive_index,
        block_size,
        fs_version: sb.fs_version_str(),
    })
}

pub fn mount_root_disk() -> bool {
    // Prefer a firmware GRUB module (RAM-backed): a single boot medium then
    // carries the kernel and /lib/firmware with no second disk. Fall back to
    // ATA drive 1 (the QEMU disk.img layout) when no module is present.
    if let Some((start, end)) = crate::arch::x86_64::grub::module("firmware") {
        let dev = crate::io::block::register_ramdisk(start, end - start);
        if dev != crate::fs::vfs::types::INVALID_U8 && mount_dev(dev) {
            serial_println!(
                "[miku_extfs] firmware store mounted from GRUB module (dev {})", dev
            );
            return true;
        }
        serial_println!(
            "[miku_extfs] firmware GRUB module present but ext mount failed; trying ATA drive 1"
        );
    }
    if STATE.lock().is_already_mounted(1, 0) {
        return true;
    }
    try_mount(1, 0)
}

pub fn try_mount(drive_index: usize, start_lba: u32) -> bool {
    try_mount_on(dev_for_idx(drive_index), drive_index, start_lba)
}

/// Mount an ext filesystem directly from a registered block-device id, keying
/// the mount bookkeeping on 'dev'. Used by the firmware loader to mount the
/// RAM-backed GRUB-module store, whose id is a dynamically-registered device
/// rather than a fixed 0-3 ATA slot. No-ops if already mounted
pub fn mount_dev(dev: crate::fs::vfs::types::BlockDevId) -> bool {
    if STATE.lock().is_already_mounted(dev as usize, 0) {
        return true;
    }
    try_mount_on(dev, dev as usize, 0)
}

/// Core mount path: read the superblock + group-descriptor table from an
/// already-resolved block device 'dev', recording the mount under the
/// bookkeeping key 'drive_index'/'start_lba'.
fn try_mount_on(dev: crate::fs::vfs::types::BlockDevId, drive_index: usize, start_lba: u32) -> bool {
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

    state.ready[slot] = false;
    state.slots[slot].reader = DiskReader::from_dev(dev, start_lba);
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

    // Report the flavour the superblock actually describes. The driver has
    // always distinguished ext2/ext3/ext4 internally (fs_version_str), but
    // every log line said "ext2", so an ext4 root looked like an ext2 one
    {
        let sb = &state.slots[slot].superblock;
        serial_println!(
            "[miku_extfs] slot {} drive {} lba {} - {} (journal={} extents={} 64bit={} \
             metadata_csum={} flex_bg={} dir_index={})",
            slot, drive_index, start_lba,
            sb.fs_version_str(),
            sb.has_journal(), sb.has_extents(), sb.has_64bit(),
            sb.has_metadata_csum(), sb.has_flex_bg(), sb.has_dir_index()
        );
    }

    let block_size       = state.slots[slot].superblock.block_size();
    let inodes_per_group = state.slots[slot].superblock.inodes_per_group();
    let blocks_per_group = state.slots[slot].superblock.blocks_per_group();
    let blocks_count     = state.slots[slot].superblock.blocks_count();
    let first_data_block = state.slots[slot].superblock.first_data_block();
    let usable           = blocks_count.saturating_sub(first_data_block);
    let group_count      = if blocks_per_group == 0 { 0 }
        else { (usable + blocks_per_group - 1) / blocks_per_group };
    let gd_size          = state.slots[slot].superblock.group_desc_size() as usize;

    // Sanity bound only - the group tables are heap-allocated now, so volume
    // size is limited by the filesystem, not by a fixed array. The old
    // [GroupDesc; 32] rejected anything past 32 groups, i.e. 4 GiB at a 4 KiB
    // block size and 256 MiB at 1 KiB
    const MAX_BLOCK_GROUPS: u32 = 1 << 20; // 128 TiB at 4 KiB blocks
    if group_count > MAX_BLOCK_GROUPS {
        print_error!(
            "  miku_extfs: implausible block group count ({}), refusing to mount",
            group_count
        );
        return false;
    }

    state.slots[slot].block_size       = block_size;
    state.slots[slot].inodes_per_group = inodes_per_group;
    state.slots[slot].blocks_per_group = blocks_per_group;
    state.slots[slot].group_count      = group_count;

    let gc = group_count as usize;
    state.slots[slot].groups.clear();
    state.slots[slot].groups.resize(gc, GroupDesc { data: [0; 64] });
    state.slots[slot].groups_dirty.clear();
    state.slots[slot].groups_dirty.resize(gc, false);
    state.slots[slot].alloc_hint.clear();
    state.slots[slot].alloc_hint.resize(gc, 0);

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

/// Graft a top-level directory of the mounted root filesystem onto the VFS
/// at the same name (e.g. disk:/bin -> /bin). The root disk is otherwise
/// only reachable through the firmware graft, so ring-3 binaries living in
/// /bin were invisible to 'resolve_path'. Returns false when the mount or
/// the directory is missing - callers treat that as "not provisioned"
pub fn graft_root_dir(name: &str) -> bool {
    let mut abs = [0u8; 64];
    abs[0] = b'/';
    let n = core::cmp::min(name.len(), abs.len() - 1);
    abs[1..1 + n].copy_from_slice(&name.as_bytes()[..n]);
    let abs_path = core::str::from_utf8(&abs[..1 + n]).unwrap_or("/");

    let dir_ino = with_ext2_pub(|fs| fs.resolve_path(abs_path).ok()).flatten();
    let dir_ino = match dir_ino {
        Some(i) => i,
        None => return false,
    };
    let ok = crate::fs::vfs::core::with_vfs(|v| v.graft_ext_dir("/", name, dir_ino).is_ok());
    if ok {
        serial_println!("[mount] {} grafted from root disk", abs_path);
    } else {
        serial_println!("[mount] failed to graft {}", abs_path);
    }
    ok
}
