//! The machine's cryptography
//!
//! SHA-1, SHA-2, HMAC, AES-128 and GCM, HKDF, RSA, ECDSA over the NIST curves,
//! BLAKE2b, Argon2id, and the DER reader signatures come wrapped in.

#![cfg_attr(not(test), no_std)]
#![allow(clippy::needless_range_loop)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod ct;
pub mod primitives;

#[cfg(feature = "alloc")]
pub mod argon2;
#[cfg(feature = "alloc")]
pub mod bignum;
#[cfg(feature = "alloc")]
pub mod blake2b;
#[cfg(feature = "alloc")]
pub mod der;
#[cfg(feature = "alloc")]
pub mod ec;
#[cfg(feature = "alloc")]
pub mod gcm;
#[cfg(feature = "alloc")]
pub mod rsa;
