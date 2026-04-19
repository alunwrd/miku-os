// uuid.rs - UUID generation and formatting
//
// Generates random UUIDs using miku_rand()
// Formats as standard 8-4-4-4-12 hex string

use crate::random;
use crate::hex;

// UUID structure (128 bits)
#[repr(C)]
pub struct MikuUuid {
    pub bytes: [u8; 16],
}

// generate UUID (random)
#[no_mangle]
pub extern "C" fn miku_uuid_gen() -> MikuUuid {
    let mut uuid = MikuUuid { bytes: [0u8; 16] };

    // fill with random bytes
    let r0 = random::miku_rand();
    let r1 = random::miku_rand();
    unsafe {
        let p = uuid.bytes.as_mut_ptr();
        core::ptr::write_unaligned(p as *mut u64, r0);
        core::ptr::write_unaligned(p.add(8) as *mut u64, r1);
    }

    // set version (4) and variant (10xx)
    uuid.bytes[6] = (uuid.bytes[6] & 0x0F) | 0x40; // version 4
    uuid.bytes[8] = (uuid.bytes[8] & 0x3F) | 0x80; // variant 1

    uuid
}

//  format UUID to string buffer
// Writes 36 bytes: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx + null
// Buffer must be at least 37 bytes
// Returns pointer to buf
#[no_mangle]
pub extern "C" fn miku_uuid_format(uuid: *const MikuUuid, buf: *mut u8) -> *mut u8 {
    if uuid.is_null() || buf.is_null() { return buf; }
    let uuid = unsafe { &*uuid };

    // hex encode groups: 4-2-2-2-6 bytes
    let groups: [(usize, usize); 5] = [
        (0, 4), (4, 2), (6, 2), (8, 2), (10, 6),
    ];

    let mut wp = 0usize;
    unsafe {
        for (gi, &(start, len)) in groups.iter().enumerate() {
            if gi > 0 {
                *buf.add(wp) = b'-';
                wp += 1;
            }
            for i in 0..len {
                let byte = uuid.bytes[start + i];
                let hi = byte >> 4;
                let lo = byte & 0x0F;
                *buf.add(wp) = hex_char(hi);
                *buf.add(wp + 1) = hex_char(lo);
                wp += 2;
            }
        }
        *buf.add(wp) = 0;
    }
    buf
}

fn hex_char(nibble: u8) -> u8 {
    if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 }
}

//  parse UUID from string
// Returns true on success, fills uuid
#[no_mangle]
pub extern "C" fn miku_uuid_parse(s: *const u8, uuid: *mut MikuUuid) -> bool {
    if s.is_null() || uuid.is_null() { return false; }

    let uuid = unsafe { &mut *uuid };
    let mut bi = 0usize; // byte index
    let mut si = 0usize; // string index

    unsafe {
        while bi < 16 && si < 36 {
            let c = *s.add(si);
            if c == b'-' { si += 1; continue; }
            if c == 0 { return false; }

            let hi = parse_hex_nibble(c);
            if hi > 15 { return false; }
            si += 1;

            let c2 = *s.add(si);
            if c2 == 0 { return false; }
            let lo = parse_hex_nibble(c2);
            if lo > 15 { return false; }
            si += 1;

            uuid.bytes[bi] = (hi << 4) | lo;
            bi += 1;
        }
    }

    bi == 16
}

fn parse_hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0xFF,
    }
}

// compare two UUID's
#[no_mangle]
pub extern "C" fn miku_uuid_eq(a: *const MikuUuid, b: *const MikuUuid) -> bool {
    if a.is_null() || b.is_null() { return false; }
    unsafe {
        crate::mem::miku_memcmp(
            (*a).bytes.as_ptr(),
            (*b).bytes.as_ptr(),
            16,
        ) == 0
    }
}

// check if UUID is nil (all zeros)
#[no_mangle]
pub extern "C" fn miku_uuid_is_nil(uuid: *const MikuUuid) -> bool {
    if uuid.is_null() { return true; }
    unsafe {
        for i in 0..16 {
            if (*uuid).bytes[i] != 0 { return false; }
        }
    }
    true
}

// nil UUID
#[no_mangle]
pub extern "C" fn miku_uuid_nil() -> MikuUuid {
    MikuUuid { bytes: [0u8; 16] }
}

// get UUID version (4 for v4)
#[no_mangle]
pub extern "C" fn miku_uuid_version(uuid: *const MikuUuid) -> u8 {
    if uuid.is_null() { return 0; }
    unsafe { ((*uuid).bytes[6] >> 4) & 0x0F }
}

// get UUID variant (should be 1 for RFC 4122)
#[no_mangle]
pub extern "C" fn miku_uuid_variant(uuid: *const MikuUuid) -> u8 {
    if uuid.is_null() { return 0; }
    unsafe {
        let v = (*uuid).bytes[8];
        if v & 0x80 == 0 { 0 }      // NCS backward compat
        else if v & 0xC0 == 0x80 { 1 } // RFC 4122
        else if v & 0xE0 == 0xC0 { 2 } // Microsoft
        else { 3 }                     // reserved
    }
}

// compare two UUID's for ordering (-1, 0, 1)
#[no_mangle]
pub extern "C" fn miku_uuid_cmp(a: *const MikuUuid, b: *const MikuUuid) -> i32 {
    if a.is_null() && b.is_null() { return 0; }
    if a.is_null() { return -1; }
    if b.is_null() { return 1; }
    crate::mem::miku_memcmp(
        unsafe { (*a).bytes.as_ptr() },
        unsafe { (*b).bytes.as_ptr() },
        16,
    )
}
