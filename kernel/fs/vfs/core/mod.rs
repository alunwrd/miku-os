// MikuVFS core module
// Global VFS state, MikuVFS type, and public entry points.
// Method implementations are split across the thematic submodules below

mod access;
mod attr;
mod bootstrap;
mod dir_ops;
mod ext2_bridge;
mod file_ops;
mod links;
mod path_resolve;
mod truncate;
mod vnode_mgmt;

extern crate alloc;

use crate::fs::vfs::fd::FdTable;
use crate::fs::vfs::lru::LruList;
use crate::fs::vfs::mount::MountTable;
use crate::fs::vfs::pages::{CachedPage, PageCache};
use crate::fs::vfs::slab::Slab;
use crate::fs::vfs::types::*;
use crate::fs::vfs::vnode::VNode;
use alloc::collections::BTreeMap;
use spin::Mutex;

// Global lock - serializes all VFS operations.
//
// Still one lock for the whole filesystem, but no longer a spin lock: it is
// routinely held across block-device I/O (read_at_vnode -> ext -> disk), and
// a spinning waiter there wastes a full timeslice per CPU and cannot even let
// the holder run on a uniprocessor. SchedMutex spins briefly, then yields
static VFS_LOCK: crate::kcore::sched_lock::SchedMutex<()> =
    crate::kcore::sched_lock::SchedMutex::new(());

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

        let nodes_ptr = core::ptr::addr_of_mut!((*ptr).nodes);
        for i in 0..MAX_VNODES {
            core::ptr::write(core::ptr::addr_of_mut!((*nodes_ptr)[i]), VNode::empty());
        }

        // PageCache is ~0.5 MiB (MAX_DATA_PAGES x CachedPage). Passing it by
        // value - 'write(dst, PageCache::new())' - makes the compiler build
        // the whole struct in a stack temporary and only then copy it here.
        // That temporary alone is larger than the BSP stack, so it ran off
        // the bottom of the stack and silently zeroed the .bss statics that
        // sit right below it (apic::LAPIC_BASE_VIRT, apic::LAPIC_TICKS_PER_HZ,
        // percpu::CPUS, the IDT...), which killed the LAPIC timer and hung the
        // kernel at the first 'sti'. Build the cache in place, field by field,
        // so nothing bigger than one CachedPage ever lands on the stack
        let cache_ptr = core::ptr::addr_of_mut!((*ptr).page_cache);
        let pages_ptr = core::ptr::addr_of_mut!((*cache_ptr).pages);
        for i in 0..MAX_DATA_PAGES {
            core::ptr::write(
                core::ptr::addr_of_mut!((*pages_ptr)[i]),
                CachedPage::empty(),
            );
        }
        core::ptr::write(
            core::ptr::addr_of_mut!((*cache_ptr).slab),
            Slab::<MAX_DATA_PAGES>::new(),
        );
        core::ptr::write(
            core::ptr::addr_of_mut!((*cache_ptr).lru),
            LruList::<MAX_DATA_PAGES>::new(),
        );
        core::ptr::write(core::ptr::addr_of_mut!((*cache_ptr).total_writes), 0);
        core::ptr::write(core::ptr::addr_of_mut!((*cache_ptr).total_reads), 0);
        core::ptr::write(core::ptr::addr_of_mut!((*cache_ptr).evictions), 0);

        core::ptr::write(core::ptr::addr_of_mut!((*ptr).mounts), MountTable::new());
        // Per-process FD tables. Pre-create pid 0 (kernel) entry so
        // pre-process and kernel callers see a valid table without an
        // implicit insertion under a shared-reference path
        let mut tables: BTreeMap<u64, FdTable> = BTreeMap::new();
        tables.insert(0, FdTable::new());
        core::ptr::write(core::ptr::addr_of_mut!((*ptr).fd_tables), tables);
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

    // Report the cache the kernel actually has, not the one it declares.
    // The slab used to cap allocation at 64 entries no matter how large
    // MAX_DATA_PAGES was, so this line is also the regression guard
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
/// file-backed mmap page-fault fill path
pub fn read_at_vnode(vid: usize, offset: u64, buf: &mut [u8]) -> Result<usize, crate::fs::vfs::types::VfsError> {
    with_vfs(|vfs| vfs.read_at_vnode(vid, offset, buf))
}

/// Write buf to a vnode at a byte offset (no fd). MAP_SHARED writeback path
pub fn write_at_vnode(vid: usize, offset: u64, buf: &[u8]) -> Result<usize, crate::fs::vfs::types::VfsError> {
    with_vfs(|vfs| vfs.write_at_vnode(vid, offset, buf))
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

pub fn with_vfs<F, R>(f: F) -> R
where
    F: FnOnce(&mut MikuVFS) -> R,
{
    let _guard = VFS_LOCK.lock();
    let vfs = get_vfs();
    sync_ctx_from_current(vfs);
    f(vfs)
}

pub fn with_vfs_ro<F, R>(f: F) -> R
where
    F: FnOnce(&MikuVFS) -> R,
{
    let _guard = VFS_LOCK.lock();
    let vfs = get_vfs();
    sync_ctx_from_current(vfs);
    f(vfs)
}

// Copy the current process's per-process identity (umask + credentials)
// into 'vfs.ctx' so existing MikuVFS code that reads 'self.ctx.umask'
// or 'self.ctx.cred.X' sees the calling process's view rather than a
// stale global. cwd is owned by the Process struct and looked up via
// 'sched::current_cwd()' directly, but vfs.ctx.cwd is kept in sync
// here too so that any legacy reader still sees correct data
fn sync_ctx_from_current(vfs: &mut MikuVFS) {
    let (umask, uid, gid, euid, egid) = crate::sched::current_identity();
    vfs.ctx.umask     = umask;
    vfs.ctx.cred.uid  = uid;
    vfs.ctx.cred.gid  = gid;
    vfs.ctx.cred.euid = euid;
    vfs.ctx.cred.egid = egid;
    vfs.ctx.cwd       = crate::sched::current_cwd() as InodeId;
}

pub struct MikuVFS {
    pub nodes: [VNode; MAX_VNODES],
    pub page_cache: PageCache,
    pub mounts: MountTable,
    /// Per-process FD tables, keyed by pid. Entries are lazily created
    /// on first access via 'fds()'. pid 0 is the kernel / pre-process
    /// fallback table. fork() clones the parent's table for the child;
    /// process exit drops the entry
    pub fd_tables: BTreeMap<u64, FdTable>,
    pub ctx: ProcessContext,
    pub ext2_mount_active: bool,
    pub(crate) vnode_free_hint: usize,
}

impl MikuVFS {
    /// Mutable reference to the FD table for the calling (current) pid
    /// Inserts an empty table if none exists yet
    #[inline]
    pub fn fds(&mut self) -> &mut FdTable {
        let pid = crate::sched::current_pid();
        self.fd_tables.entry(pid).or_insert_with(FdTable::new)
    }

    /// Mutable reference to a specific pid's FD table, creating if absent
    #[inline]
    pub fn fds_for(&mut self, pid: u64) -> &mut FdTable {
        self.fd_tables.entry(pid).or_insert_with(FdTable::new)
    }

    /// Read-only view of the current pid's table. Falls back to pid 0 if
    /// the current pid has no table yet (avoids hidden insertions on
    /// read paths)
    #[inline]
    pub fn fds_ro(&self) -> &FdTable {
        let pid = crate::sched::current_pid();
        self.fd_tables.get(&pid)
            .or_else(|| self.fd_tables.get(&0))
            .expect("[vfs] kernel fd_table (pid 0) missing - bootstrap bug")
    }

    /// Clone parent's table into the child on fork(). Each open
    /// descriptor in the parent now has a sibling in the child pointing
    /// at the same vnode, so the vnode refcounts must be bumped to keep
    /// drop_fds()'s dec_ref symmetric on exit
    pub fn fork_fds(&mut self, parent_pid: u64, child_pid: u64) {
        let parent = match self.fd_tables.get(&parent_pid) {
            Some(t) => t.clone(),
            None    => FdTable::new(),
        };
        let bumps: alloc::vec::Vec<crate::fs::vfs::InodeId> = parent
            .files.iter().filter(|f| f.active).map(|f| f.vnode_id).collect();
        for vid in bumps {
            let idx = vid as usize;
            if self.valid_vnode(idx) {
                self.nodes[idx].inc_ref();
            }
        }
        self.fd_tables.insert(child_pid, parent);
    }

    /// Drop a process's FD table at reap time. Each open descriptor
    /// dec_refs its vnode, mirroring what 'close(fd)' would do for an
    /// orderly close. Returns the list of vnode ids that were released
    /// so the caller can run any deferred-free cleanup (the inline
    /// unlink-on-close path lives in 'close()' and is intentionally not
    /// duplicated here - reaping happens after the process is already
    /// dead, the on-disk delete will be handled by the ext layer's own
    /// orphan inode logic if applicable)
    pub fn drop_fds(&mut self, pid: u64) -> alloc::vec::Vec<crate::fs::vfs::InodeId> {
        let mut victims = alloc::vec::Vec::new();
        if let Some(table) = self.fd_tables.remove(&pid) {
            for f in table.files.iter() {
                if f.active { victims.push(f.vnode_id); }
            }
        }
        victims
    }

    /// Close every descriptor flagged O_CLOEXEC in the calling process's
    /// table. Returns the released vnode ids so the caller can run the
    /// standard dec_ref + deferred-free cleanup, matching what an
    /// ordinary 'close()' would do
    pub fn close_cloexec_fds(&mut self) -> alloc::vec::Vec<crate::fs::vfs::InodeId> {
        let pid = crate::sched::current_pid();
        let mut victims = alloc::vec::Vec::new();
        if let Some(table) = self.fd_tables.get_mut(&pid) {
            for f in table.files.iter_mut() {
                if f.active && f.flags.has(crate::fs::vfs::types::OpenFlags::CLOEXEC) {
                    victims.push(f.vnode_id);
                    *f = crate::fs::vfs::fd::OpenFile::empty();
                }
            }
        }
        victims
    }
}

/// Release a process's descriptors as soon as it exits, without waiting for
/// the parent to reap it.
///
/// A zombie owns nothing but its exit code, yet its fd table used to survive
/// until reap. That is invisible for ordinary files, but fatal for pipes:
/// the reader decides EOF from "does any table still hold a writable
/// descriptor", so a writer that had exited but not been reaped kept the
/// pipe open forever - and the parent could not reap it, because it was
/// blocked waiting for the reader. `a | b` deadlocked on exactly that.
pub fn release_fds_on_exit(pid: u64) {
    let victims = with_vfs(|vfs| vfs.drop_fds(pid));
    if victims.is_empty() {
        return;
    }
    with_vfs(|vfs| {
        for vid in victims {
            let idx = vid as usize;
            if vfs.valid_vnode(idx) {
                vfs.nodes[idx].dec_ref();
            }
        }
    });
}

// Pipe support

/// pids that still hold a writable descriptor on this vnode
pub fn pipe_writer_pids(vnode_id: usize) -> alloc::vec::Vec<u64> {
    with_vfs(|vfs| {
        let mut out = alloc::vec::Vec::new();
        for (pid, table) in vfs.fd_tables.iter() {
            if table.any_writer_on(vnode_id) {
                out.push(*pid);
            }
        }
        out
    })
}

pub enum PipeRead {
    Data(usize),
    WouldBlock,
    Eof,
}

/// Non-blocking attempt to consume from a pipe. Never sleeps, so it is safe
/// under the VFS lock; the waiting is the caller's job
pub fn pipe_try_read(vnode_id: usize, buf: &mut [u8]) -> PipeRead {
    let got = with_vfs(|vfs| {
        if !vfs.valid_vnode(vnode_id) { return None; }
        let pos = vfs.nodes[vnode_id].pipe_read_pos;
        let size = vfs.nodes[vnode_id].size;
        if pos >= size { return Some(0usize); }
        match vfs.read_at_vnode(vnode_id, pos, buf) {
            Ok(n) => { vfs.nodes[vnode_id].pipe_read_pos = pos + n as u64; Some(n) }
            Err(_) => Some(0usize),
        }
    });
    match got {
        None => PipeRead::Eof,
        Some(0) => {
            if pipe_writer_pids(vnode_id).is_empty() { PipeRead::Eof } else { PipeRead::WouldBlock }
        }
        Some(n) => PipeRead::Data(n),
    }
}

pub fn pipe_vnode_of_fd(fd: usize) -> Option<usize> {
    with_vfs(|vfs| {
        let vid = vfs.fds().get(fd).ok()?.vnode_id as usize;
        if vfs.valid_vnode(vid) && vfs.nodes[vid].is_pipe() { Some(vid) } else { None }
    })
}
