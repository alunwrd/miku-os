// Low-level operations: popcount, leading/trailing zeros, byte swap, rotation, etc...
// popcount: count number of set bits

#[no_mangle]
pub extern "C" fn miku_popcount32(x: u32) -> u32 {
    // Hamming weight (Brian Kernighan's method for sparse, parallel for dense)
    let mut v = x;
    v = v - ((v >> 1) & 0x55555555);
    v = (v & 0x33333333) + ((v >> 2) & 0x33333333);
    v = (v + (v >> 4)) & 0x0F0F0F0F;
    (v.wrapping_mul(0x01010101)) >> 24
}

#[no_mangle]
pub extern "C" fn miku_popcount64(x: u64) -> u64 {
    let mut v = x;
    v = v - ((v >> 1) & 0x5555555555555555);
    v = (v & 0x3333333333333333) + ((v >> 2) & 0x3333333333333333);
    v = (v + (v >> 4)) & 0x0F0F0F0F0F0F0F0F;
    (v.wrapping_mul(0x0101010101010101)) >> 56
}

// clz: count leading zeros (number of zero bits from MSB)

#[no_mangle]
pub extern "C" fn miku_clz32(x: u32) -> u32 {
    if x == 0 { return 32; }
    let mut n: u32 = 0;
    let mut v = x;
    if v & 0xFFFF0000 == 0 { n += 16; v <<= 16; }
    if v & 0xFF000000 == 0 { n += 8;  v <<= 8;  }
    if v & 0xF0000000 == 0 { n += 4;  v <<= 4;  }
    if v & 0xC0000000 == 0 { n += 2;  v <<= 2;  }
    if v & 0x80000000 == 0 { n += 1;             }
    n
}

#[no_mangle]
pub extern "C" fn miku_clz64(x: u64) -> u64 {
    if x == 0 { return 64; }
    let hi = (x >> 32) as u32;
    if hi != 0 {
        miku_clz32(hi) as u64
    } else {
        32 + miku_clz32(x as u32) as u64
    }
}

// ctz: count trailing zeros (number of zero bits from LSB)

#[no_mangle]
pub extern "C" fn miku_ctz32(x: u32) -> u32 {
    if x == 0 { return 32; }
    let mut n: u32 = 0;
    let mut v = x;
    if v & 0x0000FFFF == 0 { n += 16; v >>= 16; }
    if v & 0x000000FF == 0 { n += 8;  v >>= 8;  }
    if v & 0x0000000F == 0 { n += 4;  v >>= 4;  }
    if v & 0x00000003 == 0 { n += 2;  v >>= 2;  }
    if v & 0x00000001 == 0 { n += 1;             }
    n
}

#[no_mangle]
pub extern "C" fn miku_ctz64(x: u64) -> u64 {
    if x == 0 { return 64; }
    let lo = x as u32;
    if lo != 0 {
        miku_ctz32(lo) as u64
    } else {
        32 + miku_ctz32((x >> 32) as u32) as u64
    }
}

// fls: find last set bit (1-indexed position of highest set bit, 0 if x==0)

#[no_mangle]
pub extern "C" fn miku_fls32(x: u32) -> u32 {
    if x == 0 { return 0; }
    32 - miku_clz32(x)
}

#[no_mangle]
pub extern "C" fn miku_fls64(x: u64) -> u64 {
    if x == 0 { return 0; }
    64 - miku_clz64(x)
}

// ffs: find first set bit (1-indexed position of lowest set bit, 0 if x==0)

#[no_mangle]
pub extern "C" fn miku_ffs32(x: u32) -> u32 {
    if x == 0 { return 0; }
    miku_ctz32(x) + 1
}

#[no_mangle]
pub extern "C" fn miku_ffs64(x: u64) -> u64 {
    if x == 0 { return 0; }
    miku_ctz64(x) + 1
}

// bswap: reverse byte order

#[no_mangle]
pub extern "C" fn miku_bswap16(x: u16) -> u16 {
    (x >> 8) | (x << 8)
}

#[no_mangle]
pub extern "C" fn miku_bswap32(x: u32) -> u32 {
    let b0 = (x >> 24) & 0xFF;
    let b1 = (x >> 8)  & 0xFF00;
    let b2 = (x << 8)  & 0xFF0000;
    let b3 = (x << 24) & 0xFF000000;
    b0 | b1 | b2 | b3
}

#[no_mangle]
pub extern "C" fn miku_bswap64(x: u64) -> u64 {
    let lo = miku_bswap32(x as u32) as u64;
    let hi = miku_bswap32((x >> 32) as u32) as u64;
    (lo << 32) | hi
}

// bit rotation

#[no_mangle]
pub extern "C" fn miku_rotl32(x: u32, n: u32) -> u32 {
    let n = n & 31;
    if n == 0 { return x; }
    (x << n) | (x >> (32 - n))
}

#[no_mangle]
pub extern "C" fn miku_rotr32(x: u32, n: u32) -> u32 {
    let n = n & 31;
    if n == 0 { return x; }
    (x >> n) | (x << (32 - n))
}

#[no_mangle]
pub extern "C" fn miku_rotl64(x: u64, n: u64) -> u64 {
    let n = n & 63;
    if n == 0 { return x; }
    (x << n) | (x >> (64 - n))
}

#[no_mangle]
pub extern "C" fn miku_rotr64(x: u64, n: u64) -> u64 {
    let n = n & 63;
    if n == 0 { return x; }
    (x >> n) | (x << (64 - n))
}

// power of two checks and rounding

#[no_mangle]
pub extern "C" fn miku_is_power_of_two(x: u64) -> bool {
    x != 0 && (x & (x - 1)) == 0
}

// round up to next power of 2 (returns x if already power of 2)
// Returns 0 when the next power of 2 does not fit in u64 (x > 2^63).
#[no_mangle]
pub extern "C" fn miku_next_power_of_two(x: u64) -> u64 {
    if x <= 1 { return 1; }
    if x > (1u64 << 63) { return 0; }
    let bits = 64 - miku_clz64(x - 1);
    1u64 << bits
}

// log2 (integer, floor)

#[no_mangle]
pub extern "C" fn miku_log2(x: u64) -> u64 {
    if x == 0 { return 0; }
    63 - miku_clz64(x)
}

// bit field extraction and insertion

// extract bits [start..start+len) from value
#[no_mangle]
pub extern "C" fn miku_bit_extract(val: u64, start: u32, len: u32) -> u64 {
    if len == 0 || start >= 64 { return 0; }
    let mask = if len >= 64 { !0u64 } else { (1u64 << len) - 1 };
    (val >> start) & mask
}

// insert "bits" into "val" at position [start..start+len)
#[no_mangle]
pub extern "C" fn miku_bit_insert(val: u64, bits: u64, start: u32, len: u32) -> u64 {
    if len == 0 || start >= 64 { return val; }
    let mask = if len >= 64 { !0u64 } else { (1u64 << len) - 1 };
    let cleared = val & !(mask << start);
    cleared | ((bits & mask) << start)
}

// alignment helpers

#[no_mangle]
pub extern "C" fn miku_align_up(val: u64, align: u64) -> u64 {
    if align == 0 || (align & (align - 1)) != 0 { return val; }
    (val + align - 1) & !(align - 1)
}

#[no_mangle]
pub extern "C" fn miku_align_down(val: u64, align: u64) -> u64 {
    if align == 0 || (align & (align - 1)) != 0 { return val; }
    val & !(align - 1)
}

#[no_mangle]
pub extern "C" fn miku_is_aligned(val: u64, align: u64) -> bool {
    if align == 0 || (align & (align - 1)) != 0 { return false; }
    val & (align - 1) == 0
}
