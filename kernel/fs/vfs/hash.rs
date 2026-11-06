// A directory's table (`children.rs`) places a name by this hash. It lives
// only in memory: nothing on a disk or across the ABI holds it, so it can
// change between builds. A fixed seed: the tables are not keyed against
// hostile names
const NAME_SEED: u64 = 0x6d69_6b75_5f76_6673;

/// hassh64, low 32 bits. Without the `aes` target feature (the kernel's target
/// has none) this is the crate's portable integer code: no SIMD, no FPU
#[inline]
pub fn name_hash(name: &str) -> u32 {
    hassh64::hassh64(name.as_bytes(), NAME_SEED) as u32
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
