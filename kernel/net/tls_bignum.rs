pub const LIMBS: usize = 64;
pub type BigNum = [u32; LIMBS];

pub fn bn_zero() -> BigNum { [0u32; LIMBS] }

pub fn bn_from_bytes_be(bytes: &[u8]) -> BigNum {
    let mut n = bn_zero();
    let len = bytes.len().min(LIMBS * 4);
    for i in 0..len {
        let src = bytes[bytes.len() - 1 - i];
        n[i / 4] |= (src as u32) << ((i % 4) * 8);
    }
    n
}

pub fn bn_to_bytes_be(n: &BigNum, out: &mut [u8]) {
    let len = out.len().min(LIMBS * 4);
    for i in 0..len {
        let limb = (len - 1 - i) / 4;
        let shift = ((len - 1 - i) % 4) * 8;
        out[i] = ((n[limb] >> shift) & 0xFF) as u8;
    }
}

pub fn bn_cmp(a: &BigNum, b: &BigNum) -> core::cmp::Ordering {
    for i in (0..LIMBS).rev() {
        if a[i] > b[i] { return core::cmp::Ordering::Greater; }
        if a[i] < b[i] { return core::cmp::Ordering::Less; }
    }
    core::cmp::Ordering::Equal
}

pub fn bn_sub(a: &BigNum, b: &BigNum) -> BigNum {
    let mut r = bn_zero();
    let mut borrow: i64 = 0;
    for i in 0..LIMBS {
        let d = a[i] as i64 - b[i] as i64 - borrow;
        if d < 0 {
            r[i] = (d + (1i64 << 32)) as u32;
            borrow = 1;
        } else {
            r[i] = d as u32;
            borrow = 0;
        }
    }
    r
}

pub fn bn_add(a: &BigNum, b: &BigNum) -> (BigNum, bool) {
    let mut r = bn_zero();
    let mut carry: u64 = 0;
    for i in 0..LIMBS {
        let s = a[i] as u64 + b[i] as u64 + carry;
        r[i] = s as u32;
        carry = s >> 32;
    }
    (r, carry != 0)
}

fn bn_mul_full(a: &BigNum, b: &BigNum) -> [u32; LIMBS * 2] {
    let mut r = [0u32; LIMBS * 2];
    for i in 0..LIMBS {
        let mut carry: u64 = 0;
        for j in 0..LIMBS {
            let prod = a[i] as u64 * b[j] as u64 + r[i+j] as u64 + carry;
            r[i+j] = prod as u32;
            carry = prod >> 32;
        }
        if i + LIMBS < LIMBS * 2 {
            r[i + LIMBS] = r[i + LIMBS].wrapping_add(carry as u32);
        }
    }
    r
}

fn bn_reduce_double(x: &[u32; LIMBS * 2], n: &BigNum) -> BigNum {
    let mut rem = [0u32; LIMBS + 1];

    for i in (0..LIMBS * 2).rev() {
        for bit in (0..32u32).rev() {
            let next_bit = (x[i] >> bit) & 1;

            let mut carry = next_bit;
            for j in 0..=LIMBS {
                let nc = rem[j] >> 31;
                rem[j] = (rem[j] << 1) | carry;
                carry = nc;
            }

            let hi = rem[LIMBS];
            let mut temp = [0u32; LIMBS];
            temp.copy_from_slice(&rem[..LIMBS]);
            if hi != 0 || bn_cmp(&temp, n) != core::cmp::Ordering::Less {
                let sub = bn_sub(&temp, n);
                rem[..LIMBS].copy_from_slice(&sub);
                rem[LIMBS] = 0;
            }
        }
    }

    let mut res = [0u32; LIMBS];
    res.copy_from_slice(&rem[..LIMBS]);
    res
}

pub fn bn_mulmod(a: &BigNum, b: &BigNum, n: &BigNum) -> BigNum {
    let full = bn_mul_full(a, b);
    bn_reduce_double(&full, n)
}

pub fn bn_powmod_u32(base: &BigNum, exp: u32, n: &BigNum) -> BigNum {
    let mut result = bn_zero();
    result[0] = 1;
    let mut b = *base;
    let mut e = exp;
    while e > 0 {
        if e & 1 != 0 {
            result = bn_mulmod(&result, &b, n);
        }
        b = bn_mulmod(&b, &b, n);
        e >>= 1;
    }
    result
}
