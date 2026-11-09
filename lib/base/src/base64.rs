//! Base64 (RFC 4648)

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
pub const PAD: u8 = b'=';

/// A character's six bits
#[inline]
pub const fn value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Bytes `n` input bytes encode to, with padding
pub const fn padded_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

/// Bytes `n` input bytes encode to, without padding
pub const fn unpadded_len(n: usize) -> usize {
    n / 3 * 4 + match n % 3 {
        1 => 2,
        2 => 3,
        _ => 0,
    }
}

fn encode_into(data: &[u8], out: &mut [u8], pad: bool) -> usize {
    let mut o = 0;
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out[o] = ALPHABET[(n >> 18 & 0x3F) as usize];
        out[o + 1] = ALPHABET[(n >> 12 & 0x3F) as usize];
        o += 2;
        if chunk.len() > 1 {
            out[o] = ALPHABET[(n >> 6 & 0x3F) as usize];
            o += 1;
        } else if pad {
            out[o] = PAD;
            o += 1;
        }
        if chunk.len() > 2 {
            out[o] = ALPHABET[(n & 0x3F) as usize];
            o += 1;
        } else if pad {
            out[o] = PAD;
            o += 1;
        }
    }
    o
}

/// With padding. The byte count, or None if `out` is too small
pub fn encode(data: &[u8], out: &mut [u8]) -> Option<usize> {
    if out.len() < padded_len(data.len()) {
        return None;
    }
    Some(encode_into(data, out, true))
}

/// Without padding. The byte count, or None if `out` is too small
pub fn encode_unpadded(data: &[u8], out: &mut [u8]) -> Option<usize> {
    if out.len() < unpadded_len(data.len()) {
        return None;
    }
    Some(encode_into(data, out, false))
}

/// Length a multiple of four, `=` only at the end
pub fn decode(text: &[u8], out: &mut [u8]) -> Option<usize> {
    if text.len() % 4 != 0 {
        return None;
    }
    let mut o = 0;
    for q in text.chunks(4) {
        let n = (value(q[0])? as u32) << 18 | (value(q[1])? as u32) << 12;
        *out.get_mut(o)? = (n >> 16) as u8;
        o += 1;
        if q[2] == PAD {
            if q[3] != PAD {
                return None;
            }
            continue;
        }
        let n = n | (value(q[2])? as u32) << 6;
        *out.get_mut(o)? = (n >> 8) as u8;
        o += 1;
        if q[3] != PAD {
            *out.get_mut(o)? = (n | value(q[3])? as u32) as u8;
            o += 1;
        }
    }
    Some(o)
}

/// No padding. A character outside the alphabet or a length of 1 mod 4 is refused
pub fn decode_unpadded(text: &[u8], out: &mut [u8]) -> Option<usize> {
    if text.len() % 4 == 1 {
        return None;
    }
    let produced = text.len() / 4 * 3 + match text.len() % 4 {
        2 => 1,
        3 => 2,
        _ => 0,
    };
    if out.len() < produced {
        return None;
    }
    let mut o = 0;
    for chunk in text.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= (value(c)? as u32) << (18 - 6 * i);
        }
        out[o] = (n >> 16) as u8;
        o += 1;
        if chunk.len() > 2 {
            out[o] = (n >> 8) as u8;
            o += 1;
        }
        if chunk.len() > 3 {
            out[o] = n as u8;
            o += 1;
        }
    }
    Some(o)
}

/// A PEM body: whitespace between lines is skipped, anything else outside the
/// alphabet refuses it. `push` gets each byte. False if refused
pub fn decode_lines(text: &[u8], mut push: impl FnMut(u8)) -> bool {
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut padding = false;
    for &c in text {
        if c == PAD {
            padding = true;
            continue;
        }
        let v = match value(c) {
            Some(v) => v,
            None if c.is_ascii_whitespace() => continue,
            None => return false,
        };
        if padding {
            return false;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            push((acc >> bits) as u8);
        }
    }
    true
}
