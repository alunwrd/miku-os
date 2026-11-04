// X.509 certificate parsing
//
// Only what a TLS client needs for a trust decision: signer, subject, validity,
// key, and the two extensions deciding whether it may sign. Fields borrow from
// the certificate's bytes, since the signature covers that encoding

use crate::der::{self, ctx, ctx_prim, Der, Tlv};

// Object identifiers, as DER content octets, with the dotted form in comments

/// 1.2.840.113549.1.1.1 rsaEncryption
pub const OID_RSA_ENC:      &[u8] = &[0x2A,0x86,0x48,0x86,0xF7,0x0D,0x01,0x01,0x01];
/// 1.2.840.113549.1.1.5 sha1WithRSAEncryption
pub const OID_SHA1_RSA:     &[u8] = &[0x2A,0x86,0x48,0x86,0xF7,0x0D,0x01,0x01,0x05];
/// 1.2.840.113549.1.1.10 RSASSA-PSS
pub const OID_RSA_PSS:      &[u8] = &[0x2A,0x86,0x48,0x86,0xF7,0x0D,0x01,0x01,0x0A];
/// 1.2.840.113549.1.1.11 sha256WithRSAEncryption
pub const OID_SHA256_RSA:   &[u8] = &[0x2A,0x86,0x48,0x86,0xF7,0x0D,0x01,0x01,0x0B];
/// 1.2.840.113549.1.1.12 sha384WithRSAEncryption
pub const OID_SHA384_RSA:   &[u8] = &[0x2A,0x86,0x48,0x86,0xF7,0x0D,0x01,0x01,0x0C];
/// 1.2.840.113549.1.1.13 sha512WithRSAEncryption
pub const OID_SHA512_RSA:   &[u8] = &[0x2A,0x86,0x48,0x86,0xF7,0x0D,0x01,0x01,0x0D];
/// 1.2.840.10045.2.1 id-ecPublicKey
pub const OID_EC_PUBKEY:    &[u8] = &[0x2A,0x86,0x48,0xCE,0x3D,0x02,0x01];
/// 1.2.840.10045.3.1.7 prime256v1 (NIST P-256)
pub const OID_P256:         &[u8] = &[0x2A,0x86,0x48,0xCE,0x3D,0x03,0x01,0x07];
/// 1.3.132.0.34 secp384r1 (NIST P-384)
pub const OID_P384:         &[u8] = &[0x2B,0x81,0x04,0x00,0x22];
/// 1.3.132.0.35 secp521r1 (NIST P-521)
pub const OID_P521:         &[u8] = &[0x2B,0x81,0x04,0x00,0x23];
/// 1.2.840.10045.4.3.2 ecdsa-with-SHA256
pub const OID_ECDSA_SHA256: &[u8] = &[0x2A,0x86,0x48,0xCE,0x3D,0x04,0x03,0x02];
/// 1.2.840.10045.4.3.3 ecdsa-with-SHA384
pub const OID_ECDSA_SHA384: &[u8] = &[0x2A,0x86,0x48,0xCE,0x3D,0x04,0x03,0x03];
/// 1.2.840.10045.4.3.4 ecdsa-with-SHA512
pub const OID_ECDSA_SHA512: &[u8] = &[0x2A,0x86,0x48,0xCE,0x3D,0x04,0x03,0x04];
/// 2.16.840.1.101.3.4.2.1 id-sha256
pub const OID_SHA256:       &[u8] = &[0x60,0x86,0x48,0x01,0x65,0x03,0x04,0x02,0x01];
/// 2.16.840.1.101.3.4.2.2 id-sha384
pub const OID_SHA384:       &[u8] = &[0x60,0x86,0x48,0x01,0x65,0x03,0x04,0x02,0x02];
/// 2.16.840.1.101.3.4.2.3 id-sha512
pub const OID_SHA512:       &[u8] = &[0x60,0x86,0x48,0x01,0x65,0x03,0x04,0x02,0x03];

const OID_EXT_KEY_USAGE:    &[u8] = &[0x55,0x1D,0x0F]; // 2.5.29.15
const OID_EXT_SAN:          &[u8] = &[0x55,0x1D,0x11]; // 2.5.29.17
const OID_EXT_BASIC_CONSTR: &[u8] = &[0x55,0x1D,0x13]; // 2.5.29.19
const OID_ATTR_CN:          &[u8] = &[0x55,0x04,0x03]; // 2.5.4.3 commonName

/// Which digest a signature algorithm uses, and which key type verifies it
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SigAlg {
    RsaPkcs1(Hash),
    RsaPss(Hash),
    Ecdsa(Hash),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hash { Sha1, Sha256, Sha384, Sha512 }

impl Hash {
    pub fn len(&self) -> usize {
        match self { Hash::Sha1 => 20, Hash::Sha256 => 32, Hash::Sha384 => 48, Hash::Sha512 => 64 }
    }
    /// Digest into a 64-byte buffer, returning the written slice
    pub fn digest<'b>(&self, data: &[u8], out: &'b mut [u8; 64]) -> &'b [u8] {
        use crate::crypto;
        match self {
            Hash::Sha1   => { out[..20].copy_from_slice(&crypto::sha1(data));   &out[..20] }
            Hash::Sha256 => { out[..32].copy_from_slice(&crypto::sha256(data)); &out[..32] }
            Hash::Sha384 => { out[..48].copy_from_slice(&crypto::sha384(data)); &out[..48] }
            Hash::Sha512 => { out[..64].copy_from_slice(&crypto::sha512(data)); &out[..64] }
        }
    }
}

/// The key itself, still in the certificate's bytes
#[derive(Clone, Copy)]
pub enum PublicKey<'a> {
    /// modulus and exponent, sign byte already stripped
    Rsa { n: &'a [u8], e: &'a [u8] },
    /// an uncompressed point, `0x04 || X || Y`, and the curve it lies on
    Ec { curve: &'a [u8], point: &'a [u8] },
}

#[derive(Clone, Copy)]
pub struct Cert<'a> {
    /// The whole certificate as received
    pub raw: &'a [u8],
    /// The exact bytes the signature covers, header included
    pub tbs: &'a [u8],
    pub serial: &'a [u8],
    /// Distinguished names as encoded bytes, which chain building compares
    pub issuer: &'a [u8],
    pub subject: &'a [u8],
    pub not_before: u64,
    pub not_after: u64,
    pub key: PublicKey<'a>,
    pub sig_alg: SigAlg,
    /// PSS parameters, when sig_alg is RsaPss and one was encoded
    pub sig_params: Option<&'a [u8]>,
    pub signature: &'a [u8],
    pub is_ca: bool,
    /// pathLenConstraint, when basicConstraints carried one
    pub path_len: Option<u32>,
    /// True when keyUsage is present and forbids certificate signing. Absent
    /// keyUsage means unconstrained
    pub forbids_cert_sign: bool,
    /// subjectAltName, still encoded; `dns_names` walks it
    san: Option<&'a [u8]>,
    /// commonName from the subject, used only without a SAN
    cn: Option<&'a [u8]>,
}

fn oid_to_sig_alg(oid: &[u8]) -> Option<SigAlg> {
    Some(match oid {
        OID_SHA256_RSA   => SigAlg::RsaPkcs1(Hash::Sha256),
        OID_SHA384_RSA   => SigAlg::RsaPkcs1(Hash::Sha384),
        OID_SHA512_RSA   => SigAlg::RsaPkcs1(Hash::Sha512),
        OID_SHA1_RSA     => SigAlg::RsaPkcs1(Hash::Sha1),
        OID_ECDSA_SHA256 => SigAlg::Ecdsa(Hash::Sha256),
        OID_ECDSA_SHA384 => SigAlg::Ecdsa(Hash::Sha384),
        OID_ECDSA_SHA512 => SigAlg::Ecdsa(Hash::Sha512),
        _ => return None,
    })
}

/// RSASSA-PSS names its digest in the parameters, not the OID
fn pss_hash(params: Option<Tlv<'_>>) -> Hash {
    let p = match params { Some(p) if p.tag == der::TAG_SEQUENCE => p, _ => return Hash::Sha1 };
    let mut r = p.reader();
    // [0] hashAlgorithm, defaulted to SHA-1 when absent
    if let Some(h) = r.maybe(ctx(0)) {
        let mut hr = h.reader();
        if let Some(alg) = hr.read() {
            if let Some((oid, _)) = der::algorithm_id(&alg) {
                return match oid {
                    OID_SHA256 => Hash::Sha256,
                    OID_SHA384 => Hash::Sha384,
                    OID_SHA512 => Hash::Sha512,
                    _          => Hash::Sha1,
                };
            }
        }
    }
    Hash::Sha1
}

fn parse_sig_alg<'a>(alg: &Tlv<'a>) -> Option<(SigAlg, Option<&'a [u8]>)> {
    let (oid, params) = der::algorithm_id(alg)?;
    if oid == OID_RSA_PSS {
        return Some((SigAlg::RsaPss(pss_hash(params)), params.map(|p| p.full)));
    }
    Some((oid_to_sig_alg(oid)?, None))
}

/// Days since the Unix epoch for a proleptic Gregorian date, to compare
/// certificate UTC to a Unix timestamp
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn dec2(b: &[u8], i: usize) -> Option<i64> {
    let (a, c) = (*b.get(i)?, *b.get(i + 1)?);
    if !a.is_ascii_digit() || !c.is_ascii_digit() { return None; }
    Some(((a - b'0') as i64) * 10 + (c - b'0') as i64)
}

/// UTCTime is `YYMMDDHHMMSSZ` with a two-digit year; GeneralizedTime has four.
/// RFC 5280 windows two digits to 1950-2049
fn parse_time(t: &Tlv<'_>) -> Option<u64> {
    let s = t.content;
    let (year, rest) = match t.tag {
        der::TAG_UTCTIME => {
            let yy = dec2(s, 0)?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, 2usize)
        }
        der::TAG_GENTIME => {
            let hi = dec2(s, 0)?;
            let lo = dec2(s, 2)?;
            (hi * 100 + lo, 4usize)
        }
        _ => return None,
    };
    let mon  = dec2(s, rest)?;
    let day  = dec2(s, rest + 2)?;
    let hour = dec2(s, rest + 4)?;
    let min  = dec2(s, rest + 6)?;
    // Seconds are optional in GeneralizedTime and mandatory in UTCTime; a
    // missing pair reads as :00
    let sec  = dec2(s, rest + 8).unwrap_or(0);
    if !(1..=12).contains(&mon) || !(1..=31).contains(&day) { return None; }
    let days = days_from_civil(year, mon, day);
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    if secs < 0 { return None; }
    Some(secs as u64)
}

/// Finds `attr` in a Name: SEQUENCE OF RelativeDistinguishedName, each a SET OF
/// AttributeTypeAndValue
fn dn_attr<'a>(name: &'a [u8], attr: &[u8]) -> Option<&'a [u8]> {
    let mut outer = Der::new(name);
    let seq = outer.expect(der::TAG_SEQUENCE)?;
    let mut rdns = seq.reader();
    let mut found = None;
    while !rdns.is_empty() {
        let set = rdns.read()?;
        let mut atvs = set.reader();
        while !atvs.is_empty() {
            let atv = atvs.read()?;
            let mut r = atv.reader();
            let oid = r.expect(der::TAG_OID)?;
            let val = r.read()?;
            // Last match wins; the most specific repeat is conventionally last
            if oid.content == attr { found = Some(val.content); }
        }
    }
    found
}

impl<'a> Cert<'a> {
    pub fn parse(raw: &'a [u8]) -> Option<Cert<'a>> {
        let mut top = Der::new(raw);
        let cert = top.expect(der::TAG_SEQUENCE)?;
        let mut c = cert.reader();

        let tbs_tlv = c.expect(der::TAG_SEQUENCE)?;
        let alg_tlv = c.expect(der::TAG_SEQUENCE)?;
        let sig_bs  = c.expect(der::TAG_BITSTRING)?;
        let signature = sig_bs.as_bitstring()?;
        let (sig_alg, sig_params) = parse_sig_alg(&alg_tlv)?;

        let mut t = tbs_tlv.reader();
        // version [0] EXPLICIT, absent on v1 certificates such as a few old
        // roots
        t.maybe(ctx(0));
        let serial = t.expect(der::TAG_INTEGER)?.content;
        let inner_alg = t.expect(der::TAG_SEQUENCE)?;
        // RFC 5280 §4.1.1.2: inner and outer algorithm must match. Only the
        // inner one is signed
        let (inner_sig, _) = parse_sig_alg(&inner_alg)?;
        if inner_sig != sig_alg { return None; }

        let issuer = t.expect(der::TAG_SEQUENCE)?.full;

        let validity = t.expect(der::TAG_SEQUENCE)?;
        let mut v = validity.reader();
        let not_before = parse_time(&v.read()?)?;
        let not_after  = parse_time(&v.read()?)?;

        let subject = t.expect(der::TAG_SEQUENCE)?.full;

        let spki = t.expect(der::TAG_SEQUENCE)?;
        let mut s = spki.reader();
        let key_alg = s.expect(der::TAG_SEQUENCE)?;
        let key_bs  = s.expect(der::TAG_BITSTRING)?;
        let key_bits = key_bs.as_bitstring()?;
        let (key_oid, key_params) = der::algorithm_id(&key_alg)?;
        let key = if key_oid == OID_RSA_ENC {
            let mut kr = Der::new(key_bits);
            let seq = kr.expect(der::TAG_SEQUENCE)?;
            let mut kseq = seq.reader();
            let n = kseq.expect(der::TAG_INTEGER)?.as_uint()?;
            let e = kseq.expect(der::TAG_INTEGER)?.as_uint()?;
            PublicKey::Rsa { n, e }
        } else if key_oid == OID_EC_PUBKEY {
            let curve = key_params.filter(|p| p.tag == der::TAG_OID)?.content;
            PublicKey::Ec { curve, point: key_bits }
        } else {
            return None;
        };

        // issuerUniqueID [1] and subjectUniqueID [2] are implicit primitives
        t.maybe(ctx_prim(1));
        t.maybe(ctx_prim(2));

        let mut is_ca = false;
        let mut path_len = None;
        let mut forbids_cert_sign = false;
        let mut san = None;
        if let Some(exts) = t.maybe(ctx(3)) {
            let mut er = exts.reader();
            if let Some(list) = er.expect(der::TAG_SEQUENCE) {
                let mut l = list.reader();
                while !l.is_empty() {
                    let ext = match l.read() { Some(e) => e, None => break };
                    let mut x = ext.reader();
                    let oid = match x.expect(der::TAG_OID) { Some(o) => o, None => continue };
                    // critical BOOLEAN DEFAULT FALSE
                    x.maybe(der::TAG_BOOLEAN);
                    let val = match x.expect(der::TAG_OCTETSTRING) { Some(v) => v, None => continue };
                    match oid.content {
                        OID_EXT_BASIC_CONSTR => {
                            let mut br = Der::new(val.content);
                            if let Some(bc) = br.expect(der::TAG_SEQUENCE) {
                                let mut b = bc.reader();
                                if let Some(ca) = b.maybe(der::TAG_BOOLEAN) {
                                    is_ca = ca.content.first().copied().unwrap_or(0) != 0;
                                }
                                if let Some(pl) = b.maybe(der::TAG_INTEGER) {
                                    path_len = pl.as_u32();
                                }
                            }
                        }
                        OID_EXT_KEY_USAGE => {
                            let mut kr = Der::new(val.content);
                            if let Some(ku) = kr.expect(der::TAG_BITSTRING) {
                                // keyCertSign is bit 5, counting from the most
                                // significant bit of the first content octet
                                let bits = &ku.content;
                                let allowed = bits.len() >= 2 && (bits[1] & 0x04) != 0;
                                forbids_cert_sign = !allowed;
                            }
                        }
                        OID_EXT_SAN => san = Some(val.content),
                        _ => {}
                    }
                }
            }
        }

        let cn = dn_attr(subject, OID_ATTR_CN);

        Some(Cert {
            raw, tbs: tbs_tlv.full, serial, issuer, subject,
            not_before, not_after, key, sig_alg, sig_params, signature,
            is_ca, path_len, forbids_cert_sign, san, cn,
        })
    }

    pub fn is_self_signed(&self) -> bool { self.issuer == self.subject }

    /// Whether this certificate names `host`. A subjectAltName, when present,
    /// is the only thing consulted; commonName is a fallback for certificates
    /// with none
    pub fn matches_host(&self, host: &str) -> bool {
        if let Some(san) = self.san {
            let mut r = Der::new(san);
            let seq = match r.expect(der::TAG_SEQUENCE) { Some(s) => s, None => return false };
            let mut names = seq.reader();
            let literal = crate::parse_ipv4(host);
            while !names.is_empty() {
                let gn = match names.read() { Some(g) => g, None => break };
                // dNSName is [2] IMPLICIT IA5String
                if gn.tag == ctx_prim(2) && literal.is_none() && name_matches(gn.content, host) {
                    return true;
                }
                // iPAddress is [7] IMPLICIT OCTET STRING, four octets for v4
                if gn.tag == ctx_prim(7) {
                    if let Some(ip) = literal {
                        if gn.content == ip { return true; }
                    }
                }
            }
            return false;
        }
        match self.cn {
            Some(cn) if crate::parse_ipv4(host).is_none() => name_matches(cn, host),
            _ => false,
        }
    }
}

/// Compares a certificate name to a hostname, case-insensitively. The one
/// wildcard form is a single `*` as the whole left-most label, matching exactly
/// one label
fn name_matches(pattern: &[u8], host: &str) -> bool {
    let host = host.as_bytes();
    if pattern.is_empty() || host.is_empty() { return false; }

    if pattern.len() > 2 && pattern[0] == b'*' && pattern[1] == b'.' {
        let suffix = &pattern[1..]; // ".example.com"
        // The wildcard covers one label, so the host has a dot where the
        // pattern does and none before it
        let dot = match host.iter().position(|&c| c == b'.') { Some(d) => d, None => return false };
        if dot == 0 { return false; }
        return eq_ascii_ci(&host[dot..], suffix);
    }
    eq_ascii_ci(host, pattern)
}

fn eq_ascii_ci(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}
