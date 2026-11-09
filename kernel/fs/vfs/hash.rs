pub use miku_base::fnv::fnv1a_32 as fnv32;

#[inline]
pub fn name_hash(name: &str) -> u32 {
    fnv32(name.as_bytes())
}


pub fn content_hash(data: &[u8]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut h: [u32; 8] = [
        0x811c9dc5,
        0x01000193,
        0x811c9dc5 ^ 0xdeadbeef,
        0x01000193 ^ 0xcafebabe,
        0x811c9dc5 ^ 0x12345678,
        0x01000193 ^ 0x9abcdef0,
        0x811c9dc5 ^ 0xfedcba98,
        0x01000193 ^ 0x76543210,
    ];
    for (i, &b) in data.iter().enumerate() {
        let lane = i & 7;
        h[lane] ^= b as u32;
        h[lane] = h[lane].wrapping_mul(0x01000193);
    }
    for i in 0..8 {
        let bytes = h[i].to_le_bytes();
        result[i * 4..i * 4 + 4].copy_from_slice(&bytes);
    }
    result
}
