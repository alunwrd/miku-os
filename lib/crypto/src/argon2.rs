// Argon2id, RFC 9106: the /etc/shadow password hash
//
// Memory-hard, so GPUs gain little. The id variant indexes data-independently
// in the first slices, keeping memory access secret-independent, and
// data-dependently afterwards, making time-memory tradeoffs expensive.
//
// The reference algorithm, checked against the RFC's vectors in
// tests/argon2.rs.

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

use crate::blake2b::{blake2b_long, Blake2b};

/// Block: 1024 bytes, as 128 64-bit words
const BLOCK_WORDS: usize = 128;
const BLOCK_BYTES: usize = BLOCK_WORDS * 8;

/// Argon2id
const TYPE_ID: u32 = 2;
const VERSION: u32 = 0x13;

/// Hash parameters. Default: 19 MiB over three passes (OWASP's profile for
/// machines not dedicated to authentication). Each stored hash carries its
/// parameters, so raising the default does not invalidate existing passwords
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Memory, in kibibytes
    pub m_cost: u32,
    /// Passes over that memory
    pub t_cost: u32,
    /// Lanes. Implemented so hashes from other systems verify; walked in order,
    /// since lanes in a slice are independent
    pub p_cost: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self { m_cost: 19456, t_cost: 3, p_cost: 1 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Argon2Error {
    /// m_cost below 8*p, above the ceiling, or t_cost zero
    BadParams,
    /// p_cost zero or above the ceiling
    Unsupported,
    /// Salt shorter than 8 bytes or longer than the ceiling
    BadSalt,
    /// Requested output length outside 4..=64
    BadOutputLen,
    /// Could not allocate m_cost KiB
    OutOfMemory,
}

/// Largest m_cost in KiB, since verify parameters come from /etc/shadow
pub const MAX_M_COST: u32 = 1 << 20; // 1 GiB
pub const MAX_T_COST: u32 = 64;
/// Most lanes a stored hash may name
pub const MAX_P_COST: u32 = 64;
pub const MIN_SALT: usize = 8;
pub const MAX_SALT: usize = 64;

impl Params {
    pub fn validate(&self) -> Result<(), Argon2Error> {
        if self.p_cost == 0 || self.p_cost > MAX_P_COST {
            return Err(Argon2Error::Unsupported);
        }
        if self.t_cost == 0 || self.t_cost > MAX_T_COST {
            return Err(Argon2Error::BadParams);
        }
        if self.m_cost < 8 * self.p_cost || self.m_cost > MAX_M_COST {
            return Err(Argon2Error::BadParams);
        }
        Ok(())
    }
}

type Block = [u64; BLOCK_WORDS];

#[inline]
fn xor_into(dst: &mut Block, src: &Block) {
    for i in 0..BLOCK_WORDS {
        dst[i] ^= src[i];
    }
}

/// Permutation P on eight 16-byte cells (RFC 9106 3.6)
#[inline]
#[allow(clippy::too_many_arguments)]
fn p_round(b: &mut [u64; 16]) {
    macro_rules! gb {
        ($a:expr, $bb:expr, $c:expr, $d:expr) => {{
            // The multiply makes a step depend on all 64 input bits
            let (mut a, mut bv, mut c, mut d) = (b[$a], b[$bb], b[$c], b[$d]);
            a = a.wrapping_add(bv).wrapping_add(
                2u64.wrapping_mul((a as u32 as u64).wrapping_mul(bv as u32 as u64)));
            d = (d ^ a).rotate_right(32);
            c = c.wrapping_add(d).wrapping_add(
                2u64.wrapping_mul((c as u32 as u64).wrapping_mul(d as u32 as u64)));
            bv = (bv ^ c).rotate_right(24);
            a = a.wrapping_add(bv).wrapping_add(
                2u64.wrapping_mul((a as u32 as u64).wrapping_mul(bv as u32 as u64)));
            d = (d ^ a).rotate_right(16);
            c = c.wrapping_add(d).wrapping_add(
                2u64.wrapping_mul((c as u32 as u64).wrapping_mul(d as u32 as u64)));
            bv = (bv ^ c).rotate_right(63);
            b[$a] = a; b[$bb] = bv; b[$c] = c; b[$d] = d;
        }};
    }
    gb!(0, 4,  8, 12);
    gb!(1, 5,  9, 13);
    gb!(2, 6, 10, 14);
    gb!(3, 7, 11, 15);
    gb!(0, 5, 10, 15);
    gb!(1, 6, 11, 12);
    gb!(2, 7,  8, 13);
    gb!(3, 4,  9, 14);
}

/// Compression G(X, Y), RFC 9106 3.5: R = X xor Y; P on rows, then columns; xor
/// with R. `with_xor` xors into the existing block, for later passes
fn compress(out: &mut Block, x: &Block, y: &Block, with_xor: bool) {
    let mut r = [0u64; BLOCK_WORDS];
    for i in 0..BLOCK_WORDS {
        r[i] = x[i] ^ y[i];
    }
    let mut q = r;

    // Rows: eight contiguous 16-word slices
    for row in 0..8 {
        let mut v = [0u64; 16];
        v.copy_from_slice(&q[row * 16..row * 16 + 16]);
        p_round(&mut v);
        q[row * 16..row * 16 + 16].copy_from_slice(&v);
    }
    // Columns: two words from each row
    for col in 0..8 {
        let mut v = [0u64; 16];
        for i in 0..8 {
            v[2 * i]     = q[16 * i + 2 * col];
            v[2 * i + 1] = q[16 * i + 2 * col + 1];
        }
        p_round(&mut v);
        for i in 0..8 {
            q[16 * i + 2 * col]     = v[2 * i];
            q[16 * i + 2 * col + 1] = v[2 * i + 1];
        }
    }

    for i in 0..BLOCK_WORDS {
        let val = q[i] ^ r[i];
        if with_xor {
            out[i] ^= val;
        } else {
            out[i] = val;
        }
    }
}

fn u32le(v: u32) -> [u8; 4] { v.to_le_bytes() }

/// Hashes `password` with `salt` into `out`. `out.len()` is the tag length,
/// 4..=64; the shadow format stores 32.
pub fn hash_into(
    password: &[u8],
    salt: &[u8],
    params: Params,
    out: &mut [u8],
) -> Result<(), Argon2Error> {
    hash_full(password, salt, &[], &[], params, out)
}

/// With Argon2's optional inputs: `secret` (a pepper) and `ad` (associated
/// data). MikuOS passes neither; the spec's test vectors use them
pub fn hash_full(
    password: &[u8],
    salt: &[u8],
    secret: &[u8],
    ad: &[u8],
    params: Params,
    out: &mut [u8],
) -> Result<(), Argon2Error> {
    params.validate()?;
    if salt.len() < MIN_SALT || salt.len() > MAX_SALT {
        return Err(Argon2Error::BadSalt);
    }
    if out.len() < 4 || out.len() > 64 {
        return Err(Argon2Error::BadOutputLen);
    }

    // m rounds down to a multiple of 4*p; validate() keeps it above zero
    let lanes = params.p_cost as usize;
    let m_prime = (params.m_cost / (4 * params.p_cost)) * 4 * params.p_cost;
    let blocks_n = m_prime as usize;
    let lane_len = blocks_n / lanes;
    let segment = lane_len / 4;

    // H0: 64-byte pre-hash of every parameter, including the tag length
    let mut h0 = [0u8; 72];
    {
        let mut h = Blake2b::new(64);
        h.update(&u32le(params.p_cost));
        h.update(&u32le(out.len() as u32));
        h.update(&u32le(params.m_cost));
        h.update(&u32le(params.t_cost));
        h.update(&u32le(VERSION));
        h.update(&u32le(TYPE_ID));
        h.update(&u32le(password.len() as u32));
        h.update(password);
        h.update(&u32le(salt.len() as u32));
        h.update(salt);
        h.update(&u32le(secret.len() as u32));
        h.update(secret);
        h.update(&u32le(ad.len() as u32));
        h.update(ad);
        let mut digest = [0u8; 64];
        h.finalize_into(&mut digest);
        h0[..64].copy_from_slice(&digest);
    }

    let mut mem: Vec<Block> = vec![[0u64; BLOCK_WORDS]; blocks_n];

    // The first two blocks of each lane come straight from H0
    for lane in 0..lanes {
        for idx in 0..2usize {
            h0[64..68].copy_from_slice(&u32le(idx as u32));
            h0[68..72].copy_from_slice(&u32le(lane as u32));
            let mut block_bytes = [0u8; BLOCK_BYTES];
            blake2b_long(&h0, &mut block_bytes);
            let dst = &mut mem[lane * lane_len + idx];
            for w in 0..BLOCK_WORDS {
                let mut b = [0u8; 8];
                b.copy_from_slice(&block_bytes[w * 8..w * 8 + 8]);
                dst[w] = u64::from_le_bytes(b);
            }
        }
    }

    for pass in 0..params.t_cost {
        for slice in 0..4usize {
            // Lanes in order. The spec only requires that a lane not read
            // another's current slice, which the addressing rule ensures
            for lane in 0..lanes {
                // Data-independent for the first two slices of the first pass,
                // data-dependent after
                let independent = pass == 0 && slice < 2;
                let mut addr_block = [0u64; BLOCK_WORDS];
                let mut input_block = [0u64; BLOCK_WORDS];
                if independent {
                    input_block[0] = pass as u64;
                    input_block[1] = lane as u64;
                    input_block[2] = slice as u64;
                    input_block[3] = blocks_n as u64;
                    input_block[4] = params.t_cost as u64;
                    input_block[5] = TYPE_ID as u64;
                }

                let start = if pass == 0 && slice == 0 { 2 } else { 0 };
                for index in start..segment {
                    let cur = lane * lane_len + slice * segment + index;
                    let prev = if cur % lane_len == 0 { cur + lane_len - 1 } else { cur - 1 };

                    let pseudo_rand = if independent {
                        let i = index % BLOCK_WORDS;
                        if i == 0 {
                            input_block[6] += 1;
                            let zero = [0u64; BLOCK_WORDS];
                            let inp = input_block;
                            compress(&mut addr_block, &zero, &inp, false);
                            let once = addr_block;
                            compress(&mut addr_block, &zero, &once, false);
                        }
                        addr_block[i]
                    } else {
                        mem[prev][0]
                    };

                    // High half picks the lane, low half the block. On the
                    // first slice of the first pass a lane references itself
                    let ref_lane = if pass == 0 && slice == 0 {
                        lane
                    } else {
                        ((pseudo_rand >> 32) % lanes as u64) as usize
                    };
                    let same_lane = ref_lane == lane;

                    let ref_index = index_alpha(
                        pass, slice, index, segment, lane_len, same_lane,
                        pseudo_rand as u32,
                    );
                    let ref_block_idx = ref_lane * lane_len + ref_index;

                    // Copies `prev` and `ref` to stay in safe code with
                    // disjoint indices
                    let prev_block = mem[prev];
                    let ref_block = mem[ref_block_idx];
                    compress(&mut mem[cur], &prev_block, &ref_block, pass > 0);
                }
            }
        }
    }

    // Final block: XOR of each lane's last block
    let mut final_block = mem[lane_len - 1];
    for lane in 1..lanes {
        let last = mem[lane * lane_len + lane_len - 1];
        xor_into(&mut final_block, &last);
    }

    let mut final_bytes = [0u8; BLOCK_BYTES];
    for w in 0..BLOCK_WORDS {
        final_bytes[w * 8..w * 8 + 8].copy_from_slice(&final_block[w].to_le_bytes());
    }
    blake2b_long(&final_bytes, out);

    // Wipes memory derived from the password before the allocator gets it back
    for b in mem.iter_mut() {
        b.fill(0);
    }
    final_block.fill(0);
    final_bytes.fill(0);
    h0.fill(0);
    Ok(())
}

/// Reference block position in its lane (RFC 9106 3.4.1.2). Squaring biases
/// toward recent blocks. `same_lane` allows everything written this pass
/// including the current segment; another lane only up to the end of the
/// previous slice
fn index_alpha(
    pass: u32,
    slice: usize,
    index: usize,
    segment: usize,
    lane_len: usize,
    same_lane: bool,
    rand: u32,
) -> usize {
    let ref_area: usize = if pass == 0 {
        if slice == 0 {
            // Only what this lane has written
            index - 1
        } else if same_lane {
            slice * segment + index - 1
        } else {
            slice * segment - if index == 0 { 1 } else { 0 }
        }
    } else if same_lane {
        lane_len - segment + index - 1
    } else {
        lane_len - segment - if index == 0 { 1 } else { 0 }
    };

    let r = ref_area as u64;
    let x = (rand as u64).wrapping_mul(rand as u64) >> 32;
    let y = r.wrapping_mul(x) >> 32;
    let z = r.saturating_sub(1).saturating_sub(y);

    // Window start: the lane's start on the first pass, otherwise the slice
    // after the current one, wrapping
    let start = if pass == 0 || slice == 3 { 0 } else { (slice + 1) * segment };
    (start + z as usize) % lane_len
}
