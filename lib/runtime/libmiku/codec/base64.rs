// Base64 encoding and decoding (RFC 4648)

use crate::heap;

use miku_base::base64::PAD;

#[inline(always)]
fn decode_byte(c: u8) -> u8 {
    miku_base::base64::value(c).unwrap_or(0xFF)
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

// Encode: writes base64 to "out", returning the bytes written (excluding null).
// "out" must have space for miku_base64_encode_len(len) bytes

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
    let (data, text) = unsafe {
        (core::slice::from_raw_parts(input, len), core::slice::from_raw_parts_mut(out, out_max))
    };
    let Some(n) = miku_base::base64::encode(data, text) else { return -1 };
    text[n] = 0;
    n as i32
}

// Decode: writes binary data to "out", returning the bytes written, or -1 on
// invalid input

#[no_mangle]
pub extern "C" fn miku_base64_decode(
    input: *const u8,
    len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || out_max == 0 { return -1; }
    if len == 0 { return 0; }
    let (text, data) = unsafe {
        (core::slice::from_raw_parts(input, len), core::slice::from_raw_parts_mut(out, out_max))
    };
    match miku_base::base64::decode(text, data) {
        Some(n) => n as i32,
        None => -1,
    }
}

// Encodes to a heap-allocated string

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
                // Padding is allowed only in the last 2 positions
                if i < len - 2 { return false; }
                // In the second to last position, the last character must also
                // be PAD
                if i == len - 2 && *input.add(len - 1) != PAD { return false; }
            } else if decode_byte(c) == 0xFF {
                return false;
            }
            i += 1;
        }
    }
    true
}

// Decodes to a heap-allocated buffer, setting *out_len

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
