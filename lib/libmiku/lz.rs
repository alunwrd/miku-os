///////////////////////////////////////////////////////////////////////////
//              Lightweight LZ77-style compression                       //            
//                                                                       //
// Simple byte-level compressor for small buffers                        //
// Format: literal run (0xxxxxxx length, then bytes)                     //
//         match reference (1xxxxxxx offset_hi, offset_lo, length)       //
//                                                                       //   
// Window size: 2048 bytes. Max match: 127 bytes, Min match: 3           //
// Not compatible with zlib/gzip - this is a MikuOS-specific format      //
///////////////////////////////////////////////////////////////////////////

use crate::heap;
use crate::mem;

const WINDOW_SIZE: usize = 2047;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 127;
const MAX_LIT_RUN: usize = 127;

// find best match in sliding window
unsafe fn find_match(
    data: *const u8,
    pos: usize,
    len: usize,
) -> (usize, usize) {
    // returns (offset_back, match_length)
    let start = if pos > WINDOW_SIZE { pos - WINDOW_SIZE } else { 0 };
    let max_len = if len - pos > MAX_MATCH { MAX_MATCH } else { len - pos };
    if max_len < MIN_MATCH {
        return (0, 0);
    }

    let mut best_off = 0usize;
    let mut best_len = 0usize;

    let mut s = start;
    while s < pos {
        let mut ml = 0usize;
        while ml < max_len && *data.add(s + ml) == *data.add(pos + ml) {
            ml += 1;
        }
        if ml >= MIN_MATCH && ml > best_len {
            best_len = ml;
            best_off = pos - s;
            if ml == max_len {
                break;
            }
        }
        s += 1;
    }

    (best_off, best_len)
}

// Compress data
// Returns heap-allocated compressed buffer, Writes output length to *out_len
// Caller must free, Returns null on failure
#[no_mangle]
pub extern "C" fn miku_lz_compress(
    input: *const u8,
    input_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    if input.is_null() || input_len == 0 {
        return core::ptr::null_mut();
    }

    // worst case: every byte is a literal -> ~input_len * 2
    let buf_size = input_len * 2 + 256;
    let buf = heap::miku_malloc(buf_size);
    if buf.is_null() {
        return core::ptr::null_mut();
    }

    let mut pos = 0usize;
    let mut wp = 0usize; // write position in output
    let mut lit_start = 0usize;
    let mut lit_count = 0usize;

    unsafe {
        while pos < input_len {
            let (off, mlen) = find_match(input, pos, input_len);

            if mlen >= MIN_MATCH {
                // flush pending literals
                if lit_count > 0 {
                    wp = match flush_literals(input, lit_start, lit_count, buf, wp, buf_size) {
                        Some(w) => w,
                        None => { heap::miku_free(buf); return core::ptr::null_mut(); }
                    };
                    lit_count = 0;
                }

                // write match: 1LLLLLLL offset_hi offset_lo
                // offset = 11 bits, length = 7 bits
                if wp + 3 > buf_size {
                    heap::miku_free(buf);
                    return core::ptr::null_mut();
                }
                *buf.add(wp) = 0x80 | (mlen as u8);
                *buf.add(wp + 1) = ((off >> 8) & 0x07) as u8;
                *buf.add(wp + 2) = (off & 0xFF) as u8;
                wp += 3;
                pos += mlen;
            } else {
                if lit_count == 0 {
                    lit_start = pos;
                }
                lit_count += 1;
                if lit_count >= MAX_LIT_RUN {
                    wp = match flush_literals(input, lit_start, lit_count, buf, wp, buf_size) {
                        Some(w) => w,
                        None => { heap::miku_free(buf); return core::ptr::null_mut(); }
                    };
                    lit_count = 0;
                }
                pos += 1;
            }
        }

        // flush remaining literals
        if lit_count > 0 {
            wp = match flush_literals(input, lit_start, lit_count, buf, wp, buf_size) {
                Some(w) => w,
                None => { heap::miku_free(buf); return core::ptr::null_mut(); }
            };
        }

        if !out_len.is_null() {
            *out_len = wp;
        }
    }
    buf
}

// Returns None on overflow so callers can propagate the error instead of
// silently truncating the stream (which would corrupt match offsets).
unsafe fn flush_literals(
    input: *const u8,
    start: usize,
    count: usize,
    out: *mut u8,
    wp: usize,
    max: usize,
) -> Option<usize> {
    let mut w = wp;
    let mut remaining = count;
    let mut off = start;

    while remaining > 0 {
        let chunk = if remaining > MAX_LIT_RUN { MAX_LIT_RUN } else { remaining };
        if w + 1 + chunk > max {
            return None;
        }
        *out.add(w) = chunk as u8; // high bit 0 = literal run
        w += 1;
        mem::miku_memcpy(out.add(w), input.add(off), chunk);
        w += chunk;
        off += chunk;
        remaining -= chunk;
    }
    Some(w)
}

// decompress data
// Returns heap-allocated decompressed buffer. Writes output length to *out_len
// Caller must free, Returns null on failure.
// max_output is a safety limit for output size
#[no_mangle]
pub extern "C" fn miku_lz_decompress(
    input: *const u8,
    input_len: usize,
    out_len: *mut usize,
    max_output: usize,
) -> *mut u8 {
    if input.is_null() || input_len == 0 || max_output == 0 {
        return core::ptr::null_mut();
    }

    let buf = heap::miku_malloc(max_output);
    if buf.is_null() {
        return core::ptr::null_mut();
    }

    let mut rp = 0usize; // read position
    let mut wp = 0usize; // write position

    unsafe {
        while rp < input_len {
            let tag = *input.add(rp);
            rp += 1;

            if tag & 0x80 != 0 {
                // match reference
                let mlen = (tag & 0x7F) as usize;
                if rp + 2 > input_len {
                    heap::miku_free(buf);
                    return core::ptr::null_mut();
                }
                let off_hi = *input.add(rp) as usize;
                let off_lo = *input.add(rp + 1) as usize;
                rp += 2;
                let offset = (off_hi << 8) | off_lo;

                if offset == 0 || offset > wp || wp + mlen > max_output {
                    heap::miku_free(buf);
                    return core::ptr::null_mut();
                }

                // copy byte by byte (overlapping allowed)
                let src = wp - offset;
                for i in 0..mlen {
                    *buf.add(wp + i) = *buf.add(src + i);
                }
                wp += mlen;
            } else {
                // literal run
                let count = tag as usize;
                if count == 0 {
                    break; // end marker
                }
                if rp + count > input_len || wp + count > max_output {
                    heap::miku_free(buf);
                    return core::ptr::null_mut();
                }
                mem::miku_memcpy(buf.add(wp), input.add(rp), count);
                rp += count;
                wp += count;
            }
        }

        if !out_len.is_null() {
            *out_len = wp;
        }
    }
    buf
}

// compress into caller-provided buffer
// Returns compressed size, or -1 on error
#[no_mangle]
pub extern "C" fn miku_lz_compress_buf(
    input: *const u8,
    input_len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || input_len == 0 {
        return -1;
    }

    let mut pos = 0usize;
    let mut wp = 0usize;
    let mut lit_start = 0usize;
    let mut lit_count = 0usize;

    unsafe {
        while pos < input_len {
            let (off, mlen) = find_match(input, pos, input_len);

            if mlen >= MIN_MATCH {
                if lit_count > 0 {
                    wp = match flush_literals(input, lit_start, lit_count, out, wp, out_max) {
                        Some(w) => w,
                        None => return -1,
                    };
                    lit_count = 0;
                }
                if wp + 3 > out_max {
                    return -1;
                }
                *out.add(wp) = 0x80 | (mlen as u8);
                *out.add(wp + 1) = ((off >> 8) & 0x07) as u8;
                *out.add(wp + 2) = (off & 0xFF) as u8;
                wp += 3;
                pos += mlen;
            } else {
                if lit_count == 0 {
                    lit_start = pos;
                }
                lit_count += 1;
                if lit_count >= MAX_LIT_RUN {
                    wp = match flush_literals(input, lit_start, lit_count, out, wp, out_max) {
                        Some(w) => w,
                        None => return -1,
                    };
                    lit_count = 0;
                }
                pos += 1;
            }
        }

        if lit_count > 0 {
            wp = match flush_literals(input, lit_start, lit_count, out, wp, out_max) {
                Some(w) => w,
                None => return -1,
            };
        }
    }
    wp as i32
}

// decompress into caller-provided buffer
// Returns decompressed size, or -1 on error
#[no_mangle]
pub extern "C" fn miku_lz_decompress_buf(
    input: *const u8,
    input_len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || input_len == 0 {
        return -1;
    }

    let mut rp = 0usize;
    let mut wp = 0usize;

    unsafe {
        while rp < input_len {
            let tag = *input.add(rp);
            rp += 1;

            if tag & 0x80 != 0 {
                let mlen = (tag & 0x7F) as usize;
                if rp + 2 > input_len {
                    return -1;
                }
                let off_hi = *input.add(rp) as usize;
                let off_lo = *input.add(rp + 1) as usize;
                rp += 2;
                let offset = (off_hi << 8) | off_lo;

                if offset == 0 || offset > wp || wp + mlen > out_max {
                    return -1;
                }

                let src = wp - offset;
                for i in 0..mlen {
                    *out.add(wp + i) = *out.add(src + i);
                }
                wp += mlen;
            } else {
                let count = tag as usize;
                if count == 0 {
                    break;
                }
                if rp + count > input_len || wp + count > out_max {
                    return -1;
                }
                mem::miku_memcpy(out.add(wp), input.add(rp), count);
                rp += count;
                wp += count;
            }
        }
    }
    wp as i32
}

// estimate worst-case compressed size
#[no_mangle]
pub extern "C" fn miku_lz_compress_bound(input_len: usize) -> usize {
    // worst case: all literals, each run of 127 has 1 byte header
    (input_len / MAX_LIT_RUN + 1) * (MAX_LIT_RUN + 1) + 1
}

////////////////////////////////////////////////////////////////////////////
//                 RLE (Run-Length Encoding)                              //
// Simple byte-level RLE: [count, byte] pairs                             //
// count 1..128 = literal run of 'count' bytes (each different)           //
// count 129..255 = repeat next byte (count - 128) times (2..127 repeats) //
// 0 = end marker                                                         //
//                                                                        //
// RLE compress into caller buffer. Returns output length or -1 on error. //
////////////////////////////////////////////////////////////////////////////

#[no_mangle]
pub extern "C" fn miku_rle_compress(
    input: *const u8,
    input_len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || input_len == 0 || out_max < 1 {
        return -1;
    }

    let mut rp = 0usize;
    let mut wp = 0usize;

    unsafe {
        while rp < input_len {
            // check for a run of same bytes
            let byte = *input.add(rp);
            let mut run = 1usize;
            while rp + run < input_len && run < 127 && *input.add(rp + run) == byte {
                run += 1;
            }

            if run >= 3 {
                // encode as repeat
                if wp + 2 > out_max { return -1; }
                *out.add(wp) = (run as u8) + 128;
                *out.add(wp + 1) = byte;
                wp += 2;
                rp += run;
            } else {
                // encode as literal run
                let lit_start = rp;
                let mut lit_len = 0usize;
                while rp + lit_len < input_len && lit_len < 128 {
                    // check if next position starts a run of 3+
                    let pos = rp + lit_len;
                    if pos + 2 < input_len
                        && *input.add(pos) == *input.add(pos + 1)
                        && *input.add(pos) == *input.add(pos + 2)
                    {
                        break;
                    }
                    lit_len += 1;
                }
                if lit_len == 0 { lit_len = 1; }
                if wp + 1 + lit_len > out_max { return -1; }
                *out.add(wp) = lit_len as u8;
                wp += 1;
                mem::miku_memcpy(out.add(wp), input.add(lit_start), lit_len);
                wp += lit_len;
                rp += lit_len;
            }
        }

        // end marker
        if wp >= out_max { return -1; }
        *out.add(wp) = 0;
        wp += 1;
    }

    wp as i32
}

// RLE decompress into caller buffer, returns output length or -1 on error
#[no_mangle]
pub extern "C" fn miku_rle_decompress(
    input: *const u8,
    input_len: usize,
    out: *mut u8,
    out_max: usize,
) -> i32 {
    if input.is_null() || out.is_null() || input_len == 0 {
        return -1;
    }

    let mut rp = 0usize;
    let mut wp = 0usize;

    unsafe {
        while rp < input_len {
            let tag = *input.add(rp);
            rp += 1;

            if tag == 0 {
                break; // end marker
            } else if tag > 128 {
                // repeat run
                let count = (tag - 128) as usize;
                if rp >= input_len { return -1; }
                let byte = *input.add(rp);
                rp += 1;
                if wp + count > out_max { return -1; }
                mem::miku_memset(out.add(wp), byte as i32, count);
                wp += count;
            } else {
                // literal run
                let count = tag as usize;
                if rp + count > input_len || wp + count > out_max { return -1; }
                mem::miku_memcpy(out.add(wp), input.add(rp), count);
                rp += count;
                wp += count;
            }
        }
    }

    wp as i32
}

// RLE worst-case output size
#[no_mangle]
pub extern "C" fn miku_rle_compress_bound(input_len: usize) -> usize {
    (input_len * 2) + (input_len / 128) + 2
}

/////////////////////////////////////////////////////////////////////////////////////////////
//                                Delta encoding                                           //
// Stores differences between consecutive bytes - good before LZ/RLE for smooth data       //
//        																				   //
//            Delta encode: out[i] = input[i] - input[i-1] (out[0] = input[0])	     	   //
/////////////////////////////////////////////////////////////////////////////////////////////

#[no_mangle]
pub extern "C" fn miku_delta_encode(
    input: *const u8,
    len: usize,
    out: *mut u8,
) {
    if input.is_null() || out.is_null() || len == 0 { return; }
    unsafe {
        *out = *input;
        for i in 1..len {
            *out.add(i) = (*input.add(i)).wrapping_sub(*input.add(i - 1));
        }
    }
}

// Delta decode: reconstruct original from deltas
#[no_mangle]
pub extern "C" fn miku_delta_decode(
    input: *const u8,
    len: usize,
    out: *mut u8,
) {
    if input.is_null() || out.is_null() || len == 0 { return; }
    unsafe {
        *out = *input;
        for i in 1..len {
            *out.add(i) = (*out.add(i - 1)).wrapping_add(*input.add(i));
        }
    }
}
