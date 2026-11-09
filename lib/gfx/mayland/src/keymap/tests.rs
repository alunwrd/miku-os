// Layouts and their serialized form, on the host
//
//     cd lib/gfx/mayland && cargo test-host
//
// Compiled-in and serialized tables are one layout read two ways, so every
// keycode, group, and modifier is compared. The keys easy to get wrong are
// listed too.

use super::*;

fn us() -> &'static Layout { &LAYOUTS[layout("us").unwrap()] }
fn ru() -> &'static Layout { &LAYOUTS[layout("ru").unwrap()] }

fn sent(layouts: &[&Layout]) -> std::vec::Vec<u8> {
    let mut b = std::vec![0xa5u8; SIZE_MAX];
    assert_eq!(serialise(&mut b, layouts), Some(size(layouts.len())));
    b
}

const MASKS: [u32; 7] = [
    0, MOD_SHIFT, MOD_CAPS, MOD_SHIFT | MOD_CAPS, MOD_CTRL,
    MOD_CTRL | MOD_SHIFT, MOD_ALT,
];

#[test]
fn the_sent_table_is_the_compiled_one() {
    let lays = [us(), ru()];
    let b = sent(&lays);
    let t = Table::new(&b).expect("a table");
    assert_eq!(t.groups(), 2);
    assert_eq!((t.code(0), t.code(1), t.code(2)), ("us", "ru", ""));
    for (g, l) in lays.iter().enumerate() {
        for kc in 0..COUNT as u16 {
            for &m in &MASKS {
                assert_eq!(t.to_char_in(kc, m, g as u32), to_char_in(l, kc, m),
                           "keycode {kc} group {g} mods {m:#x}");
            }
            assert_eq!(t.to_named(kc), to_named(kc));
        }
    }
}

#[test]
fn group_zero_is_what_to_char_always_was() {
    // `to_char` is what binds and clients with no keymap read
    assert_eq!(to_char(30, 0), Some('a'));
    assert_eq!(to_char(30, MOD_SHIFT), Some('A'));
    assert_eq!(to_char(30, MOD_SHIFT | MOD_CAPS), Some('a'));
    assert_eq!(to_char(46, MOD_CTRL), Some('\x03'));
    assert_eq!(to_char(2, MOD_SHIFT), Some('!'));
    assert_eq!(to_char(43, MOD_SHIFT), Some('|'));
    assert_eq!(to_char(28, 0), Some('\n'));
    assert_eq!(to_char(103, 0), None);
}

#[test]
fn russian_is_winkeys() {
    let r = ru();
    assert_eq!(to_char_in(r, 16, 0), Some('й'));
    assert_eq!(to_char_in(r, 16, MOD_SHIFT), Some('Й'));
    assert_eq!(to_char_in(r, 16, MOD_CAPS), Some('Й'));
    assert_eq!(to_char_in(r, 16, MOD_CAPS | MOD_SHIFT), Some('й'));
    // The keys that are easy to get wrong from memory
    assert_eq!(to_char_in(r, 41, 0), Some('ё'));
    assert_eq!(to_char_in(r, 41, MOD_SHIFT), Some('Ё'));
    assert_eq!(to_char_in(r, 43, 0), Some('\\'));
    assert_eq!(to_char_in(r, 43, MOD_SHIFT), Some('/'));
    assert_eq!(to_char_in(r, 53, 0), Some('.'));
    assert_eq!(to_char_in(r, 53, MOD_SHIFT), Some(','));
    assert_eq!(to_char_in(r, 3, MOD_SHIFT), Some('"'));
    assert_eq!(to_char_in(r, 4, MOD_SHIFT), Some('№'));
    assert_eq!(to_char_in(r, 5, MOD_SHIFT), Some(';'));
    assert_eq!(to_char_in(r, 8, MOD_SHIFT), Some('?'));
    assert_eq!(to_char_in(r, 2, MOD_SHIFT), Some('!'));
    assert_eq!(to_char_in(r, 3, 0), Some('2'));
    assert_eq!(to_char_in(r, 83, 0), Some(','));
    // Non-characters are the same keyboard
    assert_eq!(to_char_in(r, 28, 0), Some('\n'));
    assert_eq!(to_char_in(r, 57, 0), Some(' '));
    // The whole alphabet, once each
    let mut seen: std::vec::Vec<char> = (0..COUNT as u16)
        .filter_map(|kc| to_char_in(r, kc, 0))
        .filter(|c| ('а'..='я').contains(c) || *c == 'ё')
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 33);
}

#[test]
fn ctrl_reads_the_latin_letters() {
    // Ctrl+С is ^C in Russian, and Ctrl with a digit is the US digit
    let r = ru();
    assert_eq!(to_char_in(r, 46, MOD_CTRL), Some('\x03'));
    assert_eq!(to_char_in(r, 46, MOD_CTRL | MOD_SHIFT), Some('\x03'));
    assert_eq!(to_char_in(r, 4, MOD_CTRL | MOD_SHIFT), Some('#'));
    let b = sent(&[us(), ru()]);
    let t = Table::new(&b).unwrap();
    assert_eq!(t.to_char_in(46, MOD_CTRL, 1), Some('\x03'));
}

#[test]
fn the_group_wraps_and_one_group_is_allowed() {
    let b = sent(&[ru()]);
    let t = Table::new(&b).unwrap();
    assert_eq!(t.groups(), 1);
    assert_eq!(t.to_char_in(16, 0, 0), Some('й'));
    // A client a group ahead of a list that shrank types in what there is
    assert_eq!(t.to_char_in(16, 0, 1), Some('й'));
}

#[test]
fn a_rewritten_pool_reads_as_the_new_list() {
    // The compositor rewrites the pool in place, so anything past the end of a
    // shorter list must not matter
    let mut b = sent(&[us(), ru(), us(), ru()]);
    assert_eq!(serialise(&mut b, &[ru()]), Some(size(1)));
    let t = Table::new(&b).unwrap();
    assert_eq!((t.groups(), t.code(0)), (1, "ru"));
}

#[test]
fn what_is_refused() {
    let mut out = std::vec![0u8; SIZE_MAX];
    assert_eq!(serialise(&mut out, &[]), None);
    assert_eq!(serialise(&mut out, &[us(); 5]), None);
    assert_eq!(serialise(&mut out[..size(2) - 1], &[us(), ru()]), None);

    let good = sent(&[us(), ru()]);
    let with = |at: usize, v: u32| {
        let mut b = good.clone();
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        b
    };
    // Only this format is accepted
    assert!(Table::new(&with(4, 1)).is_none());
    assert!(Table::new(&with(0, 0)).is_none());
    assert!(Table::new(&with(16, 0)).is_none());
    assert!(Table::new(&with(16, 5)).is_none());
    assert!(Table::new(&good[..size(2) - 1]).is_none());
    // A letter entry below 'a' under ctrl is refused
    let entry = HEADER + 2 * CODE + 30 * ENTRY;
    let b = with(entry, '0' as u32);
    assert_eq!(Table::new(&b).unwrap().to_char_in(30, MOD_CTRL, 0), None);
}
