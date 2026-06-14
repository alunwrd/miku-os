#![no_std]
#![no_main]

// End-to-end test for file-backed mmap (SYS_MMAP_FILE / SYS_MSYNC).
// Creates a file, maps it MAP_SHARED, reads its contents through the
// pointer (lazy page-fault fill), writes through the pointer, msyncs,
// and re-reads the file from disk to confirm the write reached it.

#[path = "miku.rs"]
mod miku;
use miku::*;

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const MAP_SHARED: u64 = 1;

const PATH: &str = "/mmaptest.dat\0";
const ORIGINAL: &[u8] = b"ORIGINAL-FILE-CONTENT-1234567890";
const PATCH: &[u8] = b"PATCHED!";

fn fail(msg: &str) -> ! {
    print("  [FAIL] ");
    println(msg);
    exit(1);
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    println("file-backed mmap test");

    // 1) Create the file and write the original content
    let fd = unsafe { miku_create(PATH.as_ptr(), 0o644) };
    if fd < 0 { fail("create"); }
    let n = unsafe { miku_write_fd(fd, ORIGINAL.as_ptr(), ORIGINAL.len()) };
    if n != ORIGINAL.len() as i64 { fail("write original"); }
    unsafe { miku_close(fd); }

    // 2) Open RW and map it MAP_SHARED
    let fd = unsafe { miku_open_rw(PATH.as_ptr()) };
    if fd < 0 { fail("open rw"); }
    let ptr = unsafe {
        miku_mmap_file(0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0)
    };
    if ptr.is_null() { fail("mmap_file"); }
    unsafe { miku_close(fd); } // mapping must survive the fd closing

    // 3) Read through the pointer - this faults the page in from the file
    let mapped = unsafe { core::slice::from_raw_parts(ptr, ORIGINAL.len()) };
    if mapped != ORIGINAL {
        fail("read-through mismatch (lazy fill broken)");
    }
    println("  [ok] read-through: file contents visible via pointer");

    // 4) Write through the pointer
    unsafe {
        core::ptr::copy_nonoverlapping(PATCH.as_ptr(), ptr, PATCH.len());
    }

    // 5) msync + munmap to flush the dirty page back to the file
    if unsafe { miku_msync(ptr, 4096) } != 0 { fail("msync"); }
    if unsafe { miku_munmap(ptr, 4096) } != 0 { fail("munmap"); }

    // 6) Re-read the file from disk and confirm the patch landed
    let fd = unsafe { miku_open_rw(PATH.as_ptr()) };
    if fd < 0 { fail("reopen"); }
    let mut buf = [0u8; 64];
    let got = unsafe { miku_read_fd(fd, buf.as_mut_ptr(), buf.len()) };
    unsafe { miku_close(fd); }
    if got < PATCH.len() as i64 { fail("reread short"); }
    if &buf[..PATCH.len()] != PATCH {
        fail("writeback mismatch (msync did not reach file)");
    }
    println("  [ok] writeback: pointer write reached the file");

    println("file-backed mmap test passed");
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    println("panic!");
    exit(1);
}
