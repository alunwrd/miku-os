// UTF-8 encoding and decoding
// Validates, encodes, and iterates over UTF-8 byte sequences

const REPLACEMENT_CHAR: u32 = 0xFFFD;

// encode a single codepoint to UTF-8
// Writes to 'out', returns number of bytes written (1-4), or 0 on error

#[no_mangle]
pub extern "C" fn miku_utf8_encode(codepoint: u32, out: *mut u8) -> usize {
    if out.is_null() { return 0; }

    // reject surrogates and values above U+10FFFF
    if codepoint > 0x10FFFF || (codepoint >= 0xD800 && codepoint <= 0xDFFF) {
        return 0;
    }

    unsafe {
        if codepoint <= 0x7F {
            *out = codepoint as u8;
            1
        } else if codepoint <= 0x7FF {
            *out       = (0xC0 | (codepoint >> 6)) as u8;
            *out.add(1) = (0x80 | (codepoint & 0x3F)) as u8;
            2
        } else if codepoint <= 0xFFFF {
            *out       = (0xE0 | (codepoint >> 12)) as u8;
            *out.add(1) = (0x80 | ((codepoint >> 6) & 0x3F)) as u8;
            *out.add(2) = (0x80 | (codepoint & 0x3F)) as u8;
            3
        } else {
            *out       = (0xF0 | (codepoint >> 18)) as u8;
            *out.add(1) = (0x80 | ((codepoint >> 12) & 0x3F)) as u8;
            *out.add(2) = (0x80 | ((codepoint >> 6) & 0x3F)) as u8;
            *out.add(3) = (0x80 | (codepoint & 0x3F)) as u8;
            4
        }
    }
}

//   decode one codepoint from UTF-8 bytes
// Returns the codepoint and sets *bytes_consumed.
// On invalid sequence, returns U+FFFD and consumes 1 byte.

#[no_mangle]
pub extern "C" fn miku_utf8_decode(
    data: *const u8,
    len: usize,
    bytes_consumed: *mut usize,
) -> u32 {
    if data.is_null() || len == 0 {
        if !bytes_consumed.is_null() { unsafe { *bytes_consumed = 0; } }
        return REPLACEMENT_CHAR;
    }

    let b0 = unsafe { *data } as u32;

    // helper to check continuation byte
    let cont = |i: usize| -> Option<u32> {
        if i >= len { return None; }
        let b = unsafe { *data.add(i) } as u32;
        if b & 0xC0 != 0x80 { return None; }
        Some(b & 0x3F)
    };

    let (cp, consumed) = if b0 <= 0x7F {
        (b0, 1)
    } else if b0 & 0xE0 == 0xC0 {
        if let Some(c1) = cont(1) {
            let cp = ((b0 & 0x1F) << 6) | c1;
            // reject overlong
            if cp < 0x80 { (REPLACEMENT_CHAR, 1) } else { (cp, 2) }
        } else {
            (REPLACEMENT_CHAR, 1)
        }
    } else if b0 & 0xF0 == 0xE0 {
        if let (Some(c1), Some(c2)) = (cont(1), cont(2)) {
            let cp = ((b0 & 0x0F) << 12) | (c1 << 6) | c2;
            // reject overlong and surrogates
            if cp < 0x800 || (cp >= 0xD800 && cp <= 0xDFFF) {
                (REPLACEMENT_CHAR, 1)
            } else {
                (cp, 3)
            }
        } else {
            (REPLACEMENT_CHAR, 1)
        }
    } else if b0 & 0xF8 == 0xF0 {
        if let (Some(c1), Some(c2), Some(c3)) = (cont(1), cont(2), cont(3)) {
            let cp = ((b0 & 0x07) << 18) | (c1 << 12) | (c2 << 6) | c3;
            // reject overlong and out-of-range
            if cp < 0x10000 || cp > 0x10FFFF {
                (REPLACEMENT_CHAR, 1)
            } else {
                (cp, 4)
            }
        } else {
            (REPLACEMENT_CHAR, 1)
        }
    } else {
        (REPLACEMENT_CHAR, 1)
    };

    if !bytes_consumed.is_null() {
        unsafe { *bytes_consumed = consumed; }
    }
    cp
}

// count the number of codepoints in a UTF-8 string

#[no_mangle]
pub extern "C" fn miku_utf8_len(s: *const u8, byte_len: usize) -> usize {
    if s.is_null() { return 0; }
    let mut count = 0usize;
    let mut i = 0usize;

    while i < byte_len {
        let b = unsafe { *s.add(i) };
        if b == 0 { break; }

        // determine sequence length from lead byte
        let seq_len = if b & 0x80 == 0 { 1 }
            else if b & 0xE0 == 0xC0 { 2 }
            else if b & 0xF0 == 0xE0 { 3 }
            else if b & 0xF8 == 0xF0 { 4 }
            else { 1 }; // invalid lead byte - skip one byte

        count += 1;
        i += seq_len;
    }

    count
}

// count codepoints in a null-terminated UTF-8 string

#[no_mangle]
pub extern "C" fn miku_utf8_strlen(s: *const u8) -> usize {
    if s.is_null() { return 0; }
    let byte_len = crate::string::miku_strlen(s);
    miku_utf8_len(s, byte_len)
}

//   validate UTF-8 sequence
// Returns true if the entire string is valid UTF-8.

#[no_mangle]
pub extern "C" fn miku_utf8_valid(s: *const u8, len: usize) -> bool {
    if s.is_null() { return len == 0; }
    let mut i = 0usize;

    while i < len {
        let b0 = unsafe { *s.add(i) };
        if b0 == 0 { return true; }

        if b0 <= 0x7F {
            i += 1;
        } else if b0 & 0xE0 == 0xC0 {
            if i + 1 >= len { return false; }
            let b1 = unsafe { *s.add(i + 1) };
            if b1 & 0xC0 != 0x80 { return false; }
            let cp = ((b0 as u32 & 0x1F) << 6) | (b1 as u32 & 0x3F);
            if cp < 0x80 { return false; } // overlong
            i += 2;
        } else if b0 & 0xF0 == 0xE0 {
            if i + 2 >= len { return false; }
            let b1 = unsafe { *s.add(i + 1) };
            let b2 = unsafe { *s.add(i + 2) };
            if b1 & 0xC0 != 0x80 || b2 & 0xC0 != 0x80 { return false; }
            let cp = ((b0 as u32 & 0x0F) << 12) | ((b1 as u32 & 0x3F) << 6) | (b2 as u32 & 0x3F);
            if cp < 0x800 { return false; }
            if cp >= 0xD800 && cp <= 0xDFFF { return false; } // surrogates
            i += 3;
        } else if b0 & 0xF8 == 0xF0 {
            if i + 3 >= len { return false; }
            let b1 = unsafe { *s.add(i + 1) };
            let b2 = unsafe { *s.add(i + 2) };
            let b3 = unsafe { *s.add(i + 3) };
            if b1 & 0xC0 != 0x80 || b2 & 0xC0 != 0x80 || b3 & 0xC0 != 0x80 { return false; }
            let cp = ((b0 as u32 & 0x07) << 18) | ((b1 as u32 & 0x3F) << 12)
                | ((b2 as u32 & 0x3F) << 6) | (b3 as u32 & 0x3F);
            if cp < 0x10000 || cp > 0x10FFFF { return false; }
            i += 4;
        } else {
            return false; // invalid lead byte
        }
    }

    true
}

//   get byte offset of the n-th codepoint (0-indexed)
// Returns byte_len if n >= number of codepoints

#[no_mangle]
pub extern "C" fn miku_utf8_offset(s: *const u8, byte_len: usize, n: usize) -> usize {
    if s.is_null() { return 0; }
    let mut count = 0usize;
    let mut i = 0usize;

    while i < byte_len && count < n {
        let b = unsafe { *s.add(i) };
        if b == 0 { break; }

        let seq_len = if b & 0x80 == 0 { 1 }
            else if b & 0xE0 == 0xC0 { 2 }
            else if b & 0xF0 == 0xE0 { 3 }
            else if b & 0xF8 == 0xF0 { 4 }
            else { 1 };

        i += seq_len;
        count += 1;
    }

    i
}

// check if byte position is at a codepoint boundary

#[no_mangle]
pub extern "C" fn miku_utf8_is_boundary(s: *const u8, len: usize, pos: usize) -> bool {
    if s.is_null() || pos > len { return false; }
    if pos == 0 || pos == len { return true; }
    let b = unsafe { *s.add(pos) };
    // continuation bytes have the pattern 10xxxxxx
    (b & 0xC0) != 0x80
}
