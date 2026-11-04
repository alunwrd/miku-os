// Descriptors: one table per thread group, and the tables themselves
// (`dev/vfs-locking.md`, "4a")
//
// In shards by thread group, each a lock of its own: a leaf, under which
// nothing else is taken. A descriptor also remembers what it is to the syscall
// layer (`Route`), worked out once under the VFS lock, so that routing a call
// never needs it again.

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{fence, AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

use crate::fs::vfs::types::*;
use crate::kcore::sched_lock::{SchedMutex, SchedMutexGuard};

/// What the syscall layer does with a descriptor (`classify`). A property of
/// the vnode the descriptor holds, so it cannot change while the descriptor
/// is open; a new descriptor starts `Unknown`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    /// Not worked out yet (`MikuVFS::route_of`)
    Unknown,
    /// What the VFS answers: a file, a directory, a device. `cached` when
    /// the file's pages go through the disk page cache (`core/cached.rs`)
    Vfs { cached: bool },
    Pipe,
    Inotify,
    /// `/dev/net0`
    NetRaw,
    /// `/dev/tty`: the caller's controlling terminal, whichever it is now
    Tty,
    Console,
    PtySlave(u32),
    PtyMaster(u32),
}

#[derive(Clone, Copy)]
pub struct OpenFile {
    pub vnode_id: InodeId,
    pub flags: OpenFlags,
    pub offset: u64,
    pub active: bool,
    pub route: Route,
}

impl OpenFile {
    pub const fn empty() -> Self {
        Self {
            vnode_id: INVALID_ID,
            flags: OpenFlags::EMPTY,
            offset: 0,
            active: false,
            route: Route::Unknown,
        }
    }

    const fn open(vnode_id: InodeId, flags: OpenFlags, offset: u64) -> Self {
        Self { vnode_id, flags, offset, active: true, route: Route::Unknown }
    }

    pub fn readable(&self) -> bool {
        self.flags.readable()
    }
    pub fn writable(&self) -> bool {
        self.flags.writable()
    }
}

// A slot
//
// What a descriptor names changes only under its shard's lock, inside `seq`:
// odd while a writer is in the slot. A reader with no lock copies the fields
// and reads `seq` again, and starts over if it moved. The offset is `pos`, with
// the slot's generation in its high bits: every open, dup or close of the slot
// moves it, so a compare-and-swap of an offset read before the descriptor was
// closed and its number reused fails, and the new file's offset stays
// (`dev/vfs-locking.md`, "Descriptors read with no lock")

/// Bits of `pos` that are the offset: 256 TiB, past any file here
const OFF_BITS: u32 = 48;
pub const MAX_OFFSET: u64 = (1 << OFF_BITS) - 1;

#[inline]
fn pack(gen: u64, off: u64) -> u64 {
    (gen << OFF_BITS) | off.min(MAX_OFFSET)
}
#[inline]
fn gen_of(pos: u64) -> u64 {
    pos >> OFF_BITS
}
#[inline]
fn off_of(pos: u64) -> u64 {
    pos & MAX_OFFSET
}

fn route_word(r: Route) -> u64 {
    match r {
        Route::Unknown => 0,
        Route::Vfs { cached: false } => 1,
        Route::Vfs { cached: true } => 2,
        Route::Pipe => 3,
        Route::Inotify => 4,
        Route::NetRaw => 5,
        Route::Tty => 6,
        Route::Console => 7,
        Route::PtySlave(n) => 8 | (n as u64) << 8,
        Route::PtyMaster(n) => 9 | (n as u64) << 8,
    }
}

fn route_of(w: u64) -> Route {
    match w & 0xFF {
        1 => Route::Vfs { cached: false },
        2 => Route::Vfs { cached: true },
        3 => Route::Pipe,
        4 => Route::Inotify,
        5 => Route::NetRaw,
        6 => Route::Tty,
        7 => Route::Console,
        8 => Route::PtySlave((w >> 8) as u32),
        9 => Route::PtyMaster((w >> 8) as u32),
        _ => Route::Unknown,
    }
}

struct Slot {
    seq: AtomicU32,
    active: AtomicBool,
    vnode: AtomicU32,
    flags: AtomicU32,
    route: AtomicU64,
    pos: AtomicU64,
}

impl Slot {
    const fn empty() -> Self {
        Self {
            seq: AtomicU32::new(0),
            active: AtomicBool::new(false),
            vnode: AtomicU32::new(INVALID_ID),
            flags: AtomicU32::new(OpenFlags::EMPTY.0),
            route: AtomicU64::new(0),
            pos: AtomicU64::new(0),
        }
    }

    /// A consistent copy, and the `pos` word it was read with
    fn read(&self) -> (OpenFile, u64) {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            let active = self.active.load(Ordering::Relaxed);
            let vnode_id = self.vnode.load(Ordering::Relaxed);
            let flags = OpenFlags(self.flags.load(Ordering::Relaxed));
            let route = route_of(self.route.load(Ordering::Relaxed));
            let pos = self.pos.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return (OpenFile { vnode_id, flags, offset: off_of(pos), active, route }, pos);
            }
        }
    }

    /// Under the shard: replaces what the slot names, and with `fresh` (a new
    /// descriptor) the offset too, moving the generation
    fn write(&self, f: &OpenFile, fresh: bool) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.active.store(f.active, Ordering::Relaxed);
        self.vnode.store(f.vnode_id, Ordering::Relaxed);
        self.flags.store(f.flags.0, Ordering::Relaxed);
        self.route.store(route_word(f.route), Ordering::Relaxed);
        if fresh {
            let gen = (gen_of(self.pos.load(Ordering::Relaxed)) + 1) & 0xFFFF;
            self.pos.store(pack(gen, f.offset), Ordering::Relaxed);
        }
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// The offset moved by `delta` from wherever it is now, by swap: an
    /// unlocked read may be moving it too
    fn shift(&self, delta: i64) {
        let mut pos = self.pos.load(Ordering::Relaxed);
        loop {
            let off = (off_of(pos) as i64).saturating_add(delta).max(0) as u64;
            match self.pos.compare_exchange_weak(pos, pack(gen_of(pos), off), Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return,
                Err(now) => pos = now,
            }
        }
    }
}

/// A descriptor, copied out of its slot
pub struct Snap(OpenFile);

impl Deref for Snap {
    type Target = OpenFile;
    fn deref(&self) -> &OpenFile {
        &self.0
    }
}

/// A descriptor to change, under the shard. Written back when it goes: what
/// it names inside the slot's sequence count, a change of offset as a move
/// by the same amount (`Slot::shift`)
pub struct EntryMut<'a> {
    slot: &'a Slot,
    was: OpenFile,
    now: OpenFile,
}

impl Deref for EntryMut<'_> {
    type Target = OpenFile;
    fn deref(&self) -> &OpenFile {
        &self.now
    }
}

impl DerefMut for EntryMut<'_> {
    fn deref_mut(&mut self) -> &mut OpenFile {
        &mut self.now
    }
}

impl Drop for EntryMut<'_> {
    fn drop(&mut self) {
        let (a, b) = (&self.was, &self.now);
        if a.vnode_id != b.vnode_id || a.flags != b.flags || a.active != b.active || a.route != b.route {
            self.slot.write(b, false);
        }
        if a.offset != b.offset {
            self.slot.shift(b.offset.wrapping_sub(a.offset) as i64);
        }
    }
}

pub struct FdTable {
    files: [Slot; MAX_OPEN_FILES],
}

impl FdTable {
    pub const fn new() -> Self {
        Self { files: [const { Slot::empty() }; MAX_OPEN_FILES] }
    }

    /// An empty table, made in place: `Box::new(FdTable::new())` builds its 16
    /// KiB on the stack first, and `fork`'s copy the same
    pub fn boxed() -> Box<FdTable> {
        let mut b = Box::<FdTable>::new_uninit();
        // SAFETY: a place in the allocation, not read
        let files = unsafe { core::ptr::addr_of_mut!((*b.as_mut_ptr()).files) } as *mut Slot;
        for i in 0..MAX_OPEN_FILES {
            // SAFETY: inside the array, one slot at a time
            unsafe { files.add(i).write(Slot::empty()) };
        }
        // SAFETY: every slot written above
        unsafe { b.assume_init() }
    }

    /// The first free slot from `from`, with `f` in it
    fn install_from(&mut self, from: usize, f: OpenFile) -> VfsResult<usize> {
        for i in from..MAX_OPEN_FILES {
            if !self.files[i].active.load(Ordering::Relaxed) {
                self.files[i].write(&f, true);
                return Ok(i);
            }
        }
        Err(VfsError::TooManyOpenFiles)
    }

    pub fn alloc(&mut self, vnode_id: InodeId, flags: OpenFlags) -> VfsResult<usize> {
        // Keep ordinary VFS fds out of the stdio range reserved by syscalls
        self.install_from(RESERVED_STDIO_FDS, OpenFile::open(vnode_id, flags, 0))
    }

    pub fn alloc_at(&mut self, fd: usize, vnode_id: InodeId, flags: OpenFlags) -> VfsResult<usize> {
        if fd >= MAX_OPEN_FILES {
            return Err(VfsError::BadFd);
        }
        if self.files[fd].active.load(Ordering::Relaxed) {
            return Err(VfsError::Busy);
        }
        self.files[fd].write(&OpenFile::open(vnode_id, flags, 0), true);
        Ok(fd)
    }

    /// The lowest free slot at or above `floor`, for `fcntl(F_DUPFD)`.
    ///
    /// The floor is clamped up to the reserved stdio range, not refused:
    /// "anywhere from 0" means anywhere, and fd 0 would take the terminal from
    /// the program
    pub fn alloc_from(
        &mut self,
        floor: usize,
        vnode_id: InodeId,
        flags: OpenFlags,
    ) -> VfsResult<usize> {
        self.install_from(floor.max(RESERVED_STDIO_FDS), OpenFile::open(vnode_id, flags, 0))
    }

    pub fn dup(&mut self, old_fd: usize) -> VfsResult<usize> {
        let file = *self.get(old_fd)?;
        // Preserve the same reserved range for duplicated descriptors
        self.install_from(RESERVED_STDIO_FDS, OpenFile::open(file.vnode_id, file.flags, file.offset))
    }

    /// Every open descriptor, copied
    pub fn iter(&self) -> impl Iterator<Item = OpenFile> + '_ {
        self.files.iter().map(|s| s.read().0).filter(|f| f.active)
    }

    /// Does this table hold a writable descriptor on `vnode_id`?
    /// Tests the WRITE bit alone: RDWR is READ|WRITE, so `has(RDWR)` is
    /// also true for a read-only fd
    pub fn any_writer_on(&self, vnode_id: usize) -> bool {
        self.iter().any(|f| f.vnode_id as usize == vnode_id && f.flags.has(OpenFlags::WRITE))
    }

    /// Whether any descriptor reads `vnode_id`: a pipe needs both
    ///
    /// No writers means the reader is at end of input; no readers means the
    /// writer gets SIGPIPE
    pub fn any_reader_on(&self, vnode_id: usize) -> bool {
        self.iter().any(|f| f.vnode_id as usize == vnode_id && f.flags.has(OpenFlags::READ))
    }

    pub fn get(&self, fd: usize) -> VfsResult<Snap> {
        if fd < MAX_OPEN_FILES {
            let (f, _) = self.files[fd].read();
            if f.active {
                return Ok(Snap(f));
            }
        }
        Err(VfsError::BadFd)
    }

    pub fn get_mut(&mut self, fd: usize) -> VfsResult<EntryMut<'_>> {
        if fd < MAX_OPEN_FILES {
            let (f, _) = self.files[fd].read();
            if f.active {
                return Ok(EntryMut { slot: &self.files[fd], was: f, now: f });
            }
        }
        Err(VfsError::BadFd)
    }

    pub fn close(&mut self, fd: usize) -> VfsResult<InodeId> {
        let vid = self.get(fd)?.vnode_id;
        self.files[fd].write(&OpenFile::empty(), true);
        Ok(vid)
    }

    pub fn open_count(&self) -> usize {
        self.iter().count()
    }

    pub fn close_all(&mut self) -> usize {
        let mut count = 0;
        for s in self.files.iter() {
            if s.active.load(Ordering::Relaxed) {
                s.write(&OpenFile::empty(), true);
                count += 1;
            }
        }
        count
    }

    /// Every descriptor flagged close-on-exec closed: the vnodes they named
    pub fn close_cloexec(&mut self) -> Vec<InodeId> {
        let mut gone = Vec::new();
        for s in self.files.iter() {
            let (f, _) = s.read();
            if f.active && f.flags.has(OpenFlags::CLOEXEC) {
                gone.push(f.vnode_id);
                s.write(&OpenFile::empty(), true);
            }
        }
        gone
    }

    /// A copy, for `fork`
    fn copy(&self) -> Box<FdTable> {
        let t = FdTable::boxed();
        for (s, d) in self.files.iter().zip(t.files.iter()) {
            let (f, _) = s.read();
            if f.active {
                d.write(&f, true);
            }
        }
        t
    }
}

// The tables

/// Shards the tables are spread over, by thread group: two programs meet on
/// one lock only when their tgids are equal modulo this
const SHARDS: usize = 64;

type Tables = BTreeMap<u64, Box<FdTable>>;

struct Shard {
    tables: SchedMutex<Tables>,
    /// The task holding `tables` plus one, 0 for none
    owner: AtomicU64,
}

impl Shard {
    const fn new() -> Self {
        Self { tables: SchedMutex::named("fd", BTreeMap::new()), owner: AtomicU64::new(0) }
    }
}

static SHARD: [Shard; SHARDS] = [const { Shard::new() }; SHARDS];

fn shard_of(tgid: u64) -> &'static Shard {
    &SHARD[tgid as usize % SHARDS]
}

/// A shard, held. Nothing else is taken while one is: a task taking the same
/// shard twice would wait for itself, so that panics here
struct Held {
    guard: SchedMutexGuard<'static, Tables>,
    shard: &'static Shard,
}

#[track_caller]
fn hold(shard: &'static Shard) -> Held {
    let me = crate::sched::current_pid() + 1;
    if shard.owner.load(Ordering::Relaxed) == me {
        panic!("[fd] pid {} took its descriptor table's lock twice", me - 1);
    }
    let guard = shard.tables.lock();
    shard.owner.store(me, Ordering::Relaxed);
    Held { guard, shard }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.shard.owner.store(0, Ordering::Relaxed);
    }
}

/// One thread group's table, held: `MikuVFS::fds()` and the syscall layer
/// both use it as the table itself. For a statement or a short block
pub struct Fds {
    table: *mut FdTable,
    _held: Held,
}

impl Deref for Fds {
    type Target = FdTable;
    fn deref(&self) -> &FdTable {
        // SAFETY: a box in the map, which nobody changes while it is held
        unsafe { &*self.table }
    }
}

impl DerefMut for Fds {
    fn deref_mut(&mut self) -> &mut FdTable {
        // SAFETY: as above, and this is the only holder
        unsafe { &mut *self.table }
    }
}

/// Thread group `tgid`'s table, made empty if it has none
#[track_caller]
pub fn table(tgid: u64) -> Fds {
    let mut held = hold(shard_of(tgid));
    let table: *mut FdTable = &mut **held.guard.entry(tgid).or_insert_with(|| {
        let t = FdTable::boxed();
        publish(tgid, &*t);
        t
    });
    Fds { table, _held: held }
}

/// The calling thread's group's table
#[track_caller]
pub fn current() -> Fds {
    table(crate::sched::current_tgid())
}

/// `child`'s table, a copy of `parent`'s: what `fork` gives. Returns the
/// vnodes the copy's descriptors name, each of which the caller takes one
/// more reference on
pub fn fork(parent: u64, child: u64) -> Vec<InodeId> {
    let copy = table(parent).copy();
    let vids = copy.iter().map(|f| f.vnode_id).collect();
    publish(child, &*copy);
    hold(shard_of(child)).guard.insert(child, copy);
    reap_retired();
    vids
}

/// Thread group `tgid`'s table, taken out: it has exited. The vnodes its
/// descriptors named; the table itself is freed once no reader without the
/// lock can still be in it (`retire`)
pub fn remove(tgid: u64) -> Option<Vec<InodeId>> {
    let t = hold(shard_of(tgid)).guard.remove(&tgid)?;
    unpublish(tgid, &*t);
    let vids = t.iter().map(|f| f.vnode_id).collect();
    retire(t);
    Some(vids)
}

/// Whether any descriptor anywhere names `vid`
pub fn any_open(vid: usize) -> bool {
    SHARD.iter().any(|s| hold(s).guard.values().any(|t| t.iter().any(|f| f.vnode_id as usize == vid)))
}

// With no lock

/// Each thread group's table, by tgid, for the readers that take no shard
static TABLES: [AtomicPtr<FdTable>; crate::sched::MAX_PROCS] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; crate::sched::MAX_PROCS];

fn publish(tgid: u64, t: &FdTable) {
    if let Some(p) = TABLES.get(tgid as usize) {
        p.store(t as *const FdTable as *mut FdTable, Ordering::Release);
    }
}

fn unpublish(tgid: u64, t: &FdTable) {
    if let Some(p) = TABLES.get(tgid as usize) {
        let _ = p.compare_exchange(t as *const FdTable as *mut FdTable, core::ptr::null_mut(), Ordering::AcqRel, Ordering::Relaxed);
    }
}

/// The calling thread group's slot `fd`, with no lock: `None` when the group
/// has no table published (a tgid past `TABLES`)
fn slot(fd: usize) -> Option<&'static Slot> {
    if fd >= MAX_OPEN_FILES {
        return None;
    }
    let t = TABLES.get(crate::sched::current_tgid() as usize)?.load(Ordering::Acquire);
    // SAFETY: a published table is freed only after every CPU has switched
    // tasks since it was unpublished (`retire`); this caller is in a syscall
    // of a thread of the group, does not sleep while it holds the reference,
    // and ring 0 is not preempted
    unsafe { t.as_ref() }.map(|t| &t.files[fd])
}

/// Descriptor `fd` of the calling thread group, copied with no lock
pub fn peek(fd: usize) -> Option<OpenFile> {
    peek_pos(fd).map(|(f, _)| f)
}

/// `peek`, and the slot's offset word as it was read: what `advance` swaps
/// from. It carries the slot's generation, so it names this descriptor and
/// not whatever is opened at the same number later
pub fn peek_pos(fd: usize) -> Option<(OpenFile, u64)> {
    match slot(fd) {
        Some(s) => Some(s.read()).filter(|(f, _)| f.active),
        None => current().get(fd).ok().map(|f| (*f, f.offset)),
    }
}

/// `fd`'s offset moved to `to` with no lock, if the slot is still at `pos`
/// (`peek_pos`): the same descriptor, at the same offset. Whether it moved
pub fn advance(fd: usize, vid: InodeId, pos: u64, to: u64) -> bool {
    let Some(s) = slot(fd) else {
        let mut t = current();
        return match t.get_mut(fd) {
            Ok(mut f) if f.vnode_id == vid && f.offset == off_of(pos) => {
                f.offset = to;
                true
            }
            _ => false,
        };
    };
    s.pos.compare_exchange(pos, pack(gen_of(pos), to), Ordering::AcqRel, Ordering::Relaxed).is_ok()
}

/// `fd`'s offset set by `f` from what it is, with no lock: `lseek` from the
/// start or from where it is. The new offset, or what `f` refused
pub fn seek(fd: usize, f: impl Fn(u64) -> VfsResult<u64>) -> VfsResult<u64> {
    let Some(s) = slot(fd) else {
        let mut t = current();
        let mut e = t.get_mut(fd)?;
        let to = f(e.offset)?;
        e.offset = to;
        return Ok(to);
    };
    loop {
        let (file, pos) = s.read();
        if !file.active {
            return Err(VfsError::BadFd);
        }
        let to = f(off_of(pos))?;
        if to > MAX_OFFSET {
            return Err(VfsError::InvalidArgument);
        }
        if s.pos.compare_exchange(pos, pack(gen_of(pos), to), Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            return Ok(to);
        }
    }
}

// The grace period

/// A table taken out, and each CPU's switch count when it was
struct Retired {
    table: Box<FdTable>,
    seen: [u64; crate::arch::x86_64::percpu::MAX_CPUS],
}

static RETIRED: SchedMutex<Vec<Retired>> = SchedMutex::named("fd-retired", Vec::new());

/// `t` is unpublished: freed when every CPU has switched tasks since, or is
/// idle, so no reader without the lock can still be in it
fn retire(t: Box<FdTable>) {
    use crate::arch::x86_64::percpu;
    let mut seen = [0u64; percpu::MAX_CPUS];
    for (i, s) in seen.iter_mut().enumerate().take(percpu::cpu_count()) {
        *s = percpu::get(i).total_switches.load(Ordering::Acquire);
    }
    RETIRED.lock().push(Retired { table: t, seen });
    reap_retired();
}

fn reap_retired() {
    use crate::arch::x86_64::percpu;
    let me = percpu::current_index();
    let past = |r: &Retired| {
        (0..percpu::cpu_count()).all(|i| {
            let c = percpu::get(i);
            i == me
                || c.online.load(Ordering::Relaxed) == 0
                || c.total_switches.load(Ordering::Acquire) != r.seen[i]
                || c.current_pid.load(Ordering::Relaxed) == c.idle_pid.load(Ordering::Relaxed)
        })
    };
    // Freed outside the lock
    let done: Vec<Retired> = {
        let mut list = RETIRED.lock();
        let mut done = Vec::new();
        let mut i = 0;
        while i < list.len() {
            if past(&list[i]) {
                done.push(list.swap_remove(i));
            } else {
                i += 1;
            }
        }
        done
    };
    drop(done);
}
