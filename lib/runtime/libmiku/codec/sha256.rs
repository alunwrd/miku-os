// SHA-256 per FIPS 180-4, on the stack, producing a 32-byte hash

use crate::mem;

pub use miku_abi::libtypes::MikuSha256;
use mikucrypto::primitives::sha256_compress as compress;

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
