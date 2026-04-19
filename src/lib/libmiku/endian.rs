// Converts between host (little-endian x86-64) and big/little endian formats

use crate::bitops;

// host to big-endian

#[no_mangle]
pub extern "C" fn miku_htobe16(x: u16) -> u16 {
    bitops::miku_bswap16(x)
}

#[no_mangle]
pub extern "C" fn miku_htobe32(x: u32) -> u32 {
    bitops::miku_bswap32(x)
}

#[no_mangle]
pub extern "C" fn miku_htobe64(x: u64) -> u64 {
    bitops::miku_bswap64(x)
}

// big-endian to host

#[no_mangle]
pub extern "C" fn miku_be16toh(x: u16) -> u16 {
    bitops::miku_bswap16(x)
}

#[no_mangle]
pub extern "C" fn miku_be32toh(x: u32) -> u32 {
    bitops::miku_bswap32(x)
}

#[no_mangle]
pub extern "C" fn miku_be64toh(x: u64) -> u64 {
    bitops::miku_bswap64(x)
}

// host to little-endian (no-op on x86-64, but explicit for clarity)

#[no_mangle]
pub extern "C" fn miku_htole16(x: u16) -> u16 {
    x
}

#[no_mangle]
pub extern "C" fn miku_htole32(x: u32) -> u32 {
    x
}

#[no_mangle]
pub extern "C" fn miku_htole64(x: u64) -> u64 {
    x
}

// little-endian to host (no-op on x86-64)

#[no_mangle]
pub extern "C" fn miku_le16toh(x: u16) -> u16 {
    x
}

#[no_mangle]
pub extern "C" fn miku_le32toh(x: u32) -> u32 {
    x
}

#[no_mangle]
pub extern "C" fn miku_le64toh(x: u64) -> u64 {
    x
}

// read/write unaligned values from byte pointers
// Useful for parsing binary protocols and file formats

#[no_mangle]
pub extern "C" fn miku_read_u16_be(ptr: *const u8) -> u16 {
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let b0 = *ptr as u16;
        let b1 = *ptr.add(1) as u16;
        (b0 << 8) | b1
    }
}

#[no_mangle]
pub extern "C" fn miku_read_u32_be(ptr: *const u8) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let b0 = *ptr as u32;
        let b1 = *ptr.add(1) as u32;
        let b2 = *ptr.add(2) as u32;
        let b3 = *ptr.add(3) as u32;
        (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    }
}

#[no_mangle]
pub extern "C" fn miku_read_u64_be(ptr: *const u8) -> u64 {
    if ptr.is_null() {
        return 0;
    }
    let hi = miku_read_u32_be(ptr) as u64;
    let lo = miku_read_u32_be(unsafe { ptr.add(4) }) as u64;
    (hi << 32) | lo
}

#[no_mangle]
pub extern "C" fn miku_read_u16_le(ptr: *const u8) -> u16 {
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let b0 = *ptr as u16;
        let b1 = *ptr.add(1) as u16;
        b0 | (b1 << 8)
    }
}

#[no_mangle]
pub extern "C" fn miku_read_u32_le(ptr: *const u8) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let b0 = *ptr as u32;
        let b1 = *ptr.add(1) as u32;
        let b2 = *ptr.add(2) as u32;
        let b3 = *ptr.add(3) as u32;
        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }
}

#[no_mangle]
pub extern "C" fn miku_read_u64_le(ptr: *const u8) -> u64 {
    if ptr.is_null() {
        return 0;
    }
    let lo = miku_read_u32_le(ptr) as u64;
    let hi = miku_read_u32_le(unsafe { ptr.add(4) }) as u64;
    lo | (hi << 32)
}

// write unaligned values to byte pointers

#[no_mangle]
pub extern "C" fn miku_write_u16_be(ptr: *mut u8, val: u16) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        *ptr = (val >> 8) as u8;
        *ptr.add(1) = val as u8;
    }
}

#[no_mangle]
pub extern "C" fn miku_write_u32_be(ptr: *mut u8, val: u32) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        *ptr = (val >> 24) as u8;
        *ptr.add(1) = (val >> 16) as u8;
        *ptr.add(2) = (val >> 8) as u8;
        *ptr.add(3) = val as u8;
    }
}

#[no_mangle]
pub extern "C" fn miku_write_u16_le(ptr: *mut u8, val: u16) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        *ptr = val as u8;
        *ptr.add(1) = (val >> 8) as u8;
    }
}

#[no_mangle]
pub extern "C" fn miku_write_u32_le(ptr: *mut u8, val: u32) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        *ptr = val as u8;
        *ptr.add(1) = (val >> 8) as u8;
        *ptr.add(2) = (val >> 16) as u8;
        *ptr.add(3) = (val >> 24) as u8;
    }
}

#[no_mangle]
pub extern "C" fn miku_write_u64_be(ptr: *mut u8, val: u64) {
    if ptr.is_null() {
        return;
    }
    miku_write_u32_be(ptr, (val >> 32) as u32);
    miku_write_u32_be(unsafe { ptr.add(4) }, val as u32);
}

#[no_mangle]
pub extern "C" fn miku_write_u64_le(ptr: *mut u8, val: u64) {
    if ptr.is_null() {
        return;
    }
    miku_write_u32_le(ptr, val as u32);
    miku_write_u32_le(unsafe { ptr.add(4) }, (val >> 32) as u32);
}
