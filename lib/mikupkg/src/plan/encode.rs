//! The only thing the root half parses
//!
//! `mikupkg-apply` runs with all capabilities and reads this alone: no TLS,
//! DNS, index, configuration language, or container framing. Fixed-width
//! lengths and blobs, which are the two sections `format/` reads with bounds
//! checks.
//!
//! A plan carries the package's own signed evidence, not trusted hashes:
//! metadata, file table, and the signature over both. The root half verifies
//! the signature against a keyring it reads itself, then hashes each staged
//! file against the table.
//!
//! Contents live beside the plan, one numbered file per table entry, so the
//! root half never reads a path from an archive.

use alloc::string::String;
use alloc::vec::Vec;

use crate::format::db::Reason;
use crate::format::{put_u16, put_u32, put_u64, u16_at, u32_at, u64_at, FormatError};
use crate::name::Name;

pub const MAGIC: &[u8; 8] = b"MIKUPLAN";
pub const VERSION: u16 = 1;
const HEAD: usize = 32;
const ITEM_HEAD: usize = 48;

/// Most packages per transaction
pub const MAX_ITEMS: u32 = 256;
/// Metadata and a file table for one package, the ceiling on what is handed to
/// a root process
pub const MAX_BLOB: u32 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Install,
    Remove,
}

impl Verb {
    fn code(self) -> u8 {
        match self {
            Verb::Install => 0,
            Verb::Remove => 1,
        }
    }
    fn from_code(c: u8) -> Option<Verb> {
        match c {
            0 => Some(Verb::Install),
            1 => Some(Verb::Remove),
            _ => None,
        }
    }
}

/// One package, and the evidence for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub verb: Verb,
    pub reason: Reason,
    pub name: Name,
    /// The container's metadata section, byte for byte
    pub meta: Vec<u8>,
    /// Its file table, byte for byte: what the signature covers and what staged
    /// files are hashed against
    pub table: Vec<u8>,
    /// The signature envelope, or empty for an unsigned package
    pub signature: Vec<u8>,
    pub container_sha256: [u8; 32],
}

/// What the unprivileged half staged, and what it is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedPlan {
    pub serial: u64,
    pub time: u64,
    pub items: Vec<Item>,
}

impl StagedPlan {
    /// Where item `i`'s file `j` was staged, relative to the root. Numbered,
    /// since an archive's paths are untrusted; the root half hashes `0007`
    /// before consulting the table
    pub fn staged_at(stage_dir: &str, serial: u64, item: usize, file: usize) -> String {
        alloc::format!("{stage_dir}/{serial}/{item:03}/{file:04}")
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = alloc::vec![0u8; HEAD];
        out[..8].copy_from_slice(MAGIC);
        put_u16(&mut out, 8, VERSION);
        put_u64(&mut out, 12, self.serial);
        put_u64(&mut out, 20, self.time);
        put_u32(&mut out, 28, self.items.len() as u32);

        for item in &self.items {
            let mut head = [0u8; ITEM_HEAD];
            head[0] = item.verb.code();
            head[1] = match item.reason {
                Reason::Explicit => 0,
                Reason::Dependency => 1,
            };
            put_u16(&mut head, 2, item.name.len() as u16);
            put_u32(&mut head, 4, item.meta.len() as u32);
            put_u32(&mut head, 8, item.table.len() as u32);
            put_u16(&mut head, 12, item.signature.len() as u16);
            head[16..48].copy_from_slice(&item.container_sha256);
            out.extend_from_slice(&head);
            out.extend_from_slice(item.name.as_bytes());
            out.extend_from_slice(&item.meta);
            out.extend_from_slice(&item.table);
            out.extend_from_slice(&item.signature);
        }
        out
    }

    /// Reads a plan, checking every length before using any of it
    pub fn parse(b: &[u8]) -> Result<StagedPlan, FormatError> {
        if b.len() < HEAD {
            return Err(FormatError::Truncated { need: HEAD, have: b.len() });
        }
        if &b[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = u16_at(b, 8).unwrap();
        if version != VERSION {
            return Err(FormatError::UnsupportedVersion { found: version, understood: VERSION });
        }
        let serial = u64_at(b, 12).unwrap();
        let time = u64_at(b, 20).unwrap();
        let count = u32_at(b, 28).unwrap();
        if count > MAX_ITEMS {
            return Err(FormatError::TooLarge {
                what: "packages in one plan",
                count: count as u64,
                limit: MAX_ITEMS as u64,
            });
        }

        let mut at = HEAD;
        let mut items = Vec::with_capacity(count as usize);
        for _ in 0..count {
            if at + ITEM_HEAD > b.len() {
                return Err(FormatError::BadRecord { at });
            }
            let head = &b[at..at + ITEM_HEAD];
            let verb = Verb::from_code(head[0]).ok_or(FormatError::BadRecord { at })?;
            let reason = match head[1] {
                0 => Reason::Explicit,
                1 => Reason::Dependency,
                _ => return Err(FormatError::BadRecord { at }),
            };
            let name_len = u16_at(head, 2).unwrap() as usize;
            let meta_len = u32_at(head, 4).unwrap();
            let table_len = u32_at(head, 8).unwrap();
            let sig_len = u16_at(head, 12).unwrap() as usize;
            if meta_len > MAX_BLOB || table_len > MAX_BLOB {
                return Err(FormatError::TooLarge {
                    what: "bytes of evidence for one package",
                    count: meta_len.max(table_len) as u64,
                    limit: MAX_BLOB as u64,
                });
            }
            let mut container_sha256 = [0u8; 32];
            container_sha256.copy_from_slice(&head[16..48]);

            let mut p = at + ITEM_HEAD;
            let take = |p: &mut usize, n: usize| -> Result<&[u8], FormatError> {
                let end = p.checked_add(n).ok_or(FormatError::BadRecord { at })?;
                let slice = b.get(*p..end).ok_or(FormatError::BadRecord { at })?;
                *p = end;
                Ok(slice)
            };
            let name_bytes = take(&mut p, name_len)?;
            let name = Name::parse(
                core::str::from_utf8(name_bytes).map_err(|_| FormatError::BadRecord { at })?,
            )
            .map_err(|_| FormatError::BadRecord { at })?;
            let meta = take(&mut p, meta_len as usize)?.to_vec();
            let table = take(&mut p, table_len as usize)?.to_vec();
            let signature = take(&mut p, sig_len)?.to_vec();

            items.push(Item {
                verb,
                reason,
                name,
                meta,
                table,
                signature,
                container_sha256,
            });
            at = p;
        }

        if at != b.len() {
            // Trailing bytes are refused
            return Err(FormatError::BadRecord { at });
        }
        Ok(StagedPlan { serial, time, items })
    }
}
