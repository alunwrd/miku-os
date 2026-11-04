// 'mayland' shell command: protocol status and selftest

extern crate alloc;
use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::scanout;
use crate::mayland::input::{self, pointer};
use crate::mayland::render as compositor;
use crate::mayland::server::{self, shm};
use crate::mayland::shell::{demo, layout, wallpaper, workspace};
use crate::mayland::selftest;
use crate::{cprintln, print, print_error, println};

pub fn cmd_mayland(rest: &str) {
    let mut parts = rest.split_whitespace();
    match parts.next().unwrap_or("status") {
        "status" | "" => status(),
        "test"        => test(),
        "shm"         => pools(),
        "frame"       => frame(parts.next()),
        "start"       => start(),
        "stop"        => stop(),
        "demo"        => demo_cmd(parts.next()),
        "input"       => input_cmd(parts.next()),
        "pointer"     => pointer_cmd(parts.next(), parts.next()),
        "focus"       => focus_cmd(parts.next()),
        "effects"     => effects_cmd(parts.next(), parts.next()),
        "move"        => move_cmd(parts.next(), parts.next(), parts.next()),
        "layout"      => layout_cmd(parts.next()),
        "wm"          => wm_cmd(parts.next()),
        "term"        => term_cmd(parts.next()),
        "shell"       => shell_cmd(parts.next()),
        // The whole rest of the line: `launcher toggle` is one word to the
        // shell, a call (`aosh`'s IpcHandler)
        "signal"      => signal_cmd(Some(&parts.collect::<Vec<_>>().join(" "))),
        "workspace"   => workspace_cmd(parts.next()),
        other => {
            print_error!("  mayland: unknown subcommand '{}'", other);
            usage();
        }
    }
}

fn usage() {
    println!("Usage: mayland [status | start | stop | demo [on|off]");
    println!("                | term | shell [off] | signal <word> | wm [off]");
    println!("                | workspace <1-9>");
    println!("                | layout [master|dwindle|float] | move <c:s> <x> <y>");
    println!("                | effects [on|off|alpha|corner|shadow|margin|radius] <n>");
    println!("                | focus [next|<client>:<surface>] | input [on|off]");
    println!("                | test | shm | frame [ms] | pointer <x> <y>]");
    println!("  status  globals, connections, surfaces in stacking order, stats");
    println!("  start   start compositing at the raster's pace");
    println!("  stop    stop compositing (the screen keeps its last frame)");
    println!("  demo    connect one more in-kernel client drawing a moving square");
    println!("  focus   give the keyboard to the next window, or to one by id");
    println!("  move    put a window at x,y - the compositor owns where windows are");
    println!("  layout  master, dwindle or float - the compositor owns how big they are");
    println!("  effects blur, rounded corners and shadows, and the numbers they use");
    println!("  shell   start /bin/aosh - the bar and the launcher, in QML");
    println!("  signal  say a word to the shell: 'mayland signal launcher toggle'");
    println!("  term    start /bin/mayterm, which draws the console from ring 3");
    println!("          (the kernel keeps the text; the terminal keeps the pixels)");
    println!("  wm      start /bin/maywm, the window manager, which holds may_wm");
    println!("          (it reads /etc/mayland/may.conf, and has built-in keys without one)");
    println!("  workspace <1-9> - which windows are on the screen");
    println!("  input   give the keyboard to the focused window (Ctrl+Alt+Esc takes it back)");
    println!("  test    run the protocol selftest (no hardware touched)");
    println!("  shm     shared-memory pools");
    println!("  frame   fire frame callbacks as if a frame had been presented");
    println!("  pointer put the cursor at x,y as if the mouse had moved there");
}

fn start() {
    match compositor::start() {
        Ok(()) => cprintln!(100, 220, 150, "  compositing"),
        Err(e) => print_error!("  mayland: {}", e),
    }
}

fn stop() {
    compositor::stop_by_operator();
    println!("  compositor stopped");
}

fn demo_cmd(arg: Option<&str>) {
    match arg.unwrap_or("on") {
        "off" => {
            demo::stop();
            println!("  demo client disconnected");
        }
        _ => {
            // The demo is a client: it runs whether or not anything is drawing
            // it. With no scanout it still exercises the protocol, driven by
            // 'mayland frame'
            if !compositor::running() {
                match compositor::start() {
                    Ok(()) => println!("  compositor started"),
                    Err(e) => println!(
                        "  no compositor ({}) - drive frames with 'mayland frame'", e),
                }
            }
            match demo::start() {
                Ok(name) => cprintln!(100, 220, 150,
                    "  demo {} connected - {} windows, the newest has the keyboard",
                    name, demo::count()),
                Err(e) => print_error!("  mayland: {}", e),
            }
        }
    }
}

/// Starts the window manager, which is a program
///
/// In the background: it holds `may_wm` for as long as it runs, and a shell
/// waiting on it could not be typed at
fn wm_cmd(arg: Option<&str>) {
    const PATH: &str = "/bin/maywm";
    if arg == Some("off") {
        // An ordinary process, stopped the ordinary way. The shell does not
        // kill by name
        println!("  the window manager is a process - 'ps' to find it, 'kill' to stop it");
        return;
    }
    if crate::fs::vfs::core::with_vfs(|v| v.resolve_path(0, PATH).is_err()) {
        print_error!("  mayland: {} not present on the root filesystem", PATH);
        println!("  (mount the disk carrying /bin first, e.g. 'ext2mount <drive>')");
        return;
    }
    match crate::process::exec::exec(PATH, &[PATH], crate::process::exec::DEFAULT_ENV) {
        Ok(pid) => {
            // Exec does not take the console, and these three do not want it:
            // they draw through the compositor and the shell stays usable
            // underneath
            crate::io::input::user_stdin::clear_foreground();
            cprintln!(100, 220, 150,
                "  window manager started, pid {} - Ctrl+1..9, Super+J/K, Super+Q, Super+M",
                pid);
        }
        Err(e) => print_error!("  mayland: {}", e.as_str()),
    }
}

/// Starts the terminal, which draws the console from ring 3
///
/// In the background, as for the window manager: it holds `may_console` for as
/// long as it runs.
///
/// This is what shows what the kernel says on a running desktop. A desktop with
/// no terminal open talks to the serial port only, as `mayland status` prints
fn term_cmd(arg: Option<&str>) {
    const PATH: &str = "/bin/mayterm";
    if arg == Some("off") {
        println!("  the terminal is a process - 'ps' to find it, 'kill' to stop it");
        println!("  (with none running, the kernel's text goes to the serial port only)");
        return;
    }
    if !compositor::running() {
        print_error!("  mayland: nothing is compositing - 'mayland start' first");
        return;
    }
    if crate::fs::vfs::core::with_vfs(|v| v.resolve_path(0, PATH).is_err()) {
        print_error!("  mayland: {} not present on the root filesystem", PATH);
        println!("  (mount the disk carrying /bin first, e.g. 'ext2mount <drive>')");
        return;
    }
    match crate::process::exec::exec(PATH, &[PATH], crate::process::exec::DEFAULT_ENV) {
        Ok(pid) => {
            // Exec does not take the console, and these three do not want it:
            // they draw through the compositor and the shell stays usable
            // underneath
            crate::io::input::user_stdin::clear_foreground();
            cprintln!(100, 220, 150, "  terminal started, pid {}", pid);
        }
        Err(e) => print_error!("  mayland: {}", e.as_str()),
    }
}

/// Starts the shell: the bar, the launcher and whatever else is written down
///
/// One program for every screen. `/etc/aosh/shell.qml` says what they are
/// (`dev/aosh-architecture.md`), and the bar reserves an edge. Not started by
/// `mayland start`: it lives on the root filesystem, and a machine with no disk
/// has to come up without it.
///
/// The launcher inside it is opened by a word
/// (`bind = $mod, D, signal, launcher toggle`). Where Super never reaches the
/// guest (under QEMU), `mayland signal launcher toggle` says the same word
fn shell_cmd(arg: Option<&str>) {
    const PATH: &str = "/bin/aosh";
    if arg == Some("off") {
        println!("  the shell is a process - 'ps' to find it, 'kill' to stop it");
        println!("  (the edge the bar reserved goes back to the layout when it exits)");
        return;
    }
    if !compositor::running() {
        print_error!("  mayland: nothing is compositing - 'mayland start' first");
        return;
    }
    if crate::fs::vfs::core::with_vfs(|v| v.resolve_path(0, PATH).is_err()) {
        print_error!("  mayland: {} not present on the root filesystem", PATH);
        println!("  (mount the disk carrying /bin first, e.g. 'ext2mount <drive>')");
        return;
    }
    // With the environment a program run from this console gets, so `HOME`
    // is whoever is at it and `~/.config/aosh` is theirs
    let env = super::user_env(crate::sched::current_cred().uid);
    let env: Vec<&str> = env.iter().map(|s| s.as_str()).collect();
    match crate::process::exec::exec(PATH, &[PATH], &env) {
        Ok(pid) => {
            // Exec does not take the console, and these three do not want it:
            // they draw through the compositor and the shell stays usable
            // underneath
            crate::io::input::user_stdin::clear_foreground();
            cprintln!(100, 220, 150, "  shell started, pid {}", pid);
        }
        Err(e) => print_error!("  mayland: {}", e.as_str()),
    }
}

/// Says a word to whatever is listening on may_desktop
///
/// What opens the launcher: the window manager says `launcher toggle` on
/// `$mod D`, and this says the same word from the shell
fn signal_cmd(arg: Option<&str>) {
    let Some(word) = arg.map(|a| a.trim()).filter(|a| !a.is_empty()) else {
        println!("  mayland signal <words> - 'launcher toggle' is the call the shipped files answer to");
        return;
    };
    if !compositor::running() {
        print_error!("  mayland: nothing is compositing - 'mayland start' first");
        return;
    }
    match crate::mayland::server::iface::desktop::signal(word) {
        0 => println!("  nobody is listening - is /bin/aosh running?"),
        n => cprintln!(100, 220, 150, "  said '{}' to {} client(s)", word, n),
    }
}

/// Look at a workspace from the shell, for when there is no window manager
fn workspace_cmd(arg: Option<&str>) {
    match arg.and_then(|a| a.parse::<u32>().ok()) {
        Some(n) => {
            workspace::set_workspace(n);
            println!("  workspace {}", server::workspace());
        }
        None => println!("  workspace {} - 'mayland workspace <1-9>' to switch",
                         server::workspace()),
    }
}

/// Turns the effects pass on, off, or adjusts one of its numbers
///
/// Every window except the bottom of the stack gets blur, alpha, rounded
/// corners and a shadow
fn effects_cmd(arg: Option<&str>, value: Option<&str>) {
    let n = value.and_then(|v| v.parse::<u32>().ok());
    match arg.unwrap_or("on") {
        "off" => {
            compositor::EFFECTS.lock().on = false;
            compositor::force_full_frame();
            println!("  windows are copied, opaque and square");
        }
        "alpha" | "corner" | "shadow" | "margin" | "radius" => {
            let Some(n) = n else {
                print_error!("  mayland: effects {} wants a number", arg.unwrap_or(""));
                return;
            };
            {
                let mut e = compositor::EFFECTS.lock();
                match arg.unwrap_or("") {
                    // Percent for the two that are fractions, pixels for the
                    // rest
                    "alpha" => e.alpha = (n.min(100) as f32) / 100.0,
                    "shadow" => e.shadow = (n.min(100) as f32) / 100.0,
                    "corner" => e.corner = n as f32,
                    "margin" => e.margin = n,
                    _ => e.radius = n.max(1) as f32,
                }
            }
            compositor::force_full_frame();
            let e = compositor::effects();
            println!("  alpha {}%, corner {} px, shadow {}%, margin {} px, radius {}",
                     (e.alpha * 100.0) as u32, e.corner as u32,
                     (e.shadow * 100.0) as u32, e.margin, e.radius as u32);
        }
        _ => {
            // The processor composes the alpha and the rounded corners
            // (`render/compose.rs`), and only the blur needs the card. What is
            // said is what will happen
            let blurs = crate::drivers::gpu::nvidia::blur::ready();
            compositor::EFFECTS.lock().on = true;
            compositor::force_full_frame();
            if blurs {
                cprintln!(100, 220, 150,
                    "  every window above the backdrop is blurred, blended and rounded");
            } else {
                cprintln!(100, 220, 150,
                    "  every window above the backdrop is blended and rounded -                      no blur, which wants the card's compute pass");
            }
        }
    }
}

/// Moves the keyboard between windows from the shell, so focus can be exercised
/// without the grab or a keyboard chord
fn focus_cmd(arg: Option<&str>) {
    match arg.unwrap_or("next") {
        "next" => match server::focus_next() {
            Some((c, s)) => cprintln!(100, 220, 150,
                "  the keyboard is with client {} surface {}", c, s),
            None => print_error!("  mayland: there are no windows"),
        },
        // 'client:surface' as status prints them, or a bare surface id when
        // there is only one client to be confused about
        arg => {
            let (client, surface) = match arg.split_once(':') {
                Some((c, s)) => (c.parse::<u32>().ok(), s.parse::<u32>().ok()),
                None => (None, arg.parse::<u32>().ok()),
            };
            match surface.filter(|s| server::focus_window(client, *s)) {
                Some(s) => cprintln!(100, 220, 150,
                    "  the keyboard is with surface {}", s),
                None => print_error!("  mayland: no window '{}' - try 'client:surface'", arg),
            }
        }
    }
}

/// Tiled or floating
///
/// Under `master` the compositor decides the size as well as the place and
/// tells each window through configure. Under `float` a window keeps the size
/// it drew and is only placed out of the way
fn layout_cmd(arg: Option<&str>) {
    match arg.unwrap_or("") {
        "master" | "tile" | "tiled" => {
            layout::set_kind(layout::Kind::Master);
            cprintln!(100, 220, 150,
                "  one window on the left, the rest down the right");
        }
        "dwindle" => {
            layout::set_kind(layout::Kind::Dwindle);
            cprintln!(100, 220, 150,
                "  every window splits the one it opened on, along its longer side");
        }
        "float" | "floating" => {
            layout::set_kind(layout::Kind::Float);
            cprintln!(100, 220, 150,
                "  windows keep the size they drew and are placed out of the way");
        }
        _ => match layout::kind() {
            layout::Kind::Master => println!("  layout: master - try 'dwindle' or 'float'"),
            layout::Kind::Dwindle => println!("  layout: dwindle - try 'master' or 'float'"),
            layout::Kind::Float => println!("  layout: float - try 'master' or 'dwindle'"),
        },
    }
}

/// Puts a window somewhere
///
/// A client has no say in where it is, and the window manager reaches the same
/// code through `may_wm.move`. This is the way in with no window manager
/// running
fn move_cmd(what: Option<&str>, x: Option<&str>, y: Option<&str>) {
    let (Some(what), Some(x), Some(y)) = (what, x, y) else {
        print_error!("  mayland: move <client:surface> <x> <y>");
        return;
    };
    let (Some(x), Some(y)) = (x.parse::<i32>().ok(), y.parse::<i32>().ok()) else {
        print_error!("  mayland: x and y are whole numbers of pixels");
        return;
    };

    // 'client:surface' as status prints them, or a bare surface id resolved
    // against the stack when it is unambiguous
    let target = match what.split_once(':') {
        Some((c, s)) => match (c.parse::<u32>().ok(), s.parse::<u32>().ok()) {
            (Some(c), Some(s)) => Some((c, s)),
            _ => None,
        },
        None => what.parse::<u32>().ok().and_then(|sid| {
            compositor::stack_dump().into_iter()
                .find(|i| i.surface == sid)
                .map(|i| (i.client, i.surface))
        }),
    };

    match target.filter(|(c, s)| layout::set_window_pos(*c, *s, x, y)) {
        Some((c, s)) => cprintln!(100, 220, 150,
            "  client {} surface {} is at {},{}", c, s, x, y),
        None => print_error!("  mayland: no window '{}' - try 'client:surface'", what),
    }
}

/// Moves the pointer from the shell
///
/// With no compositor running there is no pointer loop, and this makes a client
/// see an enter and answer it
fn pointer_cmd(x: Option<&str>, y: Option<&str>) {
    let (Some(x), Some(y)) = (x.and_then(|v| v.parse::<i32>().ok()),
                              y.and_then(|v| v.parse::<i32>().ok()))
    else {
        print_error!("  usage: mayland pointer <x> <y>");
        return;
    };
    let ms = crate::fs::procfs::uptime_ticks() as u32 * 4;
    pointer::place(x, y);
    server::pointer_motion(x, y, ms);
    match server::pointer_focus() {
        Some((c, s)) => cprintln!(100, 220, 150,
            "  the pointer is at {},{} over client {} surface {}", x, y, c, s),
        None => println!("  the pointer is at {},{} over nothing", x, y),
    }
}

fn status() {
    cprintln!(120, 200, 255, "  mayland protocol");
    println!("  globals:");
    for g in server::globals() {
        println!("    {:>2}  {} v{}", g.name, g.iface.name(), g.iface.version());
    }

    let clients = server::clients();
    if clients.is_empty() {
        println!("  no connections");
    } else {
        println!("  connections:");
        for c in &clients {
            let state = if c.alive { "live" } else { "dead" };
            println!("    client {} pid {} - {} objects, {} bytes queued, {}",
                     c.id, c.pid, c.objects, c.queued, state);
            if let Some(why) = c.death {
                print_error!("      died: {}", why);
            }
        }
    }

    let surfaces = server::surfaces();
    if surfaces.is_empty() {
        println!("  no surfaces");
    } else {
        // Bottom of the stack first, which is also the order they are drawn in
        println!("  surfaces, bottom to top:");
        for s in &surfaces {
            println!("    client {} surface {} [{}] {} - {} commits, buffer {}, {} frame callbacks",
                     s.client, s.id, s.role,
                     if s.title.is_empty() { "untitled" } else { &s.title },
                     s.commits,
                     if s.has_buffer { "attached" } else { "none" },
                     s.pending_frames);
            match (s.focused, s.active) {
                (true, true) => cprintln!(100, 220, 150, "      has the keyboard, drawn as active"),
                (true, false) => cprintln!(100, 220, 150, "      has the keyboard"),
                (false, true) => cprintln!(100, 220, 150, "      drawn as active, the keyboard is elsewhere"),
                (false, false) => {}
            }
            if s.on_top && !s.focused {
                println!("      on top");
            }
        }
    }

    let visible = server::presentable();
    println!("  presentable: {}", visible.len());
    for p in &visible {
        match p.vram {
            Some(vram) => println!("    surface {} - {}x{} stride {} in video memory at {:#x}, position {},{}",
                                   p.surface, p.width, p.height, p.stride, vram,
                                   p.offset.0, p.offset.1),
            None => println!("    surface {} - {}x{} stride {} at phys {:#x}, position {},{}",
                             p.surface, p.width, p.height, p.stride, p.phys,
                             p.offset.0, p.offset.1),
        }
    }

    // Who is drawing the console, and what the text under it is doing. "Nobody"
    // is worth printing: with no terminal open, shell output goes only to the
    // serial port
    let (cols, rows) = crate::io::console::text_size();
    match server::console_window() {
        Some((cl, sf)) => println!(
            "  console: drawn by client {} surface {} - {} by {} characters",
            cl, sf, cols, rows),
        None => println!(
            "  console: nobody is drawing it - {} by {} characters held, serial only",
            cols, rows),
    }
    match wallpaper::size() {
        Some((w, h)) => println!("  wallpaper: {}x{} behind everything", w, h),
        None => match wallpaper::reserved() {
            Some((w, h)) => println!("  wallpaper: not up, {}x{} of memory held for it", w, h),
            None => println!("  wallpaper: none - the memory was refused at boot"),
        },
    }

    // The pointer: where it is, what it is over, and whether the mouse is
    // producing packets. A climbing resync count is a mouse whose stream has
    // slipped
    let (px, py) = pointer::position();
    let (packets, resyncs) = crate::drivers::input::mouse::counts();
    if pointer::visible() {
        print!("  pointer: at {},{}", px, py);
        match server::pointer_focus() {
            Some((c, s)) => println!(" over client {} surface {}", c, s),
            None => println!(" over nothing"),
        }
        if let Some((c, s)) = crate::mayland::input::grab::dragging() {
            cprintln!(100, 220, 150, "    dragging client {} surface {}", c, s);
        }
    } else {
        print!("  pointer: not drawn, at {},{}", px, py);
        match server::pointer_focus() {
            Some((c, s)) => println!(" over client {} surface {}", c, s),
            None => println!(" over nothing"),
        }
    }
    // Whose cursor it is. A client that asked for its own is how to know the
    // blending path is taken, and the answer is the protocol's, not the
    // screen's
    match server::cursor_image() {
        None => println!("    the compositor's arrow"),
        Some(None) => println!("    the client asked for no cursor at all"),
        Some(Some(i)) => cprintln!(100, 220, 150,
            "    the client's own image - {}x{}, hot spot {},{}",
            i.width, i.height, i.hot_x, i.hot_y),
    }
    if !crate::drivers::input::mouse::present() {
        println!("    no mouse - neither the 8042 nor USB produced one ('usb' says which devices there are)");
    } else {
        println!("    {} packets, {} moves, {} buttons held", packets, pointer::moves(),
                 pointer::buttons().count_ones());
        if resyncs > 0 {
            print_error!("    {} packets dropped out of frame", resyncs);
        }
    }

    println!("  keyboard: {}",
             if !input::grabbed() { "with the shell" }
             else if server::focus_key().is_some()
                     && server::focus_key() == server::console_window() {
                 "with the shell - the console is the focused window"
             } else {
                 "held by the active window (Ctrl+Alt+Tab switches, Ctrl+Alt+Esc releases)"
             });
    if input::overruns() > 0 {
        print_error!("      {} keys dropped - a client stopped reading", input::overruns());
    }

    let st = compositor::stats();
    if compositor::running() || st.frames > 0 {
        cprintln!(120, 200, 255, "  compositor: {}",
                  if compositor::running() { "running" } else { "stopped" });
        println!("    {} frames, {} copies, {} dropped, {} raster stalls",
                 st.frames, st.blits, st.dropped, st.stalls);
        // What the console costs the terminal
        let (con_total, con_last) = crate::mayland::server::iface::console::copy_stats();
        if con_total > 0 {
            println!("    console: {} B last copy, {} KiB in all",
                     con_last, con_total / 1024);
        }
        // Flips held back because the copy engine had not finished writing the
        // surface they would show. A few is the gate working; a number climbing
        // with the frame count means the engine is not keeping up
        let held = crate::drivers::gpu::nvidia::scanout::skipped_flips();
        if held > 0 {
            println!("    {} flips held back for the copy engine", held);
        }
        println!("    last frame took {} us ({} us at worst), {} us between frames",
                 st.last_us, st.worst_us, st.period_us);
        // The handover alone: the fence wait, the flip push and the kick.
        // Subtract it from the line above and what is left is composing
        println!("    of which handing it over: {} us ({} us at worst)",
                 st.flip_us, st.flip_worst_us);
        // The worst each step of a frame has been, so the worst frame above can
        // be told apart: the engines, the server lock, the copies...
        // (`render::phases`; a slow frame also says so in `dmesg debug`)
        let w = compositor::phases::worst();
        let mut line = alloc::string::String::from("    worst by step:");
        for (name, us) in compositor::phases::NAMES.iter().zip(w.iter()) {
            line.push_str(&alloc::format!(" {} {}", name, us));
        }
        println!("{} (us)", line);
        // What the frame pays for the engine. Zero means the copies finished on
        // their own by the time the next frame wanted the buffers back
        println!("    waited {} us for the engine ({} us at worst), {} frames it had not finished",
                 st.wait_us, st.wait_worst_us, st.fence_timeouts);
        // Clients that committed pixels their own GPU work had not finished.
        // Not zero means a client asks for more work than a frame has room for,
        // and its window went out one frame early at least once
        let late = crate::mayland::render::late_fences();
        if late != 0 {
            println!("    {} frame(s) went out before a client's GPU work had finished", late);
        }
        // Who decided the geometry above. A window drawn at a size nobody
        // configured shows here
        match layout::kind() {
            layout::Kind::Master =>
                println!("    layout: master - the compositor sizes and places every window"),
            layout::Kind::Dwindle =>
                println!("    layout: dwindle - every window splits the one it opened on"),
            layout::Kind::Float =>
                println!("    layout: float - windows keep the size they drew"),
        }
        // The rectangle an algorithm was given, which is not the screen: the
        // outer gap and reserved edges come off it. A tiled window not fitting
        // inside these numbers is a layout bug
        let (area, gaps, res) = (layout::work_area_now(), layout::gaps(), layout::reserved());
        println!("    work area: {}x{} at {},{} - gaps {} out {} in, reserved {} {} {} {}",
                 area.w, area.h, area.x, area.y, gaps.0, gaps.1,
                 res.top, res.bottom, res.left, res.right);
        // The ring the compositor draws around a window
        let b = compositor::border();
        if b.size > 0 {
            println!("    borders: {} px - {:#08x} with the keyboard, {:#08x} without",
                     b.size, b.active, b.inactive);
        } else {
            println!("    borders: none ('general {{ border_size }}' in may.conf)");
        }
        // Whether windows are composed by the GPU. Printed even when off,
        // because a frame that overran turns it off by itself
        let e = compositor::effects();
        if e.on {
            cprintln!(100, 220, 150,
                "    effects: blur radius {}, alpha {}%, corner {} px, shadow {}% over {} px",
                e.radius as u32, (e.alpha * 100.0) as u32, e.corner as u32,
                (e.shadow * 100.0) as u32, e.margin);
        } else if crate::drivers::gpu::nvidia::blur::ready() {
            {
        let running = layout::running();
        if crate::mayland::shell::anim::enabled() {
            println!("    animations: on, {} curve(s) running", running);
        } else {
            println!("    animations: off");
        }
    }
    println!("    effects: off, the pass is ready ('mayland effects on')");
        } else {
            {
        let running = layout::running();
        if crate::mayland::shell::anim::enabled() {
            println!("    animations: on, {} curve(s) running", running);
        } else {
            println!("    animations: off");
        }
    }
    println!("    effects: the pass is not set up");
        }
        // Where the pass writes. Both surfaces are mapped once at bring-up and
        // a frame only picks between them, so a miss here means it is painting
        // into a surface nothing else draws
        if let Some(b) = crate::drivers::gpu::nvidia::blur::state() {
            println!("    effects write into va {:#x} at {} px stride; surfaces {:#x}->{:#x}, {:#x}->{:#x}; {} client buffer(s) mapped",
                     b.target_va, b.target_stride,
                     b.targets[0].0, b.targets[0].1, b.targets[1].0, b.targets[1].1,
                     b.windows);
            // What the pass reads, its own buffer, so the frame does not lay
            // the whole region down before every run. "none" explains a desktop
            // paying that price
            match b.backdrop {
                Some((vram, va, stride)) => println!(
                    "    effects read a backdrop of their own at vram {:#x}, va {:#x}, {} px stride",
                    vram, va, stride),
                None => println!(
                    "    effects read the screen they write - no backdrop buffer (`nobackdrop`?)"),
            }
            if b.misses > 0 {
                print_error!("    {} frames the pass was pointed at an unmapped surface", b.misses);
            }
            // The shell below the windows, folded into one image and blurred
            // once (dev/mayland-below-windows.md). "not folded" with a
            // see-through layer at the bottom is a frame on the processor
            let c = compositor::base::counts();
            match b.base {
                Some((vram, va, _)) if c.folded > 0 => println!(
                    "    below the windows: {} surface(s) folded into vram {:#x}, va {:#x}; {} Kpx redrawn last time, {} updates, {} blurs{}",
                    c.folded, vram, va, c.last_px / 1000, c.updates, c.blurs,
                    if b.blurred_valid { "" } else { " - not blurred now" }),
                Some(_) => println!("    below the windows: not folded"),
                None => println!("    below the windows: no image for them - their layers are composed separately"),
            }
            if c.refused > 0 {
                print_error!("    {} update(s) below the windows refused, last: {}", c.refused, c.why);
            }
        }
        // The stack the effects rule is applied to. A window is drawn with
        // effects or copied, so it is printed rather than inferred from the
        // screen
        let stack = compositor::stack_dump();
        if stack.is_empty() {
            println!("    nothing is on the screen - no surface has committed a buffer");
        }
        for (i, s) in stack.iter().enumerate() {
            println!("    {} client {} surface {}: {}x{} at {},{} phys {:#x} stride {} - {}",
                     i, s.client, s.surface, s.rect.2, s.rect.3, s.rect.0, s.rect.1,
                     s.phys, s.stride,
                     if s.stacked { "composed with effects, stacked over the windows below" }
                     else if s.effects { "composed with effects" }
                     else if i == 0 { "copied, the backdrop" }
                     else if s.wants_effects { "copied - effects asked for and refused" }
                     else { "copied" });
            // Only where it is not the desktop's own answer, so a screen with
            // no window rules prints the same lines
            if s.effects || s.wants_effects {
                let d = compositor::effects();
                if (s.alpha - d.alpha).abs() > 0.001 || (s.corner - d.corner).abs() > 0.001 {
                    println!("        its own: alpha {}%, corner {} px",
                             (s.alpha * 100.0) as u32, s.corner as u32);
                }
            }
        }
        // What the damage tracking saves: how much of the screen the last frame
        // had to redraw
        println!("    last frame redrew {} rectangles, {} Kpx; {} Mpx in all",
                 st.last_rects, st.last_px / 1000, st.pixels / 1_000_000);
        // The backdrop's own copies, apart from the line above: they go to a
        // buffer and not the panel, and this number should stay near zero while
        // the one above moves
        if st.backdrop_pixels > 0 {
            println!("    backdrop took {} Kpx last frame; {} Mpx in all",
                     st.last_backdrop_px / 1000, st.backdrop_pixels / 1_000_000);
        }
        // The windows' copies in video memory (`render::texture`): what crossed
        // the bus to keep them current. The uploads should be the damage, not
        // the windows, every frame
        if compositor::texture::enabled() {
            let t = compositor::texture::counts();
            println!("    window copies: {} in video memory, {} MiB; uploaded {} Kpx last frame, {} Mpx in all, {} whole",
                     t.live, t.bytes >> 20, t.last_px / 1000, t.total_px / 1_000_000, t.whole);
            if t.refused > 0 || t.upload_refused > 0 {
                print_error!("    {} window-frame(s) read from the client's memory for want of a copy, {} upload(s) refused",
                             t.refused, t.upload_refused);
            }
        } else {
            println!("    window copies: off (`notex`) - every window is read from its client's memory");
        }
        // Windows whose client drew them in video memory (`gpu_alloc`): no
        // copy,
        // read where they lie, and only what the processor composes over them
        // comes down (dev/gpu-window-buffers.md)
        let t = compositor::texture::counts();
        if t.in_place > 0 || t.mirrored_total_px > 0 || t.left_out > 0 {
            println!("    {} window(s) read in place from video memory; {} Kpx mirrored for the processor last time, {} Mpx in all",
                     t.in_place, t.mirrored_last_px / 1000, t.mirrored_total_px / 1_000_000);
            if t.left_out > 0 {
                print_error!("    {} time(s) the processor left such a window out - nothing to bring it down with",
                             t.left_out);
            }
        }
        // Where the beam was while writing. Copies finishing before it reaches
        // them are never seen half-done
        println!("    raster at line {} when copying began ({} at worst), {} when it ended ({} at worst)",
                 st.line_in, st.line_in_worst, st.line_out, st.line_out_worst);
        // Where the frame is composed, which decides whether any of the raster
        // arithmetic below matters at all
        let (flips, refused) = scanout::flip_counts();
        if scanout::flipping() {
            cprintln!(100, 220, 150,
                "    composing into a second surface - {} flips, handed over at the frame boundary",
                flips);
            if refused > 0 {
                print_error!("    {} flips the window channel would not take", refused);
            }
        } else {
            println!("    one surface - frames are written into the blanking interval");
            let t = compositor::raster();
            if t.valid() {
                println!("    raster {} lines, visible {}..{} - {} blanked lines to write in",
                         t.vtotal, t.vblanke + 1, t.vblanks - 1, t.vblanke + 1);
            }
            if st.late > 0 {
                println!("    {} frames given up for starting too late", st.late);
            }
            if st.overran > 0 {
                print_error!("    {} frames ran past the blanking interval - those can be seen torn",
                             st.overran);
            }
        }
    } else {
        println!("  compositor: not started");
    }

    for d in demo::stats() {
        // The demo does not know where it is; the stack above says where the
        // compositor put it
        println!("  demo {}{}: {} frames drawn, {} waited for a buffer, {} keys",
                 d.name, if d.active { " (active)" } else { "" },
                 d.frames, d.starved, d.keys);
    }
}

fn pools() {
    let list = shm::list();
    if list.is_empty() {
        println!("  no shared-memory pools");
        return;
    }
    cprintln!(120, 200, 255, "  shared-memory pools");
    for p in &list {
        println!("    handle {} owner pid {} - {} KiB at phys {:#x}",
                 p.handle, p.owner, p.size / 1024, p.phys);
    }
}

fn frame(arg: Option<&str>) {
    let ms = arg.and_then(|s| s.parse::<u32>().ok())
        .unwrap_or_else(|| crate::fs::procfs::uptime_ticks() as u32 * 4);
    // A whole frame's worth of work, in the compositor's own order. The
    // interrupts only collect keys and mouse packets, and a frame turns them
    // into events, so with nothing scanning out they would sit in their rings
    let keys = input::drain(ms);
    if let Some((w, h)) = server::output_size() {
        pointer::drain(w as i32, h as i32, ms);
    }
    server::frame_done(ms);
    println!("  frame presented at t={} ms, {} input events delivered", ms, keys);
}

fn input_cmd(arg: Option<&str>) {
    match arg.unwrap_or("on") {
        "off" => {
            input::set_grab(false);
            println!("  the shell has the keyboard");
        }
        _ => {
            match server::focus() {
                Some((c, s, title)) => {
                    input::set_grab(true);
                    cprintln!(100, 220, 150,
                        "  keys go to client {} surface {} ({})", c, s,
                        if title.is_empty() { "untitled" } else { &title });
                    println!("  Ctrl+Alt+Tab moves to the next window, Ctrl+Alt+Esc takes the keyboard back");
                }
                None => print_error!("  mayland: no window has focus"),
            }
        }
    }
}

fn test() {
    let report = selftest::run();
    let (pass, fail) = (report.passed(), report.failed());

    for c in &report.cases {
        if c.skipped {
            cprintln!(220, 220, 100, "  skip  {}", c.name);
        } else if c.ok {
            cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            print_error!("  FAIL  {}", c.name);
        }
        println!("          {}", c.detail);
    }

    let skip = report.skipped();
    if fail == 0 {
        cprintln!(100, 220, 150, "  {} of {} passed", pass, pass);
    } else {
        print_error!("  {} passed, {} failed", pass, fail);
    }
    if skip > 0 {
        // Named rather than folded into the total: a run skipping half the
        // desktop must not just say "all passed"
        cprintln!(220, 220, 100,
            "  {} skipped - stop the window manager and the terminal to run those",
            skip);
    }
}
