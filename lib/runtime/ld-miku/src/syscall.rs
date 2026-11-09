//! What the dynamic linker asks of the kernel

use miku_abi::sys::*;

pub const PROT_READ: u64 = miku_abi::mman::PROT_READ as u64;
pub const PROT_WRITE: u64 = miku_abi::mman::PROT_WRITE as u64;
pub const PROT_EXEC: u64 = miku_abi::mman::PROT_EXEC as u64;

pub fn exit(code: i64) -> ! {
    unsafe { sc1(SYS_EXIT, code as u64); }
    loop {}
}

pub fn write(fd: u64, buf: *const u8, len: usize) -> i64 {
    unsafe { sc3(SYS_WRITE, fd, buf as u64, len as u64) }
}

pub fn klog(buf: *const u8, len: usize) -> i64 {
    unsafe { sc2(SYS_KLOG, buf as u64, len as u64) }
}

pub fn read(fd: u64, buf: *mut u8, len: usize) -> i64 {
    unsafe { sc3(SYS_READ, fd, buf as u64, len as u64) }
}

pub fn mmap(addr: u64, len: usize, prot: u64) -> *mut u8 {
    let r = unsafe { sc4(SYS_MMAP, addr, len as u64, prot, 0) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

pub fn munmap(addr: *mut u8, len: usize) {
    unsafe { sc2(SYS_MUNMAP, addr as u64, len as u64); }
}

pub fn mprotect(addr: u64, len: usize, prot: u64) {
    unsafe { sc3(SYS_MPROTECT, addr, len as u64, prot); }
}

pub fn set_tls(addr: u64) {
    unsafe { sc1(SYS_SET_TLS, addr); }
}

pub fn open(path: &[u8]) -> i64 {
    unsafe { sc2(SYS_OPEN, path.as_ptr() as u64, path.len() as u64) }
}

pub fn close(fd: i64) {
    unsafe { sc1(SYS_CLOSE, fd as u64); }
}

pub fn seek(fd: i64, offset: u64) -> i64 {
    unsafe { sc2(SYS_SEEK, fd as u64, offset) }
}

pub fn fsize(fd: i64) -> i64 {
    unsafe { sc1(SYS_FSIZE, fd as u64) }
}

pub fn map_lib(name: &[u8]) -> i64 {
    unsafe { sc2(SYS_MAP_LIB, name.as_ptr() as u64, name.len() as u64) }
}
