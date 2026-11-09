//! Hexadecimal digits

pub const LOWER: [u8; 16] = *b"0123456789abcdef";
pub const UPPER: [u8; 16] = *b"0123456789ABCDEF";

/// A digit's value, either case
#[inline]
pub const fn value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The lowercase digit of the low four bits
#[inline]
pub const fn digit(nibble: u8) -> u8 {
    LOWER[(nibble & 0xF) as usize]
}
