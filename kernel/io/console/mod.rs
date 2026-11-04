pub mod color;

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use spin::Mutex;
use x86_64::instructions::interrupts;

pub static WRITER: ConsoleLock = ConsoleLock::new();

/// The console behind its lock: a `spin::Mutex` whose guard, on release,
/// carries what was drawn since from the console's shadow to the panel
/// (`Console::mirror_now`; `dev/terminal-architecture.md`, "The panel nobody
/// reads"). It derefs to `Option<Console>` like the mutex's guard
pub struct ConsoleLock(Mutex<Option<Console>>);

pub struct ConsoleGuard<'a>(spin::MutexGuard<'a, Option<Console>>);

impl ConsoleLock {
    pub const fn new() -> Self {
        ConsoleLock(Mutex::new(None))
    }

    pub fn lock(&self) -> ConsoleGuard<'_> {
        ConsoleGuard(self.0.lock())
    }

    pub fn try_lock(&self) -> Option<ConsoleGuard<'_>> {
        self.0.try_lock().map(ConsoleGuard)
    }
}

impl core::ops::Deref for ConsoleGuard<'_> {
    type Target = Option<Console>;
    fn deref(&self) -> &Option<Console> {
        &self.0
    }
}

impl core::ops::DerefMut for ConsoleGuard<'_> {
    fn deref_mut(&mut self) -> &mut Option<Console> {
        &mut self.0
    }
}

impl Drop for ConsoleGuard<'_> {
    /// Under the lock, so nothing draws between the pixels and their copy
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            c.mirror_now();
        }
    }
}

/// Where the console lives while the framebuffer is detached.
///
/// Every WRITER user in this module is written as
/// `if let Some(w) = WRITER.lock()...`, for the window before the framebuffer
/// exists. Moving the Console out of WRITER makes all of them (print, cursor
/// redraw, fill_rect, the shell's input line) no-ops that still reach serial,
/// and moving it back restores them.
///
/// For one situation: the boot framebuffer is in a GPU's BAR1 aperture, and
/// GSP-RM remaps that aperture from physical to virtual mode during its display
/// init. CPU stores into a BAR being remapped can wedge the link, which looks
/// like a kernel hang and destroys the log
static PARKED: Mutex<Option<Console>> = Mutex::new(None);

/// Stop all framebuffer output; serial keeps working. Returns false if there
/// was no console to detach (already detached, or never initialised)
pub fn detach_framebuffer() -> bool {
    interrupts::without_interrupts(|| {
        let taken = WRITER.lock().take();
        match taken {
            Some(c) => {
                *PARKED.lock() = Some(c);
                true
            }
            None => false,
        }
    })
}

/// Put the console back on the framebuffer. Returns false if nothing was
/// parked
pub fn attach_framebuffer() -> bool {
    interrupts::without_interrupts(|| {
        let parked = PARKED.lock().take();
        match parked {
            Some(c) => {
                *WRITER.lock() = Some(c);
                true
            }
            None => false,
        }
    })
}

pub fn is_framebuffer_attached() -> bool {
    interrupts::without_interrupts(|| WRITER.lock().is_some())
}

/// Is there a console at all, drawing or parked? `retarget` can move either
pub fn exists() -> bool {
    interrupts::without_interrupts(|| WRITER.lock().is_some() || PARKED.lock().is_some())
}

/// Called after the console has drawn, to carry the pixels somewhere the CPU
/// cannot store into directly.
///
/// On GB206, after GSP-RM has taken the display, the only path from the CPU to
/// the scanned-out surface is one MMIO write per pixel through a 1 MiB BAR0
/// window. The console renders into ordinary RAM and the registered hook copies
/// the changed part across; `io::console` does not know what it is
static FLUSH_HOOK: Mutex<Option<fn()>> = Mutex::new(None);

pub fn set_flush_hook(f: fn()) {
    *FLUSH_HOOK.lock() = Some(f);
}

pub fn clear_flush_hook() {
    *FLUSH_HOOK.lock() = None;
}

/// Push what has been drawn since the last flush to the display.
///
/// A no-op unless a hook is registered. Called from the shell loop, after each
/// key and after each command, not from `print!`: a command printing thirty
/// lines costs one flush.
///
/// The hook runs outside the `WRITER` lock: a flush can take milliseconds,
/// `WRITER` is taken with interrupts disabled, and a print from an interrupt
/// would be stuck behind it
pub fn flush() {
    // The framebuffer is mapped write-combining, so the last stores may still
    // be in a fill buffer. Unconditional and before the hook: a console with no
    // hook (any machine without an NVIDIA card) would leave the tail of its
    // last line unwritten
    unsafe { core::arch::x86_64::_mm_sfence() };
    let hook = *FLUSH_HOOK.lock();
    if let Some(f) = hook {
        f();
    }
}

/// Point the console at a different framebuffer, keeping everything on it.
///
/// The character grid moves across, so the shell's scrollback, prompt and the
/// line being typed survive; the console is repainted from its own cells.
/// Returns false if there is no console.
///
/// A parked console is un-parked and retargeted without releasing the lock: it
/// is parked because its framebuffer is an aperture since remapped, and a print
/// must not land there
pub fn retarget(fb: &'static mut [u8], config: FrameBufferConfig) -> bool {
    interrupts::without_interrupts(|| {
        let mut writer = WRITER.lock();
        if writer.is_none() {
            *writer = PARKED.lock().take();
        }
        match writer.as_mut() {
            Some(w) => {
                w.retarget(fb, config);
                true
            }
            None => false,
        }
    })
}

/// Where the console is drawing now: the buffer, its length, and its geometry.
/// For moving the console elsewhere and putting it back, without rebuilding the
/// panel's slice from the boot information
pub fn target() -> Option<(*mut u8, usize, FrameBufferConfig)> {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| {
            (w.fb_ptr, w.fb_len, FrameBufferConfig {
                width: w.width,
                height: w.height,
                stride: w.stride_bytes / w.bpp.max(1),
                bytes_per_pixel: w.bpp,
                is_bgr: w.is_bgr,
            })
        })
    })
}

/// The screen itself: the panel under the console's shadow when there is one
/// (`shadow_panel`), otherwise where the console draws. For composing the
/// screen with the processor: the console's shadow is copied out only when the
/// console draws
pub fn panel() -> Option<(*mut u8, usize, FrameBufferConfig)> {
    let mirrored = interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().and_then(|w| w.mirror).map(|m| (m.panel.ptr, m.panel.len, m.panel.cfg))
    });
    mirrored.or_else(target)
}

/// Draw into a copy of the panel in ordinary memory, and only store to the
/// panel.
///
/// The panel firmware hands over is a PCI aperture on real hardware (the GPU's
/// BAR1), and reading it is an uncached round trip per load: a scroll costs
/// 0.78 s a line on the GB206 (`dev/terminal-architecture.md`, "The panel
/// nobody reads"). With this the console draws into the shadow, a scroll moves
/// memory, and each release of the console lock stores the dirtied rows to the
/// panel.
///
/// Once, when there is a heap: 8 MiB at 1080p. `fbdirect` on the boot line
/// draws on the panel directly. Returns the shadow's size, or why there is none
pub fn shadow_panel() -> Result<usize, &'static str> {
    if crate::arch::x86_64::grub::cmdline_flag("fbdirect") {
        return Err("'fbdirect' on the boot line");
    }
    let Some((ptr, len, cfg)) = target() else { return Err("no console") };
    let mut shadow: Vec<u8> = Vec::new();
    if shadow.try_reserve_exact(len).is_err() {
        return Err("no memory for a copy of the panel");
    }
    shadow.resize(len, 0);
    let shadow: &'static mut [u8] = shadow.leak();
    let taken = interrupts::without_interrupts(|| {
        let mut g = WRITER.lock();
        let Some(c) = g.as_mut() else { return false };
        // Moved somewhere else since `target`: leave it there
        if c.fb_ptr != ptr || c.mirror.is_some() {
            return false;
        }
        // A repaint from the cells into the shadow, the panel not read once;
        // the guard's drop stores all of it to the panel
        c.retarget(shadow, cfg);
        c.mirror = Some(Mirror { shadow: c.fb_ptr, panel: Surface { ptr, len, cfg } });
        true
    });
    if taken { Ok(len) } else { Err("the console moved while the shadow was made") }
}

/// The panel under the console changed mode in place: the same memory, another
/// width, height and pitch, a bochs display put into another mode on the fly
/// (`drivers::gpu::display`, the display layer's K3).
///
/// The console re-flows into the new size wherever it draws. Under a shadow
/// (`shadow_panel`) that is the shadow, which keeps its buffer, and the mirror
/// learns the panel's new geometry; without one it is the panel itself. False
/// if there is no console, or the new frame is larger than the memory either
/// was given: a mode bigger than the boot's is the caller's to refuse
pub fn panel_resized(width: usize, height: usize, pitch: usize) -> bool {
    interrupts::without_interrupts(|| {
        let mut g = WRITER.lock();
        let Some(w) = g.as_mut() else { return false };
        let bpp = w.bpp.max(1);
        let cfg = FrameBufferConfig {
            width, height, stride: pitch / bpp, bytes_per_pixel: bpp, is_bgr: w.is_bgr,
        };
        let need = pitch * height;
        if need > w.fb_len {
            return false;
        }
        if let Some(m) = w.mirror.as_mut() {
            if need > m.panel.len {
                return false;
            }
            m.panel.cfg = cfg;
        }
        // Wherever it draws now (the shadow under a mirror, the panel without
        // one): the same buffer, read at the new geometry. The repaint stores
        // every row to the panel on the way out
        let fb = unsafe { core::slice::from_raw_parts_mut(w.fb_ptr, w.fb_len) };
        w.retarget(fb, cfg);
        true
    })
}

/// Resize the console inside its framebuffer, re-wrapping the text. False if
/// there is no console, or the new size has no room for a single character
///
/// `width` must be inside the framebuffer's stride and `height` inside its
/// rows: this changes the extent only, and the caller knows the buffer's size
pub fn resize_view(width: usize, height: usize) -> bool {
    interrupts::without_interrupts(|| {
        match WRITER.lock().as_mut() {
            Some(w) => w.resize_view(width, height),
            None => false,
        }
    })
}

/// Redraw the whole console and mark the screen fully damaged, so the next
/// flush pushes every pixel.
///
/// For when something else has drawn on the same surface: the console repaints
/// only what it dirtied
pub fn repaint() -> bool {
    interrupts::without_interrupts(|| {
        match WRITER.lock().as_mut() {
            Some(w) => { w.repaint_all(); true }
            None => false,
        }
    })
}

/// The rectangle drawn into since the last call, in pixels, or None if nothing
/// has changed. Reading it clears it
pub fn take_damage() -> Option<(usize, usize, usize, usize)> {
    interrupts::without_interrupts(|| WRITER.lock().as_mut().and_then(|w| w.take_damage()))
}

/// Which rows of the character grid have changed since the last call, as (row0,
/// row1) with the far edge exclusive. Reading it clears it
///
/// The counterpart of `take_damage` for whoever draws the text, not whoever
/// pushes pixels: it still answers while the console is headless
pub fn take_row_damage() -> Option<(usize, usize)> {
    interrupts::without_interrupts(|| WRITER.lock().as_mut().and_then(|w| w.take_row_damage()))
}

/// The console's own text, for `io::tty`
///
/// tty0 is this console: it owns the panel, takes panics, and outlives the
/// compositor, so the terminal table holds no copy
pub fn with_grid<R>(f: impl FnOnce(&mut TextGrid) -> R) -> Option<R> {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_mut().map(|c| f(c.grid_mut()))
    })
}

/// Where the pen is, in characters. (0, 0) when there is no console
pub fn cursor() -> (usize, usize) {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| w.cursor()).unwrap_or((0, 0))
    })
}

/// Stop or resume painting the text, leaving the character grid running
/// either way. Returns what it was, and false when there is no console
pub fn set_headless(on: bool) -> bool {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_mut().map(|w| w.set_headless(on)).unwrap_or(false)
    })
}

pub fn is_headless() -> bool {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| w.is_headless()).unwrap_or(false)
    })
}

/// The same, for the panic path: never waits for the lock
///
/// A panic can happen with any lock held, and one held by a halted thread is
/// never released. False means the lock was busy and the console goes on not
/// painting, losing the message on screen but not on the serial port.
///
/// Called by `render::emergency` only. Ordinary code uses `set_headless`, which
/// waits
pub fn try_set_headless(on: bool) -> bool {
    interrupts::without_interrupts(|| {
        match WRITER.try_lock() {
            Some(mut g) => g.as_mut().map(|w| { w.set_headless(on); true }).unwrap_or(false),
            None => false,
        }
    })
}

/// Re-wrap the text into a grid of this many characters. False if there is no
/// console, or the shape is empty or larger than the grid can hold
pub fn set_grid(cols: usize, rows: usize) -> bool {
    interrupts::without_interrupts(|| {
        match WRITER.lock().as_mut() {
            Some(w) => w.set_grid(cols, rows),
            None => false,
        }
    })
}

/// The largest grid this console can ever have, in characters
///
/// A ceiling: the grid is allocated at it and used at whatever shape the window
/// is. Anything sharing the grid allocates once, so a reader holding no lock
/// knows the memory under it stays
pub const fn grid_capacity() -> (usize, usize) {
    (MAX_COLS, MAX_ROWS)
}

/// Copy the character grid into `dst`, four bytes a cell, row-major, and say
/// what shape it was. None if there is no console or `dst` is too small
pub fn copy_cells(dst: &mut [u8]) -> Option<(usize, usize)> {
    interrupts::without_interrupts(|| WRITER.lock().as_ref().and_then(|w| w.copy_cells(dst)))
}

/// Copy a span of rows out of the grid, for a caller that knows what moved.
///
/// Answers with the byte offset into a full-grid buffer and the length written,
/// so the caller places the span without the stride arithmetic
pub fn copy_row_range(dst: &mut [u8], r0: usize, r1: usize) -> Option<(usize, usize)> {
    interrupts::without_interrupts(
        || WRITER.lock().as_ref().and_then(|w| w.copy_rows(dst, r0, r1)))
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::console::_print_at($crate::klog_site!(), format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    _print_at(crate::kcore::klog::Site::UNKNOWN, args);
}

/// `print!`, knowing where it was written: for the kernel log's copy of the
/// mirror (`serial::echo`)
#[doc(hidden)]
pub fn _print_at(site: crate::kcore::klog::Site, args: fmt::Arguments) {
    interrupts::without_interrupts(|| {
        // Where this task's output goes. Zero, which also covers every path
        // before there is a scheduler, is the console.
        //
        // This is how one shell can run on each terminal. The kernel's `print!`
        // sites take no terminal; they all arrive here, so the decision is made
        // once, from the running task's own `ctty`
        if route_to_tty(args, COLOR_MIKU) {
            // It went to a window. With `trace` on the boot line it also goes
            // to the log: a program on a pty prints into a grid only the
            // compositor can show, so a black terminal otherwise leaves nothing
            // to read
            if crate::arch::x86_64::serial::tracing() {
                crate::arch::x86_64::serial::echo(site, args);
            }
            // The serial port gets no copy of this: `sys_write` on fd 1 arrives
            // here, and a 115200 baud UART with interrupts off would cost `cat`
            // of a ten-kilobyte file most of a second. The mirror below is for
            // a headless machine and CI, where the output is the console
            return;
        }
        write_console_at(site, args);
    });
}

/// The console, and only the console
///
/// `_print` routes by the running task's terminal; this is its tail with the
/// routing taken off. For `_print` itself once the routing has declined, and
/// for `/dev/console`, which names the console
pub fn write_console(args: fmt::Arguments) {
    write_console_at(crate::kcore::klog::Site::UNKNOWN, args);
}

fn write_console_at(site: crate::kcore::klog::Site, args: fmt::Arguments) {
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            let _ = w.write_fmt(args);
        }
        // The console's own output, mirrored. Everything a program started from
        // the text console writes lands here, so a headless boot and CI can see
        // it; kernel `serial_println!` goes straight to the port
        crate::arch::x86_64::serial::echo(site, args);
    });
}

/// Send a line to the running task's own terminal, if it has one that is not
/// the console. True when handled here.
///
/// Not recursive: a terminal's grid is written directly, not through `print!`
fn route_to_tty(args: fmt::Arguments, color: [u8; 3]) -> bool {
    let raw = crate::sched::current_tty();
    if raw == 0 { return false; }
    let id = crate::io::tty::TtyId(raw);
    if id == crate::io::tty::SYSTEM || !crate::io::tty::exists(id) { return false; }
    // Formatted through a small stack buffer and written as cells. It must not
    // allocate, as this is reachable from an interrupt, and must not truncate:
    // the buffer is a window onto the write, not a limit on it
    struct Sink {
        id: crate::io::tty::TtyId,
        color: [u8; 3],
        buf: [u8; 256],
        len: usize,
        /// At least one chunk reached the terminal
        wrote: bool,
        /// At least one did not. Tracked separately: a write larger than the
        /// buffer is delivered in pieces, and a terminal going away halfway
        /// must not leave `wrote` true for the whole, which would drop the rest
        /// from the grid and from the serial mirror
        lost: bool,
    }
    impl Sink {
        /// Put what is buffered on the terminal, keeping any trailing bytes
        /// that are half of a character
        ///
        /// A flush lands wherever the buffer filled, possibly in the middle of
        /// a UTF-8 sequence. `valid_up_to` is where the text certainly ends;
        /// the remainder moves to the front and joins the next batch
        fn flush(&mut self) {
            if self.len == 0 { return; }
            let (text_len, keep) = match core::str::from_utf8(&self.buf[..self.len]) {
                Ok(_) => (self.len, 0),
                Err(e) => {
                    let v = e.valid_up_to();
                    (v, self.len - v)
                }
            };
            if text_len > 0 {
                let text = core::str::from_utf8(&self.buf[..text_len]).unwrap_or("");
                match crate::io::tty::with_grid(self.id, |g| g.write_str(text, self.color)) {
                    Some(true) => self.wrote = true,
                    // The terminal is gone, or it consumed nothing, as a grid
                    // parked mid-escape does: this text is on nobody's screen
                    _ => self.lost = true,
                }
            }
            if keep > 0 {
                self.buf.copy_within(text_len..self.len, 0);
            }
            self.len = keep;
        }
    }
    impl fmt::Write for Sink {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for b in s.as_bytes() {
                if self.len == self.buf.len() { self.flush(); }
                // A flush that could not place its text (the terminal has gone)
                // leaves the buffer full; drop the rest to avoid spinning
                if self.len == self.buf.len() { return Ok(()); }
                self.buf[self.len] = *b;
                self.len += 1;
            }
            Ok(())
        }
    }
    let mut sink = Sink { id, color, buf: [0; 256], len: 0, wrote: false, lost: false };
    let _ = fmt::write(&mut sink, args);
    sink.flush();
    // "It all went to the window" is the only answer that lets the caller stop.
    // Anything less and the line goes to the console and the log as well: a
    // duplicate is preferable to a line that exists nowhere
    sink.wrote && !sink.lost
}

/// A line in a colour, to wherever the running task's output goes
///
/// Routes like `_print`. Most of what the kernel's commands print is
/// `cprintln!`, which separates a heading from a number by colour. The colour
/// survives: a terminal's grid holds a colour per cell
pub fn print_colored(r: u8, g: u8, b: u8, args: fmt::Arguments) {
    print_colored_at(r, g, b, crate::kcore::klog::Site::UNKNOWN, args);
}

/// `cprint!`, knowing where it was written; see `_print_at`
#[doc(hidden)]
pub fn print_colored_at(r: u8, g: u8, b: u8, site: crate::kcore::klog::Site, args: fmt::Arguments) {
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        if route_to_tty(args, [r, g, b]) {
            if crate::arch::x86_64::serial::tracing() {
                crate::arch::x86_64::serial::echo(site, args);
            }
            return;
        }
        if let Some(w) = WRITER.lock().as_mut() {
            let saved = w.fg_color;
            w.fg_color = [r, g, b];
            let _ = w.write_fmt(args);
            w.fg_color = saved;
        }
        // The console's own output, mirrored to the log. The GB206 GSP-RM boot
        // takes scanout away mid-command, and every line after that exists only
        // on serial. Colour is dropped
        crate::arch::x86_64::serial::echo(site, args);
    });
}

pub fn backspace() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.backspace(); }
    });
}

pub fn clear_screen() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.clear(); }
    });
}

pub fn set_color(r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.fg_color = [r, g, b]; }
    });
}

pub fn reset_color() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.fg_color = COLOR_MIKU; }
    });
}

pub fn move_cursor_left() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            if w.x_pos > BORDER_PADDING { w.x_pos -= CHAR_WIDTH; }
        }
    });
}

pub fn move_cursor_right() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.x_pos += CHAR_WIDTH; }
    });
}

pub fn get_x() -> usize {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map_or(0, |w| w.x_pos)
    })
}

/// Where the pen is, in cells
///
/// Where the next character would go, the one thing that survives the console
/// being drawn by someone else: a pixel position belongs to the window the
/// kernel last painted in, and after a client takes the console over there is
/// no such window
pub fn pen() -> (usize, usize) {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map_or((0, 0), |w| (w.grid.cur_row, w.grid.cur_col))
    })
}

/// Put the pen on a cell, and the pixel position with it
///
/// For the line editor: the caret's position is a fact about the line being
/// typed, not about what was printed last. A console drawn by a client reports
/// this position as the cursor
pub fn set_pen(row: usize, col: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            if row < w.grid.rows && col <= w.grid.cols {
                w.grid.cur_row = row;
                w.grid.cur_col = col;
                w.x_pos = BORDER_PADDING + col * CHAR_WIDTH;
                w.y_pos = BORDER_PADDING + row * LINE_HEIGHT;
            }
        }
    });
}

pub fn set_x(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.x_pos = x;
            w.grid.cur_col = x.saturating_sub(BORDER_PADDING) / CHAR_WIDTH;
        }
    });
}

pub fn draw_cursor(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.paint_cursor(x, 200, 220, 220);
        }
    });
}

pub fn erase_cursor(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.paint_cursor(x, 0, 0, 0);
        }
    });
}

pub fn redraw_input_line(
    start_x: usize,
    buf: &[u8],
    len: usize,
    cursor: usize,
    old_len: usize,
    dirty_start: usize,
) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.redraw_input_fast(start_x, buf, len, cursor, old_len, dirty_start);
        }
    });
}

pub fn redraw_input_line_full(
    start_x: usize,
    buf: &[u8],
    len: usize,
    cursor: usize,
    old_len: usize,
) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.redraw_input_fast(start_x, buf, len, cursor, old_len, 0);
        }
    });
}

pub fn clear_from_x(start_x: usize, count: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.clear_columns(start_x, count); }
    });
}

/// Whether the console's pixels are stored 0xFF000000 | r<<16 | g<<8 | b:
/// XRGB8888, as anything reading the console's framebuffer as a surface assumes
pub fn fb_is_bgr() -> bool {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| w.fb_is_bgr()).unwrap_or(false)
    })
}

/// How many characters fit across the console, for a line editor
pub fn text_cols() -> usize {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| w.grid.cols).unwrap_or(0)
    })
}

pub fn fb_size() -> Option<(usize, usize)> {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| (w.fb_width(), w.fb_height()))
    })
}

pub fn fill_rect(x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(c) = WRITER.lock().as_mut() { c.fill_rect(x, y, w, h, r, g, b); }
    });
}

/// Somewhere to put pixels: the bytes, how many of them, and what a pixel looks
/// like there
///
/// The panel, in practice. Separate from the console's own target, since the
/// console becoming a window draws into memory and whoever is presenting owns
/// the panel. A blit through the console would follow it into its shadow and
/// compose the desktop into a buffer nobody sees
#[derive(Clone, Copy)]
pub struct Surface {
    pub ptr: *mut u8,
    pub len: usize,
    pub cfg: FrameBufferConfig,
}

// The pointer is the panel's, which outlives everything
unsafe impl Send for Surface {}

/// A console drawing into `shadow` stores what it drew to `panel`
/// (`shadow_panel`). Both have the panel's geometry, so an offset is the same
/// place in each
#[derive(Clone, Copy)]
struct Mirror {
    shadow: *mut u8,
    panel: Surface,
}

/// Copy a rectangle of a client's XRGB8888 buffer onto a surface
///
/// No damage is recorded: this is another's frame, not the console's text, and
/// marking it would report the desktop as the console's output and ask for it
/// to be composed again
///
/// # Safety
/// `src` must be readable for `src_pitch * (sy + h)` bytes, and `dst` must
/// describe live pixels
pub unsafe fn blit_xrgb_to(
    dst: &Surface,
    src: *const u8, src_pitch: usize,
    sx: usize, sy: usize, dx: usize, dy: usize, w: usize, h: usize,
) {
    let (width, height) = (dst.cfg.width, dst.cfg.height);
    if w == 0 || h == 0 || dx >= width || dy >= height {
        return;
    }
    let w = w.min(width - dx);
    let h = h.min(height - dy);
    let bpp = dst.cfg.bytes_per_pixel;
    let sb = dst.cfg.stride * bpp;
    for row in 0..h {
        let dst_base = sb * (dy + row) + dx * bpp;
        if dst_base + w * bpp > dst.len {
            break;
        }
        let s = src.add((sy + row) * src_pitch + sx * 4);
        let d = dst.ptr.add(dst_base);
        if bpp == 4 && dst.cfg.is_bgr {
            // Already in the panel's own order, so the row is one copy
            core::ptr::copy_nonoverlapping(s, d, w * 4);
        } else {
            for i in 0..w {
                let p = s.add(i * 4);
                let (b, g, r) = (*p, *p.add(1), *p.add(2));
                let px = if dst.cfg.is_bgr {
                    (r as u32) << 16 | (g as u32) << 8 | b as u32
                } else {
                    (b as u32) << 16 | (g as u32) << 8 | r as u32
                };
                let q = d.add(i * bpp);
                if bpp == 4 {
                    core::ptr::write_unaligned(q as *mut u32, px);
                } else {
                    for k in 0..bpp {
                        *q.add(k) = (px >> (8 * k)) as u8;
                    }
                }
            }
        }
    }
}

pub fn fill_hgradient(
    x: usize, y: usize, w: usize, h: usize,
    left: (u8, u8, u8), right: (u8, u8, u8),
) {
    interrupts::without_interrupts(|| {
        if let Some(c) = WRITER.lock().as_mut() { c.fill_hgradient(x, y, w, h, left, right); }
    });
}

pub fn write_pixel_at(x: usize, y: usize, r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(c) = WRITER.lock().as_mut() { c.write_pixel(x, y, r, g, b); }
    });
}

pub fn hide_cursor() {}
pub fn show_cursor() {}

pub fn clear_char() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.clear_char_at_cursor(); }
    });
}

pub fn write_char_at_x(x: usize, ch: u8, r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.write_single_char_at(x, ch, r, g, b);
        }
    });
}

pub fn clear_char_at(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.clear_rect(x, w.y_pos, CHAR_WIDTH, CHAR_HEIGHT);
        }
    });
}

/// Enough of ANSI to colour a line and redraw it
///
/// A terminal's text here is a grid of cells, each with a colour. `CSI ... m`
/// sets the colour, and movement lets a shell edit the line being typed and
/// full-screen programs work. The CVE class in Linux's `drivers/tty/vt` is not
/// cursor movement: it is the selection buffer and `vcs`, `TIOCSTI`, swappable
/// line disciplines, and scroll regions with heap behind them, none of which
/// exist here; see `io::tty`'s own list of refusals.
///
/// Supported: movement (`A B C D`), absolute column (`G`), absolute position
/// (`H`/`f`), erase in line (`K`) and erase in display (`J`). None of them can
/// do anything unbounded:
///
///   * no new state. The pen and the cells already exist; `apply_csi`
///     writes those and nothing else.
///   * no allocation. The grid is built with the terminal and never grows.
///   * every coordinate clamped where it is used, not where it is parsed.
///   * movement never scrolls, so no sequence can push the screen into the
///     history ring.
///
/// Dropped: `DSR` (`CSI 6n`), whose answer goes into the input queue, as this
/// machine has no input-injection primitive; scroll regions, the alternate
/// screen, save/restore cursor, and cursor visibility. A private-parameter
/// sequence (`CSI ? ... h`) is never decoded as one of the above: `?25l` is not
/// a request to erase anything.
///
/// No allocation and no growth: eight parameters, two more than `38;2;R;G;B`
/// needs
pub struct Vt {
    state: u8,
    params: [u16; 8],
    n: usize,
    cur: u16,
    seen_digit: bool,
    /// Everything SGR has said so far, carried between sequences
    pen: Pen,
    /// How much of the current sequence has been swallowed, so one that never
    /// ends cannot swallow the rest of the terminal; see `SEQ_MAX`
    eaten: u8,
    /// This sequence carried a private or intermediate byte (`?`, `<`..`>`, or
    /// one of ` `..`/`), so its final byte does not mean what the same letter
    /// means alone
    ///
    /// Otherwise `CSI ? 2 J`, a legal thing to receive, would read as "erase
    /// the whole display", and the common `CSI ? 25 l` is one digit away from
    /// it
    private: bool,
}

/// What the terminal should do with one character
pub enum VtOut {
    /// An ordinary character: put it on the screen
    Print(char),
    /// Part of an escape sequence, and nothing to show
    Eaten,
    /// The text from here on looks like this
    Pen(Pen),
    /// Put the pen somewhere, or blank part of the terminal
    Move(Csi),
}

/// The sequences that move the pen or erase, decoded
///
/// Rows and columns are zero-based here and one-based on the wire: converted
/// once, in the parser. A parameter of 0 on the wire means 1, so `CSI H` and
/// `CSI 0;0 H` are both the top left corner
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Csi {
    Up(usize),
    Down(usize),
    Left(usize),
    Right(usize),
    /// `CHA`: to an absolute column on this row
    Column(usize),
    /// `VPA`: to an absolute row in this column, the symmetric half of `CHA`
    Row(usize),
    /// `CUP`/`HVP`: to an absolute (row, column)
    Position(usize, usize),
    /// `EL`: 0 to the end of the line, 1 to its start, 2 the whole line
    EraseLine(u8),
    /// `ED`: 0 to the end of the screen, 1 to its start, 2 all of it
    EraseDisplay(u8),
}

/// The eight, and their bright halves, with xterm's values, so a program's
/// `\x1b[32m` is the expected green
const SGR_BASE: [[u8; 3]; 8] = [
    [0, 0, 0], [205, 49, 49], [13, 188, 121], [229, 229, 16],
    [36, 114, 200], [188, 63, 188], [17, 168, 205], [229, 229, 229],
];
const SGR_BRIGHT: [[u8; 3]; 8] = [
    [102, 102, 102], [241, 76, 76], [35, 209, 139], [245, 245, 67],
    [59, 142, 234], [214, 112, 214], [41, 184, 219], [255, 255, 255],
];

/// The longest a sequence may be before the terminal decides it is not one
///
/// `\x1b[38;2;255;255;255m` is nineteen characters and the longest understood
/// here. The parser's state is on the grid, not on the writer, so a program
/// dying halfway through an escape leaves the terminal mid-sequence, and the
/// next program's text would be eaten by a sequence that never ends
const SEQ_MAX: u8 = 32;

/// The same bound for a string sequence, which carries a payload. A window
/// title is longer than the longest CSI, and a sequence whose terminator never
/// comes must not eat the terminal for ever
const STRING_MAX: u8 = 250;

impl Vt {
    pub const fn new() -> Self {
        Vt {
            state: 0, params: [0; 8], n: 0, cur: 0,
            seen_digit: false, pen: Pen::plain(), eaten: 0, private: false,
        }
    }

    /// One character in, one decision out
    pub fn feed(&mut self, ch: char) -> VtOut {
        // A sequence gone on too long was never a sequence. Back to ground, and
        // this character is text
        let limit = if self.state == 3 { STRING_MAX } else { SEQ_MAX };
        if self.state != 0 && self.eaten >= limit {
            self.state = 0;
            self.eaten = 0;
        }
        if self.state != 0 {
            self.eaten = self.eaten.saturating_add(1);
        }
        match self.state {
            // Ground
            0 => {
                if ch == '\x1b' {
                    self.state = 1;
                    self.eaten = 1;
                    VtOut::Eaten
                } else {
                    VtOut::Print(ch)
                }
            }
            // After ESC. `[` starts something understood here; `]`, `P`, `^`
            // and `_` start a string sequence, swallowed to its terminator;
            // anything else is a sequence with no parameters, dropped whole
            1 => {
                match ch {
                    '[' => {
                        self.state = 2;
                        self.n = 0;
                        self.cur = 0;
                        self.seen_digit = false;
                        self.private = false;
                    }
                    // OSC, DCS, PM, APC. All carry a payload and end with BEL
                    // or ST; without this `\x1b]0;title\x07`, how a ported
                    // program sets its window title, would print `0;title` as
                    // text
                    ']' | 'P' | '^' | '_' => self.state = 3,
                    _ => self.state = 0,
                }
                VtOut::Eaten
            }
            // Inside a string sequence: everything is payload until BEL, or
            // until an ESC, which is either the `ESC \` that ends it or the
            // start of something new; state 1 handles both
            3 => {
                match ch {
                    '\u{0007}' => self.state = 0,
                    '\u{001b}' => { self.state = 1; self.eaten = 1; }
                    _ => {}
                }
                VtOut::Eaten
            }
            // Inside CSI: digits and semicolons until a final byte
            _ => {
                match ch {
                    '0'..='9' => {
                        self.cur = self.cur.saturating_mul(10)
                            .saturating_add(ch as u16 - '0' as u16);
                        self.seen_digit = true;
                        VtOut::Eaten
                    }
                    ';' => {
                        self.push();
                        VtOut::Eaten
                    }
                    // Private-use and intermediate bytes: still the sequence,
                    // and a mark that its final byte is not the plain one
                    '?' | '<'..='>' | ' '..='/' => { self.private = true; VtOut::Eaten }
                    c if ('@'..='~').contains(&c) => {
                        self.push();
                        self.state = 0;
                        if self.private {
                            VtOut::Eaten
                        } else if c == 'm' {
                            VtOut::Pen(self.sgr())
                        } else {
                            match self.movement(c) {
                                Some(m) => VtOut::Move(m),
                                None => VtOut::Eaten,
                            }
                        }
                    }
                    // An ESC in the middle of a sequence abandons it and starts
                    // a new one, not dropping to ground, which would print the
                    // `[3C` of the following sequence as text. Two writers on
                    // one terminal interleave like this
                    '\u{001b}' => { self.state = 1; self.eaten = 1; VtOut::Eaten }
                    // Anything else inside a sequence ends it
                    _ => { self.state = 0; VtOut::Eaten }
                }
            }
        }
    }

    fn push(&mut self) {
        if self.n < self.params.len() {
            self.params[self.n] = if self.seen_digit { self.cur } else { 0 };
            self.n += 1;
        }
        self.cur = 0;
        self.seen_digit = false;
    }

    /// One parameter, or its default when left out
    ///
    /// A missing parameter and an explicit zero are the same thing on the wire
    /// and `push` stores both as 0, so "absent" becomes the default here: 1 for
    /// a movement (`CSI C` is one column), 0 for an erase (`CSI K` is to the
    /// end of the line)
    fn param(&self, i: usize, default: u16) -> u16 {
        if i >= self.n { return default; }
        let p = self.params[i];
        if p == 0 { default } else { p }
    }

    /// The final byte, as something to do to the grid. `None` for every
    /// sequence not implemented, which is dropped
    fn movement(&self, final_byte: char) -> Option<Csi> {
        // One-based on the wire, zero-based here. Clamped again where used,
        // against the grid's shape, unknown here
        let one = self.param(0, 1) as usize;
        Some(match final_byte {
            'A' => Csi::Up(one),
            'B' => Csi::Down(one),
            'C' => Csi::Right(one),
            'D' => Csi::Left(one),
            'G' | '`' => Csi::Column(one.saturating_sub(1)),
            'd' => Csi::Row(one.saturating_sub(1)),
            'H' | 'f' => Csi::Position(
                one.saturating_sub(1),
                (self.param(1, 1) as usize).saturating_sub(1),
            ),
            // `E` and `F` (down/up n lines and to column zero) are absent: they
            // would need a variant meaning two things at once, and callers
            // write `\r` followed by a movement. `CSI F` does arrive on this
            // machine, but as the End key, and input never reaches this parser
            'K' => Csi::EraseLine(self.param(0, 0).min(2) as u8),
            'J' => Csi::EraseDisplay(self.param(0, 0).min(2) as u8),
            _ => return None,
        })
    }

    /// What the parameters add up to. `None` is "the terminal's own colour"
    fn sgr(&mut self) -> Pen {
        let mut i = 0;
        // A bare `CSI m` is `CSI 0 m`
        if self.n == 0 { self.pen = Pen::plain(); return self.pen; }
        while i < self.n {
            let p = self.params[i];
            match p {
                0  => self.pen = Pen::plain(),
                1  => self.pen.attrs |= ATTR_BOLD,
                2  => self.pen.attrs |= ATTR_DIM,
                3  => self.pen.attrs |= ATTR_ITALIC,
                4  => self.pen.attrs |= ATTR_UNDERLINE,
                5 | 6 => self.pen.attrs |= ATTR_BLINK,
                7  => self.pen.attrs |= ATTR_REVERSE,
                8  => self.pen.attrs |= ATTR_HIDDEN,
                9  => self.pen.attrs |= ATTR_STRIKE,
                // 21 is "doubly underlined" in ECMA-48 and "bold off" in
                // practice. Both readings clear bold here
                21 | 22 => self.pen.attrs &= !(ATTR_BOLD | ATTR_DIM),
                23 => self.pen.attrs &= !ATTR_ITALIC,
                24 => self.pen.attrs &= !ATTR_UNDERLINE,
                25 => self.pen.attrs &= !ATTR_BLINK,
                27 => self.pen.attrs &= !ATTR_REVERSE,
                28 => self.pen.attrs &= !ATTR_HIDDEN,
                29 => self.pen.attrs &= !ATTR_STRIKE,

                // Bold brightens a named colour, as every palette assumes
                30..=37 => {
                    let idx = (p - 30) as usize;
                    self.pen.fg = Some(if self.pen.has(ATTR_BOLD) {
                        SGR_BRIGHT[idx]
                    } else {
                        SGR_BASE[idx]
                    });
                }
                90..=97 => self.pen.fg = Some(SGR_BRIGHT[(p - 90) as usize]),
                39 => self.pen.fg = None,

                40..=47 => {
                    self.pen.bg = Some(SGR_BASE[(p - 40) as usize]);
                    self.pen.attrs |= ATTR_BG_SET;
                }
                100..=107 => {
                    self.pen.bg = Some(SGR_BRIGHT[(p - 100) as usize]);
                    self.pen.attrs |= ATTR_BG_SET;
                }
                49 => { self.pen.bg = None; self.pen.attrs &= !ATTR_BG_SET; }

                // `38;2;R;G;B` and `38;5;N`, and the same two on 48 for the
                // background
                38 | 48 => {
                    let set_fg = p == 38;
                    let mut colour = None;
                    if i + 1 < self.n && self.params[i + 1] == 2 && i + 4 < self.n {
                        colour = Some([
                            self.params[i + 2].min(255) as u8,
                            self.params[i + 3].min(255) as u8,
                            self.params[i + 4].min(255) as u8,
                        ]);
                        i += 4;
                    } else if i + 2 < self.n && self.params[i + 1] == 5 {
                        colour = Some(xterm256(self.params[i + 2]));
                        i += 2;
                    }
                    if let Some(c) = colour {
                        if set_fg {
                            self.pen.fg = Some(c);
                        } else {
                            self.pen.bg = Some(c);
                            self.pen.attrs |= ATTR_BG_SET;
                        }
                    }
                }
                // Anything else is ignored
                _ => {}
            }
            i += 1;
        }
        self.pen
    }
}

/// Everything `SGR` can say about the text from here on
///
/// Reverse video is how `less`, `vim`, `htop` and every menu draw a selection.
///
/// `None` for a colour means the terminal's own, not any particular black or
/// white: a client picks its own background and the kernel console its own
/// foreground, and a cell naming one would stop following the terminal it is
/// drawn in
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Pen {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub attrs: u16,
}

// The attribute bits. The first eight are SGR's, in SGR's order
/// `SGR 1`
pub const ATTR_BOLD:      u16 = 1 << 0;
/// `SGR 2`
pub const ATTR_DIM:       u16 = 1 << 1;
/// `SGR 3`: carried, not drawn. The console has one face
pub const ATTR_ITALIC:    u16 = 1 << 2;
/// `SGR 4`
pub const ATTR_UNDERLINE: u16 = 1 << 3;
/// `SGR 5`: carried, not drawn. Blinking needs a clock and a repaint, and a
/// blinking terminal never idles
pub const ATTR_BLINK:     u16 = 1 << 4;
/// `SGR 7`
pub const ATTR_REVERSE:   u16 = 1 << 5;
/// `SGR 8`
pub const ATTR_HIDDEN:    u16 = 1 << 6;
/// `SGR 9`
pub const ATTR_STRIKE:    u16 = 1 << 7;
/// Not an SGR attribute: whether this cell's background was set at all.
///
/// A cell stores concrete bytes, and the terminal's own background is not a
/// colour: it is what the thing drawing this grid paints behind text. This bit
/// says "no opinion" without inventing a black that would stop following the
/// window
pub const ATTR_BG_SET:    u16 = 1 << 8;

impl Pen {
    pub const fn plain() -> Self {
        Pen { fg: None, bg: None, attrs: 0 }
    }

    #[inline]
    pub fn has(self, bit: u16) -> bool { self.attrs & bit != 0 }
}

/// What a terminal's output looks like to a master end
///
/// A fixed ring of the bytes a program wrote, in order, beside the parser: the
/// same bytes go into this and through `Vt` into the cells, from one place, in
/// one order.
///
/// `dev/terminal-architecture.md` warns about two independent producers of the
/// same bytes. Here there is one producer, the write, and two consumers
/// downstream of it: the write is the source of truth, and the grid is what it
/// renders to.
///
/// Fixed size and allocated once with the terminal: `write_str` is reachable
/// from `print!`, which is reachable from an interrupt handler. `io::tty`'s
/// rule: no allocation on any output path.
pub struct OutRing {
    buf: Vec<u8>,
    head: usize,
    len: usize,
    /// How many master descriptors are open on this terminal
    ///
    /// The ring is fed only while this is non-zero: with nobody to drain it, a
    /// filling ring would lose bytes or stop the writer. No master means the
    /// bytes go to the grid only
    masters: u32,
    /// Bytes dropped because the ring was full and nobody drained it, for a
    /// reader to learn it has fallen behind. Real backpressure, the writer
    /// waiting, is the output queue, a separate stage
    lost: u64,
}

/// How much a terminal holds for a master not keeping up: a buffer is flow
/// control, not storage
const OUT_RING_BYTES: usize = 8 * 1024;

impl OutRing {
    /// A ring that can never be opened, for a terminal that can never have a
    /// master
    ///
    /// tty0 is the machine's own panel, not a pty: `hangup` refuses it and
    /// nothing can open a master on it
    const fn none() -> Self {
        OutRing { buf: Vec::new(), head: 0, len: 0, masters: 0, lost: 0 }
    }

    fn new() -> Option<Self> {
        let mut buf = Vec::new();
        buf.try_reserve_exact(OUT_RING_BYTES).ok()?;
        buf.resize(OUT_RING_BYTES, 0);
        Some(OutRing { buf, head: 0, len: 0, masters: 0, lost: 0 })
    }

    /// Is anybody holding the other end?
    #[inline]
    fn open(&self) -> bool { self.masters > 0 }

    fn push(&mut self, bytes: &[u8]) {
        if !self.open() || self.buf.is_empty() { return; }
        for &b in bytes {
            if self.len == self.buf.len() {
                self.lost = self.lost.saturating_add(1);
                continue;
            }
            let at = (self.head + self.len) % self.buf.len();
            self.buf[at] = b;
            self.len += 1;
        }
    }

    /// Take up to `out.len()` bytes. Zero means nothing is waiting
    fn take(&mut self, out: &mut [u8]) -> usize {
        if self.buf.is_empty() { return 0; }
        let n = self.len.min(out.len());
        for slot in out.iter_mut().take(n) {
            *slot = self.buf[self.head];
            self.head = (self.head + 1) % self.buf.len();
            self.len -= 1;
        }
        n
    }

    fn pending(&self) -> usize { self.len }
}

/// The 256-colour cube, in the standard layout: 16 named, 216 of 6x6x6, 24
/// greys
fn xterm256(n: u16) -> [u8; 3] {
    match n {
        0..=7   => SGR_BASE[n as usize],
        8..=15  => SGR_BRIGHT[(n - 8) as usize],
        16..=231 => {
            let v = n - 16;
            let step = |x: u16| -> u8 { if x == 0 { 0 } else { (x * 40 + 55) as u8 } };
            [step(v / 36), step((v / 6) % 6), step(v % 6)]
        }
        _ => {
            let v = (n.min(255) - 232) as u8;
            let g = v.saturating_mul(10).saturating_add(8);
            [g, g, g]
        }
    }
}

pub const COLOR_MIKU: [u8; 3] = [57, 197, 187];
pub const COLOR_MIKU_DARK: [u8; 3] = [0, 150, 136];
pub const COLOR_MIKU_LIGHT: [u8; 3] = [128, 222, 217];
pub const COLOR_PINK: [u8; 3] = [255, 105, 140];
pub const COLOR_WHITE: [u8; 3] = [230, 240, 240];
pub const COLOR_GRAY: [u8; 3] = [120, 140, 140];
pub const COLOR_GREEN: [u8; 3] = [100, 220, 150];
pub const COLOR_YELLOW: [u8; 3] = [220, 220, 100];
pub const COLOR_CYAN: [u8; 3] = [0, 220, 220];

pub const BORDER_PADDING: usize = 10;
pub const CHAR_WIDTH: usize = 9;
const LINE_SPACING: usize = 2;
const CHAR_HEIGHT: usize = 16;
const LINE_HEIGHT: usize = CHAR_HEIGHT + LINE_SPACING;
const GLYPH_SIZE: usize = CHAR_WIDTH * CHAR_HEIGHT;

/// The grid's ceiling, in characters
///
/// A bound: `cols` and `rows` are worked out from the framebuffer and meet
/// these only at a very wide screen (1920 pixels are 212 columns). The pen
/// wraps on the column, so a ceiling below the screen is a narrower console,
/// and these clear the widest panel expected. The cost is
/// `MAX_COLS * MAX_ROWS * 4` bytes of heap, once
const MAX_COLS: usize = 256;
const MAX_ROWS: usize = 96;

const SHADOW_LINE_PIXELS: usize = MAX_COLS * CHAR_WIDTH;
const SHADOW_LINE_BYTES: usize = SHADOW_LINE_PIXELS * 4;

/// Rows kept after they have scrolled off the top
///
/// Not scrollback (nothing can look at them) but the input to a re-flow: a line
/// that ran off the right of a narrow console comes back when the console is
/// widened. `HIST_ROWS * MAX_COLS * 4` bytes, allocated once
const HIST_ROWS: usize = 256;

/// One character and the colour it was printed in.
///
/// `repr(C)` because these bytes leave the kernel: a program drawing the
/// console maps the grid and reads it as `may_console`'s cells. Field order is
/// the wire format, and so is the size: `protocol::console::CELL_BYTES`.
///
/// The character is four bytes, for Unicode text. A cell is then sixteen bytes,
/// and every offset in the grid, the history ring and the shared pool stays a
/// shift, on the path that copies a row per keystroke.
#[derive(Clone, Copy)]
#[repr(C)]
struct Cell {
    /// A `char`'s scalar value. `u32`, as this struct is reinterpreted as bytes
    /// on its way to a client, and `char` has a validity invariant a byte in a
    /// shared mapping cannot be trusted to keep: a client could write a
    /// surrogate into its own pool
    ch: u32,
    /// The colour of the text
    r: u8,
    g: u8,
    b: u8,
    /// The colour behind it, meaningful only with `ATTR_BG_SET`
    br: u8,
    bg: u8,
    bb: u8,
    /// Bold, underline, reverse and the rest; see `ATTR_*`
    attrs: u16,
    /// Keeps the struct sixteen bytes, so a cell index is a shift on the path
    /// that copies a row per keystroke. Named so the wire format has no silent
    /// hole, and where an underline colour would go.
    ///
    /// Four: `attrs` is two bytes with two-byte alignment, at offset 10, so the
    /// fields end at 12. Five would make seventeen, rounded up to twenty by
    /// `repr(C)` for the `u32`'s alignment, and readers indexing at
    /// `CELL_BYTES` would shift with every column
    _pad: [u8; 4],
}

// `CELL_BYTES` is what the protocol promises and the client and selftest index
// with; `size_of::<Cell>()` is what the kernel copies. A host model cannot
// catch a difference: it models the intended layout, not the compiler's padding
const _: () = assert!(
    core::mem::size_of::<Cell>() == crate::mayland::protocol::console::CELL_BYTES
);

impl Cell {
    const fn blank() -> Self {
        Self { ch: ' ' as u32, r: 0, g: 0, b: 0, br: 0, bg: 0, bb: 0, attrs: 0, _pad: [0; 4] }
    }

    /// A cell holding `ch` drawn with `pen`, over `fallback` when the pen has
    /// no foreground of its own
    fn new(ch: char, pen: Pen, fallback: [u8; 3]) -> Self {
        let [r, g, b] = pen.fg.unwrap_or(fallback);
        let [br, bg, bb] = pen.bg.unwrap_or([0, 0, 0]);
        Self { ch: ch as u32, r, g, b, br, bg, bb, attrs: pen.attrs, _pad: [0; 4] }
    }

    /// The colours to paint this cell in, reverse applied
    ///
    /// Reverse is applied here, not when the cell was written: a cell storing
    /// swapped colours could not be told from one whose colours were chosen so,
    /// and `\x1b[27m`, a re-flow or a repaint could not restore it
    fn colours(self, default_fg: [u8; 3], default_bg: Option<[u8; 3]>)
        -> ([u8; 3], Option<[u8; 3]>)
    {
        let fg = [self.r, self.g, self.b];
        let bg = if self.attrs & ATTR_BG_SET != 0 {
            Some([self.br, self.bg, self.bb])
        } else {
            default_bg
        };
        if self.attrs & ATTR_REVERSE != 0 {
            // Swapping needs both sides to be a colour, so the terminal's own
            // stands in for whichever was left unsaid
            (bg.unwrap_or(default_fg), Some(fg))
        } else {
            (fg, bg)
        }
    }

    #[inline]
    fn has(self, bit: u16) -> bool { self.attrs & bit != 0 }

    /// The character, or a space when the cell holds something that is not a
    /// scalar value. The kernel's writers start from a `char`; a client writing
    /// into the pool it shares can reach this, so no unwrap
    #[inline]
    fn glyph(self) -> char {
        char::from_u32(self.ch).unwrap_or(' ')
    }

    /// Is this cell blank? For the trailing-space trim and the repaint
    #[inline]
    fn is_blank(self) -> bool {
        self.ch == ' ' as u32
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FrameBufferConfig {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub is_bgr: bool,
}

/// The text half of a console: what it says, and nothing about how it looks
///
/// Everything here is per terminal. The pixels are not: there is one panel and
/// one set of glyphs on it, one `Painter` and many of these.
///
/// Nothing in here allocates after construction. The grid is built at its
/// ceiling and a resize moves text about inside it: a console is written to
/// from interrupt handlers under one lock
pub struct TextGrid {
    cells: Vec<Cell>,
    cols: usize,
    rows: usize,
    cur_col: usize,
    cur_row: usize,
    /// For each screen row: this row continues the one above, as the text ran
    /// off the right edge with no newline printed. `MAX_ROWS` long whatever
    /// `rows` is
    wrapped: Vec<bool>,
    /// Rows scrolled off the top, oldest at `hist_head`, as a ring of
    /// `HIST_ROWS` rows of `MAX_COLS` cells with the used length and the same
    /// continuation flag beside each
    hist: Vec<Cell>,
    hist_len: Vec<u16>,
    hist_wrap: Vec<bool>,
    hist_head: usize,
    hist_used: usize,
    /// Rows in the history ring. The console keeps `HIST_ROWS`; a terminal in a
    /// window keeps fewer. A field, as the ring arithmetic below has to agree
    /// with what was allocated
    hist_rows: usize,
    /// How many rows back the top of the displayed window is. `0` is the live
    /// screen.
    ///
    /// All of scrollback is this number. The shared pool a client draws from is
    /// allocated once at the grid's ceiling and never re-allocated, so
    /// scrollback is the same `rows` rows, taken from further back
    view: usize,
    /// Rows whose cells have changed since the last `take_row_damage`, as
    /// (row0, row1) with the far edge exclusive.
    ///
    /// Beside the painter's pixel `damage`: pixel damage is what was painted,
    /// nothing once the console is headless; this is what the text says, as a
    /// program drawing the console from the cell grid needs
    row_damage: Option<(usize, usize)>,
    /// Half-read escape sequences, and the colour the last finished one asked
    /// for. State, since a sequence can straddle two writes: a prompt printed
    /// in three `write`s may cut an escape in half
    vt: Vt,
    pen: Pen,
    /// What a master end reads, when there is one
    out: OutRing,
    /// Bells rung since anybody last looked.
    ///
    /// A count: two bells in one frame are two bells, and whoever answers wants
    /// to know it fell behind. There is no sound hardware in this design, so
    /// the screen is the only answer
    bell: u8,
}

impl TextGrid {
    /// A grid of its own, for a terminal that is not the console
    ///
    /// Allocated at the ceiling and used at the shape asked for, as the
    /// console's own grid is (see `Console::new`), so it can be reshaped
    /// without reallocating. A client reads the grid out of a shared pool
    /// holding nothing, so moved memory would be a window drawing from a page
    /// handed back. And this is written to under a lock with interrupts off.
    ///
    /// `None` when the shape is empty
    pub fn new(cols: usize, rows: usize) -> Option<Self> {
        if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
            return None;
        }
        // History is a quarter of the console's: a terminal in a window is
        // scrolled back far less often, and eight of these at the full ring
        // would be most of a megabyte
        let hist_rows = HIST_ROWS / 4;
        Some(Self {
            // At the ceiling, like the console's: the row stride is `cols` and
            // the live grid is the first `cols * rows` cells of it
            cells: vec![Cell::blank(); MAX_COLS * MAX_ROWS],
            cols,
            rows,
            cur_col: 0,
            cur_row: 0,
            wrapped: vec![false; MAX_ROWS],
            // `MAX_COLS`, not `cols`: the ring is indexed at that stride
            // wherever history is read or written
            hist: vec![Cell::blank(); hist_rows * MAX_COLS],
            hist_len: vec![0; hist_rows],
            hist_wrap: vec![false; hist_rows],
            hist_head: 0,
            hist_used: 0,
            hist_rows,
            view: 0,
            out: OutRing::new()?,
            bell: 0,
            row_damage: None,
            vt: Vt::new(),
            pen: Pen::plain(),
        })
    }


    /// Is the row below this one a continuation of it?
    fn continues(&self, row: usize) -> bool {
        row + 1 < self.rows && self.wrapped[row + 1]
    }

    /// Mark rows as changed, so whoever draws this grid is told
    fn dirty_rows(&mut self, r0: usize, r1: usize) {
        let r1 = r1.min(self.rows);
        if r0 >= r1 { return; }
        self.row_damage = Some(match self.row_damage {
            Some((o0, o1)) => (o0.min(r0), o1.max(r1)),
            None => (r0, r1),
        });
    }

    /// The next slot in the history ring, evicting the oldest when it is full
    fn hist_slot(&mut self) -> usize {
        let slot = (self.hist_head + self.hist_used) % self.hist_rows;
        if self.hist_used == self.hist_rows {
            self.hist_head = (self.hist_head + 1) % self.hist_rows;
        } else {
            self.hist_used += 1;
        }
        slot
    }

    /// Copy a row into the history before it leaves the screen
    fn retire_row(&mut self, row: usize) {
        let base = row * self.cols;
        let mut len = self.cols;
        // Blanks at the end of a row are padding on a line that ended and text
        // on a row that carries on. Where a line wrapped on a space, that space
        // is the last cell of the row before, and trimming it would weld the
        // words on either side at the next width change
        if !self.continues(row) {
            while len > 0 && self.cells[base + len - 1].is_blank() { len -= 1; }
        }
        let wrap = self.wrapped[row];
        let slot = self.hist_slot();
        let dst = slot * MAX_COLS;
        for i in 0..len {
            self.hist[dst + i] = self.cells[base + i];
        }
        self.hist_len[slot] = len as u16;
        self.hist_wrap[slot] = wrap;
    }

    /// Every row up by one, the top row into the history, the last one blank.
    ///
    /// The cells only. On the console the pixels move too, in `Console` after
    /// calling this: what the terminal says is here, what it looks like is
    /// there
    fn scroll(&mut self) {
        let (cols, rows) = (self.cols, self.rows);
        // The only place a row leaves the screen, and the only feeder of the
        // history a re-flow reads
        self.retire_row(0);
        let count = (rows - 1) * cols;
        self.cells.copy_within(cols..cols + count, 0);
        self.wrapped.copy_within(1..rows, 0);
        self.wrapped[rows - 1] = false;
        let last = (rows - 1) * cols;
        for i in 0..cols {
            self.cells[last + i] = Cell::blank();
        }
        self.dirty_rows(0, rows);
        self.cur_row = rows - 1;
    }

    /// The pen to the start of the next line, scrolling if it ran off the end.
    /// `wrap` says the line ended by running out of width, not on a printed
    /// newline, for a re-flow to put it back
    fn newline(&mut self, wrap: bool) {
        self.cur_col = 0;
        self.cur_row += 1;
        if self.cur_row >= self.rows {
            self.scroll();
        }
        if self.cur_row < self.rows {
            self.wrapped[self.cur_row] = wrap;
        }
    }

    /// One character into the grid at the pen, wrapping and scrolling as
    /// needed. Returns true when the line wrapped to make room
    fn put(&mut self, ch: char, pen: Pen, fallback: [u8; 3]) -> bool {
        let mut wrapped = false;
        // Wrapped on the column only: a grid handed to a client has no pixels
        if self.cur_col >= self.cols {
            self.newline(true);
            wrapped = true;
        }
        if self.cur_col < self.cols && self.cur_row < self.rows {
            let at = self.cur_row * self.cols + self.cur_col;
            self.cells[at] = Cell::new(ch, pen, fallback);
            let row = self.cur_row;
            self.dirty_rows(row, row + 1);
        }
        self.cur_col += 1;
        wrapped
    }

    /// The pen back one, and the cell it lands on blanked
    fn back(&mut self) {
        if self.cur_col == 0 { return; }
        self.cur_col -= 1;
        if self.cur_col < self.cols && self.cur_row < self.rows {
            self.cells[self.cur_row * self.cols + self.cur_col] = Cell::blank();
            let row = self.cur_row;
            self.dirty_rows(row, row + 1);
        }
    }

    // Cursor addressing
    //
    // Three functions, the safety argument for letting a program move the pen.
    // `apply_csi` computes numbers; `set_pen` and `blank_span` are the only two
    // places in this feature that touch `cur_row`, `cur_col` or index `cells`.
    // If those two are right, no sequence can reach outside the grid.

    /// The one pen clamp: land strictly inside the grid
    ///
    /// `put` tolerates `cur_col == cols` as a pending wrap: a character was
    /// written in the last column and the next one starts a new line, with the
    /// bound checked again before writing. A position from an escape sequence
    /// is absolute: "column 200" on an eighty-column terminal is the last
    /// column. Landing on `cols` would put the pen one past the end and let the
    /// next `CSI K` compute a span starting outside the row
    fn set_pen(&mut self, row: usize, col: usize) {
        if self.rows == 0 || self.cols == 0 { return; }
        self.cur_row = row.min(self.rows - 1);
        self.cur_col = col.min(self.cols - 1);
    }

    /// The one erase: blank `c0..c1` of `row`, clamped, and report it
    ///
    /// `wrapped` is cleared when the whole row goes. The flag means this row
    /// continues the one above, and `reflow` welds rows on it: a blanked row
    /// still claiming it would be glued onto the previous line at the next size
    /// change, leaving a hole in that line
    fn blank_span(&mut self, row: usize, c0: usize, c1: usize) {
        if row >= self.rows { return; }
        let c0 = c0.min(self.cols);
        let c1 = c1.min(self.cols);
        if c1 <= c0 { return; }
        let base = row * self.cols;
        for c in c0..c1 {
            self.cells[base + c] = Cell::blank();
        }
        if c0 == 0 && c1 == self.cols {
            self.wrapped[row] = false;
        }
        self.dirty_rows(row, row + 1);
    }

    /// Do one decoded sequence. Cells and the pen; never a pixel
    ///
    /// Answers with the rows whose cells changed, or `None` when the pen only
    /// moved. Row damage makes a client copy a row out of the grid, and an
    /// arrow key reporting damage would push a row across the protocol per
    /// keystroke. The compositor re-sends on a cursor that moved by itself; see
    /// `mayland::server::iface::console`
    fn apply_csi(&mut self, c: Csi) -> Option<(usize, usize)> {
        if self.rows == 0 || self.cols == 0 { return None; }
        let (row, col) = (self.cur_row, self.cur_col);
        match c {
            // Relative movement saturates at the edges and never scrolls.
            // Scrolling on `CUU` at the top row would let a program push the
            // screen into the history ring with arrow keys, and history is the
            // input to a re-flow
            Csi::Up(n)    => { self.set_pen(row.saturating_sub(n), col); None }
            Csi::Down(n)  => { self.set_pen(row.saturating_add(n), col); None }
            Csi::Left(n)  => { self.set_pen(row, col.saturating_sub(n)); None }
            Csi::Right(n) => { self.set_pen(row, col.saturating_add(n)); None }
            Csi::Column(c)      => { self.set_pen(row, c); None }
            Csi::Row(r)         => { self.set_pen(r, col); None }
            Csi::Position(r, c) => { self.set_pen(r, c); None }

            // Erasing does not move the pen: `CSI 2J` followed by `CSI H` is
            // the common idiom, and a moved pen would put the second one
            // elsewhere
            Csi::EraseLine(what) => {
                match what {
                    0 => self.blank_span(row, col, self.cols),
                    1 => self.blank_span(row, 0, col + 1),
                    _ => self.blank_span(row, 0, self.cols),
                }
                Some((row, row + 1))
            }

            Csi::EraseDisplay(what) => {
                let rows = self.rows;
                match what {
                    0 => {
                        self.blank_span(row, col, self.cols);
                        for r in row + 1..rows { self.blank_span(r, 0, self.cols); }
                        Some((row, rows))
                    }
                    1 => {
                        for r in 0..row { self.blank_span(r, 0, self.cols); }
                        self.blank_span(row, 0, col + 1);
                        Some((0, row + 1))
                    }
                    _ => {
                        for r in 0..rows { self.blank_span(r, 0, self.cols); }
                        // The history goes with it. Clearing is not scrolling,
                        // so nothing is retired into the ring, and the ring is
                        // emptied: a re-flow keeping it would put back on
                        // screen what was just cleared at the first size
                        // change, as in `Console::clear`
                        self.hist_head = 0;
                        self.hist_used = 0;
                        Some((0, rows))
                    }
                }
            }
        }
    }

    /// Text in, cells out: what a terminal with no pixels does with what is
    /// written to it.
    ///
    /// Put text on the grid, in `rgb` unless the text says otherwise. `rgb` is
    /// the caller's colour and the escape sequence's is stronger, since the
    /// escape came from the program whose text this is. It lasts until the
    /// program resets it (`\x1b[0m`), so a shell can draw a prompt in two
    /// colours and leave the terminal as it found it.
    ///
    /// Answers whether the terminal did anything with the text: the caller has
    /// somewhere else to send a line this terminal did not take, the console
    /// and the log. "Did anything" includes consuming an escape sequence: a
    /// program that only changed the colour has been served
    pub fn write_str(&mut self, text: &str, rgb: [u8; 3]) -> bool {
        // The tee. The same bytes, from one place, in one order: into the ring
        // a master end reads, and through the parser into the cells. One
        // producer and two consumers below it (`dev/terminal-architecture.md`).
        // It costs nothing when no master is open.
        //
        // The ring does not carry the `rgb` beside the text: colour arrives out
        // of band from `cprintln!`, with no byte sequence that reproduces it. A
        // program's own SGR escapes are in `text` and do reach the ring; only
        // the kernel's own coloured lines lose their colour on the way to a
        // master
        self.out.push(text.as_bytes());

        let mut acted = false;
        for ch in text.chars() {
            let ch = match self.vt.feed(ch) {
                VtOut::Print(c) => c,
                VtOut::Eaten => { acted = true; continue; }
                VtOut::Pen(p) => { self.pen = p; acted = true; continue; }
                VtOut::Move(c) => { self.apply_csi(c); acted = true; continue; }
            };
            let pen = self.pen;
            match ch {
                '\n' => { self.newline(false); acted = true; }
                '\r' => { self.cur_col = 0; acted = true; }
                '\x08' => { self.back(); acted = true; }
                // A form feed and a vertical tab move to the next line, as on
                // terminals generally
                '\u{000b}' | '\u{000c}' => { self.newline(false); acted = true; }
                '\t' => {
                    let stop = (self.cur_col / 8 + 1) * 8;
                    while self.cur_col < stop.min(self.cols) { self.put(' ', pen, rgb); }
                    acted = true;
                }
                // The bell. Nothing audible on this machine, so the screen
                // answers, through the terminal and not a cell: nothing is
                // printed
                '\u{0007}' => { self.bell = self.bell.saturating_add(1); acted = true; }
                // DEL and the C1 range are control characters, on the output
                // side as on the input side
                c if (c as u32) >= 0x20 && !(0x7f..0xa0).contains(&(c as u32)) => {
                    self.put(c, pen, rgb);
                    acted = true;
                }
                _ => {}
            }
        }
        acted
    }

    /// Which rows have changed since the last call, and clear it
    pub fn take_row_damage(&mut self) -> Option<(usize, usize)> {
        self.row_damage.take()
    }

    /// Where the pen is, in display coordinates
    ///
    /// Scrolled back, the pen is further down the window than in the grid, by
    /// the view offset, and past the bottom it is not on screen. A row past the
    /// end is reported then, which caret drawers refuse to draw
    pub fn cursor(&self) -> (usize, usize) {
        (self.cur_col, self.cur_row.saturating_add(self.view))
    }

    /// The grid's shape, in characters
    pub fn size(&self) -> (usize, usize) { (self.cols, self.rows) }

    /// Copy the whole grid out, as displayed (scrolled back if it is),
    /// row-major, four bytes a cell.
    ///
    /// Returns the grid's shape, or None when `dst` is too small: never a
    /// partial copy, as half a grid read as whole would show another's memory
    /// at the bottom
    pub fn copy_cells(&self, dst: &mut [u8]) -> Option<(usize, usize)> {
        if self.view != 0 {
            let cell = core::mem::size_of::<Cell>();
            let bytes = self.cols * self.rows * cell;
            if dst.len() < bytes { return None; }
            let mut row_buf = [Cell::blank(); MAX_COLS];
            for r in 0..self.rows {
                self.display_row(r, &mut row_buf);
                let at = r * self.cols * cell;
                let src = unsafe {
                    core::slice::from_raw_parts(row_buf.as_ptr() as *const u8, self.cols * cell)
                };
                dst[at..at + self.cols * cell].copy_from_slice(src);
            }
            return Some((self.cols, self.rows));
        }
        self.copy_cells_live(dst)
    }

    fn copy_cells_live(&self, dst: &mut [u8]) -> Option<(usize, usize)> {
        let bytes = self.cols * self.rows * core::mem::size_of::<Cell>();
        if dst.len() < bytes { return None; }
        // `cells` is allocated at the ceiling and used at the current shape,
        // with the row stride `cols`, so the live grid is the first
        // `cols * rows` cells and one copy is enough
        let src = unsafe {
            core::slice::from_raw_parts(self.cells.as_ptr() as *const u8, bytes)
        };
        dst[..bytes].copy_from_slice(src);
        Some((self.cols, self.rows))
    }

    /// The same, for a span of rows.
    ///
    /// A keystroke changes one row; copying all fifty-six would be forty-seven
    /// kilobytes where eight hundred do, with interrupts off, so the cost is
    /// latency on the keyboard being typed on
    pub fn copy_rows(&self, dst: &mut [u8], r0: usize, r1: usize)
        -> Option<(usize, usize)>
    {
        let (r0, r1) = (r0.min(self.rows), r1.min(self.rows));
        if r1 <= r0 || self.cols == 0 { return None; }
        let cell = core::mem::size_of::<Cell>();
        let stride = self.cols * cell;
        let (off, bytes) = (r0 * stride, (r1 - r0) * stride);
        if dst.len() < bytes { return None; }

        // The live screen is contiguous, so it stays one memcpy: the path a
        // keystroke takes, once a frame
        if self.view == 0 {
            let src = unsafe {
                core::slice::from_raw_parts(
                    (self.cells.as_ptr() as *const u8).add(off), bytes)
            };
            dst[..bytes].copy_from_slice(src);
            return Some((off, bytes));
        }

        // Scrolled back: the rows are not adjacent and some are not in `cells`,
        // so they are assembled one at a time
        let mut row_buf = [Cell::blank(); MAX_COLS];
        for (n, i) in (r0..r1).enumerate() {
            self.display_row(i, &mut row_buf);
            let at = n * stride;
            let src = unsafe {
                core::slice::from_raw_parts(row_buf.as_ptr() as *const u8, stride)
            };
            dst[at..at + stride].copy_from_slice(src);
        }
        Some((off, bytes))
    }

    // Re-wrapping
    //
    // `reflow` allocates, and this type's contract is no allocation on the
    // write path, where it is called from an interrupt. It is reached only from
    // `resize`, which comes from `tty::set_size` with nothing held.

    /// Re-wrap the text into a grid of a different shape
    ///
    /// What survives is the logical line, everything between two newlines, not
    /// the screen row, an artefact of the width it was printed at. Rows that no
    /// longer fit go back into the history, so narrowing the console and
    /// widening it gives the text back.
    ///
    /// It writes `cells` directly: `write_char` feeds the history through
    /// `scroll_up`, and a re-flow that way would add another copy of everything
    /// on screen to the history at every resize
    fn reflow(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 {
            return;
        }
        let lines = self.logical_lines();

        // Every logical line cut into rows of the new width. An empty line is
        // still a row: two newlines in a row are a blank line
        let mut phys: Vec<(Vec<Cell>, bool)> = Vec::new();
        for line in &lines {
            if line.is_empty() {
                phys.push((Vec::new(), false));
                continue;
            }
            for (i, chunk) in line.chunks(cols).enumerate() {
                phys.push((chunk.to_vec(), i > 0));
            }
        }

        // The newest rows are the ones on screen and the rest go back into the
        // history, which is rebuilt: `lines` was made out of it
        let keep = phys.len().min(rows);
        let drop_n = phys.len() - keep;
        self.hist_head = 0;
        self.hist_used = 0;
        self.cols = cols;
        self.rows = rows;
        for (row, wrap) in &phys[..drop_n] {
            self.push_history_row(row, *wrap);
        }

        for c in self.cells.iter_mut() { *c = Cell::blank(); }
        for w in self.wrapped.iter_mut() { *w = false; }
        for (i, (row, wrap)) in phys[drop_n..].iter().enumerate() {
            let base = i * cols;
            self.cells[base..base + row.len()].copy_from_slice(row);
            self.wrapped[i] = *wrap;
        }

        // The pen goes back to the end of the last line, where the shell
        // carries on printing
        self.cur_row = keep.saturating_sub(1);
        self.cur_col = phys.last().map(|(r, _)| r.len()).unwrap_or(0).min(cols);
        
        // Every row is now a different row's worth of text
        self.dirty_rows(0, rows);
    }

    /// The text as whole lines, oldest first: the history followed by what is
    /// on screen, with every row marked as a continuation glued back onto the
    /// line it was cut out of
    fn logical_lines(&self) -> Vec<Vec<Cell>> {
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        fn add(lines: &mut Vec<Vec<Cell>>, row: &[Cell], wrap: bool) {
            match lines.last_mut() {
                // A continuation with nothing before it (its line has fallen
                // out of the history) starts a line
                Some(last) if wrap => last.extend_from_slice(row),
                _ => lines.push(row.to_vec()),
            }
        }

        for i in 0..self.hist_used {
            let slot = (self.hist_head + i) % self.hist_rows;
            let len = self.hist_len[slot] as usize;
            let base = slot * MAX_COLS;
            add(&mut lines, &self.hist[base..base + len], self.hist_wrap[slot]);
        }
        // Only as far down as the pen: the rows below it were never written to
        let last = self.cur_row.min(self.rows.saturating_sub(1));
        for r in 0..=last {
            let base = r * self.cols;
            let mut len = self.cols;
            // Only where the line ends here; see `retire_row`
            if !self.continues(r) {
                while len > 0 && self.cells[base + len - 1].is_blank() { len -= 1; }
            }
            add(&mut lines, &self.cells[base..base + len], self.wrapped[r]);
        }
        lines
    }

    fn push_history_row(&mut self, row: &[Cell], wrap: bool) {
        let len = row.len().min(MAX_COLS);
        let slot = self.hist_slot();
        let base = slot * MAX_COLS;
        self.hist[base..base + len].copy_from_slice(&row[..len]);
        self.hist_len[slot] = len as u16;
        self.hist_wrap[slot] = wrap;
    }

    // Scrollback
    //
    // The rows that have left the screen are in the history ring `scroll`
    // feeds. The client's pool is allocated once at the grid's ceiling and
    // never re-allocated, so the window shows the same `rows` rows, taken from
    // further back. One number does all of it.

    /// Somebody opened a master end. The ring starts being fed
    pub fn master_opened(&mut self) {
        self.out.masters = self.out.masters.saturating_add(1);
    }

    /// Closed one. Answers whether that was the last
    pub fn master_closed(&mut self) -> bool {
        self.out.masters = self.out.masters.saturating_sub(1);
        if self.out.masters == 0 {
            // Nothing will read what is left, and whoever opens a master next
            // must not get stale output
            self.out.head = 0;
            self.out.len = 0;
            true
        } else {
            false
        }
    }

    pub fn master_count(&self) -> u32 { self.out.masters }

    /// Take what a program has written, for a master end to read
    pub fn out_take(&mut self, buf: &mut [u8]) -> usize { self.out.take(buf) }

    /// How much is waiting, without taking it, for `poll`
    pub fn out_pending(&self) -> usize { self.out.pending() }

    /// Bytes a master was too slow to collect
    pub fn out_lost(&self) -> u64 { self.out.lost }

    /// Bells rung since this was last asked. Destructive, like row damage:
    /// whoever answers a bell answers each one once
    pub fn take_bells(&mut self) -> u8 {
        core::mem::take(&mut self.bell)
    }

    /// How far back this terminal can be scrolled
    pub fn history_depth(&self) -> usize { self.hist_used }

    /// Where the window is. `0` is the live screen
    pub fn view(&self) -> usize { self.view }

    /// Move the window, clamped to what there is. Answers with where it landed
    ///
    /// Everything on screen is different text afterwards, so the whole grid is
    /// marked, as `ConsoleGrid` does for a reshape
    pub fn set_view(&mut self, rows_back: usize) -> usize {
        let want = rows_back.min(self.hist_used);
        if want != self.view {
            self.view = want;
            let rows = self.rows;
            self.dirty_rows(0, rows);
        }
        self.view
    }

    /// Back to the live screen, if it was not there already
    ///
    /// What a keystroke does: a terminal scrolled back while someone types
    /// would show one thing and send another. Output does not do this, or
    /// scrollback would be useless on a busy log
    pub fn snap_to_live(&mut self) -> bool {
        if self.view == 0 { return false; }
        self.view = 0;
        let rows = self.rows;
        self.dirty_rows(0, rows);
        true
    }

    /// The cells of one displayed row, from the live grid or from the history
    /// behind it
    ///
    /// Display row `i` is absolute row `(hist_used - view) + i` of the history
    /// followed by the screen. With `view == 0` that is `hist_used + i`, live
    /// row `i`, so the live case falls out of the same arithmetic
    fn display_row(&self, i: usize, out: &mut [Cell]) {
        let n = self.cols.min(out.len());
        for c in out[..n].iter_mut() { *c = Cell::blank(); }
        if n == 0 { return; }
        let abs = (self.hist_used - self.view) + i;
        if abs < self.hist_used {
            // Out of the ring, oldest at `hist_head`
            let slot = (self.hist_head + abs) % self.hist_rows;
            let base = slot * MAX_COLS;
            // A history row is stored at its used length; the rest of the
            // display row is blank, as it was on screen
            let len = (self.hist_len[slot] as usize).min(n);
            out[..len].copy_from_slice(&self.hist[base..base + len]);
        } else {
            let row = abs - self.hist_used;
            if row < self.rows {
                let base = row * self.cols;
                out[..n].copy_from_slice(&self.cells[base..base + n]);
            }
        }
    }

    /// Reshape a grid with no pixels behind it, keeping the text
    ///
    /// A configure changing the window's character count arrives whenever any
    /// other window opens or closes, so this must not clear the screen.
    ///
    /// What survives is the logical line, everything between two newlines,
    /// re-wrapped to the new width (`reflow`), not the screen row.
    ///
    /// Every row is marked changed, so whoever is drawing repaints all of it.
    /// The allocation `reflow` needs is fine here and would not be on the write
    /// path: this is reached from `tty::set_size`, with nothing held.
    pub fn resize(&mut self, cols: usize, rows: usize) -> bool {
        if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
            return false;
        }
        if cols != self.cols {
            // A different width means different lines. The height alone does
            // not
            self.reflow(cols, rows);
            // The window was scrolled back through a ring just rebuilt, so
            // where it pointed means nothing
            self.view = self.view.min(self.hist_used);
            return true;
        }
        let (old_cols, old_rows) = (self.cols, self.rows);
        if (cols, rows) == (old_cols, old_rows) {
            return true;
        }

        let keep_rows = old_rows.min(rows);
        let keep_cols = old_cols.min(cols);

        // A row's place in the buffer is `row * cols`, so changing the width
        // moves every row. Copying through one row of scratch lets the two
        // layouts overlap; the direction keeps a row from being written over
        // its own source. Narrower: every row lands at or before where it was,
        // so forwards; wider: at or after, so backwards, as `memmove` does
        let mut scratch = [Cell::blank(); MAX_COLS];
        let mut move_row = |cells: &mut Vec<Cell>, r: usize| {
            let src = r * old_cols;
            let dst = r * cols;
            scratch[..keep_cols].copy_from_slice(&cells[src..src + keep_cols]);
            cells[dst..dst + keep_cols].copy_from_slice(&scratch[..keep_cols]);
            // The new part of the row is blank, not whatever the old layout
            // left at those offsets
            for c in keep_cols..cols {
                cells[dst + c] = Cell::blank();
            }
        };
        if cols <= old_cols {
            for r in 0..keep_rows { move_row(&mut self.cells, r); }
        } else {
            for r in (0..keep_rows).rev() { move_row(&mut self.cells, r); }
        }

        // Rows the grid did not have before
        for r in keep_rows..rows {
            let dst = r * cols;
            for c in 0..cols {
                self.cells[dst + c] = Cell::blank();
            }
        }
        // A continuation flag belongs to a row that no longer exists
        for w in self.wrapped.iter_mut().skip(keep_rows) { *w = false; }

        self.cols = cols;
        self.rows = rows;
        // The pen stays where it was if the new shape has room for it, so a
        // shell carries on printing under its own prompt
        self.cur_col = self.cur_col.min(cols - 1);
        self.cur_row = self.cur_row.min(rows - 1);
        self.touch_rows(0, rows);
        true
    }

    /// Blank every cell and send the pen home, keeping the shape, for `clear`
    pub fn clear(&mut self) {
        for c in self.cells.iter_mut() { *c = Cell::blank(); }
        for w in self.wrapped.iter_mut() { *w = false; }
        // And the history: a resize would otherwise put back every line that
        // had scrolled off, as in `Console::clear`
        self.hist_head = 0;
        self.hist_used = 0;
        self.cur_col = 0;
        self.cur_row = 0;
        let rows = self.rows;
        self.touch_rows(0, rows);
    }

    /// The largest shape this grid can be reshaped to
    pub const fn capacity() -> (usize, usize) { (MAX_COLS, MAX_ROWS) }

    /// Mark rows as changed, so a client drawing this grid is told
    pub fn touch_rows(&mut self, r0: usize, r1: usize) {
        self.row_damage = Some(match self.row_damage {
            Some((a, b)) => (a.min(r0), b.max(r1)),
            None => (r0, r1),
        });
    }
}

pub struct Console {
    grid: TextGrid,
    fb_ptr: *mut u8,
    fb_len: usize,
    width: usize,
    height: usize,
    stride_bytes: usize,
    bpp: usize,
    pub x_pos: usize,
    pub y_pos: usize,
    pub fg_color: [u8; 3],
    /// What `\x1b[0m` goes back to. The console's own colour, kept separately
    /// because `fg_color` is what is being written in now: `print_colored`
    /// moves it for one line and puts it back
    base_color: [u8; 3],
    /// Everything SGR has said, for the console's own writer. The colour half
    /// is `fg_color` beside it, which the `print!` sites set directly
    pen: Pen,
    vt: Vt,
    pub is_bgr: bool,
    shadow_line: Vec<u32>,
    /// Pixels drawn into since the last `take_damage`, as (x0, y0, x1, y1) with
    /// the far edges exclusive.
    ///
    /// Of interest when the framebuffer is RAM that something else has to copy
    /// out of; see `FLUSH_HOOK`. Two comparisons per drawing call save pushing
    /// a whole 1920x1080 screen for a typed character
    damage: Option<(usize, usize, usize, usize)>,
    /// The panel this console's pixels are copied to, while it draws into the
    /// shadow `shadow_panel` made; `None` on a console drawing on the panel
    /// itself, or before there was a heap
    mirror: Option<Mirror>,
    /// Pixels drawn since the last copy to the panel: `damage` again, kept
    /// apart so the copy never takes it from the GPU's flush hook or a window
    mirror_damage: Option<(usize, usize, usize, usize)>,
    /// Stop painting; keep the text.
    ///
    /// With it set, everything that writes a cell runs and everything that
    /// writes a pixel does not, so the console keeps its text, its wrapping,
    /// its scrolling and its history while someone else decides what it looks
    /// like.
    ///
    /// The framebuffer is not given up: `fill_rect` and `blit_xrgb` keep
    /// working, as on a machine with no GPU the compositor reaches the panel
    /// through this same object
    headless: bool,
}

unsafe impl Send for Console {}
unsafe impl Sync for Console {}

impl Console {
    pub fn new_limine(framebuffer: &'static mut [u8], config: FrameBufferConfig) -> Self {
        let cols = ((config.width.saturating_sub(BORDER_PADDING)) / CHAR_WIDTH).min(MAX_COLS);
        let rows = ((config.height.saturating_sub(BORDER_PADDING)) / LINE_HEIGHT).min(MAX_ROWS);
        // The grid is allocated at its ceiling and never grown: a resize moves
        // text about inside it and must not allocate, as the console is written
        // to from interrupt handlers under one lock
        let cells = vec![Cell::blank(); MAX_COLS * MAX_ROWS];
        let stride_bytes = config.stride * config.bytes_per_pixel;
        let fb_len = framebuffer.len();
        let fb_ptr = framebuffer.as_mut_ptr();
        // Row-wise, as in `blank_area`, by hand since this runs before there is
        // a `Console` to call it on
        {
            let row_bytes = config.width * config.bytes_per_pixel;
            for y in 0..config.height {
                let base = y * stride_bytes;
                if base + row_bytes > fb_len { break; }
                unsafe { core::ptr::write_bytes(fb_ptr.add(base), 0, row_bytes); }
            }
        }

        let shadow_width = config.width;
        let shadow_line = vec![0u32; shadow_width];

        Self {
            grid: TextGrid {
                cells,
                cols,
                rows,
                cur_col: 0,
                cur_row: 0,
                wrapped: vec![false; MAX_ROWS],
                hist: vec![Cell::blank(); HIST_ROWS * MAX_COLS],
                hist_len: vec![0; HIST_ROWS],
                hist_wrap: vec![false; HIST_ROWS],
                hist_head: 0,
                hist_used: 0,
                hist_rows: HIST_ROWS,
                view: 0,
                // tty0 has no master and can never have one: it is the
                // machine's own panel, not a pty
                out: OutRing::none(),
                bell: 0,
                row_damage: None,
            vt: Vt::new(),
            pen: Pen::plain(),
            },
            fb_ptr,
            fb_len,
            width: config.width,
            height: config.height,
            stride_bytes,
            bpp: config.bytes_per_pixel,
            x_pos: BORDER_PADDING,
            y_pos: BORDER_PADDING,
            fg_color: COLOR_MIKU,
            pen: Pen::plain(),
            base_color: COLOR_MIKU,
            vt: Vt::new(),
            is_bgr: config.is_bgr,
            shadow_line,
            damage: None,
            mirror: None,
            mirror_damage: None,
            headless: false,
        }
    }

    /// Record that (x, y, w, h) was drawn into
    #[inline]
    fn dirty(&mut self, x: usize, y: usize, w: usize, h: usize) {
        if w == 0 || h == 0 || x >= self.width || y >= self.height {
            return;
        }
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        let grow = |d: Option<(usize, usize, usize, usize)>| Some(match d {
            Some((ox0, oy0, ox1, oy1)) => (ox0.min(x), oy0.min(y), ox1.max(x1), oy1.max(y1)),
            None => (x, y, x1, y1),
        });
        self.damage = grow(self.damage);
        if self.mirror.is_some() {
            self.mirror_damage = grow(self.mirror_damage);
        }
    }

    /// Store what was drawn since the last call from the shadow to the panel:
    /// whole rows of the dirtied band, what a line or a scroll touches anyway,
    /// and only while the console draws into the shadow. A console retargeted
    /// onto a GPU's buffer or parked is not copied, and coming back to the
    /// shadow restarts the copies. Stores only; the panel is never read
    fn mirror_now(&mut self) {
        let Some(m) = self.mirror else { return };
        if self.fb_ptr != m.shadow {
            return;
        }
        let Some((x0, y0, x1, y1)) = self.mirror_damage.take() else { return };
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let n = (x1 - x0) * bpp;
        for y in y0..y1 {
            let off = y * sb + x0 * bpp;
            if off + n > self.fb_len || off + n > m.panel.len {
                break;
            }
            unsafe { core::ptr::copy_nonoverlapping(m.shadow.add(off), m.panel.ptr.add(off), n) };
        }
        // Write-combining: the tail stays in a fill buffer until something
        // pushes it out
        unsafe { core::arch::x86_64::_mm_sfence() };
    }

    /// Record that rows `r0..r1` of the character grid changed
    #[inline]
    fn dirty_rows(&mut self, r0: usize, r1: usize) { self.grid.dirty_rows(r0, r1) }

    /// What has changed since the last call, and clear it
    pub fn take_damage(&mut self) -> Option<(usize, usize, usize, usize)> {
        self.damage.take()
    }

    /// Which rows of the grid have changed since the last call, and clear it
    pub fn take_row_damage(&mut self) -> Option<(usize, usize)> {
        self.grid.row_damage.take()
    }

    /// Where the pen is, in characters
    pub fn cursor(&self) -> (usize, usize) {
        (self.grid.cur_col, self.grid.cur_row)
    }

    pub fn is_headless(&self) -> bool {
        self.headless
    }

    /// Stop or resume painting the text. Returns what it was
    ///
    /// Turning it back off repaints: whatever was drawing instead has left
    /// other content in the framebuffer
    pub fn set_headless(&mut self, on: bool) -> bool {
        let was = self.headless;
        self.headless = on;
        if was && !on {
            self.repaint_all();
        }
        was
    }

    /// Re-wrap the text to a grid given in characters.
    ///
    /// The counterpart of `resize_view` for a console nobody is painting: the
    /// caller owns the font and can turn a window size into columns. False when
    /// the grid is empty or bigger than the console can hold, and nothing
    /// changes: a window too small for one character keeps its text
    pub fn set_grid(&mut self, cols: usize, rows: usize) -> bool {
        if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
            return false;
        }
        if (cols, rows) == (self.grid.cols, self.grid.rows) {
            return true;
        }
        self.reflow(cols, rows);
        if !self.headless {
            self.repaint_all();
        }
        true
    }

    /// Copy the character grid out, row-major, four bytes a cell.
    ///
    /// Returns the grid's shape, or None when `dst` is too small: never a
    /// partial copy, as half a grid read as whole would show another's memory
    /// at the bottom
    pub fn copy_cells(&self, dst: &mut [u8]) -> Option<(usize, usize)> {
        self.grid.copy_cells(dst)
    }

    /// The text half, for anything about what the console says, not how it
    /// looks
    pub fn grid(&self) -> &TextGrid { &self.grid }
    pub fn grid_mut(&mut self) -> &mut TextGrid { &mut self.grid }

    /// The same, for a span of rows.
    ///
    /// A keystroke changes one row; copying all fifty-six would be forty-seven
    /// kilobytes where eight hundred bytes do, with interrupts off, so the cost
    /// is latency on the keyboard being typed on.
    ///
    /// `r0..r1` is clamped to the live grid, and the answer gives the bytes
    /// written and their offset into a full-grid buffer, so the caller places
    /// them without the stride arithmetic
    pub fn copy_rows(&self, dst: &mut [u8], r0: usize, r1: usize) -> Option<(usize, usize)> {
        let r0 = r0.min(self.grid.rows);
        let r1 = r1.min(self.grid.rows);
        if r1 <= r0 || self.grid.cols == 0 {
            return None;
        }
        let cell = core::mem::size_of::<Cell>();
        let stride = self.grid.cols * cell;
        let (off, bytes) = (r0 * stride, (r1 - r0) * stride);
        if dst.len() < bytes {
            return None;
        }
        // As in `copy_cells`: the row stride is `cols`, so a span of rows is
        // one contiguous run and one copy
        let src = unsafe {
            core::slice::from_raw_parts(
                (self.grid.cells.as_ptr() as *const u8).add(off), bytes)
        };
        dst[..bytes].copy_from_slice(src);
        Some((off, bytes))
    }

    /// Move the console onto a different framebuffer.
    ///
    /// For the GB206, which takes the display away from the framebuffer GRUB
    /// handed over, so the console follows onto a surface reached a different
    /// way. Everything on screen comes across: the console keeps its own
    /// character grid and repaints from it, so the prompt and the line being
    /// typed are still there.
    ///
    /// The old framebuffer is dropped: it belongs to whoever passed it in, and
    /// on this path it is an aperture that no longer points anywhere
    pub fn retarget(&mut self, fb: &'static mut [u8], config: FrameBufferConfig) {
        let cols = ((config.width.saturating_sub(BORDER_PADDING)) / CHAR_WIDTH).min(MAX_COLS);
        let rows = ((config.height.saturating_sub(BORDER_PADDING)) / LINE_HEIGHT).min(MAX_ROWS);
        if cols == 0 || rows == 0 || self.grid.cols == 0 || self.grid.rows == 0 {
            return;
        }

        self.fb_len = fb.len();
        self.fb_ptr = fb.as_mut_ptr();
        self.width = config.width;
        self.height = config.height;
        self.stride_bytes = config.stride * config.bytes_per_pixel;
        self.bpp = config.bytes_per_pixel;
        self.is_bgr = config.is_bgr;
        self.shadow_line = vec![0u32; config.width];
        // Whole lines, re-wrapped to the new width. Cutting the grid would lose
        // the right-hand end of every longer line
        self.reflow(cols, rows);

        // Everything, including the black around the text: what was on the new
        // surface must not show through
        self.repaint_all();
    }

    /// Change the console's size without moving it to another framebuffer
    ///
    /// For the console as a window. Its tile is a rectangle of the same shadow
    /// anchored at the top left, so the pointer, the length and the stride stay
    /// and only the extent changes.
    ///
    /// Not `retarget`, which takes a `&'static mut [u8]`: that would be a
    /// second slice over memory this console already holds
    pub fn resize_view(&mut self, width: usize, height: usize) -> bool {
        let cols = ((width.saturating_sub(BORDER_PADDING)) / CHAR_WIDTH).min(MAX_COLS);
        let rows = ((height.saturating_sub(BORDER_PADDING)) / LINE_HEIGHT).min(MAX_ROWS);
        if cols == 0 || rows == 0 {
            return false;
        }
        self.width = width;
        self.height = height;
        self.reflow(cols, rows);
        // Including the area outside the text: the tile changed shape, and what
        // the compositor drew over the part of the shadow now ours is not the
        // console's
        self.repaint_all();
        true
    }



    /// Whether the row below `row` is the same logical line carried on
    ///
    /// `wrapped[r]` records why row `r` began: true when the text ran off the
    /// right edge of `r - 1`. So "does this row continue" is asked of the next
    /// row
    #[inline]
    fn continues(&self, row: usize) -> bool { self.grid.continues(row) }

    /// The slot the next history row goes into, dropping the oldest when the
    /// ring is full
    fn hist_slot(&mut self) -> usize { self.grid.hist_slot() }


    /// Keep a row about to leave the top of the screen
    ///
    /// Cell by cell, as both sides are in `self`. At most `MAX_COLS` cells,
    /// once per scrolled line
    fn retire_row(&mut self, row: usize) { self.grid.retire_row(row) }

    /// Clear the framebuffer and draw the whole character grid onto it again,
    /// marking every pixel damaged so the next flush pushes all of it.
    ///
    /// Two callers: `retarget`, moving onto a surface someone else was using,
    /// and the compositor claiming the screen. The console only repaints the
    /// rectangle it dirtied, so what another owner drew over it would otherwise
    /// stay.
    ///
    /// The clear is to black: erasing a character (`clear_rect`) writes black,
    /// so a coloured background would come back as black boxes wherever a
    /// character is rubbed out
    pub fn repaint_all(&mut self) {
        self.blank_area();
        self.repaint_cells();
        self.dirty(0, 0, self.width, self.height);
    }

    /// Blank the console's own area, row by row
    ///
    /// The buffer may be wider than the console: as a window it draws into a
    /// tile of a framebuffer it does not own all of, and one contiguous fill
    /// would write over whatever shares its scanlines
    fn blank_area(&mut self) {
        if self.headless { return; }
        let (sb, row_bytes) = (self.stride_bytes, self.width * self.bpp);
        for y in 0..self.height {
            let base = y * sb;
            if base + row_bytes > self.fb_len {
                break;
            }
            unsafe { core::ptr::write_bytes(self.fb_ptr.add(base), 0, row_bytes); }
        }
    }

    /// Draw every cell of the grid. The framebuffer is assumed already cleared
    fn repaint_cells(&mut self) {
        let rows = self.grid.rows;
        self.repaint_rows(0, rows);
    }

    /// Redraw a span of rows from the cells, blanking their pixels first
    ///
    /// The whole grid on a 212x56 console is nearly twelve thousand cells and a
    /// glyph rasterisation for each non-space, with interrupts off, and `CSI K`
    /// fires on every keystroke of a line editor. This is the same band
    /// `copy_rows` sends to a client
    fn repaint_rows(&mut self, r0: usize, r1: usize) {
        let r0 = r0.min(self.grid.rows);
        let r1 = r1.min(self.grid.rows);
        if r1 <= r0 { return; }
        // The pixels under the band, so a cell that became blank stops being
        // drawn. `clear_rect` and `render_glyph` are both headless-guarded, so
        // this function is free on a console with no framebuffer
        self.clear_rect(
            0,
            BORDER_PADDING + r0 * LINE_HEIGHT,
            self.width,
            (r1 - r0) * LINE_HEIGHT,
        );
        let saved = self.fg_color;
        let base = self.base_color;
        for row in r0..r1 {
            for col in 0..self.grid.cols {
                let cell = self.grid.cells[row * self.grid.cols + col];
                let x = BORDER_PADDING + col * CHAR_WIDTH;
                let y = BORDER_PADDING + row * LINE_HEIGHT;
                // The console's own background is black, a cleared framebuffer,
                // so `None` here leaves the pixels as the clear left them, and
                // a plain cell costs nothing to draw
                let (fg, bg) = cell.colours(base, None);
                if let Some(c) = bg {
                    self.fill_rect(x, y, CHAR_WIDTH, LINE_HEIGHT, c[0], c[1], c[2]);
                }
                // A blank cell with no background and no line through it has
                // nothing to draw: most of a screen
                if cell.is_blank()
                    && bg.is_none()
                    && !cell.has(ATTR_UNDERLINE)
                    && !cell.has(ATTR_STRIKE)
                {
                    continue;
                }
                if !cell.has(ATTR_HIDDEN) && !cell.is_blank() {
                    let [r, g, b] = fg;
                    self.fg_color = fg;
                    self.render_char_at(cell.glyph(), x, y, r, g, b);
                }
                // Underline and strike are a row of pixels each. Bold is in the
                // colour (SGR's brightening) and the console has one face, so
                // weight is carried in the cell and not drawn
                if cell.has(ATTR_UNDERLINE) {
                    self.fill_rect(x, y + LINE_HEIGHT - 2, CHAR_WIDTH, 1, fg[0], fg[1], fg[2]);
                }
                if cell.has(ATTR_STRIKE) {
                    self.fill_rect(x, y + LINE_HEIGHT / 2, CHAR_WIDTH, 1, fg[0], fg[1], fg[2]);
                }
            }
        }
        self.fg_color = saved;
    }

    /// Re-wrap, then put the pixel pen where the cell pen ended up
    ///
    /// The text half is the grid's, shared with every pty; this is specific to
    /// a console: `x_pos`/`y_pos` are a second record of the pen, and a re-flow
    /// moves it
    fn reflow(&mut self, cols: usize, rows: usize) {
        self.grid.reflow(cols, rows);
        self.sync_pen();
    }

    /// Put the pixel pen where the cell pen is
    ///
    /// `x_pos`/`y_pos` are the framebuffer's idea of where the next character
    /// goes and `grid.cur_col`/`cur_row` the text's, and they have to agree: a
    /// mismatch paints one line of text on top of another and stores it
    /// correctly
    #[inline]
    fn sync_pen(&mut self) {
        self.x_pos = BORDER_PADDING + self.grid.cur_col * CHAR_WIDTH;
        self.y_pos = BORDER_PADDING + self.grid.cur_row * LINE_HEIGHT;
    }

    pub fn clear(&mut self) {
        for c in self.grid.cells.iter_mut() { *c = Cell::blank(); }
        for w in self.grid.wrapped.iter_mut() { *w = false; }
        // The history goes too. Only a re-flow reads it, and a re-flow keeping
        // it would put back what was just cleared at the first size change
        self.grid.hist_head = 0;
        self.grid.hist_used = 0;
        self.blank_area();
        self.grid.cur_col = 0;
        self.grid.cur_row = 0;
        self.sync_pen();
        self.dirty(0, 0, self.width, self.height);
        let rows = self.grid.rows;
        self.dirty_rows(0, rows);
    }

    #[inline]
    fn new_line(&mut self) {
        self.advance_line(false);
    }

    /// Move the pen to the next row, recording why
    ///
    /// A row that begins because the text ran off the right edge is part of the
    /// line above it; one that begins on a printed newline is a line of its
    /// own. Nothing on screen tells them apart afterwards
    fn advance_line(&mut self, wrap: bool) {
        self.grid.cur_col = 0;
        self.grid.cur_row += 1;
        self.x_pos = BORDER_PADDING;
        self.y_pos += LINE_HEIGHT;
        if self.grid.cur_row >= self.grid.rows {
            self.scroll_up();
        }
        if self.grid.cur_row < self.grid.rows {
            self.grid.wrapped[self.grid.cur_row] = wrap;
        }
    }

    fn scroll_up(&mut self) {
        let rows = self.grid.rows;
        let sb = self.stride_bytes;
        // The cells (the row into the history, everything up by one, the last
        // one blank) are the grid's, the same on every terminal. What follows
        // is for a terminal with pixels
        self.grid.scroll();
        // Row by row, each bounded on its own.
        //
        // One memmove of the whole block would move the right bytes (the shift
        // is a whole number of scanlines), but it moves the padding too, and
        // its single bounds check is all-or-nothing: a buffer one row short
        // means the cells scroll and the pixels do not, out of step from then
        // on. Per row, a short buffer costs one row
        if self.headless {
            self.y_pos = BORDER_PADDING + self.grid.cur_row * LINE_HEIGHT;
            return;
        }
        let top = BORDER_PADDING * sb;
        let row_bytes = self.width * self.bpp;
        let moved = (rows - 1) * LINE_HEIGHT;
        for y in 0..moved {
            let dst = top + y * sb;
            let src = dst + LINE_HEIGHT * sb;
            if src + row_bytes > self.fb_len {
                break;
            }
            unsafe {
                core::ptr::copy(self.fb_ptr.add(src), self.fb_ptr.add(dst), row_bytes);
            }
        }
        for y in moved..rows * LINE_HEIGHT {
            let base = top + y * sb;
            if base + row_bytes > self.fb_len {
                break;
            }
            unsafe { core::ptr::write_bytes(self.fb_ptr.add(base), 0, row_bytes); }
        }
        self.grid.cur_row = rows - 1;
        self.y_pos = BORDER_PADDING + self.grid.cur_row * LINE_HEIGHT;
        // Every text row moved up by one, so the whole text area changed. The
        // expensive case for a flush hook that pays per pixel
        self.dirty(0, BORDER_PADDING, self.width, rows * LINE_HEIGHT);
    }

    pub fn backspace(&mut self) {
        if self.x_pos > BORDER_PADDING {
            self.x_pos -= CHAR_WIDTH;
            if self.grid.cur_col > 0 { self.grid.cur_col -= 1; }
            self.clear_char_at_cursor();
            if self.grid.cur_col < self.grid.cols && self.grid.cur_row < self.grid.rows {
                self.grid.cells[self.grid.cur_row * self.grid.cols + self.grid.cur_col] = Cell::blank();
                let row = self.grid.cur_row;
                self.dirty_rows(row, row + 1);
            }
        }
    }

    #[inline]
    pub fn clear_char_at_cursor(&mut self) {
        self.clear_rect(self.x_pos, self.y_pos, CHAR_WIDTH, CHAR_HEIGHT);
    }

    #[inline]
    fn clear_rect(&mut self, x: usize, y: usize, w: usize, h: usize) {
        if self.headless { return; }
        // Clipped to the console, not to the buffer: past its right edge is
        // another's scanline
        if x >= self.width { return; }
        let w = w.min(self.width - x);
        self.dirty(x, y, w, h);
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let byte_w = w * bpp;
        let base = sb * y + x * bpp;
        if base + sb * (h.saturating_sub(1)) + byte_w <= self.fb_len {
            let mut p = unsafe { self.fb_ptr.add(base) };
            for _ in 0..h {
                unsafe {
                    core::ptr::write_bytes(p, 0, byte_w);
                    p = p.add(sb);
                }
            }
        }
    }

    fn clear_columns(&mut self, start_x: usize, count: usize) {
        if count == 0 { return; }
        let pixel_w = count * CHAR_WIDTH;
        self.clear_rect(start_x, self.y_pos, pixel_w, CHAR_HEIGHT);
        let start_col = start_x.saturating_sub(BORDER_PADDING) / CHAR_WIDTH;
        let end_col = (start_col + count).min(self.grid.cols);
        if self.grid.cur_row < self.grid.rows && start_col < end_col {
            let row_off = self.grid.cur_row * self.grid.cols;
            self.grid.cells[row_off + start_col..row_off + end_col].fill(Cell::blank());
            let row = self.grid.cur_row;
            self.dirty_rows(row, row + 1);
        }
    }

    fn paint_cursor(&mut self, x: usize, r: u8, g: u8, b: u8) {
        if self.headless { return; }
        // Two pixels wide, and inset by a row top and bottom
        if x + 2 > self.width { return; }
        self.dirty(x, self.y_pos, 2, CHAR_HEIGHT);
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let pixel = self.make_pixel(r, g, b);
        let y0 = self.y_pos;
        for y in 1..CHAR_HEIGHT - 1 {
            let row_off = sb * (y0 + y) + x * bpp;
            self.put_pixel_unchecked(row_off, pixel);
            self.put_pixel_unchecked(row_off + bpp, pixel);
        }
    }

    fn redraw_input_fast(
        &mut self,
        start_x: usize,
        buf: &[u8],
        len: usize,
        cursor: usize,
        old_len: usize,
        dirty_start: usize,
    ) {
        let saved = self.fg_color;
        self.fg_color = [255, 255, 255];

        let redraw_from = dirty_start.min(len);
        let clear_from_col = redraw_from;
        let clear_count = if old_len > redraw_from { old_len - redraw_from } else { 0 };
        let clear_x = start_x + clear_from_col * CHAR_WIDTH;

        let total_clear = if old_len > len { old_len - redraw_from } else { len - redraw_from };
        let total_clear = total_clear.max(clear_count);
        if total_clear > 0 {
            self.clear_columns(clear_x, total_clear + 1);
        }

        self.x_pos = start_x + redraw_from * CHAR_WIDTH;
        self.grid.cur_col = (self.x_pos.saturating_sub(BORDER_PADDING)) / CHAR_WIDTH;
        for i in redraw_from..len {
            // Clipped, not wrapped. `write_char` starts a new line at the last
            // column, and an input line that wrapped would put the caret on the
            // wrong character: the shell works out the cursor with pixel
            // arithmetic (`shell::cursor_x`) that knows nothing about rows. A
            // line reaching the edge wants horizontal scrolling, hence
            // `text_cols`
            if self.grid.cur_col >= self.grid.cols {
                break;
            }
            self.write_char(buf[i] as char);
        }

        self.fg_color = saved;

        let cx = start_x + cursor * CHAR_WIDTH;
        self.x_pos = cx;
        self.grid.cur_col = cx.saturating_sub(BORDER_PADDING) / CHAR_WIDTH;
    }

    fn write_single_char_at(&mut self, x: usize, ch: u8, r: u8, g: u8, b: u8) {
        let color = [r, g, b];
        let c = ch as char;
        // The cell as well as the pixels. This is the line editor's fast path,
        // one character appended at the end of what is being typed; anything
        // that redraws from the grid (a re-flow, a repaint, a program drawing
        // the console) needs it stored
        let col = x.saturating_sub(BORDER_PADDING) / CHAR_WIDTH;
        if col < self.grid.cols && self.grid.cur_row < self.grid.rows {
            self.grid.cells[self.grid.cur_row * self.grid.cols + col] =
                Cell::new(ch as char, Pen { fg: Some([r, g, b]), ..Pen::plain() }, [r, g, b]);
            let row = self.grid.cur_row;
            self.dirty_rows(row, row + 1);
        }
        self.clear_rect(x, self.y_pos, CHAR_WIDTH, CHAR_HEIGHT);

        if let Some((glyph, _)) = crate::io::framebuffer::font::get_glyph(c) {
            self.render_glyph(glyph, x, self.y_pos, color);
        } else if let Some(raster) = noto_sans_mono_bitmap::get_raster(
            c,
            noto_sans_mono_bitmap::FontWeight::Regular,
            noto_sans_mono_bitmap::RasterHeight::Size16,
        ) {
            self.render_raster(&raster, x, self.y_pos, color);
        }
    }

    fn write_char(&mut self, c: char) {
        // The same parser the grid runs: a program's colours are the program's,
        // on whichever terminal
        let c = match self.vt.feed(c) {
            VtOut::Print(ch) => ch,
            VtOut::Eaten => return,
            VtOut::Pen(pen) => {
                // The whole pen, not only its colour. The console draws the
                // attributes it can (reverse, background, underline, strike)
                // and carries the rest in the cell for whoever redraws it
                self.pen = pen;
                self.fg_color = pen.fg.unwrap_or(self.base_color);
                return;
            }
            // The cells are the grid's to change, by the same code a pty runs;
            // this half is the pixels catching up. A pure move costs the two
            // multiplications in `sync_pen`; an erase repaints the band it
            // blanked
            VtOut::Move(cmd) => {
                let touched = self.grid.apply_csi(cmd);
                self.sync_pen();
                if let Some((r0, r1)) = touched {
                    self.repaint_rows(r0, r1);
                }
                return;
            }
        };
        match c {
            '\n' => self.new_line(),
            '\x08' => self.backspace(),
            // Column zero without a new line, as the grid does. A line editor
            // sends one before every redraw
            '\r' => { self.grid.cur_col = 0; self.sync_pen(); }
            // The same stop arithmetic as the grid, so a tab is the same width
            // on either kind of terminal
            '\t' => {
                let stop = (self.grid.cur_col / 8 + 1) * 8;
                while self.grid.cur_col < stop.min(self.grid.cols) {
                    self.write_char(' ');
                }
            }
            ch => {
                // Wrapped on the column only.
                //
                // The pixel is not consulted. Where the console paints, `cols`
                // comes from the same width, and they disagree only when `cols`
                // is capped: at 1920 wide the grid stops at MAX_COLS, and
                // characters past it would be painted, never stored, and lost
                // at the next repaint. Where the console does not paint,
                // `width` is a leftover: the last window the kernel drew this
                // text in, as a client taking `may_console` sets only the grid.
                // Both renderers clip to the right edge themselves
                if self.grid.cur_col >= self.grid.cols {
                    self.advance_line(true);
                }
                self.clear_char_at_cursor();
                if self.grid.cur_col < self.grid.cols && self.grid.cur_row < self.grid.rows {
                    let base = self.base_color;
                    self.grid.cells[self.grid.cur_row * self.grid.cols + self.grid.cur_col] =
                        Cell::new(ch, self.pen, base);
                    let row = self.grid.cur_row;
                    self.dirty_rows(row, row + 1);
                }
                if let Some((glyph, _)) = crate::io::framebuffer::font::get_glyph(ch) {
                    self.render_glyph(glyph, self.x_pos, self.y_pos, self.fg_color);
                } else if let Some(raster) = noto_sans_mono_bitmap::get_raster(
                    ch,
                    noto_sans_mono_bitmap::FontWeight::Regular,
                    noto_sans_mono_bitmap::RasterHeight::Size16,
                ) {
                    self.render_raster(&raster, self.x_pos, self.y_pos, self.fg_color);
                }
                self.x_pos += CHAR_WIDTH;
                self.grid.cur_col += 1;
            }
        }
    }

    #[inline]
    fn render_glyph(&mut self, glyph: &[u8; GLYPH_SIZE], px: usize, py: usize, color: [u8; 3]) {
        if self.headless { return; }
        let [r, g, b] = color;
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        // A glyph running past the console's right edge is not drawn at all,
        // and the cell behind it is not stored
        if px + CHAR_WIDTH > self.width { return; }
        let base = sb * py + px * bpp;
        if base + sb * (CHAR_HEIGHT - 1) + CHAR_WIDTH * bpp > self.fb_len { return; }
        self.dirty(px, py, CHAR_WIDTH, CHAR_HEIGHT);
        let is_bgr = self.is_bgr;
        let solid = Self::pixel_value(r, g, b, is_bgr);

        if bpp == 4 {
            let shadow = &mut self.shadow_line[..CHAR_WIDTH];
            let fb = self.fb_ptr;

            for y in 0..CHAR_HEIGHT {
                let row_base = base + sb * y;
                let glyph_row = y * CHAR_WIDTH;
                let mut any_pixel = false;

                for x in 0..CHAR_WIDTH {
                    let i = glyph[glyph_row + x];
                    if i == 255 {
                        shadow[x] = solid;
                        any_pixel = true;
                    } else if i > 0 {
                        let i16 = i as u16;
                        let pr = ((r as u16 * i16 + 127) >> 8) as u8;
                        let pg = ((g as u16 * i16 + 127) >> 8) as u8;
                        let pb = ((b as u16 * i16 + 127) >> 8) as u8;
                        shadow[x] = Self::pixel_value(pr, pg, pb, is_bgr);
                        any_pixel = true;
                    } else {
                        shadow[x] = 0;
                    }
                }

                if any_pixel {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            shadow.as_ptr() as *const u8,
                            fb.add(row_base),
                            CHAR_WIDTH * 4,
                        );
                    }
                }
            }
        } else {
            for y in 0..CHAR_HEIGHT {
                let row_base = base + sb * y;
                let glyph_row = y * CHAR_WIDTH;
                let mut p = unsafe { self.fb_ptr.add(row_base) };
                for x in 0..CHAR_WIDTH {
                    let i = glyph[glyph_row + x];
                    if i > 0 {
                        let i16 = i as u16;
                        let pr = ((r as u16 * i16 + 127) >> 8) as u8;
                        let pg = ((g as u16 * i16 + 127) >> 8) as u8;
                        let pb = ((b as u16 * i16 + 127) >> 8) as u8;
                        unsafe {
                            if is_bgr {
                                *p = pb; *p.add(1) = pg; *p.add(2) = pr;
                            } else {
                                *p = pr; *p.add(1) = pg; *p.add(2) = pb;
                            }
                        }
                    }
                    p = unsafe { p.add(3) };
                }
            }
        }
    }

    #[inline]
    fn render_raster(&mut self, raster: &noto_sans_mono_bitmap::RasterizedChar, px: usize, py: usize, color: [u8; 3]) {
        if self.headless { return; }
        let [r, g, b] = color;
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let is_bgr = self.is_bgr;
        let fb = self.fb_ptr;
        // Clipped to the console's right edge, the same as a glyph is
        if px >= self.width { return; }
        let room = self.width - px;
        self.dirty(px, py, raster.width().min(room), raster.height());
        for (y, row) in raster.raster().iter().enumerate() {
            let row_base = sb * (py + y) + px * bpp;
            if bpp == 4 {
                let rlen = row.len().min(self.shadow_line.len()).min(room);
                let shadow = &mut self.shadow_line[..rlen];
                let mut any = false;
                for (x, byte) in row.iter().enumerate() {
                    if x >= rlen { break; }
                    if *byte > 0 {
                        let i = *byte as u16;
                        let pr = ((r as u16 * i + 127) >> 8) as u8;
                        let pg = ((g as u16 * i + 127) >> 8) as u8;
                        let pb = ((b as u16 * i + 127) >> 8) as u8;
                        shadow[x] = Self::pixel_value(pr, pg, pb, is_bgr);
                        any = true;
                    } else {
                        shadow[x] = 0;
                    }
                }
                if any {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            shadow.as_ptr() as *const u8,
                            fb.add(row_base),
                            rlen * 4,
                        );
                    }
                }
            } else {
                let mut p = unsafe { fb.add(row_base) };
                for (x, byte) in row.iter().enumerate() {
                    if x >= room { break; }
                    if *byte > 0 {
                        let i = *byte as u16;
                        let pr = ((r as u16 * i + 127) >> 8) as u8;
                        let pg = ((g as u16 * i + 127) >> 8) as u8;
                        let pb = ((b as u16 * i + 127) >> 8) as u8;
                        unsafe {
                            if is_bgr {
                                *p = pb; *p.add(1) = pg; *p.add(2) = pr;
                            } else {
                                *p = pr; *p.add(1) = pg; *p.add(2) = pb;
                            }
                        }
                    }
                    p = unsafe { p.add(3) };
                }
            }
        }
    }

    pub fn render_char_at(&mut self, c: char, px: usize, py: usize, r: u8, g: u8, b: u8) {
        let color = [r, g, b];
        if let Some((glyph, _)) = crate::io::framebuffer::font::get_glyph(c) {
            self.render_glyph(glyph, px, py, color);
        } else if let Some(raster) = noto_sans_mono_bitmap::get_raster(
            c,
            noto_sans_mono_bitmap::FontWeight::Regular,
            noto_sans_mono_bitmap::RasterHeight::Size16,
        ) {
            self.render_raster(&raster, px, py, color);
        }
    }

    pub fn write_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        self.write_pixel_direct(x, y, r, g, b);
    }

    /// Console size in character cells
    pub fn text_size(&self) -> (usize, usize) {
        (self.grid.cols, self.grid.rows)
    }

    pub fn fb_width(&self) -> usize { self.width }
    pub fn fb_height(&self) -> usize { self.height }
    pub fn fb_is_bgr(&self) -> bool { self.is_bgr }

    /// Paint a filled rectangle at (x, y) with the given dimensions. Pixels
    /// outside the framebuffer are clipped. Uses memset for the common 32bpp
    /// case when the colour bytes repeat, otherwise a row-by-row fill.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8) {
        if w == 0 || h == 0 || x >= self.width || y >= self.height { return; }
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        self.dirty(x, y, w, h);
        let pixel = self.make_pixel(r, g, b);
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        for row in 0..h {
            let base = sb * (y + row) + x * bpp;
            if base + w * bpp > self.fb_len { break; }
            unsafe {
                let mut p = self.fb_ptr.add(base);
                if bpp == 4 {
                    for _ in 0..w {
                        (p as *mut u32).write_unaligned(pixel);
                        p = p.add(4);
                    }
                } else {
                    for _ in 0..w {
                        *p         = pixel as u8;
                        *p.add(1)  = (pixel >> 8) as u8;
                        *p.add(2)  = (pixel >> 16) as u8;
                        p = p.add(bpp);
                    }
                }
            }
        }
    }

    /// Copy a rectangle of XRGB8888 pixels straight onto the panel.
    ///
    /// For the compositor on a machine whose framebuffer is the only way to the
    /// screen: no GPU copy engine, no second surface, the processor writing
    /// rows. `src` is the whole client buffer and `src_pitch` its stride in
    /// bytes, so a caller passes the rectangle
    ///
    /// # Safety
    /// `src` must be readable for `src_pitch * (sy + h)` bytes
    pub unsafe fn blit_xrgb(
        &mut self,
        src: *const u8,
        src_pitch: usize,
        sx: usize,
        sy: usize,
        dx: usize,
        dy: usize,
        w: usize,
        h: usize,
    ) {
        if w == 0 || h == 0 || dx >= self.width || dy >= self.height {
            return;
        }
        let w = w.min(self.width - dx);
        let h = h.min(self.height - dy);
        self.dirty(dx, dy, w, h);
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        for row in 0..h {
            let dst_base = sb * (dy + row) + dx * bpp;
            if dst_base + w * bpp > self.fb_len {
                break;
            }
            let s = src.add((sy + row) * src_pitch + sx * 4);
            let d = self.fb_ptr.add(dst_base);
            if bpp == 4 && self.is_bgr {
                // The client's buffer is in the panel's own order, so the whole
                // row is one copy
                core::ptr::copy_nonoverlapping(s, d, w * 4);
            } else {
                for i in 0..w {
                    let p = s.add(i * 4);
                    let (b, g, r) = (*p, *p.add(1), *p.add(2));
                    let pixel = self.make_pixel(r, g, b);
                    let q = d.add(i * bpp);
                    if bpp == 4 {
                        (q as *mut u32).write_unaligned(pixel);
                    } else {
                        *q = pixel as u8;
                        *q.add(1) = (pixel >> 8) as u8;
                        *q.add(2) = (pixel >> 16) as u8;
                    }
                }
            }
        }
    }

    /// Paint a horizontal gradient from 'left' to 'right' across the given
    /// rectangle, for colour-bar tests.
    pub fn fill_hgradient(
        &mut self,
        x: usize, y: usize, w: usize, h: usize,
        left: (u8, u8, u8), right: (u8, u8, u8),
    ) {
        if w == 0 || h == 0 { return; }
        for col in 0..w {
            let t = col as u32;
            let denom = (w.max(1) - 1).max(1) as u32;
            let lerp = |a: u8, b: u8| -> u8 {
                let a = a as i32;
                let b = b as i32;
                (a + (b - a) * t as i32 / denom as i32) as u8
            };
            let r = lerp(left.0, right.0);
            let g = lerp(left.1, right.1);
            let b = lerp(left.2, right.2);
            self.fill_rect(x + col, y, 1, h, r, g, b);
        }
    }

    #[inline(always)]
    pub fn write_pixel_direct(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height { return; }
        self.dirty(x, y, 1, 1);
        let off = self.stride_bytes * y + x * self.bpp;
        let pixel = self.make_pixel(r, g, b);
        self.put_pixel_unchecked(off, pixel);
    }

    #[inline(always)]
    fn pixel_value(r: u8, g: u8, b: u8, is_bgr: bool) -> u32 {
        if is_bgr {
            0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | (b as u32)
        } else {
            0xFF00_0000 | (b as u32) << 16 | (g as u32) << 8 | (r as u32)
        }
    }

    #[inline(always)]
    fn make_pixel(&self, r: u8, g: u8, b: u8) -> u32 {
        Self::pixel_value(r, g, b, self.is_bgr)
    }

    #[inline(always)]
    fn put_pixel_unchecked(&mut self, off: usize, pixel: u32) {
        if self.bpp == 4 {
            if off + 4 <= self.fb_len {
                unsafe { (self.fb_ptr.add(off) as *mut u32).write_unaligned(pixel); }
            }
        } else {
            if off + 3 <= self.fb_len {
                unsafe {
                    let p = self.fb_ptr.add(off);
                    *p = pixel as u8;
                    *p.add(1) = (pixel >> 8) as u8;
                    *p.add(2) = (pixel >> 16) as u8;
                }
            }
        }
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.write_char(c);
        }
        Ok(())
    }
}

/// Terminal size in character cells, for TIOCGWINSZ. Falls back to a
/// conventional 80x25 when the framebuffer console is not up yet
pub fn text_size() -> (usize, usize) {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| w.text_size()).unwrap_or((80, 25))
    })
}

// Selftest
//
// Here, not in `mayland::selftest`, because everything it asks about is private
// to `Console`: the cell grid, the pen, and whether a write stayed inside the
// console's own width. Called from there, so one command runs everything.
//
// Nothing here touches the real console. Each case builds a `Console` of its
// own over a heap buffer

/// Run `f` with a `Console` over a scratch buffer of the given geometry
///
/// The buffer is a `Vec` that outlives the `Console`: the closure returns
/// before it is dropped, which justifies the `'static` the constructor asks
/// for. A `static mut` array would cost the image half a megabyte of zeroes
fn with_scratch_console<R>(
    width: usize, height: usize, stride: usize, fill: u8,
    f: impl FnOnce(&mut Console, &[u8]) -> R,
) -> R {
    let mut buf = vec![fill; stride * height * 4];
    let ptr = buf.as_mut_ptr();
    let len = buf.len();
    let fb: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    let mut console = Console::new_limine(fb, FrameBufferConfig {
        width, height, stride, bytes_per_pixel: 4, is_bgr: true,
    });
    let out = f(&mut console, unsafe { core::slice::from_raw_parts(ptr, len) });
    drop(console);
    drop(buf);
    out
}

pub fn selftest() -> Vec<(&'static str, bool, alloc::string::String)> {
    use alloc::format;
    use core::fmt::Write;
    let mut out = Vec::new();

    // Nothing is written outside the console's own width
    //
    // As a window the console draws into a tile of a framebuffer with wider
    // rows, and a contiguous fill would smear across the scanline
    {
        const W: usize = 200;
        const H: usize = 126;
        const PAD: usize = 64;
        let (intact, first_bad) = with_scratch_console(W, H, W + PAD, 0xAB, |c, mem| {
            for i in 0..40 {
                let _ = write!(c, "line {} with some text on it\n", i);
            }
            c.repaint_all();
            c.clear();
            let _ = write!(c, "after the clear\n");

            // Every byte of every row past the console's own width
            let mut bad = None;
            for y in 0..H {
                for x in W..(W + PAD) {
                    let at = (y * (W + PAD) + x) * 4;
                    if mem[at..at + 4].iter().any(|b| *b != 0xAB) {
                        bad = Some((x, y));
                        break;
                    }
                }
                if bad.is_some() { break; }
            }
            (bad.is_none(), bad)
        });
        out.push(("console: nothing is drawn outside its own width", intact,
                  format!("{} padding columns checked over {} rows, first stray at {:?}",
                          PAD, H, first_bad)));
    }

    // Every painted column is a stored cell
    //
    // The pen wraps at `cols`, so at any width past MAX_COLS * 9 nothing is
    // drawn and then lost at the next repaint
    {
        const W: usize = 1600;
        const H: usize = 90;
        let (row, col, cols, stored) = with_scratch_console(W, H, W, 0, |c, _| {
            let cols = c.grid.cols;
            for _ in 0..cols + 5 {
                let _ = write!(c, "x");
            }
            // Characters on the row that was filled, counted from the cells
            let stored = (0..cols).filter(|&i| !c.grid.cells[i].is_blank()).count();
            (c.grid.cur_row, c.grid.cur_col, cols, stored)
        });
        out.push(("console: every painted column is a stored cell",
                  row == 1 && col == 5 && stored == cols,
                  format!("{} columns: {} stored on the first row, pen at row {} col {}",
                          cols, stored, row, col)));
    }

    // A grid wider than the framebuffer wraps at the grid
    //
    // Once a client holds `may_console` the framebuffer is not this console's:
    // `width` is the last window the kernel painted the text in, and nothing
    // updates it. Wrapping at it would keep a 135-column terminal at 67
    // characters a line
    {
        // cols = (400 - 10) / 9 = 43, and the grid is nearly three times that
        const W: usize = 400;
        const H: usize = 200;
        const GRID: usize = 120;
        const RUN: usize = 100;
        let (took, row, col, stored) = with_scratch_console(W, H, W, 0, |c, _| {
            c.set_headless(true);
            let took = c.set_grid(GRID, 20);
            for _ in 0..RUN {
                let _ = write!(c, "x");
            }
            let stored = (0..GRID).filter(|&i| !c.grid.cells[i].is_blank()).count();
            (took, c.grid.cur_row, c.grid.cur_col, stored)
        });
        out.push(("console: a grid wider than the framebuffer wraps at the grid",
                  took && row == 0 && col == RUN && stored == RUN,
                  format!("{} characters into a {}-column grid over a {}-pixel \
                           framebuffer: grid taken {}, {} stored on the first row, \
                           pen at row {} col {}",
                          RUN, GRID, W, took, stored, row, col)));
    }

    // A resize re-wraps lines instead of cutting them
    //
    // A narrower screen has no room for the right-hand end of a long line. It
    // is kept in the history and put back when there is room again
    {
        // cols = (370 - 10) / 9 = 40, rows = (190 - 10) / 18 = 10
        const W: usize = 370;
        const H: usize = 190;
        const LONG: usize = 100;

        let screen = |c: &Console| -> alloc::string::String {
            (0..c.grid.rows).map(|r| {
                let base = r * c.grid.cols;
                let row: alloc::string::String = (0..c.grid.cols)
                    .map(|i| c.grid.cells[base + i].glyph()).collect();
                let mut row = alloc::string::String::from(row.trim_end());
                row.push('\n');
                row
            }).collect()
        };
        let lines = |c: &Console| -> Vec<alloc::string::String> {
            c.grid.logical_lines().iter()
                .map(|l| l.iter().map(|x| x.glyph()).collect())
                .collect()
        };

        let (wide_cols, kept, narrow_kept, back, pen, pen_back) =
            with_scratch_console(W, H, W, 0, |c, _| {
                // Enough to scroll, so the history is carrying part of it
                for i in 0..12 {
                    let _ = write!(c, "line {}\n", i);
                }
                for _ in 0..LONG { let _ = write!(c, "A"); }
                let _ = write!(c, "\ntail");

                let before = screen(c);
                let pen = (c.grid.cur_row, c.grid.cur_col);
                let wide_cols = c.grid.cols;

                // Half the width: the long line no longer fits on three rows
                c.reflow(20, 10);
                let narrow = lines(c).iter()
                    .any(|l| l.len() == LONG && l.chars().all(|ch| ch == 'A'));

                // And back again
                c.reflow(wide_cols, 10);
                (wide_cols, before, narrow, screen(c), pen, (c.grid.cur_row, c.grid.cur_col))
            });

        out.push(("console: a resize re-wraps lines instead of cutting them",
                  narrow_kept && kept == back && pen == pen_back,
                  format!("{} columns down to 20 and back: the {}-character line \
                           survived {}, the screen came back {}, pen {:?} then {:?}",
                          wide_cols, LONG, narrow_kept, kept == back, pen, pen_back)));
    }

    // A line that wrapped on a space keeps the space
    //
    // Blanks at the end of a row are padding when the line ended there and text
    // when it did not, so `retire_row` and `logical_lines` trim only the first.
    // "the built-in bindings are up" must not come back as "bindingsare up". It
    // takes a console narrow enough to wrap in the middle of the sentence
    {
        // cols = (379 - 10) / 9 = 41, which puts the space before "are" in the
        // last cell of the first row
        const W: usize = 379;
        const H: usize = 190;
        const SENTENCE: &str = "no config file, so the built-in bindings are up";

        let joined = with_scratch_console(W, H, W, 0, |c, _| {
            let _ = write!(c, "{}", SENTENCE);
            // Wide enough to hold it on one row, so the two rows are joined
            c.reflow(80, 10);
            c.grid.logical_lines()
                .iter()
                .map(|l| l.iter().map(|x| x.glyph()).collect::<alloc::string::String>())
                .find(|l| l.contains("bindings"))
                .unwrap_or_default()
        });

        out.push(("console: a line that wrapped on a space keeps it",
                  joined.trim_end() == SENTENCE,
                  format!("41 columns to 80: {:?}", joined.trim_end())));
    }

    // The history is a ring and never grows
    //
    // It is fed from `scroll_up`, under the console lock from interrupt
    // handlers. Growth would make printing allocate, and printing is what the
    // allocator does when it fails
    {
        const W: usize = 370;
        const H: usize = 190;
        let (sizes_before, sizes_after, used, last_line) =
            with_scratch_console(W, H, W, 0, |c, _| {
                let sizes = |c: &Console| (c.grid.hist.len(), c.grid.hist.capacity(),
                                           c.grid.hist_len.len(), c.grid.hist_wrap.len(),
                                           c.grid.cells.len(), c.grid.cells.capacity(),
                                           c.grid.wrapped.len());
                let before = sizes(c);
                for i in 0..5000 {
                    let _ = write!(c, "row number {} of a great many\n", i);
                }
                // A re-flow after the ring has wrapped right round
                c.reflow(20, 10);
                c.reflow(40, 10);
                // The pen is on an empty line after the last newline, so the
                // newest text is the line before it
                let all = c.grid.logical_lines();
                let last = all.iter().rev().map(|l| l.iter().map(|x| x.glyph())
                        .collect::<alloc::string::String>())
                    .find(|l| !l.is_empty()).unwrap_or_default();
                (before, sizes(c), c.grid.hist_used, last)
            });

        let newest = "row number 4999 of a great many";
        out.push(("console: the line history is a ring and never grows",
                  sizes_before == sizes_after && used <= HIST_ROWS && last_line == newest,
                  format!("{} rows kept of {}, buffers unchanged {}, newest line {:?}",
                          used, HIST_ROWS, sizes_before == sizes_after, last_line)));
    }

    // Headless: the text carries on and the pixels stop
    //
    // The two halves are interleaved everywhere: `write_char` stores a cell and
    // paints a glyph in one call
    {
        const W: usize = 400;
        const H: usize = 200;
        let (touched, said, rows, back) = with_scratch_console(W, H, W, 0xAB, |c, mem| {
            c.set_headless(true);
            c.take_row_damage();
            // The baseline is taken here, not from the fill byte: building a
            // console clears the framebuffer it was handed, before there is a
            // console to make headless
            let before = mem.to_vec();
            for i in 0..12 {
                let _ = write!(c, "headless line {}\n", i);
            }
            // Not one byte of the framebuffer
            let touched = mem != &before[..];
            // But the text is all there, and it said which rows moved
            let rows = c.take_row_damage();
            let said = c.grid.logical_lines().iter().any(|l| {
                l.iter().map(|x| x.glyph()).collect::<alloc::string::String>()
                    == "headless line 11"
            });
            // And turning it off puts the text back on the screen
            c.set_headless(false);
            let back = mem != &before[..];
            (touched, said, rows, back)
        });

        out.push(("console: headless keeps the text and draws nothing",
                  !touched && said && rows.is_some() && back,
                  format!("framebuffer touched while headless {}, text kept {}, \
                           rows reported {:?}, painted again after {}",
                          touched, said, rows, back)));
    }

    out
}
