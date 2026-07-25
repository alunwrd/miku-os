use crate::fs::vfs::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JournalOp {
    CreateFile = 1,
    DeleteFile = 2,
    WriteData = 3,
    CreateDir = 4,
    DeleteDir = 5,
    Rename = 6,
    SetAttr = 7,
    Link = 8,
    Symlink = 9,
}

#[derive(Clone, Copy)]
pub struct JournalEntry {
    pub op: JournalOp,
    pub vnode_id: InodeId,
    pub parent_id: InodeId,
    pub name: NameBuf,
    pub timestamp: Timestamp,
    pub committed: bool,
    pub active: bool,
}

impl JournalEntry {
    pub const fn empty() -> Self {
        Self {
            op: JournalOp::CreateFile,
            vnode_id: INVALID_ID,
            parent_id: INVALID_ID,
            name: NameBuf::empty(),
            timestamp: 0,
            committed: false,
            active: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalState {
    Idle,
    Recording,
    Committing,
    Recovering,
}

pub struct Journal {
    pub entries: [JournalEntry; MAX_JOURNAL_BLOCKS],
    pub write_pos: usize,
    pub state: JournalState,
    pub sequence: u64,
}

impl Journal {
    pub const fn new() -> Self {
        Self {
            entries: [JournalEntry::empty(); MAX_JOURNAL_BLOCKS],
            write_pos: 0,
            state: JournalState::Idle,
            sequence: 0,
        }
    }

    pub fn begin(&mut self) -> VfsResult<()> {
        if self.state != JournalState::Idle {
            return Err(VfsError::Busy);
        }
        self.state = JournalState::Recording;
        Ok(())
    }

    pub fn record(
        &mut self,
        op: JournalOp,
        vnode_id: InodeId,
        parent_id: InodeId,
        name: &str,
        timestamp: Timestamp,
    ) -> VfsResult<()> {
        if self.state != JournalState::Recording {
            return Err(VfsError::InvalidArgument);
        }

        let idx = self.write_pos % MAX_JOURNAL_BLOCKS;
        self.entries[idx] = JournalEntry {
            op,
            vnode_id,
            parent_id,
            name: NameBuf::from_str(name),
            timestamp,
            committed: false,
            active: true,
        };
        self.write_pos += 1;
        Ok(())
    }

    pub fn commit(&mut self) -> VfsResult<()> {
        if self.state != JournalState::Recording {
            return Err(VfsError::InvalidArgument);
        }
        self.state = JournalState::Committing;

        for entry in self.entries.iter_mut() {
            if entry.active && !entry.committed {
                entry.committed = true;
            }
        }

        self.sequence += 1;
        self.state = JournalState::Idle;
        Ok(())
    }

    pub fn abort(&mut self) {
        let mut removed = 0usize;
        for entry in self.entries.iter_mut() {
            if entry.active && !entry.committed {
                *entry = JournalEntry::empty();
                removed += 1;
            }
        }

        self.write_pos = self.write_pos.saturating_sub(removed);
        self.state = JournalState::Idle;
    }

    pub fn clear(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = JournalEntry::empty();
        }
        self.write_pos = 0;
        self.state = JournalState::Idle;
    }

    pub fn entry_count(&self) -> usize {
        self.entries.iter().filter(|e| e.active).count()
    }

    pub fn pending_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.active && !e.committed)
            .count()
    }
}
