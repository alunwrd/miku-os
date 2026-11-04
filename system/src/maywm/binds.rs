// Keys, modifiers, and dispatchers
//
// A config line names a dispatcher (`killactive`, `workspace`,
// `togglefloating`), looked up here; new kinds are a row in `DISPATCHERS` and
// an arm in `run`. The compositor matches chords against a table this program
// fills, and sends back an id, which is the binding's index in the config.

use crate::conf;
use crate::conn::Conn;
// The requests themselves, shared with the kernel's client half
use crate::requests::Requests;
use crate::config::{Action, Config};
use crate::say;
use crate::keymap;
use crate::protocol::wm::{self, param, state};
use crate::state::State;

// Dispatcher numbers, internal to this program
pub const D_KILLACTIVE: u8 = 0;
pub const D_WORKSPACE: u8 = 1;
pub const D_MOVETOWORKSPACE: u8 = 2;
pub const D_MOVEFOCUS: u8 = 3;
pub const D_SWAPWINDOW: u8 = 4;
pub const D_TOGGLEFLOATING: u8 = 5;
pub const D_FULLSCREEN: u8 = 6;
pub const D_PIN: u8 = 7;
pub const D_SPLITRATIO: u8 = 8;
pub const D_LAYOUT: u8 = 9;
pub const D_EXEC: u8 = 10;
/// Say one word to every listener on `may_desktop`, and mean nothing by it
pub const D_SIGNAL: u8 = 27;
/// Moves or resizes the window under the pointer
/// (`bindm = SUPER, mouse:272, movewindow`); the compositor does the work
pub const D_BEGIN_MOVE: u8 = 11;
pub const D_BEGIN_RESIZE: u8 = 12;
/// Moves or resizes the focused window by pixels, or centers it
pub const D_MOVEACTIVE: u8 = 13;
pub const D_RESIZEACTIVE: u8 = 14;
pub const D_CENTERWINDOW: u8 = 15;
/// Focuses a named window: `focuswindow, class:mayterm`. The match is the same
/// as a window rule's
pub const D_FOCUSWINDOW: u8 = 16;
/// Focuses the previous window
pub const D_FOCUSLAST: u8 = 17;
/// Floats or tiles unconditionally, unlike `togglefloating`
pub const D_SETFLOATING: u8 = 18;
pub const D_SETTILED: u8 = 19;
/// Trade places with the next window, without a direction to give it
pub const D_SWAPNEXT: u8 = 20;
/// Sends the window away without following; `movetoworkspace` follows
pub const D_MOVETOWS_SILENT: u8 = 21;
/// Changes the bound key map (`apply_submap`)
pub const D_SUBMAP: u8 = 22;
/// Moves the pointer to a position or corner
pub const D_MOVECURSOR: u8 = 23;
pub const D_CURSORCORNER: u8 = 24;
/// Closes a named window
pub const D_KILLWINDOW: u8 = 25;
/// Rereads the config and applies it
pub const D_RELOAD: u8 = 26;

/// What the event loop does after a chord. A reload happens outside `run`,
/// which borrows the config
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum After {
    Nothing,
    Reload,
}

static DISPATCHERS: &[(&str, u8)] = &[
    ("killactive",      D_KILLACTIVE),
    ("closewindow",     D_KILLACTIVE),
    ("workspace",       D_WORKSPACE),
    ("movetoworkspace", D_MOVETOWORKSPACE),
    ("movefocus",       D_MOVEFOCUS),
    ("cyclenext",       D_MOVEFOCUS),
    ("swapwindow",      D_SWAPWINDOW),
    ("togglefloating",  D_TOGGLEFLOATING),
    ("fullscreen",      D_FULLSCREEN),
    ("pin",             D_PIN),
    ("splitratio",      D_SPLITRATIO),
    ("layoutmsg",       D_LAYOUT),
    ("layout",          D_LAYOUT),
    ("exec",            D_EXEC),
    // A word to `may_desktop` listeners: `bind = $mod, D, signal, launcher`
    ("signal",          D_SIGNAL),
    // Same names as the directional forms. With a mouse button they begin a
    // drag; with a key and direction they move one place (`argument`)
    ("movewindow",      D_BEGIN_MOVE),
    ("resizewindow",    D_BEGIN_RESIZE),
    ("moveactive",      D_MOVEACTIVE),
    ("resizeactive",    D_RESIZEACTIVE),
    ("centerwindow",    D_CENTERWINDOW),
    // Second form is the first with `class:` assumed
    ("focuswindow",     D_FOCUSWINDOW),
    ("focuswindowbyclass", D_FOCUSWINDOW),
    ("focuscurrentorlast", D_FOCUSLAST),
    ("setfloating",     D_SETFLOATING),
    ("settiled",        D_SETTILED),
    ("swapnext",        D_SWAPNEXT),
    ("movetoworkspacesilent", D_MOVETOWS_SILENT),
    ("submap",          D_SUBMAP),
    ("movecursor",      D_MOVECURSOR),
    ("movecursortocorner", D_CURSORCORNER),
    ("killwindow",      D_KILLWINDOW),
    ("reload",          D_RELOAD),
    ("reloadconfig",    D_RELOAD),
    // `execr` is the same as `exec`, with no shell
    ("execr",           D_EXEC),
];

/// Workspace argument: `1`, `+1`, `-1`, `e+1`, `previous`, `r-2`. Mode in the
/// second byte, number in the first, resolved at run time since `e+1` depends
/// on occupied workspaces
pub mod ws {
    /// A workspace by number
    pub const ABS: i32 = 0;
    /// Relative, wrapping
    pub const REL: i32 = 1;
    /// The previous workspace
    pub const PREV: i32 = 2;
    /// Relative among occupied workspaces (Hyprland's `e`)
    pub const USED: i32 = 3;

    pub fn pack(mode: i32, n: i32) -> i32 {
        (mode << 8) | (n.clamp(-128, 127) as u8) as i32
    }
    pub fn mode(arg: i32) -> i32 { arg >> 8 }
    pub fn count(arg: i32) -> i32 { (arg & 0xff) as u8 as i8 as i32 }
}

/// `1`, `+1`, `e-2`, `previous`. `e` counts occupied workspaces; `r` counts
/// within the monitor, the same as plain with one output
fn workspace_arg(arg: &str) -> i32 {
    let a = arg.trim();
    if a == "previous" || a == "prev" {
        return ws::pack(ws::PREV, 0);
    }
    let (mode, rest) = match a.as_bytes().first() {
        Some(b'e') => (ws::USED, &a[1..]),
        Some(b'r') => (ws::REL, &a[1..]),
        Some(b'm') => (ws::REL, &a[1..]),
        _ => (ws::REL, a),
    };
    // A leading sign makes it relative
    match rest.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let n = crate::conf::number(rest).unwrap_or(0.0) as i32;
            ws::pack(mode, n)
        }
        _ => {
            // `e2` with no sign is a number, as in Hyprland
            ws::pack(ws::ABS, crate::conf::number(rest).unwrap_or(0.0) as i32)
        }
    }
}

/// Workspace a packed argument means now. `None` if out of range or no previous
/// one; the dispatcher then does nothing
pub fn resolve_workspace(st: &State, arg: i32) -> Option<u32> {
    let total = wm::WORKSPACES as i32;
    let here = st.workspace as i32;
    match ws::mode(arg) {
        ws::ABS => {
            let n = ws::count(arg);
            (n >= 1 && n <= total).then_some(n as u32)
        }
        ws::PREV => (st.prev_workspace >= 1).then_some(st.prev_workspace),
        ws::REL => {
            let n = ws::count(arg);
            Some(((here - 1 + n).rem_euclid(total) + 1) as u32)
        }
        _ => {
            // The n-th occupied workspace in that direction; with none, a plain
            // relative step
            let n = ws::count(arg);
            let step = if n < 0 { -1 } else { 1 };
            let mut at = here;
            let mut left = n.abs();
            for _ in 0..total * n.abs().max(1) {
                at = (at - 1 + step).rem_euclid(total) + 1;
                if at == here {
                    break;
                }
                if st.workspace_used(at as u32) {
                    left -= 1;
                    if left == 0 {
                        return Some(at as u32);
                    }
                }
            }
            Some(((here - 1 + n).rem_euclid(total) + 1) as u32)
        }
    }
}

/// Whether a button or a key is what a chord is bound to
pub fn is_mouse(key: u32) -> bool { (0x100..0x200).contains(&key) }

/// Dispatcher once the bound key is known. On a button it begins a grab; on a
/// key `movewindow` swaps with the neighbor
pub fn for_key(dispatch: u8, key: u32) -> u8 {
    match dispatch {
        D_BEGIN_MOVE if !is_mouse(key) => D_SWAPWINDOW,
        D_BEGIN_RESIZE if !is_mouse(key) => D_RESIZEACTIVE,
        other => other,
    }
}

pub fn dispatcher(name: &str) -> Option<u8> {
    DISPATCHERS.iter().find(|(n, _)| *n == name).map(|(_, d)| *d)
}

/// Meaning of the argument column, per dispatcher
pub fn argument(dispatch: u8, arg: &str) -> i32 {
    match dispatch {
        // Packed, since `+1`, `e-2`, and `previous` resolve at key press
        D_WORKSPACE | D_MOVETOWORKSPACE | D_MOVETOWS_SILENT => workspace_arg(arg),
        // Hyprland numbers the corners anticlockwise from the bottom left
        D_CURSORCORNER => crate::conf::number(arg).unwrap_or(0.0) as i32 & 3,
        // Two numbers, as `moveactive` takes them
        D_MOVECURSOR => pack_pair(arg),
        // `movewindow, l` on a key becomes a swap, remapped after this call, so
        // both names are listed
        D_MOVEFOCUS | D_SWAPWINDOW | D_BEGIN_MOVE => match arg {
            "prev" | "up" | "l" | "left" => -1,
            _ => 1,
        },
        D_LAYOUT => match arg {
            "dwindle" => wm::LAYOUT_DWINDLE as i32,
            "float" | "floating" => wm::LAYOUT_FLOAT as i32,
            "master" => wm::LAYOUT_MASTER as i32,
            // No name: cycles
            _ => -1,
        },
        // Ratio in hundredths
        D_SPLITRATIO => (crate::conf::number(arg).unwrap_or(0.0) * 100.0) as i32,
        // `moveactive 100 0`, `resizeactive -50 0`: two numbers packed in the
        // argument's halves
        D_MOVEACTIVE | D_RESIZEACTIVE => pack_pair(arg),
        _ => crate::conf::number(arg).unwrap_or(0.0) as i32,
    }
}

/// Two numbers in the halves of one
fn pack_pair(arg: &str) -> i32 {
    let mut it = arg.split_whitespace();
    let dx = it.next().and_then(crate::conf::number).unwrap_or(0.0) as i32;
    let dy = it.next().and_then(crate::conf::number).unwrap_or(0.0) as i32;
    ((dx.clamp(-32000, 32000) as u32 & 0xffff) << 16
     | (dy.clamp(-32000, 32000) as u32 & 0xffff)) as i32
}

/// `SUPER SHIFT` or `SUPER+SHIFT`, in either spelling
pub fn modmask(s: &str) -> u32 {
    let mut mask = 0;
    for word in s.split(|c: char| c == '+' || c.is_whitespace()) {
        let w = word.trim();
        if w.is_empty() {
            continue;
        }
        mask |= match_mod(w);
    }
    mask
}

fn match_mod(w: &str) -> u32 {
    // Case-insensitive
    let mut buf = [0u8; 16];
    let n = w.len().min(buf.len());
    for i in 0..n {
        buf[i] = w.as_bytes()[i].to_ascii_lowercase();
    }
    match core::str::from_utf8(&buf[..n]).unwrap_or("") {
        "super" | "mod" | "logo" | "win" => keymap::MOD_LOGO,
        "shift" => keymap::MOD_SHIFT,
        "ctrl" | "control" => keymap::MOD_CTRL,
        "alt" | "mod1" => keymap::MOD_ALT,
        _ => 0,
    }
}

/// Mouse buttons in evdev numbering, matching the compositor's chord table and
/// `mouse:272`
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// Case-insensitive `starts_with`, with no allocator
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() {
        return None;
    }
    let (head, rest) = s.split_at(prefix.len());
    head.bytes().zip(prefix.bytes())
        .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
        .then_some(rest)
}

pub fn keycode(name: &str) -> Option<u32> {
    // `mouse:272` and its aliases. evdev numbers buttons from 0x110 and
    // keyboard keys stop below 0x100, so buttons share the chord table
    if let Some(rest) = strip_prefix_ci(name, "mouse:") {
        return crate::conf::number(rest).map(|n| n as u32)
            .filter(|&c| (0x100..0x200).contains(&c));
    }
    // Keys with no character, by xkb names, in evdev numbers (below 0x59 the
    // same as AT set-1). Punctuation is included, since a comma separates
    // fields
    let named: &[(&str, u32)] = &[
        ("mouse_left",   BTN_LEFT),
        ("mouse_right",  BTN_RIGHT),
        ("mouse_middle", BTN_MIDDLE),
        ("return", keymap::KEY_ENTER as u32),
        ("enter",  keymap::KEY_ENTER as u32),
        ("escape", keymap::KEY_ESC as u32),
        ("esc",    keymap::KEY_ESC as u32),
        ("tab",    keymap::KEY_TAB as u32),
        ("space",  keymap::KEY_SPACE as u32),
        // Modifier keys, for tap bindings: `bindr = SUPER, Super_L`
        // (`protocol::wm::chord::RELEASE`)
        ("super_l", 125), ("super_r", 126),
        ("control_l", 29), ("control_r", 97),
        ("shift_l", 42), ("shift_r", 54),
        ("alt_l", 56), ("alt_r", 100),
        ("backspace", keymap::KEY_BACKSPACE as u32),
        // Function row; F11 and F12 are not contiguous in the AT set
        ("f1", 59), ("f2", 60), ("f3", 61), ("f4", 62), ("f5", 63), ("f6", 64),
        ("f7", 65), ("f8", 66), ("f9", 67), ("f10", 68), ("f11", 87), ("f12", 88),
        // Navigation, renumbered from 0xE0-prefixed codes by the compositor
        ("home", 102), ("up", 103), ("pageup", 104), ("prior", 104),
        ("left", 105), ("right", 106),
        ("end", 107), ("down", 108), ("pagedown", 109), ("next", 109),
        ("insert", 110), ("delete", 111), ("del", 111),
        // Punctuation, by the names xkb gives it
        ("minus", 12), ("equal", 13),
        ("bracketleft", 26), ("bracketright", 27),
        ("semicolon", 39), ("apostrophe", 40), ("grave", 41),
        ("backslash", 43), ("comma", 51), ("period", 52), ("slash", 53),
    ];
    let mut buf = [0u8; 16];
    let n = name.len().min(buf.len());
    for i in 0..n {
        buf[i] = name.as_bytes()[i].to_ascii_lowercase();
    }
    let lower = core::str::from_utf8(&buf[..n]).unwrap_or("");
    if let Some(&(_, code)) = named.iter().find(|(k, _)| *k == lower) {
        return Some(code);
    }
    let mut chars = lower.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    // The keycodes the compositor actually sends, asked what they produce
    (1u16..128).find(|&k| keymap::to_char(k, 0) == Some(c)).map(|k| k as u32)
}

/// Binds the live chords. Ids are config indices across key maps, so a queued
/// chord still fires its action. Rebinding is required, since a bound chord is
/// swallowed even while another map is up. The compositor holds 32 chords and a
/// config may have 64
pub fn apply_submap(c: &mut Conn, wm: u32, cfg: &Config, st: &mut State, map: u8) {
    // The whole id space, since ids past a shorter reloaded config remain bound
    // and would swallow keys
    for i in 0..crate::config::MAX_BINDS {
        c.wm_unbind_chord(wm, i as u32);
    }
    for i in 0..cfg.n_binds {
        let b = cfg.binds[i];
        // The default map is always live
        if b.submap == 0 || b.submap == map {
            c.wm_bind_chord(wm, i as u32, b.mods, b.key, b.flags);
        }
    }
    st.submap = map;
}

/// Binds the default map
pub fn bind_all(c: &mut Conn, wm: u32, cfg: &Config, st: &mut State) {
    apply_submap(c, wm, cfg, st, 0);
}

/// The two halves of a packed `dx dy` argument, sign restored
fn unpack(arg: i32) -> (i32, i32) {
    let v = arg as u32;
    (((v >> 16) as u16) as i16 as i32, (v as u16) as i16 as i32)
}

/// Minimum window size from a binding
const MIN_SIZE: i32 = 64;

/// Runs a chord's action
pub fn run(c: &mut Conn, desk: u32, wm: u32, st: &mut State, cfg: &Config, a: Action)
    -> After
{
    match a.dispatch {
        D_KILLACTIVE => {
            let (fc, fs) = st.focus;
            // Never the console or wallpaper
            if fc != 0 && !st.is_sticky(fc, fs) {
                c.desk_close(desk, fc, fs);
            }
        }
        D_WORKSPACE => {
            if let Some(to) = resolve_workspace(st, a.arg) {
                c.desk_workspace(desk, to);
            }
        }
        // `movetoworkspace` follows, the silent form does not
        D_MOVETOWORKSPACE | D_MOVETOWS_SILENT => {
            let (fc, fs) = st.focus;
            let Some(to) = resolve_workspace(st, a.arg) else { return After::Nothing };
            if fc != 0 && !st.is_sticky(fc, fs) {
                c.desk_move_to_workspace(desk, fc, fs, to);
                st.set_workspace_of(fc, fs, to);
                if a.dispatch == D_MOVETOWORKSPACE {
                    c.desk_workspace(desk, to);
                }
            }
        }
        D_MOVEFOCUS => {
            if let Some(w) = st.neighbour(a.arg) {
                c.desk_focus(desk, w.0, w.1);
            }
        }
        D_SWAPWINDOW => {
            let (fc, fs) = st.focus;
            if let Some(w) = st.neighbour(a.arg) {
                if fc != 0 {
                    c.wm_swap(wm, fc, fs, w.0, w.1);
                }
            }
        }
        D_TOGGLEFLOATING => {
            let (fc, fs) = st.focus;
            if fc != 0 {
                let now = !st.is_floating(fc, fs);
                c.wm_set_window_state(wm, fc, fs, state::FLOATING,
                                      if now { state::FLOATING } else { 0 });
                st.set_floating(fc, fs, now);
                if let Some(i) = st.find(fc, fs) {
                    crate::rules::refloat(c, wm, st, cfg, i);
                }
            }
        }
        D_FULLSCREEN => {
            let (fc, fs) = st.focus;
            if fc != 0 {
                let now = !st.is_fullscreen(fc, fs);
                c.wm_set_window_state(wm, fc, fs, state::FULLSCREEN,
                                      if now { state::FULLSCREEN } else { 0 });
                st.set_fullscreen(fc, fs, now);
            }
        }
        D_PIN => {
            let (fc, fs) = st.focus;
            if fc != 0 {
                let now = !st.is_pinned(fc, fs);
                c.wm_set_window_state(wm, fc, fs, state::PINNED,
                                      if now { state::PINNED } else { 0 });
                st.set_pinned(fc, fs, now);
            }
        }
        D_SPLITRATIO => {
            // Moves the split above the focused window, relative, so a held key
            // walks the divider
            let (fc, fs) = st.focus;
            if fc != 0 {
                st.split = (st.split + a.arg as f32 / 100.0).clamp(0.1, 1.9);
                c.wm_set_window_param(wm, fc, fs, param::SPLIT_RATIO, st.split, true);
            }
        }
        D_LAYOUT => {
            st.layout = if a.arg < 0 {
                // Cycles master, dwindle, float
                match st.layout {
                    wm::LAYOUT_MASTER => wm::LAYOUT_DWINDLE,
                    wm::LAYOUT_DWINDLE => wm::LAYOUT_FLOAT,
                    _ => wm::LAYOUT_MASTER,
                }
            } else {
                a.arg as u32
            };
            c.wm_set_layout(wm, st.layout);
        }
        D_EXEC => exec(a.text.as_str()),
        D_SIGNAL => c.desk_signal(desk, a.text.as_str()),
        // Handled by the caller
        D_RELOAD => return After::Reload,
        D_FOCUSWINDOW => {
            if let Some((wc, wsf)) = st.match_window(a.text.as_str()) {
                c.desk_focus(desk, wc, wsf);
            }
        }
        D_KILLWINDOW => {
            if let Some((wc, wsf)) = st.match_window(a.text.as_str()) {
                if !st.is_sticky(wc, wsf) {
                    c.desk_close(desk, wc, wsf);
                }
            }
        }
        D_FOCUSLAST => {
            let (pc, ps) = st.prev_focus;
            if pc != 0 && st.find(pc, ps).is_some() {
                c.desk_focus(desk, pc, ps);
            }
        }
        D_SETFLOATING | D_SETTILED => {
            let (fc, fs) = st.focus;
            if fc != 0 {
                let on = a.dispatch == D_SETFLOATING;
                c.wm_set_window_state(wm, fc, fs, state::FLOATING,
                                      if on { state::FLOATING } else { 0 });
                st.set_floating(fc, fs, on);
                if let Some(i) = st.find(fc, fs) {
                    crate::rules::refloat(c, wm, st, cfg, i);
                }
            }
        }
        D_SWAPNEXT => {
            let (fc, fs) = st.focus;
            if let Some(w) = st.neighbour(1) {
                if fc != 0 && w != (fc, fs) {
                    c.wm_swap(wm, fc, fs, w.0, w.1);
                }
            }
        }
        D_SUBMAP => {
            let name = a.text.as_str().trim();
            // `reset` returns to the default map; an unknown name is refused
            let map = match name {
                "reset" | "" => Some(0u8),
                n => (1..=cfg.n_submaps as u8)
                    .find(|&i| cfg.submap_name(i) == n),
            };
            match map {
                Some(m) => apply_submap(c, wm, cfg, st, m),
                None => say("submap: no key map of that name"),
            }
        }
        D_MOVECURSOR => {
            let (x, y) = unpack(a.arg);
            c.wm_warp_pointer(wm, x, y);
        }
        D_CURSORCORNER => {
            // Anticlockwise from bottom left, in the work area
            let (ax, ay, aw, ah) = st.work_area;
            if aw > 0 && ah > 0 {
                let (x, y) = match a.arg {
                    0 => (ax, ay + ah - 1),
                    1 => (ax + aw - 1, ay + ah - 1),
                    2 => (ax + aw - 1, ay),
                    _ => (ax, ay),
                };
                c.wm_warp_pointer(wm, x, y);
            }
        }
        // Window `0, 0` means the one under the pointer, chosen by the
        // compositor
        D_BEGIN_MOVE => c.wm_begin_move(wm, 0, 0),
        // Edges of 0: the compositor derives the corner from the pointer
        D_BEGIN_RESIZE => c.wm_begin_resize(wm, 0, 0, 0),
        D_MOVEACTIVE => {
            let (dx, dy) = unpack(a.arg);
            if let Some((fc, fs, x, y, _, _)) = st.focus_box() {
                c.wm_move(wm, fc, fs, x + dx, y + dy);
            }
        }
        D_RESIZEACTIVE => {
            let (dx, dy) = unpack(a.arg);
            if let Some((fc, fs, _, _, w, h)) = st.focus_box() {
                // Clamped here as well, so a negative size gives the smallest
                // window
                c.wm_resize(wm, fc, fs, (w + dx).max(MIN_SIZE), (h + dy).max(MIN_SIZE));
            }
        }
        D_CENTERWINDOW => {
            let (ax, ay, aw, ah) = st.work_area;
            if let Some((fc, fs, _, _, w, h)) = st.focus_box() {
                if aw > 0 && ah > 0 {
                    c.wm_move(wm, fc, fs, ax + (aw - w) / 2, ay + (ah - h) / 2);
                }
            }
        }
        _ => {}
    }
    After::Nothing
}

/// Starts a program through `libmay::spawn`, reporting failures with the
/// command name
fn exec(cmd: &str) {
    if let Err(why) = libmay::spawn::run(cmd) {
        let mut buf = [0u8; conf::MAX_TEXT + 48];
        let mut n = 0;
        let mut put = |s: &str| {
            let take = s.len().min(buf.len() - n);
            buf[n..n + take].copy_from_slice(&s.as_bytes()[..take]);
            n += take;
        };
        put("exec: ");
        put(why.as_str());
        put(" - ");
        put(cmd.trim());
        say(core::str::from_utf8(&buf[..n]).unwrap_or("exec: could not start it"));
    }
}
