// checksum.rs - lightweight checksum algorithms
// Adler-32: fast, used in zlib
// Fletcher-16: simple 16-bit checksum
// XOR checksum: trivial parity check
// Internet checksum (RFC 1071): for IP/TCP/UDP header
// These are NOT cryptographic - use for data integrity checks only

// Adler-32
// Two running sums modulo 65521 (largest prime < 2^16).
// s1 = 1 + sum of all bytes
// s2 = sum of all s1 values
// result = (s2 << 16) | s1

const ADLER_MOD: u32 = 65521;

#[no_mangle]
pub extern "C" fn miku_adler32(data: *const u8, len: usize) -> u32 {
    miku_adler32_update(1, data, len)
}

#[no_mangle]
pub extern "C" fn miku_adler32_update(prev: u32, data: *const u8, len: usize) -> u32 {
    if data.is_null() || len == 0 {
        return prev;
    }
    let mut s1 = prev & 0xFFFF;
    let mut s2 = (prev >> 16) & 0xFFFF;

    // process in chunks of 5552 to avoid overflow
    let mut remaining = len;
    let mut offset = 0usize;

    while remaining > 0 {
        let chunk = if remaining > 5552 { 5552 } else { remaining };
        unsafe {
            for i in 0..chunk {
                s1 += *data.add(offset + i) as u32;
                s2 += s1;
            }
        }
        s1 %= ADLER_MOD;
        s2 %= ADLER_MOD;
        offset += chunk;
        remaining -= chunk;
    }

    (s2 << 16) | s1
}

// Fletcher-16
// Two 8-bit sums, result is 16-bit

#[no_mangle]
pub extern "C" fn miku_fletcher16(data: *const u8, len: usize) -> u16 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let mut s1: u16 = 0;
    let mut s2: u16 = 0;
    unsafe {
        for i in 0..len {
            s1 = (s1 + *data.add(i) as u16) % 255;
            s2 = (s2 + s1) % 255;
        }
    }
    (s2 << 8) | s1
}

// Fletcher-32
// Two 16-bit sums, result is 32-bit
// Processes input as 16-bit words (little-endian)

#[no_mangle]
pub extern "C" fn miku_fletcher32(data: *const u8, len: usize) -> u32 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let mut s1: u32 = 0;
    let mut s2: u32 = 0;
    let words = len / 2;
    unsafe {
        for i in 0..words {
            let lo = *data.add(i * 2) as u32;
            let hi = *data.add(i * 2 + 1) as u32;
            let word = (hi << 8) | lo;
            s1 = (s1 + word) % 65535;
            s2 = (s2 + s1) % 65535;
        }
        // handle trailing byte
        if len % 2 != 0 {
            let word = *data.add(len - 1) as u32;
            s1 = (s1 + word) % 65535;
            s2 = (s2 + s1) % 65535;
        }
    }
    (s2 << 16) | s1
}

// XOR checksum
// Simple byte-wise XOR, Good for quick parity checks +-

#[no_mangle]
pub extern "C" fn miku_xor_checksum(data: *const u8, len: usize) -> u8 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let mut xor: u8 = 0;
    unsafe {
        for i in 0..len {
            xor ^= *data.add(i);
        }
    }
    xor
}

// Internet Checksum (RFC 1071)
// Used in IP, TCP, UDP headers, 16-bit one's complement sum

#[no_mangle]
pub extern "C" fn miku_inet_checksum(data: *const u8, len: usize) -> u16 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let mut sum: u32 = 0;
    let words = len / 2;
    unsafe {
        for i in 0..words {
            let hi = *data.add(i * 2) as u32;
            let lo = *data.add(i * 2 + 1) as u32;
            sum += (hi << 8) | lo;
        }
        if len % 2 != 0 {
            sum += (*data.add(len - 1) as u32) << 8;
        }
    }
    // fold 32-bit sum to 16-bit
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

// Sum checksum
// Simple byte sum modulo 256, Used in simple serial protocols

#[no_mangle]
pub extern "C" fn miku_sum8(data: *const u8, len: usize) -> u8 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let mut s: u8 = 0;
    unsafe {
        for i in 0..len {
            s = s.wrapping_add(*data.add(i));
        }
    }
    s
}

// BSD checksum (16-bit rotating)
#[no_mangle]
pub extern "C" fn miku_bsd_checksum(data: *const u8, len: usize) -> u16 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let mut ck: u16 = 0;
    unsafe {
        for i in 0..len {
            ck = (ck >> 1) + ((ck & 1) << 15);
            ck = ck.wrapping_add(*data.add(i) as u16);
        }
    }
    ck
}

// CRC-16 (CRC-CCITT, used in Modbus, HDLC, X.25)
// Polynomial: 0xA001 (reversed 0x8005)

static CRC16_TABLE: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut i = 0u16;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

#[no_mangle]
pub extern "C" fn miku_crc16(data: *const u8, len: usize) -> u16 {
    miku_crc16_update(0xFFFF, data, len)
}

#[no_mangle]
pub extern "C" fn miku_crc16_update(prev: u16, data: *const u8, len: usize) -> u16 {
    if data.is_null() || len == 0 {
        return prev;
    }
    let mut crc = prev;
    unsafe {
        for i in 0..len {
            let byte = *data.add(i);
            let idx = ((crc as u8) ^ byte) as usize;
            crc = (crc >> 8) ^ CRC16_TABLE[idx];
        }
    }
    crc
}

// Luhn check digit algorithm (credit cards, IMEI, etc.)
// Returns true if digit string passes Luhn check
#[no_mangle]
pub extern "C" fn miku_luhn_check(data: *const u8, len: usize) -> bool {
    if data.is_null() || len == 0 {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    unsafe {
        let mut i = len;
        while i > 0 {
            i -= 1;
            let c = *data.add(i);
            if c < b'0' || c > b'9' { return false; }
            let mut d = (c - b'0') as u32;
            if double {
                d *= 2;
                if d > 9 { d -= 9; }
            }
            sum += d;
            double = !double;
        }
    }
    sum % 10 == 0
}

// Compute Luhn check digit for a digit string
// Returns the digit (0-9) that would make the string pass Luhn
#[no_mangle]
pub extern "C" fn miku_luhn_digit(data: *const u8, len: usize) -> u8 {
    if data.is_null() || len == 0 {
        return 0;
    }
    let mut sum = 0u32;
    let mut double = true; // starts true because check digit is appended
    unsafe {
        let mut i = len;
        while i > 0 {
            i -= 1;
            let c = *data.add(i);
            if c < b'0' || c > b'9' { return 0; }
            let mut d = (c - b'0') as u32;
            if double {
                d *= 2;
                if d > 9 { d -= 9; }
            }
            sum += d;
            double = !double;
        }
    }
    ((10 - (sum % 10)) % 10) as u8
}

// Parity bit (even parity)
// Returns 0 or 1 such that total set bits (data + parity) is even
#[no_mangle]
pub extern "C" fn miku_parity8(byte: u8) -> u8 {
    let mut b = byte;
    b ^= b >> 4;
    b ^= b >> 2;
    b ^= b >> 1;
    b & 1
}

// Parity over buffer (even parity bit)
#[no_mangle]
pub extern "C" fn miku_parity(data: *const u8, len: usize) -> u8 {
    if data.is_null() || len == 0 { return 0; }
    let mut p: u8 = 0;
    unsafe {
        for i in 0..len {
            p ^= *data.add(i);
        }
    }
    miku_parity8(p)
}

// SYSV checksum (16-bit, used by sum command)
#[no_mangle]
pub extern "C" fn miku_sysv_checksum(data: *const u8, len: usize) -> u16 {
    if data.is_null() || len == 0 { return 0; }
    let mut s: u32 = 0;
    unsafe {
        for i in 0..len {
            s += *data.add(i) as u32;
        }
    }
    // fold to 16 bits
    let r = (s & 0xFFFF) + (s >> 16);
    ((r & 0xFFFF) + (r >> 16)) as u16
}

// Combine two CRC32 values (for parallel computation)
// crc32_combine(crc1, crc2, len2) = CRC of data1 ++ data2
// Uses matrix exponentiation on GF(2) - O(log(len2))
#[no_mangle]
pub extern "C" fn miku_crc32_combine(crc1: u32, crc2: u32, len2: usize) -> u32 {
    if len2 == 0 { return crc1; }

    // GF(2) matrix multiply: result = mat * vec
    fn gf2_matrix_times(mat: &[u32; 32], vec: u32) -> u32 {
        let mut result = 0u32;
        let mut v = vec;
        let mut i = 0;
        while v != 0 && i < 32 {
            if v & 1 != 0 {
                result ^= mat[i];
            }
            v >>= 1;
            i += 1;
        }
        result
    }

    fn gf2_matrix_square(square: &mut [u32; 32], mat: &[u32; 32]) {
        for n in 0..32 {
            square[n] = gf2_matrix_times(mat, mat[n]);
        }
    }

    // build even/odd power-of-two zeros operator
    let mut even = [0u32; 32];
    let mut odd = [0u32; 32];

    // odd = polynomial representation of x^1 mod p(x)
    odd[0] = 0xEDB88320; // polynomial
    let mut row: u32 = 1;
    for i in 1..32 {
        odd[i] = row;
        row <<= 1;
    }

    gf2_matrix_square(&mut even, &odd);
    gf2_matrix_square(&mut odd, &even);

    let mut c1 = crc1;
    let mut n = len2;

    loop {
        // apply zeros operator for this bit of len2
        gf2_matrix_square(&mut even, &odd);
        if n & 1 != 0 {
            c1 = gf2_matrix_times(&even, c1);
        }
        n >>= 1;
        if n == 0 { break; }

        gf2_matrix_square(&mut odd, &even);
        if n & 1 != 0 {
            c1 = gf2_matrix_times(&odd, c1);
        }
        n >>= 1;
        if n == 0 { break; }
    }

    c1 ^ crc2
}
