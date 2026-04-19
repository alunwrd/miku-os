// mikuD journal - ring-buffer event log for service lifecycle

extern crate alloc;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

const JOURNAL_SIZE: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Started,
    Stopped,
    Exited,
    Failed,
    DepFailed,
    Registered,
    TargetChanged,
    Reloaded,
    Shutdown,
    BurstLimit,
    WatchdogTimeout,
    WatchdogPing,
    Ready,
    Timeout,
    Masked,
    Unmasked,
    Enabled,
    Disabled,
    TimerFired,
    SocketActivated,
    ExecFailed,
}

impl Event {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::DepFailed => "dep-failed",
            Self::Registered => "registered",
            Self::TargetChanged => "target-changed",
            Self::Reloaded => "reloaded",
            Self::Shutdown => "shutdown",
            Self::BurstLimit => "burst-limit",
            Self::WatchdogTimeout => "watchdog-timeout",
            Self::WatchdogPing => "watchdog-ping",
            Self::Ready => "ready",
            Self::Timeout => "timeout",
            Self::Masked => "masked",
            Self::Unmasked => "unmasked",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::TimerFired => "timer-fired",
            Self::SocketActivated => "socket-activated",
            Self::ExecFailed => "exec-failed",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Started => "+",
            Self::Stopped => "-",
            Self::Exited => "x",
            Self::Failed => "!",
            Self::DepFailed => "d",
            Self::Registered => "r",
            Self::TargetChanged => "t",
            Self::Reloaded => "R",
            Self::Shutdown => "S",
            Self::BurstLimit => "B",
            Self::WatchdogTimeout => "W",
            Self::WatchdogPing => "w",
            Self::Ready => "N",
            Self::Timeout => "T",
            Self::Masked => "M",
            Self::Unmasked => "U",
            Self::Enabled => "e",
            Self::Disabled => "D",
            Self::TimerFired => "F",
            Self::SocketActivated => "A",
            Self::ExecFailed => "E",
        }
    }

    pub fn severity(self) -> u8 {
        match self {
            Self::Started | Self::Stopped | Self::Registered
            | Self::Enabled | Self::Disabled | Self::WatchdogPing => 0, // info
            Self::Exited | Self::TargetChanged | Self::Reloaded
            | Self::Ready | Self::Masked | Self::Unmasked => 1, // notice
            Self::Failed | Self::DepFailed | Self::Timeout => 2, // warning
            Self::BurstLimit | Self::WatchdogTimeout | Self::Shutdown => 3, // critical
            Self::TimerFired | Self::SocketActivated => 0, // info
            Self::ExecFailed => 2, // warning
        }
    }
}

#[derive(Clone, Copy)]
pub struct JournalEntry {
    pub tick: u64,
    pub event: Event,
    pub service: &'static str,
    pub pid: u64,
    pub code: u64,
    pub valid: bool,
}

impl JournalEntry {
    const fn empty() -> Self {
        Self {
            tick: 0,
            event: Event::Started,
            service: "",
            pid: 0,
            code: 0,
            valid: false,
        }
    }
}

struct Journal {
    entries: [JournalEntry; JOURNAL_SIZE],
    head: usize,
    count: usize,
}

impl Journal {
    const fn new() -> Self {
        Self {
            entries: [JournalEntry::empty(); JOURNAL_SIZE],
            head: 0,
            count: 0,
        }
    }

    fn push(&mut self, entry: JournalEntry) {
        self.entries[self.head] = entry;
        self.head = (self.head + 1) % JOURNAL_SIZE;
        if self.count < JOURNAL_SIZE {
            self.count += 1;
        }
    }

    fn iter(&self) -> JournalIter<'_> {
        let start = if self.count < JOURNAL_SIZE {
            0
        } else {
            self.head
        };
        JournalIter {
            journal: self,
            pos: start,
            remaining: self.count,
        }
    }

    fn last_n(&self, n: usize) -> JournalIter<'_> {
        let take = n.min(self.count);
        let skip = self.count - take;
        let start = if self.count < JOURNAL_SIZE {
            skip
        } else {
            (self.head + skip) % JOURNAL_SIZE
        };
        JournalIter {
            journal: self,
            pos: start,
            remaining: take,
        }
    }
}

pub struct JournalIter<'a> {
    journal: &'a Journal,
    pos: usize,
    remaining: usize,
}

impl<'a> Iterator for JournalIter<'a> {
    type Item = &'a JournalEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let entry = &self.journal.entries[self.pos];
        self.pos = (self.pos + 1) % JOURNAL_SIZE;
        self.remaining -= 1;
        Some(entry)
    }
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal::new());
static TOTAL_EVENTS: AtomicUsize = AtomicUsize::new(0);

pub fn log(event: Event, service: &'static str, pid: u64, code: u64) {
    let tick = crate::interrupts::get_tick();
    let entry = JournalEntry {
        tick,
        event,
        service,
        pid,
        code,
        valid: true,
    };
    JOURNAL.lock().push(entry);
    TOTAL_EVENTS.fetch_add(1, Ordering::Relaxed);
}

pub fn recent(n: usize) -> alloc::vec::Vec<JournalEntry> {
    let journal = JOURNAL.lock();
    journal.last_n(n).copied().collect()
}

pub fn all_entries() -> alloc::vec::Vec<JournalEntry> {
    let journal = JOURNAL.lock();
    journal.iter().copied().collect()
}

pub fn total_events() -> usize {
    TOTAL_EVENTS.load(Ordering::Relaxed)
}

pub fn entries_for_service(name: &str) -> alloc::vec::Vec<JournalEntry> {
    let journal = JOURNAL.lock();
    journal.iter().filter(|e| e.service == name).copied().collect()
}

pub fn entries_by_severity(min_severity: u8) -> alloc::vec::Vec<JournalEntry> {
    let journal = JOURNAL.lock();
    journal.iter().filter(|e| e.event.severity() >= min_severity).copied().collect()
}

pub fn clear() {
    let mut journal = JOURNAL.lock();
    *journal = Journal::new();
    TOTAL_EVENTS.store(0, Ordering::Relaxed);
}
