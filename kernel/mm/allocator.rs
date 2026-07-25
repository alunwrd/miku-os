use linked_list_allocator::LockedHeap;

// 128 MiB. The GSP-RM firmware image (gsp-570.144.bin) is ~28.5 MiB and
// fwload::read_path pulls a firmware file into a single contiguous Vec; with
// the block buffer cache also holding the file's blocks during the read, a
// 32 MiB heap could not fit it (the read failed with NoSpace and the GPU saw
// "firmware unavailable"). The heap lives in .bss at physical ~2 MiB and the
// boot page tables map the whole low 1 GiB with a huge page (see boot_entry.rs),
// so growing it stays comfortably inside mapped RAM.
pub const HEAP_SIZE: usize = 128 * 1024 * 1024;

#[repr(align(4096))]
struct HeapMemory([u8; HEAP_SIZE]);

static mut HEAP_MEMORY: HeapMemory = HeapMemory([0; HEAP_SIZE]);

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    unsafe {
        let heap_start = HEAP_MEMORY.0.as_mut_ptr();
        ALLOCATOR.lock().init(heap_start, HEAP_SIZE);
    }
    crate::serial_println!("[heap] {} KB initialized", HEAP_SIZE / 1024);
}

pub fn used() -> usize {
    ALLOCATOR.lock().used()
}

pub fn free() -> usize {
    ALLOCATOR.lock().free()
}
