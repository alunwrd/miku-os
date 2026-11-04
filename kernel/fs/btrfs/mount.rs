// btrfs mount state
//
// A table of its own, not a share of ext's. Dispatch goes through
// `fs::registry::with_mounted`; only where the object lives is parallel.
//
// A unified table (`[Option<Box<dyn FileSystem>>; N]` beside `MountEntry`) is
// the end state. It rewrites the path that mounts the root at boot, in
// `fs/ext/mount.rs`, which has no tests.
//
// Read-only: see the module note.

use crate::fs::host::KernelHost;
use crate::fs::vfs::types::{BlockDevId, FsType};
use crate::kcore::sched_lock::SchedMutex;
use crate::{print_error, print_success, serial_println};

/// A mounted btrfs volume
pub type BtrfsVolume = btrfs::Volume<KernelHost>;

/// Two; ext's table is four and mostly empty
pub const MAX_MOUNTS: usize = 2;

pub struct BtrfsState {
    /// `Option`: a `Volume` owns heap (the chunk map), so it cannot be
    /// const-constructed, and an empty volume would not be safe in a slot
    pub slots: [Option<BtrfsVolume>; MAX_MOUNTS],
    pub drive_idx: [usize; MAX_MOUNTS],
    pub start_lba: [u32; MAX_MOUNTS],
    /// The vnode each slot is grafted at, so `umount <path>` can drop one
    pub mount_vnode: [crate::fs::vfs::types::InodeId; MAX_MOUNTS],
}

pub const INVALID_VNODE: crate::fs::vfs::types::InodeId = crate::fs::vfs::types::INVALID_ID;

impl BtrfsState {
    const fn new() -> Self {
        Self {
            slots: [const { None }; MAX_MOUNTS],
            drive_idx: [0; MAX_MOUNTS],
            start_lba: [0; MAX_MOUNTS],
            mount_vnode: [INVALID_VNODE; MAX_MOUNTS],
        }
    }
}

/// A `SchedMutex`, as for the ext table: it is held across whatever it was
/// given, which reads the disk
pub static STATE: SchedMutex<BtrfsState> = SchedMutex::named("btrfs", BtrfsState::new());

/// Run `f` against the volume in `slot`. `None` when the slot is empty
pub fn with_btrfs_for<F, R>(slot: u8, f: F) -> Option<R>
where
    F: FnOnce(&mut BtrfsVolume) -> R,
{
    let mut state = STATE.lock();
    let idx = slot as usize;
    if idx >= MAX_MOUNTS {
        return None;
    }
    state.slots[idx].as_mut().map(f)
}

pub fn is_ready(slot: u8) -> bool {
    let state = STATE.lock();
    (slot as usize) < MAX_MOUNTS && state.slots[slot as usize].is_some()
}

/// Where this slot's filesystem lives, for the shell's mount listing
pub fn slot_info(slot: usize) -> Option<(usize, u32)> {
    let state = STATE.lock();
    if slot >= MAX_MOUNTS || state.slots[slot].is_none() {
        return None;
    }
    Some((state.drive_idx[slot], state.start_lba[slot]))
}

pub fn set_mount_vnode(slot: u8, vnode: crate::fs::vfs::types::InodeId) {
    let mut state = STATE.lock();
    if (slot as usize) < MAX_MOUNTS {
        state.mount_vnode[slot as usize] = vnode;
    }
}

pub fn slot_at_vnode(vnode: crate::fs::vfs::types::InodeId) -> Option<u8> {
    let state = STATE.lock();
    (0..MAX_MOUNTS).find(|&s| state.slots[s].is_some() && state.mount_vnode[s] == vnode)
        .map(|s| s as u8)
}

/// Mount a btrfs volume found on `dev` at `start_lba`.
///
/// Opening reads the superblock and the bootstrap chunk array. The chunk tree
/// is read here too: without it only the system chunks can be reached.
pub fn try_mount_on(dev: BlockDevId, drive_index: usize, start_lba: u32) -> Option<u8> {
    let mut state = STATE.lock();

    for s in 0..MAX_MOUNTS {
        if state.slots[s].is_some()
            && state.drive_idx[s] == drive_index
            && state.start_lba[s] == start_lba
        {
            serial_println!("[btrfs] drive {} lba {} already mounted", drive_index, start_lba);
            return None;
        }
    }
    let Some(slot) = (0..MAX_MOUNTS).find(|&s| state.slots[s].is_none()) else {
        print_error!("  no free btrfs mount slot");
        return None;
    };

    let host = KernelHost::new(dev);
    let mut vol = match btrfs::Volume::open(host, start_lba as u64) {
        Ok(v) => v,
        Err(e) => {
            serial_println!("[btrfs] drive {} lba {}: {:?}", drive_index, start_lba, e);
            return None;
        }
    };
    if let Err(e) = vol.load_chunk_tree() {
        print_error!("  btrfs: the chunk tree would not load: {:?}", e);
        return None;
    }

    let label = vol.label();
    print_success!(
        "  btrfs on drive {} lba {} - {} KiB nodes, label {:?}",
        drive_index,
        start_lba,
        vol.geometry.nodesize / 1024,
        label
    );
    serial_println!(
        "[btrfs] slot {} drive {} lba {} sectorsize {} nodesize {} incompat {:#x}",
        slot, drive_index, start_lba,
        vol.geometry.sectorsize, vol.geometry.nodesize, vol.geometry.incompat
    );

    state.slots[slot] = Some(vol);
    state.drive_idx[slot] = drive_index;
    state.start_lba[slot] = start_lba;
    state.mount_vnode[slot] = INVALID_VNODE;
    Some(slot as u8)
}

/// Drop a mount. There is nothing to flush: this filesystem never writes
pub fn unmount_slot(slot: u8) {
    let mut state = STATE.lock();
    let idx = slot as usize;
    if idx >= MAX_MOUNTS {
        return;
    }
    state.slots[idx] = None;
    state.mount_vnode[idx] = INVALID_VNODE;
    serial_println!("[btrfs] slot {} unmounted", slot);
}

/// The flavour of a slot: `Btrfs` when there is one. On ext three flavours
/// share a driver and the superblock decides
pub fn fs_type_of(slot: u8) -> Option<FsType> {
    is_ready(slot).then_some(FsType::Btrfs)
}

/// btrfs's entry in the probe registry (`kernel/fs/registry.rs`).
///
/// ext's superblock is at byte 1024, btrfs's at byte 65536 with its magic at
/// +64. Where a filesystem keeps its superblock is known only to its prober.
///
/// Reads and judges, and writes nothing: a probe runs over every device and
/// partition on the machine.
pub fn probe_btrfs(drive_index: usize, start_lba: u32) -> Option<crate::fs::registry::FsProbe> {
    use crate::fs::registry::FsProbe;
    use btrfs::structs::{SUPER_INFO_OFFSET, SUPER_INFO_SIZE};
    use fshost::BlockHost;

    let dev = crate::fs::ext::mount::dev_for_idx(drive_index);
    if dev == crate::fs::vfs::types::INVALID_U8 {
        return None;
    }
    let mut host = KernelHost::new(dev);

    // 4096 bytes at byte 65536: eight sectors at sector 128, plus the partition
    // start
    let mut sb = btrfs::Superblock::zeroed();
    let lba = start_lba as u64 + SUPER_INFO_OFFSET / 512;
    if host
        .read_sectors(lba, (SUPER_INFO_SIZE / 512) as u32, &mut sb.data)
        .is_err()
    {
        return None;
    }

    // The full check, not just the magic: a volume refused at mount should not
    // be offered as mountable
    let geo = btrfs::check_superblock(&sb, Some(SUPER_INFO_OFFSET)).ok()?;

    let name = sb.label();
    let mut label = [0u8; 16];
    let label_len = name.len().min(16);
    label[..label_len].copy_from_slice(&name.as_bytes()[..label_len]);

    Some(FsProbe {
        fs_type: FsType::Btrfs,
        drive: drive_index,
        start_lba,
        block_size: geo.sectorsize,
        version: "btrfs",
        label,
        label_len: label_len as u8,
    })
}
