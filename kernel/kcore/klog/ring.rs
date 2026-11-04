// One ring of log records over a byte slice
//
// Records are variable length and eight-aligned: a 40-byte header, the text,
// padding. The writer goes forward and drops the oldest records to make room; a
// record not fitting before the end goes to the start, and the end is closed
// with a wrap marker (or, with less than a header's room left, by nothing: a
// reader treats "no room for a header" the same).
//
// Every record has an index within this ring, counted from zero at boot, held
// by a reader's cursor: an index at or past `first_idx` is a record still here,
// at the offset the cursor remembered; one below it was overwritten, and the
// reader is told how many it lost.
//
// No lock and no static here, so the host test drives it over a `Vec`. The
// caller holds the lock the slice lives under.

/// Header bytes before the text
pub const HDR: usize = 40;

/// Longest text a record carries; the rest of a line is cut and flagged
pub const MAX_TEXT: usize = 1000;

const KIND_RECORD: u8 = 0;
const KIND_WRAP: u8 = 1;

/// Everything about a record except its text
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub seq: u64,
    pub ts_us: u64,
    /// `module_path!()` of the call site. Always a `&'static str`: the ring
    /// keeps the pointer, not the bytes
    pub module: &'static str,
    pub line: u32,
    pub pid: u32,
    pub level: u8,
    pub cpu: u8,
    pub flags: u8,
}

/// Where the ring's records are. All zero is the empty ring, so a static of
/// these is in .bss
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// Where the next record goes
    pub head: usize,
    /// Where the oldest record is
    pub tail: usize,
    /// Records in the ring
    pub count: u64,
    /// Index of the oldest record; `first_idx + count` is the next one's
    pub first_idx: u64,
}

impl State {
    pub const EMPTY: State = State { head: 0, tail: 0, count: 0, first_idx: 0 };

    pub fn next_idx(&self) -> u64 {
        self.first_idx + self.count
    }
}

/// A reader's place in one ring: the index of the next record it wants and
/// where that record starts
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub idx: u64,
    pub off: usize,
}

pub fn record_size(text_len: usize) -> usize {
    (HDR + text_len.min(MAX_TEXT) + 7) & !7
}

fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}

//   0 seq   8 ts_us   16 module ptr   24 module len (u16)   26 text len (u16)
//  28 line  32 pid    36 level  37 cpu  38 flags  39 kind
fn write_header(b: &mut [u8], at: usize, m: &Meta, text_len: usize) {
    put_u64(b, at, m.seq);
    put_u64(b, at + 8, m.ts_us);
    put_u64(b, at + 16, m.module.as_ptr() as u64);
    put_u16(b, at + 24, m.module.len().min(u16::MAX as usize) as u16);
    put_u16(b, at + 26, text_len as u16);
    put_u32(b, at + 28, m.line);
    put_u32(b, at + 32, m.pid);
    b[at + 36] = m.level;
    b[at + 37] = m.cpu;
    b[at + 38] = m.flags;
    b[at + 39] = KIND_RECORD;
}

/// A header and the length of its text, from bytes this module wrote
pub fn read_header(b: &[u8], at: usize) -> (Meta, usize) {
    let ptr = u64_at(b, at + 16) as *const u8;
    let len = u16_at(b, at + 24) as usize;
    // Written from a `&'static str` by `write_header` and by nothing else
    let module = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, len)) };
    let meta = Meta {
        seq: u64_at(b, at),
        ts_us: u64_at(b, at + 8),
        module,
        line: u32_at(b, at + 28),
        pid: u32_at(b, at + 32),
        level: b[at + 36],
        cpu: b[at + 37],
        flags: b[at + 38],
    };
    (meta, u16_at(b, at + 26) as usize)
}

/// Where the record that belongs at `off` really starts: the start of the
/// ring when the end was closed there
fn settle(b: &[u8], off: usize) -> usize {
    if off + HDR > b.len() || b[off + 39] == KIND_WRAP {
        0
    } else {
        off
    }
}

/// Drop the oldest record
fn evict(st: &mut State, b: &[u8]) {
    let (_, text_len) = read_header(b, st.tail);
    st.count -= 1;
    st.first_idx += 1;
    if st.count == 0 {
        st.head = 0;
        st.tail = 0;
    } else {
        // Another record follows, so what is at the new tail is its header or
        // the marker the writer left when it went round
        st.tail = settle(b, st.tail + record_size(text_len));
    }
}

/// Append one record, dropping the oldest as needed; its index in this ring.
/// `text` past `MAX_TEXT` is cut, and the caller flags that
pub fn push(st: &mut State, b: &mut [u8], meta: &Meta, text: &[u8]) -> u64 {
    let text = &text[..text.len().min(MAX_TEXT)];
    let size = record_size(text.len());
    let n = b.len();
    debug_assert!(size * 2 <= n);
    let at = loop {
        if st.count == 0 {
            st.head = 0;
            st.tail = 0;
            break 0;
        }
        if st.tail < st.head {
            // Free: [head, n) and [0, tail)
            if st.head + size <= n {
                break st.head;
            }
            // Close the end and go round. The records after the tail remain;
            // the next turn evicts what is in the way
            if n - st.head >= HDR {
                b[st.head + 39] = KIND_WRAP;
            }
            st.head = 0;
            continue;
        }
        // tail >= head: free is [head, tail), nothing when they meet
        if st.head + size <= st.tail {
            break st.head;
        }
        evict(st, b);
    };
    write_header(b, at, meta, text.len());
    b[at + HDR..at + HDR + text.len()].copy_from_slice(text);
    st.head = at + size;
    st.count += 1;
    st.next_idx() - 1
}

pub fn cursor_start(st: &State) -> Cursor {
    Cursor { idx: st.first_idx, off: st.tail }
}

pub fn cursor_end(st: &State) -> Cursor {
    Cursor { idx: st.next_idx(), off: st.head }
}

/// Hand every record from `cur` on to `f`, header and text, and move `cur`
/// past them. Returns how many records were overwritten before the reader
/// got to them
pub fn read(st: &State, b: &[u8], cur: &mut Cursor, f: &mut dyn FnMut(&Meta, &[u8])) -> u64 {
    let mut lost = 0;
    if cur.idx < st.first_idx || cur.idx > st.next_idx() {
        lost = st.first_idx.saturating_sub(cur.idx);
        *cur = cursor_start(st);
    }
    let mut off = cur.off;
    while cur.idx < st.next_idx() {
        off = settle(b, off);
        let (meta, text_len) = read_header(b, off);
        f(&meta, &b[off + HDR..off + HDR + text_len]);
        off += record_size(text_len);
        cur.idx += 1;
    }
    cur.off = off;
    lost
}

/// Lay one record out at the start of `dst`, `record_size` bytes: how a reader
/// copies records out of the ring and parses them after the lock is gone
pub fn encode(dst: &mut [u8], m: &Meta, text: &[u8]) {
    let text = &text[..text.len().min(MAX_TEXT)];
    write_header(dst, 0, m, text.len());
    dst[HDR..HDR + text.len()].copy_from_slice(text);
}

/// Walk records laid end to end by `encode`
pub fn decode_all(b: &[u8], f: &mut dyn FnMut(&Meta, &[u8])) {
    let mut off = 0;
    while off + HDR <= b.len() {
        let (meta, text_len) = read_header(b, off);
        f(&meta, &b[off + HDR..off + HDR + text_len]);
        off += record_size(text_len);
    }
}

/// Bytes the live records take, for `stat`
pub fn used(st: &State, n: usize) -> usize {
    if st.count == 0 {
        0
    } else if st.tail < st.head {
        st.head - st.tail
    } else {
        n - st.tail + st.head
    }
}
