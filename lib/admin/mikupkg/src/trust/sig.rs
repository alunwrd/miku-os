//! Signature envelope
//!
//! Names the key to try and the algorithm in fixed-width fields, the same for a
//! package or an index. Sixteen header bytes, then the signature, since
//! `mikupkg-apply` reads this as root.

use super::keyring::KeyId;
use crate::format::{put_u16, u16_at, FormatError};

pub const MAGIC: &[u8; 4] = b"MSG\0";
pub const HEADER_LEN: usize = 16;

/// ECDSA over P-256 with SHA-256, as TLS already verifies
pub const ALG_ECDSA_P256_SHA256: u8 = 1;

/// A P-521 signature is the longest the signer can produce
pub const MAX_SIG: usize = 144;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub key_id: KeyId,
    pub alg: u8,
    /// The DER `SEQUENCE { r, s }`
    pub bytes: alloc::vec::Vec<u8>,
}

impl Signature {
    pub fn parse(b: &[u8]) -> Result<Signature, FormatError> {
        if b.len() < HEADER_LEN {
            return Err(FormatError::Truncated { need: HEADER_LEN, have: b.len() });
        }
        if &b[..4] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let alg = b[4];
        let len = u16_at(b, 6).unwrap() as usize;
        if len > MAX_SIG {
            return Err(FormatError::TooLarge {
                what: "bytes of signature",
                count: len as u64,
                limit: MAX_SIG as u64,
            });
        }
        let end = HEADER_LEN + len;
        if b.len() < end {
            return Err(FormatError::Truncated { need: end, have: b.len() });
        }
        let mut key_id = [0u8; 8];
        key_id.copy_from_slice(&b[8..16]);
        Ok(Signature {
            key_id: KeyId(key_id),
            alg,
            bytes: b[HEADER_LEN..end].to_vec(),
        })
    }

    pub fn encode(&self) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec![0u8; HEADER_LEN + self.bytes.len()];
        out[..4].copy_from_slice(MAGIC);
        out[4] = self.alg;
        // 5 reserved
        put_u16(&mut out, 6, self.bytes.len() as u16);
        out[8..16].copy_from_slice(&self.key_id.0);
        out[HEADER_LEN..].copy_from_slice(&self.bytes);
        out
    }
}
