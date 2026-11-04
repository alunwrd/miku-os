//  ext2/ext3/ext4 mount state
//
// The VFS, the syscall layer and the firmware loader read and write the mounted
// filesystem through this table; the shell is one of its users

use crate::drivers::block::ata::AtaDrive;
pub use crate::fs::registry::FsProbe;
use crate::fs::vfs::types::FsType;
use crate::fs::ext::ext3::journal::DEFAULT_JOURNAL_BLOCKS;
use crate::fs::host::KernelHost;
use crate::fs::ext::structs::*;
use crate::fs::ext::mount_check::{MountError, MountReject};
use crate::fs::ext::{DiskReader, MikuFS};
use crate::{print_error, print_success, println, serial_println};
use alloc::vec::Vec;
use spin::Mutex;

pub const MAX_MOUNTS: usize = 4;

/// What an unused mount slot holds.
///
/// The table is a fixed array of filesystem handles with a `ready` flag, so
/// every slot needs a value. Its device id is the invalid one, so operations on
/// an unready slot fail at the block layer
const EMPTY_FS: MikuFS = MikuFS::with_reader(DiskReader::new_read_only(
    KernelHost::invalid(),
    0,
));

pub struct ExtFsState {
    pub slots:       [MikuFS; MAX_MOUNTS],
    pub ready:       [bool; MAX_MOUNTS],
    pub drive_idx:   [usize; MAX_MOUNTS],
    pub start_lba:   [u32; MAX_MOUNTS],
    pub active_slot: usize,
    /// VFS vnode id of each slot's mountpoint, or INVALID_VNODE when the slot
    /// is mounted at the disk layer but not attached to the VFS. For an umount
    /// of a specific path, not of active_slot
    pub mount_vnode: [crate::fs::vfs::types::InodeId; MAX_MOUNTS],
}

pub const INVALID_VNODE: crate::fs::vfs::types::InodeId = crate::fs::vfs::types::INVALID_ID;

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

/// The mount table.
///
/// A `SchedMutex`, like `VFS_LOCK`: `with_ext2_for` holds this across whatever
/// it was given, which reads and writes the disk. A spinning waiter burns a
/// full timeslice per CPU, and on a uniprocessor cannot let the holder finish
/// its I/O
pub static STATE: crate::kcore::sched_lock::SchedMutex<ExtFsState> =
    crate::kcore::sched_lock::SchedMutex::named("ext", ExtFsState::new());


/// Work on one mounted filesystem by slot number, not the active one
///
/// An install deals with two: the boot medium it reads and the disk it writes.
/// Switching the active slot between chunks would move it under anything else
/// running
pub fn with_ext2_slot<F, R>(slot: usize, f: F) -> Option<R>
where
    F: FnOnce(&mut MikuFS) -> R,
{
    let mut state = STATE.lock();
    if slot >= MAX_MOUNTS || !state.ready[slot] {
        return None;
    }
    Some(f(&mut state.slots[slot]))
}

/// True when that slot is mounted and refuses writes
pub fn slot_read_only(slot: usize) -> bool {
    let state = STATE.lock();
    slot < MAX_MOUNTS && state.ready[slot] && state.slots[slot].reader.read_only
}

/// True when that slot will accept a write.
///
/// Not the negation of `slot_read_only`: an unmounted slot is neither read-only
/// nor writable, and a caller asking "may I write here" must be told no.
/// `slot_read_only` answers false for an empty slot.
pub fn slot_writable(slot: usize) -> bool {
    let state = STATE.lock();
    slot < MAX_MOUNTS && state.ready[slot] && !state.slots[slot].reader.read_only
}

/// Which drive a mounted slot came from, for a caller that has a slot and
/// needs to know which disk it must not touch
pub fn slot_drive(slot: usize) -> Option<usize> {
    let state = STATE.lock();
    if slot < MAX_MOUNTS && state.ready[slot] {
        Some(state.drive_idx[slot])
    } else {
        None
    }
}

pub fn is_ext2_ready() -> bool {
    let state = STATE.lock();
    state.ready[state.active_slot]
}

pub fn active_slot_index() -> usize {
    STATE.lock().active_slot
}

/// True when the active ext mount refuses writes. Boot-time mounts are
/// read-only, so neither the ring-3 shell exec'd off the disk nor an atime
/// stamp can dirty the medium before someone asks
pub fn active_read_only() -> bool {
    let state = STATE.lock();
    let slot = state.active_slot;
    state.ready[slot] && state.slots[slot].reader.read_only
}

/// Promote a mounted slot to read-write. Journal replay, orphan cleanup and
/// stamping the superblock mount state are all deferred to this point, so a
/// read-only boot leaves the disk byte-for-byte untouched
pub fn remount_rw(slot: usize) -> Result<(), &'static str> {
    if slot >= MAX_MOUNTS { return Err("no such slot"); }
    let mut state = STATE.lock();
    if !state.ready[slot] { return Err("slot is empty"); }
    if !state.slots[slot].reader.read_only { return Err("already read-write"); }

    state.slots[slot].reader.read_only = false;

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
    match state.slots[slot].cleanup_orphans() {
        Ok(0) => {}
        Ok(n) => serial_println!("[mount] cleaned {} orphan inodes", n),
        Err(e) => serial_println!("[mount] orphan cleanup failed: {:?}", e),
    }
    state.slots[slot].update_mount_state();
    let _ = state.slots[slot].flush_all_dirty_metadata();
    serial_println!("[miku_extfs] slot {} remounted read-write", slot);
    Ok(())
}

/// Drop a slot back to read-only. Flushes whatever is dirty first, and marks
/// the filesystem cleanly unmounted so a later fsck does not see it as dirty
pub fn remount_ro(slot: usize) -> Result<(), &'static str> {
    if slot >= MAX_MOUNTS { return Err("no such slot"); }
    let mut state = STATE.lock();
    if !state.ready[slot] { return Err("slot is empty"); }
    if state.slots[slot].reader.read_only { return Err("already read-only"); }

    let _ = state.slots[slot].finish_unmount();
    state.slots[slot].reader.read_only = true;
    serial_println!("[miku_extfs] slot {} remounted read-only", slot);
    Ok(())
}

/// The flavour of the filesystem in one slot.
///
/// `None` when that slot holds nothing. A vnode knows its slot, so this is the
/// call to use: with the firmware store mounted first and the root disk second,
/// the active slot at graft time is the ramdisk, not the root
pub fn fs_type_of(slot: u8) -> Option<crate::fs::vfs::types::FsType> {
    use crate::fs::vfs::types::FsType;
    let state = STATE.lock();
    let slot = slot as usize;
    if slot >= MAX_MOUNTS || !state.ready[slot] {
        return None;
    }
    let sb = &state.slots[slot].superblock;
    Some(if sb.is_ext4() {
        FsType::Ext4
    } else if sb.is_ext3() {
        FsType::Ext3
    } else {
        FsType::Ext2
    })
}

/// The same, as the short name the shell and the mount log print
pub fn fs_version_tag_of(slot: u8) -> &'static str {
    let state = STATE.lock();
    let slot = slot as usize;
    if slot >= MAX_MOUNTS || !state.ready[slot] {
        return "ext";
    }
    state.slots[slot].superblock.fs_version_str()
}

/// The flavour of whichever filesystem is active.
///
/// Correct only with exactly one mounted, as for `with_ext2_pub`. Anything with
/// a vnode uses `fs_type_of(vnode.ext2_slot)`
pub fn active_fs_type() -> crate::fs::vfs::types::FsType {
    let slot = STATE.lock().active_slot as u8;
    fs_type_of(slot).unwrap_or(crate::fs::vfs::types::FsType::Ext2)
}

pub fn ext_fs_version_tag() -> &'static str {
    let slot = STATE.lock().active_slot as u8;
    fs_version_tag_of(slot)
}

/// Work on whichever filesystem is active.
///
/// Correct only when there is exactly one; see `with_ext2_for`
pub fn with_ext2_pub<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut MikuFS) -> R,
{
    STATE.lock().active_fs().map(f)
}


/// Make every mounted filesystem durable. Returns the number synced, and the
/// first error if any
pub fn sync_all_mounts() -> Result<usize, crate::fs::ext::FsError> {
    let mut state = STATE.lock();
    let mut synced = 0;
    let mut first_err = None;
    for slot in 0..MAX_MOUNTS {
        if !state.ready[slot] || state.slots[slot].reader.read_only {
            continue;
        }
        match state.slots[slot].sync() {
            Ok(()) => synced += 1,
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(synced),
    }
}

/// The journal's periodic commit and checkpoint, on every mounted filesystem:
/// the commit interval belongs to each, and a mount nothing has looked at
/// recently has the oldest open transaction. bdflush's, every two seconds, so
/// no operation closing a five-second-old transaction pays for the commit under
/// the VFS lock
pub fn periodic_sync() {
    for_each_mount(|fs| fs.check_periodic_sync());
}

/// Run `f` on every mounted filesystem. The commit timer ticks on all of them,
/// not only the active one
pub fn for_each_mount<F>(mut f: F)
where
    F: FnMut(&mut MikuFS),
{
    let mut state = STATE.lock();
    for slot in 0..MAX_MOUNTS {
        if state.ready[slot] {
            f(&mut state.slots[slot]);
        }
    }
}

/// Work on the filesystem a particular vnode belongs to.
///
/// `slot` is the vnode's `ext2_slot`. `INVALID_U8` means a vnode no mount
/// attributed, and falls back to the active slot.
///
/// An inode number means nothing without its inode table: resolved against the
/// wrong filesystem it reads a different file
pub fn with_ext2_for<F, R>(slot: u8, f: F) -> Option<R>
where
    F: FnOnce(&mut MikuFS) -> R,
{
    let mut state = STATE.lock();
    let idx = if (slot as usize) < MAX_MOUNTS && state.ready[slot as usize] {
        slot as usize
    } else {
        state.active_slot
    };
    if !state.ready[idx] {
        return None;
    }
    Some(f(&mut state.slots[idx]))
}

pub fn invalidate_vfs_ext_mounts() {
    let mut dropped_any = false;

    crate::fs::vfs::core::with_vfs(|vfs| {
        for id in vfs.nodes.ids() {
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
            vfs.nodes[id].ext2_slot = crate::fs::vfs::types::INVALID_U8;
            vfs.nodes[id].children_loaded = false;
            dropped_any = true;
        }

        if dropped_any {
            vfs.ext2_mount_active = false;
        }
    });

    if dropped_any {
        // Anyone holding a cwd inside the vanished subtree has to be told. The
        // filesystem publishes the event and interested layers subscribe
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
pub fn register_mount_vnode(slot: usize, vnode: crate::fs::vfs::types::InodeId) {
    if slot >= MAX_MOUNTS { return; }
    let mut state = STATE.lock();
    state.mount_vnode[slot] = vnode;
}

/// Reverse lookup: given a VFS vnode id, return the ext slot whose mount
/// root is that vnode, or None if the vnode is not a known mountpoint
pub fn slot_for_vnode(vnode: crate::fs::vfs::types::InodeId) -> Option<usize> {
    let state = STATE.lock();
    for s in 0..MAX_MOUNTS {
        if state.ready[s] && state.mount_vnode[s] == vnode {
            return Some(s);
        }
    }
    None
}

/// Tear down a specific ext slot. Unlike force_unmount (always active_slot),
/// this lets umount <path> drop only the slot owning that path, leaving sibling
/// mounts intact
///
/// The caller has already evicted the corresponding VFS subtree
pub fn unmount_slot(slot: usize) {
    if slot >= MAX_MOUNTS { return; }
    let mut state = STATE.lock();
    if !state.ready[slot] { return; }

    // A read-only slot has nothing dirty and no mount state to clear
    if !state.slots[slot].reader.read_only {
        let _ = state.slots[slot].finish_unmount();
    }
    state.ready[slot] = false;
    state.slots[slot].block_cache = None;
    state.slots[slot].journal_inode_cached = None;
    state.mount_vnode[slot] = INVALID_VNODE;

    // If the active slot was dropped, fail over to any other ready slot, so ext
    // commands keep working without an explicit fs.select
    if state.active_slot == slot {
        for s in 0..MAX_MOUNTS {
            if state.ready[s] {
                state.active_slot = s;
                break;
            }
        }
    }
}

/// Snapshot every ext slot with a VFS mount attached. Returns up to MAX_MOUNTS
/// (slot, vnode_id, fs_version_tag) tuples; slots mounted at the disk layer but
/// not grafted onto the VFS are skipped
pub fn mounted_slots_snapshot() -> [Option<(usize, crate::fs::vfs::types::InodeId, &'static str)>; MAX_MOUNTS] {
    let mut out: [Option<(usize, crate::fs::vfs::types::InodeId, &'static str)>; MAX_MOUNTS] = [None; MAX_MOUNTS];
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

/// Every ext filesystem this machine can see, whole-disk and in-partition
/// alike, without mounting any of them. Reads superblocks; writes nothing
pub fn scan_volumes() -> Vec<FsProbe> {
    let mut found = Vec::new();

    for drive in 0..crate::fs::vfs::types::MAX_BLOCK_DEVICES {
        let dev = dev_for_idx(drive);

        // Ask before reading. An empty IDE slot answers every read with a fault
        // the block layer retries twice, at a hardware timeout each: most of a
        // second per absent drive. A device that never answered IDENTIFY, or
        // answered with no sectors, has nothing to probe, as in
        // gpt_probe_sectors
        match crate::io::block::info(dev) {
            Some(i) if i.total_sectors > 0 => {}
            _ => continue,
        }

        if let Some(p) = crate::fs::registry::probe(drive, 0) {
            found.push(p);
        }

        // A filesystem laid down by 'install' is inside a GPT partition, so LBA
        // 0 alone would miss it
        if let Ok(table) = crate::fs::gpt::gpt_read(dev) {
            for e in table.entries.iter() {
                if !e.is_used() || e.is_swap() || e.start_lba == 0 {
                    continue;
                }
                let lba = match u32::try_from(e.start_lba) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(p) = crate::fs::registry::probe(drive, lba) {
                    found.push(p);
                }
            }
        }
    }

    found
}

/// ext's entry in the probe registry (`kernel/fs/registry.rs`).
///
/// The superblock is at byte 1024, so sector 2
pub fn probe_ext(drive_index: usize, start_lba: u32) -> Option<FsProbe> {
    let mut reader = DiskReader::new_partitioned(KernelHost::new(dev_for_idx(drive_index)), start_lba);
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

    let name = sb.volume_name();
    let mut label = [0u8; 16];
    let label_len = name.len().min(16);
    label[..label_len].copy_from_slice(&name.as_bytes()[..label_len]);

    Some(FsProbe {
        fs_type: if sb.is_ext4() {
            FsType::Ext4
        } else if sb.is_ext3() {
            FsType::Ext3
        } else {
            FsType::Ext2
        },
        drive: drive_index,
        start_lba,
        block_size,
        version: sb.fs_version_str(),
        label,
        label_len: label_len as u8,
    })
}

/// What the boot line asked for as a root
enum RootSpec {
    /// 'root=<drive>[:<lba>]': the disk by its number on this machine
    Numeric { drive: usize, lba: u32 },
    /// 'root=LABEL=<name>': the disk by what is written inside it, which
    /// survives a move to another port
    Label { name: [u8; 16], len: usize },
}

/// Parse 'root=' off the boot command line. There is no default: a physical
/// disk is touched only when the boot line names it, by number or by label
fn root_from_cmdline() -> Option<RootSpec> {
    let mut buf = [0u8; 64];
    let n = crate::arch::x86_64::grub::cmdline_arg("root", &mut buf)?;
    let val = core::str::from_utf8(&buf[..n]).ok()?;

    if let Some(rest) = val.strip_prefix("LABEL=") {
        // The superblock holds 16 bytes, so a longer name could never match;
        // refused, not truncated
        if rest.is_empty() || rest.len() > 16 {
            serial_println!(
                "[miku_extfs] root=LABEL= wants 1 to 16 characters, got {}", rest.len()
            );
            return None;
        }
        let mut name = [0u8; 16];
        name[..rest.len()].copy_from_slice(rest.as_bytes());
        return Some(RootSpec::Label { name, len: rest.len() });
    }

    let (drive_str, lba_str) = match val.split_once(':') {
        Some((d, l)) => (d, l),
        None => (val, ""),
    };
    let drive: usize = drive_str.parse().ok()?;
    if drive >= crate::fs::vfs::types::MAX_BLOCK_DEVICES {
        return None;
    }
    let lba: u32 = if lba_str.is_empty() { 0 } else { lba_str.parse().ok()? };
    Some(RootSpec::Numeric { drive, lba })
}

/// Why the boot line's root was not mounted, kept for the console.
///
/// `mount_root_disk` runs long before the console exists, when a refusal
/// reaches only a serial cable
static ROOT_TROUBLE: Mutex<Vec<alloc::string::String>> = Mutex::new(Vec::new());

fn trouble(line: alloc::string::String) {
    serial_println!("{}", line);
    ROOT_TROUBLE.lock().push(line);
}

/// Hand over what went wrong with the root, once. The shell prints it under
/// its banner
pub fn take_root_trouble() -> Vec<alloc::string::String> {
    core::mem::take(&mut *ROOT_TROUBLE.lock())
}

/// Resolve 'root=LABEL=<name>' to the one volume that answers to it.
///
/// One match is the boot line naming a disk. None and several are both
/// refusals: picking one of two disks called 'mikuos' would be auto-selection
fn root_from_label(name: &str) -> Option<(usize, u32)> {
    let all = scan_volumes();
    let mut hits = all.iter().filter(|p| p.label_str() == name);

    let first = match hits.next() {
        Some(p) => p,
        None => {
            trouble(alloc::format!("  no filesystem here is labelled '{}'", name));
            if all.is_empty() {
                trouble(alloc::format!("  no ext filesystem found on any drive at all"));
            } else {
                trouble(alloc::format!("  what is here:"));
                for p in &all {
                    trouble(alloc::format!(
                        "    drive {}:{} - {} labelled '{}'",
                        p.drive, p.start_lba, p.version, p.label_str()
                    ));
                }
            }
            trouble(alloc::format!(
                "  edit root= at the GRUB menu, or mount one with 'ext2mount <drive> <lba>'"
            ));
            return None;
        }
    };

    if let Some(second) = hits.next() {
        trouble(alloc::format!(
            "  more than one filesystem is labelled '{}' - not guessing:", name
        ));
        trouble(alloc::format!("    drive {}:{}", first.drive, first.start_lba));
        trouble(alloc::format!("    drive {}:{}", second.drive, second.start_lba));
        for p in hits {
            trouble(alloc::format!("    drive {}:{}", p.drive, p.start_lba));
        }
        trouble(alloc::format!(
            "  boot the 'root by number' menu entry, or edit root= at the GRUB menu"
        ));
        return None;
    }

    Some((first.drive, first.start_lba))
}

/// Bring up the root filesystem at boot.
///
/// Two rules. Nothing is auto-selected: a physical disk is mounted only when
/// the boot line says 'root=<drive>'. Nothing is mounted read-write: mounting
/// rw stamps the superblock mount counter, replays the journal and cleans
/// orphans, and reading a file off it (exec of /bin/mikudo) stamps atime.
/// 'fs.rw' opts in
pub fn mount_root_disk() -> bool {
    // The firmware GRUB module is RAM-backed, the boot medium and not a user's
    // disk, so it comes up without being named. Read-only as well: writes to it
    // are lost at reboot
    if let Some((start, end)) = crate::arch::x86_64::grub::module("firmware") {
        let dev = crate::io::block::register_ramdisk(start, end - start);
        if dev != crate::fs::vfs::types::INVALID_U8 && mount_dev_ro(dev) {
            serial_println!(
                "[miku_extfs] firmware store mounted read-only from GRUB module (dev {})", dev
            );
            return true;
        }
        serial_println!("[miku_extfs] firmware GRUB module present but ext mount failed");
    }

    mount_root_from_cmdline()
}

/// Mount whatever `root=` on the boot line names.
///
/// For two cases: no GRUB module, so this is the only root; and a module
/// supplying /lib/firmware with the boot line asking for a persistent /bin and
/// /etc. Nothing is mounted unless `root=` names it, and nothing is writable
/// unless `rootrw` says so
pub fn mount_root_from_cmdline() -> bool {
    match root_from_cmdline() {
        Some(spec) => {
            let (drive, lba) = match spec {
                RootSpec::Numeric { drive, lba } => {
                    serial_println!("[miku_extfs] boot line asks for root={}:{}", drive, lba);
                    (drive, lba)
                }
                RootSpec::Label { name, len } => {
                    let name = match core::str::from_utf8(&name[..len]) {
                        Ok(s) => s,
                        Err(_) => return false,
                    };
                    serial_println!("[miku_extfs] boot line asks for root=LABEL={}", name);
                    match root_from_label(name) {
                        Some(found) => {
                            serial_println!(
                                "[miku_extfs] label '{}' -> drive {} lba {}",
                                name, found.0, found.1
                            );
                            found
                        }
                        None => return false,
                    }
                }
            };
            if STATE.lock().is_already_mounted(drive, lba) {
                return true;
            }
            // 'rootrw' next to 'root=' is the boot-line spelling of fs.rw
            let rw = crate::arch::x86_64::grub::cmdline_flag("rootrw");
            serial_println!(
                "[miku_extfs] mounting drive {} lba {} {}",
                drive, lba, if rw { "read-write" } else { "read-only" }
            );
            if rw { try_mount(drive, lba) } else { try_mount_ro(drive, lba) }
        }
        None => {
            serial_println!(
                "[miku_extfs] no root= on the boot line; no disk mounted automatically \
                 (use 'ext2mount <drive>' to pick one)"
            );
            false
        }
    }
}

pub fn try_mount(drive_index: usize, start_lba: u32) -> bool {
    try_mount_on(dev_for_idx(drive_index), drive_index, start_lba, false)
}

/// Same, mounted read-only: no write reaches the medium until 'remount_rw'
pub fn try_mount_ro(drive_index: usize, start_lba: u32) -> bool {
    try_mount_on(dev_for_idx(drive_index), drive_index, start_lba, true)
}

/// Mount an ext filesystem directly from a registered block-device id, keying
/// the mount bookkeeping on 'dev'. Used by the firmware loader for the
/// RAM-backed GRUB-module store, whose id is dynamically registered, not a
/// fixed 0-3 ATA slot. No-op if already mounted
pub fn mount_dev(dev: crate::fs::vfs::types::BlockDevId) -> bool {
    if STATE.lock().is_already_mounted(dev as usize, 0) {
        return true;
    }
    try_mount_on(dev, dev as usize, 0, false)
}

/// Read-only variant of 'mount_dev', used by the boot path
pub fn mount_dev_ro(dev: crate::fs::vfs::types::BlockDevId) -> bool {
    if STATE.lock().is_already_mounted(dev as usize, 0) {
        return true;
    }
    try_mount_on(dev, dev as usize, 0, true)
}

/// Core mount path: read the superblock + group-descriptor table from an
/// already-resolved block device 'dev', recording the mount under the
/// bookkeeping key 'drive_index'/'start_lba'.
fn try_mount_on(
    dev: crate::fs::vfs::types::BlockDevId,
    drive_index: usize,
    start_lba: u32,
    read_only: bool,
) -> bool {
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

    // Read, validate and bring up the filesystem. The checks that keep a
    // corrupted or foreign volume from panicking this kernel are in the crate,
    // under `cargo test` against hostile images
    let fs = match MikuFS::open(KernelHost::new(dev), start_lba, read_only) {
        Ok(fs) => fs,
        Err(MountError::Io(_)) => {
            serial_println!(
                "[miku_extfs] drive {} lba {} - cannot read the superblock",
                drive_index, start_lba
            );
            return false;
        }
        Err(MountError::Rejected(why)) => {
            match why {
                MountReject::BadMagic(m) => serial_println!(
                    "[miku_extfs] drive {} lba {} - bad magic 0x{:04X}, skip",
                    drive_index, start_lba, m
                ),
                MountReject::UnsupportedIncompat(bits) => print_error!(
                    "  miku_extfs: volume uses incompatible features 0x{:X} this driver does \
                     not implement - refusing to mount rather than misread it",
                    bits
                ),
                other => print_error!(
                    "  miku_extfs: unusable superblock on drive {} lba {}: {:?}",
                    drive_index, start_lba, other
                ),
            }
            return false;
        }
    };

    state.slots[slot] = fs;

    // Log the flavour the superblock describes (fs_version_str)
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

    let block_size   = state.slots[slot].block_size;
    let blocks_count = state.slots[slot].superblock.blocks_count();
    let group_count  = state.slots[slot].group_count;

    // A volume with unimplemented read-only-compat features comes up read-only
    // whether or not asked: readable, but writing would leave structures we do
    // not maintain inconsistent
    let forced_ro = state.slots[slot].reader.read_only;
    if forced_ro && !read_only {
        print_error!(
            "  miku_extfs: volume uses read-only features this driver does not maintain \
             - mounted read-only"
        );
    }

    state.ready[slot]     = true;
    state.drive_idx[slot] = drive_index;
    state.start_lba[slot] = start_lba;
    state.active_slot     = slot;

    let _ = state.slots[slot].warm_cache();

    // Journal replay, orphan cleanup and the mount-state stamp all write. A
    // read-only mount defers them to 'remount_rw', so mounting alone never
    // touches the medium
    if forced_ro {
        serial_println!(
            "[miku_extfs] slot {} mounted read-only (no journal replay, no orphan cleanup, \
             no mount-state stamp) - 'fs.rw {}' to opt in",
            slot, slot
        );
    } else {
        match state.slots[slot].activate() {
            Ok((0, 0)) => {}
            Ok((replayed, orphans)) => serial_println!(
                "[miku_extfs] slot {}: replayed {} journal blocks, cleaned {} orphan inodes",
                slot, replayed, orphans
            ),
            Err(e) => serial_println!("[miku_extfs] slot {} activation failed: {:?}", slot, e),
        }
    }

    let total_inodes = state.slots[slot].superblock.inodes_count();
    let free_blocks  = state.slots[slot].superblock.free_blocks_count();
    let free_inodes  = state.slots[slot].superblock.free_inodes_count();
    let version      = state.slots[slot].superblock.fs_version_str();

    let mode = if read_only { "read-only" } else { "read-write" };
    print_success!(
        "  {} mounted {} -> slot {} (drive {} lba={})",
        version, mode, slot, drive_index, start_lba
    );
    println!("  Block:   {} bytes", block_size);
    println!("  Blocks:  {} total, {} free", blocks_count, free_blocks);
    println!("  Inodes:  {} total, {} free", total_inodes, free_inodes);
    println!("  Groups:  {}", group_count);
    println!("  Cache:   enabled");
    println!("  Use 'fs.select <0|1>' to switch slots");
    true
}

/// Graft a top-level directory of the mounted root filesystem onto the VFS at
/// the same name (e.g. disk:/bin -> /bin). The root disk is otherwise reachable
/// only through the firmware graft, and ring-3 binaries in /bin would be
/// invisible to 'resolve_path'. Returns false when the mount or the directory
/// is missing, which callers treat as "not provisioned"
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
