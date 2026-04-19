// Base64 encoding and decoding (RFC 4648)
// Encodes binary data to ASCII and decodes back

use crate::heap;

const ENCODE_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD: u8 = b'=';

// decode lookup: maps ASCII byte to 6-bit value, 0xFF = invalid
#[inline(always)]
fn decode_byte(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0xFF,
    }
}

// calculate encoded output size (including null terminator)

#[no_mangle]
pub extern "C" fn miku_base64_encode_len(input_len: usize) -> usize {
    if input_len == 0 { return 1; }
    ((input_len + 2) / 3) * 4 + 1
}

// calculate maximum decoded output size

#[no_mangle]
pub extern "C" fn miku_base64_decode_len(input_len: usize) -> usize {
    if input_len == 0 { return 0; }
    (input_len / 4) * 3
}

//  encode: writes base64 to "out", returns number of bytes written (excluding null)
//  "out" must have space for miku_base64_encode_len(len) bytes

#[no_mangle]
pub extern "C" fn miku_base64_encode(
    input: *const u8,
    len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || out_max == 0 { return -1; }
    if len == 0 {
        unsafe { *out = 0; }
        return 0;
    }

    let needed = miku_base64_encode_len(len);
    if out_max < needed { return -1; }

    let mut oi = 0usize;
    let mut i = 0usize;

    unsafe {
        // process full 3-byte groups
        while i + 2 < len {
            let b0 = *input.add(i) as u32;
            let b1 = *input.add(i + 1) as u32;
            let b2 = *input.add(i + 2) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;

            *out.add(oi)     = ENCODE_TABLE[((triple >> 18) & 0x3F) as usize];
            *out.add(oi + 1) = ENCODE_TABLE[((triple >> 12) & 0x3F) as usize];
            *out.add(oi + 2) = ENCODE_TABLE[((triple >> 6)  & 0x3F) as usize];
            *out.add(oi + 3) = ENCODE_TABLE[(triple         & 0x3F) as usize];

            i += 3;
            oi += 4;
        }

        // handle remaining 1 or 2 bytes
        let remaining = len - i;
        if remaining == 1 {
            let b0 = *input.add(i) as u32;
            *out.add(oi)     = ENCODE_TABLE[((b0 >> 2) & 0x3F) as usize];
            *out.add(oi + 1) = ENCODE_TABLE[((b0 << 4) & 0x3F) as usize];
            *out.add(oi + 2) = PAD;
            *out.add(oi + 3) = PAD;
            oi += 4;
        } else if remaining == 2 {
            let b0 = *input.add(i) as u32;
            let b1 = *input.add(i + 1) as u32;
            *out.add(oi)     = ENCODE_TABLE[((b0 >> 2)              & 0x3F) as usize];
            *out.add(oi + 1) = ENCODE_TABLE[(((b0 << 4) | (b1 >> 4)) & 0x3F) as usize];
            *out.add(oi + 2) = ENCODE_TABLE[((b1 << 2)              & 0x3F) as usize];
            *out.add(oi + 3) = PAD;
            oi += 4;
        }

        *out.add(oi) = 0;
    }

    oi as i32
}

// decode: writes binary data to "out", returns number of bytes written
// Returns -1 on invalid input

#[no_mangle]
pub extern "C" fn miku_base64_decode(
    input: *const u8,
    len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || out_max == 0 { return -1; }
    if len == 0 { return 0; }
    if len % 4 != 0 { return -1; }

    let mut oi = 0usize;
    let mut i = 0usize;

    unsafe {
        while i < len {
            let c0 = *input.add(i);
            let c1 = *input.add(i + 1);
            let c2 = *input.add(i + 2);
            let c3 = *input.add(i + 3);

            let v0 = decode_byte(c0);
            let v1 = decode_byte(c1);
            if v0 == 0xFF || v1 == 0xFF { return -1; }

            let triple = (v0 as u32) << 18 | (v1 as u32) << 12;

            // first byte always present
            if oi >= out_max { return -1; }
            *out.add(oi) = (triple >> 16) as u8;
            oi += 1;

            if c2 != PAD {
                let v2 = decode_byte(c2);
                if v2 == 0xFF { return -1; }
                let triple = triple | (v2 as u32) << 6;
                if oi >= out_max { return -1; }
                *out.add(oi) = (triple >> 8) as u8;
                oi += 1;

                if c3 != PAD {
                    let v3 = decode_byte(c3);
                    if v3 == 0xFF { return -1; }
                    let triple = triple | v3 as u32;
                    if oi >= out_max { return -1; }
                    *out.add(oi) = triple as u8;
                    oi += 1;
                }
            } else {
                // Strict parsing: if c2 is PAD, c3 MUST be PAD
                if c3 != PAD { return -1; }
            }

            i += 4;
        }
    }

    oi as i32
}

// convenience: encode to heap-allocated string

#[no_mangle]
pub extern "C" fn miku_base64_encode_alloc(input: *const u8, len: usize) -> *mut u8 {
    if input.is_null() || len == 0 { return core::ptr::null_mut(); }
    let out_len = miku_base64_encode_len(len);
    let buf = heap::miku_malloc(out_len);
    if buf.is_null() { return core::ptr::null_mut(); }
    let written = miku_base64_encode(input, len, buf, out_len);
    if written < 0 {
        heap::miku_free(buf);
        return core::ptr::null_mut();
    }
    buf
}

// validate base64 string without decoding
#[no_mangle]
pub extern "C" fn miku_base64_is_valid(input: *const u8, len: usize) -> bool {
    if input.is_null() || len == 0 { return true; }
    if len % 4 != 0 { return false; }
    unsafe {
        let mut i = 0usize;
        while i < len {
            let c = *input.add(i);
            if c == PAD {
                // padding only allowed in last 2 positions
                if i < len - 2 { return false; }
                // if it's the second to last character, the last character must also be PAD
                if i == len - 2 && *input.add(len - 1) != PAD { return false; }
            } else if decode_byte(c) == 0xFF {
                return false;
            }
            i += 1;
        }
    }
    true
}

// convenience: decode to heap-allocated buffer, sets *out_len

#[no_mangle]
pub extern "C" fn miku_base64_decode_alloc(
    input: *const u8,
    len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    if input.is_null() || len == 0 { return core::ptr::null_mut(); }
    let max = miku_base64_decode_len(len);
    let buf = heap::miku_malloc(max);
    if buf.is_null() { return core::ptr::null_mut(); }
    let written = miku_base64_decode(input, len, buf, max);
    if written < 0 {
        heap::miku_free(buf);
        return core::ptr::null_mut();
    }
    if !out_len.is_null() {
        unsafe { *out_len = written as usize; }
    }
    buf
}
