//! CRC-32 (IEEE 802.3): zip, PNG, GPT

const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut c = i;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            j += 1;
        }
        table[i as usize] = c;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = make_table();

/// Continues a CRC: pass the previous result, 0 to start
pub fn update(prev: u32, data: &[u8]) -> u32 {
    let mut crc = !prev;
    for &b in data {
        crc = TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

pub fn crc32(data: &[u8]) -> u32 {
    update(0, data)
}
