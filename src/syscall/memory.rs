// Memory-management syscalls: mmap, munmap, mprotect, brk, TLS, map_lib, getcwd

use x86_64::VirtAddr;
use x86_64::registers::model_specific::FsBase;

use super::errno::{err, EFAULT, EINVAL};
use super::user_mem::{current_cr3, user_ptr_mapped};
use crate::mmap;

// 3  mmap(addr, len, prot, flags) -> addr_or_errno
pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64) -> u64 {
    if len == 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let mflags = (flags as u32) | 0x20;
    let result = mmap::sys_mmap(cr3, addr, len, prot as u32, mflags, -1, 0);
    if result < 0 { err(result as i64) } else { result as u64 }
}

// 4  munmap(addr, len) -> 0 / errno
pub fn sys_munmap(addr: u64, len: u64) -> u64 {
    if addr & 0xFFF != 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let result = mmap::sys_munmap(cr3, addr, len);
    if result < 0 { err(result as i64) } else { 0 }
}

// 5  mprotect(addr, len, prot) -> 0 / errno
pub fn sys_mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    if addr & 0xFFF != 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let result = mmap::sys_mprotect(cr3, addr, len, prot as u32);
    if result < 0 { err(result as i64) } else { 0 }
}

// 6  brk(addr) -> new_brk
pub fn sys_brk(addr: u64) -> u64 {
    mmap::sys_brk(current_cr3(), addr)
}

// 8  getcwd(buf, size) -> buf  (cwd is always "/" until cwd-tracking lands)
pub fn sys_getcwd(buf: u64, size: u64) -> u64 {
    if size < 2 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, buf, size) { return err(EFAULT); }
    unsafe {
        let p = buf as *mut u8;
        p.write(b'/');
        p.add(1).write(0);
    }
    buf
}

// 9  set_tls(addr) -> 0
pub fn sys_set_tls(addr: u64) -> u64 {
    FsBase::write(VirtAddr::new(addr));
    crate::serial_println!("[syscall] set_tls={:#x}", addr);
    0
}

// 10  get_tls() -> fs_base
pub fn sys_get_tls() -> u64 {
    FsBase::read().as_u64()
}

// 15  map_lib(name_ptr, name_len) -> base_addr
pub fn sys_map_lib(name_ptr: u64, name_len: u64) -> u64 {
    if name_len == 0 || name_len > 256 { return err(EINVAL); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, name_ptr, name_len) { return err(EFAULT); }
    let name_bytes = unsafe {
        core::slice::from_raw_parts(name_ptr as *const u8, name_len as usize)
    };
    let soname = match core::str::from_utf8(name_bytes) {
        Ok(s)  => s.trim_end_matches('\0'),
        Err(_) => return err(EINVAL),
    };
    match crate::solib::map_into_process(soname, cr3) {
        Ok(base) => base,
        Err(e)   => e as u64,
    }
}
