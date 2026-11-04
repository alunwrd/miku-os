// Keycodes to characters
//
// Shared between the compositor and clients. The compositor sends an evdev
// keycode, a modifier mask, and a group, plus a table of each key in each
// group, as Wayland sends an xkb keymap. There is no xkb here, so this file
// holds the layouts: US, and each other one as the keys it changes.
//
// Below 0x59, evdev keycodes are the AT set-1 make codes unchanged; see
// `to_keycode` in the compositor's input path.
//
// No allocation or state. A client passes back the mask and group it was last
// sent.

/// Modifier bits, as the `modifiers` event carries them. The first few match
/// libxkbcommon's default keymap
pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_CAPS: u32 = 1 << 1;
pub const MOD_CTRL: u32 = 1 << 2;
pub const MOD_ALT: u32 = 1 << 3;
pub const MOD_LOGO: u32 = 1 << 6;

/// A key that is not a character. A client wanting only letters can use
/// `to_char`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Named {
    Escape,
    Backspace,
    Enter,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Function(u8),
    /// A modifier, a lock, or a key this layout has no name for
    Other,
}

pub const KEY_ESC: u16 = 1;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_ENTER: u16 = 28;
pub const KEY_SPACE: u16 = 57;

impl Named {
    /// This name's number on the wire. Stable, as part of the keymap format
    pub fn code(self) -> u32 {
        match self {
            Named::Other => 0,
            Named::Escape => 1,
            Named::Backspace => 2,
            Named::Enter => 3,
            Named::Tab => 4,
            Named::Up => 5,
            Named::Down => 6,
            Named::Left => 7,
            Named::Right => 8,
            Named::Home => 9,
            Named::End => 10,
            Named::PageUp => 11,
            Named::PageDown => 12,
            Named::Insert => 13,
            Named::Delete => 14,
            Named::Function(n) => 0x100 + n as u32,
        }
    }

    pub fn from_code(v: u32) -> Named {
        match v {
            1 => Named::Escape,
            2 => Named::Backspace,
            3 => Named::Enter,
            4 => Named::Tab,
            5 => Named::Up,
            6 => Named::Down,
            7 => Named::Left,
            8 => Named::Right,
            9 => Named::Home,
            10 => Named::End,
            11 => Named::PageUp,
            12 => Named::PageDown,
            13 => Named::Insert,
            14 => Named::Delete,
            0x100..=0x1ff => Named::Function((v - 0x100) as u8),
            _ => Named::Other,
        }
    }
}

/// What a keycode is, ignoring modifiers
pub fn to_named(keycode: u16) -> Named {
    match keycode {
        KEY_ESC => Named::Escape,
        KEY_BACKSPACE => Named::Backspace,
        KEY_TAB => Named::Tab,
        KEY_ENTER | 96 => Named::Enter,
        59..=68 => Named::Function((keycode - 58) as u8),
        87 => Named::Function(11),
        88 => Named::Function(12),
        102 => Named::Home,
        103 => Named::Up,
        104 => Named::PageUp,
        105 => Named::Left,
        106 => Named::Right,
        107 => Named::End,
        108 => Named::Down,
        109 => Named::PageDown,
        110 => Named::Insert,
        111 => Named::Delete,
        _ => Named::Other,
    }
}

/// Character for a keycode under `mods` in US, if any.
///
/// Ctrl with a letter returns the control character: ctrl+C is 0x03. Enter,
/// tab, and backspace return ASCII, so a text field needs no `to_named`.
/// Anything else returns `None`.
///
/// Group 0 only, which the window manager's binds match in: Super+Q closes a
/// window in either alphabet
pub fn to_char(keycode: u16, mods: u32) -> Option<char> {
    to_char_in(&LAYOUTS[0], keycode, mods)
}

/// Character for a keycode under `mods` in `layout`. Ctrl reads US whatever the
/// layout, so Ctrl+С is Ctrl+C
pub fn to_char_in(layout: &Layout, keycode: u16, mods: u32) -> Option<char> {
    let layout = if mods & MOD_CTRL != 0 {
        if let Some(l) = letter(keycode) {
            // ctrl+a is 1, ctrl+z is 26. Alt is untouched, so a client sees the
            // letter and the mask
            return Some(((l as u8 - b'a') + 1) as char);
        }
        &LAYOUTS[0]
    } else {
        layout
    };
    let (plain, shifted, is_letter) = key_in(layout, keycode)?;
    let shift = mods & MOD_SHIFT != 0;
    // Caps and shift each flip a letter's case, so both give lower. Caps means
    // nothing to the rest
    let upper = if is_letter { shift ^ (mods & MOD_CAPS != 0) } else { shift };
    Some(if upper { shifted } else { plain })
}

/// A key in a layout: unshifted, shifted, and whether it is a letter (caps lock
/// applies)
fn key_in(layout: &Layout, keycode: u16) -> Option<(char, char, bool)> {
    if let Some(&(_, plain, shifted)) = layout.keys.iter().find(|k| k.0 == keycode) {
        return Some((plain, shifted, plain.is_alphabetic()));
    }
    if let Some(c) = letter(keycode) {
        return Some((c, c.to_ascii_uppercase(), true));
    }
    let (plain, shifted) = symbol(keycode)?;
    Some((plain, shifted, false))
}

// The layouts

/// One keyboard layout: its name and how it differs from US, as replacements
/// over the US tables below
pub struct Layout {
    /// As a config and Hyprland write it: xkb's name
    pub code: &'static str,
    /// What a bar spells out: xkb's description
    pub name: &'static str,
    /// `(evdev keycode, unshifted, shifted)` for every key this layout changes
    pub keys: &'static [(u16, char, char)],
}

/// Every layout, US first. The order is the index of a config's `kb_layout`, so
/// a layout is only added at the end
pub static LAYOUTS: &[Layout] = &[
    Layout { code: "us", name: "English (US)", keys: &[] },
    Layout { code: "ru", name: "Russian", keys: RU },
];

/// A layout by the name a config gives it
pub fn layout(code: &str) -> Option<usize> {
    LAYOUTS.iter().position(|l| l.code == code)
}

/// ЙЦУКЕН, as xkb's `ru(winkeys)`, which is plain `ru`.
///
/// Where winkeys differs from US: ё is on the key left of 1, not the backslash
/// key; Shift on the digit row gives `" № ; % : ? *`; the slash key is `.` and
/// `,`, and backslash is `\` and `/`. The keypad's decimal key is a comma, as
/// xkb's `kpdl(comma)`
const RU: &[(u16, char, char)] = &[
    (41, 'ё', 'Ё'),
    (3, '2', '"'), (4, '3', '№'), (5, '4', ';'), (6, '5', '%'),
    (7, '6', ':'), (8, '7', '?'), (9, '8', '*'),
    (16, 'й', 'Й'), (17, 'ц', 'Ц'), (18, 'у', 'У'), (19, 'к', 'К'),
    (20, 'е', 'Е'), (21, 'н', 'Н'), (22, 'г', 'Г'), (23, 'ш', 'Ш'),
    (24, 'щ', 'Щ'), (25, 'з', 'З'), (26, 'х', 'Х'), (27, 'ъ', 'Ъ'),
    (30, 'ф', 'Ф'), (31, 'ы', 'Ы'), (32, 'в', 'В'), (33, 'а', 'А'),
    (34, 'п', 'П'), (35, 'р', 'Р'), (36, 'о', 'О'), (37, 'л', 'Л'),
    (38, 'д', 'Д'), (39, 'ж', 'Ж'), (40, 'э', 'Э'),
    (43, '\\', '/'),
    (44, 'я', 'Я'), (45, 'ч', 'Ч'), (46, 'с', 'С'), (47, 'м', 'М'),
    (48, 'и', 'И'), (49, 'т', 'Т'), (50, 'ь', 'Ь'), (51, 'б', 'Б'),
    (52, 'ю', 'Ю'), (53, '.', ','),
    (83, ',', ','),
];

/// The letter rows, in keyboard order
fn letter(keycode: u16) -> Option<char> {
    const TOP: &[u8] = b"qwertyuiop";
    const HOME: &[u8] = b"asdfghjkl";
    const BOTTOM: &[u8] = b"zxcvbnm";
    let c = match keycode {
        16..=25 => TOP[(keycode - 16) as usize],
        30..=38 => HOME[(keycode - 30) as usize],
        44..=50 => BOTTOM[(keycode - 44) as usize],
        _ => return None,
    };
    Some(c as char)
}

/// Everything else that produces a character, unshifted and shifted
fn symbol(keycode: u16) -> Option<(char, char)> {
    // The digit row. The shifted half is spelled out, since it is not derivable
    // on any layout
    const DIGITS: &[(char, char)] = &[
        ('1', '!'), ('2', '@'), ('3', '#'), ('4', '$'), ('5', '%'),
        ('6', '^'), ('7', '&'), ('8', '*'), ('9', '('), ('0', ')'),
    ];
    Some(match keycode {
        2..=11 => DIGITS[(keycode - 2) as usize],
        12 => ('-', '_'),
        13 => ('=', '+'),
        26 => ('[', '{'),
        27 => (']', '}'),
        39 => (';', ':'),
        40 => ('\'', '"'),
        41 => ('`', '~'),
        43 => ('\\', '|'),
        51 => (',', '<'),
        52 => ('.', '>'),
        53 => ('/', '?'),
        KEY_SPACE => (' ', ' '),
        // Not characters in the layout, but a text field wants them as ASCII
        KEY_ENTER | 96 => ('\n', '\n'),
        KEY_TAB => ('\t', '\t'),
        KEY_BACKSPACE => ('\x08', '\x08'),
        KEY_ESC => ('\x1b', '\x1b'),
        // The keypad gives digits regardless of shift; the compositor does not
        // track numlock
        71 => ('7', '7'), 72 => ('8', '8'), 73 => ('9', '9'), 74 => ('-', '-'),
        75 => ('4', '4'), 76 => ('5', '5'), 77 => ('6', '6'), 78 => ('+', '+'),
        79 => ('1', '1'), 80 => ('2', '2'), 81 => ('3', '3'),
        82 => ('0', '0'), 83 => ('.', '.'),
        55 => ('*', '*'),
        98 => ('/', '/'),
        _ => return None,
    })
}

// The keymap on the wire
//
// The compositor serializes the seat's layouts and sends them, as Wayland sends
// an xkb keymap: `may_keyboard.keymap(format, handle, size)`, and the client
// maps the handle. The format field would allow XKB_V1.
//
// Version 2, `KEYMAP_MAY_V2`. A client knowing only version 1 refuses it and
// types US from its compiled-in tables.
//
//      0   u32 magic "MAYK"     4   u32 version, 2
//      8   u32 keycodes, 128   12   u32 entry size, 16
//     16   u32 groups, 1..=4   20   reserved, zero to 32
//     32   groups x 16 bytes   each group's layout code, NUL-padded: "us"
//     ..   groups x 128 x 16   each group's table, complete, one entry per
//                              keycode:
//
//      u32 plain   the character with nothing held, 0 for none
//      u32 shift   the character with shift held
//      u32 named   Named::code(), 0 when the key has no name
//      u32 flags   bit 0: a letter, so caps lock applies to it
//
// Every group is whole, so a client reads an entry with one multiplication.
//
// The pool is always `SIZE_MAX` bytes, room for four groups. A config change
// rewrites it in place and sends nothing: a client reads the header on every
// keystroke

/// "MAYK" as little-endian bytes
pub const MAGIC: u32 = 0x4b59_414d;
pub const VERSION: u32 = 2;
/// Keycodes described, from zero. The AT set-1 make codes stop below 128
pub const COUNT: usize = 128;
pub const ENTRY: usize = 16;
pub const HEADER: usize = 32;
/// Bytes of layout code per group; xkb's names are two or three letters
pub const CODE: usize = 16;
/// Most layouts a seat switches between: xkb's limit
pub const MAX_GROUPS: usize = 4;

/// The bytes a keymap of `groups` groups takes
pub const fn size(groups: usize) -> usize {
    HEADER + groups * (CODE + COUNT * ENTRY)
}
/// Pool size, fitting any list of layouts
pub const SIZE_MAX: usize = size(MAX_GROUPS);

/// Caps lock means something for this key
pub const FLAG_LETTER: u32 = 1 << 0;

/// Writes `layouts` into `out` as groups 0, 1..., returning the bytes used. The
/// compositor's side. Fails if the buffer is too small or the list is empty or
/// longer than `MAX_GROUPS`
pub fn serialise(out: &mut [u8], layouts: &[&Layout]) -> Option<usize> {
    let groups = layouts.len();
    if groups == 0 || groups > MAX_GROUPS || out.len() < size(groups) {
        return None;
    }
    let mut put = |at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(0, MAGIC);
    put(4, VERSION);
    put(8, COUNT as u32);
    put(12, ENTRY as u32);
    put(16, groups as u32);
    for at in (20..HEADER).step_by(4) {
        put(at, 0);
    }
    for (g, l) in layouts.iter().enumerate() {
        let at = HEADER + g * CODE;
        let name = l.code.as_bytes();
        let n = name.len().min(CODE - 1);
        out[at..at + CODE].fill(0);
        out[at..at + n].copy_from_slice(&name[..n]);
    }
    let tables = HEADER + groups * CODE;
    for (g, l) in layouts.iter().enumerate() {
        for kc in 0..COUNT {
            let at = tables + (g * COUNT + kc) * ENTRY;
            let (plain, shift, flags) = match key_in(l, kc as u16) {
                Some((p, s, letter)) => (p as u32, s as u32, if letter { FLAG_LETTER } else { 0 }),
                None => (0, 0, 0),
            };
            let mut put = |i: usize, v: u32| out[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            put(0, plain);
            put(1, shift);
            put(2, to_named(kc as u16).code());
            put(3, flags);
        }
    }
    Some(size(groups))
}

/// A keymap as received, borrowing the mapped bytes
#[derive(Clone, Copy)]
pub struct Table<'a> {
    bytes: &'a [u8],
    count: usize,
    groups: usize,
}

impl<'a> Table<'a> {
    /// Checks the header and takes the table, or None; a client then uses its
    /// compiled-in tables
    pub fn new(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < HEADER {
            return None;
        }
        let word = |at: usize| u32::from_le_bytes([
            bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        if word(0) != MAGIC || word(4) != VERSION || word(12) as usize != ENTRY {
            return None;
        }
        let count = word(8) as usize;
        let groups = word(16) as usize;
        if groups == 0 || groups > MAX_GROUPS || count > COUNT {
            return None;
        }
        if bytes.len() < HEADER + groups * (CODE + count * ENTRY) {
            return None;
        }
        Some(Table { bytes, count, groups })
    }

    /// How many layouts the seat switches between
    pub fn groups(&self) -> usize {
        self.groups
    }

    /// Name of group `g`'s layout, "us" or "ru", or "" if out of range
    pub fn code(&self, g: usize) -> &'a str {
        if g >= self.groups {
            return "";
        }
        let at = HEADER + g * CODE;
        let raw = &self.bytes[at..at + CODE];
        let n = raw.iter().position(|&b| b == 0).unwrap_or(CODE);
        core::str::from_utf8(&raw[..n]).unwrap_or("")
    }

    fn entry(&self, g: usize, keycode: u16) -> Option<(u32, u32, u32, u32)> {
        let kc = keycode as usize;
        if kc >= self.count || g >= self.groups {
            return None;
        }
        let at = HEADER + self.groups * CODE + (g * self.count + kc) * ENTRY;
        let word = |i: usize| {
            let o = at + i * 4;
            u32::from_le_bytes([self.bytes[o], self.bytes[o + 1],
                                self.bytes[o + 2], self.bytes[o + 3]])
        };
        Some((word(0), word(1), word(2), word(3)))
    }

    /// Character for this keycode under `mods` in group `group`, by the rules
    /// of `to_char_in`. The group is taken modulo the count, as in Wayland, for
    /// a client ahead of a config that shrank the list
    pub fn to_char_in(&self, keycode: u16, mods: u32, group: u32) -> Option<char> {
        // Ctrl reads group 0, as `to_char_in` does
        let g = if mods & MOD_CTRL != 0 { 0 } else { group as usize % self.groups };
        let (plain, shift, _, flags) = self.entry(g, keycode)?;
        if plain == 0 {
            return None;
        }
        let shifted = mods & MOD_SHIFT != 0;
        if flags & FLAG_LETTER != 0 {
            if mods & MOD_CTRL != 0 {
                // Unlike `to_char_in`, a range check is needed: here the word
                // comes from a pool, and an entry claiming to be a letter below
                // 'a' would subtract past zero and panic
                let c = char::from_u32(plain)?;
                if !c.is_ascii_lowercase() {
                    return None;
                }
                return Some(((c as u8 - b'a') + 1) as char);
            }
            let upper = shifted ^ (mods & MOD_CAPS != 0);
            return char::from_u32(if upper { shift } else { plain });
        }
        char::from_u32(if shifted { shift } else { plain })
    }

    /// Character in group 0, US, for matching a shortcut
    pub fn to_char(&self, keycode: u16, mods: u32) -> Option<char> {
        self.to_char_in(keycode, mods, 0)
    }

    /// What the key is called, the same in every group: a layout moves
    /// characters, never Enter or the arrows
    pub fn to_named(&self, keycode: u16) -> Named {
        match self.entry(0, keycode) {
            Some((_, _, named, _)) => Named::from_code(named),
            None => Named::Other,
        }
    }
}

#[cfg(test)]
mod tests;
