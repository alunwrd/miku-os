//! What a package may ask to happen after install
//!
//! A closed vocabulary: a package declares an intent by name, and
//! `mikupkg-apply` holds the table of what each does. An unknown trigger is
//! refused by name, at build time on the host and at plan time on the machine.
//! Adding one takes a variant here and a handler in `apply/trigger.rs`.
//!
//! The root half runs only code shipped with the machine, and metadata says
//! what an install will do.

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum Trigger {
    /// Rebuilds the shared-library cache, named after `ldconfig` in the kernel
    /// shell. `/lib` is baked into the kernel image, so nothing can usefully
    /// ask for this yet
    Ldconfig = 1,
    /// Tells the init daemon to re-read unit files after this package changed
    /// one: `sv scan`
    Services = 2,
}

impl Trigger {
    pub fn from_code(code: u16) -> Option<Trigger> {
        match code {
            1 => Some(Trigger::Ldconfig),
            2 => Some(Trigger::Services),
            _ => None,
        }
    }

    pub fn parse(name: &str) -> Option<Trigger> {
        match name {
            "ldconfig" => Some(Trigger::Ldconfig),
            "services" => Some(Trigger::Services),
            _ => None,
        }
    }

    pub fn code(self) -> u16 {
        self as u16
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Ldconfig => "ldconfig",
            Trigger::Services => "services",
        }
    }

    /// Every trigger, so a refusal can list them
    pub const ALL: &'static [Trigger] = &[Trigger::Ldconfig, Trigger::Services];
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
