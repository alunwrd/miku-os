//! codec_miku: hashes and identifiers

#[repr(C)]
pub struct MikuSha256 {
    pub state: [u32; 8],
    pub buf: [u8; 64],
    pub buf_len: usize,
    pub total_len: u64,
}

// UUID structure (128 bits)
#[repr(C)]
pub struct MikuUuid {
    pub bytes: [u8; 16],
}
