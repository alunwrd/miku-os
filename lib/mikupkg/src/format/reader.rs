//! Reads a `.mpk`
//!
//! Answers whether the bytes say what they claim: the header describes a file,
//! the table points inside the payload, each path may be owned, each file
//! hashes as the table says. Whether to trust it is in `trust/`.
//!
//! Content is verified on the way out. `open` reads a few kilobytes, and only a
//! caller wanting the bytes pays for hashing.

use alloc::vec::Vec;

use super::meta as meta_codec;
use super::{manifest_hash, sha256, FormatError, FileKind, FileTable, Header};
use crate::meta::PackageMeta;

/// A package, borrowed. `Debug` prints a line instead of the payload
pub struct Package<'a> {
    bytes: &'a [u8],
    header: Header,
    table: FileTable,
}

impl<'a> Package<'a> {
    /// Check the shape of a package and read its file table.
    pub fn open(bytes: &'a [u8]) -> Result<Package<'a>, FormatError> {
        let header = Header::parse(bytes, bytes.len() as u64)?;
        let table_bytes = &bytes[header.table_off as usize
            ..header.table_off as usize + header.table_len as usize];
        let table = FileTable::parse(
            table_bytes,
            header.file_count as usize,
            header.payload_len,
        )?;
        Ok(Package { bytes, header, table })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn table(&self) -> &FileTable {
        &self.table
    }

    pub fn meta_bytes(&self) -> &'a [u8] {
        let off = self.header.meta_off as usize;
        &self.bytes[off..off + self.header.meta_len as usize]
    }

    pub fn table_bytes(&self) -> &'a [u8] {
        let off = self.header.table_off as usize;
        &self.bytes[off..off + self.header.table_len as usize]
    }

    /// The detached signature, or None. Unsigned is legal, as a developer's
    /// build or a fresh clone's `builder` output
    pub fn signature(&self) -> Option<&'a [u8]> {
        if self.header.sig_len == 0 {
            return None;
        }
        let off = self.header.sig_off as usize;
        Some(&self.bytes[off..off + self.header.sig_len as usize])
    }

    /// What a signature over this package is a signature *of*.
    pub fn manifest_hash(&self) -> [u8; 32] {
        manifest_hash(self.meta_bytes(), self.table_bytes())
    }

    /// Reads the metadata. `note` gets every non-critical tag this build does
    /// not understand, with its offset
    pub fn meta(&self, note: impl FnMut(u16, usize)) -> Result<PackageMeta, FormatError> {
        meta_codec::decode(self.meta_bytes(), note)
    }

    /// One file's contents, checked against the table's hash. For a symlink,
    /// the target's bytes
    pub fn content(&self, index: usize) -> Result<&'a [u8], FormatError> {
        let e = self
            .table
            .entries
            .get(index)
            .ok_or(FormatError::BadExtent { index })?;
        if e.kind == FileKind::Directory {
            return Ok(&[]);
        }
        if e.codec != super::table::CODEC_NONE {
            return Err(FormatError::UnknownCodec { index, codec: e.codec });
        }

        let start = (self.header.payload_off + e.payload_rel) as usize;
        let end = start + e.stored as usize;
        let raw = self
            .bytes
            .get(start..end)
            .ok_or(FormatError::BadExtent { index })?;

        if sha256(raw) != e.sha256 {
            return Err(FormatError::WrongHash { index });
        }
        Ok(raw)
    }

    /// Hashes every file, for `mikupkg verify <file.mpk>`
    pub fn verify_all(&self) -> Result<(), FormatError> {
        for i in 0..self.table.entries.len() {
            self.content(i)?;
        }
        Ok(())
    }

    /// Every path the package would own, in table order.
    pub fn paths(&self) -> Vec<&str> {
        self.table.entries.iter().map(|e| e.path.as_str()).collect()
    }
}

impl core::fmt::Debug for Package<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Package {{ {} files, {} bytes of payload, {} }}",
            self.header.file_count,
            self.header.payload_len,
            if self.signature().is_some() { "signed" } else { "unsigned" }
        )
    }
}
