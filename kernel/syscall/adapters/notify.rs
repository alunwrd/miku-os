// inotify: 116 inotify_init1, 117 inotify_add_watch, 118 inotify_rm_watch
//
// Linux's three calls with Linux's meaning, so `inotify-sys`, which declares
// them itself with no `cfg`, works against `libc_miku.so`. The instance and its
// events are `fs::vfs::notify`; reading and polling a descriptor on one is
// `io::sys_read`/`io::revents_for`, through `FdKind::Inotify`. See
// dev/fs-architecture.md, "Watching".

use crate::fs::vfs::notify;
use crate::fs::vfs::types::OpenFlags;
use crate::syscall::errno::{err, vfs_err, EINVAL};
use crate::syscall::usercopy::read_user_path_at;

// 116  inotify_init1(flags) -> fd
//
// `flags` is `IN_NONBLOCK | IN_CLOEXEC`, which are MikuOS's `O_NONBLOCK` and
// `O_CLOEXEC`. Anything else is `EINVAL`, as on Linux
pub fn sys_inotify_init1(flags: u64) -> u64 {
    let allowed = (OpenFlags::NONBLOCK | OpenFlags::CLOEXEC) as u64;
    if flags & !allowed != 0 {
        return err(EINVAL);
    }
    crate::fs::vfs::core::with_vfs(|vfs| match vfs.inotify_init(flags as u32) {
        Ok(fd) => fd as u64,
        Err(e) => vfs_err(e),
    })
}

// 117  inotify_add_watch(fd, path_ptr, path_len, mask) -> wd
//
// The path is a pointer and a length, as every path is here; `libc_miku.so`
// takes C's string and measures it
pub fn sys_inotify_add_watch(fd: u64, path_ptr: u64, path_len: u64, mask: u64) -> u64 {
    let (path, base) = match read_user_path_at(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mask = mask as u32;
    crate::fs::vfs::core::with_vfs(|vfs| {
        let cwd = match base.start(vfs) { Ok(v) => v, Err(e) => return e };
        let inst = match vfs.inotify_of(fd as usize) {
            Ok(v) => v,
            Err(e) => return vfs_err(e),
        };
        let follow = mask & notify::IN_DONT_FOLLOW == 0;
        let only_dir = mask & notify::IN_ONLYDIR != 0;
        let target = match vfs.watch_target(cwd, &path, follow, only_dir) {
            Ok(v) => v,
            Err(e) => return vfs_err(e),
        };
        match notify::add_watch(inst, target, mask) {
            Ok(wd) => wd as u64,
            Err(e) => err(e),
        }
    })
}

// 118  inotify_rm_watch(fd, wd) -> 0
pub fn sys_inotify_rm_watch(fd: u64, wd: u64) -> u64 {
    crate::fs::vfs::core::with_vfs(|vfs| {
        let inst = match vfs.inotify_of(fd as usize) {
            Ok(v) => v,
            Err(e) => return vfs_err(e),
        };
        match notify::rm_watch(inst, wd as i32) {
            Ok(()) => 0,
            Err(e) => err(e),
        }
    })
}

/// `read` on an instance: whole events, waiting for one unless the descriptor
/// is non-blocking. Outside the VFS lock
pub fn read(fd: u64, vid: usize, buf: &mut [u8]) -> u64 {
    use crate::syscall::errno::{EAGAIN, EINTR};
    let take = |buf: &mut [u8]| match notify::read(vid, buf) {
        Ok(n) => Some(n as u64),
        Err(notify::ReadError::TooSmall) | Err(notify::ReadError::NotInstance) => Some(err(EINVAL)),
        Err(notify::ReadError::Empty) => None,
    };
    let queues = [&crate::sched::wait::POLL];
    loop {
        if let Some(r) = take(buf) {
            return r;
        }
        if super::file::fd_is_nonblocking(fd) {
            return err(EAGAIN);
        }
        if crate::process::signal::has_deliverable(crate::sched::current_pid()) {
            return err(EINTR);
        }
        // On the queue first, then the look that counts (`wait::join`), so an
        // event between the empty queue and the sleep is not left to the
        // backstop
        let joined = crate::sched::wait::join(&queues);
        if let Some(r) = take(buf) {
            return r;
        }
        joined.sleep(crate::kcore::time::now_ns().saturating_add(100_000_000));
    }
}
