// Little-endian reads and writes into a byte buffer
//
// Every structure this driver exchanges with the GPU (firmware containers, RPC
// payloads, RM control parameters, the boot-args region) is a C struct laid out
// little-endian.
//
// The three forms differ in what happens when the buffer is too short:
//
//   `u32_at`   panics, for a buffer whose length was already checked
//   `u32_or0`  returns 0, for a reply that may legitimately be short
//   `try_u32`  returns None, for a blob off the disk, where short is an error

/// Read a little-endian u32 at `off`. Panics if the buffer is shorter than
/// `off + 4`
#[inline]
pub fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

/// Read a little-endian u64 at `off`. Panics like `u32_at`
#[inline]
pub fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

/// Read a little-endian u16 at `off`. Panics like `u32_at`
#[inline]
pub fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().unwrap())
}

/// Read a little-endian u32 at `off`, or 0 if the buffer stops short.
///
/// A reply with fewer bytes than the newest version of the struct reads as zero
/// in the fields it lacks
#[inline]
pub fn u32_or0(b: &[u8], off: usize) -> u32 {
    if b.len() < off + 4 { return 0; }
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

/// Read a little-endian u32 at `off`, or `None` if the buffer stops short.
///
/// A truncated firmware container is an error to report
#[inline]
pub fn try_u32(b: &[u8], off: usize) -> Option<u32> {
    if off.checked_add(4)? > b.len() { return None; }
    Some(u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]))
}

/// Write a little-endian u32 at `off`. Panics if the buffer is too short
#[inline]
pub fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Write a little-endian u64 at `off`. Panics if the buffer is too short
#[inline]
pub fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
