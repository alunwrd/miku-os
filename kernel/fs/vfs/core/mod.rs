// MikuVFS core: global VFS state, the MikuVFS type, and public entry points.
// Method implementations are in the submodules below

pub mod access;
mod attr;
mod backing;
mod bootstrap;
mod cached;
mod dir_ops;
mod file_ops;
pub use file_ops::Closed;
pub(crate) mod fill;
mod links;
mod names;
pub(crate) mod path_resolve;
pub(crate) mod truncate;
mod reclaim;
mod unlinked;
pub mod writeback;
mod vnode_mgmt;
mod watch;

pub use reclaim::{mapping_released, VNODE_SHRINKER};

extern crate alloc;

use crate::fs::vfs::mount::MountTable;
use crate::fs::vfs::pages::PageCache;
use crate::fs::vfs::table::Table;
use crate::fs::vfs::types::*;
use crate::fs::vfs::vnode::VNode;

/// Where every vnode's memory comes from, by name in `/proc/slabinfo`
static VNODE_CACHE: crate::mm::heap::kcache::KCache<VNode> = crate::mm::heap::kcache::KCache::new("vnode");
/// What an id with no vnode reads as (`vfs::table`)
static NO_VNODE: VNode = VNode::empty();
use alloc::collections::BTreeMap;
use spin::Mutex;

// Global lock for the whole filesystem tree.
//
// Not a spin lock: it is held across block-device I/O (read_at_vnode -> ext ->
// disk), where a spinning waiter wastes a full timeslice per CPU and on a
// uniprocessor cannot let the holder run. A read-write lock
// (`dev/vfs-locking.md`, "4c"): `with_vfs` holds it alone, and `with_vfs_read`
// beside other readers for what the cache can answer unchanged
static VFS_LOCK: crate::kcore::sched_lock::SchedRwLock<()> =
    crate::kcore::sched_lock::SchedRwLock::named("vfs", ());

#[repr(C, align(4096))]
struct VfsStorage {
    data: core::mem::MaybeUninit<MikuVFS>,
}

static mut VFS_STORAGE: VfsStorage = VfsStorage {
    data: core::mem::MaybeUninit::uninit(),
};

static VFS_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn init_vfs() -> Result<(), &'static str> {
    unsafe {
        let ptr = VFS_STORAGE.data.as_mut_ptr();

        // Empty: `bootstrap` makes the root first, and it gets id 0
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).nodes), Table::new(&VNODE_CACHE, &NO_VNODE, 0));

        // The page cache is a few words (its pages are frames, `vfs::pages`),
        // so it goes in whole
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).page_cache), PageCache::new());

        core::ptr::write(core::ptr::addr_of_mut!((*ptr).mounts), MountTable::new());
        core::ptr::write(
            core::ptr::addr_of_mut!((*ptr).ctx),
            ProcessContext::root_context(),
        );
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).ext2_mount_active), false);
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).lru), crate::fs::vfs::lru::Lru::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).op), 0);
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).cwds), BTreeMap::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).icache), BTreeMap::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).dcache), names::DcacheStats::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).defer), false);
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).deferred), None);
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).late_truncate), core::cell::Cell::new(None));
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).detached), alloc::vec::Vec::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).yielding), false);
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).gave_way), None);

        let vfs = &mut *ptr;
        vfs.bootstrap();

        VFS_INITIALIZED.store(true, core::sync::atomic::Ordering::Release);
    }

    // The kernel's cache: tmpfs's share of memory
    let (cap, used) = unsafe {
        let vfs = &*VFS_STORAGE.data.as_ptr();
        (vfs.page_cache.total_capacity(), vfs.page_cache.used_pages())
    };
    crate::serial_println!(
        "[vfs] page cache: {} pages x {} B = {} KB ({} in use)",
        cap, PAGE_SIZE, cap * PAGE_SIZE / 1024, used
    );
    crate::serial_println!("[vfs] init done");
    Ok(())
}

/// Register /dev block-device nodes (/dev/blkN, /dev/blkNpM). Call once
/// after 'block::probe', since the block layer must already know its
/// devices and partitions
pub fn register_block_nodes() {
    with_vfs(|vfs| vfs.register_block_nodes());
}

/// Re-read a device's GPT and refresh its /dev/blkNpM nodes (partprobe).
/// Returns '(partitions_now, stale_removed)'
pub fn rescan_block_partitions(dev: u8) -> (usize, usize) {
    with_vfs(|vfs| vfs.rescan_block_partitions(dev))
}

/// Read up to buf.len() bytes from a vnode at a byte offset (no fd). The
/// file-backed mmap page-fault fill path, and the ELF loader's.
///
/// Through the page cache for a file on a disk: a miss reads its pages with the
/// VFS lock released (`cached.rs`), and the read starts over
pub fn read_at_vnode(vid: usize, offset: u64, buf: &mut [u8]) -> Result<usize, crate::fs::vfs::types::VfsError> {
    for _ in 0..FILL_ROUNDS {
        match with_vfs(|vfs| vfs.read_at_step(vid, offset, buf)) {
            cached::ReadStep::Done(r) => return r,
            cached::ReadStep::Fill(plan) => fill_and_insert(plan),
        }
    }
    with_vfs(|vfs| vfs.read_at_vnode(vid, offset, buf))
}

/// `close(2)` of a descriptor the VFS answers: a file's or a directory's
/// beside other readers (`MikuVFS::close_as`); the last reference to an
/// unlinked one, and every other kind, under the lock held alone
pub fn close_fd(fd: usize) -> Result<(), crate::fs::vfs::types::VfsError> {
    use crate::fs::vfs::types::VfsError;
    let quick = with_vfs_read_only(|vfs, _| match vfs.close_as(fd) {
        Err(VfsError::Deferred) => None,
        r => Some(r),
    });
    match quick {
        Some(Ok(Closed::Done)) => Ok(()),
        Some(Ok(Closed::Last(vid))) => {
            with_vfs(|vfs| vfs.drop_ref(vid));
            Ok(())
        }
        Some(Err(e)) => Err(e),
        None => with_vfs(|vfs| vfs.close(fd)),
    }
}

/// `read(2)` on a descriptor the VFS answers: a file, a device node, a
/// directory's error. A file on a disk reads through the page cache, and a miss
/// reads its pages without the VFS lock (`cached.rs`)
pub fn read_fd(fd: usize, buf: &mut [u8]) -> Result<usize, crate::fs::vfs::types::VfsError> {
    // Every page in the cache: no VFS lock at all (`cached::read_hit`). A
    // read longer than one hit pins takes the rest the locked way
    let done = match cached::read_hit(fd, buf) {
        Some(n) if n == 0 || n == buf.len() => return Ok(n),
        Some(n) => n,
        None => 0,
    };
    let rest = &mut buf[done..];
    let r = 'read: {
        // A tmpfs file's pages, beside other readers (`MikuVFS::read_as`)
        if matches!(crate::fs::vfs::fd::peek(fd).map(|f| f.route), Some(crate::fs::vfs::fd::Route::Vfs { cached: false })) {
            if let Some(r) = with_vfs_read_only(|vfs, _| match vfs.read_as(fd, rest) {
                Err(crate::fs::vfs::types::VfsError::Deferred) => None,
                r => Some(r),
            }) {
                break 'read r;
            }
        }
        for _ in 0..FILL_ROUNDS {
            match with_vfs(|vfs| vfs.read_step(fd, rest)) {
                cached::ReadStep::Done(r) => break 'read r,
                cached::ReadStep::Fill(plan) => fill_and_insert(plan),
            }
        }
        with_vfs(|vfs| vfs.read(fd, rest))
    };
    match r {
        Ok(n) => Ok(done + n),
        Err(_) if done > 0 => Ok(done),
        Err(e) => Err(e),
    }
}

/// `lseek`. From the start or from the current position, the descriptor table
/// alone (`dev/vfs-locking.md`, "4a"), since a read loop may seek before every
/// read. From the end needs the size, which is the VFS's
pub fn seek_fd(fd: usize, whence: SeekFrom) -> Result<u64, crate::fs::vfs::types::VfsError> {
    use crate::fs::vfs::types::VfsError;
    // With no lock at all (`fd::seek`)
    let to = |from: u64| -> Result<u64, VfsError> {
        let to = match whence {
            SeekFrom::Start(pos) => pos as i64,
            SeekFrom::Current(delta) => from as i64 + delta,
            SeekFrom::End(_) => unreachable!(),
        };
        if to < 0 {
            return Err(VfsError::SeekError);
        }
        Ok(to as u64)
    };
    match whence {
        SeekFrom::End(_) => with_vfs(|vfs| vfs.seek(fd, whence)),
        _ => crate::fs::vfs::fd::seek(fd, to),
    }
}

/// `f` under the VFS lock, with what it would read of a directory from the disk
/// read with the lock released (`dev/vfs-locking.md`, "4d").
///
/// A name the cache does not have, or a listing of a directory it has not read,
/// is planned, not read (`MikuVFS::defer`): `f` meets `VfsError::Deferred`
/// there and gives up. The lock is released, the plan is read from the
/// filesystem, what it found goes in under the lock if the directory has not
/// changed meanwhile, and `f` runs again from the start. So `f` must change
/// nothing before it walks: `stat`, `access`, `readlink`, `getdents`, an `open`
/// that does not create. Every other call reads under the lock, as does this
/// one after `WALK_STALLS` rounds in a row that put nothing in
#[track_caller]
pub fn with_vfs_walk<R>(f: impl FnMut(&mut MikuVFS) -> R) -> R {
    walk_rounds(f, false)
}

/// `with_vfs_walk` for a call that changes a disk (a create, an `O_TRUNC`, an
/// `unlink`), which walks first and so may defer what the walk misses. One that
/// finds the filesystem's lock taken before it has changed anything gives up
/// (`MikuVFS::give_way`); the lock is waited for with the VFS lock released,
/// and the call runs again. After two of those it waits under the lock. An
/// `unlink` waiting there for a flush's megabyte holds every other VFS call 4
/// ms
#[track_caller]
pub fn with_vfs_disk<R>(f: impl FnMut(&mut MikuVFS) -> R) -> R {
    walk_rounds(f, true)
}

/// The rounds of `with_vfs_walk` and `with_vfs_disk`: `f` under the lock
/// until it neither deferred a read nor gave way
#[track_caller]
fn walk_rounds<R>(mut f: impl FnMut(&mut MikuVFS) -> R, yields: bool) -> R {
    // `walklocked` on the boot line: every walk reads the disk under the lock
    static OFF: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(2);
    let off = match OFF.load(core::sync::atomic::Ordering::Relaxed) {
        2 => {
            let v = crate::arch::x86_64::grub::cmdline_flag("walklocked") as u8;
            OFF.store(v, core::sync::atomic::Ordering::Relaxed);
            v
        }
        v => v,
    };
    let mut stalls = 0;
    let mut gave = 0;
    for _ in 0..WALK_ROUNDS {
        let (r, plan, busy) = with_vfs(|vfs| {
            vfs.defer = off != 1;
            vfs.yielding = yields && gave < 2;
            let r = f(vfs);
            vfs.defer = false;
            vfs.yielding = false;
            (r, vfs.deferred.take(), vfs.gave_way.take())
        });
        if let Some(fs_type) = busy {
            GAVE_WAY.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            crate::fs::registry::wait_idle(fs_type);
            gave += 1;
            continue;
        }
        let Some(mut plan) = plan else { return r };
        fill::fetch(&mut plan);
        // A listing goes in a part at a time, with the lock let go between
        let took = loop {
            if let Some(took) = with_vfs(|vfs| vfs.take_deferred(&mut plan)) {
                break took;
            }
        };
        if took {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls >= WALK_STALLS {
                break;
            }
        }
    }
    with_vfs(f)
}

/// Reads one `with_vfs_walk` makes with the lock released, at most: a path of
/// this many names the cache has never seen
const WALK_ROUNDS: usize = 48;
/// Rounds in a row whose read was thrown away (the directory changed under it)
/// before the walk reads under the lock
const WALK_STALLS: usize = 3;

/// Fills a read may make before it gives up on the cache and reads under the
/// lock, for a cache that keeps losing what was just put in it to other reads
const FILL_ROUNDS: usize = 4;

fn fill_and_insert(plan: cached::FillPlan) {
    let filled = cached::fill(&plan);
    with_vfs(|vfs| vfs.cache_insert(&plan, filled));
}

/// Everything the page cache holds of `vid`'s file goes: for a change made
/// to it below the VFS, by its inode number (`fallocate`, punching a hole)
pub fn drop_file_cache(vid: usize) {
    crate::fs::vfs::filecache::lock().drop_file(vid);
}

/// The disk was changed past the VFS: the kernel shell's ext builtins write,
/// make and remove by inode number. Every file's cached pages go, and what each
/// directory knows about the names it does not have (its negative entries, and
/// being complete, `names.rs`), or a file made that way could not be found by
/// its path
pub fn changed_below_vfs() {
    with_vfs(|vfs| {
        for vid in vfs.nodes.ids() {
            crate::fs::vfs::filecache::lock().drop_file(vid);
            vfs.nodes[vid].attrs_fresh.store(false, core::sync::atomic::Ordering::Relaxed);
            if vfs.nodes[vid].is_dir() && vfs.has_backing(vid) {
                vfs.nodes[vid].children.drop_negatives();
                vfs.nodes[vid].children_loaded = false;
            }
        }
    });
}

/// The cached pages of files on a disk, as a shrinker (`mm::shrink`): every one
/// can be read again, so they go first. An allocation that finds no frames asks
/// too, and from there the cache's lock may be its own caller's. The VFS lock
/// is not needed: the cache has its own (`vfs::filecache`)
pub static FILE_CACHE_SHRINKER: crate::mm::shrink::Shrinker = crate::mm::shrink::Shrinker {
    name: "file pages",
    count: || crate::fs::vfs::filecache::FILE_CACHE.try_lock().map_or(0, |fc| fc.used_pages()),
    scan: |frames, direct| {
        use crate::fs::vfs::filecache::FILE_CACHE;
        if direct {
            FILE_CACHE.try_lock().map_or(0, |mut fc| fc.shrink(frames, usize::MAX))
        } else {
            FILE_CACHE.lock().shrink(frames, usize::MAX)
        }
    },
};

/// `f` under the VFS lock if it is free now, or `None` without waiting: for
/// what memory pressure asks of the VFS, which may come from inside an
/// operation already holding it
#[track_caller]
fn try_vfs<R>(f: impl FnOnce(&mut MikuVFS) -> R) -> Option<R> {
    let _guard = VFS_LOCK.try_lock()?;
    Some(f(get_vfs()))
}

/// Write buf to a vnode at a byte offset (no fd). MAP_SHARED writeback path
pub fn write_at_vnode(vid: usize, offset: u64, buf: &[u8]) -> Result<usize, crate::fs::vfs::types::VfsError> {
    with_vfs(|vfs| vfs.write_at_vnode(vid, offset, buf))
}

/// The frame of a tmpfs file's page at `offset`, with a reference taken for the
/// mapping about to hold it, or `None` when the file is not a tmpfs file and a
/// shared mapping of it has to read a copy (`mm::mmap`)
pub fn shared_frame(vid: usize, offset: u64) -> Option<u64> {
    with_vfs(|vfs| vfs.shared_frame(vid, offset))
}

/// Resolve an open fd in the current process to its (vnode_id, size) so
/// mmap can pin a backing object that outlives the fd
pub fn fd_backing(fd: usize) -> Option<(u32, u64)> {
    with_vfs(|vfs| vfs.fd_backing(fd))
}

fn get_vfs() -> &'static mut MikuVFS {
    if !VFS_INITIALIZED.load(core::sync::atomic::Ordering::Acquire) {
        panic!("[vfs] accessed before init");
    }
    unsafe { &mut *VFS_STORAGE.data.as_mut_ptr() }
}

/// The VFS shared, for readers holding the lock to read: no `&mut` made
fn get_vfs_ref() -> &'static MikuVFS {
    if !VFS_INITIALIZED.load(core::sync::atomic::Ordering::Acquire) {
        panic!("[vfs] accessed before init");
    }
    unsafe { &*VFS_STORAGE.data.as_ptr() }
}

/// `read` with the VFS lock held to read, beside every other reader
/// (`dev/vfs-locking.md`, "4c"). It answers from what is cached, with the
/// caller's own credential, and changes nothing but atomics (`Reader`,
/// `VNode::referenced`, the name-cache counters). What it cannot answer that
/// way (a name only the filesystem knows, a disk file whose attributes have to
/// be asked again) it gives up (`None`, from a `VfsError::Deferred`), and
/// `write` makes the call again with the lock held to write and the disk read
/// with it released (`with_vfs_walk`). `vfsmutex` on the boot line sends every
/// call to `write`
#[track_caller]
pub fn with_vfs_read<R>(
    read: impl FnOnce(&MikuVFS, &crate::fs::vfs::types::Credentials) -> Option<R>,
    write: impl FnMut(&mut MikuVFS) -> R,
) -> R {
    match with_vfs_read_only(read) {
        Some(r) => r,
        None => with_vfs_walk(write),
    }
}

/// `with_vfs_read`'s first half alone: `None` when `read` gave up, or with
/// `vfsmutex` on the boot line, for a caller with a fallback of its own
#[track_caller]
pub fn with_vfs_read_only<R>(
    read: impl FnOnce(&MikuVFS, &crate::fs::vfs::types::Credentials) -> Option<R>,
) -> Option<R> {
    static OFF: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(2);
    let off = match OFF.load(core::sync::atomic::Ordering::Relaxed) {
        2 => {
            let v = crate::arch::x86_64::grub::cmdline_flag("vfsmutex") as u8;
            OFF.store(v, core::sync::atomic::Ordering::Relaxed);
            v
        }
        v => v,
    };
    if off != 0 {
        return None;
    }
    let cred = crate::sched::current_cred();
    let _guard = VFS_LOCK.read();
    read(get_vfs_ref(), &cred)
}

/// The task holding the VFS lock to write, 0 for none: for the count of disk
/// reads made under it (`io::block`, `/proc/lockstat`)
static VFS_WRITER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Whether the calling task holds the VFS lock to write
pub fn writer_is_current() -> bool {
    let w = VFS_WRITER.load(core::sync::atomic::Ordering::Relaxed);
    w != 0 && w == crate::sched::current_pid()
}

/// How many `MikuVFS::detached` holds, read without the lock
static DETACHED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// tmpfs pages released under the lock, freed a part at a time with the lock
/// taken for each part and none of `with_vfs`'s epilogue, which would come back
/// here
fn reap_detached() {
    use core::sync::atomic::Ordering;
    while DETACHED.load(Ordering::Acquire) > 0 {
        let _guard = VFS_LOCK.lock();
        let vfs = get_vfs();
        let MikuVFS { detached, page_cache, .. } = vfs;
        let Some(d) = detached.last_mut() else { return };
        if d.free_part(page_cache, crate::fs::vfs::address_space::FREE_PART) {
            detached.pop();
            DETACHED.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Calls that gave way to a filesystem's lock (`with_vfs_disk`), and calls
/// that found it taken and waited under the VFS lock all the same: for
/// `/proc/lockstat`
pub static GAVE_WAY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static WAITED_UNDER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

impl MikuVFS {
    /// Whether a call run by `with_vfs_disk` gives up here instead of waiting
    /// under the VFS lock for the lock of `id`'s filesystem, which is taken.
    /// Asked before the call has changed anything; it then returns `Deferred`,
    /// and the filesystem is noted for the wait
    pub fn give_way(&mut self, id: usize) -> bool {
        let Some((fs_type, _, _)) = self.backing_of(id) else { return false };
        if !crate::fs::registry::busy(fs_type) {
            return false;
        }
        if !self.yielding {
            WAITED_UNDER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return false;
        }
        self.gave_way = Some(fs_type);
        true
    }

    /// `id`'s tmpfs pages off it: freed now, or after the lock for a big
    /// file (`reap_detached`)
    pub(super) fn detach_pages(&mut self, id: usize) {
        let space = self.nodes[id].addr_space;
        if space.nr_pages as usize <= crate::fs::vfs::address_space::FREE_PART {
            let mut space = space;
            for pid in space.truncate(&mut self.page_cache, 0) {
                self.page_cache.free_page(pid);
            }
            self.nodes[id].addr_space = space;
            return;
        }
        self.nodes[id].addr_space = crate::fs::vfs::address_space::AddressSpace::new();
        self.detached.push(crate::fs::vfs::address_space::Detached::new(space, &self.page_cache));
        DETACHED.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
    }
}

#[track_caller]
pub fn with_vfs<F, R>(f: F) -> R
where
    F: FnOnce(&mut MikuVFS) -> R,
{
    let (r, late) = {
        let _guard = VFS_LOCK.lock();
        VFS_WRITER.store(crate::sched::current_pid(), core::sync::atomic::Ordering::Relaxed);
        let vfs = get_vfs();
        sync_ctx_from_current(vfs);
        vfs.op += 1;
        // Unlinked files whose last mapping went while nobody held the lock
        vfs.release_unmapped();
        let r = f(vfs);
        // ext's lock before the VFS's goes: whatever reaches that disk after
        // comes after the truncate (`truncate::LateTruncate`)
        let late = vfs.late_truncate.take().map(|t| (t, crate::fs::ext::mount::STATE.lock()));
        VFS_WRITER.store(0, core::sync::atomic::Ordering::Relaxed);
        (r, late)
    };
    if let Some((t, mut state)) = late {
        t.finish(&mut state);
    }
    // A big file released under the lock gives its blocks back after it, and
    // its pages
    unlinked::drain();
    crate::fs::vfs::filecache::reap();
    reap_detached();
    // An inotify event raised under the lock wakes its reader after it, and a
    // terminal whose last master went is hung up after it: the scheduler is
    // not entered underneath the filesystem's lock
    crate::fs::vfs::notify::wake_if_raised();
    crate::io::tty::run_hangups();
    r
}

#[track_caller]
pub fn with_vfs_ro<F, R>(f: F) -> R
where
    F: FnOnce(&MikuVFS) -> R,
{
    let _guard = VFS_LOCK.lock();
    let vfs = get_vfs();
    sync_ctx_from_current(vfs);
    f(vfs)
}

// Copy the current process's credential into 'vfs.ctx', so MikuVFS code reading
// 'self.ctx.umask' or 'self.ctx.cred.X' sees the calling process's view. cwd is
// owned by the Process struct and looked up via 'sched::current_cwd()';
// vfs.ctx.cwd is kept in sync here too. The whole credential is assigned, so a
// field added to Cred cannot be forgotten
fn sync_ctx_from_current(vfs: &mut MikuVFS) {
    let cred = crate::sched::current_cred();
    vfs.ctx.cred  = cred;
    vfs.ctx.umask = cred.umask;
    vfs.ctx.pid   = crate::sched::current_pid() as u16;
    vfs.ctx.cwd   = crate::sched::current_cwd() as InodeId;
}

pub struct MikuVFS {
    /// Every vnode, by id, as many as are cached (`vfs::table`)
    pub nodes: Table<VNode>,
    pub page_cache: PageCache,
    pub mounts: MountTable,
    pub ctx: ProcessContext,
    pub ext2_mount_active: bool,
    /// The vnodes reclaim could give back, most recently used first
    /// (`vfs::lru`)
    pub lru: crate::fs::vfs::lru::Lru,
    /// `with_vfs` calls so far: the clock `VNode::used` is stamped with
    pub op: u64,
    /// The working directory each thread group holds a reference on, by
    /// tgid (`change_cwd`). The process's own `cwd` is what lookups use; this
    /// is which vnode its reference is on, so the reference can be given back
    pub cwds: BTreeMap<u64, InodeId>,
    /// (mount, inode number) to the vnode caching it, for the files on a
    /// disk with more than one name (`names.rs`)
    pub icache: BTreeMap<(u16, u32), InodeId>,
    /// What the name cache answered, for `/proc/stat` (`names.rs`)
    pub dcache: names::DcacheStats,
    /// Set by `with_vfs_walk` for the length of its closure: a name or a
    /// listing the cache does not have is planned (`deferred`) instead of
    /// read from the disk under the lock (`fill.rs`)
    pub defer: bool,
    pub deferred: Option<fill::Deferred>,
    /// The disk's half of an `O_TRUNC`, for `with_vfs` to finish after the
    /// lock (`truncate::LateTruncate`). A `Cell` so the paths to a disk can
    /// finish it first from `&self`; only a writer ever holds one, and it is
    /// gone before the lock is
    pub late_truncate: core::cell::Cell<Option<truncate::LateTruncate>>,
    /// tmpfs files' pages taken off them to be freed after the lock
    /// (`reap_detached`), as the page cache's are (`filecache::reap`)
    pub detached: alloc::vec::Vec<crate::fs::vfs::address_space::Detached>,
    /// Set by `with_vfs_disk` for a call that may give up rather than wait
    /// for a filesystem's lock under this one (`give_way`)
    pub yielding: bool,
    /// The filesystem a call gave way to
    pub gave_way: Option<FsType>,
}

impl MikuVFS {
    /// The calling thread group's descriptor table, held (`vfs::fd`). Its lock
    /// is a leaf: copy out what is needed and drop the guard before anything
    /// else that might want it
    #[inline]
    pub fn fds(&self) -> crate::fs::vfs::fd::Fds {
        crate::fs::vfs::fd::current()
    }

    /// Thread group `pid`'s table, held, made empty if it has none
    #[inline]
    pub fn fds_for(&self, pid: u64) -> crate::fs::vfs::fd::Fds {
        crate::fs::vfs::fd::table(pid)
    }

    /// Clone parent's table into the child on fork(). Each open descriptor in
    /// the parent has a sibling in the child pointing at the same vnode, so the
    /// vnode refcounts are bumped to keep drop_fds()'s dec_ref symmetric on
    /// exit
    pub fn fork_fds(&mut self, parent_pid: u64, child_pid: u64) {
        for vid in crate::fs::vfs::fd::fork(parent_pid, child_pid) {
            let idx = vid as usize;
            if self.valid_vnode(idx) {
                self.nodes[idx].inc_ref();
            }
        }
        // The child's working directory is the parent's, and holds its own
        // reference on it (`change_cwd`)
        if let Some(&cwd) = self.cwds.get(&parent_pid) {
            self.nodes[cwd as usize].inc_ref();
            self.cwds.insert(child_pid, cwd);
        }
    }

    /// Drop a process's FD table at reap time. Each open descriptor dec_refs
    /// its vnode, as 'close(fd)' does. Returns the released vnode ids for any
    /// deferred-free cleanup. The inline unlink-on-close path in 'close()' is
    /// not duplicated here: reaping happens after the process is dead, and the
    /// on-disk delete falls to the ext layer's orphan inode logic
    pub fn drop_fds(&mut self, pid: u64) -> alloc::vec::Vec<crate::fs::vfs::InodeId> {
        let mut victims = alloc::vec::Vec::new();
        if let Some(vids) = crate::fs::vfs::fd::remove(pid) {
            victims.extend(vids);
        }
        // The working directory's reference goes with the descriptors'
        if let Some(cwd) = self.cwds.remove(&pid) {
            victims.push(cwd);
        }
        victims
    }

    /// Close every descriptor flagged O_CLOEXEC in the calling process's table.
    /// Returns the released vnode ids for the standard dec_ref + deferred-free
    /// cleanup, as for 'close()'
    pub fn close_cloexec_fds(&mut self) -> alloc::vec::Vec<crate::fs::vfs::InodeId> {
        let key = crate::sched::current_tgid();
        let victims = crate::fs::vfs::fd::table(key).close_cloexec();
        // A close like any other, so the record locks on these go too
        for &v in &victims {
            crate::fs::vfs::lock::release(key, v);
        }
        victims
    }
}

/// Release a process's descriptors as soon as it exits, without waiting for the
/// parent to reap it.
///
/// A zombie owns only its exit code. A pipe reader decides EOF from whether any
/// writer remains, so a writer exited but not reaped would keep the pipe open
/// while the parent, waiting for the reader, could not reap it: `a | b` would
/// deadlock
pub fn release_fds_on_exit(pid: u64) {
    let victims = with_vfs(|vfs| vfs.drop_fds(pid));
    if victims.is_empty() {
        return;
    }
    // Each drop is a `dec_ref`, which also hangs up a terminal whose last
    // master this process held (`io::tty::master_unref`) and counts a pipe's
    // end down (`vfs::pipe::ref_changed`). The keys first: a last descriptor
    // takes its end of the pipe with it
    let pipes: alloc::vec::Vec<u64> = with_vfs(|vfs| {
        let pipes = victims.iter().map(|&v| v as usize)
            .filter(|&v| crate::fs::vfs::pipe::is_end(v))
            .map(crate::fs::vfs::pipe::wait_key)
            .collect();
        for vid in victims {
            vfs.drop_ref(vid as usize);
        }
        pipes
    });
    // Woken after the counts went down: a reader woken first still counts this
    // process among the writers and sleeps again. A reader of a pipe this
    // process was the last writer of has its end of file now; a writer whose
    // last reader this was, its EPIPE; a poller of either, its hangup
    for key in pipes {
        crate::sched::wait::wake_key(key);
    }
    crate::sched::wait::PIPE.wake_all();
    crate::sched::wait::wake_pollers();
}

/// `fd`'s vnode and route, from the descriptor table alone, except on the first
/// lookup, when the route is worked out under the VFS lock and kept on the
/// descriptor if it still names that vnode (`dev/vfs-locking.md`, "4a")
pub fn route(fd: usize) -> Option<(usize, crate::fs::vfs::fd::Route)> {
    use crate::fs::vfs::fd::{self, Route};
    let (vid, route) = fd::peek(fd).map(|f| (f.vnode_id as usize, f.route))?;
    if route != Route::Unknown {
        return Some((vid, route));
    }
    let route = with_vfs(|vfs| vfs.route_of(vid));
    if let Ok(mut f) = fd::current().get_mut(fd) {
        if f.vnode_id as usize == vid {
            f.route = route;
        }
    }
    Some((vid, route))
}

impl MikuVFS {
    /// What the syscall layer does with a descriptor on `vid`, kept on the
    /// descriptor after the first look (`vfs::fd::Route`)
    pub fn route_of(&self, vid: usize) -> crate::fs::vfs::fd::Route {
        use crate::fs::devfs::DevType;
        use crate::fs::vfs::fd::Route;
        if !self.valid_vnode(vid) {
            return Route::Vfs { cached: false };
        }
        if crate::fs::vfs::pipe::is_end(vid) {
            return Route::Pipe;
        }
        if self.nodes[vid].is_notify_instance() {
            return Route::Inotify;
        }
        match crate::fs::devfs::dev_type_from_node(self.nodes[vid].dev_major, self.nodes[vid].dev_minor) {
            Some(DevType::NetRaw) => Route::NetRaw,
            Some(DevType::Tty) => Route::Tty,
            Some(DevType::Console) => Route::Console,
            Some(DevType::PtySlave { slot }) => Route::PtySlave(slot as u32),
            Some(DevType::PtyMaster { slot }) => Route::PtyMaster(slot as u32),
            _ => Route::Vfs { cached: self.cacheable(vid) },
        }
    }
}

// Pipe support: the ring is `vfs::pipe`; these are the descriptor questions,
// which need the table

pub use crate::fs::vfs::pipe::PipeRead;

/// Non-blocking attempt to consume from a pipe. Never sleeps and takes no VFS
/// lock: the bytes are behind the pipe's own (`vfs::pipe`). The waiting is the
/// caller's job, as is waking the writers this made room for
pub fn pipe_try_read(vnode_id: usize, buf: &mut [u8]) -> PipeRead {
    crate::fs::vfs::pipe::try_read(vnode_id, buf)
}

// Socket nodes
//
// A local socket is reached by a name, and the name lives here, not in a
// private table inside the socket layer, so it can be listed, diagnosed and
// cleaned up like any other file.

/// Create a node that names a local socket, in `dir`, called `name`
///
/// Fails if something is already there: a stale name is an error, not a silent
/// hijack of another's address
pub fn create_socket_node(dir: &str, name: &str) -> Result<usize, ()> {
    with_vfs(|vfs| {
        let parent = vfs.resolve_path(0, dir).map_err(|_| ())?;
        let vid = vfs
            .create_file(parent, name, crate::fs::vfs::types::FileMode::default_pipe())
            .map_err(|_| ())?;
        vfs.nodes[vid].kind = crate::fs::vfs::types::VNodeKind::Socket;
        Ok(vid)
    })
}

/// The node behind `path`, if it is a socket
pub fn socket_node_at(path: &str) -> Option<usize> {
    with_vfs(|vfs| {
        let vid = vfs.resolve_path(0, path).ok()?;
        if vfs.valid_vnode(vid) && vfs.nodes[vid].kind == crate::fs::vfs::types::VNodeKind::Socket {
            Some(vid)
        } else {
            None
        }
    })
}

/// Remove a name, ignoring whether it was there
pub fn unlink_path(dir: &str, name: &str) {
    with_vfs(|vfs| {
        if let Ok(parent) = vfs.resolve_path(0, dir) {
            let _ = vfs.unlink(parent, name);
        }
    });
}

/// Is there something to read on this pipe, and has the last writer gone?
///
/// Non-consuming, unlike `pipe_try_read`: otherwise the read following a poll
/// finds an empty pipe and blocks.
///
/// Hangup is reported separately from readability: a pipe with bytes left in it
/// and no writers is both, and a reader drains it first
pub fn pipe_poll(vnode_id: usize) -> (bool, bool) {
    let (readable, _, hup) = crate::fs::vfs::pipe::poll(vnode_id);
    (readable, hup)
}

pub fn pipe_vnode_of_fd(fd: usize) -> Option<usize> {
    with_vfs(|vfs| {
        let vid = vfs.fds().get(fd).ok()?.vnode_id as usize;
        (vfs.valid_vnode(vid) && crate::fs::vfs::pipe::is_end(vid)).then_some(vid)
    })
}


/// Whether the calling process may execute the program at `path`.
///
/// A free function: exec() reaches the VFS through `vfs_read::read_file`, which
/// returns bytes and checks no permission. The check is in do_exec, before the
/// image is built
pub fn may_exec_path(path: &str) -> Result<(), crate::fs::vfs::types::VfsError> {
    with_vfs(|vfs| {
        let cwd = crate::sched::current_cwd() as usize;
        let id  = vfs.resolve_path_follow(cwd, path)?;
        if vfs.nodes[id].is_dir() {
            return Err(crate::fs::vfs::types::VfsError::IsDirectory);
        }
        vfs.check_exec_node(id)
    })
}


/// May the calling process read what is at `path`?
///
/// The in-kernel shell's builtins resolve a path and then read the bytes; `cat`
/// goes straight to the ext inode. Resolution checks search permission on every
/// directory it walks, not read permission on the thing at the end.
///
/// This is the check `open` makes, for callers not going through `open`. A
/// directory is the same question: listing one is reading it
pub fn may_read_path(cwd: usize, path: &str) -> Result<(), crate::fs::vfs::types::VfsError> {
    with_vfs(|vfs| {
        let id = vfs.resolve_path_follow(cwd, path)?;
        vfs.check_access(id, crate::fs::vfs::types::OpenFlags(crate::fs::vfs::types::OpenFlags::READ))
    })
}
