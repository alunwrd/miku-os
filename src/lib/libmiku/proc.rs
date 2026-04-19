use crate::sys::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_exit(code: i64) -> ! {
    unsafe { sc1(SYS_EXIT, code as u64); }
    loop {}
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getpid() -> u64 {
    unsafe { sc0(SYS_GETPID) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    let r = unsafe { sc2(SYS_GETCWD, buf as u64, size as u64) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_brk(addr: u64) -> u64 {
    unsafe { sc1(SYS_BRK, addr) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_mmap(addr: u64, len: usize, prot: u64) -> *mut u8 {
    let r = unsafe { sc4(SYS_MMAP, addr, len as u64, prot, 0) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_munmap(addr: *mut u8, len: usize) -> i64 {
    unsafe { sc2(SYS_MUNMAP, addr as u64, len as u64) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_mprotect(addr: u64, len: usize, prot: u64) -> i64 {
    unsafe { sc3(SYS_MPROTECT, addr, len as u64, prot) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_set_tls(addr: u64) -> i64 {
    unsafe { sc1(SYS_SET_TLS, addr) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_get_tls() -> u64 {
    unsafe { sc1(SYS_GET_TLS, 0) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_map_lib(name: *const u8, name_len: usize) -> i64 {
    unsafe { sc2(SYS_MAP_LIB, name as u64, name_len as u64) }
}
