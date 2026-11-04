#![no_std]
#![no_main]

// Mayland client process: a bouncing square, with two buffers, drawing only
// into one the compositor has released

pub use miku;

pub use libmay::conn;
pub use libmay::protocol;
pub use libmay::wire;
// Every request a client can make, shared with the kernel half
pub use libmay::requests;
// Console font, `draw_text`, and the keycode layout, compiled from the kernel's
// files
pub use libmay::font;
pub use libmay::keymap;
pub use libmay::paint;
pub use libmay::text;

use conn::{Conn, Event, Pool};
use protocol::Interface;
use requests::Requests;

/// Wide enough for a line of text at nine pixels a character
const W: i32 = 320;
const H: i32 = 140;
const STRIDE: i32 = W * 4;
const BUF_BYTES: usize = (STRIDE * H) as usize;
const BUFFERS: usize = 2;

/// Ten seconds at 60 Hz
const FRAMES: u64 = 600;

fn fail(msg: &str) -> ! {
    miku::print("  maydemo: ");
    miku::println(msg);
    miku::exit(1);
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("mayland client");

    let mut c = match Conn::open() {
        Some(c) => c,
        None => fail("cannot reach the display server"),
    };

    // Handshake: lists globals
    let mut evs = [Event::Other {
        object: 0,
        opcode: 0,
    }; 32];
    let (mut screen_w, mut screen_h) = (0i32, 0i32);

    let (registry, globals) = c.handshake();
    if globals.failed {
        fail("the server refused the handshake");
    }
    if !globals.has(Interface::Shm)
        || !globals.has(Interface::Compositor)
        || !globals.has(Interface::Xdg)
    {
        fail("the registry is missing a global we need");
    }

    let shm = c.bind(registry, globals.name(Interface::Shm), Interface::Shm, 1);
    let comp = c.bind(
        registry,
        globals.name(Interface::Compositor),
        Interface::Compositor,
        1,
    );
    let xdg = c.bind(registry, globals.name(Interface::Xdg), Interface::Xdg, 1);
    if globals.has(Interface::Seat) {
        let seat = c.bind(registry, globals.name(Interface::Seat), Interface::Seat, 1);
        c.get_keyboard(seat);
    }

    // Binds may_output for the mode
    if globals.has(Interface::Output) {
        c.bind(
            registry,
            globals.name(Interface::Output),
            Interface::Output,
            1,
        );
    }
    c.sync();
    c.flush();

    // Layout from the compositor, held for the program's lifetime
    let mut layout: Option<Pool> = None;
    let mut settled = false;
    while !settled {
        let n = c.read_events(&mut evs);
        if n == 0 {
            fail("the server closed the connection");
        }
        for e in &evs[..n] {
            match *e {
                Event::OutputMode { width, height, .. } => {
                    screen_w = width;
                    screen_h = height;
                }
                Event::Done { .. } => settled = true,
                // Arrives during this handshake, when the keyboard is bound
                Event::Keymap {
                    format,
                    handle,
                    size,
                } => layout = adopt_layout(format, handle, size),
                Event::Error { .. } => fail("bind refused"),
                _ => {}
            }
        }
    }
    if screen_w <= 0 || screen_h <= 0 {
        // No display: continues as if 640x480
        screen_w = 640;
        screen_h = 480;
        miku::println("  no screen geometry - the compositor is not running");
    }

    // our pixels
    let mut pool = match Pool::new(BUF_BYTES * BUFFERS) {
        Some(p) => p,
        None => fail("the kernel would not give us a pool"),
    };
    let pool_obj = c.create_pool(shm, pool.handle, (BUF_BYTES * BUFFERS) as i32);
    let mut buffers = [(0u32, true); BUFFERS];
    let mut i = 0;
    while i < BUFFERS {
        buffers[i].0 = c.create_buffer(
            pool_obj,
            (i * BUF_BYTES) as i32,
            W,
            H,
            STRIDE,
            protocol::shm::FORMAT_XRGB8888,
        );
        i += 1;
    }
    let surface = c.create_surface(comp);

    // Window role; the compositor does not show a surface without one
    let xdg_surface = c.get_xdg_surface(xdg, surface);
    let toplevel = c.get_toplevel(xdg_surface);
    c.set_title(toplevel, "maydemo");
    c.flush();

    // Acknowledges the proposed size before drawing; the server refuses earlier
    // buffers
    let mut configured = false;
    while !configured {
        let n = c.read_events(&mut evs);
        if n == 0 {
            fail("no configure arrived for the window");
        }
        for e in &evs[..n] {
            match *e {
                Event::XdgConfigure { serial, .. } => {
                    c.ack_configure(xdg_surface, serial);
                    configured = true;
                }
                Event::Error { .. } => fail("the window role was refused"),
                _ => {}
            }
        }
    }
    c.flush();

    miku::print("  connected, pool handle ");
    miku::print_int(pool.handle as i64);
    miku::print(" mapped at ");
    miku::print_hex(pool.addr as u64);
    miku::println("");

    // the loop
    let (mut x, mut y) = ((screen_w - W) / 2, (screen_h - H) / 2);
    let (mut dx, mut dy) = (7i32, 5i32);
    let mut tint: u32 = 0;
    let mut frames: u64 = 0;
    let mut starved: u64 = 0;
    let mut keys: u64 = 0;
    let mut frame_cb: u32 = 0;
    // Typed text and modifiers, in a fixed line
    let mut typed = [0u8; TYPED_MAX];
    let mut typed_len = 0usize;
    let mut mods: u32 = 0;
    // The keyboard layout in use, from the same event as the mask
    let mut group: u32 = 0;
    let mut armed = true; // the first paint waits for nothing
                          // Active state, reported only in configure
    let mut active = true;

    while frames < FRAMES {
        if !armed {
            let n = c.read_events(&mut evs);
            if n == 0 {
                if c.closed {
                    miku::print("  ");
                    miku::println(c.death);
                } else {
                    miku::println("  no frame callback arrived - nothing is compositing");
                }
                break;
            }
            let mut died = false;
            for e in &evs[..n] {
                match *e {
                    Event::BufferRelease { id } => {
                        let mut k = 0;
                        while k < BUFFERS {
                            if buffers[k].0 == id {
                                buffers[k].1 = true;
                            }
                            k += 1;
                        }
                    }
                    Event::Done { id, .. } if id == frame_cb => armed = true,
                    // Key reached this window: the square reverses.
                    // Modifiers arrive before the key they apply to
                    Event::Modifiers { mask, group: g } => {
                        mods = mask;
                        group = g;
                    }
                    // Layout in a pool, held for the program's lifetime
                    Event::Keymap {
                        format,
                        handle,
                        size,
                    } => layout = adopt_layout(format, handle, size),
                    Event::Key {
                        keycode,
                        pressed: true,
                        ..
                    } => {
                        dx = -dx;
                        keys += 1;
                        // The compositor's table, else the compiled-in layout
                        let ch = match &layout {
                            Some(p) => keymap::Table::new(p.as_slice())
                                .and_then(|t| t.to_char_in(keycode as u16, mods, group)),
                            None => keymap::to_char(keycode as u16, mods),
                        };
                        if let Some(ch) = ch {
                            type_char(&mut typed, &mut typed_len, ch);
                        }
                    }
                    Event::ToplevelConfigure { activated, .. } => {
                        active = activated;
                    }
                    Event::XdgConfigure { serial, .. } => {
                        c.ack_configure(xdg_surface, serial);
                    }
                    Event::Close { .. } => {
                        miku::println("  the compositor asked us to close");
                        died = true;
                    }
                    Event::Error { msg, msg_len, .. } => {
                        miku::print("  protocol error: ");
                        if let Ok(s) = core::str::from_utf8(&msg[..msg_len]) {
                            miku::println(s);
                        } else {
                            miku::println("(unprintable)");
                        }
                        died = true;
                    }
                    _ => {}
                }
            }
            if died {
                break;
            }
            if !armed {
                continue;
            }
        }

        // Picks a released buffer
        let mut slot = usize::MAX;
        let mut k = 0;
        while k < BUFFERS {
            if buffers[k].1 {
                slot = k;
                break;
            }
            k += 1;
        }
        if slot == usize::MAX {
            // Drawing faster than the screen refreshes. Waiting is correct
            starved += 1;
            armed = false;
            continue;
        }

        paint(pool.bytes(), slot * BUF_BYTES, tint, active);
        label(pool.bytes(), slot * BUF_BYTES, &typed[..typed_len], active);
        tint = tint.wrapping_add(3);

        x += dx;
        y += dy;
        if x <= 0 {
            x = 0;
            dx = -dx;
        }
        if y <= 0 {
            y = 0;
            dy = -dy;
        }
        if x + W >= screen_w {
            x = screen_w - W;
            dx = -dx;
        }
        if y + H >= screen_h {
            y = screen_h - H;
            dy = -dy;
        }

        c.attach(surface, buffers[slot].0, x, y);
        c.damage(surface, 0, 0, W, H);
        frame_cb = c.frame(surface);
        c.commit(surface);
        c.flush();
        buffers[slot].1 = false;
        armed = false;
        frames += 1;
    }

    c.destroy_surface(surface);
    c.flush();
    pool.free();
    c.close();

    miku::print("  ");
    miku::print_int(frames as i64);
    miku::print(" frames drawn, ");
    miku::print_int(starved as i64);
    miku::print(" waited for a buffer, ");
    miku::print_int(keys as i64);
    miku::println(" keys received");
    miku::exit(0);
}

/// Maps the compositor's keymap pool. An unknown format falls back to
/// compiled-in tables
fn adopt_layout(format: u32, handle: u32, size: u32) -> Option<Pool> {
    if format != protocol::keyboard::KEYMAP_MAY_V2 {
        return None;
    }
    let pool = Pool::adopt(handle, size as usize);
    miku::print("  layout from the compositor: ");
    miku::print_int(size as i64);
    miku::println(if pool.is_some() {
        " bytes, mapped"
    } else {
        " bytes, could not be mapped"
    });
    pool
}

/// As many characters as fit on one line of the window
const TYPED_MAX: usize = 28;

/// Adds a keystroke. Backspace deletes; a full line scrolls
fn type_char(buf: &mut [u8; TYPED_MAX], len: &mut usize, c: char) {
    match c {
        '\x08' => {
            if *len > 0 {
                *len -= 1;
            }
        }
        c if (c as u32) < 0x20 => {}
        c => {
            if *len == TYPED_MAX {
                buf.copy_within(1.., 0);
                *len -= 1;
            }
            // One byte per cell; non-ASCII shows as a question mark
            buf[*len] = if c.is_ascii() { c as u8 } else { b'?' };
            *len += 1;
        }
    }
}

/// Draws the window's name and typed text with the console's font
fn label(px: &mut [u8], offset: usize, typed: &[u8], active: bool) {
    let buf = &mut px[offset..offset + BUF_BYTES];
    let ink = if active { 0x00ff_ffff } else { 0x00b0_b0b8 };
    text::draw_text(buf, W as usize, 10, 12, "maydemo", ink, None);
    let line = match core::str::from_utf8(typed) {
        Ok(s) => s,
        Err(_) => "",
    };
    let pen = text::draw_text(buf, W as usize, 10, 40, line, 0x0000_0000, None);
    if active {
        text::fill(
            buf,
            W as usize,
            pen + 1,
            40,
            2,
            text::CELL_H as i32,
            0x0000_0000,
        );
    }
}

/// Body color shifts every frame, with a white border only while active
fn paint(px: &mut [u8], offset: usize, tint: u32, active: bool) {
    let edge: u32 = if active { 0x00ff_ffff } else { 0x0050_5058 };
    let mut row = 0usize;
    while row < H as usize {
        let base = offset + row * STRIDE as usize;
        let mut col = 0usize;
        while col < W as usize {
            let border = row < 4 || col < 4 || row >= H as usize - 4 || col >= W as usize - 4;
            let colour: u32 = if border {
                edge
            } else {
                let r = ((tint >> 1) & 0x7f) + 0x40;
                (r << 16) | (0x60 << 8) | 0xc0
            };
            let p = base + col * 4;
            px[p..p + 4].copy_from_slice(&colour.to_le_bytes());
            col += 1;
        }
        row += 1;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::println("maydemo: panic!");
    miku::exit(1);
}
