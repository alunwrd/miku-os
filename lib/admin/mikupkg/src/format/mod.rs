//! Bytes on disk: the container, the index, the installed record, and the
//! journal
//!
//! Arithmetic only. This module knows offsets, byte order, and lengths, and
//! answers one question: do these bytes say what they claim. Judgement is in
//! `trust/`.
//!
//! Everything is little-endian, and only this module knows it.

pub mod container;
pub mod db;
pub mod index;
pub mod journal;
pub mod meta;
pub mod reader;
pub mod table;
pub mod writer;

use core::fmt;

pub use container::{Header, HEADER_LEN, MAGIC};
pub use index::{Index, IndexEntry};
pub use reader::Package;
pub use table::{FileEntry, FileKind, FileTable, RECORD_LEN};

/// What a reader refuses, and why. Each variant names an offset or quantity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    /// Too short to hold what it must hold
    Truncated { need: usize, have: usize },
    /// Not an `.mpk`
    BadMagic,
    /// Ours, from a future
    UnsupportedVersion { found: u16, understood: u16 },
    /// A section that runs past the end of the file, or into another one
    BadSection { what: &'static str, off: u64, len: u64 },
    /// Sections must appear in the order the header declares, so a reader works
    /// forwards
    SectionsOutOfOrder { what: &'static str },
    /// A metadata record whose length runs past the section
    BadRecord { at: usize },
    /// A critical metadata tag this build does not understand. Named, so a
    /// newer mikupkg can be requested
    UnknownCriticalTag { tag: u16, at: usize },
    /// A required metadata tag is absent
    MissingTag { tag: u16 },
    /// A tag appearing twice where it may appear once
    RepeatedTag { tag: u16, at: usize },
    /// The text is not valid for this field
    BadValue { tag: u16, at: usize },
    /// A file table entry pointing outside the payload
    BadExtent { index: usize },
    /// A path a package may not own
    BadPath { index: usize },
    /// File bytes do not hash to what the table says
    WrongHash { index: usize },
    /// A codec byte this build has no decoder for
    UnknownCodec { index: usize, codec: u8 },
    /// More files, or more metadata, than a package may hold
    TooLarge { what: &'static str, count: u64, limit: u64 },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            FormatError::Truncated { need, have } => write!(
                f,
                "the package stops early: {need} bytes are needed and there are {have}"
            ),
            FormatError::BadMagic => write!(f, "this is not a mikupkg package"),
            FormatError::UnsupportedVersion { found, understood } => write!(
                f,
                "the package is format version {found} and this build understands {understood}"
            ),
            FormatError::BadSection { what, off, len } => write!(
                f,
                "the {what} section claims {len} bytes at offset {off}, which is not \
                 inside the file"
            ),
            FormatError::SectionsOutOfOrder { what } => {
                write!(f, "the {what} section is not where the header says it should be")
            }
            FormatError::BadRecord { at } => {
                write!(f, "byte {at}: a metadata record runs past the end of the section")
            }
            FormatError::UnknownCriticalTag { tag, at } => write!(
                f,
                "byte {at}: metadata tag {tag:#06x} is marked critical and this build does \
                 not understand it, so the package cannot be installed safely. A newer \
                 mikupkg will know what it means"
            ),
            FormatError::MissingTag { tag } => {
                write!(f, "the package has no {} in it", crate::meta::tag::tag_name(tag).unwrap_or("?"))
            }
            FormatError::RepeatedTag { tag, at } => write!(
                f,
                "byte {at}: {} appears twice and may appear once",
                crate::meta::tag::tag_name(tag).unwrap_or("?")
            ),
            FormatError::BadValue { tag, at } => write!(
                f,
                "byte {at}: the {} field does not hold something this field may hold",
                crate::meta::tag::tag_name(tag).unwrap_or("?")
            ),
            FormatError::BadExtent { index } => write!(
                f,
                "file {index} claims bytes that are not inside the package's payload"
            ),
            FormatError::BadPath { index } => {
                write!(f, "file {index} has a path a package may not own")
            }
            FormatError::WrongHash { index } => write!(
                f,
                "file {index} does not hash to what the package says it should - the \
                 package is damaged or was changed after it was built"
            ),
            FormatError::UnknownCodec { index, codec } => write!(
                f,
                "file {index} is stored with codec {codec}, which this build cannot read"
            ),
            FormatError::TooLarge { what, count, limit } => {
                write!(f, "the package has {count} {what} and the limit is {limit}")
            }
        }
    }
}

// Little-endian readers
//
// Bounds-checked and returning `Option`, since every byte here came from
// elsewhere

pub fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}
pub fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}
pub fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

pub fn put_u16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
pub fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// The one hash on this machine, from the crate that already owns it.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    mikucrypto::primitives::sha256(data)
}

/// Digest a signature covers: metadata and file table, in that order, behind a
/// label.
///
/// The table holds a SHA-256 per file, a depth-2 Merkle tree, so one signature
/// covers every byte. The label is domain separation against signatures over
/// other structures
pub fn manifest_hash(meta: &[u8], table: &[u8]) -> [u8; 32] {
    let mut h = mikucrypto::primitives::Sha256State::new();
    h.update(MANIFEST_LABEL);
    h.update(&(meta.len() as u64).to_le_bytes());
    h.update(meta);
    h.update(&(table.len() as u64).to_le_bytes());
    h.update(table);
    h.finalize()
}

/// Hashed before the manifest, along with the lengths, so moving bytes between
/// sections changes the digest
pub const MANIFEST_LABEL: &[u8] = b"mikupkg:manifest:v1";
