//! The order of operations
//!
//! - A directory before anything in it. Sorting paths as text does this: `bin`
//!   before `bin/mayterm`.
//! - Removals deepest first: a directory must be empty, so reverse path order.
//! - Removals after everything else: an upgrade removing a file the new version
//!   also ships must not delete what it placed.
//!
//! No topological sort. Package dependencies are the resolver's; only paths are
//! left here, and paths are a tree.

use alloc::vec::Vec;

use super::Op;

pub fn sort(ops: &mut Vec<Op>) {
    ops.sort_by(|a, b| {
        let phase = |o: &Op| match o {
            // Directories first, for their contents
            Op::Dir { .. } => 0,
            Op::File { .. } | Op::Symlink { .. } => 1,
            Op::Remove { .. } => 2,
        };
        match phase(a).cmp(&phase(b)) {
            core::cmp::Ordering::Equal => {
                if a.is_removal() {
                    // Deepest first, so a directory is empty when its removal
                    // comes
                    b.path().cmp(a.path())
                } else {
                    a.path().cmp(b.path())
                }
            }
            other => other,
        }
    });
}
