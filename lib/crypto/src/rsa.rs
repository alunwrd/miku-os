// RSA: encryption for the TLS 1.2 key exchange, verification for signatures
//
// Padding schemes: PKCS#1 v1.5, used by most certificates, and PSS, on newer
// chains and in every TLS 1.3 CertificateVerify.
//
// Verification compares the whole recovered block with one built here.
// Comparing only the digest is the Bleichenbacher e=3 mistake, accepting
// garbage in the padding

use crate::bignum::{bn_from_bytes_be, bn_to_bytes_be, BigNum, MontCtx, MAX_LIMBS};
use crate::primitives::Hash;

pub struct RsaPublicKey {
    pub n:     BigNum,
    pub e:     u32,
    /// Modulus size in bytes: the size of every signature and padded block
    /// under this key
    pub n_len: usize,
}

impl RsaPublicKey {
    pub fn from_parts(n_bytes: &[u8], e_bytes: &[u8]) -> Option<RsaPublicKey> {
        let n = bn_from_bytes_be(n_bytes)?;
        if e_bytes.is_empty() || e_bytes.len() > 4 { return None; }
        let mut e = 0u32;
        for &b in e_bytes { e = (e << 8) | b as u32; }
        // An even or trivial exponent is not a usable key. e=1 would make every
        // signature equal its plaintext
        if e < 3 || e & 1 == 0 { return None; }
        let n_len = n_bytes.iter().position(|&b| b != 0).map(|p| n_bytes.len() - p)?;
        if n_len < 64 { return None; } // below 512 bits is not a usable key
        Some(RsaPublicKey { n, e, n_len })
    }

    pub fn bits(&self) -> usize { crate::bignum::bn_bits(&self.n) }
}

/// s^e mod n, big-endian into `out` at the modulus width
fn public_op(key: &RsaPublicKey, sig: &[u8], out: &mut [u8; MAX_LIMBS * 4]) -> Option<usize> {
    let k = key.n_len;
    if k > MAX_LIMBS * 4 { return None; }
    // RFC 8017: a signature is exactly the modulus length and, as an integer,
    // below the modulus
    if sig.len() != k { return None; }
    let s = bn_from_bytes_be(sig)?;
    if crate::bignum::bn_cmp(&s, &key.n) != core::cmp::Ordering::Less { return None; }
    let ctx = MontCtx::new(&key.n)?;
    let m = ctx.pow_u32(&s, key.e);
    bn_to_bytes_be(&m, &mut out[..k]);
    Some(k)
}

/// The DER DigestInfo header that PKCS#1 v1.5 puts in front of the hash
fn digest_info_prefix(h: Hash) -> &'static [u8] {
    match h {
        Hash::Sha1 => &[0x30,0x21,0x30,0x09,0x06,0x05,0x2b,0x0e,0x03,0x02,0x1a,0x05,0x00,0x04,0x14],
        Hash::Sha256 => &[0x30,0x31,0x30,0x0d,0x06,0x09,0x60,0x86,0x48,0x01,0x65,0x03,0x04,0x02,0x01,0x05,0x00,0x04,0x20],
        Hash::Sha384 => &[0x30,0x41,0x30,0x0d,0x06,0x09,0x60,0x86,0x48,0x01,0x65,0x03,0x04,0x02,0x02,0x05,0x00,0x04,0x30],
        Hash::Sha512 => &[0x30,0x51,0x30,0x0d,0x06,0x09,0x60,0x86,0x48,0x01,0x65,0x03,0x04,0x02,0x03,0x05,0x00,0x04,0x40],
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for i in 0..a.len() { diff |= a[i] ^ b[i]; }
    diff == 0
}

/// RSASSA-PKCS1-v1_5 verification. Rebuilds the expected block
/// `00 01 FF..FF 00 || DigestInfo || H` and compares all of it
pub fn rsa_pkcs1v15_verify(key: &RsaPublicKey, hash: Hash, msg: &[u8], sig: &[u8]) -> bool {
    let mut em = [0u8; MAX_LIMBS * 4];
    let k = match public_op(key, sig, &mut em) { Some(k) => k, None => return false };

    let mut hbuf = [0u8; 64];
    let digest = hash.digest(msg, &mut hbuf);
    let prefix = digest_info_prefix(hash);
    let t_len = prefix.len() + digest.len();
    // At least eight padding octets, per RFC 8017 §9.2
    if k < t_len + 11 { return false; }

    let mut expect = [0u8; MAX_LIMBS * 4];
    expect[0] = 0x00;
    expect[1] = 0x01;
    let ps_len = k - t_len - 3;
    for i in 0..ps_len { expect[2 + i] = 0xFF; }
    expect[2 + ps_len] = 0x00;
    expect[3 + ps_len..3 + ps_len + prefix.len()].copy_from_slice(prefix);
    expect[3 + ps_len + prefix.len()..3 + ps_len + t_len].copy_from_slice(digest);

    ct_eq(&em[..k], &expect[..k])
}

/// MGF1, the mask generation for PSS: hash of the seed with a big-endian
/// counter, concatenated until long enough
fn mgf1(hash: Hash, seed: &[u8], out: &mut [u8]) {
    let hlen = hash.len();
    let mut counter: u32 = 0;
    let mut pos = 0;
    let mut input = [0u8; 68];
    input[..seed.len()].copy_from_slice(seed);
    while pos < out.len() {
        input[seed.len()..seed.len() + 4].copy_from_slice(&counter.to_be_bytes());
        let mut hbuf = [0u8; 64];
        let d = hash.digest(&input[..seed.len() + 4], &mut hbuf);
        let take = (out.len() - pos).min(hlen);
        out[pos..pos + take].copy_from_slice(&d[..take]);
        pos += take;
        counter += 1;
    }
}

/// RSASSA-PSS verification (EMSA-PSS-VERIFY, RFC 8017 §9.1.2). The salt length
/// is recovered from the decoded block, so any salt length verifies
pub fn rsa_pss_verify(key: &RsaPublicKey, hash: Hash, msg: &[u8], sig: &[u8]) -> bool {
    let mut em = [0u8; MAX_LIMBS * 4];
    let k = match public_op(key, sig, &mut em) { Some(k) => k, None => return false };

    let hlen = hash.len();
    let em_bits = key.bits() - 1;
    let em_len = (em_bits + 7) / 8;
    if em_len > k || em_len < hlen + 2 { return false; }
    // The block is right-aligned in the modulus-width buffer
    let em = &mut em[k - em_len..k];

    if em[em_len - 1] != 0xBC { return false; }

    let db_len = em_len - hlen - 1;
    let (masked_db, h) = em.split_at_mut(db_len);
    let h = &h[..hlen];

    let mut db_mask = [0u8; MAX_LIMBS * 4];
    mgf1(hash, h, &mut db_mask[..db_len]);
    for i in 0..db_len { masked_db[i] ^= db_mask[i]; }

    // The unused leading bits of the block must be zero after unmasking
    let unused = 8 * em_len - em_bits;
    if unused > 0 {
        masked_db[0] &= 0xFFu8 >> unused;
    }

    // DB is PS (zeros) || 0x01 || salt
    let sep = match masked_db.iter().position(|&b| b != 0) {
        Some(i) if masked_db[i] == 0x01 => i,
        _ => return false,
    };
    let salt = &masked_db[sep + 1..];

    let mut hbuf = [0u8; 64];
    let m_hash = hash.digest(msg, &mut hbuf);

    let mut mprime = [0u8; 8 + 64 + 64];
    if salt.len() > 64 { return false; }
    mprime[8..8 + hlen].copy_from_slice(m_hash);
    mprime[8 + hlen..8 + hlen + salt.len()].copy_from_slice(salt);
    let mut h2buf = [0u8; 64];
    let hprime = hash.digest(&mprime[..8 + hlen + salt.len()], &mut h2buf);

    ct_eq(h, hprime)
}

pub fn rsa_pkcs1_encrypt(
    key: &RsaPublicKey,
    data: &[u8],
    out: &mut [u8; 256],
    random: fn(&mut [u8]) -> bool,
) -> usize {
    let k = key.n_len;
    if k > 256 || data.len() + 11 > k { return 0; }

    let mut em = [0u8; 256];
    em[0] = 0x00;
    em[1] = 0x02;
    let ps_len = k - 3 - data.len();

    // Padding bytes must be nonzero and unpredictable; they keep two
    // encryptions of one premaster different. A failed draw returns nothing
    let mut i = 0;
    while i < ps_len {
        let mut r = [0u8; 8];
        if !random(&mut r) {
            return 0;
        }
        for &b in r.iter() {
            if b != 0 && i < ps_len {
                em[2 + i] = b;
                i += 1;
            }
        }
    }

    em[2 + ps_len] = 0x00;
    em[3 + ps_len..3 + ps_len + data.len()].copy_from_slice(data);

    let m = match bn_from_bytes_be(&em[..k]) { Some(m) => m, None => return 0 };
    let ctx = match MontCtx::new(&key.n) { Some(c) => c, None => return 0 };
    let c = ctx.pow_u32(&m, key.e);
    bn_to_bytes_be(&c, &mut out[..k]);

    k
}
