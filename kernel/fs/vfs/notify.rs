// inotify: telling a program that a file it cares about changed
//
// Linux's interface, byte for byte: `notify`, under Zed, has an inotify
// backend, and `inotify-sys` declares the three calls with no `cfg`. See
// dev/fs-architecture.md, "Watching".
//
// An instance is what `inotify_init1`(116) makes: a queue of events and the
// watches that feed it. It is reached through a descriptor on an anonymous
// vnode (`MikuVFS::anon_node`), and keyed here by that vnode's id; it goes with
// the vnode's last reference (`VNode::dec_ref` calls `destroy`). A watch is
// `(wd, vnode, mask)` inside one instance.
//
// Events are raised by `kernel/fs/vfs/core` where each operation succeeds,
// through `raise` (to one vnode's watches) and the two helpers under it that
// know the Linux rule: an event on a file goes to that file's watches with no
// name and to its directory's with the file's name.
//
// One table, one lock, taken inside the VFS lock and never outside it. Queuing
// only marks a wake; `with_vfs` does it once the VFS lock is gone, since the
// scheduler is not entered under the filesystem's lock.

extern crate alloc;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use spin::Mutex;

use super::core::MikuVFS;

// The event bits, Linux's values
pub use miku_abi::inotify::IN_ACCESS;
pub use miku_abi::inotify::IN_MODIFY;
pub use miku_abi::inotify::IN_ATTRIB;
pub use miku_abi::inotify::IN_CLOSE_WRITE;
pub use miku_abi::inotify::IN_CLOSE_NOWRITE;
pub use miku_abi::inotify::IN_OPEN;
pub use miku_abi::inotify::IN_MOVED_FROM;
pub use miku_abi::inotify::IN_MOVED_TO;
pub use miku_abi::inotify::IN_CREATE;
pub use miku_abi::inotify::IN_DELETE;
pub use miku_abi::inotify::IN_DELETE_SELF;
pub use miku_abi::inotify::IN_MOVE_SELF;
/// Every event a watch can ask for
pub use miku_abi::inotify::IN_ALL_EVENTS;
// Raised whatever the watch asked for
pub use miku_abi::inotify::IN_UNMOUNT;
pub use miku_abi::inotify::IN_Q_OVERFLOW;
pub use miku_abi::inotify::IN_IGNORED;
// `inotify_add_watch` flags
pub use miku_abi::inotify::IN_ONLYDIR;
pub use miku_abi::inotify::IN_DONT_FOLLOW;
pub use miku_abi::inotify::IN_EXCL_UNLINK;
pub use miku_abi::inotify::IN_MASK_CREATE;
pub use miku_abi::inotify::IN_MASK_ADD;
/// In an event: the subject is a directory
pub use miku_abi::inotify::IN_ISDIR;
pub use miku_abi::inotify::IN_ONESHOT;

/// Instances on the machine, Linux's per-user default
pub const MAX_INSTANCES: usize = 128;
/// Watches in one instance
pub const MAX_WATCHES: usize = 8192;
/// Events one instance holds unread before it overflows. Linux's default
/// is 16384; each event here carries its name, so this is kept smaller
pub const MAX_QUEUED: usize = 4096;

/// The device numbers an instance's vnode carries: major 0 is never a devfs
/// node, so `devfs` answers `None` for it and `classify` asks here first
pub const DEV: (u8, u8) = (0, 1);

/// `struct inotify_event` without its name: wd, mask, cookie, len
const EVENT_HEADER: usize = 16;

pub enum ReadError {
    /// Nothing queued
    Empty,
    /// The next event does not fit in the buffer: `EINVAL`, as on Linux
    TooSmall,
    /// Not an instance
    NotInstance,
}

struct Event {
    wd: i32,
    mask: u32,
    cookie: u32,
    name: Vec<u8>,
}

impl Event {
    /// The name is NUL-terminated and padded so the next record starts on a
    /// 16-byte boundary; `len` counts all of that, as on Linux
    fn padded_name_len(&self) -> usize {
        if self.name.is_empty() {
            0
        } else {
            (self.name.len() + 1 + EVENT_HEADER - 1) & !(EVENT_HEADER - 1)
        }
    }

    fn size(&self) -> usize {
        EVENT_HEADER + self.padded_name_len()
    }
}

struct Watch {
    wd: i32,
    vnode: usize,
    mask: u32,
}

struct Instance {
    /// The anonymous vnode behind the descriptor
    vnode: usize,
    watches: Vec<Watch>,
    next_wd: i32,
    queue: VecDeque<Event>,
    overflowed: bool,
}

impl Instance {
    fn push(&mut self, ev: Event) {
        // Linux coalesces an event identical to the one at the tail. A file
        // written in a loop is one IN_MODIFY waiting, not a thousand
        if let Some(last) = self.queue.back() {
            if last.wd == ev.wd && last.mask == ev.mask && last.cookie == ev.cookie && last.name == ev.name {
                return;
            }
        }
        if self.queue.len() >= MAX_QUEUED {
            if !self.overflowed {
                self.overflowed = true;
                self.queue.push_back(Event { wd: -1, mask: IN_Q_OVERFLOW, cookie: 0, name: Vec::new() });
                WAKE.store(true, Ordering::Release);
            }
            return;
        }
        self.queue.push_back(ev);
        WAKE.store(true, Ordering::Release);
    }
}

static TABLE: Mutex<Vec<Instance>> = Mutex::new(Vec::new());
/// Watches on the machine. Zero means every hook below is one load and out
static WATCHES: AtomicUsize = AtomicUsize::new(0);
/// Watches on each vnode, for the cache's reclaim: a watched vnode is not
/// evicted, which would end its watch. Beside the instances, under `TABLE`'s
/// lock, so the question is one lookup
static WATCHED: Mutex<BTreeMap<usize, u32>> = Mutex::new(BTreeMap::new());

fn watched_add(vnode: usize) {
    *WATCHED.lock().entry(vnode).or_insert(0) += 1;
}

fn watched_sub(vnode: usize) {
    let mut w = WATCHED.lock();
    if let Some(n) = w.get_mut(&vnode) {
        *n -= 1;
        if *n == 0 {
            w.remove(&vnode);
        }
    }
}

/// Whether any watch names `vnode`
pub fn is_watched(vnode: usize) -> bool {
    active() && WATCHED.lock().contains_key(&vnode)
}
/// Something was queued under the VFS lock and its readers are not awake yet
static WAKE: AtomicBool = AtomicBool::new(false);
/// Pairs an `IN_MOVED_FROM` with its `IN_MOVED_TO`
static COOKIE: AtomicU32 = AtomicU32::new(1);

/// True when any watch exists anywhere. The hooks' fast path
#[inline]
pub fn active() -> bool {
    WATCHES.load(Ordering::Relaxed) != 0
}

/// A fresh cookie for one rename
pub fn next_cookie() -> u32 {
    COOKIE.fetch_add(1, Ordering::Relaxed).max(1)
}

/// Wake pollers and readers if an event was queued. Called by `with_vfs`
/// once the VFS lock is released
pub fn wake_if_raised() {
    if WAKE.swap(false, Ordering::AcqRel) {
        crate::sched::wait::wake_pollers();
    }
}

/// Make the instance behind anonymous vnode `vnode`. False when the machine
/// already has `MAX_INSTANCES`
pub fn create(vnode: usize) -> bool {
    let mut t = TABLE.lock();
    if t.len() >= MAX_INSTANCES {
        return false;
    }
    t.push(Instance { vnode, watches: Vec::new(), next_wd: 1, queue: VecDeque::new(), overflowed: false });
    true
}

/// The instance behind `vnode` has lost its last descriptor
pub fn destroy(vnode: usize) {
    let mut t = TABLE.lock();
    if let Some(i) = t.iter().position(|x| x.vnode == vnode) {
        let gone = t[i].watches.len();
        for w in &t[i].watches {
            watched_sub(w.vnode);
        }
        t.swap_remove(i);
        WATCHES.fetch_sub(gone, Ordering::Relaxed);
    }
}


pub fn is_instance(vnode: usize) -> bool {
    TABLE.lock().iter().any(|x| x.vnode == vnode)
}

/// Add a watch on `target`, or change the one this instance has on it. Linux's
/// rule: one watch per (instance, inode), and the same `wd` back. `Err` is an
/// errno constant, ready for `errno::err`
pub fn add_watch(inst: usize, target: usize, mask: u32) -> Result<i32, i64> {
    use crate::syscall::errno::{EEXIST, EINVAL, ENOSPC};
    let events = mask & IN_ALL_EVENTS;
    if events == 0 {
        return Err(EINVAL);
    }
    if mask & IN_MASK_ADD != 0 && mask & IN_MASK_CREATE != 0 {
        return Err(EINVAL);
    }
    let keep = events | (mask & IN_ONESHOT) | (mask & IN_EXCL_UNLINK);
    let mut t = TABLE.lock();
    let ins = t.iter_mut().find(|x| x.vnode == inst).ok_or(EINVAL)?;
    if let Some(w) = ins.watches.iter_mut().find(|w| w.vnode == target) {
        if mask & IN_MASK_CREATE != 0 {
            return Err(EEXIST);
        }
        w.mask = if mask & IN_MASK_ADD != 0 { w.mask | keep } else { keep };
        return Ok(w.wd);
    }
    if ins.watches.len() >= MAX_WATCHES {
        return Err(ENOSPC);
    }
    let wd = ins.next_wd;
    ins.next_wd = ins.next_wd.wrapping_add(1).max(1);
    ins.watches.push(Watch { wd, vnode: target, mask: keep });
    watched_add(target);
    WATCHES.fetch_add(1, Ordering::Relaxed);
    Ok(wd)
}

/// Remove a watch, queuing its `IN_IGNORED`
pub fn rm_watch(inst: usize, wd: i32) -> Result<(), i64> {
    use crate::syscall::errno::EINVAL;
    let mut t = TABLE.lock();
    let ins = t.iter_mut().find(|x| x.vnode == inst).ok_or(EINVAL)?;
    let i = ins.watches.iter().position(|w| w.wd == wd).ok_or(EINVAL)?;
    let w = ins.watches.swap_remove(i);
    watched_sub(w.vnode);
    WATCHES.fetch_sub(1, Ordering::Relaxed);
    ins.push(Event { wd, mask: IN_IGNORED, cookie: 0, name: Vec::new() });
    Ok(())
}

/// True when a `read` would not wait
pub fn pending(inst: usize) -> bool {
    TABLE.lock().iter().find(|x| x.vnode == inst).is_some_and(|x| !x.queue.is_empty())
}

/// As many whole events as fit in `buf`
pub fn read(inst: usize, buf: &mut [u8]) -> Result<usize, ReadError> {
    let mut t = TABLE.lock();
    let ins = t.iter_mut().find(|x| x.vnode == inst).ok_or(ReadError::NotInstance)?;
    let first = ins.queue.front().ok_or(ReadError::Empty)?;
    if first.size() > buf.len() {
        return Err(ReadError::TooSmall);
    }
    let mut at = 0;
    while let Some(ev) = ins.queue.front() {
        let n = ev.size();
        if at + n > buf.len() {
            break;
        }
        let name_len = ev.padded_name_len();
        buf[at..at + 4].copy_from_slice(&ev.wd.to_ne_bytes());
        buf[at + 4..at + 8].copy_from_slice(&ev.mask.to_ne_bytes());
        buf[at + 8..at + 12].copy_from_slice(&ev.cookie.to_ne_bytes());
        buf[at + 12..at + 16].copy_from_slice(&(name_len as u32).to_ne_bytes());
        let name = &mut buf[at + EVENT_HEADER..at + n];
        name.fill(0);
        name[..ev.name.len()].copy_from_slice(&ev.name);
        at += n;
        if ev.mask & IN_Q_OVERFLOW != 0 {
            ins.overflowed = false;
        }
        ins.queue.pop_front();
    }
    Ok(at)
}

/// Queue `mask` for every watch on `vnode` that asked for it
///
/// `IN_ONESHOT` watches go after their first event, with the `IN_IGNORED`
/// Linux sends
pub fn raise(vnode: usize, mask: u32, cookie: u32, name: Option<&str>) {
    if !active() {
        return;
    }
    let mut t = TABLE.lock();
    let mut dropped = 0;
    for ins in t.iter_mut() {
        let mut i = 0;
        while i < ins.watches.len() {
            let w = &ins.watches[i];
            if w.vnode != vnode || w.mask & mask & IN_ALL_EVENTS == 0 {
                i += 1;
                continue;
            }
            let wd = w.wd;
            let oneshot = w.mask & IN_ONESHOT != 0;
            let name = name.map(|n| n.as_bytes().to_vec()).unwrap_or_default();
            ins.push(Event { wd, mask, cookie, name });
            if oneshot {
                ins.watches.swap_remove(i);
                watched_sub(vnode);
                dropped += 1;
                ins.push(Event { wd, mask: IN_IGNORED, cookie: 0, name: Vec::new() });
            } else {
                i += 1;
            }
        }
    }
    WATCHES.fetch_sub(dropped, Ordering::Relaxed);
}

/// Every watch on `vnode` ends: `IN_DELETE_SELF` first when the file itself
/// went (`deleted`), then `IN_IGNORED`, and the watch is removed
pub fn forget(vnode: usize, deleted: bool, is_dir: bool) {
    if !active() {
        return;
    }
    let mut t = TABLE.lock();
    let mut dropped = 0;
    for ins in t.iter_mut() {
        let mut i = 0;
        while i < ins.watches.len() {
            if ins.watches[i].vnode != vnode {
                i += 1;
                continue;
            }
            let w = ins.watches.swap_remove(i);
            watched_sub(w.vnode);
            dropped += 1;
            if deleted && w.mask & IN_DELETE_SELF != 0 {
                let dir = if is_dir { IN_ISDIR } else { 0 };
                ins.push(Event { wd: w.wd, mask: IN_DELETE_SELF | dir, cookie: 0, name: Vec::new() });
            }
            ins.push(Event { wd: w.wd, mask: IN_IGNORED, cookie: 0, name: Vec::new() });
        }
    }
    WATCHES.fetch_sub(dropped, Ordering::Relaxed);
}

// the rules, for the hooks in `core`

fn dir_bit(vfs: &MikuVFS, vnode: usize) -> u32 {
    if vfs.valid_vnode(vnode) && vfs.nodes[vnode].is_dir() { IN_ISDIR } else { 0 }
}

/// Files that are not files as far as a watcher is concerned: pipes,
/// devices, sockets, and the anonymous vnodes behind descriptors
fn unwatchable(vfs: &MikuVFS, vnode: usize) -> bool {
    use crate::fs::vfs::types::VNodeKind;
    !vfs.valid_vnode(vnode)
        || !matches!(vfs.nodes[vnode].kind, VNodeKind::Regular | VNodeKind::Directory | VNodeKind::Symlink)
}

/// `mask` happened to `vnode` itself: its own watches hear it, and its
/// directory's hear it with its name
pub fn node_event(vfs: &MikuVFS, vnode: usize, mask: u32) {
    if !active() || unwatchable(vfs, vnode) {
        return;
    }
    let mask = mask | dir_bit(vfs, vnode);
    raise(vnode, mask, 0, None);
    let parent = vfs.nodes[vnode].parent as usize;
    if parent != vnode && vfs.valid_vnode(parent) && vfs.nodes[vnode].nlinks > 0 {
        raise(parent, mask, 0, Some(vfs.nodes[vnode].get_name()));
    }
}

/// `mask` happened to the entry `name` in directory `dir`: a create, a delete,
/// one half of a move
pub fn entry_event(dir: usize, name: &str, is_dir: bool, mask: u32, cookie: u32) {
    if !active() {
        return;
    }
    raise(dir, mask | if is_dir { IN_ISDIR } else { 0 }, cookie, Some(name));
}
