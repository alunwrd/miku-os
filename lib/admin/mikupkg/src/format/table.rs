//! The file table: one fixed-size record per file and a region of paths
//!
//! Fixed-size because the root half of the installer reads it. Paths follow the
//! records so a record stays constant in size.
//!
//! Each record has a SHA-256 of the file's content, so one signature covers the
//! payload, and `mikupkg verify` can compare an installed file.

use alloc::string::String;
use alloc::vec::Vec;

use super::{put_u16, put_u32, put_u64, u16_at, u32_at, u64_at, FormatError};
use crate::meta::path;

/// One record of 64 bytes:
///
/// ```text
///  0  u8  kind          1  u8  codec
///  2  u16 mode          4  u16 uid        6  u16 gid
///  8  u32 size         12  u32 stored
/// 16  u64 payload_rel
/// 24  u8  sha256[32]
/// 56  u8  flags        57  u8  path_len   58  u16 path_off
/// 60  u32 reserved
/// ```
pub const RECORD_LEN: usize = 64;

/// What the entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Contents are in the payload
    File,
    /// No contents; mode and ownership only
    Directory,
    /// The target is in the payload, as a file's contents would be, and
    /// verified the same way
    Symlink,
}

impl FileKind {
    fn code(self) -> u8 {
        match self {
            FileKind::File => 0,
            FileKind::Directory => 1,
            FileKind::Symlink => 2,
        }
    }
    fn from_code(c: u8) -> Option<FileKind> {
        match c {
            0 => Some(FileKind::File),
            1 => Some(FileKind::Directory),
            2 => Some(FileKind::Symlink),
            _ => None,
        }
    }
}

/// The only codec so far. Packages are made on the build host, so MikuOS needs
/// only the receiving half
pub const CODEC_NONE: u8 = 0;

/// Set on a default shipped under a `default.d`-class directory, so
/// `mikupkg verify` can say "changed" instead of "damaged"
pub const ENTRY_DEFAULT_CONFIG: u8 = 1 << 0;

/// The kernel keeps 64 bytes of a symlink target on the vnode
/// (`kernel/fs/vfs/core/links.rs`). The writer refuses a longer one
pub const MAX_SYMLINK_TARGET: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub kind: FileKind,
    pub codec: u8,
    pub mode: u16,
    pub uid: u16,
    pub gid: u16,
    /// The content's length once decoded
    pub size: u32,
    /// Size in the payload; equal to `size` under `CODEC_NONE`
    pub stored: u32,
    /// Where it starts, counted from the payload's own offset
    pub payload_rel: u64,
    pub sha256: [u8; 32],
    pub flags: u8,
}

impl FileEntry {
    pub fn is_default_config(&self) -> bool {
        self.flags & ENTRY_DEFAULT_CONFIG != 0
    }
}

/// Every entry, in the order they appear in the payload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileTable {
    pub entries: Vec<FileEntry>,
}

impl FileTable {
    /// Reads a table, checking each entry against the payload and the path
    /// rules, so no later caller has to
    pub fn parse(b: &[u8], count: usize, payload_len: u64) -> Result<FileTable, FormatError> {
        let records_len = count * RECORD_LEN;
        if b.len() < records_len {
            return Err(FormatError::Truncated { need: records_len, have: b.len() });
        }

        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let r = &b[i * RECORD_LEN..(i + 1) * RECORD_LEN];

            let kind = FileKind::from_code(r[0])
                .ok_or(FormatError::BadRecord { at: i * RECORD_LEN })?;
            let codec = r[1];
            let size = u32_at(r, 8).unwrap();
            let stored = u32_at(r, 12).unwrap();
            let payload_rel = u64_at(r, 16).unwrap();
            let path_len = r[57] as usize;
            let path_off = u16_at(r, 58).unwrap() as usize;

            // The path region is everything after the records
            let region = &b[records_len..];
            let path_bytes = region
                .get(path_off..path_off + path_len)
                .ok_or(FormatError::BadRecord { at: i * RECORD_LEN })?;
            let path = core::str::from_utf8(path_bytes)
                .map_err(|_| FormatError::BadPath { index: i })?;
            let checked = if kind == FileKind::Directory {
                path::check_dir(path)
            } else {
                path::check(path)
            };
            checked.map_err(|_| FormatError::BadPath { index: i })?;

            // Where the bytes are, if there are any
            match kind {
                FileKind::Directory => {
                    if size != 0 || stored != 0 {
                        return Err(FormatError::BadExtent { index: i });
                    }
                }
                FileKind::File | FileKind::Symlink => {
                    let end = payload_rel
                        .checked_add(stored as u64)
                        .ok_or(FormatError::BadExtent { index: i })?;
                    if end > payload_len {
                        return Err(FormatError::BadExtent { index: i });
                    }
                    if kind == FileKind::Symlink && size as usize > MAX_SYMLINK_TARGET {
                        return Err(FormatError::BadExtent { index: i });
                    }
                }
            }

            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(&r[24..56]);

            entries.push(FileEntry {
                path: String::from(path),
                kind,
                codec,
                mode: u16_at(r, 2).unwrap(),
                uid: u16_at(r, 4).unwrap(),
                gid: u16_at(r, 6).unwrap(),
                size,
                stored,
                payload_rel,
                sha256,
                flags: r[56],
            });
        }

        Ok(FileTable { entries })
    }

    /// Records first, then paths. The inverse of `parse`
    pub fn encode(&self) -> Vec<u8> {
        let records_len = self.entries.len() * RECORD_LEN;
        let paths_len: usize = self.entries.iter().map(|e| e.path.len()).sum();
        let mut out = alloc::vec![0u8; records_len + paths_len];

        let mut path_at = 0usize;
        for (i, e) in self.entries.iter().enumerate() {
            let base = i * RECORD_LEN;
            {
                let r = &mut out[base..base + RECORD_LEN];
                r[0] = e.kind.code();
                r[1] = e.codec;
                put_u16(r, 2, e.mode);
                put_u16(r, 4, e.uid);
                put_u16(r, 6, e.gid);
                put_u32(r, 8, e.size);
                put_u32(r, 12, e.stored);
                put_u64(r, 16, e.payload_rel);
                r[24..56].copy_from_slice(&e.sha256);
                r[56] = e.flags;
                r[57] = e.path.len() as u8;
                put_u16(r, 58, path_at as u16);
            }
            out[records_len + path_at..records_len + path_at + e.path.len()]
                .copy_from_slice(e.path.as_bytes());
            path_at += e.path.len();
        }
        out
    }
}
