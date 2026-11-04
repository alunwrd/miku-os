// Print fragments into whole lines, one CPU at a time
//
// `serial_print!` without a newline, and `write_fmt` handing its arguments over
// in pieces, both arrive here as fragments. Each CPU assembles its own line:
// every caller has interrupts off (the serial path runs without them), so the
// slot of the CPU it runs on is touched by nothing else, and two CPUs printing
// at once do not interleave halves of lines.
//
// A line takes the level and the site of its first fragment. The UART has
// already had every byte; what does not fit in `MAX_TEXT` is cut here only.
//
// Before a CPU has its per-CPU block (GS base still zero: the first lines of
// boot, an AP in its first instructions) there is no slot, and each fragment is
// committed on its own.

use core::cell::UnsafeCell;
use core::fmt;

use super::ring::MAX_TEXT;
use super::{commit, flags, Level, Site};
use crate::arch::x86_64::percpu::MAX_CPUS;

struct Pending {
    len: usize,
    level: Level,
    site: Site,
    flags: u8,
    buf: [u8; MAX_TEXT],
}

impl Pending {
    const EMPTY: Pending = Pending { len: 0, level: Level::Info, site: Site::UNKNOWN, flags: 0, buf: [0; MAX_TEXT] };
}

struct Slots([UnsafeCell<Pending>; MAX_CPUS]);

// Each slot is only ever touched by its own CPU with interrupts off
unsafe impl Sync for Slots {}

static SLOTS: Slots = Slots([const { UnsafeCell::new(Pending::EMPTY) }; MAX_CPUS]);

/// This CPU's slot, once it has a per-CPU block
fn slot() -> Option<&'static mut Pending> {
    let i = super::this_cpu()?;
    Some(unsafe { &mut *SLOTS.0[i.min(MAX_CPUS - 1)].get() })
}

/// Where a formatted print goes on its way into the log
pub struct Writer {
    pub level: Level,
    pub site: Site,
    pub flags: u8,
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        feed(self.level, self.site, self.flags, s.as_bytes());
        Ok(())
    }
}

/// Take one fragment. The caller has interrupts off
pub fn feed(level: Level, site: Site, fl: u8, mut bytes: &[u8]) {
    let Some(p) = slot() else {
        // No slot yet: every piece is its own record
        for piece in bytes.split(|&b| b == b'\n') {
            finish(level, site, fl, piece);
        }
        return;
    };
    while !bytes.is_empty() {
        let (piece, rest, ends) = match bytes.iter().position(|&b| b == b'\n') {
            Some(i) => (&bytes[..i], &bytes[i + 1..], true),
            None => (bytes, &[][..], false),
        };
        if p.len == 0 && p.flags & flags::STARTED == 0 {
            p.level = level;
            p.site = site;
            p.flags = fl | flags::STARTED;
        }
        let room = MAX_TEXT - p.len;
        let take = piece.len().min(room);
        p.buf[p.len..p.len + take].copy_from_slice(&piece[..take]);
        p.len += take;
        if take < piece.len() {
            p.flags |= flags::TRUNC;
        }
        if ends {
            finish(p.level, p.site, p.flags & !flags::STARTED, &p.buf[..p.len]);
            p.len = 0;
            p.flags = 0;
        }
        bytes = rest;
    }
}

/// Commit a line: a trailing carriage return dropped, an empty line not kept
fn finish(level: Level, site: Site, fl: u8, mut text: &[u8]) {
    if let [head @ .., b'\r'] = text {
        text = head;
    }
    if text.is_empty() {
        return;
    }
    commit(level, site, fl, text);
}
