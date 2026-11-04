// Memory management: the heap, device memory (dma), physical (pmm) and virtual
// (vmm) memory, mmap, and swap (backing + bitmap)
pub mod dma;
pub mod heap;
pub mod kstack;
pub mod mmap;
pub mod pmm;
pub mod selftest;
pub mod swap;
pub mod swap_map;
pub mod shrink;
pub mod tlb;
pub mod vmap;
pub mod vmm;
