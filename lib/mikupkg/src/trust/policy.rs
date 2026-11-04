//! Whether a blob is authentic and fresh enough
//!
//! A bad signature means someone produced bytes they had no right to. A stale
//! index means someone is hiding newer ones, which a signature alone cannot
//! detect.
//!
//! Three rules, a subset of TUF without its role hierarchy:
//!
//! - **Rollback.** An index with a serial below the one held is refused.
//! - **Freeze.** An index past its expiry is refused.
//! - **An unknown clock is not a pass.** The report is "cannot tell", as
//!   `verify.rs` treats `now == 0` for certificates.

use core::fmt;

use super::keyring::{Key, KeyId, Keyring};
use super::sig::{Signature, ALG_ECDSA_P256_SHA256};
use crate::name::Name;

/// What the machine concluded. An enum, since "cannot tell" is a real answer
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Signed by a held key entitled to sign it
    Trusted { key: KeyId },
    /// There is no signature at all
    Unsigned,
    /// Signed by an unknown key
    UnknownKey { key: KeyId },
    /// Signed by a key that may not speak for this repository
    OutOfScope { key: KeyId },
    /// Signed by a key outside its validity window
    Expired { key: KeyId },
    /// The signature fails against the key it names
    Forged { key: KeyId },
    /// An algorithm this build cannot verify
    UnknownAlgorithm { alg: u8 },
    /// The envelope could not be read
    Malformed,
}

impl Verdict {
    pub fn is_trusted(&self) -> bool {
        matches!(self, Verdict::Trusted { .. })
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Trusted { key } => write!(f, "signed by {key}"),
            Verdict::Unsigned => write!(f, "not signed"),
            Verdict::UnknownKey { key } => write!(
                f,
                "signed by {key}, which is not a key this machine holds - add it to \
                 /etc/mikupkg/keys if you meant to trust it"
            ),
            Verdict::OutOfScope { key } => {
                write!(f, "signed by {key}, which may not sign for this repository")
            }
            Verdict::Expired { key } => write!(f, "signed by {key}, outside its validity"),
            Verdict::Forged { key } => write!(
                f,
                "the signature does not check out against {key} - these bytes were \
                 changed after they were signed"
            ),
            Verdict::UnknownAlgorithm { alg } => {
                write!(f, "signed with algorithm {alg}, which this build cannot check")
            }
            Verdict::Malformed => write!(f, "the signature could not be read at all"),
        }
    }
}

/// Checks a detached signature over `digest`: a package's manifest hash, or an
/// index's digest of everything above its signature. Never the bytes themselves
pub fn check(
    digest: &[u8; 32],
    envelope: Option<&[u8]>,
    keyring: &Keyring,
    repo: Option<&Name>,
    now: u64,
) -> Verdict {
    let envelope = match envelope {
        None => return Verdict::Unsigned,
        Some(e) => e,
    };
    let sig = match Signature::parse(envelope) {
        Ok(s) => s,
        Err(_) => return Verdict::Malformed,
    };
    if sig.alg != ALG_ECDSA_P256_SHA256 {
        return Verdict::UnknownAlgorithm { alg: sig.alg };
    }
    let key: &Key = match keyring.find(&sig.key_id) {
        None => return Verdict::UnknownKey { key: sig.key_id },
        Some(k) => k,
    };
    if !key.covers(repo) {
        return Verdict::OutOfScope { key: key.id };
    }
    if !key.valid_at(now) {
        return Verdict::Expired { key: key.id };
    }

    // The certificate verifier
    let curve = mikunet::ec::Curve::p256();
    if mikunet::ec::ecdsa_verify(&curve, &key.point, digest, &sig.bytes) {
        Verdict::Trusted { key: key.id }
    } else {
        Verdict::Forged { key: key.id }
    }
}

/// What the machine concluded about *when*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Newer than what is held and not past its expiry
    Current,
    /// Older than the index on this machine
    Rollback { held: u64, offered: u64 },
    /// Past its expiry. Somebody has stopped you seeing anything newer
    Expired { expired_at: u64, now: u64 },
    /// The clock is not set, so there is no answer, which is not a pass
    UnknownClock,
}

impl Freshness {
    pub fn is_current(&self) -> bool {
        matches!(self, Freshness::Current)
    }
}

impl fmt::Display for Freshness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Freshness::Current => write!(f, "current"),
            Freshness::Rollback { held, offered } => write!(
                f,
                "the index offered is serial {offered} and this machine already has \
                 {held}. An index cannot go backwards; somebody is serving an old one"
            ),
            Freshness::Expired { expired_at, now } => write!(
                f,
                "the index expired at {expired_at} and it is now {now}. Either the \
                 repository has stopped publishing or something is keeping newer \
                 indexes from reaching this machine"
            ),
            Freshness::UnknownClock => write!(
                f,
                "this machine does not know the time, so it cannot tell whether the \
                 index is current. Set the clock (ntpdate) before trusting it"
            ),
        }
    }
}

/// The rollback and freeze rules together
pub fn freshness(held_serial: u64, offered_serial: u64, not_after: u64, now: u64) -> Freshness {
    if offered_serial < held_serial {
        return Freshness::Rollback { held: held_serial, offered: offered_serial };
    }
    if now == 0 {
        return Freshness::UnknownClock;
    }
    if not_after != 0 && now > not_after {
        return Freshness::Expired { expired_at: not_after, now };
    }
    Freshness::Current
}
