//! Post-transaction work
//!
//! A package declares intents by name from a closed vocabulary; here they
//! become a list to run, once, in a fixed order. The handlers are compiled into
//! the root half of the installer, so only code shipped with the machine runs
//! with privilege. An unrecognized intent is refused by name.

use alloc::vec::Vec;

use crate::meta::Trigger;

/// Order when a transaction owes several, fixed so declaration order does not
/// change behavior
pub const ORDER: &[Trigger] = &[Trigger::Ldconfig, Trigger::Services];

/// Collect what a set of packages asks for, deduplicated and ordered.
pub fn owed<'a>(packages: impl Iterator<Item = &'a [Trigger]>) -> Vec<Trigger> {
    let mut seen: Vec<Trigger> = Vec::new();
    for list in packages {
        for t in list {
            if !seen.contains(t) {
                seen.push(*t);
            }
        }
    }
    let mut out = Vec::with_capacity(seen.len());
    for t in ORDER {
        if seen.contains(t) {
            out.push(*t);
        }
    }
    out
}
