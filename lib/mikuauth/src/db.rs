// /etc/passwd, /etc/group, and /etc/shadow parsers (tests/db.rs)
//
// - An empty field's meaning depends on position: an empty shell is the
//   default, an empty uid is a broken line.
// - A line with too few fields is rejected, not padded.
// - A numeric field that does not fit is rejected, not wrapped.
// - A NUL or newline in a field makes the whole file read as empty.
//
// Nothing allocates; iterators borrow the caller's buffer.

/// Longest line in any of the three files. Longer is refused, not truncated
pub const MAX_LINE: usize = 4096;
/// The longest single field
pub const MAX_FIELD: usize = 256;
/// Largest id, as in the kernel. 0xFFFF is chown's sentinel
pub const ID_MAX: u32 = 0xFFFE;

/// Meaning of an empty seventh field: the session program
pub const DEFAULT_SESSION: &str = "/bin/maysession";

/// The command interpreter, for `mikudo -s`
pub const DEFAULT_SHELL: &str = "/bin/msh";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbError {
    /// Fewer fields than the format requires
    TooFewFields,
    /// More fields than the format allows
    TooManyFields,
    /// A field longer than MAX_FIELD, or a line longer than MAX_LINE
    TooLong,
    /// A numeric field that is empty, non-numeric, or out of range
    BadId,
    /// A NUL byte, or not UTF-8
    BadBytes,
    /// Empty name, or one with a forbidden character
    BadName,
}

/// Whether a byte may appear in a user or group name. Excludes `:`, `,`, `$`,
/// `/`, and whitespace, which mean something in these files or mikudo.conf
fn name_byte_ok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'
}

fn check_name(s: &str) -> Result<(), DbError> {
    if s.is_empty() || s.len() > 32 {
        return Err(DbError::BadName);
    }
    // A leading `-` reads as an option, a leading `.` hides it from a listing
    if s.starts_with('-') || s.starts_with('.') {
        return Err(DbError::BadName);
    }
    if !s.bytes().all(name_byte_ok) {
        return Err(DbError::BadName);
    }
    Ok(())
}

fn parse_id(s: &str) -> Result<u16, DbError> {
    if s.is_empty() || s.len() > 5 {
        return Err(DbError::BadId);
    }
    let mut n: u32 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return Err(DbError::BadId);
        }
        n = n * 10 + (b - b'0') as u32;
        if n > ID_MAX {
            return Err(DbError::BadId);
        }
    }
    Ok(n as u16)
}

/// Splits into at most `N` fields, refusing more. `splitn` would put the
/// overflow in the last field
fn fields<'a, const N: usize>(line: &'a str) -> Result<[&'a str; N], DbError> {
    let mut out = [""; N];
    let mut n = 0usize;
    for f in line.split(':') {
        if n == N {
            return Err(DbError::TooManyFields);
        }
        if f.len() > MAX_FIELD {
            return Err(DbError::TooLong);
        }
        out[n] = f;
        n += 1;
    }
    if n < N {
        return Err(DbError::TooFewFields);
    }
    Ok(out)
}

/// One line of /etc/passwd
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Passwd<'a> {
    pub name: &'a str,
    pub uid: u16,
    pub gid: u16,
    pub gecos: &'a str,
    pub home: &'a str,
    pub shell: &'a str,
}

impl<'a> Passwd<'a> {
    pub fn parse(line: &'a str) -> Result<Self, DbError> {
        let f = fields::<7>(line)?;
        let name = f[0];
        check_name(name)?;
        // f[1] is the unused password field, 'x'; the hash is in /etc/shadow
        Ok(Passwd {
            name,
            uid: parse_id(f[2])?,
            gid: parse_id(f[3])?,
            gecos: f[4],
            home: if f[5].is_empty() { "/" } else { f[5] },
            shell: if f[6].is_empty() { DEFAULT_SESSION } else { f[6] },
        })
    }
}

/// One /etc/group line. Members stay raw; `Group::members()` walks them
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group<'a> {
    pub name: &'a str,
    pub gid: u16,
    pub member_list: &'a str,
}

impl<'a> Group<'a> {
    pub fn parse(line: &'a str) -> Result<Self, DbError> {
        let f = fields::<4>(line)?;
        check_name(f[0])?;
        Ok(Group { name: f[0], gid: parse_id(f[2])?, member_list: f[3] })
    }

    pub fn members(&self) -> impl Iterator<Item = &'a str> {
        self.member_list.split(',').filter(|m| !m.is_empty())
    }

    pub fn has_member(&self, user: &str) -> bool {
        self.members().any(|m| m == user)
    }
}

/// One /etc/shadow line. Dates are days since the epoch; empty means not set
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shadow<'a> {
    pub name: &'a str,
    pub hash: &'a str,
    pub last_change: Option<u32>,
    pub min_days: Option<u32>,
    pub max_days: Option<u32>,
    pub warn_days: Option<u32>,
    pub inactive_days: Option<u32>,
    pub expires: Option<u32>,
}

impl<'a> Shadow<'a> {
    pub fn parse(line: &'a str) -> Result<Self, DbError> {
        let f = fields::<9>(line)?;
        check_name(f[0])?;
        if f[1].len() > crate::hash::MAX_HASH_LEN {
            return Err(DbError::TooLong);
        }
        Ok(Shadow {
            name: f[0],
            hash: f[1],
            last_change:   parse_days(f[2])?,
            min_days:      parse_days(f[3])?,
            max_days:      parse_days(f[4])?,
            warn_days:     parse_days(f[5])?,
            inactive_days: parse_days(f[6])?,
            expires:       parse_days(f[7])?,
        })
    }

    /// Whether the account may log in today, ignoring the password. `today` is
    /// days since the epoch
    pub fn usable_on(&self, today: u32) -> bool {
        if let Some(exp) = self.expires {
            if today >= exp {
                return false;
            }
        }
        if let (Some(changed), Some(max)) = (self.last_change, self.max_days) {
            if let Some(inactive) = self.inactive_days {
                // Past the maximum age and the grace period
                if today > changed.saturating_add(max).saturating_add(inactive) {
                    return false;
                }
            }
        }
        true
    }
}

fn parse_days(s: &str) -> Result<Option<u32>, DbError> {
    if s.is_empty() {
        return Ok(None);
    }
    if s.len() > 9 {
        return Err(DbError::BadId);
    }
    let mut n: u32 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return Err(DbError::BadId);
        }
        n = n * 10 + (b - b'0') as u32;
    }
    Ok(Some(n))
}

/// Walks the lines of a file, skipping comments and blanks. Unparsable lines
/// are skipped and reported through `on_error`. A file that is not UTF-8 yields
/// nothing
pub struct Lines<'a> {
    rest: &'a str,
    line_no: usize,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a str) -> Self {
        Lines { rest: text, line_no: 0 }
    }
}

impl<'a> Iterator for Lines<'a> {
    /// (line number, contents)
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let (line, rest) = match self.rest.find('\n') {
                Some(i) => (&self.rest[..i], &self.rest[i + 1..]),
                None => (self.rest, ""),
            };
            self.rest = rest;
            self.line_no += 1;
            // Strips the carriage return of CRLF
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.len() > MAX_LINE {
                continue;
            }
            return Some((self.line_no, line));
        }
    }
}

/// Reads bytes as text, refusing non-UTF-8 or a NUL
pub fn as_text(bytes: &[u8]) -> Result<&str, DbError> {
    if bytes.contains(&0) {
        return Err(DbError::BadBytes);
    }
    core::str::from_utf8(bytes).map_err(|_| DbError::BadBytes)
}

/// Find one user by name
pub fn find_user<'a>(passwd: &'a str, name: &str) -> Option<Passwd<'a>> {
    Lines::new(passwd)
        .filter_map(|(_, l)| Passwd::parse(l).ok())
        .find(|p| p.name == name)
}

/// Finds a user by uid; the first match wins. `useradd` refuses duplicates
pub fn find_uid(passwd: &str, uid: u16) -> Option<Passwd<'_>> {
    Lines::new(passwd)
        .filter_map(|(_, l)| Passwd::parse(l).ok())
        .find(|p| p.uid == uid)
}

pub fn find_group<'a>(group: &'a str, name: &str) -> Option<Group<'a>> {
    Lines::new(group)
        .filter_map(|(_, l)| Group::parse(l).ok())
        .find(|g| g.name == name)
}

pub fn find_gid(group: &str, gid: u16) -> Option<Group<'_>> {
    Lines::new(group)
        .filter_map(|(_, l)| Group::parse(l).ok())
        .find(|g| g.gid == gid)
}

pub fn find_shadow<'a>(shadow: &'a str, name: &str) -> Option<Shadow<'a>> {
    Lines::new(shadow)
        .filter_map(|(_, l)| Shadow::parse(l).ok())
        .find(|s| s.name == name)
}

/// Writes every group of `user`, primary included, to `out`. Returns the count;
/// extras are dropped, as the kernel does past MAX_GROUPS
pub fn groups_of(group_file: &str, user: &str, primary: u16, out: &mut [u16]) -> usize {
    let mut n = 0usize;
    if !out.is_empty() {
        out[0] = primary;
        n = 1;
    }
    for (_, line) in Lines::new(group_file) {
        let g = match Group::parse(line) {
            Ok(g) => g,
            Err(_) => continue,
        };
        if g.gid == primary || !g.has_member(user) {
            continue;
        }
        if n >= out.len() {
            break;
        }
        out[n] = g.gid;
        n += 1;
    }
    n
}
