// Config grammar
//
// `hyprland.conf` is nested blocks of `key = value`, plus list lines such as
// `bind`, `windowrule`, `modules-left`.
//
//     $mod = SUPER
//     general {
//         gaps_in = 12
//         layout = dwindle
//         blur { size = 3 }
//     }
//     source = /etc/mayland/keys.conf
//     bind  = $mod, Q, killactive
//     binde = $mod, L, resizeactive, 10 0
//
// The parser walks the blocks and hands `(where, section, key, value)` to a
// `Sink`, which decides what a key means. The two readers are the window
// manager, turning keys into param numbers and binds, and the panel, turning
// them into module lists.
//
// No allocator: fixed arrays and a section path two deep.
//
// Every call carries a `Where`. Only the reader knows if a line means anything,
// so the grammar says where and the reader complains.
//
// Handled by the grammar itself:
//
//     $name = value   a variable, collected by `collect_vars` from every file before any line is dispatched,
//                     so it may be defined in a file read after its use
//     source = path   another file, read in place. The reader supplies the bytes through `Sink` (see `super::manifest`)
//     bind[flags]     which spellings of `bind` are list lines
//
// Absent: per-monitor lines and `env`, which have no destination here. The
// reader names both instead of dropping them.

#![allow(dead_code)]

use crate::protocol::wm::chord;

/// Longest string carried out of a file: a rule's match, a command, or a
/// module's name. `exec = <path>` goes through here, so it covers every path on
/// this filesystem
pub const MAX_TEXT: usize = 64;

/// A short string with nowhere to allocate
#[derive(Clone, Copy)]
pub struct Text {
    pub bytes: [u8; MAX_TEXT],
    pub len: usize,
}

impl Text {
    pub const EMPTY: Text = Text { bytes: [0; MAX_TEXT], len: 0 };

    pub fn from(s: &str) -> Text {
        // All of it when it fits, otherwise as many whole characters as fit. A
        // cut inside a multi-byte character would make `as_str` return ""
        let b = s.as_bytes();
        let mut len = b.len().min(MAX_TEXT);
        while len > 0 && !s.is_char_boundary(len) {
            len -= 1;
        }
        let mut bytes = [0u8; MAX_TEXT];
        bytes[..len].copy_from_slice(&b[..len]);
        Text { bytes, len }
    }

    /// Whether a string of this length arrived whole. `exec` asks: a truncated
    /// command line is a different command
    pub fn fits(s: &str) -> bool {
        s.len() <= MAX_TEXT
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Whether `hay` contains this, for a window rule. `.*` matches every
    /// window, as Hyprland configs write it (`no_blur` on `class .*`); anything
    /// else is a plain substring
    pub fn matches(&self, hay: &str) -> bool {
        let needle = self.as_str();
        if needle.is_empty() {
            return false;
        }
        if needle == ".*" {
            return true;
        }
        let (n, h) = (needle.as_bytes(), hay.as_bytes());
        if n.len() > h.len() {
            return false;
        }
        (0..=h.len() - n.len()).any(|i| &h[i..i + n.len()] == n)
    }
}

/// Which line of which file a call is about. `file` indexes the `Manifest`, the
/// ordered list of the config's files, and complaints print it:
/// `default.d/50-keys.conf:12`. An index keeps `Where` eight bytes and `Copy`
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Where {
    pub line: u32,
    pub file: u8,
}

impl Where {
    pub const NOWHERE: Where = Where { line: 0, file: 0 };

    pub const fn in_file(file: u8, line: u32) -> Where {
        Where { line, file }
    }
}

/// Something the grammar could not carry: the four ways a line is wrong before
/// anyone decides its meaning
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// `$name` used with nothing of that name defined above it
    UnknownVar,
    /// The line exceeded the buffer after variable expansion
    TooLong,
    /// More variables than there is room for
    TooManyVars,
    /// `source =` nested deeper than the limit allows
    TooDeep,
    /// `source =` named a file the reader could not produce
    NoSuchFile,
    /// More files than the manifest can hold
    TooManyFiles,
    /// A path too long to carry whole
    PathTooLong,
    /// `$a = $b`: a variable defined by another. Refused, since variables are
    /// config-global with no order, so forbidding `$` in a value makes a cycle
    /// unwritable
    VarInVar,
}

impl Note {
    /// What to print, here so all readers of a file say the same thing
    pub fn as_str(self) -> &'static str {
        match self {
            Note::UnknownVar  => "no variable of that name has been set yet",
            Note::TooLong     => "this line is too long once its variables are put in",
            Note::TooManyVars => "too many variables - the rest are ignored",
            Note::TooDeep     => "source: nested too deep",
            Note::NoSuchFile  => "source: nothing could be read from there",
            Note::VarInVar    => "a variable's value may not contain another variable",
            Note::TooManyFiles => "more config files than there is room for - the rest are unread",
            Note::PathTooLong  => "that path is too long to carry whole",
        }
    }
}

/// What a reader of the file answers. A setting belongs to a section and is
/// asked about by name; a list line is handed over whole
pub trait Sink {
    /// `section` is the block path joined by dots: `""`, `"general"`,
    /// `"decoration.blur"`
    fn key(&mut self, at: Where, section: &str, key: &str, value: &str);

    /// A line whose key is a kind, not a name: `bind`, `windowrule`,
    /// `modules-left`. Ignored by default
    fn list(&mut self, at: Where, key: &str, value: &str) {
        let _ = (at, key, value);
    }

    /// Whether this key is a list instead of a setting. Asked before `key`
    fn is_list(&self, key: &str) -> bool {
        let _ = key;
        false
    }

    /// The file a `source =` line asked for: its bytes and its number.
    ///
    /// `'static` because a reader with no allocator keeps its mapping. The
    /// number is the second half of `Where::file`, assigned by the reader
    /// holding the `Manifest`. `Manifest` already read the file, so this is a
    /// lookup, which also keeps a diamond (two files sourcing a third) from
    /// reading it twice
    fn source(&mut self, at: Where, path: &str) -> Option<(&'static str, u8)> {
        let _ = (at, path);
        None
    }

    /// A line the grammar could not carry. Silent by default
    fn note(&mut self, at: Where, note: Note, text: &str) {
        let _ = (at, note, text);
    }
}

// Variables

/// How many `$name = value` lines a config may have
pub const MAX_VARS: usize = 32;

/// Longest line after variable expansion. A line with no `$` is passed through
/// and not measured, so variables cannot shorten what a config could already
/// say
const LINE_BUF: usize = 256;

/// Deepest `source =` nesting. A depth limit instead of a seen-set, since a
/// diamond is a valid config; a cycle hits the same limit
const MAX_DEPTH: u8 = 4;

/// Every `$name` the config defines, from every file. Held by the loader, since
/// a variable belongs to the config, see `collect_vars`
#[derive(Clone, Copy)]
pub struct Vars {
    names: [Text; MAX_VARS],
    vals: [Text; MAX_VARS],
    n: usize,
}

impl Vars {
    pub const fn new() -> Vars {
        Vars { names: [Text::EMPTY; MAX_VARS], vals: [Text::EMPTY; MAX_VARS], n: 0 }
    }

    /// Defines or redefines one. As in Hyprland, the later line wins
    fn set(&mut self, name: &str, value: &str) -> bool {
        if let Some(i) = (0..self.n).find(|&i| self.names[i].as_str() == name) {
            self.vals[i] = Text::from(value);
            return true;
        }
        if self.n == MAX_VARS {
            return false;
        }
        self.names[self.n] = Text::from(name);
        self.vals[self.n] = Text::from(value);
        self.n += 1;
        true
    }

    /// Longest defined name that `s` starts with, so `$modshift` does not read
    /// as `$mod` followed by `shift`
    fn longest_match(&self, s: &str) -> Option<(usize, &str)> {
        let mut best: Option<(usize, &str)> = None;
        for i in 0..self.n {
            let name = self.names[i].as_str();
            if name.is_empty() || !s.starts_with(name) {
                continue;
            }
            if best.map_or(true, |(n, _)| name.len() > n) {
                best = Some((name.len(), self.vals[i].as_str()));
            }
        }
        best
    }
}

/// Whether a byte may appear in a variable's name
fn name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Replaces every `$name` in `line`, in one left-to-right pass over the whole
/// config's table. One pass suffices since a value may not contain a `$`.
///
/// A line with no `$` is returned uncopied
fn expand<'a>(line: &'a str, vars: &Vars, buf: &'a mut [u8; LINE_BUF])
    -> Result<(&'a str, Option<Note>), Note>
{
    if !line.as_bytes().contains(&b'$') {
        return Ok((line, None));
    }
    let mut n = 0usize;
    let mut unknown = None;
    let b = line.as_bytes();
    let mut i = 0usize;

    // Copy `bytes` into the buffer, or say the line has outgrown it
    macro_rules! put {
        ($src:expr) => {{
            let s: &[u8] = $src;
            if n + s.len() > LINE_BUF {
                return Err(Note::TooLong);
            }
            buf[n..n + s.len()].copy_from_slice(s);
            n += s.len();
        }};
    }

    while i < b.len() {
        if b[i] != b'$' {
            put!(&b[i..i + 1]);
            i += 1;
            continue;
        }
        let rest = &line[i + 1..];
        match vars.longest_match(rest) {
            Some((len, value)) => {
                put!(value.as_bytes());
                i += 1 + len;
            }
            None => {
                // Left as written; a stray `$` in a value may not be a variable
                if rest.as_bytes().first().is_some_and(|&c| name_byte(c)) {
                    unknown = Some(Note::UnknownVar);
                }
                put!(&b[i..i + 1]);
                i += 1;
            }
        }
    }
    let s = core::str::from_utf8(&buf[..n]).map_err(|_| Note::TooLong)?;
    Ok((s, unknown))
}

// bind and its suffix letters

/// The flags on a `bind` line, and letters with no effect. `bindel = ...` is
/// `bind` plus `e` plus `l`. `is_list` needs the lexer before reading the
/// value, so it is here; each bit's meaning is in the protocol
#[derive(Clone, Copy)]
pub struct BindKey {
    pub flags: u32,
    /// Letters this system does nothing with, kept so the reader can say so
    pub unhandled: [u8; 8],
    pub n_unhandled: u8,
}

impl BindKey {
    pub fn unhandled(&self) -> &[u8] {
        &self.unhandled[..self.n_unhandled as usize]
    }
}

/// `bind`, `bindm`, `binde`, `bindrn`..., or None. More than eight letters
/// after `bind` is another key
pub fn bind_key(key: &str) -> Option<BindKey> {
    let rest = key.strip_prefix("bind")?;
    if rest.len() > 8 {
        return None;
    }
    let mut bk = BindKey { flags: 0, unhandled: [0; 8], n_unhandled: 0 };
    for &c in rest.as_bytes() {
        let bit = match c {
            b'e' => chord::REPEAT,
            b'r' => chord::RELEASE,
            b'n' => chord::PASSTHROUGH,
            b'l' => chord::LOCKED,
            b'i' => chord::IGNORE_MODS,
            // A mouse binding is an ordinary one whose key is a button: evdev
            // numbers buttons above every key, so `mouse:272` needs no flag,
            // table, or id space. The letter is accepted for configs that write
            // it and means nothing, so it is not in `unhandled`
            b'm' => 0,
            _ => {
                if (bk.n_unhandled as usize) < bk.unhandled.len() {
                    bk.unhandled[bk.n_unhandled as usize] = c;
                    bk.n_unhandled += 1;
                }
                0
            }
        };
        bk.flags |= bit;
    }
    Some(bk)
}

// The parser

/// Reads one config file: its variables, then its lines. Both passes
/// (`collect_vars`, `parse_into`) for the one-file case, a built-in default or
/// a test
pub fn parse(text: &str, sink: &mut impl Sink) {
    let mut vars = Vars::new();
    collect_vars(text, &mut vars, 0, sink);
    parse_into(text, sink, &vars, 0);
}

/// First pass: every `$name = value` in one file.
///
/// Variables belong to the config, not the file. A config is read lowest
/// authority first, so collecting definitions before dispatching any line lets
/// `$mod = ALT` work from a drop-in read last. In exchange a value may not
/// contain a variable, see `Note::VarInVar`
pub fn collect_vars<S: Sink>(text: &str, vars: &mut Vars, file: u8, sink: &mut S) {
    for (no, raw) in text.lines().enumerate() {
        let at = Where { line: no as u32 + 1, file };
        let trimmed = strip_comment(raw).trim();
        let Some(rest) = trimmed.strip_prefix('$') else { continue };
        let Some((name, value)) = split_once(rest, '=') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() {
            sink.note(at, Note::UnknownVar, trimmed);
            continue;
        }
        if value.as_bytes().contains(&b'$') {
            sink.note(at, Note::VarInVar, trimmed);
            continue;
        }
        if !vars.set(name, value) {
            sink.note(at, Note::TooManyVars, name);
        }
    }
}

/// Second pass: every line of one file, with the config's variables. `file` is
/// this text's number, placed in every `Where`
pub fn parse_into<S: Sink>(text: &str, sink: &mut S, vars: &Vars, file: u8) {
    walk(text, sink, vars, 0, file);
}

fn walk<S: Sink>(text: &str, sink: &mut S, vars: &Vars, depth: u8, file: u8) {
    let mut path: [Text; 2] = [Text::EMPTY; 2];
    let mut block = 0usize;

    for (no, raw) in text.lines().enumerate() {
        let at = Where { line: no as u32 + 1, file };
        let trimmed = strip_comment(raw).trim();
        if trimmed.is_empty() {
            continue;
        }

        // A definition, already handled in the first pass. Expanding
        // `$modshift = ...` under a `$mod` would half-substitute its name
        if trimmed.starts_with('$') && split_once(trimmed, '=').is_some() {
            continue;
        }

        let mut buf = [0u8; LINE_BUF];
        let line = match expand(trimmed, vars, &mut buf) {
            Ok((s, warn)) => {
                if let Some(w) = warn {
                    sink.note(at, w, trimmed);
                }
                s
            }
            Err(e) => {
                sink.note(at, e, trimmed);
                continue;
            }
        };

        if line == "}" {
            block = block.saturating_sub(1);
            continue;
        }
        if let Some(name) = line.strip_suffix('{') {
            if block < 2 {
                path[block] = Text::from(name.trim());
            }
            block += 1;
            continue;
        }
        let Some((key, value)) = split_once(line, '=') else { continue };
        let (key, value) = (key.trim(), value.trim());

        // Another file, read in place. `Manifest` already read it, so this is a
        // lookup, and the recursion walks it where the line asks, keeping "the
        // later line wins" across a `source`
        if key == "source" {
            if depth + 1 >= MAX_DEPTH {
                sink.note(at, Note::TooDeep, value);
                continue;
            }
            match sink.source(at, value) {
                Some((text, idx)) => walk(text, sink, vars, depth + 1, idx),
                None => sink.note(at, Note::NoSuchFile, value),
            }
            continue;
        }

        if sink.is_list(key) {
            sink.list(at, key, value);
            continue;
        }

        let mut joined = [0u8; 64];
        let section = join(&path, block.min(2), &mut joined);
        sink.key(at, section, key, value);
    }
}

// Small parser helpers

/// Everything before the comment, if any. A `#` begins a comment only at the
/// line's start or after a space, so `col.active_border = #cfbdfe` keeps its
/// value
pub fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') {
            return &line[..i];
        }
        i += 1;
    }
    line
}

pub fn split_once(s: &str, sep: char) -> Option<(&str, &str)> {
    let i = s.find(sep)?;
    Some((&s[..i], &s[i + sep.len_utf8()..]))
}

pub fn join<'a>(path: &[Text; 2], depth: usize, out: &'a mut [u8; 64]) -> &'a str {
    let mut n = 0;
    for i in 0..depth {
        if n > 0 && n < out.len() {
            out[n] = b'.';
            n += 1;
        }
        let s = path[i].as_str().as_bytes();
        let take = s.len().min(out.len() - n);
        out[n..n + take].copy_from_slice(&s[..take]);
        n += take;
    }
    core::str::from_utf8(&out[..n]).unwrap_or("")
}

/// A color in any spelling a config uses: `rgba(cfbdfeee)`, `rgb(cfbdfe)`,
/// `#cfbdfe`, `0xcfbdfe`, as in Hyprland.
///
/// A form that gives no alpha is opaque. Hyprland writes the alpha last,
/// `rgba(RRGGBBAA)`; a `u32` color here carries it first, `0xAARRGGBB`, the
/// framebuffer and `shm` byte order, so the eight-digit form is rotated
pub fn colour(s: &str) -> Option<u32> {
    let s = s.trim();
    let body = match () {
        _ if s.starts_with("rgba(") || s.starts_with("rgb(") => {
            s.split('(').nth(1)?.strip_suffix(')')?
        }
        _ => s,
    };
    let digits = body
        .trim()
        .strip_prefix('#')
        .or_else(|| body.trim().strip_prefix("0x"))
        .unwrap_or(body.trim());
    if digits.len() != 6 && digits.len() != 8 {
        return None;
    }
    let mut v = 0u32;
    for c in digits.bytes() {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        v = (v << 4) | d as u32;
    }
    // Eight digits: the low byte is the alpha and moves to the top. Six: solid
    Some(if digits.len() == 8 { (v >> 8) | (v << 24) } else { v | 0xFF00_0000 })
}

/// A number, a boolean, or None. Booleans return 1 and 0, on their way to a
/// param. `true`, `on`, and `yes` are all accepted
pub fn number(s: &str) -> Option<f32> {
    let s = s.trim();
    match s {
        "true" | "yes" | "on" => return Some(1.0),
        "false" | "no" | "off" => return Some(0.0),
        _ => {}
    }
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty() {
        return None;
    }
    let (int_part, frac_part) = match split_once(digits, '.') {
        Some((a, b)) => (a, b),
        None => (digits, ""),
    };
    let mut v = 0.0f32;
    for c in int_part.bytes() {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10.0 + (c - b'0') as f32;
    }
    let mut scale = 0.1f32;
    for c in frac_part.bytes() {
        if !c.is_ascii_digit() {
            return None;
        }
        v += (c - b'0') as f32 * scale;
        scale *= 0.1;
    }
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.*` is every window, and nothing else reads as a pattern
    #[test]
    fn rule_text_matches() {
        assert!(Text::from(".*").matches("kitty"));
        assert!(Text::from(".*").matches(""));
        assert!(Text::from("term").matches("mayterm"));
        assert!(!Text::from("kitty").matches("mayterm"));
        assert!(!Text::from("may.*").matches("mayterm"));
        assert!(!Text::EMPTY.matches("mayterm"));
    }

    /// The alpha of `rgba()` is the last two digits and lands on top; six
    /// digits are solid
    #[test]
    fn border_colours() {
        assert_eq!(colour("rgba(41484d77)"), Some(0x7741_484D));
        assert_eq!(colour("rgba(181c2033)"), Some(0x3318_1C20));
        assert_eq!(colour("rgb(cfbdfe)"), Some(0xFFCF_BDFE));
    }
}
