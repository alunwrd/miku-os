//! Whether a blob is authentic and fresh enough. The only place in this crate
//! that knows what a key is
//!
//! Signature check is `mikucrypto::ec::ecdsa_verify` with the same SHA-256 as TLS.

pub mod keyring;
pub mod policy;
pub mod sig;

pub use keyring::{Key, KeyId, Keyring};
pub use policy::{check, freshness, Freshness, Verdict};
pub use sig::Signature;

use crate::format::{FormatError, Package};
use crate::name::Name;

/// Both halves of "may I install this", in order. The signature covers metadata
/// and the file table, not the payload, so the per-file SHA-256s must be
/// checked too
pub fn package(
    pkg: &Package<'_>,
    keyring: &Keyring,
    repo: Option<&Name>,
    now: u64,
) -> Result<Verdict, FormatError> {
    // Contents first: bytes not matching the table are damaged whoever signed
    // them
    pkg.verify_all()?;
    Ok(check(&pkg.manifest_hash(), pkg.signature(), keyring, repo, now))
}
