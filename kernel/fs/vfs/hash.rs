#[inline]
pub fn fnv32(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

#[inline]
pub fn name_hash(name: &str) -> u32 {
    fnv32(name.as_bytes())
}

#[inline]
pub fn dentry_hash(parent: u16, name: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    let pb = parent.to_le_bytes();
    for &b in &pb {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    for &b in name.as_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
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
