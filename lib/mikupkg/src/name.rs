//! What a package may be called
//!
//! A name becomes a path component, `/pkg/db/<name>.mpkr`. It is also typed,
//! printed, and matched against an index, so it is lowercase, with no space,
//! slash, or wildcard.

use core::fmt;

/// Longest package name
pub const MAX_NAME: usize = 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong { len: usize },
    /// A byte not allowed in a name
    BadByte { at: usize, byte: u8 },
    /// An upper-case letter, kept apart from `BadByte` since the advice differs
    UpperCase { at: usize, byte: u8 },
    /// A name starts and ends with a letter or digit
    BadEdge { at: usize, byte: u8 },
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            NameError::Empty => write!(f, "a package name cannot be empty"),
            NameError::TooLong { len } => write!(
                f,
                "a package name may be {MAX_NAME} bytes at most, this one is {len}"
            ),
            NameError::BadByte { at, byte } => write!(
                f,
                "byte {at}: {:?} cannot appear in a package name; use a-z, 0-9, '-', '_', '.' or '+'",
                byte as char
            ),
            NameError::UpperCase { at, byte } => write!(
                f,
                "byte {at}: {:?} - package names are lower case",
                byte as char
            ),
            NameError::BadEdge { at, byte } => write!(
                f,
                "byte {at}: a package name starts and ends with a letter or a digit, not {:?}",
                byte as char
            ),
        }
    }
}

/// A checked package name. Fixed size and `Copy`, since the resolver holds and
/// compares many
#[derive(Clone, Copy)]
pub struct Name {
    buf: [u8; MAX_NAME],
    len: u8,
}

impl Name {
    pub fn parse(text: &str) -> Result<Name, NameError> {
        let b = text.as_bytes();
        if b.is_empty() {
            return Err(NameError::Empty);
        }
        if b.len() > MAX_NAME {
            return Err(NameError::TooLong { len: b.len() });
        }
        for (i, &c) in b.iter().enumerate() {
            if c.is_ascii_uppercase() {
                return Err(NameError::UpperCase { at: i, byte: c });
            }
            let ok = c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, b'-' | b'_' | b'.' | b'+');
            if !ok {
                return Err(NameError::BadByte { at: i, byte: c });
            }
        }
        let first = b[0];
        let last = b[b.len() - 1];
        if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
            return Err(NameError::BadEdge { at: 0, byte: first });
        }
        if !(last.is_ascii_lowercase() || last.is_ascii_digit()) {
            return Err(NameError::BadEdge { at: b.len() - 1, byte: last });
        }

        let mut buf = [0u8; MAX_NAME];
        buf[..b.len()].copy_from_slice(b);
        Ok(Name { buf, len: b.len() as u8 })
    }

    pub fn as_str(&self) -> &str {
        // Every byte was checked to be ASCII on the way in
        core::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}
impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl core::hash::Hash for Name {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({:?})", self.as_str())
    }
}
