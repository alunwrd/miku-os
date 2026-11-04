// POSIX record locks: `fcntl`'s F_GETLK, F_SETLK and F_SETLKW
//
// Advisory byte-range locks on a file, owned by a process (a thread group:
// every thread of one process shares its locks, as POSIX says). SQLite's
// concurrency depends on these: a reader takes a shared lock on one byte range,
// a writer an exclusive one on another.
//
// The semantics are POSIX's:
//
//   - A lock is keyed by (vnode, process), never by descriptor. Two
//     descriptors on one file in one process share its locks.
//   - Locks of one process never conflict with each other. A new lock
//     replaces whatever that process held over the same bytes, splitting an
//     older range if it covers only part of one.
//   - Closing any descriptor on a file releases all of that process's locks
//     on it, even ones taken through another descriptor. `vfs.close` and the
//     close-on-exec sweep call `release`; process exit calls `release_owner`.
//
// A range is [start, end) in bytes, with `end == u64::MAX` meaning to the end
// of the file, however long it grows (`l_len == 0`).
//
// Not here: deadlock detection for F_SETLKW (a waiter that would complete a
// cycle waits until a signal ends it, with no EDEADLK), and the Linux-only
// open-file-description locks (F_OFD_*). One global table: the number of locks
// on a machine is small, and a scan of it is shorter than the syscall.

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::vfs::types::InodeId;
use crate::sched::wait::WaitQueue;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug)]
pub struct Lock {
    pub vnode: InodeId,
    pub owner: u64,
    pub start: u64,
    pub end: u64,
    pub kind: Kind,
}

impl Lock {
    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.start < end && start < self.end
    }
}

static LOCKS: Mutex<Vec<Lock>> = Mutex::new(Vec::new());

/// Everyone in an F_SETLKW, woken whenever a lock goes away
static RELEASED: WaitQueue = WaitQueue::new();

fn first_conflict(locks: &[Lock], vnode: InodeId, owner: u64, start: u64, end: u64, kind: Kind) -> Option<Lock> {
    locks.iter().copied().find(|l| {
        l.vnode == vnode
            && l.owner != owner
            && l.overlaps(start, end)
            && (kind == Kind::Write || l.kind == Kind::Write)
    })
}

/// The first lock another process holds that `kind` over [start, end) would
/// conflict with: any overlapping lock if either side is a writer
pub fn conflict(vnode: InodeId, owner: u64, start: u64, end: u64, kind: Kind) -> Option<Lock> {
    first_conflict(&LOCKS.lock(), vnode, owner, start, end, kind)
}

/// Take `kind` over [start, end), or drop the caller's locks there if `kind`
/// is `None`. `Err` with the conflicting lock if another process's is in the
/// way; nothing changes in that case
pub fn set(vnode: InodeId, owner: u64, start: u64, end: u64, kind: Option<Kind>) -> Result<(), Lock> {
    let released;
    {
        let mut locks = LOCKS.lock();
        if let Some(k) = kind {
            if let Some(c) = first_conflict(&locks, vnode, owner, start, end, k) {
                return Err(c);
            }
        }

        // This owner's locks over the range go: whole, or trimmed to the part
        // outside it. A lock that straddles the range on both sides becomes two
        let mut kept: Vec<Lock> = Vec::new();
        let mut changed = false;
        locks.retain(|l| {
            if l.vnode != vnode || l.owner != owner || !l.overlaps(start, end) {
                return true;
            }
            changed = true;
            if l.start < start {
                kept.push(Lock { end: start, ..*l });
            }
            if l.end > end {
                kept.push(Lock { start: end, ..*l });
            }
            false
        });
        locks.extend(kept);
        if let Some(k) = kind {
            locks.push(Lock { vnode, owner, start, end, kind: k });
        }
        // A downgrade (write to read) releases as much as an unlock does, for a
        // waiting reader
        released = changed;
    }
    if released {
        RELEASED.wake_all();
    }
    Ok(())
}

/// Wait until `set` can succeed, then do it. `Err(())` if a signal arrived
/// first: F_SETLKW's EINTR
pub fn set_wait(vnode: InodeId, owner: u64, start: u64, end: u64, kind: Kind) -> Result<(), ()> {
    let queues = [&RELEASED];
    loop {
        if set(vnode, owner, start, end, Some(kind)).is_ok() {
            return Ok(());
        }
        if crate::process::signal::has_deliverable(crate::sched::current_pid()) {
            return Err(());
        }
        // On the queue, then tried again (`wait::join`): a release between the
        // failed `set` and the sleep wakes it. A signal wakes it too; the
        // backstop is one second
        let joined = crate::sched::wait::join(&queues);
        if set(vnode, owner, start, end, Some(kind)).is_ok() {
            return Ok(());
        }
        joined.sleep(crate::kcore::time::now_ns().saturating_add(1_000_000_000));
    }
}

/// POSIX's close rule: every lock `owner` holds on `vnode` goes, whichever
/// descriptor took it
pub fn release(owner: u64, vnode: InodeId) {
    let removed = {
        let mut locks = LOCKS.lock();
        let before = locks.len();
        locks.retain(|l| !(l.owner == owner && l.vnode == vnode));
        before != locks.len()
    };
    if removed {
        RELEASED.wake_all();
    }
}

/// Everything `owner` holds, at exit
pub fn release_owner(owner: u64) {
    let removed = {
        let mut locks = LOCKS.lock();
        let before = locks.len();
        locks.retain(|l| l.owner != owner);
        before != locks.len()
    };
    if removed {
        RELEASED.wake_all();
    }
}
