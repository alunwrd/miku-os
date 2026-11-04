//! One installed package's record
//!
//! The container's metadata and file table verbatim, plus a trailer.
//! `format/meta.rs` and `format/table.rs` read and write both.
//!
//! One file per package: fewer directory entries for the dentry cache, and an
//! update is one atomic rename.
//!
//! No path-to-package index. `mikupkg owner` scanning a thousand records takes
//! milliseconds.

use alloc::vec::Vec;

use super::{put_u16, put_u32, put_u64, u16_at, u32_at, u64_at, FormatError, FileTable};

pub const MAGIC: &[u8; 8] = b"MIKUREC\0";
pub const VERSION: u16 = 1;
const HEAD: usize = 32;

/// Why this package is on the machine, for orphan tracking: a dependency goes
/// with the last thing needing it, an explicit install stays
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Somebody asked for it
    Explicit,
    /// Installed because something else needed it
    Dependency,
}

impl Reason {
    fn code(self) -> u8 {
        match self {
            Reason::Explicit => 0,
            Reason::Dependency => 1,
        }
    }
    fn from_code(c: u8) -> Option<Reason> {
        match c {
            0 => Some(Reason::Explicit),
            1 => Some(Reason::Dependency),
            _ => None,
        }
    }
}

/// What the machine remembers about one installed package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The metadata section, as the container held it
    pub meta: Vec<u8>,
    pub files: FileTable,
    pub reason: Reason,
    pub install_time: u64,
    /// Of the source container, so `mikupkg verify` can tell a reinstall from a
    /// different package with the same version
    pub container_sha256: [u8; 32],
}

impl Installed {
    pub fn encode(&self) -> Vec<u8> {
        let table = self.files.encode();
        let mut out = alloc::vec![0u8; HEAD];
        out[..8].copy_from_slice(MAGIC);
        put_u16(&mut out, 8, VERSION);
        out[10] = self.reason.code();
        put_u32(&mut out, 12, self.meta.len() as u32);
        put_u32(&mut out, 16, table.len() as u32);
        put_u32(&mut out, 20, self.files.entries.len() as u32);
        put_u64(&mut out, 24, self.install_time);
        out.extend_from_slice(&self.container_sha256);
        out.extend_from_slice(&self.meta);
        out.extend_from_slice(&table);
        out
    }

    pub fn parse(b: &[u8]) -> Result<Installed, FormatError> {
        if b.len() < HEAD + 32 {
            return Err(FormatError::Truncated { need: HEAD + 32, have: b.len() });
        }
        if &b[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = u16_at(b, 8).unwrap();
        if version != VERSION {
            return Err(FormatError::UnsupportedVersion { found: version, understood: VERSION });
        }
        let reason = Reason::from_code(b[10]).ok_or(FormatError::BadValue { tag: 0, at: 10 })?;
        let meta_len = u32_at(b, 12).unwrap() as usize;
        let table_len = u32_at(b, 16).unwrap() as usize;
        let count = u32_at(b, 20).unwrap() as usize;
        let install_time = u64_at(b, 24).unwrap();

        let mut container_sha256 = [0u8; 32];
        container_sha256.copy_from_slice(&b[HEAD..HEAD + 32]);

        let m0 = HEAD + 32;
        let m1 = m0 + meta_len;
        let t1 = m1 + table_len;
        if t1 > b.len() {
            return Err(FormatError::Truncated { need: t1, have: b.len() });
        }

        // Parsed with no payload: an installed record describes files on the
        // filesystem. `u64::MAX` means do not judge the extents; paths and
        // hashes are checked as usual
        let files = FileTable::parse(&b[m1..t1], count, u64::MAX)?;

        Ok(Installed {
            meta: b[m0..m1].to_vec(),
            files,
            reason,
            install_time,
            container_sha256,
        })
    }
}
