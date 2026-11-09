//! Keys the machine believes and what each may say
//!
//! One key per file, as a fixed-size record, since `mikupkg-apply` reads these
//! as root. A directory listing and a length check are the whole parser.
//!
//! Each key has a scope, the repository it may sign an index for, and a
//! validity window, for retiring it.

use alloc::vec::Vec;
use core::fmt;

use crate::format::{put_u64, u64_at, FormatError};
use crate::name::Name;

pub const MAGIC: &[u8; 8] = b"MIKUKEY\0";
pub const RECORD_LEN: usize = 128;
pub const VERSION: u16 = 1;

/// An uncompressed P-256 point
pub const POINT_LEN: usize = 65;

/// Eight bytes of `SHA-256("mikupkg:key:v1" || point)`. A hint for which key to
/// try; the signature decides
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(pub [u8; 8]);

impl KeyId {
    pub fn of(point: &[u8]) -> KeyId {
        let mut h = mikucrypto::primitives::Sha256State::new();
        h.update(b"mikupkg:key:v1");
        h.update(point);
        let digest = h.finalize();
        let mut id = [0u8; 8];
        id.copy_from_slice(&digest[..8]);
        KeyId(id)
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// One trusted key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub id: KeyId,
    pub alg: u8,
    pub point: [u8; POINT_LEN],
    /// Unix seconds. Zero means always
    pub not_before: u64,
    /// Unix seconds. Zero means no end, as for a root key
    pub not_after: u64,
    /// Which repository this key may sign an index for. `None` is any
    pub scope: Option<Name>,
}

impl Key {
    pub fn parse(b: &[u8]) -> Result<Key, FormatError> {
        if b.len() < RECORD_LEN {
            return Err(FormatError::Truncated { need: RECORD_LEN, have: b.len() });
        }
        if &b[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = crate::format::u16_at(b, 8).unwrap();
        if version != VERSION {
            return Err(FormatError::UnsupportedVersion { found: version, understood: VERSION });
        }
        let alg = b[10];
        let not_before = u64_at(b, 16).unwrap();
        let not_after = u64_at(b, 24).unwrap();

        let mut point = [0u8; POINT_LEN];
        point.copy_from_slice(&b[32..32 + POINT_LEN]);
        if point[0] != 0x04 {
            return Err(FormatError::BadValue { tag: 0, at: 32 });
        }

        let scope_len = b[97] as usize;
        let scope = if scope_len == 0 {
            None
        } else {
            let text = core::str::from_utf8(&b[98..98 + scope_len.min(26)])
                .map_err(|_| FormatError::BadValue { tag: 0, at: 98 })?;
            Some(Name::parse(text).map_err(|_| FormatError::BadValue { tag: 0, at: 98 })?)
        };

        Ok(Key {
            id: KeyId::of(&point),
            alg,
            point,
            not_before,
            not_after,
            scope,
        })
    }

    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        b[..8].copy_from_slice(MAGIC);
        crate::format::put_u16(&mut b, 8, VERSION);
        b[10] = self.alg;
        put_u64(&mut b, 16, self.not_before);
        put_u64(&mut b, 24, self.not_after);
        b[32..32 + POINT_LEN].copy_from_slice(&self.point);
        if let Some(scope) = &self.scope {
            b[97] = scope.len() as u8;
            b[98..98 + scope.len()].copy_from_slice(scope.as_bytes());
        }
        b
    }

    /// May this key speak for `repo`?
    pub fn covers(&self, repo: Option<&Name>) -> bool {
        match (&self.scope, repo) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(mine), Some(theirs)) => mine == theirs,
        }
    }

    /// Whether `now` is inside the window. A `now` of zero means the time is
    /// unknown and fails
    pub fn valid_at(&self, now: u64) -> bool {
        if now == 0 {
            return false;
        }
        if self.not_before != 0 && now < self.not_before {
            return false;
        }
        if self.not_after != 0 && now > self.not_after {
            return false;
        }
        true
    }
}

/// Every key the machine holds.
#[derive(Debug, Clone, Default)]
pub struct Keyring {
    keys: Vec<Key>,
}

impl Keyring {
    pub fn new() -> Keyring {
        Keyring { keys: Vec::new() }
    }

    pub fn add(&mut self, key: Key) {
        // One entry per key
        if !self.keys.iter().any(|k| k.id == key.id) {
            self.keys.push(key);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    pub fn find(&self, id: &KeyId) -> Option<&Key> {
        self.keys.iter().find(|k| k.id == *id)
    }
}
