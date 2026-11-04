//! A path a package may own
//!
//! **The filesystem.** `MAX_PATH_DEPTH` is 32; deeper paths could not be
//! resolved. A name is bounded by `MAX_PATH`.
//!
//! **Traversal.** `..` and absolute paths write outside the unpack tree.
//! Checked in the container writer on the host, so such a package cannot be
//! built, and again in the reader.
//!
//! **Configuration.** A package may not own a file under any `conf.d`
//! directory: `conf.d` is where nothing is installed, `default.d` is what the
//! build ships. A package file and a user file never share a path, so there is
//! no three-way merge.

use core::fmt;

/// `MAX_PATH_DEPTH` in `kernel/fs/vfs/types.rs`
pub const MAX_DEPTH: usize = 32;
/// The whole path, and so any name in it: `NAME_LEN` in
/// `kernel/fs/vfs/types.rs`
pub const MAX_PATH: usize = 255;

/// Directory name a package may never write into, at any depth
pub const USER_CONFIG_DIR: &str = "conf.d";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    Empty,
    /// Longer than `MAX_PATH`
    TooLong { len: usize },
    /// Deeper than `MAX_DEPTH`
    TooDeep { depth: usize },
    /// A path inside a package is relative to the root it is installed into
    Absolute,
    /// `..`, `.`, or an empty component between two slashes
    BadComponent { at: usize },
    /// A byte that has no business in a path
    BadByte { at: usize, byte: u8 },
    /// Under a `conf.d` directory, which belongs to whoever runs the machine
    UserConfig { at: usize },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            PathError::Empty => write!(f, "a path cannot be empty"),
            PathError::TooLong { len } => {
                write!(f, "a path may be {MAX_PATH} bytes at most, this one is {len}")
            }
            PathError::TooDeep { depth } => write!(
                f,
                "a path may be {MAX_DEPTH} components deep at most, this one is {depth}"
            ),
            PathError::Absolute => write!(
                f,
                "a path inside a package is relative to the root it is installed \
                 into; drop the leading '/'"
            ),
            PathError::BadComponent { at } => write!(
                f,
                "byte {at}: '.', '..' and an empty component cannot appear in a \
                 package's path"
            ),
            PathError::BadByte { at, byte } => {
                write!(f, "byte {at}: {:?} cannot appear in a path", byte as char)
            }
            PathError::UserConfig { at } => write!(
                f,
                "byte {at}: a package may not own a file under '{USER_CONFIG_DIR}' - that \
                 directory belongs to whoever runs the machine, and nothing is ever \
                 installed there. Ship the default under a 'default.d' directory instead"
            ),
        }
    }
}

/// Checks a path a package proposes to own. Returns the depth
pub fn check(path: &str) -> Result<usize, PathError> {
    check_kind(path, false)
}

/// The same for a directory. A package may own the `conf.d` directory itself
/// but nothing inside it, so an image can ship an empty `/etc/mayland/conf.d`
pub fn check_dir(path: &str) -> Result<usize, PathError> {
    check_kind(path, true)
}

fn check_kind(path: &str, is_dir: bool) -> Result<usize, PathError> {
    let b = path.as_bytes();
    if b.is_empty() {
        return Err(PathError::Empty);
    }
    if b.len() > MAX_PATH {
        return Err(PathError::TooLong { len: b.len() });
    }
    if b[0] == b'/' {
        return Err(PathError::Absolute);
    }

    for (i, &c) in b.iter().enumerate() {
        // Printable ASCII only, no backslash
        if c < 0x20 || c >= 0x7F || c == b'\\' {
            return Err(PathError::BadByte { at: i, byte: c });
        }
    }

    let components = b.split(|&c| c == b'/').count();
    let mut depth = 0usize;
    let mut at = 0usize;
    for part in b.split(|&c| c == b'/') {
        if part.is_empty() || part == b"." || part == b".." {
            return Err(PathError::BadComponent { at });
        }
        // A directory may be `conf.d`; nothing may be inside one, and no file
        // may take its name
        let last = depth + 1 == components;
        if part == USER_CONFIG_DIR.as_bytes() && !(is_dir && last) {
            return Err(PathError::UserConfig { at });
        }
        depth += 1;
        if depth > MAX_DEPTH {
            return Err(PathError::TooDeep { depth });
        }
        at += part.len() + 1;
    }

    Ok(depth)
}
