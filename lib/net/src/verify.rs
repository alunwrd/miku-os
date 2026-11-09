// Certificate chain validation
//
// Checks, in order:
// * the leaf names the host (SAN, wildcards, no CN when a SAN exists)
// * every certificate is inside its validity window
// * each signature verifies under its issuer's key
// * every issuer is a CA allowed to sign certificates
// * the top of the chain is signed by a key from the trust store
//
// Anything the peer sends is only evidence; the anchors are what it cannot
// supply. A store that failed to load is empty, an empty store anchors nothing,
// and such a chain is refused through the ordinary path.

extern crate alloc;
use alloc::vec::Vec;

use crate::x509::{Cert, PublicKey, SigAlg};
use crate::rsa::{RsaPublicKey, rsa_pkcs1v15_verify, rsa_pss_verify};
use crate::ec::{curve_by_oid, ecdsa_verify};

/// Longest chain followed. Real chains are two or three long
const MAX_DEPTH: usize = 8;

/// What a connection is checked against: the anchors and `insecure`, passed to
/// each connection so the choice is visible at the call site
pub struct Trust {
    /// All roots, concatenated. One allocation, since the store is read often
    /// and never edited
    der: Vec<u8>,
    index: Vec<(u32, u32)>,
    /// Accepts a chain that failed, for a local test server with a self-signed
    /// certificate. Every connection under it says so
    pub insecure: bool,
}

impl Trust {
    /// An empty store: every chain fails unless `insecure` is set
    pub fn empty() -> Self {
        Self { der: Vec::new(), index: Vec::new(), insecure: false }
    }

    /// Reads every CERTIFICATE block from a PEM bundle. Anything else is
    /// skipped, since bundles carry comments and subject lines
    pub fn from_pem(pem: &[u8]) -> Self {
        const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
        const END: &[u8] = b"-----END CERTIFICATE-----";

        let mut der: Vec<u8> = Vec::new();
        let mut index: Vec<(u32, u32)> = Vec::new();
        let mut pos = 0usize;

        while let Some(b) = find(pem, BEGIN, pos) {
            let body_start = b + BEGIN.len();
            let e = match find(pem, END, body_start) {
                Some(e) => e,
                None => break,
            };
            if let Some(bytes) = b64_decode(&pem[body_start..e]) {
                let off = der.len() as u32;
                der.extend_from_slice(&bytes);
                index.push((off, bytes.len() as u32));
            }
            pos = e + END.len();
        }

        Self { der, index, insecure: false }
    }

    pub fn len(&self) -> usize { self.index.len() }
    pub fn is_empty(&self) -> bool { self.index.is_empty() }

    pub fn roots(&self) -> impl Iterator<Item = &[u8]> {
        self.index
            .iter()
            .map(move |&(off, len)| &self.der[off as usize..off as usize + len as usize])
    }

    /// The anchor that signed `cert`, if the store holds one
    fn anchor_for(&self, cert: &Cert<'_>) -> bool {
        for der in self.roots() {
            let root = match Cert::parse(der) { Some(r) => r, None => continue };
            if root.subject != cert.issuer { continue; }
            if verify_signature(&root.key, cert.sig_alg, cert.tbs, cert.signature) {
                return true;
            }
        }
        false
    }
}

/// Trust anchors on a MikuOS system. A file, so a developer can inspect and
/// edit them; a program that cannot read it refuses to connect
#[cfg(feature = "userspace")]
pub const SYSTEM_TRUST: &str = "/etc/ssl/certs/ca-bundle.pem";

#[cfg(feature = "userspace")]
pub fn system_trust() -> Result<Trust, &'static str> {
    load_trust(SYSTEM_TRUST)
}

/// Reads a bundle from the filesystem. Reports which failure happened: a
/// missing file or a file with no roots
#[cfg(feature = "userspace")]
pub fn load_trust(path: &str) -> Result<Trust, &'static str> {
    let mut cpath = [0u8; 256];
    let n = path.len().min(cpath.len() - 1);
    cpath[..n].copy_from_slice(&path.as_bytes()[..n]);

    let mut size: usize = 0;
    let buf = unsafe { miku::miku_read_file(cpath.as_ptr(), &mut size as *mut usize) };
    if buf.is_null() {
        return Err("no such file");
    }
    let pem = unsafe { core::slice::from_raw_parts(buf, size) };
    let t = Trust::from_pem(pem);
    unsafe { miku::miku_free(buf) };
    if t.is_empty() {
        return Err("no certificates in it");
    }
    Ok(t)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() { return None; }
    let mut i = from;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle { return Some(i); }
        i += 1;
    }
    None
}

/// Base64, skipping bytes outside the alphabet, such as the newlines in a PEM
/// body
fn b64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    if !miku_base::base64::decode_lines(input, |b| out.push(b)) || out.is_empty() {
        return None;
    }
    Some(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VerifyError {
    /// Could not be parsed
    Malformed,
    /// Valid, but not for this host
    NameMismatch,
    Expired,
    NotYetValid,
    /// The chain ran out without reaching anything we trust
    UnknownCa,
    /// A signature in the chain did not verify
    BadSignature,
    /// A middle certificate is not a CA or may not sign
    NotACa,
    /// Well-formed, but uses something not implemented here
    Unsupported,
    /// The server sent no certificate
    Empty,
}

impl VerifyError {
    /// TLS alert for this failure, which tells the other end whether to fix its
    /// chain, clock, or name
    pub fn alert(&self) -> u8 {
        match self {
            VerifyError::Expired | VerifyError::NotYetValid => 45, // certificate_expired
            VerifyError::UnknownCa                          => 48, // unknown_ca
            VerifyError::NameMismatch                       => 42, // bad_certificate
            VerifyError::Unsupported                        => 43, // unsupported_certificate
            _                                               => 42, // bad_certificate
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VerifyError::Malformed    => "certificate could not be parsed",
            VerifyError::NameMismatch => "certificate is not valid for this host",
            VerifyError::Expired      => "certificate has expired",
            VerifyError::NotYetValid  => "certificate is not valid yet",
            VerifyError::UnknownCa    => "chain does not reach a trusted root",
            VerifyError::BadSignature => "signature in the chain is invalid",
            VerifyError::NotACa       => "issuer is not permitted to sign certificates",
            VerifyError::Unsupported  => "unsupported key or signature algorithm",
            VerifyError::Empty        => "no certificate was sent",
        }
    }
}

/// Verifies `sig` over `msg` under `key`, using the algorithm the certificate
/// or CertificateVerify named
pub fn verify_signature(key: &PublicKey, alg: SigAlg, msg: &[u8], sig: &[u8]) -> bool {
    match (key, alg) {
        (PublicKey::Rsa { n, e }, SigAlg::RsaPkcs1(h)) => {
            match RsaPublicKey::from_parts(n, e) {
                Some(k) => rsa_pkcs1v15_verify(&k, h, msg, sig),
                None => false,
            }
        }
        (PublicKey::Rsa { n, e }, SigAlg::RsaPss(h)) => {
            match RsaPublicKey::from_parts(n, e) {
                Some(k) => rsa_pss_verify(&k, h, msg, sig),
                None => false,
            }
        }
        (PublicKey::Ec { curve, point }, SigAlg::Ecdsa(h)) => {
            let c = match curve_by_oid(curve) {
                Some(c) => c,
                None => { netlog_err!("tls: ECDSA on an unsupported curve"); return false; }
            };
            // The digest may be wider than the curve; ECDSA truncates it
            let mut hbuf = [0u8; 64];
            let digest = h.digest(msg, &mut hbuf);
            ecdsa_verify(&c, point, digest, sig)
        }
        _ => false,
    }
}

/// Whether `now` is inside the certificate's validity window
fn time_ok(c: &Cert<'_>, now: u64) -> Result<(), VerifyError> {
    // An unset clock cannot tell expired from valid, and refusing every
    // connection would be worse. The RTC is read at boot, so this is rare, and
    // it is logged
    if now == 0 { return Ok(()); }
    if now < c.not_before { return Err(VerifyError::NotYetValid); }
    if now > c.not_after  { return Err(VerifyError::Expired); }
    Ok(())
}

/// Checks a chain as the server sent it: `certs[0]` is the leaf, followed by
/// intermediates in no guaranteed order. Returns the depth walked, for logging
pub fn verify_chain(certs: &[Vec<u8>], host: &str, now: u64, trust: &Trust) -> Result<usize, VerifyError> {
    if certs.is_empty() { return Err(VerifyError::Empty); }

    let leaf = Cert::parse(&certs[0]).ok_or(VerifyError::Malformed)?;

    if !leaf.matches_host(host) {
        return Err(VerifyError::NameMismatch);
    }
    time_ok(&leaf, now)?;

    // A server may send its root, which is never trusted from here. `used`
    // keeps a cycle from walking the same certificate twice
    let mut used = [false; 16];
    used[0] = true;

    let mut current = leaf;
    for depth in 0..MAX_DEPTH {
        // Anchored? Then the chain is complete and nothing below matters
        if trust.anchor_for(&current) {
            return Ok(depth);
        }

        // Otherwise find the next link among what the server sent
        let mut next: Option<(usize, Cert<'_>)> = None;
        for (i, der) in certs.iter().enumerate().skip(1) {
            if i < used.len() && used[i] { continue; }
            let cand = match Cert::parse(der) { Some(c) => c, None => continue };
            if cand.subject != current.issuer { continue; }
            if !verify_signature(&cand.key, current.sig_alg, current.tbs, current.signature) {
                continue;
            }
            next = Some((i, cand));
            break;
        }

        let (idx, issuer) = match next {
            Some(v) => v,
            // No remaining signer and not anchored: a broken chain, or a
            // self-signed end
            None => return Err(VerifyError::UnknownCa),
        };

        // An intermediate must be a CA. Otherwise any leaf signed by a public
        // CA could sign for any domain, the original basicConstraints flaw
        if !issuer.is_ca || issuer.forbids_cert_sign {
            return Err(VerifyError::NotACa);
        }
        // pathLenConstraint counts the intermediates allowed below this one
        if let Some(pl) = issuer.path_len {
            if depth as u32 > pl { return Err(VerifyError::NotACa); }
        }
        time_ok(&issuer, now)?;

        if idx < used.len() { used[idx] = true; }
        current = issuer;
    }

    Err(VerifyError::UnknownCa)
}

/// The whole check, with the escape hatch and logging in one place
pub fn check(certs: &[Vec<u8>], host: &str, trust: &Trust) -> Result<(), VerifyError> {
    // No clock: dates are skipped and logged. The RTC is read at boot, so this
    // is rare
    let now = crate::plat::now_unix().unwrap_or(0);
    if now == 0 {
        netlog_err!("tls: clock not set - certificate validity dates not checked");
    }
    match verify_chain(certs, host, now, trust) {
        Ok(depth) => {
            netlog!("tls: certificate chain verified for {} ({} link{})",
                host, depth, if depth == 1 { "" } else { "s" });
            Ok(())
        }
        Err(e) => {
            if trust.insecure {
                netlog_err!("tls: {} - accepted anyway, verification is off", e.as_str());
                Ok(())
            } else {
                netlog_err!("tls: {}", e.as_str());
                Err(e)
            }
        }
    }
}
