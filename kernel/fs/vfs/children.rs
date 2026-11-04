// A directory's names (`dev/vfs-dentries-inodes.md`, "Stage 5: dentries and
// inodes")
//
// Linux keeps a dentry for every name it has looked up, and a negative one for
// every name it looked up and did not find. Here a name is an entry in its
// directory's table, one of three kinds:
//
// - positive: the name, and the vnode it names;
// - negative: a name the filesystem does not have. The next lookup of it is
//   answered here;
// - whiteout: a name still on the disk whose file was unlinked while open. The
//   disk keeps the name until the last close; without this a lookup or a
//   listing would find it there and make the file again.
//
// `find_by_name`, `iter`, `len` and `is_empty` see positive entries only;
// `probe` tells a known absence from a name never asked.
//
// Open addressing with tombstones. Negative entries come and go far more often
// than files, so tombstones are counted and the table is rebuilt in place when
// they and the entries fill three quarters of it.
//
// A listing reads the positive entries in slot order, and `getdents` asks for
// them by position (`docs/MikuOS_ABI.md`). The table remembers where the last
// pass stopped (position and slot) for as long as its positive entries have not
// moved (`gen`), and the next pass from that position starts there; otherwise a
// directory read in passes costs the square of its size.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use crate::fs::vfs::hash::name_hash;
use crate::fs::vfs::types::{InodeId, INVALID_ID};

const INITIAL_CAPACITY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SlotState {
    Empty     = 0,
    Occupied  = 1,
    Tombstone = 2,
}

/// What an occupied slot says about its name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryKind {
    Positive = 0,
    Negative = 1,
    Whiteout = 2,
}

/// What a directory's table knows about one name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// It names this vnode
    Node(InodeId),
    /// Known not to be there: a negative entry or a whiteout
    Absent,
    /// The table has no record of it
    Unknown,
}

/// One entry: a name, and the vnode it names.
///
/// The name is boxed and whole, so two files sharing a prefix cannot resolve to
/// one vnode. Boxed, not widened to 255: a directory of four thousand children
/// would cost two megabytes of slots for names averaging a dozen bytes.
#[derive(Clone)]
pub struct ChildSlot {
    pub hash:  u32,
    /// The vnode, for a positive entry; `INVALID_ID` for the other two
    pub id:    InodeId,
    pub state: SlotState,
    pub kind:  EntryKind,
    name:      Option<Box<str>>,
}

impl ChildSlot {
    pub const EMPTY: Self = Self {
        hash:  0,
        id:    INVALID_ID,
        state: SlotState::Empty,
        kind:  EntryKind::Positive,
        name:  None,
    };

    fn new(hash: u32, name: &str, id: InodeId, kind: EntryKind) -> Self {
        Self { hash, id, state: SlotState::Occupied, kind, name: Some(name.into()) }
    }

    #[inline]
    pub fn used(&self) -> bool {
        self.state == SlotState::Occupied && self.kind == EntryKind::Positive
    }

    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }

    /// Whole name against whole name. No prefix, no "close enough"
    pub fn name_matches(&self, name: &str) -> bool {
        self.name() == name
    }
}

pub struct Children {
    pub slots: Vec<ChildSlot>,
    /// Positive entries
    pub count: u32,
    /// Negative entries and whiteouts
    negatives: u32,
    whiteouts: u32,
    tombstones: u32,
    /// Moves whenever a positive entry is added, removed or moved; makes a
    /// listing's resume point stale
    gen: u32,
    /// Where the last listing stopped: position << 32 | slot, and `gen` then.
    /// Atomics: a `getdents` under the VFS lock held to read writes them beside
    /// others (`dev/vfs-locking.md`, "4c"). Those writers all see one `gen`, as
    /// nothing changes the directory while they hold the lock, so any pair one
    /// of them wrote is right; the pair is stored before the `gen` it is for,
    /// and read after it
    resume: core::sync::atomic::AtomicU64,
    resume_gen: core::sync::atomic::AtomicU32,
}

impl Children {
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            count: 0,
            negatives: 0,
            whiteouts: 0,
            tombstones: 0,
            gen: 0,
            resume: core::sync::atomic::AtomicU64::new(0),
            resume_gen: core::sync::atomic::AtomicU32::new(u32::MAX),
        }
    }

    #[inline]
    fn cap(&self) -> usize {
        self.slots.len()
    }

    /// Room for one more: the table made, or rebuilt when its entries and
    /// tombstones fill three quarters of it; twice as large when the entries
    /// alone fill half, otherwise the same size
    fn make_room(&mut self) {
        if self.slots.is_empty() {
            self.slots.resize(INITIAL_CAPACITY, ChildSlot::EMPTY);
            return;
        }
        let live = (self.count + self.negatives) as usize;
        if (live + self.tombstones as usize) * 4 >= self.cap() * 3 {
            let cap = if live * 2 >= self.cap() { self.cap() * 2 } else { self.cap() };
            self.rebuild(cap, |_| true);
        }
    }

    /// Every entry `keep` keeps, into a table of `cap` slots with no
    /// tombstones. `take`: a slot owns its name, and rebuilding moves it
    fn rebuild(&mut self, cap: usize, keep: impl Fn(&ChildSlot) -> bool) {
        let mut slots = Vec::with_capacity(cap);
        slots.resize(cap, ChildSlot::EMPTY);
        let (mut count, mut negatives, mut whiteouts) = (0, 0, 0);
        for slot in self.slots.iter_mut() {
            if slot.state != SlotState::Occupied || !keep(slot) {
                continue;
            }
            let moved = core::mem::replace(slot, ChildSlot::EMPTY);
            match moved.kind {
                EntryKind::Positive => count += 1,
                EntryKind::Negative => negatives += 1,
                EntryKind::Whiteout => {
                    negatives += 1;
                    whiteouts += 1;
                }
            }
            let start = moved.hash as usize % cap;
            for i in 0..cap {
                let idx = (start + i) % cap;
                if slots[idx].state == SlotState::Empty {
                    slots[idx] = moved;
                    break;
                }
            }
        }
        self.slots = slots;
        self.count = count;
        self.negatives = negatives;
        self.whiteouts = whiteouts;
        self.tombstones = 0;
        self.gen = self.gen.wrapping_add(1);
    }

    /// The slot holding `name`, of any kind
    fn slot_of(&self, h: u32, name: &str) -> Option<usize> {
        let cap = self.cap();
        if cap == 0 {
            return None;
        }
        let start = h as usize % cap;
        for i in 0..cap {
            let idx = (start + i) % cap;
            match self.slots[idx].state {
                SlotState::Empty => return None,
                SlotState::Tombstone => continue,
                SlotState::Occupied => {
                    if self.slots[idx].hash == h && self.slots[idx].name_matches(name) {
                        return Some(idx);
                    }
                }
            }
        }
        None
    }

    /// A new slot for a name that has none: the first tombstone on its
    /// chain, or the empty slot that ends it
    fn put(&mut self, h: u32, name: &str, id: InodeId, kind: EntryKind) -> bool {
        self.make_room();
        let cap = self.cap();
        let start = h as usize % cap;
        let mut at = None;
        for i in 0..cap {
            let idx = (start + i) % cap;
            match self.slots[idx].state {
                SlotState::Empty => {
                    at = at.or(Some(idx));
                    break;
                }
                SlotState::Tombstone => {
                    at = at.or(Some(idx));
                }
                SlotState::Occupied => {}
            }
        }
        let Some(idx) = at else { return false };
        if self.slots[idx].state == SlotState::Tombstone {
            self.tombstones -= 1;
        }
        self.slots[idx] = ChildSlot::new(h, name, id, kind);
        match kind {
            EntryKind::Positive => {
                self.count += 1;
                self.gen = self.gen.wrapping_add(1);
            }
            EntryKind::Negative => self.negatives += 1,
            EntryKind::Whiteout => {
                self.negatives += 1;
                self.whiteouts += 1;
            }
        }
        true
    }

    fn bury(&mut self, idx: usize) {
        match self.slots[idx].kind {
            EntryKind::Positive => {
                self.count -= 1;
                self.gen = self.gen.wrapping_add(1);
            }
            EntryKind::Negative => self.negatives -= 1,
            EntryKind::Whiteout => {
                self.negatives -= 1;
                self.whiteouts -= 1;
            }
        }
        self.slots[idx] = ChildSlot::EMPTY;
        self.slots[idx].state = SlotState::Tombstone;
        self.tombstones += 1;
    }

    /// Moved by every name that comes or goes (positive entries): a read of the
    /// directory made with the VFS lock released goes in only if this has not
    /// moved since it was planned (`core/fill.rs`, `Deferred`)
    pub fn gen(&self) -> u32 {
        self.gen
    }

    /// `name` names `id`. A negative entry for it becomes positive; a positive
    /// one or a whiteout already there refuses
    pub fn insert(&mut self, name: &str, id: InodeId) -> bool {
        let h = name_hash(name);
        if let Some(idx) = self.slot_of(h, name) {
            if self.slots[idx].kind != EntryKind::Negative {
                return false;
            }
            self.slots[idx].kind = EntryKind::Positive;
            self.slots[idx].id = id;
            self.negatives -= 1;
            self.count += 1;
            self.gen = self.gen.wrapping_add(1);
            return true;
        }
        self.put(h, name, id, EntryKind::Positive)
    }

    /// `name` is not in the directory. Does nothing when the table already has
    /// an entry for it
    pub fn insert_negative(&mut self, name: &str) -> bool {
        let h = name_hash(name);
        self.slot_of(h, name).is_none() && self.put(h, name, INVALID_ID, EntryKind::Negative)
    }

    /// `name` is still on the disk and must not be found there: its file
    /// was unlinked while open. Over a negative entry, or a new one
    pub fn insert_whiteout(&mut self, name: &str) -> bool {
        let h = name_hash(name);
        match self.slot_of(h, name) {
            Some(idx) if self.slots[idx].kind == EntryKind::Negative => {
                self.slots[idx].kind = EntryKind::Whiteout;
                self.whiteouts += 1;
                true
            }
            Some(_) => false,
            None => self.put(h, name, INVALID_ID, EntryKind::Whiteout),
        }
    }

    /// The whiteout for `name` gone: the disk has let go of the name
    pub fn remove_whiteout(&mut self, name: &str) -> bool {
        match self.slot_of(name_hash(name), name) {
            Some(idx) if self.slots[idx].kind == EntryKind::Whiteout => {
                self.bury(idx);
                true
            }
            _ => false,
        }
    }

    pub fn probe(&self, name: &str) -> Probe {
        match self.slot_of(name_hash(name), name) {
            Some(idx) if self.slots[idx].kind == EntryKind::Positive => Probe::Node(self.slots[idx].id),
            Some(_) => Probe::Absent,
            None => Probe::Unknown,
        }
    }

    /// Whether `name` has a whiteout: still on the disk, and not to be seen
    pub fn is_whiteout(&self, name: &str) -> bool {
        matches!(self.slot_of(name_hash(name), name), Some(idx) if self.slots[idx].kind == EntryKind::Whiteout)
    }

    pub fn find_by_name(&self, name: &str) -> Option<InodeId> {
        match self.probe(name) {
            Probe::Node(id) => Some(id),
            _ => None,
        }
    }

    /// A positive entry out, by its name
    pub fn remove_by_name(&mut self, name: &str) -> Option<InodeId> {
        let idx = self.slot_of(name_hash(name), name)?;
        if self.slots[idx].kind != EntryKind::Positive {
            return None;
        }
        let id = self.slots[idx].id;
        self.bury(idx);
        Some(id)
    }

    /// A positive entry out, by its name's hash and the vnode it names
    pub fn remove(&mut self, h: u32, id: InodeId) -> bool {
        let cap = self.cap();
        if cap == 0 {
            return false;
        }
        let start = h as usize % cap;
        for i in 0..cap {
            let idx = (start + i) % cap;
            match self.slots[idx].state {
                SlotState::Empty => return false,
                SlotState::Tombstone => continue,
                SlotState::Occupied => {
                    if self.slots[idx].used() && self.slots[idx].hash == h && self.slots[idx].id == id {
                        self.bury(idx);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Every negative entry gone, whiteouts kept: too many of them, or the
    /// directory is complete and answers a miss without them
    pub fn drop_negatives(&mut self) {
        if self.negatives > self.whiteouts {
            let cap = self.cap();
            self.rebuild(cap, |s| s.kind != EntryKind::Negative);
        }
    }

    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = ChildSlot::EMPTY;
        }
        self.count = 0;
        self.negatives = 0;
        self.whiteouts = 0;
        self.tombstones = 0;
        self.gen = self.gen.wrapping_add(1);
    }

    pub fn is_empty(&self) -> bool { self.count == 0 }
    pub fn len(&self) -> usize { self.count as usize }
    /// Negative entries, whiteouts not counted
    pub fn negatives(&self) -> usize { (self.negatives - self.whiteouts) as usize }
    pub fn whiteouts(&self) -> usize { self.whiteouts as usize }

    /// Positive entries: (hash, vnode)
    pub fn iter(&self) -> impl Iterator<Item = (u32, InodeId)> + '_ {
        self.slots.iter().filter(|s| s.used()).map(|s| (s.hash, s.id))
    }

    /// Positive entries with their names: a hard link's name is its entry's,
    /// not the vnode's first name
    pub fn iter_named(&self) -> impl Iterator<Item = (&str, InodeId)> + '_ {
        self.slots.iter().filter(|s| s.used()).map(|s| (s.name(), s.id))
    }

    /// Positive entries from slot `from` on, with their slots
    pub fn named_from(&self, from: usize) -> impl Iterator<Item = (usize, &str, InodeId)> + '_ {
        self.slots.iter().enumerate().skip(from).filter(|(_, s)| s.used()).map(|(i, s)| (i, s.name(), s.id))
    }

    /// The slot to start at for listing position `pos`, if the last listing
    /// stopped exactly there and nothing has moved since
    pub fn resume_at(&self, pos: usize) -> Option<usize> {
        use core::sync::atomic::Ordering;
        if self.resume_gen.load(Ordering::Acquire) != self.gen {
            return None;
        }
        let w = self.resume.load(Ordering::Acquire);
        ((w >> 32) as usize == pos).then_some(w as u32 as usize)
    }

    /// The last listing stopped before position `pos`, which is at slot
    /// `slot` or after it
    pub fn stopped_at(&self, pos: usize, slot: usize) {
        use core::sync::atomic::Ordering;
        self.resume.store((pos as u64) << 32 | slot as u32 as u64, Ordering::Release);
        self.resume_gen.store(self.gen, Ordering::Release);
    }
}
