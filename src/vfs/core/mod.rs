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

use crate::vfs::fd::FdTable;
use crate::vfs::mount::MountTable;
use crate::vfs::pages::PageCache;
use crate::vfs::types::*;
use crate::vfs::vnode::VNode;
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
