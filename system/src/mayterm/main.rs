#![no_std]
#![no_main]

// Terminal process
//
// The kernel keeps the text: pen, wrapping, scrolling, and reflow. This keeps
// the pixels. They share `may_console`: a handle to the character grid and
// which rows changed.
//
// `grid`: the character grid and its rendering
// `size`: the pool, its buffers, and resizing

pub use miku;

pub use libmay::wire;
pub use libmay::protocol;
pub use libmay::conn;
// Every request a client can make, shared with the kernel half
pub use libmay::requests;
// Console font and blitter, compiled from the kernel's files
#[allow(dead_code)]
pub use libmay::font;
#[allow(dead_code)]
pub use libmay::paint;
#[allow(dead_code)]
pub use libmay::text;

mod grid;
mod menu;
mod size;

use conn::{Conn, Event};
use requests::Requests;
use protocol::Interface;

fn fail(msg: &str) -> ! {
    miku::print("  mayterm: ");
    miku::println(msg);
    miku::exit(1);
}

/// Rows of glyphs one frame may draw. The remainder carries over
const ROWS_PER_FRAME: usize = 8;

/// Rows this frame may draw for its debt. A debt of most of the screen is paid
/// at once: each of the two buffers owes its rows separately, and showing
/// different halves alternately flickers. The frame continues until the slot
/// owes nothing; this sets only the first, declared run
fn row_budget(f: &size::Frame, slot: usize, rows: usize) -> usize {
    if rows == 0 {
        return ROWS_PER_FRAME;
    }
    if f.owed_rows(slot, rows) * 2 >= rows {
        rows
    } else {
        ROWS_PER_FRAME
    }
}

/// Initial window size: 80 by 25
fn wanted_window() -> (i32, i32) {
    (
        2 * grid::PAD + 80 * grid::CELL_W,
        2 * grid::PAD + 25 * grid::LINE_H,
    )
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    // `mayterm console` draws the machine's console instead of its own terminal
    let args = miku::args(argc, argv);
    let want_console = args.iter().any(|a| *a == "console");

    let mut c = match Conn::open() {
        Some(c) => c,
        None => fail("cannot reach the display server"),
    };

    // Handshake: lists globals
    let mut evs = [Event::Other { object: 0, opcode: 0 }; 32];
    let (registry, globals) = c.handshake();
    if globals.failed {
        fail("the server refused the handshake");
    }
    if !globals.has(Interface::Shm) || !globals.has(Interface::Compositor)
        || !globals.has(Interface::Xdg)
    {
        fail("the registry is missing a global we need");
    }
    if !globals.has(Interface::Console) {
        fail("this compositor has no may_console - nothing to draw");
    }

    let shm = c.bind(registry, globals.name(Interface::Shm), Interface::Shm, 1);
    let comp = c.bind(registry, globals.name(Interface::Compositor),
                      Interface::Compositor, 1);
    let xdg = c.bind(registry, globals.name(Interface::Xdg), Interface::Xdg, 1);
    let surface = c.create_surface(comp);
    let xdg_surface = c.get_xdg_surface(xdg, surface);
    let toplevel = c.get_toplevel(xdg_surface);
    c.set_title(toplevel, "console");
    // What a window rule matches on. `windowrule = float, class:mayterm`
    c.set_app_id(toplevel, "mayterm");
    c.flush();

    // Waits for the proposed size before drawing
    let (mut win_w, mut win_h) = wanted_window();
    let mut g = grid::Grid::new();
    let mut f = size::Frame::new();
    let mut configured = false;
    while !configured {
        let n = c.read_events(&mut evs);
        if n == 0 { fail("no configure arrived for the window"); }
        for e in &evs[..n] {
            match *e {
                Event::ToplevelConfigure { width, height, .. } if width > 0 && height > 0 => {
                    win_w = width;
                    win_h = height;
                }
                Event::XdgConfigure { serial, .. } => {
                    c.ack_configure(xdg_surface, serial);
                    configured = true;
                }
                Event::ConsoleGrid { handle, size, cols, rows } => {
                    g.adopt(handle, size, cols, rows);
                }
                Event::Error { msg, msg_len, .. } => {
                    report(msg, msg_len);
                    fail("the window role was refused");
                }
                _ => {}
            }
        }
    }

    if !f.make(&mut c, shm, win_w, win_h) {
        fail("the kernel would not give us a buffer");
    }
    f.mark_all();
    // Sends the pool and buffers now; the first frame draws into them
    c.flush();

    // This window's terminal, with a shell. `/bin/mayterm console` draws the
    // machine's console instead
    let (cols0, rows0) = (grid::cols_for(win_w), grid::rows_for(win_h));
    // Master held for the window's lifetime. Closing it hangs up the terminal
    // and sends the shell SIGHUP
    let mut master: i64 = -1;
    let tty = if want_console {
        miku::tty::SYSTEM
    } else {
        match miku::tty::open_pty() {
            Ok((fd, t)) => { master = fd; t }
            // No terminal available, so it falls back to the machine's console
            Err(_) => miku::tty::SYSTEM,
        }
    };
    // Actual window shape; a master opens at 80 by 24
    if master >= 0 {
        let ws = [rows0.max(1) as u16, cols0.max(1) as u16, 0u16, 0u16];
        const TIOCSWINSZ: u64 = 0x5414;
        unsafe { miku::miku_ioctl(master as u64, TIOCSWINSZ, ws.as_ptr() as u64) };
    }

    // Bound last, since taking the console stops the kernel's own console
    // window
    let console = c.bind(registry, globals.name(Interface::Console),
                         Interface::Console, 1);
    // Selects the terminal before other requests
    c.console_bind(console, tty);
    // Last reported shape, to skip repeated configures
    let mut told = (0usize, 0usize);
    tell_size(&mut c, console, win_w, win_h, &mut told);

    // Flushed before the fork, since `fork` copies `Conn`'s buffer into the
    // child
    c.flush();

    // The child attaches before exec, so `ctty` is set and inherited by
    // everything the shell starts
    if tty != miku::tty::SYSTEM {
        start_shell(tty);
    }

    // Pointer for selection and menu; keys go past this program to the shell
    let seat = if globals.has(Interface::Seat) {
        c.bind(registry, globals.name(Interface::Seat), Interface::Seat, 1)
    } else { 0 };
    let pointer = if seat != 0 { c.get_pointer(seat) } else { 0 };
    // Keyboard for its serials only; copies and menus need one. Keys go to the
    // shell through `input::intercept`
    let keyboard = if seat != 0 { c.get_keyboard(seat) } else { 0 };
    let clip = if globals.has(Interface::Clipboard) {
        c.bind(registry, globals.name(Interface::Clipboard), Interface::Clipboard, 1)
    } else { 0 };
    let _ = (pointer, keyboard);
    let mut m = menu::Menu::new();
    // Scrollback offset. The kernel clamps it, so it is only compared with
    // itself
    let mut view: i32 = 0;
    c.flush();

    // the loop
    let mut frame_cb: u32 = 0;
    // Set by the frame callback; true for the first frame
    let mut armed = true;
    let mut pending: Option<(i32, i32)> = None;
    let mut frames: u64 = 0;
    loop {
        // Frame gates: the compositor finished the last frame (`armed`), a
        // buffer is free (`free_slot`), and rows are owed
        let job = if armed && g.ready() && f.live() && pending.is_none() {
            f.free_slot()
                .and_then(|slot| f.next_run(slot, row_budget(&f, slot, g.rows), g.rows)
                    .map(|d| (slot, d)))
        } else {
            None
        };

        if job.is_none() {
            let n = c.read_events(&mut evs);
            // Ends only at end of stream, and still applies a resize otherwise
            if n == 0 && c.closed {
                miku::print("  mayterm: ");
                miku::println(c.death);
                break;
            }
            let mut died = false;
            for e in &evs[..n] {
                match *e {
                    Event::BufferRelease { id } => f.released(id),
                    Event::Done { id, .. } if id == frame_cb => armed = true,
                    // Grid location, changing on reallocation, so every row is
                    // new
                    Event::ConsoleGrid { handle, size, cols, rows } => {
                        if g.adopt(handle, size, cols, rows) {
                            let rows = g.rows;
                            f.mark(0, rows);
                            // A narrower grid leaves old text at the new margin
                            f.reshaped();
                        }
                    }
                    Event::ConsoleDamage { row0, row1, cursor_col, cursor_row } => {
                        // Both caret rows, so moving it restores the cell it
                        // covered
                        let was = g.caret;
                        g.caret = (cursor_col as usize, cursor_row as usize);
                        f.mark(was.1, was.1 + 1);
                        f.mark(g.caret.1, g.caret.1 + 1);
                        f.mark(row0 as usize, row1 as usize);
                    }
                    Event::ToplevelConfigure { width, height, .. } if width > 0 && height > 0 => {
                        if (width, height) != (f.width, f.height) {
                            pending = Some((width, height));
                        }
                    }
                    // Configure for the menu's popup surface, which has its own
                    // serial
                    Event::XdgConfigure { serial, .. } => {
                        if m.is_open() && m.awaiting_configure() {
                            m.configure(&mut c, serial);
                        } else {
                            c.ack_configure(xdg_surface, serial);
                        }
                    }
                    Event::PopupDone => {
                        // Dismissed by the compositor; only the objects remain
                        // to release
                        m.close(&mut c);
                    }
                    // Kept for its serial
                    Event::Key { serial, pressed: true, .. } => m.serial = serial,
                    Event::PointerEnter { surface, x, y } => {
                        m.entered(surface, x, y);
                        if m.pointer_in_menu() && m.hover(y) {
                            m.paint(&mut c);
                        }
                    }
                    Event::PointerLeave { surface } => m.left(surface),
                    Event::PointerMotion { x, y } => {
                        // Motion has no surface, so the last enter decides
                        if m.pointer_in_menu() {
                            if m.hover(y) { m.paint(&mut c); }
                        } else {
                            // Marks both the old and new selection rows, so a
                            // released row is repainted
                            let was = m.touched_rows(&g);
                            if m.drag_to(&g, x, y) {
                                let now = m.touched_rows(&g);
                                let (r0, r1) = if was.1 > was.0 {
                                    (was.0.min(now.0), was.1.max(now.1))
                                } else {
                                    now
                                };
                                f.mark(r0, r1);
                            }
                        }
                    }
                    Event::PointerButton { button, pressed, serial } => {
                        if pressed { m.serial = serial; }
                        if m.pointer_in_menu() {
                            // Inside the menu: the hovered item. The compositor
                            // swallows clicks outside
                            if !pressed {
                                if let Some(item) = m.hot_item() {
                                    match item {
                                        0 => { m.copy(&mut c, &g, clip); }
                                        _ => { m.paste(&mut c, clip); }
                                    }
                                }
                                m.close(&mut c);
                                let rows = g.rows;
                                f.mark(0, rows);
                            }
                        } else if menu::opens_menu(button) && pressed {
                            m.open(&mut c, shm, xdg, comp, xdg_surface, seat);
                        } else if button == menu::BTN_LEFT {
                            if pressed {
                                // Old selection rows and the new start row
                                let was = m.touched_rows(&g);
                                m.begin_drag(&g, m.at.0, m.at.1);
                                let now = m.touched_rows(&g);
                                let (r0, r1) = if was.1 > was.0 {
                                    (was.0.min(now.0), was.1.max(now.1))
                                } else {
                                    now
                                };
                                f.mark(r0, r1);
                            } else {
                                let (r0, r1) = m.touched_rows(&g);
                                m.end_drag(&g);
                                // Rows repaint whether the selection stayed or
                                // went
                                f.mark(r0, r1);
                            }
                        }
                    }
                    // Selection received; passed to the console as input
                    Event::ClipboardData { handle, size, .. } =>
                        menu::deliver(&mut c, console, handle, size),
                    Event::Close { .. } => died = true,
                    // The terminal is gone, so the window closes.
                    // Wheel: scrollback through `may_console.SET_VIEW`. The
                    // kernel clamps and reports through damage
                    Event::PointerAxis { value, .. } => {
                        // 15 units per notch; 3 rows per notch
                        let notches = value / 15;
                        if notches != 0 {
                            let step = notches * 3;
                            view = (view - step).max(0);
                            let _ = c.console_set_view(console, view as u32);
                            // Every row is different text
                            f.mark_all();
                            // Selection is in screen coordinates, so it no
                            // longer applies
                            m.span = None;
                        }
                    }
                    Event::ConsoleGone => died = true,
                    Event::Error { msg, msg_len, .. } => {
                        report(msg, msg_len);
                        died = true;
                    }
                    _ => {}
                }
            }
            if died { break; }

            // Reaps the shell without blocking
            let _ = miku::proc::try_wait_any();

            // Resizes between frames, once `armed` shows the compositor is not
            // reading. Runs even if the read found nothing, so a configure
            // during a frame does not wait for a socket timeout
            if let Some((w, h)) = pending {
                if armed {
                    pending = None;
                    f.teardown(&mut c);
                    if !f.make(&mut c, shm, w, h) {
                        fail("the kernel would not give us a buffer after a resize");
                    }
                    f.mark_all();
                    tell_size(&mut c, console, w, h, &mut told);
                    c.flush();
                }
            }
            continue;
        }

        // All three gates held; `next_run` applied the budget
        let Some((slot, (r0, r1))) = job else { continue };

        let stride_px = f.stride_px();
        let (caret_was, caret_now) = f.caret_rows(slot, g.caret.1);
        // Paints the margin when owed
        let whole = f.take_surround(slot);
        let (fw, fh) = (f.width, f.height);
        let sel = m.span;
        {
            let px = f.pixels(slot);
            if whole {
                g.draw_surround(px, stride_px, fw, fh);
            }
            g.draw_rows(px, stride_px, r0, r1, sel);
            // Caret rows first, outside the budget, so typing appears
            // immediately during heavy output
            for row in [caret_was, caret_now] {
                if row < g.rows && !(r0..r1).contains(&row) {
                    g.draw_rows(px, stride_px, row, row + 1, sel);
                }
            }
            g.draw_caret(px, stride_px);
        }

        // Draws everything this buffer still owes before attach, since a
        // committed buffer must be whole: the compositor re-reads it beyond
        // declared damage when repairing its older surface (`OWED`)
        let mut more: Option<(usize, usize)> = None;
        {
            // Clears the run just drawn so the sweep does not repeat it
            let mut r = r0;
            while r < r1 {
                f.drew_row(slot, r);
                r += 1;
            }
        }
        let mut guard = 0;
        while guard < g.rows {
            let Some((a, b)) = f.next_run(slot, g.rows, g.rows) else { break };
            {
                let px = f.pixels(slot);
                g.draw_rows(px, stride_px, a, b, sel);
            }
            let mut r = a;
            while r < b {
                f.drew_row(slot, r);
                r += 1;
            }
            // One rectangle over all of them, since damage may be wider than
            // the truth
            more = Some(match more {
                Some((x, y)) => (x.min(a), y.max(b)),
                None => (a, b),
            });
            guard += 1;
        }

        let y0 = grid::PAD + r0 as i32 * grid::LINE_H;
        let y1 = grid::PAD + r1 as i32 * grid::LINE_H;
        c.attach(surface, f.id(slot), 0, 0);
        c.damage(surface, 0, y0, f.width, y1 - y0);
        if let Some((a, b)) = more {
            let ay = grid::PAD + a as i32 * grid::LINE_H;
            let by = grid::PAD + b as i32 * grid::LINE_H;
            c.damage(surface, 0, ay, f.width, by - ay);
        }
        // Declares painted rows, or the compositor never copies them
        for row in [caret_was, caret_now] {
            if row < g.rows && !(r0..r1).contains(&row) {
                let y = grid::PAD + row as i32 * grid::LINE_H;
                c.damage(surface, 0, y, f.width, grid::LINE_H);
            }
        }
        if whole {
            // Bands lie outside every row, so they get their own four
            // rectangles
            for (x, y, w, h) in g.surround_rects(fw, fh) {
                c.damage(surface, x, y, w, h);
            }
        }
        frame_cb = c.frame(surface);
        c.commit(surface);
        c.flush();
        f.committed(slot, r0, r1, g.rows);
        // Caret rows are paid outside the budget (`Frame::drew_row`)
        for row in [caret_was, caret_now] {
            if row < g.rows && !(r0..r1).contains(&row) {
                f.drew_row(slot, row);
            }
        }
        armed = false;
        frames += 1;
    }

    // Returns the console, so the kernel's window comes back on the next frame
    c.console_detach(console);
    f.teardown(&mut c);
    c.destroy_surface(surface);
    c.flush();
    c.close();

    miku::print("  mayterm: ");
    miku::print_int(frames as i64);
    miku::println(" frames drawn");
    miku::exit(0);
}

/// Forks, joins the terminal, and execs a shell. `tty::attach` in the child
/// sets the controlling terminal, inherited across `exec`. Only the master and
/// its descendants may join
fn start_shell(tty: u32) {
    let pid = unsafe { miku::miku_fork() };
    if pid < 0 {
        miku::eprintln("mayterm: cannot fork a shell");
        return;
    }
    if pid != 0 {
        return;
    }
    if miku::tty::attach(tty).is_err() {
        miku::eprintln("mayterm: the terminal would not take the shell");
        miku::exit(1);
    }
    let path = b"/bin/msh\0";
    let argv = [path.as_ptr(), core::ptr::null()];
    // With the session's environment, from `mikudod`
    unsafe {
        let envp = miku::miku_environ() as *const *const u8;
        miku::miku_execve(path.as_ptr(), argv.as_ptr(), 1, envp, miku::miku_env_count())
    };
    // Reached only if exec failed
    miku::eprintln("mayterm: no /bin/msh");
    miku::exit(127);
}

/// Tells the kernel the size in characters only when it changes, since dragging
/// changes pixels far more often
fn tell_size(c: &mut Conn, console: u32, width: i32, height: i32, last: &mut (usize, usize)) {
    let (cols, rows) = (grid::cols_for(width), grid::rows_for(height));
    if cols == 0 || rows == 0 {
        // Too small for a character; the grid stays
        return;
    }
    if (cols, rows) == *last {
        return;
    }
    *last = (cols, rows);
    c.console_set_size(console, cols as u32, rows as u32);
}

fn report(msg: [u8; 64], msg_len: usize) {
    miku::print("  mayterm: protocol error: ");
    match core::str::from_utf8(&msg[..msg_len]) {
        Ok(s) => miku::println(s),
        Err(_) => miku::println("(unprintable)"),
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    miku::println("  mayterm: panic");
    miku::exit(101);
}
