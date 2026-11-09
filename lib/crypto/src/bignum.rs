// Fixed-width modular arithmetic for RSA and elliptic curves
//
// Up to 4096 bits, since many trust-store roots have keys that long. Reduction
// is Montgomery: 2*k^2 word multiplies, about 33 000 at 4096 bits.
//
// Parameterized on width as well as modulus: RSA uses 128 limbs, a curve 17.

pub const MAX_LIMBS: usize = 128; // 4096 bits
pub type BigNum = [u32; MAX_LIMBS];

// Width-generic primitives

pub fn zero<const L: usize>() -> [u32; L] { [0u32; L] }

pub fn one<const L: usize>() -> [u32; L] { let mut r = [0u32; L]; r[0] = 1; r }

/// Big-endian bytes to limbs, or None when the value does not fit
pub fn from_be<const L: usize>(bytes: &[u8]) -> Option<[u32; L]> {
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    let sig = &bytes[start..];
    if sig.len() > L * 4 { return None; }
    let mut n = [0u32; L];
    for (i, &src) in sig.iter().rev().enumerate() {
        n[i / 4] |= (src as u32) << ((i % 4) * 8);
    }
    Some(n)
}

pub fn to_be<const L: usize>(n: &[u32; L], out: &mut [u8]) {
    let len = out.len();
    for i in 0..len {
        let idx = len - 1 - i;
        out[i] = if idx / 4 >= L { 0 } else { ((n[idx / 4] >> ((idx % 4) * 8)) & 0xFF) as u8 };
    }
}

/// Number of limbs actually used
pub fn limbs<const L: usize>(n: &[u32; L]) -> usize {
    for i in (0..L).rev() {
        if n[i] != 0 { return i + 1; }
    }
    0
}

/// Number of significant bits
pub fn bits<const L: usize>(n: &[u32; L]) -> usize {
    let l = limbs(n);
    if l == 0 { return 0; }
    l * 32 - n[l - 1].leading_zeros() as usize
}

pub fn is_zero<const L: usize>(n: &[u32; L]) -> bool { n.iter().all(|&x| x == 0) }

pub fn cmp<const L: usize>(a: &[u32; L], b: &[u32; L]) -> core::cmp::Ordering {
    for i in (0..L).rev() {
        if a[i] > b[i] { return core::cmp::Ordering::Greater; }
        if a[i] < b[i] { return core::cmp::Ordering::Less; }
    }
    core::cmp::Ordering::Equal
}

fn cmp_len<const L: usize>(a: &[u32; L], b: &[u32; L], len: usize) -> core::cmp::Ordering {
    for i in (0..len).rev() {
        if a[i] > b[i] { return core::cmp::Ordering::Greater; }
        if a[i] < b[i] { return core::cmp::Ordering::Less; }
    }
    core::cmp::Ordering::Equal
}

/// a -= b over `len` limbs, wrapping; returns the borrow out
fn sub_in_place<const L: usize>(a: &mut [u32; L], b: &[u32; L], len: usize) -> u32 {
    let mut borrow: i64 = 0;
    for i in 0..len {
        let d = a[i] as i64 - b[i] as i64 - borrow;
        if d < 0 { a[i] = (d + (1i64 << 32)) as u32; borrow = 1; }
        else     { a[i] = d as u32;                  borrow = 0; }
    }
    borrow as u32
}

/// x = 2x mod n for reduced x. 2x < 2n, so one conditional subtraction is
/// enough
fn dbl_mod<const L: usize>(x: &mut [u32; L], n: &[u32; L], len: usize) {
    let mut carry = 0u32;
    for i in 0..len {
        let nc = x[i] >> 31;
        x[i] = (x[i] << 1) | carry;
        carry = nc;
    }
    if carry != 0 || cmp_len(x, n, len) != core::cmp::Ordering::Less {
        sub_in_place(x, n, len);
    }
}

// Montgomery

/// A modulus prepared for Montgomery arithmetic. `a` is stored as `aR mod n`,
/// `R = 2^(32*len)`; `enter` and `leave` convert
pub struct Mont<const L: usize> {
    pub n:   [u32; L],
    pub len: usize,
    /// -n^-1 mod 2^32
    n0inv:   u32,
    /// R^2 mod n, the multiplier that puts a plain value into Montgomery form
    rr:      [u32; L],
}

/// The width RSA uses
pub type MontCtx = Mont<MAX_LIMBS>;

impl<const L: usize> Mont<L> {
    /// Fails on an even or trivial modulus: Montgomery needs n coprime to 2
    pub fn new(n: &[u32; L]) -> Option<Mont<L>> {
        let len = limbs(n);
        if len == 0 || n[0] & 1 == 0 { return None; }

        // Newton's iteration on Z/2^32: each step doubles the correct bits, so
        // five take 1 bit to 32
        let n0 = n[0];
        let mut inv = 1u32;
        for _ in 0..5 {
            inv = inv.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inv)));
        }
        let n0inv = inv.wrapping_neg();

        // R^2 mod n by doubling. 2^(32*(len-1)) is below n since n's top limb
        // is non-zero; 32 doublings reach R mod n, 32*len more square it
        let mut rr = [0u32; L];
        rr[len - 1] = 1;
        for _ in 0..(32 + 32 * len) {
            dbl_mod(&mut rr, n, len);
        }

        Some(Mont { n: *n, len, n0inv, rr })
    }

    /// Montgomery product (a * b * R^-1) mod n, CIOS: the reduction limb is
    /// folded in after each row, with no double-width product.
    ///
    /// The two words above the working limbs are in `th` and `thh`, so the
    /// array is exactly as wide as the modulus
    pub fn mul(&self, a: &[u32; L], b: &[u32; L]) -> [u32; L] {
        let len = self.len;
        let mut t = [0u32; L];
        let mut th: u32 = 0;
        let mut thh: u32;

        for i in 0..len {
            let ai = a[i] as u64;
            let mut carry: u64 = 0;
            for j in 0..len {
                let s = t[j] as u64 + ai * b[j] as u64 + carry;
                t[j] = s as u32;
                carry = s >> 32;
            }
            let s = th as u64 + carry;
            th = s as u32;
            thh = (s >> 32) as u32;

            let m = t[0].wrapping_mul(self.n0inv) as u64;
            let s = t[0] as u64 + m * self.n[0] as u64;
            let mut carry = s >> 32;
            for j in 1..len {
                let s = t[j] as u64 + m * self.n[j] as u64 + carry;
                t[j - 1] = s as u32;
                carry = s >> 32;
            }
            let s = th as u64 + carry;
            t[len - 1] = s as u32;
            th = thh.wrapping_add((s >> 32) as u32);
        }

        for i in len..L { t[i] = 0; }
        if th != 0 || cmp_len(&t, &self.n, len) != core::cmp::Ordering::Less {
            sub_in_place(&mut t, &self.n, len);
        }
        t
    }

    pub fn sqr(&self, a: &[u32; L]) -> [u32; L] { self.mul(a, a) }

    /// Plain value to Montgomery form
    pub fn enter(&self, a: &[u32; L]) -> [u32; L] { self.mul(a, &self.rr) }

    /// Montgomery form back to a plain value
    pub fn leave(&self, a: &[u32; L]) -> [u32; L] { self.mul(a, &one()) }

    /// R mod n, the Montgomery form of 1
    pub fn one_mont(&self) -> [u32; L] { self.enter(&one()) }

    /// a mod n, for an `a` that may be larger than n
    pub fn reduce(&self, a: &[u32; L]) -> [u32; L] {
        // aR * R^-1 = a, and `mul` reduces on the way through
        self.leave(&self.enter(a))
    }

    /// (a + b) mod n, for operands already reduced
    pub fn add(&self, a: &[u32; L], b: &[u32; L]) -> [u32; L] {
        let mut r = [0u32; L];
        let mut carry: u64 = 0;
        for i in 0..self.len {
            let s = a[i] as u64 + b[i] as u64 + carry;
            r[i] = s as u32;
            carry = s >> 32;
        }
        if carry != 0 || cmp_len(&r, &self.n, self.len) != core::cmp::Ordering::Less {
            sub_in_place(&mut r, &self.n, self.len);
        }
        r
    }

    /// (a - b) mod n for reduced operands
    pub fn sub(&self, a: &[u32; L], b: &[u32; L]) -> [u32; L] {
        let mut r = *a;
        let borrow = sub_in_place(&mut r, b, self.len);
        if borrow != 0 {
            let mut carry: u64 = 0;
            for i in 0..self.len {
                let s = r[i] as u64 + self.n[i] as u64 + carry;
                r[i] = s as u32;
                carry = s >> 32;
            }
        }
        r
    }

    /// base^exp mod n, square-and-multiply. For 65537: 17 squarings, one
    /// multiply
    pub fn pow_u32(&self, base: &[u32; L], exp: u32) -> [u32; L] {
        let mut result = self.one_mont();
        let mut b = self.enter(base);
        let mut e = exp;
        while e > 0 {
            if e & 1 != 0 { result = self.mul(&result, &b); }
            b = self.sqr(&b);
            e >>= 1;
        }
        self.leave(&result)
    }

    /// base^exp mod n for a big-endian exponent, base and result in Montgomery
    /// form
    pub fn pow_mont(&self, base_m: &[u32; L], exp: &[u8]) -> [u32; L] {
        let mut result = self.one_mont();
        let mut started = false;
        for &byte in exp {
            for bit in (0..8).rev() {
                if started { result = self.sqr(&result); }
                if (byte >> bit) & 1 == 1 {
                    result = if started { self.mul(&result, base_m) } else { *base_m };
                    started = true;
                }
            }
        }
        result
    }

    /// a^-1 mod n in Montgomery form, for prime n only
    pub fn inv_mont(&self, a_m: &[u32; L]) -> [u32; L] {
        let mut e = self.n;
        // n - 2; n is odd, so the low limb cannot borrow
        e[0] = e[0].wrapping_sub(2);
        let mut be = [0u8; 64];
        let nbytes = self.len * 4;
        if nbytes <= be.len() {
            to_be(&e, &mut be[..nbytes]);
            return self.pow_mont(a_m, &be[..nbytes]);
        }
        // Wider than any curve; RSA needs no inverse
        let mut wide = [0u8; MAX_LIMBS * 4];
        to_be(&e, &mut wide[..nbytes]);
        self.pow_mont(a_m, &wide[..nbytes])
    }

    /// a^-1 mod n, for a plain value
    pub fn inv_prime(&self, a: &[u32; L]) -> [u32; L] {
        let am = self.enter(a);
        self.leave(&self.inv_mont(&am))
    }

    /// Is `a` in [1, n)? The range check every signature component needs
    pub fn in_range(&self, a: &[u32; L]) -> bool {
        !is_zero(a) && cmp(a, &self.n) == core::cmp::Ordering::Less
    }
}

// RSA-width helpers

pub fn bn_from_bytes_be(bytes: &[u8]) -> Option<BigNum> { from_be(bytes) }
pub fn bn_to_bytes_be(n: &BigNum, out: &mut [u8]) { to_be(n, out) }
pub fn bn_bits(n: &BigNum) -> usize { bits(n) }
pub fn bn_cmp(a: &BigNum, b: &BigNum) -> core::cmp::Ordering { cmp(a, b) }
