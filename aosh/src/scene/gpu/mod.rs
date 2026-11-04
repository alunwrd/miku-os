// GPU drawing, two ways: on the processor following GPU rules (`raster.rs`) and
// recorded for the kernel's `paint.cu` (`record.rs`). Compared pixel for pixel
// on the host through CUDA (`check.rs`)

mod raster;
pub mod record;
pub mod device;
#[cfg(test)]
pub mod check;

pub use raster::*;

/// Unique id for pictures and images, keying their video memory copies. An
/// address could be reused
pub fn serial() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
