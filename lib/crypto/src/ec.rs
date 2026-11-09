// Elliptic curves over prime fields: ECDH for the TLS key exchange, ECDSA for
// certificate and CertificateVerify signatures
//
// Generic over the Montgomery code in `tls_bignum`, at a width wide enough for
// P-521. 37 of the 121 trust-store roots are P-384 or P-521.
//
// A public point is checked against the curve equation before use, against
// invalid-curve attacks. Both ECDSA components are range-checked, since zero
// verifies against anything.

use crate::bignum::{self, Mont};
use crate::der::{self, Der};

/// Wide enough for P-521's 521 bits
const L: usize = 17;
type Num = [u32; L];

// prime256v1 (256-bit)
const P256_P: [u8; 32] = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
const P256_A: [u8; 32] = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFC];
const P256_B: [u8; 32] = [0x5A, 0xC6, 0x35, 0xD8, 0xAA, 0x3A, 0x93, 0xE7, 0xB3, 0xEB, 0xBD, 0x55, 0x76, 0x98, 0x86, 0xBC, 0x65, 0x1D, 0x06, 0xB0, 0xCC, 0x53, 0xB0, 0xF6, 0x3B, 0xCE, 0x3C, 0x3E, 0x27, 0xD2, 0x60, 0x4B];
const P256_GX: [u8; 32] = [0x6B, 0x17, 0xD1, 0xF2, 0xE1, 0x2C, 0x42, 0x47, 0xF8, 0xBC, 0xE6, 0xE5, 0x63, 0xA4, 0x40, 0xF2, 0x77, 0x03, 0x7D, 0x81, 0x2D, 0xEB, 0x33, 0xA0, 0xF4, 0xA1, 0x39, 0x45, 0xD8, 0x98, 0xC2, 0x96];
const P256_GY: [u8; 32] = [0x4F, 0xE3, 0x42, 0xE2, 0xFE, 0x1A, 0x7F, 0x9B, 0x8E, 0xE7, 0xEB, 0x4A, 0x7C, 0x0F, 0x9E, 0x16, 0x2B, 0xCE, 0x33, 0x57, 0x6B, 0x31, 0x5E, 0xCE, 0xCB, 0xB6, 0x40, 0x68, 0x37, 0xBF, 0x51, 0xF5];
const P256_N: [u8; 32] = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xBC, 0xE6, 0xFA, 0xAD, 0xA7, 0x17, 0x9E, 0x84, 0xF3, 0xB9, 0xCA, 0xC2, 0xFC, 0x63, 0x25, 0x51];

// secp384r1 (384-bit)
const P384_P: [u8; 48] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
const P384_A: [u8; 48] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFC];
const P384_B: [u8; 48] = [0xB3, 0x31, 0x2F, 0xA7, 0xE2, 0x3E, 0xE7, 0xE4, 0x98, 0x8E, 0x05, 0x6B, 0xE3, 0xF8, 0x2D, 0x19, 0x18, 0x1D, 0x9C, 0x6E, 0xFE, 0x81, 0x41, 0x12, 0x03, 0x14, 0x08, 0x8F, 0x50, 0x13, 0x87, 0x5A, 0xC6, 0x56, 0x39, 0x8D, 0x8A, 0x2E, 0xD1, 0x9D, 0x2A, 0x85, 0xC8, 0xED, 0xD3, 0xEC, 0x2A, 0xEF];
const P384_GX: [u8; 48] = [0xAA, 0x87, 0xCA, 0x22, 0xBE, 0x8B, 0x05, 0x37, 0x8E, 0xB1, 0xC7, 0x1E, 0xF3, 0x20, 0xAD, 0x74, 0x6E, 0x1D, 0x3B, 0x62, 0x8B, 0xA7, 0x9B, 0x98, 0x59, 0xF7, 0x41, 0xE0, 0x82, 0x54, 0x2A, 0x38, 0x55, 0x02, 0xF2, 0x5D, 0xBF, 0x55, 0x29, 0x6C, 0x3A, 0x54, 0x5E, 0x38, 0x72, 0x76, 0x0A, 0xB7];
const P384_GY: [u8; 48] = [0x36, 0x17, 0xDE, 0x4A, 0x96, 0x26, 0x2C, 0x6F, 0x5D, 0x9E, 0x98, 0xBF, 0x92, 0x92, 0xDC, 0x29, 0xF8, 0xF4, 0x1D, 0xBD, 0x28, 0x9A, 0x14, 0x7C, 0xE9, 0xDA, 0x31, 0x13, 0xB5, 0xF0, 0xB8, 0xC0, 0x0A, 0x60, 0xB1, 0xCE, 0x1D, 0x7E, 0x81, 0x9D, 0x7A, 0x43, 0x1D, 0x7C, 0x90, 0xEA, 0x0E, 0x5F];
const P384_N: [u8; 48] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xC7, 0x63, 0x4D, 0x81, 0xF4, 0x37, 0x2D, 0xDF, 0x58, 0x1A, 0x0D, 0xB2, 0x48, 0xB0, 0xA7, 0x7A, 0xEC, 0xEC, 0x19, 0x6A, 0xCC, 0xC5, 0x29, 0x73];

// secp521r1 (524-bit)
const P521_P: [u8; 66] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
const P521_A: [u8; 66] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFC];
const P521_B: [u8; 66] = [0x00, 0x51, 0x95, 0x3E, 0xB9, 0x61, 0x8E, 0x1C, 0x9A, 0x1F, 0x92, 0x9A, 0x21, 0xA0, 0xB6, 0x85, 0x40, 0xEE, 0xA2, 0xDA, 0x72, 0x5B, 0x99, 0xB3, 0x15, 0xF3, 0xB8, 0xB4, 0x89, 0x91, 0x8E, 0xF1, 0x09, 0xE1, 0x56, 0x19, 0x39, 0x51, 0xEC, 0x7E, 0x93, 0x7B, 0x16, 0x52, 0xC0, 0xBD, 0x3B, 0xB1, 0xBF, 0x07, 0x35, 0x73, 0xDF, 0x88, 0x3D, 0x2C, 0x34, 0xF1, 0xEF, 0x45, 0x1F, 0xD4, 0x6B, 0x50, 0x3F, 0x00];
const P521_GX: [u8; 66] = [0x00, 0xC6, 0x85, 0x8E, 0x06, 0xB7, 0x04, 0x04, 0xE9, 0xCD, 0x9E, 0x3E, 0xCB, 0x66, 0x23, 0x95, 0xB4, 0x42, 0x9C, 0x64, 0x81, 0x39, 0x05, 0x3F, 0xB5, 0x21, 0xF8, 0x28, 0xAF, 0x60, 0x6B, 0x4D, 0x3D, 0xBA, 0xA1, 0x4B, 0x5E, 0x77, 0xEF, 0xE7, 0x59, 0x28, 0xFE, 0x1D, 0xC1, 0x27, 0xA2, 0xFF, 0xA8, 0xDE, 0x33, 0x48, 0xB3, 0xC1, 0x85, 0x6A, 0x42, 0x9B, 0xF9, 0x7E, 0x7E, 0x31, 0xC2, 0xE5, 0xBD, 0x66];
const P521_GY: [u8; 66] = [0x01, 0x18, 0x39, 0x29, 0x6A, 0x78, 0x9A, 0x3B, 0xC0, 0x04, 0x5C, 0x8A, 0x5F, 0xB4, 0x2C, 0x7D, 0x1B, 0xD9, 0x98, 0xF5, 0x44, 0x49, 0x57, 0x9B, 0x44, 0x68, 0x17, 0xAF, 0xBD, 0x17, 0x27, 0x3E, 0x66, 0x2C, 0x97, 0xEE, 0x72, 0x99, 0x5E, 0xF4, 0x26, 0x40, 0xC5, 0x50, 0xB9, 0x01, 0x3F, 0xAD, 0x07, 0x61, 0x35, 0x3C, 0x70, 0x86, 0xA2, 0x72, 0xC2, 0x40, 0x88, 0xBE, 0x94, 0x76, 0x9F, 0xD1, 0x66, 0x50];
const P521_N: [u8; 66] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFA, 0x51, 0x86, 0x87, 0x83, 0xBF, 0x2F, 0x96, 0x6B, 0x7F, 0xCC, 0x01, 0x48, 0xF7, 0x09, 0xA5, 0xD0, 0x3B, 0xB5, 0xC9, 0xB8, 0x89, 0x9C, 0x47, 0xAE, 0xBB, 0x6F, 0xB7, 0x1E, 0x91, 0x38, 0x64, 0x09];

/// Short-Weierstrass curve `y^2 = x^3 + ax + b` over a prime field, with
/// multiplied values in Montgomery form
pub struct Curve {
    /// The field prime
    fp: Mont<L>,
    /// The group order; ECDSA needs this modulus too
    fo: Mont<L>,
    a: Num,
    b: Num,
    gx: Num,
    gy: Num,
    /// Coordinate width in bytes
    pub bytes: usize,
}

fn num(bytes: &[u8]) -> Num {
    bignum::from_be(bytes).expect("curve constant fits")
}

impl Curve {
    fn build(p: &[u8], a: &[u8], b: &[u8], gx: &[u8], gy: &[u8], n: &[u8]) -> Option<Curve> {
        let fp = Mont::<L>::new(&num(p))?;
        let fo = Mont::<L>::new(&num(n))?;
        Some(Curve {
            a:  fp.enter(&num(a)),
            b:  fp.enter(&num(b)),
            gx: fp.enter(&num(gx)),
            gy: fp.enter(&num(gy)),
            bytes: p.len(),
            fp, fo,
        })
    }

    pub fn p256() -> Curve {
        Curve::build(&P256_P, &P256_A, &P256_B, &P256_GX, &P256_GY, &P256_N).unwrap()
    }
    pub fn p384() -> Curve {
        Curve::build(&P384_P, &P384_A, &P384_B, &P384_GX, &P384_GY, &P384_N).unwrap()
    }
    pub fn p521() -> Curve {
        Curve::build(&P521_P, &P521_A, &P521_B, &P521_GX, &P521_GY, &P521_N).unwrap()
    }

    /// `y^2 == x^3 + ax + b`, coordinates in Montgomery form and below the
    /// prime
    fn on_curve(&self, x: &Num, y: &Num) -> bool {
        let f = &self.fp;
        let lhs = f.sqr(y);
        let rhs = f.add(&f.add(&f.mul(&f.sqr(x), x), &f.mul(&self.a, x)), &self.b);
        bignum::cmp(&lhs, &rhs) == core::cmp::Ordering::Equal
    }

    /// Decodes `0x04 || X || Y`, refusing anything not on this curve
    fn decode_point(&self, enc: &[u8]) -> Option<(Num, Num)> {
        let w = self.bytes;
        if enc.len() != 1 + 2 * w || enc[0] != 0x04 { return None; }
        let x: Num = bignum::from_be(&enc[1..1 + w])?;
        let y: Num = bignum::from_be(&enc[1 + w..1 + 2 * w])?;
        // Coordinates must be field elements
        if bignum::cmp(&x, &self.fp.n) != core::cmp::Ordering::Less { return None; }
        if bignum::cmp(&y, &self.fp.n) != core::cmp::Ordering::Less { return None; }
        let xm = self.fp.enter(&x);
        let ym = self.fp.enter(&y);
        if !self.on_curve(&xm, &ym) { return None; }
        Some((xm, ym))
    }
}

/// Jacobian coordinates: the affine point is `(X/Z^2, Y/Z^3)`, `Z = 0` is
/// infinity. Keeps field inversion out of the inner loop
struct Pt { x: Num, y: Num, z: Num }

fn cmov(dst: &mut Num, src: &Num, cond: u32) {
    let mask = 0u32.wrapping_sub(cond & 1);
    for i in 0..L { dst[i] ^= mask & (dst[i] ^ src[i]); }
}

fn pt_cmov(dst: &mut Pt, src: &Pt, cond: u32) {
    cmov(&mut dst.x, &src.x, cond);
    cmov(&mut dst.y, &src.y, cond);
    cmov(&mut dst.z, &src.z, cond);
}

impl Curve {
    fn inf(&self) -> Pt {
        Pt { x: self.fp.one_mont(), y: self.fp.one_mont(), z: bignum::zero() }
    }
    fn is_inf(&self, p: &Pt) -> bool { bignum::is_zero(&p.z) }

    fn from_affine(&self, x: &Num, y: &Num) -> Pt {
        Pt { x: *x, y: *y, z: self.fp.one_mont() }
    }

    fn to_affine(&self, p: &Pt) -> Option<(Num, Num)> {
        if self.is_inf(p) { return None; }
        let f = &self.fp;
        let zi  = f.inv_mont(&p.z);
        let zi2 = f.sqr(&zi);
        let zi3 = f.mul(&zi2, &zi);
        Some((f.mul(&p.x, &zi2), f.mul(&p.y, &zi3)))
    }

    fn double(&self, p: &Pt) -> Pt {
        if self.is_inf(p) || bignum::is_zero(&p.y) { return self.inf(); }
        let f = &self.fp;
        let y2 = f.sqr(&p.y);
        let s  = {
            let t = f.mul(&p.x, &y2);
            let t2 = f.add(&t, &t);
            f.add(&t2, &t2)                       // 4*X*Y^2
        };
        let z2 = f.sqr(&p.z);
        let z4 = f.sqr(&z2);
        let x2 = f.sqr(&p.x);
        // 3X^2 + aZ^4. No a = -3 shortcut, which would be wrong for a curve
        // without it
        let m  = f.add(&f.add(&x2, &f.add(&x2, &x2)), &f.mul(&self.a, &z4));
        let x3 = f.sub(&f.sqr(&m), &f.add(&s, &s));
        let y4 = f.sqr(&y2);
        let y4_8 = { let a = f.add(&y4, &y4); let b = f.add(&a, &a); f.add(&b, &b) };
        let y3 = f.sub(&f.mul(&m, &f.sub(&s, &x3)), &y4_8);
        let z3 = { let t = f.mul(&p.y, &p.z); f.add(&t, &t) };
        Pt { x: x3, y: y3, z: z3 }
    }

    fn add_affine(&self, p: &Pt, ax: &Num, ay: &Num) -> Pt {
        if self.is_inf(p) { return self.from_affine(ax, ay); }
        let f = &self.fp;
        let z2 = f.sqr(&p.z);
        let z3 = f.mul(&z2, &p.z);
        let u2 = f.mul(ax, &z2);
        let s2 = f.mul(ay, &z3);
        let h  = f.sub(&u2, &p.x);
        let r  = f.sub(&s2, &p.y);
        if bignum::is_zero(&h) {
            // Same x: the same point (doubling) or its negation (infinity)
            return if bignum::is_zero(&r) {
                self.double(&self.from_affine(ax, ay))
            } else {
                self.inf()
            };
        }
        let h2   = f.sqr(&h);
        let h3   = f.mul(&h2, &h);
        let x1h2 = f.mul(&p.x, &h2);
        let x3   = f.sub(&f.sub(&f.sqr(&r), &h3), &f.add(&x1h2, &x1h2));
        let y3   = f.sub(&f.mul(&r, &f.sub(&x1h2, &x3)), &f.mul(&p.y, &h3));
        let z3   = f.mul(&p.z, &h);
        Pt { x: x3, y: y3, z: z3 }
    }

    /// k*(px, py), left to right, with the add selected by conditional move: in
    /// ECDH the scalar is a private key
    fn scalar_mul(&self, k: &[u8], px: &Num, py: &Num) -> Option<(Num, Num)> {
        let mut r = self.inf();
        for byte in k {
            for bit in (0..8).rev() {
                r = self.double(&r);
                let added = self.add_affine(&r, px, py);
                pt_cmov(&mut r, &added, ((byte >> bit) & 1) as u32);
            }
        }
        self.to_affine(&r)
    }
}

// ECDH
//
// P-256 only, the one group the ClientHello offers

pub fn ecdh_keypair(rand_bytes: &[u8; 32]) -> ([u8; 32], [u8; 65]) {
    let curve = Curve::p256();
    let mut priv_key = *rand_bytes;
    // A zero scalar has no public point
    if priv_key.iter().all(|&b| b == 0) { priv_key[31] = 1; }

    let mut pub_key = [0u8; 65];
    pub_key[0] = 0x04;
    if let Some((x, y)) = curve.scalar_mul(&priv_key, &curve.gx, &curve.gy) {
        let (xp, yp) = (curve.fp.leave(&x), curve.fp.leave(&y));
        bignum::to_be(&xp, &mut pub_key[1..33]);
        bignum::to_be(&yp, &mut pub_key[33..65]);
    }
    (priv_key, pub_key)
}

pub fn ecdh_shared(priv_key: &[u8; 32], pub_point: &[u8; 65]) -> Option<[u8; 32]> {
    let curve = Curve::p256();
    // Checks the peer's point against the curve, against invalid-curve attacks
    let (px, py) = curve.decode_point(pub_point)?;
    let (sx, _) = curve.scalar_mul(priv_key, &px, &py)?;
    let sx = curve.fp.leave(&sx);
    let mut out = [0u8; 32];
    bignum::to_be(&sx, &mut out);
    Some(out)
}

// ECDSA

/// Split a DER `SEQUENCE { r INTEGER, s INTEGER }` signature
fn parse_sig(sig: &[u8]) -> Option<(Num, Num)> {
    let mut d = Der::new(sig);
    let seq = d.expect(der::TAG_SEQUENCE)?;
    let mut r = seq.reader();
    let rv = r.expect(der::TAG_INTEGER)?.as_uint()?;
    let sv = r.expect(der::TAG_INTEGER)?.as_uint()?;
    if !r.is_empty() { return None; }
    Some((bignum::from_be(rv)?, bignum::from_be(sv)?))
}

/// Message representative: the digest's leftmost bits, as many as the order
/// has. A wider digest is truncated by bits, which for P-521 is not a whole
/// number of bytes
fn digest_to_int(curve: &Curve, hash: &[u8]) -> Option<Num> {
    let order_bits = bignum::bits(&curve.fo.n);
    let take = hash.len().min((order_bits + 7) / 8);
    let mut e: Num = bignum::from_be(&hash[..take])?;
    // Only when the digest is wider than the order. SHA-256 under a P-384 key
    // is legal, and subtracting the other way would underflow into an endless
    // shift loop
    let excess = (take * 8).saturating_sub(order_bits);
    if excess > 0 {
        // Shifts right by the bits the order has no room for
        for _ in 0..excess {
            let mut carry = 0u32;
            for i in (0..L).rev() {
                let nc = e[i] & 1;
                e[i] = (e[i] >> 1) | (carry << 31);
                carry = nc;
            }
        }
    }
    Some(e)
}

/// Verifies an ECDSA signature over `hash` under the uncompressed public point
/// `pub_point`
pub fn ecdsa_verify(curve: &Curve, pub_point: &[u8], hash: &[u8], sig: &[u8]) -> bool {
    let (qx, qy) = match curve.decode_point(pub_point) { Some(q) => q, None => return false };
    let (r, s) = match parse_sig(sig) { Some(v) => v, None => return false };

    let o = &curve.fo;
    // Both components must be in [1, n)
    if !o.in_range(&r) || !o.in_range(&s) { return false; }

    let e = match digest_to_int(curve, hash) { Some(e) => e, None => return false };
    let e = o.reduce(&e);

    let wm  = o.inv_mont(&o.enter(&s));
    let u1  = o.leave(&o.mul(&o.enter(&e), &wm));
    let u2  = o.leave(&o.mul(&o.enter(&r), &wm));

    let mut u1b = [0u8; L * 4];
    let mut u2b = [0u8; L * 4];
    let w = curve.bytes;
    bignum::to_be(&u1, &mut u1b[..w]);
    bignum::to_be(&u2, &mut u2b[..w]);

    let p1 = curve.scalar_mul(&u1b[..w], &curve.gx, &curve.gy);
    let p2 = curve.scalar_mul(&u2b[..w], &qx, &qy);

    let sx = match (p1, p2) {
        (Some((ax, ay)), Some((bx, by))) => {
            let sum = curve.add_affine(&curve.from_affine(&ax, &ay), &bx, &by);
            match curve.to_affine(&sum) {
                Some((x, _)) => x,
                // u1*G + u2*Q at infinity has no x coordinate to equal r
                None => return false,
            }
        }
        (Some((x, _)), None) | (None, Some((x, _))) => x,
        (None, None) => return false,
    };

    // r is compared to x(R) reduced mod the order
    let xi = o.reduce(&curve.fp.leave(&sx));
    bignum::cmp(&xi, &r) == core::cmp::Ordering::Equal
}

/// 1.2.840.10045.3.1.7 prime256v1 (NIST P-256)
pub const OID_P256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
/// 1.3.132.0.34 secp384r1 (NIST P-384)
pub const OID_P384: &[u8] = &[0x2B, 0x81, 0x04, 0x00, 0x22];
/// 1.3.132.0.35 secp521r1 (NIST P-521)
pub const OID_P521: &[u8] = &[0x2B, 0x81, 0x04, 0x00, 0x23];

/// Curve for a `SubjectPublicKeyInfo` parameter OID, if supported
pub fn curve_by_oid(oid: &[u8]) -> Option<Curve> {
    if oid == OID_P256 { return Some(Curve::p256()); }
    if oid == OID_P384 { return Some(Curve::p384()); }
    if oid == OID_P521 { return Some(Curve::p521()); }
    None
}

// ECDSA signing
//
// Compiled into the build host: `builder` signs packages, and the machine only
// verifies. The private key never reaches MikuOS.

/// Largest DER signature: two P-521 integers with leading zero bytes, in a
/// SEQUENCE using the long form length
pub const MAX_SIG_LEN: usize = 3 + 2 * (2 + 1 + 66);

/// Why a signature could not be produced. Values, not panics, so the host tool
/// can report and exit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignError {
    /// Not a scalar of this curve: zero, or at least the group order
    BadPrivateKey,
    /// The output buffer is smaller than `MAX_SIG_LEN`
    NoRoom,
    /// The digest could not be read as a number
    BadDigest,
    /// Too many unusable `k`s. RFC 6979 makes the sequence deterministic and
    /// one failure about 2^-128; the loop still needs a bound
    Exhausted,
}

/// Public point for a private scalar, uncompressed: `0x04 || X || Y`. Returns
/// the bytes written
pub fn ecdsa_public_key(
    curve: &Curve,
    priv_key: &[u8],
    out: &mut [u8],
) -> Result<usize, SignError> {
    let w = curve.bytes;
    if out.len() < 1 + 2 * w {
        return Err(SignError::NoRoom);
    }
    let d: Num = bignum::from_be(priv_key).ok_or(SignError::BadPrivateKey)?;
    if !curve.fo.in_range(&d) {
        return Err(SignError::BadPrivateKey);
    }
    let (x, y) = curve
        .scalar_mul(&be_fixed(&d, w)[..w], &curve.gx, &curve.gy)
        .ok_or(SignError::BadPrivateKey)?;
    out[0] = 0x04;
    bignum::to_be(&curve.fp.leave(&x), &mut out[1..1 + w]);
    bignum::to_be(&curve.fp.leave(&y), &mut out[1 + w..1 + 2 * w]);
    Ok(1 + 2 * w)
}

/// Signs `hash` under `priv_key`, writing a DER `SEQUENCE { r, s }` into `out`.
///
/// `k` is deterministic per RFC 6979, with no RNG option. A repeated or biased
/// `k` reveals the private key, and `plat::random` in a non-`userspace` build
/// is a deterministic xorshift.
pub fn ecdsa_sign(
    curve: &Curve,
    priv_key: &[u8],
    hash: &[u8],
    out: &mut [u8],
) -> Result<usize, SignError> {
    let w = curve.bytes;
    let o = &curve.fo;

    let d: Num = bignum::from_be(priv_key).ok_or(SignError::BadPrivateKey)?;
    if !o.in_range(&d) {
        return Err(SignError::BadPrivateKey);
    }

    // bits2int, reduced: the representative, and the `h1` input to nonce
    // derivation in octet form
    let z1 = digest_to_int(curve, hash).ok_or(SignError::BadDigest)?;
    let e = o.reduce(&z1);
    let h1_octets = be_fixed(&e, w);

    let mut gen = Rfc6979::new(&be_fixed(&d, w)[..w], &h1_octets[..w]);

    // RFC 6979 retries on a `k` giving r = 0 or s = 0. Bounded so an arithmetic
    // bug is a refusal, not a hang
    for _ in 0..32 {
        let k = gen.next_k(curve, w);
        let k = match k {
            Some(k) => k,
            None => continue,
        };

        // r = x(kG) mod n
        let (rx, _) = match curve.scalar_mul(&be_fixed(&k, w)[..w], &curve.gx, &curve.gy) {
            Some(p) => p,
            None => continue,
        };
        let r = o.reduce(&curve.fp.leave(&rx));
        if bignum::is_zero(&r) {
            continue;
        }

        // s = k^-1 (e + r*d) mod n
        let km = o.enter(&k);
        let kinv = o.inv_mont(&km);
        let sum = o.add(&o.enter(&e), &o.mul(&o.enter(&r), &o.enter(&d)));
        let s = o.leave(&o.mul(&kinv, &sum));
        if bignum::is_zero(&s) {
            continue;
        }

        // No low-S normalization. RFC 6979 already gives one signature per
        // input, and `tests/signatures.rs` compares `r` and `s` to its
        // published values
        return der_sig(&r, &s, w, out);
    }
    Err(SignError::Exhausted)
}

/// A number as `w` big-endian bytes, in a buffer wide enough for any curve
fn be_fixed(n: &Num, w: usize) -> [u8; L * 4] {
    let mut buf = [0u8; L * 4];
    bignum::to_be(n, &mut buf[..w]);
    buf
}

/// `SEQUENCE { INTEGER r, INTEGER s }`, both positive. `parse_sig` is the
/// matching reader
fn der_sig(r: &Num, s: &Num, w: usize, out: &mut [u8]) -> Result<usize, SignError> {
    if out.len() < MAX_SIG_LEN {
        return Err(SignError::NoRoom);
    }
    let mut body = [0u8; 2 * (2 + 1 + 66)];
    let mut at = 0usize;
    for v in [r, s] {
        let be = be_fixed(v, w);
        // A DER INTEGER has no leading zeros, and a first byte of 0x80 or more
        // would be negative, so one zero goes back in
        let mut start = 0usize;
        while start + 1 < w && be[start] == 0 {
            start += 1;
        }
        let need_pad = be[start] & 0x80 != 0;
        let len = (w - start) + usize::from(need_pad);
        body[at] = 0x02;
        body[at + 1] = len as u8;
        at += 2;
        if need_pad {
            body[at] = 0;
            at += 1;
        }
        body[at..at + (w - start)].copy_from_slice(&be[start..w]);
        at += w - start;
    }

    out[0] = 0x30;
    // DER's short form stops at 127. Two P-521 integers come to 138 bytes; a
    // P-256 signature never reaches the long form
    if at < 0x80 {
        out[1] = at as u8;
        out[2..2 + at].copy_from_slice(&body[..at]);
        Ok(2 + at)
    } else {
        out[1] = 0x81;
        out[2] = at as u8;
        out[3..3 + at].copy_from_slice(&body[..at]);
        Ok(3 + at)
    }
}

/// RFC 6979 HMAC-SHA256 nonce generator: `K`, `V`, and the loop producing
/// candidates. A struct because a retry continues the sequence:
/// `K = HMAC_K(V || 0x00)` before the next attempt
struct Rfc6979 {
    k: [u8; 32],
    v: [u8; 32],
    started: bool,
}

impl Rfc6979 {
    fn new(x_octets: &[u8], h1_octets: &[u8]) -> Rfc6979 {
        use crate::primitives::hmac_sha256;

        let mut v = [0x01u8; 32];
        let mut k = [0x00u8; 32];

        // K = HMAC_K(V || 0x00 || int2octets(x) || bits2octets(h1))
        let mut buf = [0u8; 32 + 1 + 66 + 66];
        let mut n = 0usize;
        buf[..32].copy_from_slice(&v);
        n += 32;
        buf[n] = 0x00;
        n += 1;
        buf[n..n + x_octets.len()].copy_from_slice(x_octets);
        n += x_octets.len();
        buf[n..n + h1_octets.len()].copy_from_slice(h1_octets);
        n += h1_octets.len();
        k = hmac_sha256(&k, &buf[..n]);
        v = hmac_sha256(&k, &v);

        // Again with 0x01, separating the two derivations
        buf[..32].copy_from_slice(&v);
        buf[32] = 0x01;
        k = hmac_sha256(&k, &buf[..n]);
        v = hmac_sha256(&k, &v);

        Rfc6979 { k, v, started: false }
    }

    /// Next candidate, or `None` when outside `[1, n)` and the caller should
    /// ask again
    fn next_k(&mut self, curve: &Curve, w: usize) -> Option<Num> {
        use crate::primitives::hmac_sha256;

        if self.started {
            // K = HMAC_K(V || 0x00); V = HMAC_K(V)
            let mut buf = [0u8; 33];
            buf[..32].copy_from_slice(&self.v);
            buf[32] = 0x00;
            self.k = hmac_sha256(&self.k, &buf);
            self.v = hmac_sha256(&self.k, &self.v);
        }
        self.started = true;

        // T, filled a digest at a time until at least as wide as the order
        let mut t = [0u8; L * 4];
        let mut have = 0usize;
        while have < w {
            self.v = hmac_sha256(&self.k, &self.v);
            let take = (w - have).min(32);
            t[have..have + take].copy_from_slice(&self.v[..take]);
            have += take;
        }

        let k = digest_to_int(curve, &t[..w])?;
        if curve.fo.in_range(&k) {
            Some(k)
        } else {
            None
        }
    }
}
