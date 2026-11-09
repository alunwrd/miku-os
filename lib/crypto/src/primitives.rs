const SBOX: [u8; 256] = [
    0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
    0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
    0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
    0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
    0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
    0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
    0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
    0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
    0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
    0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
    0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
    0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
    0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
    0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
    0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
    0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16,
];

const SBOX_INV: [u8; 256] = [
    0x52,0x09,0x6a,0xd5,0x30,0x36,0xa5,0x38,0xbf,0x40,0xa3,0x9e,0x81,0xf3,0xd7,0xfb,
    0x7c,0xe3,0x39,0x82,0x9b,0x2f,0xff,0x87,0x34,0x8e,0x43,0x44,0xc4,0xde,0xe9,0xcb,
    0x54,0x7b,0x94,0x32,0xa6,0xc2,0x23,0x3d,0xee,0x4c,0x95,0x0b,0x42,0xfa,0xc3,0x4e,
    0x08,0x2e,0xa1,0x66,0x28,0xd9,0x24,0xb2,0x76,0x5b,0xa2,0x49,0x6d,0x8b,0xd1,0x25,
    0x72,0xf8,0xf6,0x64,0x86,0x68,0x98,0x16,0xd4,0xa4,0x5c,0xcc,0x5d,0x65,0xb6,0x92,
    0x6c,0x70,0x48,0x50,0xfd,0xed,0xb9,0xda,0x5e,0x15,0x46,0x57,0xa7,0x8d,0x9d,0x84,
    0x90,0xd8,0xab,0x00,0x8c,0xbc,0xd3,0x0a,0xf7,0xe4,0x58,0x05,0xb8,0xb3,0x45,0x06,
    0xd0,0x2c,0x1e,0x8f,0xca,0x3f,0x0f,0x02,0xc1,0xaf,0xbd,0x03,0x01,0x13,0x8a,0x6b,
    0x3a,0x91,0x11,0x41,0x4f,0x67,0xdc,0xea,0x97,0xf2,0xcf,0xce,0xf0,0xb4,0xe6,0x73,
    0x96,0xac,0x74,0x22,0xe7,0xad,0x35,0x85,0xe2,0xf9,0x37,0xe8,0x1c,0x75,0xdf,0x6e,
    0x47,0xf1,0x1a,0x71,0x1d,0x29,0xc5,0x89,0x6f,0xb7,0x62,0x0e,0xaa,0x18,0xbe,0x1b,
    0xfc,0x56,0x3e,0x4b,0xc6,0xd2,0x79,0x20,0x9a,0xdb,0xc0,0xfe,0x78,0xcd,0x5a,0xf4,
    0x1f,0xdd,0xa8,0x33,0x88,0x07,0xc7,0x31,0xb1,0x12,0x10,0x59,0x27,0x80,0xec,0x5f,
    0x60,0x51,0x7f,0xa9,0x19,0xb5,0x4a,0x0d,0x2d,0xe5,0x7a,0x9f,0x93,0xc9,0x9c,0xef,
    0xa0,0xe0,0x3b,0x4d,0xae,0x2a,0xf5,0xb0,0xc8,0xeb,0xbb,0x3c,0x83,0x53,0x99,0x61,
    0x17,0x2b,0x04,0x7e,0xba,0x77,0xd6,0x26,0xe1,0x69,0x14,0x63,0x55,0x21,0x0c,0x7d,
];

const RCON: [u8; 10] = [0x01,0x02,0x04,0x08,0x10,0x20,0x40,0x80,0x1b,0x36];

fn xtime(a: u8) -> u8 {
    if a & 0x80 != 0 { (a << 1) ^ 0x1b } else { a << 1 }
}

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 { p ^= a; }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 { a ^= 0x1b; }
        b >>= 1;
    }
    p
}

pub struct Aes128 {
    rk: [u8; 176],
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Self {
        let mut rk = [0u8; 176];
        rk[..16].copy_from_slice(key);
        for i in 1..=10usize {
            let base = i * 16;
            let prev = base - 16;
            let t0 = SBOX[rk[base-3] as usize]; 
            let t1 = SBOX[rk[base-2] as usize]; 
            let t2 = SBOX[rk[base-1] as usize]; 
            let t3 = SBOX[rk[base-4] as usize]; 
            rk[base]   = rk[prev]   ^ t0 ^ RCON[i-1];
            rk[base+1] = rk[prev+1] ^ t1;
            rk[base+2] = rk[prev+2] ^ t2;
            rk[base+3] = rk[prev+3] ^ t3;
            for j in 4..16 {
                rk[base+j] = rk[prev+j] ^ rk[base+j-4];
            }
        }
        Self { rk }
    }

    pub fn encrypt_block(&self, b: &mut [u8; 16]) {
        for i in 0..16 { b[i] ^= self.rk[i]; }
        for round in 1..=10usize {
            for i in 0..16 { b[i] = SBOX[b[i] as usize]; }
            let tmp = b[1]; b[1] = b[5]; b[5] = b[9]; b[9] = b[13]; b[13] = tmp;
            let tmp = b[2]; b[2] = b[10]; b[10] = tmp;
            let tmp = b[6]; b[6] = b[14]; b[14] = tmp;
            let tmp = b[15]; b[15] = b[11]; b[11] = b[7]; b[7] = b[3]; b[3] = tmp;
            if round < 10 {
                for col in 0..4usize {
                    let s0 = b[col*4]; let s1 = b[col*4+1];
                    let s2 = b[col*4+2]; let s3 = b[col*4+3];
                    b[col*4]   = xtime(s0)^xtime(s1)^s1^s2^s3;
                    b[col*4+1] = s0^xtime(s1)^xtime(s2)^s2^s3;
                    b[col*4+2] = s0^s1^xtime(s2)^xtime(s3)^s3;
                    b[col*4+3] = xtime(s0)^s0^s1^s2^xtime(s3);
                }
            }
            let rk_off = round * 16;
            for i in 0..16 { b[i] ^= self.rk[rk_off + i]; }
        }
    }

    pub fn decrypt_block(&self, b: &mut [u8; 16]) {
        for i in 0..16 { b[i] ^= self.rk[160 + i]; }
        for round in (0..10usize).rev() {
            let tmp = b[13]; b[13] = b[9]; b[9] = b[5]; b[5] = b[1]; b[1] = tmp;
            let tmp = b[2]; b[2] = b[10]; b[10] = tmp;
            let tmp = b[6]; b[6] = b[14]; b[14] = tmp;
            let tmp = b[3]; b[3] = b[7]; b[7] = b[11]; b[11] = b[15]; b[15] = tmp;
            for i in 0..16 { b[i] = SBOX_INV[b[i] as usize]; }
            let rk_off = round * 16;
            for i in 0..16 { b[i] ^= self.rk[rk_off + i]; }
            if round > 0 {
                for col in 0..4usize {
                    let s0 = b[col*4]; let s1 = b[col*4+1];
                    let s2 = b[col*4+2]; let s3 = b[col*4+3];
                    b[col*4]   = gf_mul(14,s0)^gf_mul(11,s1)^gf_mul(13,s2)^gf_mul(9,s3);
                    b[col*4+1] = gf_mul(9,s0)^gf_mul(14,s1)^gf_mul(11,s2)^gf_mul(13,s3);
                    b[col*4+2] = gf_mul(13,s0)^gf_mul(9,s1)^gf_mul(14,s2)^gf_mul(11,s3);
                    b[col*4+3] = gf_mul(11,s0)^gf_mul(13,s1)^gf_mul(9,s2)^gf_mul(14,s3);
                }
            }
        }
    }
}

pub fn cbc_encrypt(key: &[u8; 16], iv: &[u8; 16], plaintext: &[u8], out: &mut [u8]) -> usize {
    let aes = Aes128::new(key);
    let mut prev = *iv;
    let n = plaintext.len();
    for i in 0..(n / 16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(&plaintext[i*16..(i+1)*16]);
        for j in 0..16 { block[j] ^= prev[j]; }
        aes.encrypt_block(&mut block);
        out[i*16..(i+1)*16].copy_from_slice(&block);
        prev = block;
    }
    n
}

pub fn cbc_decrypt(key: &[u8; 16], iv: &[u8; 16], ciphertext: &[u8], out: &mut [u8]) -> usize {
    let aes = Aes128::new(key);
    let mut prev = *iv;
    let n = ciphertext.len();
    for i in 0..(n / 16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(&ciphertext[i*16..(i+1)*16]);
        let cipher_block = block;
        aes.decrypt_block(&mut block);
        for j in 0..16 { block[j] ^= prev[j]; }
        out[i*16..(i+1)*16].copy_from_slice(&block);
        prev = cipher_block;
    }
    n
}

pub fn tls_pad(data: &[u8], out: &mut [u8]) -> usize {
    let pad_len = 16 - (data.len() % 16);
    let total = data.len() + pad_len;
    out[..data.len()].copy_from_slice(data);
    
    let pad_val = (pad_len - 1) as u8; 
    
    for i in data.len()..total {
        out[i] = pad_val;
    }
    total
}

pub fn tls_unpad(data: &[u8]) -> Option<&[u8]> {
    if data.is_empty() { return None; }

    let pad_val = *data.last().unwrap() as usize;
    let pad_len = pad_val + 1;

    if pad_len > 16 || pad_len > data.len() { return None; }

    let pad_start = data.len() - pad_len;
    let mut diff = 0u8;
    for b in &data[pad_start..] {
        diff |= *b ^ pad_val as u8;
    }
    if diff != 0 { return None; }

    Some(&data[..pad_start])
}

pub struct Sha1State {
    h: [u32; 5],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Sha1State {
    pub fn new() -> Self {
        Self {
            h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buf: [0u8; 64],
            buf_len: 0,
            total: 0,
        }
    }

    fn compress(&mut self) {
        let mut w = [0u32; 80];
        for t in 0..16 {
            w[t] = u32::from_be_bytes([
                self.buf[t*4], self.buf[t*4+1], self.buf[t*4+2], self.buf[t*4+3]
            ]);
        }
        for t in 16..80 {
            w[t] = (w[t-3]^w[t-8]^w[t-14]^w[t-16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4]);
        for t in 0..80 {
            let (f, k) = match t {
                0..=19  => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _       => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[t]);
            e=d; d=c; c=b.rotate_left(30); b=a; a=tmp;
        }
        self.h[0]=self.h[0].wrapping_add(a); self.h[1]=self.h[1].wrapping_add(b);
        self.h[2]=self.h[2].wrapping_add(c); self.h[3]=self.h[3].wrapping_add(d); self.h[4]=self.h[4].wrapping_add(e);
    }

    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.buf_len] = b;
            self.buf_len += 1;
            self.total += 8; 
            if self.buf_len == 64 {
                self.compress();
                self.buf_len = 0;
            }
        }
    }

    pub fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.total;
        self.update(&[0x80]);
        while self.buf_len != 56 { self.update(&[0x00]); }
        self.update(&bit_len.to_be_bytes());
        let mut out = [0u8; 20];
        for i in 0..5 { out[i*4..(i+1)*4].copy_from_slice(&self.h[i].to_be_bytes()); }
        out
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut s = Sha1State::new();
    s.update(data);
    s.finalize()
}

pub struct Sha256State {
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

const K256: [u32; 64] = [
    0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
    0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
    0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
    0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
    0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
    0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
    0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
    0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
];

/// One SHA-256 block into a running state
pub fn sha256_compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([block[i*4],block[i*4+1],block[i*4+2],block[i*4+3]]);
    }
    for i in 16..64 {
        let s0 = w[i-15].rotate_right(7)^w[i-15].rotate_right(18)^(w[i-15]>>3);
        let s1 = w[i-2].rotate_right(17)^w[i-2].rotate_right(19)^(w[i-2]>>10);
        w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
    }
    let (mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut h) =
        (state[0],state[1],state[2],state[3],state[4],state[5],state[6],state[7]);
    for i in 0..64 {
        let s1_val = e.rotate_right(6)^e.rotate_right(11)^e.rotate_right(25);
        let ch = (e&f)^((!e)&g);
        let t1 = h.wrapping_add(s1_val).wrapping_add(ch).wrapping_add(K256[i]).wrapping_add(w[i]);
        let s0_val = a.rotate_right(2)^a.rotate_right(13)^a.rotate_right(22);
        let maj = (a&b)^(a&c)^(b&c);
        let t2 = s0_val.wrapping_add(maj);
        h=g; g=f; f=e; e=d.wrapping_add(t1);
        d=c; c=b; b=a; a=t1.wrapping_add(t2);
    }
    state[0]=state[0].wrapping_add(a); state[1]=state[1].wrapping_add(b);
    state[2]=state[2].wrapping_add(c); state[3]=state[3].wrapping_add(d);
    state[4]=state[4].wrapping_add(e); state[5]=state[5].wrapping_add(f);
    state[6]=state[6].wrapping_add(g); state[7]=state[7].wrapping_add(h);
}

impl Sha256State {
    pub fn new() -> Self {
        Self {
            h: [0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
                0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19],
            buf: [0u8; 64],
            buf_len: 0,
            total: 0,
        }
    }

    fn compress(&mut self) {
        let block = self.buf;
        sha256_compress(&mut self.h, &block);
    }

    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.buf_len] = b;
            self.buf_len += 1;
            self.total += 8;
            if self.buf_len == 64 {
                self.compress();
                self.buf_len = 0;
            }
        }
    }

    pub fn clone_finalize(&self) -> [u8; 32] {
        let copy = Sha256State {
            h: self.h,
            buf: self.buf,
            buf_len: self.buf_len,
            total: self.total,
        };
        copy.finalize()
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total;
        self.update(&[0x80]);
        while self.buf_len != 56 { self.update(&[0x00]); }
        self.update(&bit_len.to_be_bytes());
        let mut out = [0u8; 32];
        for i in 0..8 { out[i*4..(i+1)*4].copy_from_slice(&self.h[i].to_be_bytes()); }
        out
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut s = Sha256State::new();
    s.update(data);
    s.finalize()
}

pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let h = sha1(key);
        k[..20].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    for i in 0..64 { ipad[i] = k[i] ^ 0x36; opad[i] = k[i] ^ 0x5c; }

    let mut s = Sha1State::new();
    s.update(&ipad);
    s.update(data);
    let inner_hash = s.finalize();

    let mut s2 = Sha1State::new();
    s2.update(&opad);
    s2.update(&inner_hash);
    s2.finalize()
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let h = sha256(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    for i in 0..64 { ipad[i] = k[i] ^ 0x36; opad[i] = k[i] ^ 0x5c; }

    let mut s = Sha256State::new();
    s.update(&ipad);
    s.update(data);
    let h1 = s.finalize();

    let mut s2 = Sha256State::new();
    s2.update(&opad);
    s2.update(&h1);
    s2.finalize()
}

pub fn prf_sha256(secret: &[u8], label: &[u8], seed: &[u8], out: &mut [u8]) {
    let mut ls = [0u8; 128];
    let lslen = label.len() + seed.len();
    ls[..label.len()].copy_from_slice(label);
    ls[label.len()..lslen].copy_from_slice(seed);
    let ls = &ls[..lslen];

    let mut a = hmac_sha256(secret, ls);
    let mut pos = 0;
    while pos < out.len() {
        let mut ab = [0u8; 160];
        ab[..32].copy_from_slice(&a);
        ab[32..32+lslen].copy_from_slice(ls);
        let h = hmac_sha256(secret, &ab[..32+lslen]);
        let take = (out.len() - pos).min(32);
        out[pos..pos+take].copy_from_slice(&h[..take]);
        pos += take;
        a = hmac_sha256(secret, &a);
    }
}

// SHA-384 / SHA-512, for certificate chains using sha384WithRSAEncryption and
// ecdsa-with-SHA384. SHA-384 is SHA-512 with another starting state and a
// truncated output

const K512: [u64; 80] = [
    0x428a2f98d728ae22,0x7137449123ef65cd,0xb5c0fbcfec4d3b2f,0xe9b5dba58189dbbc,
    0x3956c25bf348b538,0x59f111f1b605d019,0x923f82a4af194f9b,0xab1c5ed5da6d8118,
    0xd807aa98a3030242,0x12835b0145706fbe,0x243185be4ee4b28c,0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,0x80deb1fe3b1696b1,0x9bdc06a725c71235,0xc19bf174cf692694,
    0xe49b69c19ef14ad2,0xefbe4786384f25e3,0x0fc19dc68b8cd5b5,0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,0x4a7484aa6ea6e483,0x5cb0a9dcbd41fbd4,0x76f988da831153b5,
    0x983e5152ee66dfab,0xa831c66d2db43210,0xb00327c898fb213f,0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,0xd5a79147930aa725,0x06ca6351e003826f,0x142929670a0e6e70,
    0x27b70a8546d22ffc,0x2e1b21385c26c926,0x4d2c6dfc5ac42aed,0x53380d139d95b3df,
    0x650a73548baf63de,0x766a0abb3c77b2a8,0x81c2c92e47edaee6,0x92722c851482353b,
    0xa2bfe8a14cf10364,0xa81a664bbc423001,0xc24b8b70d0f89791,0xc76c51a30654be30,
    0xd192e819d6ef5218,0xd69906245565a910,0xf40e35855771202a,0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,0x1e376c085141ab53,0x2748774cdf8eeb99,0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,0x4ed8aa4ae3418acb,0x5b9cca4f7763e373,0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,0x78a5636f43172f60,0x84c87814a1f0ab72,0x8cc702081a6439ec,
    0x90befffa23631e28,0xa4506cebde82bde9,0xbef9a3f7b2c67915,0xc67178f2e372532b,
    0xca273eceea26619c,0xd186b8c721c0c207,0xeada7dd6cde0eb1e,0xf57d4f7fee6ed178,
    0x06f067aa72176fba,0x0a637dc5a2c898a6,0x113f9804bef90dae,0x1b710b35131c471b,
    0x28db77f523047d84,0x32caab7b40c72493,0x3c9ebe0a15c9bebc,0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,0x597f299cfc657e2a,0x5fcb6fab3ad6faec,0x6c44198c4a475817,
];

pub struct Sha512State {
    h: [u64; 8],
    buf: [u8; 128],
    buf_len: usize,
    /// Message length in bits. SHA-512 reserves 128 bits; a u64 is enough here
    total: u64,
}

impl Sha512State {
    pub fn new() -> Self {
        Self {
            h: [0x6a09e667f3bcc908,0xbb67ae8584caa73b,0x3c6ef372fe94f82b,0xa54ff53a5f1d36f1,
                0x510e527fade682d1,0x9b05688c2b3e6c1f,0x1f83d9abfb41bd6b,0x5be0cd19137e2179],
            buf: [0u8; 128], buf_len: 0, total: 0,
        }
    }

    pub fn new_384() -> Self {
        Self {
            h: [0xcbbb9d5dc1059ed8,0x629a292a367cd507,0x9159015a3070dd17,0x152fecd8f70e5939,
                0x67332667ffc00b31,0x8eb44a8768581511,0xdb0c2e0d64f98fa7,0x47b5481dbefa4fa4],
            buf: [0u8; 128], buf_len: 0, total: 0,
        }
    }

    fn compress(&mut self) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            let mut v = 0u64;
            for j in 0..8 { v = (v << 8) | self.buf[i * 8 + j] as u64; }
            w[i] = v;
        }
        for i in 16..80 {
            let s0 = w[i-15].rotate_right(1) ^ w[i-15].rotate_right(8) ^ (w[i-15] >> 7);
            let s1 = w[i-2].rotate_right(19) ^ w[i-2].rotate_right(61) ^ (w[i-2] >> 6);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let (mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut h) =
            (self.h[0],self.h[1],self.h[2],self.h[3],self.h[4],self.h[5],self.h[6],self.h[7]);
        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K512[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h=g; g=f; f=e; e=d.wrapping_add(t1);
            d=c; c=b; b=a; a=t1.wrapping_add(t2);
        }
        self.h[0]=self.h[0].wrapping_add(a); self.h[1]=self.h[1].wrapping_add(b);
        self.h[2]=self.h[2].wrapping_add(c); self.h[3]=self.h[3].wrapping_add(d);
        self.h[4]=self.h[4].wrapping_add(e); self.h[5]=self.h[5].wrapping_add(f);
        self.h[6]=self.h[6].wrapping_add(g); self.h[7]=self.h[7].wrapping_add(h);
    }

    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.buf_len] = b;
            self.buf_len += 1;
            self.total = self.total.wrapping_add(8);
            if self.buf_len == 128 {
                self.compress();
                self.buf_len = 0;
            }
        }
    }

    fn finish(mut self) -> [u64; 8] {
        let bit_len = self.total;
        self.update(&[0x80]);
        while self.buf_len != 112 { self.update(&[0x00]); }
        self.update(&[0u8; 8]);              // high half of the 128-bit length
        self.update(&bit_len.to_be_bytes());
        self.h
    }

    pub fn finalize(self) -> [u8; 64] {
        let h = self.finish();
        let mut out = [0u8; 64];
        for i in 0..8 { out[i*8..(i+1)*8].copy_from_slice(&h[i].to_be_bytes()); }
        out
    }

    pub fn finalize_384(self) -> [u8; 48] {
        let h = self.finish();
        let mut out = [0u8; 48];
        for i in 0..6 { out[i*8..(i+1)*8].copy_from_slice(&h[i].to_be_bytes()); }
        out
    }
}

pub fn sha512(data: &[u8]) -> [u8; 64] {
    let mut s = Sha512State::new();
    s.update(data);
    s.finalize()
}

pub fn sha384(data: &[u8]) -> [u8; 48] {
    let mut s = Sha512State::new_384();
    s.update(data);
    s.finalize_384()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hash { Sha1, Sha256, Sha384, Sha512 }

impl Hash {
    pub fn len(&self) -> usize {
        match self { Hash::Sha1 => 20, Hash::Sha256 => 32, Hash::Sha384 => 48, Hash::Sha512 => 64 }
    }
    /// Digest into a 64-byte buffer, returning the written slice
    pub fn digest<'b>(&self, data: &[u8], out: &'b mut [u8; 64]) -> &'b [u8] {
        match self {
            Hash::Sha1   => { out[..20].copy_from_slice(&sha1(data));   &out[..20] }
            Hash::Sha256 => { out[..32].copy_from_slice(&sha256(data)); &out[..32] }
            Hash::Sha384 => { out[..48].copy_from_slice(&sha384(data)); &out[..48] }
            Hash::Sha512 => { out[..64].copy_from_slice(&sha512(data)); &out[..64] }
        }
    }
}
