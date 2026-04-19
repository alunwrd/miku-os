// Hexadecimal encoding and decoding
// Convert between raw bytes and hex strings
// Both lowercase and uppercase encoding
// Decode accepts mixed case

use crate::heap;

const HEX_LOWER: [u8; 16] = *b"0123456789abcdef";
const HEX_UPPER: [u8; 16] = *b"0123456789ABCDEF";

fn hex_digit(c: u8) -> i32 {
    match c {
        b'0'..=b'9' => (c - b'0') as i32,
        b'a'..=b'f' => (c - b'a' + 10) as i32,
        b'A'..=b'F' => (c - b'A' + 10) as i32,
        _ => -1,
    }
}

// calculate output length for hex encoding
#[no_mangle]
pub extern "C" fn miku_hex_encode_len(input_len: usize) -> usize {
    input_len * 2
}

// calculate output length for hex decoding
#[no_mangle]
pub extern "C" fn miku_hex_decode_len(input_len: usize) -> usize {
    input_len / 2
}

// encode bytes to lowercase hex into buffer
// Returns number of bytes written, or -1 on error
#[no_mangle]
pub extern "C" fn miku_hex_encode(
    input: *const u8,
    len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || out_max < len * 2 {
        return -1;
    }
    unsafe {
        for i in 0..len {
            let b = *input.add(i);
            *out.add(i * 2)     = HEX_LOWER[(b >> 4) as usize];
            *out.add(i * 2 + 1) = HEX_LOWER[(b & 0x0F) as usize];
        }
    }
    (len * 2) as i32
}

// encode bytes to uppercase hex into buffer
#[no_mangle]
pub extern "C" fn miku_hex_encode_upper(
    input: *const u8,
    len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || out_max < len * 2 {
        return -1;
    }
    unsafe {
        for i in 0..len {
            let b = *input.add(i);
            *out.add(i * 2)     = HEX_UPPER[(b >> 4) as usize];
            *out.add(i * 2 + 1) = HEX_UPPER[(b & 0x0F) as usize];
        }
    }
    (len * 2) as i32
}

// decode hex string to bytes
// Returns number of bytes written, or -1 on error
#[no_mangle]
pub extern "C" fn miku_hex_decode(
    input: *const u8,
    len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() {
        return -1;
    }
    if len % 2 != 0 {
        return -1; // odd length
    }
    let out_len = len / 2;
    if out_max < out_len {
        return -1;
    }
    unsafe {
        for i in 0..out_len {
            let hi = hex_digit(*input.add(i * 2));
            let lo = hex_digit(*input.add(i * 2 + 1));
            if hi < 0 || lo < 0 {
                return -1; // invalid character
            }
            *out.add(i) = ((hi << 4) | lo) as u8;
        }
    }
    out_len as i32
}

// encode to heap-allocated null-terminated hex string
// Caller must free the result
#[no_mangle]
pub extern "C" fn miku_hex_encode_alloc(input: *const u8, len: usize) -> *mut u8 {
    if input.is_null() || len == 0 {
        return core::ptr::null_mut();
    }
    let out_len = len * 2;
    let out = heap::miku_malloc(out_len + 1);
    if out.is_null() {
        return core::ptr::null_mut();
    }
    let r = miku_hex_encode(input, len, out, out_len);
    if r < 0 {
        heap::miku_free(out);
        return core::ptr::null_mut();
    }
    unsafe { *out.add(out_len) = 0; }
    out
}

// decode hex string to heap-allocated bytes
// Writes decoded length to *out_len, caller must free
#[no_mangle]
pub extern "C" fn miku_hex_decode_alloc(
    input: *const u8,
    len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    if input.is_null() || len == 0 || len % 2 != 0 {
        return core::ptr::null_mut();
    }
    let dec_len = len / 2;
    let out = heap::miku_malloc(dec_len);
    if out.is_null() {
        return core::ptr::null_mut();
    }
    let r = miku_hex_decode(input, len, out, dec_len);
    if r < 0 {
        heap::miku_free(out);
        return core::ptr::null_mut();
    }
    if !out_len.is_null() {
        unsafe { *out_len = dec_len; }
    }
    out
}

// encode single u64 to hex (16 chars, lowercase, no 0x prefix)
// Buffer must be at least 17 bytes (16 hex + null)
#[no_mangle]
pub extern "C" fn miku_hex_u64(val: u64, buf: *mut u8) {
    if buf.is_null() {
        return;
    }
    let mut v = val;
    unsafe {
        for i in (0..16).rev() {
            *buf.add(i) = HEX_LOWER[(v & 0xF) as usize];
            v >>= 4;
        }
        *buf.add(16) = 0;
    }
}

// encode single u32 to hex (8 chars, lowercase, no 0x prefix)
#[no_mangle]
pub extern "C" fn miku_hex_u32(val: u32, buf: *mut u8) {
    if buf.is_null() {
        return;
    }
    let mut v = val;
    unsafe {
        for i in (0..8).rev() {
            *buf.add(i) = HEX_LOWER[(v & 0xF) as usize];
            v >>= 4;
        }
        *buf.add(8) = 0;
    }
}
