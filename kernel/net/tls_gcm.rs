use super::tls_crypto::{Aes128, hmac_sha256, sha256};

fn ghash_mul(x: &mut [u64; 2], h: &[u64; 2]) {
    let mut z = [0u64; 2];
    let mut v = *h;
    let mut xi = *x;
    for i in 0..2 {
        for bit in (0..64).rev() {
            if (xi[i] >> bit) & 1 == 1 {
                z[0] ^= v[0];
                z[1] ^= v[1];
            }
            let lsb = v[1] & 1;
            v[1] = (v[1] >> 1) | (v[0] << 63);
            v[0] >>= 1;
            if lsb != 0 {
                v[0] ^= 0xe100000000000000u64;
            }
        }
    }
    *x = z;
}

fn ghash(h: &[u64; 2], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut y = [0u64; 2];

    let process = |y: &mut [u64; 2], data: &[u8]| {
        let mut i = 0;
        while i + 16 <= data.len() {
            let hi = u64::from_be_bytes(data[i..i+8].try_into().unwrap());
            let lo = u64::from_be_bytes(data[i+8..i+16].try_into().unwrap());
            y[0] ^= hi;
            y[1] ^= lo;
            ghash_mul(y, h);
            i += 16;
        }
        if i < data.len() {
            let mut block = [0u8; 16];
            block[..data.len()-i].copy_from_slice(&data[i..]);
            let hi = u64::from_be_bytes(block[0..8].try_into().unwrap());
            let lo = u64::from_be_bytes(block[8..16].try_into().unwrap());
            y[0] ^= hi;
            y[1] ^= lo;
            ghash_mul(y, h);
        }
    };

    process(&mut y, aad);
    process(&mut y, ciphertext);

    let aad_bits = (aad.len() as u64) * 8;
    let ct_bits  = (ciphertext.len() as u64) * 8;
    y[0] ^= aad_bits;
    y[1] ^= ct_bits;
    ghash_mul(&mut y, h);

    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&y[0].to_be_bytes());
    out[8..16].copy_from_slice(&y[1].to_be_bytes());
    out
}

fn ctr_block(aes: &Aes128, nonce_base: &[u8; 12], counter: u32) -> [u8; 16] {
    let mut block = [0u8; 16];
    block[..12].copy_from_slice(nonce_base);
    block[12..16].copy_from_slice(&counter.to_be_bytes());
    aes.encrypt_block(&mut block);
    block
}

pub fn aes128gcm_seal(
    key:        &[u8; 16],
    nonce:      &[u8; 12],
    aad:        &[u8],
    plaintext:  &[u8],
    out:        &mut [u8],
) -> usize {
    let aes = Aes128::new(key);

    let mut h_block = [0u8; 16];
    aes.encrypt_block(&mut h_block);
    let h = [
        u64::from_be_bytes(h_block[0..8].try_into().unwrap()),
        u64::from_be_bytes(h_block[8..16].try_into().unwrap()),
    ];

    let ectr0 = ctr_block(&aes, nonce, 1);

    let ct_len = plaintext.len();
    for i in 0..ct_len {
        let block_idx = (i / 16) as u32 + 2;
        let block_off = i % 16;
        if block_off == 0 {
            let kb = ctr_block(&aes, nonce, block_idx);
            let take = ct_len - i;
            for j in 0..take.min(16) {
                out[i + j] = plaintext[i + j] ^ kb[j];
            }
        }
    }

    let tag_raw = ghash(&h, aad, &out[..ct_len]);

    let mut tag = [0u8; 16];
    for i in 0..16 { tag[i] = tag_raw[i] ^ ectr0[i]; }

    out[ct_len..ct_len + 16].copy_from_slice(&tag);
    ct_len + 16
}

pub fn aes128gcm_open(
    key:        &[u8; 16],
    nonce:      &[u8; 12],
    aad:        &[u8],
    ciphertext: &[u8],
    out:        &mut [u8],
) -> Option<usize> {
    if ciphertext.len() < 16 { return None; }
    let ct_len  = ciphertext.len() - 16;
    let ct_data = &ciphertext[..ct_len];
    let tag_in  = &ciphertext[ct_len..];

    let aes = Aes128::new(key);

    let mut h_block = [0u8; 16];
    aes.encrypt_block(&mut h_block);
    let h = [
        u64::from_be_bytes(h_block[0..8].try_into().unwrap()),
        u64::from_be_bytes(h_block[8..16].try_into().unwrap()),
    ];

    let ectr0 = ctr_block(&aes, nonce, 1);

    let tag_raw = ghash(&h, aad, ct_data);
    let mut tag_exp = [0u8; 16];
    for i in 0..16 { tag_exp[i] = tag_raw[i] ^ ectr0[i]; }

    let mut diff = 0u8;
    for i in 0..16 { diff |= tag_exp[i] ^ tag_in[i]; }
    if diff != 0 { return None; }

    let mut i = 0;
    while i < ct_len {
        let block_idx = (i / 16) as u32 + 2;
        let kb = ctr_block(&aes, nonce, block_idx);
        let take = (ct_len - i).min(16);
        for j in 0..take {
            out[i + j] = ct_data[i + j] ^ kb[j];
        }
        i += 16;
    }

    Some(ct_len)
}

pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    hmac_sha256(salt, ikm)
}

pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], out: &mut [u8]) {
    let mut t     = [0u8; 32];
    let mut pos   = 0usize;
    let mut count = 1u8;
    while pos < out.len() {
        let mut input = [0u8; 256];
        let mut ilen  = 0usize;
        if count > 1 {
            input[..32].copy_from_slice(&t);
            ilen += 32;
        }
        input[ilen..ilen + info.len()].copy_from_slice(info);
        ilen += info.len();
        input[ilen] = count;
        ilen += 1;
        t = hmac_sha256(prk, &input[..ilen]);
        let take = (out.len() - pos).min(32);
        out[pos..pos + take].copy_from_slice(&t[..take]);
        pos   += take;
        count += 1;
    }
}

pub fn hkdf_expand_label(prk: &[u8; 32], label: &[u8], context: &[u8], out: &mut [u8]) {
    let len    = out.len() as u16;
    let label_full = {
        let mut buf = [0u8; 64];
        buf[..6].copy_from_slice(b"tls13 ");
        buf[6..6 + label.len()].copy_from_slice(label);
        (buf, 6 + label.len())
    };

    let mut info = [0u8; 256];
    let mut ip   = 0usize;
    info[ip..ip+2].copy_from_slice(&len.to_be_bytes()); ip += 2;
    info[ip] = label_full.1 as u8; ip += 1;
    info[ip..ip + label_full.1].copy_from_slice(&label_full.0[..label_full.1]); ip += label_full.1;
    info[ip] = context.len() as u8; ip += 1;
    info[ip..ip + context.len()].copy_from_slice(context); ip += context.len();

    hkdf_expand(prk, &info[..ip], out);
}

pub fn derive_secret(secret: &[u8; 32], label: &[u8], transcript_hash: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf_expand_label(secret, label, transcript_hash, &mut out);
    out
}
