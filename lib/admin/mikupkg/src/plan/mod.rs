//! A transaction as a bounded, ordered list of operations
//!
//! Handed to the root half of the installer as fixed-shape records.
//! `mikupkg-apply` parses this and nothing else; anything with a grammar has
//! been read by the unprivileged half.
//!
//! Carries a hash and size per file, which the root half rechecks.

pub mod conflict;
pub mod encode;
pub mod order;
pub mod space;

use alloc::string::String;
use alloc::vec::Vec;

use crate::apply::root::RootResult;
use crate::meta::{PackageMeta, Trigger};
use crate::name::Name;

/// Which package and which file a file's bytes come from. An index, so a
/// transaction over a thousand files holds one file's contents at a time;
/// `MAX_VNODES` is shared across the machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentKey {
    pub package: usize,
    pub file: usize,
}

/// Where the applier gets the bytes. A trait, so one plan applies from opened
/// containers on the host, a staging directory on the machine, or a table in a
/// test
pub trait Source {
    fn content(&self, key: ContentKey) -> RootResult<Vec<u8>>;
}

/// One thing that will happen to the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Created if absent. Several packages declaring `bin` is normal
    Dir { path: String, mode: u16, uid: u16, gid: u16 },
    File {
        path: String,
        mode: u16,
        uid: u16,
        gid: u16,
        key: ContentKey,
        size: u64,
        /// Rechecked by the applier against the bytes it receives
        sha256: [u8; 32],
    },
    Symlink { path: String, target: String, uid: u16, gid: u16 },
    /// Removes something, keeping it until the transaction commits
    Remove { path: String },
}

impl Op {
    pub fn path(&self) -> &str {
        match self {
            Op::Dir { path, .. }
            | Op::File { path, .. }
            | Op::Symlink { path, .. }
            | Op::Remove { path } => path,
        }
    }

    /// Bytes this will add to the filesystem
    pub fn size(&self) -> u64 {
        match self {
            Op::File { size, .. } => *size,
            _ => 0,
        }
    }

    pub fn is_removal(&self) -> bool {
        matches!(self, Op::Remove { .. })
    }
}

/// Everything one transaction will do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Monotonic, and the name of this transaction in the journal
    pub serial: u64,
    pub ops: Vec<Op>,
    /// What will be recorded as installed afterwards
    pub installs: Vec<PackageMeta>,
    /// What will stop being recorded
    pub removals: Vec<Name>,
    /// Owed once everything is in place, deduplicated
    pub triggers: Vec<Trigger>,
}

impl Plan {
    pub fn new(serial: u64) -> Plan {
        Plan { serial, ..Plan::default() }
    }

    /// Bytes the filesystem needs. High on purpose: a replaced file counts
    /// whole, since the old copy stays until commit
    pub fn bytes_needed(&self) -> u64 {
        self.ops.iter().map(|o| o.size()).sum()
    }

    /// Orders the operations and collects the triggers. Called once, by the
    /// planner
    pub fn finish(&mut self) {
        order::sort(&mut self.ops);
        self.triggers.sort();
        self.triggers.dedup();
    }
}
