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

use crate::vfs::fd::FdTable;
use crate::vfs::mount::MountTable;
use crate::vfs::pages::PageCache;
use crate::vfs::types::*;
use crate::vfs::vnode::VNode;
use alloc::collections::BTreeMap;
use spin::Mutex;

// Global lock - serializes all VFS operations
static VFS_LOCK: Mutex<()> = Mutex::new(());

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

        core::ptr::write(core::ptr::addr_of_mut!((*ptr).page_cache), PageCache::new());
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

    crate::serial_println!("[vfs] init done");
    Ok(())
}

/// Register /dev block-device nodes (/dev/blkN, /dev/blkNpM). Call once
/// after 'block::probe', since the block layer must already know its
/// devices and partitions
pub fn register_block_nodes() {
    with_vfs(|vfs| vfs.register_block_nodes());
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
// 'scheduler::current_cwd()' directly, but vfs.ctx.cwd is kept in sync
// here too so that any legacy reader still sees correct data
fn sync_ctx_from_current(vfs: &mut MikuVFS) {
    let (umask, uid, gid, euid, egid) = crate::scheduler::current_identity();
    vfs.ctx.umask     = umask;
    vfs.ctx.cred.uid  = uid;
    vfs.ctx.cred.gid  = gid;
    vfs.ctx.cred.euid = euid;
    vfs.ctx.cred.egid = egid;
    vfs.ctx.cwd       = crate::scheduler::current_cwd() as InodeId;
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
        let pid = crate::scheduler::current_pid();
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
        let pid = crate::scheduler::current_pid();
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
        let bumps: alloc::vec::Vec<crate::vfs::InodeId> = parent
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
    /// dec_refs its vnode, mirroring what `close(fd)` would do for an
    /// orderly close. Returns the list of vnode ids that were released
    /// so the caller can run any deferred-free cleanup (the inline
    /// unlink-on-close path lives in `close()` and is intentionally not
    /// duplicated here - reaping happens after the process is already
    /// dead, the on-disk delete will be handled by the ext layer's own
    /// orphan inode logic if applicable)
    pub fn drop_fds(&mut self, pid: u64) -> alloc::vec::Vec<crate::vfs::InodeId> {
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
    /// ordinary `close()` would do
    pub fn close_cloexec_fds(&mut self) -> alloc::vec::Vec<crate::vfs::InodeId> {
        let pid = crate::scheduler::current_pid();
        let mut victims = alloc::vec::Vec::new();
        if let Some(table) = self.fd_tables.get_mut(&pid) {
            for f in table.files.iter_mut() {
                if f.active && f.flags.has(crate::vfs::types::OpenFlags::CLOEXEC) {
                    victims.push(f.vnode_id);
                    *f = crate::vfs::fd::OpenFile::empty();
                }
            }
        }
        victims
    }
}
