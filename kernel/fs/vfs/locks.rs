use crate::fs::vfs::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LockType {
    Shared = 0,
    Exclusive = 1,
}

#[derive(Clone, Copy)]
pub struct FileLock {
    pub vnode_id: InodeId,
    pub lock_type: LockType,
    pub pid: u16,
    pub offset: u64,
    pub length: u64,
    pub active: bool,
}

impl FileLock {
    pub const fn empty() -> Self {
        Self {
            vnode_id: INVALID_ID,
            lock_type: LockType::Shared,
            pid: 0,
            offset: 0,
            length: 0,
            active: false,
        }
    }

    pub fn conflicts_with(&self, other: &FileLock) -> bool {
        if !self.active || !other.active {
            return false;
        }
        if self.vnode_id != other.vnode_id {
            return false;
        }
        if self.pid == other.pid {
            return false;
        }

        let s1 = self.offset;
        let e1 = if self.length == 0 {
            u64::MAX
        } else {
            self.offset + self.length
        };
        let s2 = other.offset;
        let e2 = if other.length == 0 {
            u64::MAX
        } else {
            other.offset + other.length
        };

        if s1 >= e2 || s2 >= e1 {
            return false;
        }

        if self.lock_type == LockType::Shared && other.lock_type == LockType::Shared {
            return false;
        }

        true
    }
}

pub struct LockManager {
    pub locks: [FileLock; MAX_LOCKS],
}

impl LockManager {
    pub const fn new() -> Self {
        Self {
            locks: [FileLock::empty(); MAX_LOCKS],
        }
    }

    pub fn acquire(
        &mut self,
        vnode_id: InodeId,
        pid: u16,
        lock_type: LockType,
        offset: u64,
        length: u64,
    ) -> VfsResult<()> {
        let new_lock = FileLock {
            vnode_id,
            lock_type,
            pid,
            offset,
            length,
            active: true,
        };

        for lock in &self.locks {
            if lock.conflicts_with(&new_lock) {
                return Err(VfsError::WouldBlock);
            }
        }

        for lock in self.locks.iter_mut() {
            if !lock.active {
                *lock = new_lock;
                return Ok(());
            }
        }

        Err(VfsError::NoSpace)
    }

    pub fn release(&mut self, vnode_id: InodeId, pid: u16) -> VfsResult<()> {
        let mut found = false;
        for lock in self.locks.iter_mut() {
            if lock.active && lock.vnode_id == vnode_id && lock.pid == pid {
                *lock = FileLock::empty();
                found = true;
            }
        }
        if found {
            Ok(())
        } else {
            Err(VfsError::NoLock)
        }
    }

    pub fn release_all_for_pid(&mut self, pid: u16) {
        for lock in self.locks.iter_mut() {
            if lock.active && lock.pid == pid {
                *lock = FileLock::empty();
            }
        }
    }

    pub fn release_all_for_vnode(&mut self, vnode_id: InodeId) {
        for lock in self.locks.iter_mut() {
            if lock.active && lock.vnode_id == vnode_id {
                *lock = FileLock::empty();
            }
        }
    }

    pub fn has_lock(&self, vnode_id: InodeId, pid: u16) -> bool {
        self.locks
            .iter()
            .any(|l| l.active && l.vnode_id == vnode_id && l.pid == pid)
    }

    pub fn lock_count(&self) -> usize {
        self.locks.iter().filter(|l| l.active).count()
    }
}
