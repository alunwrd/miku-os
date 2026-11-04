// crypt(3) base64: not the RFC 4648 alphabet, and no padding

const ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub const MAX_DECODED: usize = 64;

/// Encodes without padding, appending to `out`. Returns the byte count, or None
/// if too small
pub fn encode(data: &[u8], out: &mut [u8]) -> Option<usize> {
    let need = data.len().div_ceil(3) * 4 - match data.len() % 3 {
        1 => 2,
        2 => 1,
        _ => 0,
    };
    if out.len() < need {
        return None;
    }
    let mut o = 0usize;
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out[o] = ALPHABET[((n >> 18) & 0x3f) as usize];
        out[o + 1] = ALPHABET[((n >> 12) & 0x3f) as usize];
        o += 2;
        if chunk.len() > 1 {
            out[o] = ALPHABET[((n >> 6) & 0x3f) as usize];
            o += 1;
        }
        if chunk.len() > 2 {
            out[o] = ALPHABET[(n & 0x3f) as usize];
            o += 1;
        }
    }
    Some(o)
}

fn value_of(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 26),
        b'0'..=b'9' => Some((c - b'0') as u32 + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decodes into `out`, returning the byte count. Strict: an out-of-alphabet
/// character or a length of 1 mod 4 is refused
pub fn decode(text: &[u8], out: &mut [u8]) -> Option<usize> {
    if text.len() % 4 == 1 {
        return None;
    }
    let produced = text.len() / 4 * 3
        + match text.len() % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        };
    if out.len() < produced {
        return None;
    }
    let mut o = 0usize;
    for chunk in text.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value_of(c)? << (18 - 6 * i);
        }
        out[o] = (n >> 16) as u8;
        o += 1;
        if chunk.len() > 2 {
            out[o] = ((n >> 8) & 0xff) as u8;
            o += 1;
        }
        if chunk.len() > 3 {
            out[o] = (n & 0xff) as u8;
            o += 1;
        }
    }
    Some(o)
}
