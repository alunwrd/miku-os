//! Everything this parser can complain about, and where
//!
//! An unrecognized cosmetic line is a note with file and line, and the run
//! continues, as in `lib/gfx/mayland/src/conf.rs`.
//!
//! A malformed line inside a `repo` block, or a bad `key`, `keyring`, or `url`,
//! is fatal, as in `/etc/mikudo.conf`: `key =` decides which signatures this
//! machine believes.

use alloc::string::String;
use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Complaint {
    /// Unknown setting
    UnknownSetting,
    /// A line in a `repo` block that is not one of its settings
    UnknownRepoSetting,
    /// `key` with nothing after it
    MissingValue,
    /// A value this setting cannot hold
    BadValue { wanted: &'static str },
    /// `repo` with no name or an invalid one
    BadRepoName,
    /// A `repo` block that never closed, or a `}` with no block
    Unbalanced,
    /// A repository with no `url`
    RepoWithoutUrl,
    /// Past a bound
    TooMany { what: &'static str, limit: usize },
    TooBig { limit: usize },
}

impl fmt::Display for Complaint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Complaint::UnknownSetting => write!(f, "no setting by that name"),
            Complaint::UnknownRepoSetting => write!(
                f,
                "a repository may set url, key, priority and mirror, and nothing else"
            ),
            Complaint::MissingValue => write!(f, "nothing after the '='"),
            Complaint::BadValue { wanted } => write!(f, "expected {wanted}"),
            Complaint::BadRepoName => write!(f, "a repository needs a name, and a legal one"),
            Complaint::Unbalanced => write!(f, "a repository block that does not close"),
            Complaint::RepoWithoutUrl => {
                write!(f, "a repository with no url is one nothing can be fetched from")
            }
            Complaint::TooMany { what, limit } => {
                write!(f, "more than {limit} {what} in one configuration")
            }
            Complaint::TooBig { limit } => {
                write!(f, "a configuration file may be {limit} bytes at most")
            }
        }
    }
}

/// One complaint, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub file: String,
    /// One-based, as an editor counts
    pub line: usize,
    pub what: Complaint,
    /// The line as written, trimmed
    pub text: String,
    /// Stops the run
    pub fatal: bool,
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.what)?;
        if !self.text.is_empty() {
            write!(f, "  -  {:?}", self.text)?;
        }
        Ok(())
    }
}
