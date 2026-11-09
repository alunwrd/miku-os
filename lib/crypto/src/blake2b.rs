// BLAKE2b, RFC 7693, for Argon2: a keyed hash of arbitrary digest length and
// the variable-length extension H'

/// IV: fractional parts of the square roots of the first eight primes
const IV: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];

/// Message permutation. Rounds 10 and 11 reuse the first two
const SIGMA: [[usize; 16]; 12] = [
    [ 0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15],
    [14, 10,  4,  8,  9, 15, 13,  6,  1, 12,  0,  2, 11,  7,  5,  3],
    [11,  8, 12,  0,  5,  2, 15, 13, 10, 14,  3,  6,  7,  1,  9,  4],
    [ 7,  9,  3,  1, 13, 12, 11, 14,  2,  6,  5, 10,  4,  0, 15,  8],
    [ 9,  0,  5,  7,  2,  4, 10, 15, 14,  1, 11, 12,  6,  8,  3, 13],
    [ 2, 12,  6, 10,  0, 11,  8,  3,  4, 13,  7,  5, 15, 14,  1,  9],
    [12,  5,  1, 15, 14, 13,  4, 10,  0,  7,  6,  3,  9,  2,  8, 11],
    [13, 11,  7, 14, 12,  1,  3,  9,  5,  0, 15,  4,  8,  6,  2, 10],
    [ 6, 15, 14,  9, 11,  3,  0,  8, 12,  2, 13,  7,  1,  4, 10,  5],
    [10,  2,  8,  4,  7,  6,  1,  5, 15, 11,  9, 14,  3, 12, 13,  0],
    [ 0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15],
    [14, 10,  4,  8,  9, 15, 13,  6,  1, 12,  0,  2, 11,  7,  5,  3],
];

pub const BLOCK_LEN: usize = 128;
pub const MAX_DIGEST: usize = 64;

pub struct Blake2b {
    h: [u64; 8],
    buf: [u8; BLOCK_LEN],
    buf_len: usize,
    /// Bytes absorbed, a 128-bit counter as in the spec
    t: (u64, u64),
    digest_len: usize,
}

impl Blake2b {
    pub fn new(digest_len: usize) -> Self {
        Self::keyed(digest_len, &[])
    }

    pub fn keyed(digest_len: usize, key: &[u8]) -> Self {
        assert!(digest_len > 0 && digest_len <= MAX_DIGEST);
        assert!(key.len() <= MAX_DIGEST);
        let mut h = IV;
        h[0] ^= 0x0101_0000 ^ ((key.len() as u64) << 8) ^ (digest_len as u64);
        let mut s = Self {
            h,
            buf: [0u8; BLOCK_LEN],
            buf_len: 0,
            t: (0, 0),
            digest_len,
        };
        if !key.is_empty() {
            // The key is one full padded block
            let mut block = [0u8; BLOCK_LEN];
            block[..key.len()].copy_from_slice(key);
            s.update(&block);
        }
        s
    }

    fn add_counter(&mut self, n: u64) {
        let (lo, carry) = self.t.0.overflowing_add(n);
        self.t.0 = lo;
        if carry {
            self.t.1 = self.t.1.wrapping_add(1);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            // A full buffer is compressed only when more arrives, since the
            // last block has the final flag
            if self.buf_len == BLOCK_LEN {
                self.add_counter(BLOCK_LEN as u64);
                let block = self.buf;
                self.compress(&block, false);
                self.buf_len = 0;
            }
            let take = core::cmp::min(BLOCK_LEN - self.buf_len, data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
        }
    }

    pub fn finalize_into(mut self, out: &mut [u8]) {
        assert_eq!(out.len(), self.digest_len);
        self.add_counter(self.buf_len as u64);
        for b in self.buf[self.buf_len..].iter_mut() {
            *b = 0;
        }
        let block = self.buf;
        self.compress(&block, true);
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let word = self.h[i].to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }

    fn compress(&mut self, block: &[u8; BLOCK_LEN], last: bool) {
        let mut m = [0u64; 16];
        for (i, w) in m.iter_mut().enumerate() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&block[i * 8..i * 8 + 8]);
            *w = u64::from_le_bytes(b);
        }

        let mut v = [0u64; 16];
        v[..8].copy_from_slice(&self.h);
        v[8..].copy_from_slice(&IV);
        v[12] ^= self.t.0;
        v[13] ^= self.t.1;
        if last {
            v[14] = !v[14];
        }

        for round in 0..12 {
            let s = &SIGMA[round];
            g(&mut v, 0, 4, 8,  12, m[s[0]],  m[s[1]]);
            g(&mut v, 1, 5, 9,  13, m[s[2]],  m[s[3]]);
            g(&mut v, 2, 6, 10, 14, m[s[4]],  m[s[5]]);
            g(&mut v, 3, 7, 11, 15, m[s[6]],  m[s[7]]);
            g(&mut v, 0, 5, 10, 15, m[s[8]],  m[s[9]]);
            g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(&mut v, 2, 7, 8,  13, m[s[12]], m[s[13]]);
            g(&mut v, 3, 4, 9,  14, m[s[14]], m[s[15]]);
        }

        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

/// One-shot, for a digest that fits in a single call
pub fn blake2b(data: &[u8], out: &mut [u8]) {
    let mut h = Blake2b::new(out.len());
    h.update(data);
    h.finalize_into(out);
}

/// Argon2's H', RFC 9106 3.3: chained digests emitting 32 bytes each, with the
/// output length prefixed to the input as a little-endian u32
pub fn blake2b_long(input: &[u8], out: &mut [u8]) {
    let out_len = out.len();
    let mut len_prefix = [0u8; 4];
    len_prefix.copy_from_slice(&(out_len as u32).to_le_bytes());

    if out_len <= MAX_DIGEST {
        let mut h = Blake2b::new(out_len);
        h.update(&len_prefix);
        h.update(input);
        h.finalize_into(out);
        return;
    }

    let mut buf = [0u8; MAX_DIGEST];
    {
        let mut h = Blake2b::new(MAX_DIGEST);
        h.update(&len_prefix);
        h.update(input);
        h.finalize_into(&mut buf);
    }
    let mut written = 0usize;
    out[..32].copy_from_slice(&buf[..32]);
    written += 32;

    // Each step rehashes the previous digest, until fewer than 64 bytes remain
    while out_len - written > MAX_DIGEST {
        let prev = buf;
        blake2b(&prev, &mut buf);
        out[written..written + 32].copy_from_slice(&buf[..32]);
        written += 32;
    }
    let rest = out_len - written;
    let mut tail = [0u8; MAX_DIGEST];
    blake2b(&buf, &mut tail[..rest]);
    out[written..].copy_from_slice(&tail[..rest]);
}
