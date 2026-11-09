// CRC32c and CRC16 arithmetic
//
// `crc32c`: the standard Castagnoli CRC-32C, inverted at both ends, used by
// published test vectors.
//
// `crc32c_raw`: accumulation with no inversion: seed in, state out. Linux's
// `ext4_chksum`, used by all ext4 metadata checksums.
//
// Both chain, but for one seed they give different values, and using the wrong
// one produces checksums only this code accepts.

/// The reflected CRC-32C polynomial
const POLY: u32 = 0x82F6_3B78;

/// Slicing-by-8 tables. The first is the byte table; each later one shifts a
/// further byte through the polynomial
const fn generate_tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        t[0][i] = crc;
        i += 1;
    }
    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            let prev = t[k - 1][i];
            t[k][i] = (prev >> 8) ^ t[0][(prev & 0xFF) as usize];
            i += 1;
        }
        k += 1;
    }
    t
}

static TABLES: [[u32; 256]; 8] = generate_tables();

/// CRC32c over `data` from `seed`, with no inversion (Linux's `ext4_chksum`).
/// The result can seed the next call. Uses SSE4.2 when available, otherwise
/// eight bytes per step
pub fn crc32c_raw(seed: u32, data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if hw::available() {
            // SAFETY: only reached when CPUID reports SSE4.2
            return unsafe { hw::crc32c_raw(seed, data) };
        }
    }
    crc32c_raw_table(seed, data)
}

/// Portable path: eight bytes per step
pub fn crc32c_raw_table(seed: u32, data: &[u8]) -> u32 {
    let mut crc = seed;
    let mut rest = data;

    while rest.len() >= 8 {
        let w0 = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]) ^ crc;
        let w1 = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]);
        crc = TABLES[7][(w0 & 0xFF) as usize]
            ^ TABLES[6][((w0 >> 8) & 0xFF) as usize]
            ^ TABLES[5][((w0 >> 16) & 0xFF) as usize]
            ^ TABLES[4][((w0 >> 24) & 0xFF) as usize]
            ^ TABLES[3][(w1 & 0xFF) as usize]
            ^ TABLES[2][((w1 >> 8) & 0xFF) as usize]
            ^ TABLES[1][((w1 >> 16) & 0xFF) as usize]
            ^ TABLES[0][((w1 >> 24) & 0xFF) as usize];
        rest = &rest[8..];
    }

    for &b in rest {
        crc = (crc >> 8) ^ TABLES[0][((crc ^ b as u32) & 0xFF) as usize];
    }
    crc
}

/// SSE4.2's CRC32C, with the same polynomial and reflection
#[cfg(target_arch = "x86_64")]
mod hw {
    use core::sync::atomic::{AtomicU8, Ordering};

    const UNKNOWN: u8 = 0;
    const YES: u8 = 1;
    const NO: u8 = 2;
    static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);

    pub fn available() -> bool {
        match STATE.load(Ordering::Relaxed) {
            YES => true,
            NO => false,
            _ => {
                // CPUID leaf 1, ECX bit 20
                let has = core::arch::x86_64::__cpuid(1).ecx & (1 << 20) != 0;
                STATE.store(if has { YES } else { NO }, Ordering::Relaxed);
                has
            }
        }
    }

    #[target_feature(enable = "sse4.2")]
    pub unsafe fn crc32c_raw(seed: u32, data: &[u8]) -> u32 {
        use core::arch::x86_64::{_mm_crc32_u32, _mm_crc32_u64, _mm_crc32_u8};
        let mut crc = seed;
        let mut rest = data;
        while rest.len() >= 8 {
            let w = u64::from_le_bytes([
                rest[0], rest[1], rest[2], rest[3], rest[4], rest[5], rest[6], rest[7],
            ]);
            crc = _mm_crc32_u64(crc as u64, w) as u32;
            rest = &rest[8..];
        }
        if rest.len() >= 4 {
            let w = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
            crc = _mm_crc32_u32(crc, w);
            rest = &rest[4..];
        }
        for &b in rest {
            crc = _mm_crc32_u8(crc, b);
        }
        crc
    }
}

/// Standard CRC-32C: pre-invert, accumulate, post-invert
pub fn crc32c(initial: u32, data: &[u8]) -> u32 {
    !crc32c_raw(!initial, data)
}

/// CRC16, for `GDT_CSUM` (uninit_bg) without `metadata_csum`
pub fn crc16(seed: u16, data: &[u8]) -> u16 {
    let mut crc = seed;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}