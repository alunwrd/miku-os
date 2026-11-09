// Pipes: a ring of bytes between two anonymous vnodes
//
// A pipe is a 64 KiB ring with a lock of its own, and two vnodes: one the read
// end's descriptors name, one the write end's. Two, so that a vnode's reference
// count is how many descriptors hold that end. The VFS keeps that count right
// through `fork`, `dup`, `close`, the close-on-exec sweep, exit and a
// descriptor passed over a socket: they all go through `VNode::inc_ref` and
// `dec_ref`, which tell `ref_changed` here. Bytes move under the pipe's lock
// only.
//
// Waiters on a pipe wait on one key for both ends (`wait_key`): a read wakes
// the writers, a write wakes the readers, and the last descriptor of one end
// going wakes the other (`sys_close`, `release_fds_on_exit`, which wake that
// key after the VFS lock is dropped).
//
// Named FIFOs on a disk are not this: opening one finds a pipe vnode with a
// filesystem behind it, and its bytes go to the disk.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

/// What one pipe holds before a writer waits: Linux's default
pub const CAPACITY: usize = 64 * 1024;

/// A write this size or smaller goes in whole or not at all (POSIX
/// `PIPE_BUF`): two writers' records never interleave
pub const ATOMIC_WRITE: usize = miku_abi::limits::PIPE_BUF;

struct Ring {
    /// Allocated by the first write: most pipes carry a line or two
    buf: Vec<u8>,
    head: usize,
    len: usize,
}

impl Ring {
    fn room(&self) -> usize {
        CAPACITY - self.len
    }

    fn push(&mut self, data: &[u8]) -> usize {
        if self.buf.is_empty() {
            self.buf.resize(CAPACITY, 0);
        }
        let n = data.len().min(self.room());
        let tail = (self.head + self.len) % CAPACITY;
        let first = n.min(CAPACITY - tail);
        self.buf[tail..tail + first].copy_from_slice(&data[..first]);
        self.buf[..n - first].copy_from_slice(&data[first..n]);
        self.len += n;
        n
    }

    fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        let first = n.min(CAPACITY - self.head);
        out[..first].copy_from_slice(&self.buf[self.head..self.head + first]);
        out[first..n].copy_from_slice(&self.buf[..n - first]);
        self.head = (self.head + n) % CAPACITY;
        self.len -= n;
        if self.len == 0 {
            self.head = 0;
        }
        n
    }
}

pub struct Pipe {
    ring: Mutex<Ring>,
    /// Descriptors on each end, kept by `ref_changed`
    readers: AtomicU32,
    writers: AtomicU32,
    read_vid: usize,
    write_vid: usize,
}

/// Both ends' vnodes, each to its pipe
static PIPES: Mutex<BTreeMap<usize, Arc<Pipe>>> = Mutex::new(BTreeMap::new());

/// A new pipe between `read_vid` and `write_vid`, with no descriptors yet:
/// the caller's `inc_ref`s count them in
pub fn create(read_vid: usize, write_vid: usize) {
    let p = Arc::new(Pipe {
        ring: Mutex::new(Ring { buf: Vec::new(), head: 0, len: 0 }),
        readers: AtomicU32::new(0),
        writers: AtomicU32::new(0),
        read_vid,
        write_vid,
    });
    let mut t = PIPES.lock();
    t.insert(read_vid, p.clone());
    t.insert(write_vid, p);
}

fn of(vid: usize) -> Option<Arc<Pipe>> {
    PIPES.lock().get(&vid).cloned()
}

/// Whether `vid` is an end of a pipe
pub fn is_end(vid: usize) -> bool {
    PIPES.lock().contains_key(&vid)
}

/// A descriptor on `vid` came or went (`VNode::inc_ref`, `dec_ref`)
pub fn ref_changed(vid: usize, up: bool) {
    let Some(p) = of(vid) else { return };
    let count = if vid == p.read_vid { &p.readers } else { &p.writers };
    if up {
        count.fetch_add(1, Ordering::AcqRel);
    } else {
        let _ = count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| c.checked_sub(1));
    }
}

/// `vid`'s vnode is gone: its end of the pipe with it, and the pipe when it
/// was the second end to go
pub fn forget(vid: usize) {
    PIPES.lock().remove(&vid);
}

/// The key both ends wait on and are woken by
pub fn wait_key(vid: usize) -> u64 {
    let base = of(vid).map_or(vid, |p| p.read_vid);
    crate::sched::wait::key::pipe(base)
}

/// Wake whoever waits on either end of `vid`'s pipe
pub fn wake(vid: usize) {
    crate::sched::wait::wake_key(wait_key(vid));
}

pub enum PipeRead {
    Data(usize),
    WouldBlock,
    Eof,
}

/// Take what is there, up to `out.len()`. Never waits
pub fn try_read(vid: usize, out: &mut [u8]) -> PipeRead {
    let Some(p) = of(vid) else { return PipeRead::Eof };
    let n = p.ring.lock().pop(out);
    if n > 0 {
        return PipeRead::Data(n);
    }
    if p.writers.load(Ordering::Acquire) == 0 {
        // Looked at once more: a write between the pop above and the last
        // writer going is data, not the end
        return match p.ring.lock().pop(out) {
            0 => PipeRead::Eof,
            n => PipeRead::Data(n),
        };
    }
    PipeRead::WouldBlock
}

pub enum PipeWrite {
    Wrote(usize),
    Full,
    Broken,
}

/// Put in what fits. A write of `ATOMIC_WRITE` bytes or fewer goes in whole
/// or not at all. Never waits
pub fn try_write(vid: usize, data: &[u8]) -> PipeWrite {
    let Some(p) = of(vid) else { return PipeWrite::Broken };
    if p.readers.load(Ordering::Acquire) == 0 {
        return PipeWrite::Broken;
    }
    let mut r = p.ring.lock();
    if data.len() <= ATOMIC_WRITE && r.room() < data.len() {
        return PipeWrite::Full;
    }
    match r.push(data) {
        0 => PipeWrite::Full,
        n => PipeWrite::Wrote(n),
    }
}

/// Bytes waiting, whether a write would find room, and whether the other
/// end has gone, for `poll`: (readable, writable, hung up)
pub fn poll(vid: usize) -> (bool, bool, bool) {
    let Some(p) = of(vid) else { return (false, false, true) };
    let (len, room) = {
        let r = p.ring.lock();
        (r.len, r.room())
    };
    if vid == p.read_vid {
        (len > 0, false, p.writers.load(Ordering::Acquire) == 0)
    } else {
        (false, room > 0, p.readers.load(Ordering::Acquire) == 0)
    }
}

/// Bytes a write to `vid`'s pipe would find room for now
pub fn room(vid: usize) -> usize {
    of(vid).map_or(0, |p| p.ring.lock().room())
}

/// The read end of `vid`'s pipe, for a caller that has either
pub fn read_end(vid: usize) -> Option<usize> {
    of(vid).map(|p| p.read_vid)
}
