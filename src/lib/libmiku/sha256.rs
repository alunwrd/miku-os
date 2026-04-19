// sha256.rs - SHA-256 hash
//
// Full SHA-256 implementation per FIPS 180-4.
// No heap allocation - works entirely on stack
// Produces 32-byte (256-bit) hash

use crate::mem;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[repr(C)]
pub struct MikuSha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

#[inline]
fn rotr(x: u32, n: u32) -> u32 {
    (x >> n) | (x << (32 - n))
}

#[inline]
fn ch(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (!x & z) }

#[inline]
fn maj(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (x & z) ^ (y & z) }

#[inline]
fn sigma0(x: u32) -> u32 { rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22) }

#[inline]
fn sigma1(x: u32) -> u32 { rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25) }

#[inline]
fn gamma0(x: u32) -> u32 { rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3) }

#[inline]
fn gamma1(x: u32) -> u32 { rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10) }

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];

    // prepare message schedule
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        w[i] = gamma1(w[i - 2])
            .wrapping_add(w[i - 7])
            .wrapping_add(gamma0(w[i - 15]))
            .wrapping_add(w[i - 16]);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;

    for i in 0..64 {
        let t1 = h
            .wrapping_add(sigma1(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let t2 = sigma0(a).wrapping_add(maj(a, b, c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

// init SHA-256 context
#[no_mangle]
pub extern "C" fn miku_sha256_init(ctx: *mut MikuSha256) {
    if ctx.is_null() { return; }
    let ctx = unsafe { &mut *ctx };
    ctx.state = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    ctx.buf = [0u8; 64];
    ctx.buf_len = 0;
    ctx.total_len = 0;
}

// update with more data
#[no_mangle]
pub extern "C" fn miku_sha256_update(
    ctx: *mut MikuSha256,
    data: *const u8,
    len: usize,
) {
    if ctx.is_null() || data.is_null() || len == 0 { return; }
    let ctx = unsafe { &mut *ctx };

    ctx.total_len += len as u64;
    let mut pos = 0usize;

    // fill buffer
    if ctx.buf_len > 0 {
        let need = 64 - ctx.buf_len;
        let copy = len.min(need);
        unsafe {
            mem::miku_memcpy(ctx.buf.as_mut_ptr().add(ctx.buf_len), data, copy);
        }
        ctx.buf_len += copy;
        pos = copy;

        if ctx.buf_len == 64 {
            let block = ctx.buf;
            compress(&mut ctx.state, &block);
            ctx.buf_len = 0;
        }
    }

    // process full blocks
    while pos + 64 <= len {
        let mut block = [0u8; 64];
        unsafe {
            mem::miku_memcpy(block.as_mut_ptr(), data.add(pos), 64);
        }
        compress(&mut ctx.state, &block);
        pos += 64;
    }

    // save remainder
    if pos < len {
        let remaining = len - pos;
        unsafe {
            mem::miku_memcpy(ctx.buf.as_mut_ptr(), data.add(pos), remaining);
        }
        ctx.buf_len = remaining;
    }
}

// finalize and produce 32-byte hash
#[no_mangle]
pub extern "C" fn miku_sha256_finish(ctx: *mut MikuSha256, out: *mut u8) {
    if ctx.is_null() || out.is_null() { return; }
    let ctx = unsafe { &mut *ctx };

    let bits = ctx.total_len * 8;

    // padding
    ctx.buf[ctx.buf_len] = 0x80;
    ctx.buf_len += 1;

    if ctx.buf_len > 56 {
        // not enough room for length - pad and compress
        while ctx.buf_len < 64 {
            ctx.buf[ctx.buf_len] = 0;
            ctx.buf_len += 1;
        }
        let block = ctx.buf;
        compress(&mut ctx.state, &block);
        ctx.buf_len = 0;
        ctx.buf = [0u8; 64];
    }

    while ctx.buf_len < 56 {
        ctx.buf[ctx.buf_len] = 0;
        ctx.buf_len += 1;
    }

    // append length in bits as big-endian u64
    let be_bits = bits.to_be_bytes();
    ctx.buf[56..64].copy_from_slice(&be_bits);
    let block = ctx.buf;
    compress(&mut ctx.state, &block);

    // write output
    unsafe {
        for i in 0..8 {
            let be = ctx.state[i].to_be_bytes();
            *out.add(i * 4) = be[0];
            *out.add(i * 4 + 1) = be[1];
            *out.add(i * 4 + 2) = be[2];
            *out.add(i * 4 + 3) = be[3];
        }
    }
}

// one-shot hash: compute SHA-256 of data
#[no_mangle]
pub extern "C" fn miku_sha256(data: *const u8, len: usize, out: *mut u8) {
    let mut ctx = MikuSha256 {
        state: [0; 8],
        buf: [0; 64],
        buf_len: 0,
        total_len: 0,
    };
    miku_sha256_init(&mut ctx);
    miku_sha256_update(&mut ctx, data, len);
    miku_sha256_finish(&mut ctx, out);
}

// compare two hashes
#[no_mangle]
pub extern "C" fn miku_sha256_eq(a: *const u8, b: *const u8) -> bool {
    if a.is_null() || b.is_null() { return false; }
    mem::miku_memcmp(a, b, 32) == 0
}

// format hash as hex string (64 chars + null)
#[no_mangle]
pub extern "C" fn miku_sha256_hex(hash: *const u8, out: *mut u8) {
    if hash.is_null() || out.is_null() { return; }
    crate::hex::miku_hex_encode(hash, 32, out, 65);
}

// HMAC-SHA256 (RFC 2104)
#[no_mangle]
pub extern "C" fn miku_sha256_hmac(
    key: *const u8,
    key_len: usize,
    data: *const u8,
    data_len: usize,
    out: *mut u8,
) {
    if out.is_null() { return; }

    let mut k_pad = [0u8; 64];

    // if key > 64 bytes, hash it first
    if key_len > 64 {
        miku_sha256(key, key_len, k_pad.as_mut_ptr());
        // rest of k_pad is already 0
    } else if !key.is_null() && key_len > 0 {
        unsafe { mem::miku_memcpy(k_pad.as_mut_ptr(), key, key_len); }
    }

    // inner hash: SHA256(K ^ ipad || data)
    let mut ipad = [0u8; 64];
    for i in 0..64 { ipad[i] = k_pad[i] ^ 0x36; }

    let mut ctx = MikuSha256 { state: [0; 8], buf: [0; 64], buf_len: 0, total_len: 0 };
    miku_sha256_init(&mut ctx);
    miku_sha256_update(&mut ctx, ipad.as_ptr(), 64);
    if !data.is_null() && data_len > 0 {
        miku_sha256_update(&mut ctx, data, data_len);
    }
    let mut inner = [0u8; 32];
    miku_sha256_finish(&mut ctx, inner.as_mut_ptr());

    // outer hash: SHA256(K ^ opad || inner_hash)
    let mut opad = [0u8; 64];
    for i in 0..64 { opad[i] = k_pad[i] ^ 0x5C; }

    miku_sha256_init(&mut ctx);
    miku_sha256_update(&mut ctx, opad.as_ptr(), 64);
    miku_sha256_update(&mut ctx, inner.as_ptr(), 32);
    miku_sha256_finish(&mut ctx, out);
}
