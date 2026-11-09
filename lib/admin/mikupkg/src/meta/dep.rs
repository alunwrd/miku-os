//! A dependency: a name and the versions that would do
//!
//! - `depends` is a constraint for the resolver.
//! - `conflicts` is a constraint, symmetric even when declared one way, so
//!   index order does not decide.
//! - `replaces` is a rewrite of the installed set before the solve.
//! - `optdepends` never reaches the solver; it is a sentence printed after a
//!   transaction.

use alloc::string::String;
use core::fmt;

use crate::name::{Name, NameError};
use crate::range::VersionSet;
use crate::version::{Version, VersionError};

/// Text form of a dependency, in a manifest, on the command line, and on the
/// wire.
///
/// ```text
/// dep        := name [ clause ( '|' clause )* ]
/// clause     := constraint ( ',' constraint )*
/// constraint := ( '>=' | '<=' | '!=' | '=' | '>' | '<' ) version
/// ```
///
/// Commas intersect and bars union: `libmay>=2.0,<3.0` is the overlap,
/// `libmay<2.0|>2.0` is everything but 2.0.
///
/// The bar lets the grammar write every set it can produce. `!=2.0` is two
/// intervals, and metadata is written back on rebuild and in the installed
/// database, so a round trip must be the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepError {
    Name(NameError),
    Version { at: usize, why: VersionError },
    /// A comparison with nothing to compare against
    MissingVersion { at: usize },
    /// Text after the name that is not a comparison
    BadOperator { at: usize },
    /// The constraints cannot all hold: `>2.0,<1.0`
    Impossible,
}

impl fmt::Display for DepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DepError::Name(e) => write!(f, "{e}"),
            DepError::Version { at, why } => write!(f, "in the version at byte {at}: {why}"),
            DepError::MissingVersion { at } => {
                write!(f, "byte {at}: a comparison with no version after it")
            }
            DepError::BadOperator { at } => write!(
                f,
                "byte {at}: expected one of >=, <=, !=, =, > or < after the package name"
            ),
            DepError::Impossible => write!(
                f,
                "these constraints leave no version at all, so nothing could ever \
                 satisfy them"
            ),
        }
    }
}

/// A name and the versions of it that satisfy this relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: Name,
    pub versions: VersionSet,
}

impl Dep {
    /// Any version of it
    pub fn any(name: Name) -> Dep {
        Dep { name, versions: VersionSet::full() }
    }

    pub fn exactly(name: Name, v: Version) -> Dep {
        Dep { name, versions: VersionSet::exact(v) }
    }

    /// Is this package, at this version, what the dependency asked for?
    pub fn satisfied_by(&self, name: &Name, v: &Version) -> bool {
        *name == self.name && self.versions.contains(v)
    }

    pub fn parse(text: &str) -> Result<Dep, DepError> {
        let b = text.as_bytes();
        let split = b
            .iter()
            .position(|&c| matches!(c, b'>' | b'<' | b'=' | b'!'))
            .unwrap_or(b.len());

        let name = Name::parse(&text[..split]).map_err(DepError::Name)?;
        if split == b.len() {
            return Ok(Dep::any(name));
        }

        // Bars union clauses, commas intersect within one, left to right
        let mut versions = VersionSet::empty();
        let mut clause = VersionSet::full();
        let mut i = split;
        loop {
            let (set, next) = read_constraint(text, i)?;
            clause = clause.intersection(&set);
            i = next;
            if i >= b.len() {
                break;
            }
            match b[i] {
                b',' => {
                    i += 1;
                }
                b'|' => {
                    versions = versions.union(&clause);
                    clause = VersionSet::full();
                    i += 1;
                }
                _ => return Err(DepError::BadOperator { at: i }),
            }
        }
        versions = versions.union(&clause);

        if versions.is_empty() {
            return Err(DepError::Impossible);
        }
        Ok(Dep { name, versions })
    }
}

fn read_constraint(text: &str, at: usize) -> Result<(VersionSet, usize), DepError> {
    let b = text.as_bytes();
    // Two bytes before one: `>=` must not read as `>` and a version beginning
    // `=`
    let (op, skip): (&str, usize) = match (b.get(at), b.get(at + 1)) {
        (Some(b'>'), Some(b'=')) => (">=", 2),
        (Some(b'<'), Some(b'=')) => ("<=", 2),
        (Some(b'!'), Some(b'=')) => ("!=", 2),
        (Some(b'='), _) => ("=", 1),
        (Some(b'>'), _) => (">", 1),
        (Some(b'<'), _) => ("<", 1),
        _ => return Err(DepError::BadOperator { at }),
    };

    let start = at + skip;
    let end = b[start..]
        .iter()
        .position(|&c| c == b',' || c == b'|')
        .map(|p| start + p)
        .unwrap_or(b.len());
    if end == start {
        return Err(DepError::MissingVersion { at: start });
    }

    let v = Version::parse(&text[start..end])
        .map_err(|why| DepError::Version { at: start, why })?;

    let set = match op {
        ">=" => VersionSet::at_least(v),
        "<=" => VersionSet::at_most(v),
        ">" => VersionSet::greater_than(v),
        "<" => VersionSet::less_than(v),
        "=" => VersionSet::exact(v),
        "!=" => VersionSet::exact(v).complement(),
        _ => unreachable!("the operator table above is the only source"),
    };
    Ok((set, end))
}

impl Dep {
    /// The form `Dep::parse` accepts, for the wire and a manifest. Unlike
    /// `Display`, which adds a space for reading, this is `parse`'s inverse,
    /// checked in `tests/meta.rs`
    pub fn to_wire(&self) -> String {
        use alloc::format;
        use core::fmt::Write;

        use crate::range::Bound;

        if self.versions.is_full() {
            return format!("{}", self.name);
        }
        let mut s = format!("{}", self.name);
        for (k, part) in self.versions.intervals().iter().enumerate() {
            if k > 0 {
                let _ = s.write_char('|');
            }
            // A single point is one `=`, not `>=1.0,<=1.0`
            if let (Bound::Included(a), Bound::Included(b)) = (part.lo, part.hi) {
                if a == b {
                    let _ = write!(s, "={a}");
                    continue;
                }
            }
            let mut wrote = false;
            match part.lo {
                Bound::Included(v) => {
                    let _ = write!(s, ">={v}");
                    wrote = true;
                }
                Bound::Excluded(v) => {
                    let _ = write!(s, ">{v}");
                    wrote = true;
                }
                Bound::Unbounded => {}
            }
            match part.hi {
                Bound::Included(v) => {
                    let _ = write!(s, "{}<={v}", if wrote { "," } else { "" });
                }
                Bound::Excluded(v) => {
                    let _ = write!(s, "{}<{v}", if wrote { "," } else { "" });
                }
                Bound::Unbounded => {}
            }
        }
        s
    }
}

impl fmt::Display for Dep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.versions.is_full() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{} {}", self.name, self.versions)
        }
    }
}

/// What a package says it also is. A provide has its own version, as in
/// `vi provides editor=1.0`, so a dependency can constrain it. Without one, the
/// provider's version is used
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provide {
    pub name: Name,
    pub version: Option<Version>,
}

impl Provide {
    pub fn parse(text: &str) -> Result<Provide, DepError> {
        match text.find('=') {
            None => Ok(Provide {
                name: Name::parse(text).map_err(DepError::Name)?,
                version: None,
            }),
            Some(at) => {
                let name = Name::parse(&text[..at]).map_err(DepError::Name)?;
                if at + 1 == text.len() {
                    return Err(DepError::MissingVersion { at: at + 1 });
                }
                let version = Version::parse(&text[at + 1..])
                    .map_err(|why| DepError::Version { at: at + 1, why })?;
                Ok(Provide { name, version: Some(version) })
            }
        }
    }

    /// The version this provide stands at, given the package providing it
    pub fn version_or(&self, own: Version) -> Version {
        self.version.unwrap_or(own)
    }
}

impl fmt::Display for Provide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.version {
            None => write!(f, "{}", self.name),
            Some(v) => write!(f, "{}={}", self.name, v),
        }
    }
}

/// A suggestion with its reason
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptDep {
    pub dep: Dep,
    pub why: String,
}

impl fmt::Display for OptDep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.why.is_empty() {
            write!(f, "{}", self.dep)
        } else {
            write!(f, "{}: {}", self.dep, self.why)
        }
    }
}
