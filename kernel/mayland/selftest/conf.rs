// The config grammar, and the letters after `bind`
//
// A line such as `binde = $mod, L, resizeactive` has to get from the bytes on
// disk to what the keyboard interrupt does with the L key.
//
// The grammar half runs with no screen, client or server: the kernel links
// `libmay`, the same crate as the window manager, so `libmay::conf` can be
// handed a string. The cases that matter are ones nobody types on purpose: a
// variable defined as itself, a line that outgrows its buffer, a file that
// sources a file that sources it back.
//
// The chord half goes through `input::intercept`, called with real scancodes,
// and out through `input::drain` to a real `may_wm` client. A chord matches on
// the make code, and the keyboard's typematic sends make codes for as long as a
// key is held.

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use libmay::conf::{self, Note, Sink, Text, Where};
use libmay::manifest::{self, Layer, Manifest, Read};
use libmay::qoi;

use super::{exchange, Runner};
use crate::mayland::client::Event;
use crate::mayland::input;
use crate::mayland::protocol::{self, Interface};
use crate::mayland::requests::Requests;
use crate::mayland::server;

/// A dictionary that understands nothing and remembers everything
///
/// Every reader of a config decides what a line means; this one decides that no
/// line means anything, so the tests can ask what the grammar did. `files`
/// answers a `source =` line, so no filesystem is needed
struct Rec {
    keys: Vec<(u32, u8, String, String, String)>,
    lists: Vec<(u32, String, String)>,
    /// Which file each list line came from, beside `lists` so tests reading
    /// `lists` are unaffected
    list_files: Vec<u8>,
    notes: Vec<(u32, &'static str)>,
    files: Vec<(&'static str, &'static str)>,
    /// What a `Manifest` already resolved, if any: path, bytes, and its number
    /// in the manifest. Consulted before `files`, as a real dictionary does:
    /// the file behind a `source =` line has been read by the time it is
    /// dispatched
    src: Vec<(String, &'static str, u8)>,
}

impl Rec {
    fn new() -> Rec {
        Rec {
            keys: Vec::new(), lists: Vec::new(), list_files: Vec::new(),
            notes: Vec::new(), files: Vec::new(), src: Vec::new(),
        }
    }

    /// What the value of one key came out as, whatever section it was in
    fn value(&self, key: &str) -> Option<&str> {
        self.keys.iter().find(|k| k.3 == key).map(|k| k.4.as_str())
    }

    fn noted(&self, what: &str) -> bool {
        self.notes.iter().any(|n| n.1 == what)
    }
}

impl Sink for Rec {
    fn is_list(&self, key: &str) -> bool {
        conf::bind_key(key).is_some() || key == "windowrule"
    }

    fn key(&mut self, at: Where, section: &str, key: &str, value: &str) {
        self.keys.push((at.line, at.file, section.to_string(),
                        key.to_string(), value.to_string()));
    }

    fn list(&mut self, at: Where, key: &str, value: &str) {
        self.lists.push((at.line, key.to_string(), value.to_string()));
        self.list_files.push(at.file);
    }

    fn source(&mut self, _at: Where, path: &str) -> Option<(&'static str, u8)> {
        // File zero is whatever was handed to `parse`; a sourced file is
        // numbered by where it sits in this table, which is all a `Where`
        // needs to name it again
        if let Some(e) = self.src.iter().find(|(p, _, _)| p == path) {
            return Some((e.1, e.2));
        }
        self.files.iter().position(|(p, _)| *p == path)
            .map(|i| (self.files[i].1, i as u8 + 1))
    }

    fn note(&mut self, at: Where, note: Note, _text: &str) {
        self.notes.push((at.line, note.as_str()));
    }
}

/// Where a line was
///
/// A dictionary that does not recognise a line can only complain usefully if
/// the grammar says which line it was. Checked across a `source` as well: the
/// line number restarts in the included file, and reporting the outer number
/// would point at the wrong file
pub(super) fn test_conf_line_numbers(t: &mut Runner) {
    const NAME: &str = "conf: every line knows which line it is, sourced files included";
    let mut r = Rec::new();
    r.files.push(("inner.conf", "\n\n# a comment\nfrom_inner = yes\n"));
    conf::parse("\
general {
    gaps_in = 4
}
source = inner.conf
tail = 1
", &mut r);

    let gaps = r.keys.iter().find(|k| k.3 == "gaps_in").map(|k| (k.0, k.1));
    let inner = r.keys.iter().find(|k| k.3 == "from_inner").map(|k| (k.0, k.1));
    let tail = r.keys.iter().find(|k| k.3 == "tail").map(|k| (k.0, k.1));
    let section = r.keys.iter().find(|k| k.3 == "gaps_in").map(|k| k.2.clone());

    t.check(NAME,
            gaps == Some((2, 0)) && inner == Some((4, 1)) && tail == Some((5, 0))
                && section.as_deref() == Some("general"),
            format!("gaps_in at {:?}, the sourced key at {:?}, the line after \
                     the source at {:?}, section {:?}",
                    gaps, inner, tail, section));
}

/// `$mod = SUPER`, and the two ways it could go wrong
///
/// Longest match, because `$mod` and `$modshift` can both be defined and the
/// second must not read as the first with `shift` on the end. And a variable
/// defined as itself, the shape a cycle would take
pub(super) fn test_conf_variables(t: &mut Runner) {
    const NAME: &str = "conf: variables expand, longest name wins, and a cycle cannot be written";
    let mut r = Rec::new();
    conf::parse("\
$mod = SUPER
$modshift = SUPER SHIFT
$a = $a
bind = $mod, Q, killactive
bind = $modshift, Q, killactive
plain = $a
missing = $nothing
", &mut r);

    let first = r.lists.first().map(|l| l.2.clone());
    let second = r.lists.get(1).map(|l| l.2.clone());
    // `$a = $a` is refused: a value may not contain a variable, since variables
    // are collected from every file before any line is read. `a` therefore does
    // not exist, and using it later says so and leaves the text alone. A cycle
    // cannot be written
    let plain = r.value("plain").map(|s| s.to_string());
    let unknown = r.noted(Note::UnknownVar.as_str());
    let refused = r.noted(Note::VarInVar.as_str());

    t.check(NAME,
            first.as_deref() == Some("SUPER, Q, killactive")
                && second.as_deref() == Some("SUPER SHIFT, Q, killactive")
                && plain.as_deref() == Some("$a")
                && unknown && refused,
            format!("first {:?}, second {:?}, the self-defined one {:?}, \
                     an undefined name was noted {}, a variable inside a \
                     variable was refused {}",
                    first, second, plain, unknown, refused));
}

/// A line that outgrows its buffer is refused, not cut short
///
/// Half a command line is a different command line, and running it is worse
/// than not running it. `Text::from` follows the same rule
pub(super) fn test_conf_line_too_long(t: &mut Runner) {
    const NAME: &str = "conf: a line that outgrows its buffer is refused rather than cut short";
    let mut r = Rec::new();
    // Sixty-four bytes, expanded twenty times over: past the line buffer
    conf::parse("\
$big = 0123456789012345678901234567890123456789012345678901234567890123
exec = $big $big $big $big $big $big $big $big $big $big $big $big $big $big $big $big $big $big $big $big
short = $big
", &mut r);

    let refused = r.noted(Note::TooLong.as_str());
    // And the ordinary one beside it still went through whole
    let short = r.value("short").map(|s| s.len());
    // Nothing was delivered for the long line, not even a truncated version
    let no_exec = r.value("exec").is_none() && !r.lists.iter().any(|l| l.1 == "exec");

    t.check(NAME, refused && short == Some(64) && no_exec,
            format!("refused {}, the short one came out {:?} bytes, \
                     nothing was delivered for the long one {}",
                    refused, short, no_exec));
}

/// `source` has a floor, and a cycle finds it
///
/// A depth limit and not a set of files already seen: two files both sourcing a
/// third is a valid config
pub(super) fn test_conf_source_depth(t: &mut Runner) {
    const NAME: &str = "conf: source has a depth limit, so a cycle stops rather than spins";
    let mut r = Rec::new();
    r.files.push(("loop.conf", "seen = 1\nsource = loop.conf\n"));
    r.files.push(("gone.conf", ""));
    conf::parse("source = loop.conf\nsource = nowhere.conf\n", &mut r);

    let times = r.keys.iter().filter(|k| k.3 == "seen").count();
    let deep = r.noted(Note::TooDeep.as_str());
    let missing = r.noted(Note::NoSuchFile.as_str());

    t.check(NAME, times == 3 && deep && missing,
            format!("the cycle was read {} times before it stopped, \
                     depth was reported {}, a missing file was reported {}",
                    times, deep, missing));
}

/// `bind`, `bindm`, `bindel`, and something that only looks like one
pub(super) fn test_conf_bind_key(t: &mut Runner) {
    const NAME: &str = "conf: the letters after bind become flags, and the rest are named";
    use protocol::wm::chord;

    let plain = conf::bind_key("bind").map(|b| (b.flags, b.n_unhandled));
    // `m` is accepted and means nothing: a mouse binding is an ordinary
    // binding whose key is a button, so there is no flag for it to set
    let mouse = conf::bind_key("bindm").map(|b| (b.flags, b.n_unhandled));
    let repeat = conf::bind_key("binde").map(|b| (b.flags, b.n_unhandled));
    let two = conf::bind_key("bindrn").map(|b| (b.flags, b.n_unhandled));
    let odd = conf::bind_key("bindz").map(|b| (b.flags, b.n_unhandled));
    // Not a binding at all, and must not be read as one
    let not = conf::bind_key("binding_style_and_then_some");
    let unrelated = conf::bind_key("gaps_in");

    t.check(NAME,
            plain == Some((0, 0))
                && mouse == Some((0, 0))
                && repeat == Some((chord::REPEAT, 0))
                && two == Some((chord::RELEASE | chord::PASSTHROUGH, 0))
                && odd == Some((0, 1))
                && not.is_none() && unrelated.is_none(),
            format!("bind {:?}, bindm {:?}, binde {:?}, bindrn {:?}, \
                     bindz {:?}, a long word starting with bind is a bind {}",
                    plain, mouse, repeat, two, odd, not.is_some()));
}

// The layers: which files a config is made of

/// A filesystem made of strings
///
/// `Manifest` takes a trait because the layering (order, masking, an empty
/// file, a `source =` across layers) is decided by which bytes come back from
/// which path. The kernel links `libmay`, so the arrangement two ring-3
/// programs depend on is checked at boot, with no filesystem, client or screen
struct FakeFs {
    dirs: Vec<(&'static str, Vec<&'static str>)>,
    files: Vec<(&'static str, &'static str)>,
}

impl manifest::Files for FakeFs {
    fn read(&mut self, path: &str) -> Read {
        match self.files.iter().find(|(p, _)| *p == path) {
            // Present and empty, which is how a drop-in switches a shipped file
            // off. A real filesystem cannot tell this from a missing file
            // without asking for the size separately
            Some((_, t)) if t.is_empty() => Read::Empty,
            Some((_, t)) => Read::Text(t),
            None => Read::Missing,
        }
    }

    fn list(&mut self, dir: &str, out: &mut [Text]) -> usize {
        let Some((_, names)) = self.dirs.iter().find(|(d, _)| *d == dir) else {
            return 0;
        };
        let mut n = 0;
        for name in names {
            if n == out.len() {
                break;
            }
            out[n] = Text::from(name);
            n += 1;
        }
        n
    }
}

/// The layers, in order, with one shipped file masked and one variable
/// redefined from the file read last
///
/// Four properties of one arrangement:
///
///   - order: shipped first, then the machine's own file, then its
///     drop-ins, and within a directory by name
///   - masking: a drop-in with the same name replaces the shipped file
///     rather than adding to it, so an empty drop-in switches one off
///     (`Read::Empty`)
///   - variables are the config's, not the file's: `$mod` is redefined in
///     the drop-in read last and still reaches the shipped bindings read
///     first, because every definition is collected before any line is
///     dispatched
///   - `source` reads in place: the file it names is walked at the line
///     that asked for it, so "the later line wins" holds across an include
pub(super) fn test_conf_layers(t: &mut Runner) {
    const NAME: &str = "conf: layers read in order, a drop-in masks a shipped file, \
                        and a variable reaches backwards";
    let mut fs = FakeFs {
        dirs: alloc::vec![
            (manifest::DEFAULT_D, alloc::vec![
                // Out of order on purpose: names decide, not the directory
                "50-keys.conf", "10-theme.conf", "30-keys.conf",
                // Not a config: an editor's leftovers stay out
                "notes.txt",
            ]),
            (manifest::CONF_D, alloc::vec!["99-mine.conf", "50-keys.conf"]),
        ],
        files: alloc::vec![
            ("/etc/mayland/default.d/10-theme.conf", "theme {\n  accent = shipped\n}\n"),
            ("/etc/mayland/default.d/30-keys.conf", "bind = $mod, T, killactive\n"),
            // Masked by the empty one in conf.d, so this never runs
            ("/etc/mayland/default.d/50-keys.conf", "bind = , Z, killactive\n"),
            ("/etc/mayland/default.d/notes.txt", "not a config at all\n"),
            (manifest::MAIN, "source = /etc/mayland/extra.conf\nafter = 1\n"),
            ("/etc/mayland/extra.conf", "sourced = 1\n"),
            ("/etc/mayland/conf.d/50-keys.conf", ""),
            ("/etc/mayland/conf.d/99-mine.conf", "$mod = ALT\n"),
        ],
    };

    let m = Manifest::build(&mut fs, &manifest::LAYERS);
    let order: Vec<String> = m.all().map(|i| m.path(i).to_string()).collect();
    let roots = m.roots().len();

    let mut r = Rec::new();
    for i in m.all() {
        r.src.push((m.path(i).to_string(), m.text(i), i));
    }
    let mut vars = conf::Vars::new();
    for i in m.all() {
        conf::collect_vars(m.text(i), &mut vars, i, &mut r);
    }
    let mut roots_of = [0u8; manifest::MAX_FILES];
    roots_of[..roots].copy_from_slice(m.roots());
    for k in 0..roots {
        let f = roots_of[k];
        conf::parse_into(m.text(f), &mut r, &vars, f);
    }

    let expected = [
        "/etc/mayland/default.d/10-theme.conf",
        "/etc/mayland/default.d/30-keys.conf",
        "/etc/mayland/may.conf",
        "/etc/mayland/extra.conf",
        "/etc/mayland/conf.d/50-keys.conf",
        "/etc/mayland/conf.d/99-mine.conf",
    ];
    let ordered = order.len() == expected.len()
        && order.iter().zip(expected).all(|(a, b)| a == b);
    // The sourced file is not a layer file: it is walked by the recursion at
    // the `source =` line, so five of the six are roots
    let root_count = roots == 5;
    // The shipped binding, with the modifier the drop-in read last chose
    let bound = r.lists.iter().find(|l| l.1 == "bind").map(|l| l.2.clone());
    // Nothing from the masked file
    let masked = !r.lists.iter().any(|l| l.2.contains(", Z,"));
    // The sourced line came out, and it knows which file it was in
    let sourced = r.keys.iter().find(|k| k.3 == "sourced").map(|k| k.1);
    // And the line after the `source =` came after it, in its own file
    let after = r.keys.iter().find(|k| k.3 == "after").map(|k| k.1);
    let in_order = r.keys.iter().position(|k| k.3 == "sourced")
        < r.keys.iter().position(|k| k.3 == "after");
    // A file that is not a config was never opened
    let ignored = !order.iter().any(|p| p.ends_with("notes.txt"));

    t.check(NAME,
            ordered && root_count && bound.as_deref() == Some("ALT, T, killactive")
                && masked && sourced == Some(3) && after == Some(2) && in_order
                && ignored,
            format!("order {:?}, roots {}, the shipped binding came out {:?}, \
                     the masked file contributed nothing {}, the sourced key is \
                     in file {:?} and the line after it in file {:?}, in that \
                     order {}, notes.txt ignored {}",
                    order, roots, bound, masked, sourced, after, in_order, ignored));
}

/// More files than the manifest holds are dropped, and it says so
///
/// A half-read config must not go unnoticed, so the message says the rest are
/// unread
pub(super) fn test_conf_too_many_files(t: &mut Runner) {
    const NAME: &str = "conf: more files than there is room for are dropped out loud";
    // Sixteen in each directory is the per-directory ceiling, and thirty-two
    // is past the manifest's twenty-four
    static A: [&str; 16] = [
        "a00.conf", "a01.conf", "a02.conf", "a03.conf", "a04.conf", "a05.conf",
        "a06.conf", "a07.conf", "a08.conf", "a09.conf", "a10.conf", "a11.conf",
        "a12.conf", "a13.conf", "a14.conf", "a15.conf",
    ];
    static B: [&str; 16] = [
        "b00.conf", "b01.conf", "b02.conf", "b03.conf", "b04.conf", "b05.conf",
        "b06.conf", "b07.conf", "b08.conf", "b09.conf", "b10.conf", "b11.conf",
        "b12.conf", "b13.conf", "b14.conf", "b15.conf",
    ];
    let mut files: Vec<(&'static str, &'static str)> = Vec::new();
    let mut leak = |dir: &str, name: &'static str| -> &'static str {
        let path = format!("{}/{}", dir, name);
        alloc::boxed::Box::leak(path.into_boxed_str())
    };
    for n in A.iter() {
        let p = leak(manifest::DEFAULT_D, n);
        files.push((p, "x = 1\n"));
    }
    for n in B.iter() {
        let p = leak(manifest::CONF_D, n);
        files.push((p, "x = 1\n"));
    }
    let mut fs = FakeFs {
        dirs: alloc::vec![
            (manifest::DEFAULT_D, A.to_vec()),
            (manifest::CONF_D, B.to_vec()),
        ],
        files,
    };

    let m = Manifest::build(&mut fs, &manifest::LAYERS);
    let held = m.len();
    let complained = matches!(m.complaint(), Some((Note::TooManyFiles, _)));

    t.check(NAME,
            held == manifest::MAX_FILES && complained,
            format!("held {} of 32 offered, and said so {}", held, complained));
}

// The one image format the machine decodes

/// Every chunk QOI has, in nine bytes
///
/// Written by hand so the case is about the format and not a file on disk. Five
/// pixels use all five ways QOI encodes a pixel: written out, a small
/// difference, a run, an earlier pixel by index, and a difference relative to
/// green.
///
/// The stream and its pixels were checked against an independent decoder
pub(super) fn test_qoi_decode(t: &mut Runner) {
    const NAME: &str = "qoi: every chunk kind decodes, and a file that is not one is refused";
    #[rustfmt::skip]
    const STREAM: &[u8] = &[
        b'q', b'o', b'i', b'f',
        0, 0, 0, 5,          // width
        0, 0, 0, 1,          // height
        4, 0,                // RGBA, sRGB
        0xFE, 10, 20, 30,    // RGB:   (10, 20, 30)
        0x79,                // DIFF:  +1, 0, -1
        0xC0,                // RUN:   one more of the same
        0x09,                // INDEX: back to the first pixel
        0xA5, 0xA5,          // LUMA:  +7, +5, +2
        0, 0, 0, 0, 0, 0, 0, 1,
    ];
    // BGRA, which is what a framebuffer holds and what this writes
    const WANT: &[u8] = &[
        30, 20, 10, 255,
        29, 20, 11, 255,
        29, 20, 11, 255,
        30, 20, 10, 255,
        32, 25, 17, 255,
    ];

    let size = libmay::qoi::size(STREAM);
    let mut out = [0u8; 5 * 4];
    let rows = libmay::qoi::decode(STREAM, &mut out);
    let pixels = out == WANT;

    // A file that is not a QOI must be refused, not turned into noise: the
    // wallpaper is a path in a config, and a mistyped one should cost one
    // console line
    let not_one = libmay::qoi::size(b"\x89PNG\r\n\x1a\n and so on, for a while").is_none();
    let truncated = libmay::qoi::size(b"qoif\0\0").is_none();

    t.check(NAME,
            size == Some((5, 1)) && rows == 1 && pixels && not_one && truncated,
            format!("header {:?}, {} row decoded, pixels {:?}, a PNG is refused {}, \
                     a short header is refused {}",
                    size, rows, &out[..], not_one, truncated));
}

/// Sensitivity below one slows the cursor rather than stopping it
///
/// A mouse moved slowly reports one pixel at a time, and a factor of a half
/// applied to ones is zero every time if the fraction is dropped. The remainder
/// is carried, and dropped when the factor changes, or the cursor spends the
/// old factor's change under the new one
pub(super) fn test_input_scaling(t: &mut Runner) {
    use crate::mayland::input::conf;
    use crate::mayland::protocol::wm::param;
    const NAME: &str = "input: sensitivity slows the pointer without stopping it, and the wheel turns both ways";

    // Untouched, movement passes through exactly
    let plain = conf::scale(3, -4);

    // Hyprland writes -1..1 with 0 meaning "as the device sends it", so a
    // half is -0.5
    conf::set(param::SENSITIVITY, -0.5);
    let slow: i32 = (0..8).map(|_| conf::scale(1, 0).0).sum();
    // Twice as fast
    conf::set(param::SENSITIVITY, 1.0);
    let fast = conf::scale(10, 0).0;

    conf::set(param::NATURAL_SCROLL, 1.0);
    let inverted = conf::wheel(1);
    conf::set(param::NATURAL_SCROLL, 0.0);
    let plain_wheel = conf::wheel(1);

    // Puts the desktop back as it was
    conf::set(param::SENSITIVITY, 0.0);
    let restored = conf::scale(7, 9);

    t.check(NAME,
            plain == (3, -4) && slow == 4 && fast == 20
                && inverted == -1 && plain_wheel == 1 && restored == (7, 9),
            format!("untouched {:?}, eight one-pixel moves at half speed came                      to {}, ten at double came to {}, wheel inverted {} and                      plain {}, back to untouched {:?}",
                    plain, slow, fast, inverted, plain_wheel, restored));
}

// The chords themselves, through the interrupt's own entry point

/// Set-1 make codes. The digit row starts at 2, so `2` is 3
const MAKE_2: u8 = 0x03;
const MAKE_3: u8 = 0x04;
const MAKE_LOGO: u8 = 0x5B; // extended
const KEY_2: u32 = 3;
const KEY_3: u32 = 4;

/// Hold the Logo key down, or let it up, the way a keyboard does
fn logo(down: bool) {
    input::intercept(0xE0);
    input::intercept(if down { MAKE_LOGO } else { MAKE_LOGO | 0x80 });
}

/// Every chord the interrupt has queued, delivered and read back
fn fired(conn: &mut crate::mayland::client::ClientConn, client: u32) -> Vec<u32> {
    input::drain(0);
    exchange(conn, client).iter().filter_map(|e| match e {
        Event::WmChord { id } => Some(*id), _ => None,
    }).collect()
}

/// A held key is a stream of make codes, and only `binde` asks for a stream
///
/// A chord is matched on the make code, and typematic in the keyboard produces
/// make codes for as long as a key is held, so holding the close-window chord
/// must not close a stream of windows
pub(super) fn test_chord_repeat(t: &mut Runner) {
    const NAME: &str = "chord: plain bind fires once while held, binde fires each time";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl, mut conn, _desk, wm, _, _)) = super::wm::wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    use protocol::wm::chord;

    conn.wm_bind_chord(wm, 1, input::MOD_LOGO, KEY_2, 0);
    conn.wm_bind_chord(wm, 2, input::MOD_LOGO, KEY_3, chord::REPEAT);
    let _ = exchange(&mut conn, cl);

    logo(true);
    // Three make codes, as typematic sends them, then the release
    for _ in 0..3 { input::intercept(MAKE_2); }
    input::intercept(MAKE_2 | 0x80);
    let once = fired(&mut conn, cl);

    for _ in 0..3 { input::intercept(MAKE_3); }
    input::intercept(MAKE_3 | 0x80);
    let many = fired(&mut conn, cl);

    // And it can be pressed a second time
    input::intercept(MAKE_2);
    input::intercept(MAKE_2 | 0x80);
    let again = fired(&mut conn, cl);
    logo(false);
    server::disconnect(cl);

    t.check(NAME,
            once == [1] && many == [2, 2, 2] && again == [1],
            format!("held plain bind fired {:?}, held binde fired {:?}, \
                     pressed again {:?}", once, many, again));
}

/// `bindr` fires on the way up, even when the modifier went up first
///
/// A release is matched on the key alone because Super often goes up before Q.
/// Matched the ordinary way it would not find its chord, which would stay
/// marked as held for ever
pub(super) fn test_chord_release(t: &mut Runner) {
    const NAME: &str = "chord: bindr fires on release, and a released modifier does not strand it";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl, mut conn, _desk, wm, _, _)) = super::wm::wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    use protocol::wm::chord;

    conn.wm_bind_chord(wm, 3, input::MOD_LOGO, KEY_2, chord::RELEASE);
    conn.wm_bind_chord(wm, 4, input::MOD_LOGO, KEY_3, 0);
    let _ = exchange(&mut conn, cl);

    logo(true);
    input::intercept(MAKE_2);
    let on_press = fired(&mut conn, cl);
    input::intercept(MAKE_2 | 0x80);
    let on_release = fired(&mut conn, cl);

    // Key down, modifier up, key up
    input::intercept(MAKE_3);
    let plain_pressed = fired(&mut conn, cl);
    logo(false);
    input::intercept(MAKE_3 | 0x80);
    let _ = fired(&mut conn, cl);
    // Without the release recognised the chord stays marked held, and this
    // second press would be swallowed without firing
    logo(true);
    input::intercept(MAKE_3);
    let second_press = fired(&mut conn, cl);
    input::intercept(MAKE_3 | 0x80);
    logo(false);
    server::disconnect(cl);

    t.check(NAME,
            on_press.is_empty() && on_release == [3]
                && plain_pressed == [4] && second_press == [4],
            format!("bindr on press {:?}, on release {:?}; the plain one \
                     fired {:?} and after a modifier-first release {:?}",
                    on_press, on_release, plain_pressed, second_press));
}

/// `bindr` on a modifier key is a tap of it alone: Super pressed and released
/// fires; Super held through Super+2 or a click does not; Super's own typematic
/// does not count as something else pressed
pub(super) fn test_chord_lone_modifier(t: &mut Runner) {
    const NAME: &str = "chord: bindr on Super fires for Super alone, not after Super+2 or a click";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl, mut conn, _desk, wm, _, _)) = super::wm::wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    use protocol::wm::chord;
    const KEY_LEFTMETA: u32 = 125;

    conn.wm_bind_chord(wm, 5, input::MOD_LOGO, KEY_LEFTMETA, chord::RELEASE | chord::PASSTHROUGH);
    conn.wm_bind_chord(wm, 6, input::MOD_LOGO, KEY_2, 0);
    let _ = exchange(&mut conn, cl);

    // Alone, held long enough for the keyboard to repeat it
    logo(true);
    logo(true);
    logo(false);
    let alone = fired(&mut conn, cl);
    // Through a chord
    logo(true);
    input::intercept(MAKE_2);
    input::intercept(MAKE_2 | 0x80);
    logo(false);
    let through = fired(&mut conn, cl);
    // Through a click
    logo(true);
    input::spoil_held(0);
    logo(false);
    let clicked = fired(&mut conn, cl);
    // Alone again: the spoiling ends with the press it spoilt
    logo(true);
    logo(false);
    let again = fired(&mut conn, cl);
    server::disconnect(cl);

    t.check(NAME,
            alone == [5] && through == [6] && clicked.is_empty() && again == [5],
            format!("alone {alone:?}, through Super+2 {through:?}, through a click {clicked:?}, alone again {again:?}"));
}

/// `bindn` does not take the key away from whoever would have had it
pub(super) fn test_chord_passthrough(t: &mut Runner) {
    const NAME: &str = "chord: bindn fires and still lets the key through";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl, mut conn, _desk, wm, _, _)) = super::wm::wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    use protocol::wm::chord;

    conn.wm_bind_chord(wm, 7, input::MOD_LOGO, KEY_2, 0);
    conn.wm_bind_chord(wm, 8, input::MOD_LOGO, KEY_3, chord::PASSTHROUGH);
    let _ = exchange(&mut conn, cl);

    logo(true);
    // `intercept` answers whether the key was consumed, which is the difference
    // the flag makes
    let swallowed = input::intercept(MAKE_2);
    input::intercept(MAKE_2 | 0x80);
    let through = !input::intercept(MAKE_3);
    input::intercept(MAKE_3 | 0x80);
    let both = fired(&mut conn, cl);
    logo(false);
    server::disconnect(cl);

    t.check(NAME, swallowed && through && both == [7, 8],
            format!("the plain one was swallowed {}, the bindn one went \
                     through {}, and both fired {:?}", swallowed, through, both));
}
