// Hash functions for general use
// Provides FNV-1a, DJB2, CRC32, and SipHash-2-4
// FNV-1a (Fowler-Noll-Vo)

#[no_mangle]
pub extern "C" fn miku_fnv1a_32(data: *const u8, len: usize) -> u32 {
    if data.is_null() || len == 0 {
        return 0x811c9dc5;
    }
    let mut h: u32 = 0x811c9dc5;
    for i in 0..len {
        h ^= unsafe { *data.add(i) } as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

#[no_mangle]
pub extern "C" fn miku_fnv1a_64(data: *const u8, len: usize) -> u64 {
    if data.is_null() || len == 0 {
        return 0xcbf29ce484222325;
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for i in 0..len {
        h ^= unsafe { *data.add(i) } as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// DJB2 //

#[no_mangle]
pub extern "C" fn miku_djb2(data: *const u8, len: usize) -> u64 {
    if data.is_null() || len == 0 {
        return 5381;
    }
    let mut h: u64 = 5381;
    for i in 0..len {
        let c = unsafe { *data.add(i) } as u64;
        h = h.wrapping_mul(33).wrapping_add(c); // h = h * 33 + c
    }
    h
}

// DJB2 for null-terminated C strings
#[no_mangle]
pub extern "C" fn miku_djb2_str(s: *const u8) -> u64 {
    if s.is_null() {
        return 5381;
    }
    let mut h: u64 = 5381;
    let mut i = 0usize;
    unsafe {
        loop {
            let c = *s.add(i);
            if c == 0 {
                break;
            }
            h = h.wrapping_mul(33).wrapping_add(c as u64);
            i += 1;
        }
    }
    h
}

// CRC32 (ISO 3309 / ITU-T V.42, polynomial 0xEDB88320) //

// precomputed table generated at compile time
const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = make_crc32_table();

#[no_mangle]
pub extern "C" fn miku_crc32(data: *const u8, len: usize) -> u32 {
    miku_crc32_update(0, data, len)
}

// incremental CRC32 - pass previous CRC to continue
#[no_mangle]
pub extern "C" fn miku_crc32_update(prev_crc: u32, data: *const u8, len: usize) -> u32 {
    if data.is_null() || len == 0 {
        return prev_crc;
    }
    let mut crc = !prev_crc;
    for i in 0..len {
        let b = unsafe { *data.add(i) };
        let index = ((crc as u8) ^ b) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    !crc
}

// SipHash-2-4
// Keyed hash function, good for hash tables (DoS-resistant).

struct SipState {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
}

impl SipState {
    fn new(k0: u64, k1: u64) -> Self {
        Self {
            v0: k0 ^ 0x736f6d6570736575,
            v1: k1 ^ 0x646f72616e646f6d,
            v2: k0 ^ 0x6c7967656e657261,
            v3: k1 ^ 0x7465646279746573,
        }
    }

    #[inline]
    fn sipround(&mut self) {
        self.v0 = self.v0.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(13);
        self.v1 ^= self.v0;
        self.v0 = self.v0.rotate_left(32);
        self.v2 = self.v2.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(16);
        self.v3 ^= self.v2;
        self.v0 = self.v0.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(21);
        self.v3 ^= self.v0;
        self.v2 = self.v2.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(17);
        self.v1 ^= self.v2;
        self.v2 = self.v2.rotate_left(32);
    }
}

fn read_u64_le(data: *const u8, offset: usize) -> u64 {
    let mut val = 0u64;
    for i in 0..8 {
        val |= (unsafe { *data.add(offset + i) } as u64) << (i * 8);
    }
    val
}

#[no_mangle]
pub extern "C" fn miku_siphash(data: *const u8, len: usize, k0: u64, k1: u64) -> u64 {
    let mut s = SipState::new(k0, k1);

    // process full 8-byte blocks
    let blocks = len / 8;
    for i in 0..blocks {
        let m = if data.is_null() {
            0
        } else {
            read_u64_le(data, i * 8)
        };
        s.v3 ^= m;
        s.sipround();
        s.sipround();
        s.v0 ^= m;
    }

    // process remaining bytes
    let mut last: u64 = (len as u64) << 56;
    let tail = blocks * 8;
    let remaining = len - tail;
    if !data.is_null() {
        for i in 0..remaining {
            last |= (unsafe { *data.add(tail + i) } as u64) << (i * 8);
        }
    }

    s.v3 ^= last;
    s.sipround();
    s.sipround();
    s.v0 ^= last;

    // finalization
    s.v2 ^= 0xFF;
    s.sipround();
    s.sipround();
    s.sipround();
    s.sipround();

    s.v0 ^ s.v1 ^ s.v2 ^ s.v3
}

// convenience: hash with default key (not cryptographic, just for hash tables uh...)

#[no_mangle]
pub extern "C" fn miku_hash_bytes(data: *const u8, len: usize) -> u64 {
    miku_fnv1a_64(data, len)
}

#[no_mangle]
pub extern "C" fn miku_hash_str(s: *const u8) -> u64 {
    miku_djb2_str(s)
}

#[no_mangle]
pub extern "C" fn miku_hash_u64(val: u64) -> u64 {
    // splitmix64 finalizer - good integer hash
    let mut x = val;
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    x
}

// hash_combine - combine two hash values (boost-style)
#[no_mangle]
pub extern "C" fn miku_hash_combine(seed: u64, value: u64) -> u64 {
    seed ^ (value
        .wrapping_add(0x9e3779b97f4a7c15)
        .wrapping_add(seed << 6)
        .wrapping_add(seed >> 2))
}

// hash_u32 - 32-bit integer hash (Wang's finalizer)
#[no_mangle]
pub extern "C" fn miku_hash_u32(val: u32) -> u32 {
    let mut x = val;
    x = x.wrapping_add(!(x << 15));
    x ^= x >> 10;
    x = x.wrapping_add(x << 3);
    x ^= x >> 6;
    x = x.wrapping_add(!(x << 11));
    x ^= x >> 16;
    x
}

// MurmurHash3 finalizer for 64-bit
#[no_mangle]
pub extern "C" fn miku_murmurhash3_fmix64(mut k: u64) -> u64 {
    k ^= k >> 33;
    k = k.wrapping_mul(0xff51afd7ed558ccd);
    k ^= k >> 33;
    k = k.wrapping_mul(0xc4ceb9fe1a85ec53);
    k ^= k >> 33;
    k
}

// MurmurHash3 for byte arrays (128-bit output, returns low 64 bits)
#[no_mangle]
pub extern "C" fn miku_murmurhash3(data: *const u8, len: usize, seed: u64) -> u64 {
    if data.is_null() || len == 0 {
        return miku_murmurhash3_fmix64(seed);
    }

    let mut h1 = seed;
    let mut h2 = seed;
    let c1: u64 = 0x87c37b91114253d5;
    let c2: u64 = 0x4cf5ad432745937f;

    // process 16-byte blocks
    let nblocks = len / 16;
    for i in 0..nblocks {
        let mut k1 = read_u64_le(data, i * 16);
        let mut k2 = read_u64_le(data, i * 16 + 8);

        k1 = k1.wrapping_mul(c1);
        k1 = k1.rotate_left(31);
        k1 = k1.wrapping_mul(c2);
        h1 ^= k1;
        h1 = h1.rotate_left(27);
        h1 = h1.wrapping_add(h2);
        h1 = h1.wrapping_mul(5).wrapping_add(0x52dce729);

        k2 = k2.wrapping_mul(c2);
        k2 = k2.rotate_left(33);
        k2 = k2.wrapping_mul(c1);
        h2 ^= k2;
        h2 = h2.rotate_left(31);
        h2 = h2.wrapping_add(h1);
        h2 = h2.wrapping_mul(5).wrapping_add(0x38495ab5);
    }

    // tail
    let tail = nblocks * 16;
    let remaining = len - tail;
    let mut k1: u64 = 0;
    let mut k2: u64 = 0;

    if remaining > 8 {
        for i in (8..remaining).rev() {
            k2 ^= (unsafe { *data.add(tail + i) } as u64) << ((i - 8) * 8);
        }
        k2 = k2.wrapping_mul(c2).rotate_left(33).wrapping_mul(c1);
        h2 ^= k2;
    }
    let r1_len = if remaining > 8 { 8 } else { remaining };
    for i in (0..r1_len).rev() {
        k1 ^= (unsafe { *data.add(tail + i) } as u64) << (i * 8);
    }
    if r1_len > 0 {
        k1 = k1.wrapping_mul(c1).rotate_left(31).wrapping_mul(c2);
        h1 ^= k1;
    }

    // finalization
    h1 ^= len as u64;
    h2 ^= len as u64;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    h1 = miku_murmurhash3_fmix64(h1);
    h2 = miku_murmurhash3_fmix64(h2);
    h1 = h1.wrapping_add(h2);
    h1
}
