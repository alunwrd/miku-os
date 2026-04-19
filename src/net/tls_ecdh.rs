type Fe = [u32; 8];

const P: Fe = [
    0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0x00000000,
    0x00000000, 0x00000000, 0x00000001, 0xFFFFFFFF,
];

const GX: Fe = [
    0xD898C296, 0xF4A13945, 0x2DEB33A0, 0x77037D81,
    0x63A440F2, 0xF8BCE6E5, 0xE12C4247, 0x6B17D1F2,
];

const GY: Fe = [
    0x37BF51F5, 0xCBB64068, 0x6B315ECE, 0x2BCE3357,
    0x7C0F9E16, 0x8EE7EB4A, 0xFE1A7F9B, 0x4FE342E2,
];

fn fe_zero() -> Fe { [0u32; 8] }
fn fe_one()  -> Fe { let mut r = fe_zero(); r[0] = 1; r }
fn fe_is_zero(a: &Fe) -> bool { a.iter().all(|&x| x == 0) }

fn fe_from_bytes(b: &[u8]) -> Fe {
    let mut r = [0u32; 8];
    for i in 0..8 {
        r[7-i] = u32::from_be_bytes([b[4*i], b[4*i+1], b[4*i+2], b[4*i+3]]);
    }
    r
}

fn fe_to_bytes(a: &Fe, out: &mut [u8; 32]) {
    for i in 0..8 {
        let b = a[7-i].to_be_bytes();
        out[4*i..4*i+4].copy_from_slice(&b);
    }
}

fn fe_cmp(a: &Fe, b: &Fe) -> core::cmp::Ordering {
    for i in (0..8).rev() {
        if a[i] > b[i] { return core::cmp::Ordering::Greater; }
        if a[i] < b[i] { return core::cmp::Ordering::Less;    }
    }
    core::cmp::Ordering::Equal
}

fn fe_reduce(mut r: Fe) -> Fe {
    if fe_cmp(&r, &P) != core::cmp::Ordering::Less {
        let mut borrow: i64 = 0;
        for i in 0..8 {
            let d = r[i] as i64 - P[i] as i64 - borrow;
            if d < 0 { r[i] = (d + (1i64 << 32)) as u32; borrow = 1; }
            else      { r[i] = d as u32;                  borrow = 0; }
        }
    }
    r
}

fn fe_from_wide(t: &[u32; 16]) -> Fe {
    let v = |i: usize| t[i] as i64;

    let mut acc = [0i64; 9];
    acc[0] =  v(0) + v(8)  + v(9)                    - v(11) - v(12) - v(13) - v(14);
    acc[1] =  v(1) + v(9)  + v(10)          - v(12) - v(13) - v(14) - v(15);
    acc[2] =  v(2) + v(10) + v(11)          - v(13) - v(14) - v(15);
    acc[3] =  v(3) + 2*v(11) + 2*v(12) + v(13)       - v(15) - v(8)  - v(9);
    acc[4] =  v(4) + 2*v(12) + 2*v(13) + v(14)               - v(9)  - v(10);
    acc[5] =  v(5) + 2*v(13) + 2*v(14) + v(15)               - v(10) - v(11);
    acc[6] =  v(6) + 3*v(14) + 2*v(15) + v(13)       - v(8)  - v(9);
    acc[7] =  v(7) + 3*v(15) + v(8)                  - v(10) - v(11) - v(12) - v(13);

    for _ in 0..4 {
        for i in 0..8 {
            let c    = acc[i] >> 32;
            acc[i]  -= c << 32;
            acc[i+1] += c;
        }
        let hi = acc[8];
        if hi != 0 {
            acc[8]  = 0;
            acc[7] += hi;
            acc[6] -= hi;
            acc[3] -= hi;
            acc[0] += hi;
        }
    }

    let mut r = [0u32; 8];
    for i in 0..8 {
        r[i] = acc[i] as u32;
    }
    fe_reduce(r)
}

fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    let mut t = [0u128; 16];
    for i in 0..8 {
        for j in 0..8 {
            t[i+j] += a[i] as u128 * b[j] as u128;
        }
    }
    let mut wide = [0u32; 16];
    let mut carry = 0u128;
    for i in 0..16 {
        let val  = t[i] + carry;
        wide[i]  = val as u32;
        carry    = val >> 32;
    }
    fe_from_wide(&wide)
}

fn fe_sqr(a: &Fe) -> Fe { fe_mul(a, a) }

fn fe_add(a: &Fe, b: &Fe) -> Fe {
    let mut r = [0u32; 8];
    let mut carry: u64 = 0;
    for i in 0..8 {
        let s = a[i] as u64 + b[i] as u64 + carry;
        r[i]  = s as u32;
        carry = s >> 32;
    }
    if carry != 0 {
        let mut borrow: i64 = 0;
        for i in 0..8 {
            let d = r[i] as i64 - P[i] as i64 - borrow;
            if d < 0 { r[i] = (d + (1i64 << 32)) as u32; borrow = 1; }
            else      { r[i] = d as u32;                  borrow = 0; }
        }
        return r;
    }
    fe_reduce(r)
}

fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    let mut r = [0u32; 8];
    let mut borrow: i64 = 0;
    for i in 0..8 {
        let d = a[i] as i64 - b[i] as i64 - borrow;
        if d < 0 { r[i] = (d + (1i64 << 32)) as u32; borrow = 1; }
        else      { r[i] = d as u32;                  borrow = 0; }
    }
    if borrow != 0 {
        let mut c: u64 = 0;
        for i in 0..8 {
            let s = r[i] as u64 + P[i] as u64 + c;
            r[i]  = s as u32;
            c     = s >> 32;
        }
    }
    r
}

fn fe_double(a: &Fe) -> Fe { fe_add(a, a) }
fn fe_triple(a: &Fe) -> Fe { fe_add(&fe_double(a), a) }

fn fe_inv(a: &Fe) -> Fe {
    let exp: [u8; 32] = [
        0xFF,0xFF,0xFF,0xFF, 0x00,0x00,0x00,0x01,
        0x00,0x00,0x00,0x00, 0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00, 0xFF,0xFF,0xFF,0xFF,
        0xFF,0xFF,0xFF,0xFF, 0xFF,0xFF,0xFF,0xFD,
    ];
    let mut r = fe_one();
    for i in 0..256 {
        r = fe_sqr(&r);
        if (exp[i/8] >> (7-(i%8))) & 1 == 1 {
            r = fe_mul(&r, a);
        }
    }
    r
}

struct JacPoint { x: Fe, y: Fe, z: Fe }

fn jac_inf() -> JacPoint { JacPoint { x: fe_one(), y: fe_one(), z: fe_zero() } }
fn jac_is_inf(p: &JacPoint) -> bool { fe_is_zero(&p.z) }

fn jac_from_affine(x: &Fe, y: &Fe) -> JacPoint {
    JacPoint { x: *x, y: *y, z: fe_one() }
}

fn jac_to_affine(p: &JacPoint) -> Option<(Fe, Fe)> {
    if jac_is_inf(p) { return None; }
    let zi  = fe_inv(&p.z);
    let zi2 = fe_sqr(&zi);
    let zi3 = fe_mul(&zi2, &zi);
    Some((fe_mul(&p.x, &zi2), fe_mul(&p.y, &zi3)))
}

fn jac_double(p: &JacPoint) -> JacPoint {
    if jac_is_inf(p) || fe_is_zero(&p.y) { return jac_inf(); }
    let z2  = fe_sqr(&p.z);
    let t   = fe_triple(&fe_mul(&fe_sub(&p.x, &z2), &fe_add(&p.x, &z2)));
    let y2  = fe_sqr(&p.y);
    let u   = fe_double(&fe_double(&fe_mul(&p.x, &y2)));
    let x3  = fe_sub(&fe_sqr(&t), &fe_double(&u));
    let z3  = fe_double(&fe_mul(&p.y, &p.z));
    let y4  = fe_sqr(&y2);
    let y3  = fe_sub(&fe_mul(&t, &fe_sub(&u, &x3)),
                     &fe_double(&fe_double(&fe_double(&y4))));
    JacPoint { x: x3, y: y3, z: z3 }
}

fn jac_add_affine(p: &JacPoint, ax: &Fe, ay: &Fe) -> JacPoint {
    if jac_is_inf(p) { return jac_from_affine(ax, ay); }
    let z2 = fe_sqr(&p.z);
    let z3 = fe_mul(&z2, &p.z);
    let u2 = fe_mul(ax, &z2);
    let s2 = fe_mul(ay, &z3);
    let h  = fe_sub(&u2, &p.x);
    let r  = fe_sub(&s2, &p.y);
    if fe_is_zero(&h) {
        return if fe_is_zero(&r) {
            jac_double(&jac_from_affine(ax, ay))
        } else {
            jac_inf()
        };
    }
    let h2   = fe_sqr(&h);
    let h3   = fe_mul(&h2, &h);
    let x1h2 = fe_mul(&p.x, &h2);
    let x3   = fe_sub(&fe_sub(&fe_sqr(&r), &h3), &fe_double(&x1h2));
    let y3   = fe_sub(&fe_mul(&r, &fe_sub(&x1h2, &x3)), &fe_mul(&p.y, &h3));
    let z3   = fe_mul(&p.z, &h);
    JacPoint { x: x3, y: y3, z: z3 }
}

fn fe_cmov(dst: &mut Fe, src: &Fe, cond: u32) {
    let mask = 0u32.wrapping_sub(cond & 1);
    for i in 0..8 {
        dst[i] ^= mask & (dst[i] ^ src[i]);
    }
}

fn jac_cmov(dst: &mut JacPoint, src: &JacPoint, cond: u32) {
    fe_cmov(&mut dst.x, &src.x, cond);
    fe_cmov(&mut dst.y, &src.y, cond);
    fe_cmov(&mut dst.z, &src.z, cond);
}

fn scalar_mul(k: &[u8; 32], px: &Fe, py: &Fe) -> Option<(Fe, Fe)> {
    let mut r = jac_inf();
    for byte in k.iter() {
        for bit in (0..8).rev() {
            r = jac_double(&r);
            let added = jac_add_affine(&r, px, py);
            let cond  = ((byte >> bit) & 1) as u32;
            jac_cmov(&mut r, &added, cond);
        }
    }
    jac_to_affine(&r)
}

pub fn ecdh_keypair(rand_bytes: &[u8; 32]) -> ([u8; 32], [u8; 65]) {
    let mut priv_key = *rand_bytes;
    if priv_key.iter().all(|&b| b == 0) { priv_key[31] = 1; }

    let (pub_x, pub_y) = scalar_mul(&priv_key, &GX, &GY)
        .unwrap_or((fe_zero(), fe_zero()));

    let mut pub_key = [0u8; 65];
    pub_key[0] = 0x04;
    let (mut xb, mut yb) = ([0u8; 32], [0u8; 32]);
    fe_to_bytes(&pub_x, &mut xb);
    fe_to_bytes(&pub_y, &mut yb);
    pub_key[1..33].copy_from_slice(&xb);
    pub_key[33..65].copy_from_slice(&yb);
    (priv_key, pub_key)
}

pub fn ecdh_shared(priv_key: &[u8; 32], pub_point: &[u8; 65]) -> Option<[u8; 32]> {
    if pub_point[0] != 0x04 { return None; }
    let px = fe_from_bytes(&pub_point[1..33]);
    let py = fe_from_bytes(&pub_point[33..65]);
    let (sx, _) = scalar_mul(priv_key, &px, &py)?;
    let mut out = [0u8; 32];
    fe_to_bytes(&sx, &mut out);
    Some(out)
}
