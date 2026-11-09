//! Whether a transaction fits
//!
//! Asked before anything is downloaded. The estimate is high: a replaced file
//! counts whole, since the old copy stays until commit.

use core::fmt;

use crate::apply::root::Root;
use crate::plan::Plan;

/// What the filesystem had to say about room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Enough { needed: u64, free: u64 },
    Short { needed: u64, free: u64 },
    /// The filesystem did not say; reported, not read as plenty
    Unknown { needed: u64 },
}

impl Space {
    pub fn is_enough(&self) -> bool {
        matches!(self, Space::Enough { .. })
    }
}

impl fmt::Display for Space {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Space::Enough { needed, free } => {
                write!(f, "{needed} bytes needed, {free} free")
            }
            Space::Short { needed, free } => write!(
                f,
                "{needed} bytes are needed and {free} are free. Nothing has been \
                 downloaded or written"
            ),
            Space::Unknown { needed } => write!(
                f,
                "{needed} bytes are needed and the filesystem would not say how much \
                 is free"
            ),
        }
    }
}

/// A floor added to the plan's estimate for the journal, backup directory
/// entries, and filesystem rounding
pub const MARGIN: u64 = 256 * 1024;

pub fn check<R: Root + ?Sized>(plan: &Plan, root: &R) -> Space {
    let needed = plan.bytes_needed().saturating_add(MARGIN);
    match root.free_space() {
        None => Space::Unknown { needed },
        Some(free) if free >= needed => Space::Enough { needed, free },
        Some(free) => Space::Short { needed, free },
    }
}
