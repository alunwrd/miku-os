// 8.3 names, as a directory entry stores them

use crate::FatError;

/// `name` as eleven bytes: eight and three, upper case, padded with spaces.
/// A name that would need a long entry is refused
pub fn short_name(name: &str) -> Result<[u8; 11], FatError> {
    let (base, ext) = match name.rsplit_once('.') {
        Some((_, "")) => return Err(FatError::BadName),
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || base.contains('.') {
        return Err(FatError::BadName);
    }
    let mut out = [b' '; 11];
    for (i, c) in base.bytes().enumerate() {
        out[i] = short_char(c)?;
    }
    for (i, c) in ext.bytes().enumerate() {
        out[8 + i] = short_char(c)?;
    }
    Ok(out)
}

fn short_char(c: u8) -> Result<u8, FatError> {
    match c {
        b'a'..=b'z' => Ok(c - 32),
        b'A'..=b'Z' | b'0'..=b'9' => Ok(c),
        b'$' | b'%' | b'\'' | b'-' | b'_' | b'@' | b'~' | b'`' | b'!' | b'(' | b')' | b'{' | b'}'
        | b'^' | b'#' | b'&' => Ok(c),
        _ => Err(FatError::BadName),
    }
}
