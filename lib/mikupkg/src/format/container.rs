//! The `.mpk` header and its section offsets
//!
//! Header, metadata, file table, and signature precede the payload, so a reader
//! checks the signature and table within a few kilobytes and hashes the payload
//! as it writes it. tar interleaves metadata with data, so it cannot do this.
//!
//! No checksum in the header. Every byte is covered by the structural check,
//! the signature over metadata and table, or a per-file SHA-256.

use super::{u16_at, u32_at, u64_at, put_u16, put_u32, put_u64, FormatError};

pub const MAGIC: &[u8; 8] = b"MIKUPKG\0";
pub const HEADER_LEN: usize = 64;
/// The only format version. A higher one is refused by number
pub const FORMAT_VERSION: u16 = 1;

/// Most files per package; more would also exhaust the vnode table
pub const MAX_FILES: u32 = 4096;
/// Largest metadata section
pub const MAX_META: u32 = 1 << 20;

/// The package carries file contents. A package of only directories and
/// symlinks has no payload
pub const FLAG_HAS_PAYLOAD: u16 = 1 << 0;

/// Where everything is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format_version: u16,
    pub flags: u16,
    pub meta_off: u32,
    pub meta_len: u32,
    pub table_off: u32,
    pub table_len: u32,
    pub sig_off: u32,
    pub sig_len: u32,
    pub payload_off: u64,
    pub payload_len: u64,
    pub file_count: u32,
}

impl Header {
    /// Reads a header and checks its offsets. `total` is the container's
    /// length; every section is checked against it here
    pub fn parse(b: &[u8], total: u64) -> Result<Header, FormatError> {
        if b.len() < HEADER_LEN {
            return Err(FormatError::Truncated { need: HEADER_LEN, have: b.len() });
        }
        if &b[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let format_version = u16_at(b, 8).unwrap();
        if format_version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion {
                found: format_version,
                understood: FORMAT_VERSION,
            });
        }

        let h = Header {
            format_version,
            flags: u16_at(b, 10).unwrap(),
            meta_off: u32_at(b, 12).unwrap(),
            meta_len: u32_at(b, 16).unwrap(),
            table_off: u32_at(b, 20).unwrap(),
            table_len: u32_at(b, 24).unwrap(),
            sig_off: u32_at(b, 28).unwrap(),
            sig_len: u32_at(b, 32).unwrap(),
            payload_off: u64_at(b, 36).unwrap(),
            payload_len: u64_at(b, 44).unwrap(),
            file_count: u32_at(b, 52).unwrap(),
        };
        h.check(total)?;
        Ok(h)
    }

    /// Whether this describes a file. In order: each section fits, the sections
    /// follow the declared order, none overlap, and counts are within limits
    fn check(&self, total: u64) -> Result<(), FormatError> {
        let fits = |what: &'static str, off: u64, len: u64| -> Result<(), FormatError> {
            match off.checked_add(len) {
                Some(end) if end <= total && off >= HEADER_LEN as u64 => Ok(()),
                // An empty section at offset zero means none, and need not be
                // inside
                _ if len == 0 && off == 0 => Ok(()),
                _ => Err(FormatError::BadSection { what, off, len }),
            }
        };

        fits("metadata", self.meta_off as u64, self.meta_len as u64)?;
        fits("file table", self.table_off as u64, self.table_len as u64)?;
        fits("signature", self.sig_off as u64, self.sig_len as u64)?;
        fits("payload", self.payload_off, self.payload_len)?;

        if self.meta_len == 0 {
            return Err(FormatError::MissingTag { tag: crate::meta::tag::NAME });
        }
        if self.meta_len > MAX_META {
            return Err(FormatError::TooLarge {
                what: "bytes of metadata",
                count: self.meta_len as u64,
                limit: MAX_META as u64,
            });
        }
        if self.file_count > MAX_FILES {
            return Err(FormatError::TooLarge {
                what: "files",
                count: self.file_count as u64,
                limit: MAX_FILES as u64,
            });
        }

        // The declared order, as a streaming reader walks: metadata, table,
        // signature, payload
        let mut cursor = HEADER_LEN as u64;
        for (what, off, len) in [
            ("metadata", self.meta_off as u64, self.meta_len as u64),
            ("file table", self.table_off as u64, self.table_len as u64),
            ("signature", self.sig_off as u64, self.sig_len as u64),
        ] {
            if len == 0 {
                continue;
            }
            if off < cursor {
                return Err(FormatError::SectionsOutOfOrder { what });
            }
            cursor = off + len;
        }
        if self.payload_len != 0 && self.payload_off < cursor {
            return Err(FormatError::SectionsOutOfOrder { what: "payload" });
        }

        let table_needed = (self.file_count as u64) * super::RECORD_LEN as u64;
        if (self.table_len as u64) < table_needed {
            return Err(FormatError::BadSection {
                what: "file table",
                off: self.table_off as u64,
                len: self.table_len as u64,
            });
        }

        Ok(())
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[..8].copy_from_slice(MAGIC);
        put_u16(&mut b, 8, self.format_version);
        put_u16(&mut b, 10, self.flags);
        put_u32(&mut b, 12, self.meta_off);
        put_u32(&mut b, 16, self.meta_len);
        put_u32(&mut b, 20, self.table_off);
        put_u32(&mut b, 24, self.table_len);
        put_u32(&mut b, 28, self.sig_off);
        put_u32(&mut b, 32, self.sig_len);
        put_u64(&mut b, 36, self.payload_off);
        put_u64(&mut b, 44, self.payload_len);
        put_u32(&mut b, 52, self.file_count);
        // 56..64 reserved and required to be zero
        b
    }

    pub fn has_payload(&self) -> bool {
        self.flags & FLAG_HAS_PAYLOAD != 0
    }
}
