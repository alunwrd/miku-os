//! A version and the total order over versions
//!
//! Text that does not fit the grammar is a parse error naming the offset.
//!
//! ```text
//! version := [ epoch ':' ] num ( '.' num ){0,3} [ '-' pre ] [ '_' rel ]
//! pre     := ident ( '.' ident )*          ident := digits | alphanumerics
//! ```
//!
//! - **epoch**: set by hand when upstream numbering restarts.
//! - **release numbers**: compared left to right, a missing one is zero; `1.2`
//!   and `1.2.0` are one version.
//! - **pre-release**: sorts before the version without it, as in semver.
//!   Identifiers compare left to right, a numeric one numerically and below an
//!   alphanumeric one, a prefix below its extension.
//! - **rel**: the packaging revision. Affects which build is newer, not what a
//!   dependency means.

use core::cmp::Ordering;
use core::fmt;

/// Most release numbers
pub const MAX_NUMS: usize = 4;
/// Most pre-release identifiers. `rc.1` is two
pub const MAX_PRE_IDS: usize = 4;
/// Longest pre-release identifier
pub const MAX_ID_LEN: usize = 12;
/// Whole text; fits `99:1.2.3.4-alpha.10.beta.9_255`
pub const MAX_TEXT: usize = 64;

/// Why a version could not be read, and where
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionError {
    /// Nothing at all
    Empty,
    /// Longer than `MAX_TEXT`
    TooLong { len: usize },
    /// A byte not allowed here
    BadByte { at: usize, byte: u8 },
    /// Does not fit in 32 bits, or has no digits
    BadNumber { at: usize },
    /// More than `MAX_NUMS` release numbers
    TooManyNumbers { at: usize },
    /// More than `MAX_PRE_IDS` pre-release identifiers
    TooManyPreIds { at: usize },
    /// A pre-release identifier longer than `MAX_ID_LEN`
    PreIdTooLong { at: usize },
    /// A separator with nothing after it
    Trailing { at: usize },
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            VersionError::Empty => write!(f, "a version cannot be empty"),
            VersionError::TooLong { len } => {
                write!(f, "a version may be {MAX_TEXT} bytes at most, this one is {len}")
            }
            VersionError::BadByte { at, byte } => write!(
                f,
                "byte {at}: {:?} cannot appear in a version",
                byte as char
            ),
            VersionError::BadNumber { at } => write!(f, "byte {at}: expected a number"),
            VersionError::TooManyNumbers { at } => write!(
                f,
                "byte {at}: a version has at most {MAX_NUMS} release numbers"
            ),
            VersionError::TooManyPreIds { at } => write!(
                f,
                "byte {at}: a pre-release has at most {MAX_PRE_IDS} identifiers"
            ),
            VersionError::PreIdTooLong { at } => write!(
                f,
                "byte {at}: a pre-release identifier may be {MAX_ID_LEN} bytes at most"
            ),
            VersionError::Trailing { at } => write!(f, "byte {at}: nothing follows the separator"),
        }
    }
}

/// One pre-release identifier. Numeric identifiers compare as numbers: `rc.2`
/// before `rc.10`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreId {
    Num(u32),
    Text([u8; MAX_ID_LEN], u8),
}

impl PreId {
    fn text(&self) -> &[u8] {
        match self {
            PreId::Num(_) => &[],
            PreId::Text(buf, len) => &buf[..*len as usize],
        }
    }
}

impl Ord for PreId {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PreId::Num(a), PreId::Num(b)) => a.cmp(b),
            // Semver 11.4.3: numeric identifiers rank below non-numeric ones
            (PreId::Num(_), PreId::Text(..)) => Ordering::Less,
            (PreId::Text(..), PreId::Num(_)) => Ordering::Greater,
            (PreId::Text(..), PreId::Text(..)) => self.text().cmp(other.text()),
        }
    }
}

impl PartialOrd for PreId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreId::Num(n) => write!(f, "{n}"),
            PreId::Text(..) => {
                for &b in self.text() {
                    write!(f, "{}", b as char)?;
                }
                Ok(())
            }
        }
    }
}

/// A version. Fixed size and `Copy`, since the resolver holds thousands in sets
/// and intervals
#[derive(Debug, Clone, Copy)]
pub struct Version {
    epoch: u32,
    nums: [u32; MAX_NUMS],
    num_count: u8,
    pre: [PreId; MAX_PRE_IDS],
    pre_count: u8,
    rel: u32,
}

impl Version {
    /// The lowest version, for the closed end of an interval
    pub const ZERO: Version = Version {
        epoch: 0,
        nums: [0; MAX_NUMS],
        num_count: 1,
        pre: [PreId::Num(0); MAX_PRE_IDS],
        pre_count: 0,
        rel: 0,
    };

    pub fn epoch(&self) -> u32 {
        self.epoch
    }
    pub fn release(&self) -> u32 {
        self.rel
    }
    pub fn numbers(&self) -> &[u32] {
        &self.nums[..self.num_count as usize]
    }
    pub fn pre_release(&self) -> &[PreId] {
        &self.pre[..self.pre_count as usize]
    }
    pub fn is_pre_release(&self) -> bool {
        self.pre_count > 0
    }

    /// Read a version, or say where it stopped making sense.
    pub fn parse(text: &str) -> Result<Version, VersionError> {
        let b = text.as_bytes();
        if b.is_empty() {
            return Err(VersionError::Empty);
        }
        if b.len() > MAX_TEXT {
            return Err(VersionError::TooLong { len: b.len() });
        }

        let mut v = Version::ZERO;
        let mut i = 0usize;

        // epoch: only if ':' appears before any '-' or '_'
        if let Some(colon) = b.iter().position(|&c| c == b':') {
            if b[..colon].iter().all(|c| c.is_ascii_digit()) && colon > 0 {
                v.epoch = read_num(b, &mut i, colon)?;
                i = colon + 1;
            } else {
                return Err(VersionError::BadByte { at: colon, byte: b':' });
            }
        }

        // release numbers
        v.num_count = 0;
        loop {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                return Err(VersionError::BadNumber { at: start });
            }
            if v.num_count as usize == MAX_NUMS {
                return Err(VersionError::TooManyNumbers { at: start });
            }
            let mut j = start;
            v.nums[v.num_count as usize] = read_num(b, &mut j, i)?;
            v.num_count += 1;
            if i < b.len() && b[i] == b'.' {
                i += 1;
                if i == b.len() {
                    return Err(VersionError::Trailing { at: i - 1 });
                }
                continue;
            }
            break;
        }

        // pre-release
        if i < b.len() && b[i] == b'-' {
            i += 1;
            if i == b.len() {
                return Err(VersionError::Trailing { at: i - 1 });
            }
            loop {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                if i == start {
                    return Err(VersionError::BadByte {
                        at: start,
                        byte: *b.get(start).unwrap_or(&b'?'),
                    });
                }
                if v.pre_count as usize == MAX_PRE_IDS {
                    return Err(VersionError::TooManyPreIds { at: start });
                }
                let seg = &b[start..i];
                v.pre[v.pre_count as usize] = if seg.iter().all(|c| c.is_ascii_digit()) {
                    let mut j = start;
                    PreId::Num(read_num(b, &mut j, i)?)
                } else {
                    if seg.len() > MAX_ID_LEN {
                        return Err(VersionError::PreIdTooLong { at: start });
                    }
                    let mut buf = [0u8; MAX_ID_LEN];
                    buf[..seg.len()].copy_from_slice(seg);
                    PreId::Text(buf, seg.len() as u8)
                };
                v.pre_count += 1;
                if i < b.len() && b[i] == b'.' {
                    i += 1;
                    if i == b.len() {
                        return Err(VersionError::Trailing { at: i - 1 });
                    }
                    continue;
                }
                break;
            }
        }

        // packaging revision
        if i < b.len() && b[i] == b'_' {
            i += 1;
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                return Err(VersionError::BadNumber { at: start });
            }
            let mut j = start;
            v.rel = read_num(b, &mut j, i)?;
        }

        if i != b.len() {
            return Err(VersionError::BadByte { at: i, byte: b[i] });
        }
        Ok(v)
    }
}

fn read_num(b: &[u8], i: &mut usize, end: usize) -> Result<u32, VersionError> {
    let start = *i;
    let mut acc: u64 = 0;
    while *i < end {
        if !b[*i].is_ascii_digit() {
            return Err(VersionError::BadNumber { at: *i });
        }
        acc = acc * 10 + (b[*i] - b'0') as u64;
        if acc > u32::MAX as u64 {
            return Err(VersionError::BadNumber { at: start });
        }
        *i += 1;
    }
    if *i == start {
        return Err(VersionError::BadNumber { at: start });
    }
    Ok(acc as u32)
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // epoch first; nothing below can overturn it
        if self.epoch != other.epoch {
            return self.epoch.cmp(&other.epoch);
        }

        // A missing release number is zero
        for k in 0..MAX_NUMS {
            let a = *self.nums.get(k).filter(|_| k < self.num_count as usize).unwrap_or(&0);
            let b = *other.nums.get(k).filter(|_| k < other.num_count as usize).unwrap_or(&0);
            if a != b {
                return a.cmp(&b);
            }
        }

        // A pre-release is less than its release
        match (self.pre_count, other.pre_count) {
            (0, 0) => {}
            (0, _) => return Ordering::Greater,
            (_, 0) => return Ordering::Less,
            _ => {
                let n = (self.pre_count as usize).min(other.pre_count as usize);
                for k in 0..n {
                    match self.pre[k].cmp(&other.pre[k]) {
                        Ordering::Equal => {}
                        other_order => return other_order,
                    }
                }
                // A prefix ranks below its extension: `rc` before `rc.1`
                if self.pre_count != other.pre_count {
                    return self.pre_count.cmp(&other.pre_count);
                }
            }
        }

        self.rel.cmp(&other.rel)
    }
}

/// Equality follows the order, not the fields. `1.2` and `1.2.0` hold different
/// counts but compare equal, and the resolver's sets rely on equality agreeing
/// with `Ord`
impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.epoch != 0 {
            write!(f, "{}:", self.epoch)?;
        }
        for (k, n) in self.numbers().iter().enumerate() {
            if k > 0 {
                write!(f, ".")?;
            }
            write!(f, "{n}")?;
        }
        for (k, id) in self.pre_release().iter().enumerate() {
            write!(f, "{}{}", if k == 0 { "-" } else { "." }, id)?;
        }
        if self.rel != 0 {
            write!(f, "_{}", self.rel)?;
        }
        Ok(())
    }
}

impl core::hash::Hash for Version {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        // Hashes what `cmp` compares, in the same normalized form, since `1.2`
        // and `1.2.0` compare equal
        self.epoch.hash(state);
        for k in 0..MAX_NUMS {
            let a = if k < self.num_count as usize { self.nums[k] } else { 0 };
            a.hash(state);
        }
        self.pre_release().hash(state);
        self.rel.hash(state);
    }
}

impl core::hash::Hash for PreId {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        match self {
            PreId::Num(n) => (0u8, n).hash(state),
            PreId::Text(..) => (1u8, self.text()).hash(state),
        }
    }
}
