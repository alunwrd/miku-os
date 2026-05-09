// User-space memory access helpers: pointer validation and string copy-in

use super::errno::{err, EFAULT, EINVAL};
use crate::vmm::AddressSpace;

pub const PAGE_SIZE: u64 = 4096;
pub const USER_MAX:  u64 = 0x0000_7FFF_FFFF_FFFF;

#[inline]
pub fn current_cr3() -> u64 {
    let (frame, _) = x86_64::registers::control::Cr3::read();
    frame.start_address().as_u64()
}

#[inline]
pub fn current_pid() -> u64 {
    crate::scheduler::current_pid()
}

// Returns true if [ptr, ptr+len) is fully mapped in the user portion of 'cr3'
pub fn user_ptr_mapped(cr3: u64, ptr: u64, len: u64) -> bool {
    if ptr == 0 || len == 0 { return false; }
    if ptr > USER_MAX { return false; }
    let end = match ptr.checked_add(len) {
        Some(e) if e <= USER_MAX + 1 => e,
        _ => return false,
    };
    let aspace = AddressSpace::from_raw(cr3);
    let start_page = ptr & !0xFFF;
    let end_page = (end + 0xFFF) & !0xFFF;
    let mut va = start_page;
    let mut ok = true;
    while va < end_page {
        if aspace.virt_to_phys(va).is_none() { ok = false; break; }
        va += PAGE_SIZE;
    }
    let _ = aspace.into_raw();
    ok
}

// Copy a user-supplied path string into a kernel-readable &str
// Returns EFAULT if memory is unmapped, EINVAL on bad UTF-8 or out-of-range length
pub fn read_user_path(ptr: u64, len: u64) -> Result<&'static str, u64> {
    if len == 0 || len > 4096 { return Err(err(EINVAL)); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, ptr, len) { return Err(err(EFAULT)); }
    let bytes = unsafe {
        core::slice::from_raw_parts(ptr as *const u8, len as usize)
    };
    match core::str::from_utf8(bytes) {
        Ok(s)  => Ok(s.trim_end_matches('\0')),
        Err(_) => Err(err(EINVAL)),
    }
}
