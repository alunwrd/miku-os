//! FNV-1a

pub const OFFSET_32: u32 = 0x811c_9dc5;
pub const PRIME_32: u32 = 0x0100_0193;
pub const OFFSET_64: u64 = 0xcbf2_9ce4_8422_2325;
pub const PRIME_64: u64 = 0x0000_0100_0000_01b3;

/// One byte into a running 64-bit hash, for input with no slice to hand
#[inline(always)]
pub const fn step_64(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(PRIME_64)
}

pub fn fnv1a_32(data: &[u8]) -> u32 {
    let mut h = OFFSET_32;
    for &b in data {
        h = (h ^ b as u32).wrapping_mul(PRIME_32);
    }
    h
}

pub fn fnv1a_64(data: &[u8]) -> u64 {
    let mut h = OFFSET_64;
    for &b in data {
        h = step_64(h, b);
    }
    h
}
