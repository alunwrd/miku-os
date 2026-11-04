//! Metadata tags, and which an old machine may skip
//!
//! Metadata is `u16 tag, u32 len, bytes` records, since the root half of the
//! installer parses it.
//!
//! The high bit of a tag means critical. A critical tag this build does not
//! understand refuses the package and names the tag. A non-critical one is
//! reported as a note and skipped.

/// Set on a tag whose meaning a reader must understand to be safe.
pub const CRITICAL: u16 = 0x8000;

// Critical: not understanding one could install the wrong thing

/// The package's name. Without it there is nothing to install
pub const NAME: u16 = CRITICAL | 0x01;
/// Its version
pub const VERSION: u16 = CRITICAL | 0x02;
/// Target architecture
pub const ARCH: u16 = CRITICAL | 0x03;
/// What must be installed with it
pub const DEPENDS: u16 = CRITICAL | 0x04;
/// What it also counts as. Skipping one could install a second provider of a
/// virtual name
pub const PROVIDES: u16 = CRITICAL | 0x05;
/// What may not be installed beside it
pub const CONFLICTS: u16 = CRITICAL | 0x06;
/// What it takes over from
pub const REPLACES: u16 = CRITICAL | 0x07;
/// What must happen after install
pub const TRIGGER: u16 = CRITICAL | 0x08;

// Not critical: skipping one costs a line of output

/// One line, for `mikupkg info` and `search`
pub const DESC: u16 = 0x41;
pub const URL: u16 = 0x42;
pub const LICENSE: u16 = 0x43;
/// Build time, in seconds since the epoch
pub const BUILD_TIME: u16 = 0x44;
/// Bytes once installed, to weigh a transaction against `statfs`
pub const INSTALLED_SIZE: u16 = 0x45;
/// A suggestion and its reason, never a constraint
pub const OPTDEPENDS: u16 = 0x46;

/// Is this tag one a reader must understand?
pub const fn is_critical(tag: u16) -> bool {
    tag & CRITICAL != 0
}

/// Name for a tag, for the refusal message. An unknown tag has none
pub fn tag_name(tag: u16) -> Option<&'static str> {
    Some(match tag {
        NAME => "name",
        VERSION => "version",
        ARCH => "arch",
        DEPENDS => "depends",
        PROVIDES => "provides",
        CONFLICTS => "conflicts",
        REPLACES => "replaces",
        TRIGGER => "trigger",
        DESC => "desc",
        URL => "url",
        LICENSE => "license",
        BUILD_TIME => "build_time",
        INSTALLED_SIZE => "installed_size",
        OPTDEPENDS => "optdepends",
        _ => return None,
    })
}
