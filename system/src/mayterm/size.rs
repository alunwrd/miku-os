// Window pixels and resizing
//
// A resize destroys old buffers and the pool, since otherwise object ids and
// contiguous memory run out. Uses two buffers when available, one otherwise.

use super::conn::{Conn, Pool};
// The requests themselves, shared with the kernel's client half
use super::requests::Requests;
use super::grid;
use super::protocol;

pub const MAX_BUFFERS: usize = 2;

/// One bit per row of the grid, rounded up to whole words
const DIRTY_WORDS: usize = (grid::MAX_ROWS + 63) / 64;

pub struct Frame {
    pool: Option<Pool>,
    pool_obj: u32,
    /// (buffer id, the compositor is done with it)
    bufs: [(u32, bool); MAX_BUFFERS],
    /// Per-buffer bitmap of stale grid rows, since the buffers hold different
    /// frames. A bitmap rather than a span, since the kernel marks the whole
    /// grid on every scroll
    dirty: [[u64; DIRTY_WORDS]; MAX_BUFFERS],
    /// Where the budget next looks, per buffer. It sweeps, so every dirty row
    /// is reached within `ceil(rows / ROWS_PER_FRAME)` frames
    sweep: [usize; MAX_BUFFERS],
    /// Buffer needs its margin painted: new, or the grid changed shape. Pools
    /// are not cleared, so they may hold the previous pool's pixels
    surround: [bool; MAX_BUFFERS],
    /// Row with the caret in this buffer, per buffer. The row it leaves must be
    /// restored from the grid
    caret_row: [usize; MAX_BUFFERS],
    pub count: usize,
    pub width: i32,
    pub height: i32,
    stride: i32,
    bytes: usize,
}

impl Frame {
    pub const fn new() -> Self {
        Frame {
            pool: None,
            pool_obj: 0,
            bufs: [(0, true); MAX_BUFFERS],
            dirty: [[0; DIRTY_WORDS]; MAX_BUFFERS],
            sweep: [0; MAX_BUFFERS],
            surround: [true; MAX_BUFFERS],
            caret_row: [0; MAX_BUFFERS],
            count: 0,
            width: 0,
            height: 0,
            stride: 0,
            bytes: 0,
        }
    }

    pub fn live(&self) -> bool {
        self.pool.is_some()
    }

    pub fn stride_px(&self) -> usize {
        (self.stride / 4) as usize
    }

    /// Creates a pool and buffers for this size. False if not even one buffer's
    /// contiguous memory is available
    pub fn make(&mut self, c: &mut Conn, shm: u32, width: i32, height: i32) -> bool {
        if width <= 0 || height <= 0 {
            return false;
        }
        let stride = width * 4;
        let bytes = (stride * height) as usize;

        // Two if we can have them, one if we cannot
        let (pool, count) = match Pool::new(bytes * MAX_BUFFERS) {
            Some(p) => (p, MAX_BUFFERS),
            None => match Pool::new(bytes) {
                Some(p) => (p, 1),
                None => return false,
            },
        };

        self.pool = Some(pool);
        self.count = count;
        self.width = width;
        self.height = height;
        self.stride = stride;
        self.bytes = bytes;

        let handle = self.pool.as_ref().map(|p| p.handle).unwrap_or(0);
        self.pool_obj = c.create_pool(shm, handle, (bytes * count) as i32);
        let mut i = 0;
        while i < count {
            self.bufs[i] = (
                c.create_buffer(
                    self.pool_obj, (i * bytes) as i32, width, height, stride,
                    // ARGB: the default background is translucent
                    // (`grid::BG_ALPHA`)
                    protocol::shm::FORMAT_ARGB8888,
                ),
                true,
            );
            self.dirty[i] = [0; DIRTY_WORDS];
            self.sweep[i] = 0;
            self.surround[i] = true;
            i += 1;
        }

        // Not cleared: cells paint their own boxes, and `Grid::draw_surround`
        // paints the bands on a buffer's first frame
        true
    }

    /// Releases the buffers, then the pool
    pub fn teardown(&mut self, c: &mut Conn) {
        let mut i = 0;
        while i < self.count {
            if self.bufs[i].0 != 0 {
                c.destroy_buffer(self.bufs[i].0);
            }
            self.bufs[i] = (0, true);
            self.dirty[i] = [0; DIRTY_WORDS];
            self.sweep[i] = 0;
            i += 1;
        }
        if self.pool_obj != 0 {
            c.destroy_pool(self.pool_obj);
            self.pool_obj = 0;
        }
        c.flush();
        if let Some(p) = self.pool.as_mut() {
            p.free();
        }
        self.pool = None;
        self.count = 0;
    }

    /// A buffer the compositor is not reading, or None
    pub fn free_slot(&self) -> Option<usize> {
        // Single-buffered never waits
        if self.count == 1 {
            return Some(0);
        }
        let mut i = 0;
        while i < self.count {
            if self.bufs[i].1 {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    pub fn released(&mut self, id: u32) {
        let mut i = 0;
        while i < self.count {
            if self.bufs[i].0 == id {
                self.bufs[i].1 = true;
            }
            i += 1;
        }
    }

    /// Whether the margin must be painted into this buffer; answers once per
    /// buffer
    pub fn take_surround(&mut self, slot: usize) -> bool {
        let owed = self.surround[slot];
        self.surround[slot] = false;
        owed
    }

    /// Grid shape changed, so the bands did too
    pub fn reshaped(&mut self) {
        self.surround = [true; MAX_BUFFERS];
    }

    /// Current and next caret rows for this buffer. Both are painted and
    /// declared regardless of the budget, so typing responds during heavy
    /// output
    pub fn caret_rows(&mut self, slot: usize, row: usize) -> (usize, usize) {
        let was = self.caret_row[slot];
        self.caret_row[slot] = row;
        (was, row)
    }

    /// Marks every row: a new pool or a recut grid. Uses the ceiling, since the
    /// drawer clamps to the grid
    pub fn mark_all(&mut self) {
        self.mark(0, grid::MAX_ROWS);
    }

    /// Marks rows `r0..r1` stale in every buffer
    pub fn mark(&mut self, r0: usize, r1: usize) {
        let r1 = r1.min(grid::MAX_ROWS);
        if r0 >= r1 {
            return;
        }
        let mut i = 0;
        while i < self.count {
            let mut r = r0;
            while r < r1 {
                self.dirty[i][r / 64] |= 1u64 << (r % 64);
                r += 1;
            }
            i += 1;
        }
    }

    fn is_dirty(&self, slot: usize, row: usize) -> bool {
        row < grid::MAX_ROWS && self.dirty[slot][row / 64] & (1u64 << (row % 64)) != 0
    }

    /// Rows this buffer owes within the grid
    pub fn owed_rows(&self, slot: usize, rows: usize) -> usize {
        let rows = rows.min(grid::MAX_ROWS);
        (0..rows).filter(|&r| self.is_dirty(slot, r)).count()
    }

    /// Next run of owed rows: at most `budget`, adjacent so one frame is one
    /// rectangle, swept from where the last frame stopped. None if nothing is
    /// owed
    pub fn next_run(&self, slot: usize, budget: usize, rows: usize) -> Option<(usize, usize)> {
        let rows = rows.min(grid::MAX_ROWS);
        if rows == 0 || budget == 0 {
            return None;
        }
        let start = self.sweep[slot] % rows;
        let mut n = 0;
        let mut first = None;
        while n < rows {
            let r = (start + n) % rows;
            if self.is_dirty(slot, r) {
                first = Some(r);
                break;
            }
            n += 1;
        }
        let r0 = first?;
        // Stops at the grid's bottom, since a wrapped run would be two
        // rectangles
        let mut r1 = r0 + 1;
        while r1 < rows && r1 - r0 < budget && self.is_dirty(slot, r1) {
            r1 += 1;
        }
        Some((r0, r1))
    }

    /// Rows `r0..r1` were drawn and committed; the compositor owns this buffer
    /// until release. Per buffer
    pub fn committed(&mut self, slot: usize, r0: usize, r1: usize, rows: usize) {
        let mut r = r0;
        while r < r1 {
            self.drew_row(slot, r);
            r += 1;
        }
        // Next sweep position, past the run just drawn
        self.sweep[slot] = if rows == 0 { 0 } else { r1 % rows };
        self.bufs[slot].1 = false;
    }

    /// Clears a row painted outside the budget (the caret), so a still caret
    /// leaves nothing owed
    pub fn drew_row(&mut self, slot: usize, row: usize) {
        if row < grid::MAX_ROWS {
            self.dirty[slot][row / 64] &= !(1u64 << (row % 64));
        }
    }

    pub fn id(&self, slot: usize) -> u32 {
        self.bufs[slot].0
    }

    /// This slot's pixels, exactly one buffer long, since `text::draw_char`
    /// derives height from the length
    pub fn pixels(&mut self, slot: usize) -> &mut [u8] {
        let (off, len) = (slot * self.bytes, self.bytes);
        match self.pool.as_mut() {
            Some(p) => &mut p.bytes()[off..off + len],
            None => &mut [],
        }
    }
}
