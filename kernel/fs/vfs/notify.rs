use crate::fs::vfs::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NotifyEvent {
    Created = 1,
    Deleted = 2,
    Modified = 3,
    Renamed = 4,
    AttrChanged = 5,
    Opened = 6,
    Closed = 7,
}

#[derive(Clone, Copy)]
pub struct NotifyEntry {
    pub event: NotifyEvent,
    pub vnode_id: InodeId,
    pub parent_id: InodeId,
    pub name: NameBuf,
    pub timestamp: Timestamp,
    pub active: bool,
}

impl NotifyEntry {
    pub const fn empty() -> Self {
        Self {
            event: NotifyEvent::Created,
            vnode_id: INVALID_ID,
            parent_id: INVALID_ID,
            name: NameBuf::empty(),
            timestamp: 0,
            active: false,
        }
    }
}

pub struct NotifyManager {
    pub events: [NotifyEntry; MAX_NOTIFY_EVENTS],
    pub write_pos: usize,
    pub total_events: u64,
}

impl NotifyManager {
    pub const fn new() -> Self {
        Self {
            events: [NotifyEntry::empty(); MAX_NOTIFY_EVENTS],
            write_pos: 0,
            total_events: 0,
        }
    }

    pub fn emit(
        &mut self,
        event: NotifyEvent,
        vnode_id: InodeId,
        parent_id: InodeId,
        name: &str,
        timestamp: Timestamp,
    ) {
        let idx = self.write_pos % MAX_NOTIFY_EVENTS;
        self.events[idx] = NotifyEntry {
            event,
            vnode_id,
            parent_id,
            name: NameBuf::from_str(name),
            timestamp,
            active: true,
        };
        self.write_pos += 1;
        self.total_events += 1;
    }

    pub fn recent(&self, count: usize) -> RecentIter<'_> {
        let start = if self.write_pos > count {
            self.write_pos - count
        } else {
            0
        };
        RecentIter {
            manager: self,
            pos: start,
            end: self.write_pos,
        }
    }

    pub fn clear(&mut self) {
        for e in self.events.iter_mut() {
            *e = NotifyEntry::empty();
        }
        self.write_pos = 0;
    }
}

pub struct RecentIter<'a> {
    manager: &'a NotifyManager,
    pos: usize,
    end: usize,
}

impl<'a> Iterator for RecentIter<'a> {
    type Item = &'a NotifyEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.end {
            return None;
        }
        let idx = self.pos % MAX_NOTIFY_EVENTS;
        self.pos += 1;
        let entry = &self.manager.events[idx];
        if entry.active {
            Some(entry)
        } else {
            None
        }
    }
}
