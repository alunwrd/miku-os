#![no_std]
#![no_main]

// The window manager process
//
// Holds `may_wm`, which one client may bind, and drives workspaces, keyboard
// focus, closing, layout, and its numbers. The compositor computes geometry, so
// the desktop works without this process and windows appear without a round
// trip.
//
// `config`: the file in Hyprland's shape
// `binds`: key names, modifiers, dispatchers
// `rules`: window rules
// `state`: what the compositor reports
//
// Chords match in the kernel's keyboard interrupt against a table this program
// fills, and arrive as events. Ctrl+Alt+Escape and Ctrl+Alt+Tab are the
// compositor's and cannot be bound over.

pub use miku;

pub use libmay::wire;
pub use libmay::protocol;
pub use libmay::conn;
// Every request a client can make, shared with the kernel half
pub use libmay::requests;
pub use libmay::keymap;
// Config grammar
pub use libmay::conf;
// Config file list
pub use libmay::manifest;
// `monitor =` lines, shared with the kernel's selftest
pub use libmay::monitor;

mod binds;
mod config;
mod outputs;
mod rules;
mod state;

use conn::{Conn, Event};
use requests::Requests;
use config::{Config, Text};
use protocol::Interface;
use state::{State, Win};

fn fail(msg: &str) -> ! {
    miku::print("  maywm: ");
    miku::println(msg);
    miku::exit(1);
}

fn say(msg: &str) {
    miku::print("  maywm: ");
    miku::println(msg);
}

/// Print a small number without an allocator
fn say_num(prefix: &str, n: u32) {
    let mut buf = [0u8; 20];
    let len = miku::fmt_dec(&mut buf, n as u64);
    miku::print("  maywm: ");
    miku::print(prefix);
    miku::print(core::str::from_utf8(&buf[..len]).unwrap_or("?"));
    miku::println("");
}

/// Prints the server's error message
fn say_error(object: u32, code: u32, msg: &[u8]) {
    miku::print("  maywm: the display server objected: ");
    miku::print(core::str::from_utf8(msg).unwrap_or("?"));
    say_num(" - object ", object);
    say_num("   code ", code);
}

/// Desktop for a machine with no config, parsed on every boot. `binde` keys
/// repeat when held. 30 of the compositor's 32 chords
const BUILT_IN: &str = "\
$mod = SUPER
general {
    gaps_in = 12
    gaps_out = 16
    layout = master
    border_size = 2
    col.active_border = rgba(cfbdfeee)
    col.inactive_border = rgba(4c3e75aa)
}
bind = CTRL, 1, workspace, 1
bind = CTRL, 2, workspace, 2
bind = CTRL, 3, workspace, 3
bind = CTRL, 4, workspace, 4
bind = CTRL, 5, workspace, 5
bind = CTRL, 6, workspace, 6
bind = CTRL, 7, workspace, 7
bind = CTRL, 8, workspace, 8
bind = CTRL, 9, workspace, 9
bind = CTRL, 0, workspace, 10
bind = CTRL SHIFT, 1, movetoworkspace, 1
bind = CTRL SHIFT, 2, movetoworkspace, 2
bind = CTRL SHIFT, 3, movetoworkspace, 3
bind = CTRL SHIFT, 4, movetoworkspace, 4
bind = CTRL SHIFT, 5, movetoworkspace, 5
bind = CTRL SHIFT, 6, movetoworkspace, 6
bind = CTRL SHIFT, 7, movetoworkspace, 7
bind = CTRL SHIFT, 8, movetoworkspace, 8
bind = CTRL SHIFT, 9, movetoworkspace, 9
bind = CTRL SHIFT, 0, movetoworkspace, 10
bind = $mod, J, movefocus, next
bind = $mod, K, movefocus, prev
bind = $mod, TAB, workspace, previous
bind = $mod, Q, killactive
bind = $mod, M, layout
bind = $mod SHIFT, R, reload
bindm = ALT, mouse:272, movewindow
bindm = ALT, mouse:273, resizewindow
bind = $mod, V, togglefloating
bind = $mod, F, fullscreen
bind = $mod, P, pin
binde = $mod, H, splitratio, -0.05
binde = $mod, L, splitratio, 0.05
";

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    // Config first, since it holds the chords
    let mut cfg = config::load();
    if !cfg.from_file {
        cfg = Config::new();
        config::parse(BUILT_IN, &mut cfg);
    }

    let mut c = match Conn::open() {
        Some(c) => c,
        None => fail("cannot reach the display server"),
    };

    let mut evs = [Event::Other { object: 0, opcode: 0 }; 32];
    let (registry, globals) = c.handshake();
    if globals.failed {
        fail("the display server refused the handshake");
    }
    // Needs `may_desktop` and `may_wm`; the server allows one client to hold
    // the latter
    if !globals.has(Interface::Desktop) {
        fail("this display server has no may_desktop - nothing to see windows with");
    }
    if !globals.has(Interface::Wm) {
        fail("this display server has no may_wm - nothing to manage windows with");
    }

    let desk = c.bind(registry, globals.name(Interface::Desktop),
                      Interface::Desktop, Interface::Desktop.version());
    let wm = c.bind(registry, globals.name(Interface::Wm),
                    Interface::Wm, Interface::Wm.version());
    // Outputs after `may_wm`: the compositor gives the output manager only to
    // its holder
    let mut outs = outputs::Outputs::new(if globals.has(Interface::OutputManager) {
        c.bind(registry, globals.name(Interface::OutputManager),
               Interface::OutputManager, Interface::OutputManager.version())
    } else {
        0
    });

    let mut st = State::new();
    apply(&mut c, wm, &cfg, &mut st);
    c.desk_get_windows(desk, 1);
    c.sync();
    if !c.flush() {
        fail("the display server would not take the chords");
    }

    say(if cfg.from_file {
        "holding may_wm, the desktop is what /etc/mayland says"
    } else {
        "holding may_wm, no config file, so the built-in bindings are up"
    });
    describe(&cfg);
    // Starts programs once the desktop is up. Outside `apply`, so a reload does
    // not restart them
    for i in 0..cfg.n_execs {
        let _ = binds::run(&mut c, desk, wm, &mut st, &cfg, config::Action {
            dispatch: binds::D_EXEC, arg: 0, text: cfg.execs[i],
        });
    }
    if cfg.n_anims > 0 {
        say_num("animations: ", cfg.n_anims as u32);
    }
    if cfg.n_rules > 0 {
        say_num("window rules: ", cfg.n_rules as u32);
    }

    loop {
        let n = c.read_events(&mut evs);
        if n == 0 {
            // Only end of stream means the server is gone
            if c.closed {
                say(c.death);
                miku::exit(0);
            }
            // One frame; the read returns empty and would otherwise spin
            miku::sleep_ms(16);
            continue;
        }
        let mut acted = false;
        // Applied after the loop, since the config cannot change while a chord
        // reads it
        let mut reload = false;

        for i in 0..n {
            match evs[i] {
                // Not fatal: one refused request leaves the rest working
                Event::Error { object, code, msg, msg_len } =>
                    say_error(object, code, &msg[..msg_len]),

                // End of a full listing. A focus not in it means a window
                // closed in flight
                Event::DeskWindowsDone { .. } => {
                    if st.focus != (0, 0) && st.find(st.focus.0, st.focus.1).is_none() {
                        st.focus = (0, 0);
                    }
                }

                Event::DeskWindow { client, surface, flags, workspace,
                                  x, y, width, height,
                                  title, title_len, app_id, app_id_len } => {
                    let mut w = Win::EMPTY;
                    w.client = client;
                    w.surface = surface;
                    w.workspace = workspace;
                    w.x = x;
                    w.y = y;
                    w.w = width;
                    w.h = height;
                    w.sticky = flags & protocol::wm::FLAG_STICKY != 0;
                    w.backdrop = flags & protocol::wm::FLAG_BACKDROP != 0;
                    w.floating = flags & protocol::wm::FLAG_FLOATING != 0;
                    w.fullscreen = flags & protocol::wm::FLAG_FULLSCREEN != 0;
                    w.pinned = flags & protocol::wm::FLAG_PINNED != 0;
                    w.app_id = Text::from(
                        core::str::from_utf8(&app_id[..app_id_len]).unwrap_or(""));
                    w.title = Text::from(
                        core::str::from_utf8(&title[..title_len]).unwrap_or(""));
                    st.add(w);
                    if flags & protocol::wm::FLAG_FOCUSED != 0 {
                        st.set_focus((client, surface));
                    }
                    // Applies rules when a window first appears
                    if let Some(idx) = st.find(client, surface) {
                        if rules::apply(&mut c, desk, wm, &mut st, &cfg, idx) {
                            acted = true;
                        }
                    }
                }
                Event::DeskOpened { client, surface } => {
                    // Requests the list again, since this event has no
                    // workspace, flags, or app id
                    let mut w = Win::EMPTY;
                    w.client = client;
                    w.surface = surface;
                    w.workspace = st.workspace;
                    st.add(w);
                    c.desk_get_windows(desk, 2);
                    acted = true;
                }
                Event::DeskClosed { client, surface } => st.remove(client, surface),
                Event::DeskFocused { client, surface } => st.set_focus((client, surface)),
                // Not logged
                Event::DeskWorkspace { index } => st.set_workspace(index),
                Event::DeskWorkArea { x, y, width, height, .. } =>
                    st.work_area = (x, y, width, height),

                // The outputs and the answers to configuring them
                ev @ (Event::OutHead { .. } | Event::OutMode { .. } | Event::OutDone { .. }
                      | Event::OutConfigSucceeded { .. } | Event::OutConfigFailed { .. }
                      | Event::OutConfigCancelled { .. }) => {
                    if outs.event(&mut c, &ev, &cfg) {
                        acted = true;
                    }
                }

                // The id is the binding's config index
                Event::WmChord { id } => {
                    let i = id as usize;
                    if i < cfg.n_binds {
                        // Copied, since `run` also needs the config
                        let action = cfg.binds[i].action;
                        if binds::run(&mut c, desk, wm, &mut st, &cfg, action)
                            == binds::After::Reload
                        {
                            reload = true;
                        }
                        acted = true;
                    }
                }
                _ => {}
            }
        }

        if reload {
            // Parsed separately and swapped only if it came from a file, so a
            // missing file leaves the desktop as is
            let fresh = config::load();
            if fresh.from_file {
                cfg = fresh;
                apply(&mut c, wm, &cfg, &mut st);
                say("config re-read");
                describe(&cfg);
                // Reapplies `monitor =` lines
                outs.reload(&mut c, &cfg);
            } else {
                say("config: nothing could be read from /etc/mayland - \
                     the desktop is unchanged");
            }
            acted = true;
        }

        if acted {
            c.flush();
        }
    }
}

/// Applies a config to the desktop, for both startup and reload. Starts by
/// resetting, since `set_param` only sends changes and a deleted line must take
/// effect. Leaves the window list, `exec-once`, and open windows alone, since
/// rules ran at open (`Win::ruled`)
fn apply(c: &mut Conn, wm: u32, cfg: &Config, st: &mut State) {
    c.wm_reset(wm);
    // Chords first, restoring the default key map
    binds::bind_all(c, wm, cfg, st);
    for i in 0..cfg.n_params {
        let (p, v, _) = cfg.params[i];
        c.wm_set_param(wm, p, v);
    }
    // Curves before the animations naming them; numbered by definition order
    for i in 0..cfg.n_curves {
        let cv = cfg.curves[i];
        c.wm_set_curve(wm, i as u32, cv.x1, cv.y1, cv.x2, cv.y2);
    }
    for i in 0..cfg.n_anims {
        let a = cfg.anims[i];
        c.wm_set_animation(wm, a.class, a.enabled, a.speed, a.curve, a.style, a.arg);
    }
    c.wm_set_layout(wm, cfg.layout);
    c.wm_set_border(wm, cfg.border_size, cfg.border_active, cfg.border_inactive);
    // After the reset above, which forgot the last config's
    for i in 0..cfg.n_layer_rules {
        let (rule, ns) = cfg.layer_rules[i];
        c.wm_set_layer_rule(wm, rule, ns.as_str());
    }
    // Always sent, since `wm_reset` restored the gradient
    c.wm_set_wallpaper(wm, cfg.wallpaper.as_str());
    st.layout = cfg.layout;
}

/// Logs what the config contains
fn describe(cfg: &Config) {
    // Logs the files read
    for i in cfg.manifest.all() {
        miku::print("  maywm: read ");
        miku::println(cfg.manifest.path(i));
    }
    say_num("bindings: ", cfg.n_binds as u32);
    if cfg.n_submaps > 0 {
        say_num("key maps besides the default: ", cfg.n_submaps as u32);
    }
    if cfg.n_anims > 0 {
        say_num("animations: ", cfg.n_anims as u32);
    }
    if cfg.n_rules > 0 {
        say_num("window rules: ", cfg.n_rules as u32);
    }
    if cfg.n_monitors > 0 {
        say_num("monitor lines: ", cfg.n_monitors as u32);
    }
    if cfg.n_notes() > 0 {
        // Count as well as lines, since printing stops at 16
        say_num("lines this desktop did not understand: ", cfg.n_notes());
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::println("maywm: PANIC");
    miku::exit(1);
}
