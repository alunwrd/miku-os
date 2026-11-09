//! text_miku: string builders

#[repr(C)]
pub struct MikuStr {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
}
