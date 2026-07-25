// mm - memory management: heap allocator, physical (pmm) and virtual (vmm)
// memory, mmap, and swap (backing + bitmap)
pub mod allocator;
pub mod kstack;
pub mod mmap;
pub mod pmm;
pub mod swap;
pub mod swap_map;
pub mod vmm;
