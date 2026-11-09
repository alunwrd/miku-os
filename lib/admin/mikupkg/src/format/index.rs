//! A repository index: every package offered, and when the list was made
//!
//! Each entry carries the package's whole metadata block, so the resolver works
//! offline, and `mikupkg info` answers for a package not installed.
//!
//! A signature proves who made an index, not which one. The serial exposes a
//! replay of an old signed index, and the validity window stops a freeze. Rules
//! in `trust/policy.rs`.

use alloc::string::String;
use alloc::vec::Vec;

use super::{put_u16, put_u32, put_u64, u16_at, u32_at, u64_at, FormatError};
use crate::name::Name;

pub const MAGIC: &[u8; 8] = b"MIKUIDX\0";
pub const HEADER_LEN: usize = 64;
pub const VERSION: u16 = 1;
/// The fixed part of an entry
pub const ENTRY_HEAD: usize = 48;

/// Most packages in an index held in a 128 MiB heap
pub const MAX_ENTRIES: u32 = 8192;

pub const DIGEST_LABEL: &[u8] = b"mikupkg:index:v1";

/// One package a repository offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// Of the container, to check a download before opening it
    pub sha256: [u8; 32],
    pub size: u64,
    /// Where it is, relative to the repository's URL
    pub path: String,
    /// The package's metadata section, byte for byte, so the resolver does not
    /// need the container
    pub meta: Vec<u8>,
}

/// Everything a repository offers, and when it said so
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub repo: Name,
    /// Strictly increasing. Going backwards is a rollback attack
    pub serial: u64,
    pub not_before: u64,
    /// Zero means no expiry, which a real repository should not publish
    pub not_after: u64,
    pub entries: Vec<IndexEntry>,
    /// The detached signature envelope, if it carries one
    pub signature: Option<Vec<u8>>,
}

impl Index {
    /// What a signature over this index covers. Field by field, since a digest
    /// defined by exclusion changes with the layout. Lengths are included so
    /// moving bytes between fields changes the digest
    pub fn digest(&self) -> [u8; 32] {
        let mut h = mikucrypto::primitives::Sha256State::new();
        h.update(DIGEST_LABEL);
        h.update(&self.serial.to_le_bytes());
        h.update(&self.not_before.to_le_bytes());
        h.update(&self.not_after.to_le_bytes());
        h.update(&(self.repo.len() as u64).to_le_bytes());
        h.update(self.repo.as_bytes());
        let body = self.encode_entries();
        h.update(&(body.len() as u64).to_le_bytes());
        h.update(&body);
        h.finalize()
    }

    pub fn find(&self, path: &str) -> Option<&IndexEntry> {
        self.entries.iter().find(|e| e.path == path)
    }

    fn encode_entries(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for e in &self.entries {
            let mut head = [0u8; ENTRY_HEAD];
            head[..32].copy_from_slice(&e.sha256);
            put_u64(&mut head, 32, e.size);
            put_u32(&mut head, 40, e.meta.len() as u32);
            put_u16(&mut head, 44, e.path.len() as u16);
            out.extend_from_slice(&head);
            out.extend_from_slice(e.path.as_bytes());
            out.extend_from_slice(&e.meta);
        }
        out
    }

    pub fn encode(&self) -> Vec<u8> {
        let repo = self.repo.as_bytes();
        let body = self.encode_entries();
        let sig = self.signature.as_deref().unwrap_or(&[]);

        let repo_off = HEADER_LEN;
        let entries_off = repo_off + repo.len();
        let sig_off = entries_off + body.len();

        let mut b = alloc::vec![0u8; sig_off + sig.len()];
        b[..8].copy_from_slice(MAGIC);
        put_u16(&mut b, 8, VERSION);
        put_u32(&mut b, 12, self.entries.len() as u32);
        put_u64(&mut b, 16, self.serial);
        put_u64(&mut b, 24, self.not_before);
        put_u64(&mut b, 32, self.not_after);
        put_u32(&mut b, 40, repo.len() as u32);
        put_u32(&mut b, 44, entries_off as u32);
        put_u32(&mut b, 48, body.len() as u32);
        put_u32(&mut b, 52, if sig.is_empty() { 0 } else { sig_off as u32 });
        put_u32(&mut b, 56, sig.len() as u32);

        b[repo_off..repo_off + repo.len()].copy_from_slice(repo);
        b[entries_off..entries_off + body.len()].copy_from_slice(&body);
        b[sig_off..].copy_from_slice(sig);
        b
    }

    pub fn parse(b: &[u8]) -> Result<Index, FormatError> {
        if b.len() < HEADER_LEN {
            return Err(FormatError::Truncated { need: HEADER_LEN, have: b.len() });
        }
        if &b[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = u16_at(b, 8).unwrap();
        if version != VERSION {
            return Err(FormatError::UnsupportedVersion { found: version, understood: VERSION });
        }

        let count = u32_at(b, 12).unwrap();
        if count > MAX_ENTRIES {
            return Err(FormatError::TooLarge {
                what: "packages in one index",
                count: count as u64,
                limit: MAX_ENTRIES as u64,
            });
        }
        let serial = u64_at(b, 16).unwrap();
        let not_before = u64_at(b, 24).unwrap();
        let not_after = u64_at(b, 32).unwrap();
        let repo_len = u32_at(b, 40).unwrap() as usize;
        let entries_off = u32_at(b, 44).unwrap() as usize;
        let entries_len = u32_at(b, 48).unwrap() as usize;
        let sig_off = u32_at(b, 52).unwrap() as usize;
        let sig_len = u32_at(b, 56).unwrap() as usize;

        let section = |what: &'static str, off: usize, len: usize| -> Result<&[u8], FormatError> {
            b.get(off..off + len)
                .ok_or(FormatError::BadSection { what, off: off as u64, len: len as u64 })
        };

        let repo_bytes = section("repository name", HEADER_LEN, repo_len)?;
        let repo = Name::parse(
            core::str::from_utf8(repo_bytes).map_err(|_| FormatError::BadValue { tag: 0, at: HEADER_LEN })?,
        )
        .map_err(|_| FormatError::BadValue { tag: 0, at: HEADER_LEN })?;

        let body = section("entries", entries_off, entries_len)?;
        let signature = if sig_len == 0 {
            None
        } else {
            Some(section("signature", sig_off, sig_len)?.to_vec())
        };

        let mut entries = Vec::with_capacity(count as usize);
        let mut at = 0usize;
        for _ in 0..count {
            if at + ENTRY_HEAD > body.len() {
                return Err(FormatError::BadRecord { at: entries_off + at });
            }
            let head = &body[at..at + ENTRY_HEAD];
            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(&head[..32]);
            let size = u64_at(head, 32).unwrap();
            let meta_len = u32_at(head, 40).unwrap() as usize;
            let path_len = u16_at(head, 44).unwrap() as usize;

            let p0 = at + ENTRY_HEAD;
            let m0 = p0 + path_len;
            let m1 = m0 + meta_len;
            if m1 > body.len() {
                return Err(FormatError::BadRecord { at: entries_off + at });
            }
            let path = core::str::from_utf8(&body[p0..m0])
                .map_err(|_| FormatError::BadRecord { at: entries_off + p0 })?;

            entries.push(IndexEntry {
                sha256,
                size,
                path: String::from(path),
                meta: body[m0..m1].to_vec(),
            });
            at = m1;
        }
        if at != body.len() {
            // Trailing bytes inside the entries section are refused
            return Err(FormatError::BadRecord { at: entries_off + at });
        }

        Ok(Index {
            repo,
            serial,
            not_before,
            not_after,
            entries,
            signature,
        })
    }
}
