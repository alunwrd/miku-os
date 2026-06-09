// User-space memory access helpers: pointer validation and string copy-in
//
// Security rules enforced here:
//   - Range must lie entirely within the canonical user half
//   - Every page in the range must be PRESENT *and* USER_ACCESSIBLE
//     (otherwise the kernel could be tricked into reading or writing a
//     supervisor-only mapping that happens to sit in the user VA range)
//   - Paths are copied into kernel-owned memory, never borrowed from
//     user pages - the user can otherwise unmap or rewrite the bytes
//     after validation and before the kernel finishes using them (TOCTOU)

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::errno::{err, EFAULT, EINVAL};
use crate::vmm::AddressSpace;

pub const PAGE_SIZE: u64 = 4096;
pub const USER_MAX:  u64 = 0x0000_7FFF_FFFF_FFFF;

const PTE_PRESENT:  u64 = 1 << 0;
const PTE_WRITABLE: u64 = 1 << 1;
const PTE_USER:     u64 = 1 << 2;

// Largest path length we'll accept from userspace. Larger requests are
// almost certainly malicious or buggy
const MAX_PATH_LEN: u64 = 4096;

// Hard caps on exec()-time argv to keep stack-build bounded and to mirror
// the elf_loader MAX_ARGS = 64 limit
const MAX_ARGV_ENTRIES: u64 = 64;
const MAX_ARG_BYTES:    u64 = 4096;

#[inline]
pub fn current_cr3() -> u64 {
    let (frame, _) = x86_64::registers::control::Cr3::read();
    frame.start_address().as_u64()
}

#[inline]
pub fn current_pid() -> u64 {
    crate::scheduler::current_pid()
}

#[inline]
fn check_range(ptr: u64, len: u64) -> Option<(u64, u64)> {
    if ptr == 0 || len == 0 { return None; }
    if ptr > USER_MAX { return None; }
    let end = ptr.checked_add(len)?;
    if end > USER_MAX + 1 { return None; }
    let start_page = ptr & !0xFFF;
    let end_page   = (end + 0xFFF) & !0xFFF;
    Some((start_page, end_page))
}

// Walk PTEs for every page in [ptr, ptr+len) and verify required_flags
// are all set. PTE_PRESENT is always required
fn check_user_range(cr3: u64, ptr: u64, len: u64, required_flags: u64) -> bool {
    let Some((start_page, end_page)) = check_range(ptr, len) else { return false; };
    let want = PTE_PRESENT | required_flags;
    let aspace = AddressSpace::from_raw(cr3);
    let mut va = start_page;
    let mut ok = true;
    while va < end_page {
        match aspace.read_pte_raw(va) {
            Some(pte) if pte & want == want => {}
            _ => { ok = false; break; }
        }
        va += PAGE_SIZE;
    }
    let _ = aspace.into_raw();
    ok
}

// Returns true if [ptr, ptr+len) is fully mapped and accessible to the
// owning userspace process - PRESENT + USER on every page
pub fn user_ptr_mapped(cr3: u64, ptr: u64, len: u64) -> bool {
    check_user_range(cr3, ptr, len, PTE_USER)
}

// Same as user_ptr_mapped but also requires every page to be writable
// from userspace (PTE_WRITABLE). Use when the kernel is about to
// store-through the pointer - read-only mappings would silently bypass
// the user's intent and the kernel ignores W^X at CPL=0
pub fn user_ptr_writable(cr3: u64, ptr: u64, len: u64) -> bool {
    check_user_range(cr3, ptr, len, PTE_USER | PTE_WRITABLE)
}

// Copy a user-supplied path string into kernel-owned memory.
//
// Returning &'static str borrowed from user memory was unsound: the
// owning process can unmap or rewrite the bytes between this check and
// any subsequent VFS call that may yield, leading to TOCTOU - the
// kernel ends up acting on a different path than it validated
pub fn read_user_path(ptr: u64, len: u64) -> Result<String, u64> {
    if len == 0 || len > MAX_PATH_LEN { return Err(err(EINVAL)); }
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, ptr, len) { return Err(err(EFAULT)); }

    // Copy into a kernel buffer before any UTF-8 / parsing work. After
    // this point the bytes can't change under us
    let mut buf = vec![0u8; len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(
            ptr as *const u8,
            buf.as_mut_ptr(),
            len as usize,
        );
    }
    // Strip trailing NUL padding before UTF-8 validation
    while let Some(&0) = buf.last() { buf.pop(); }

    match String::from_utf8(buf) {
        Ok(s)  => Ok(s),
        Err(_) => Err(err(EINVAL)),
    }
}

// Copy a NUL-terminated C string from user space into kernel memory.
// Reads byte by byte (after page-level mapping check) up to `max` bytes,
// stopping at the first NUL. Returns EINVAL on missing terminator and
// EFAULT on unmapped pages
pub fn read_user_cstr(ptr: u64, max: u64) -> Result<String, u64> {
    if ptr == 0 || max == 0 { return Err(err(EINVAL)); }
    let max = max.min(MAX_ARG_BYTES);
    let cr3 = current_cr3();

    let mut buf: Vec<u8> = Vec::new();
    let mut va = ptr;
    let mut checked_to: u64 = 0;
    while (buf.len() as u64) < max {
        // Validate one page worth at a time so we don't reject strings
        // that legitimately span into a page that ends before max
        if va >= checked_to {
            if !user_ptr_mapped(cr3, va, 1) { return Err(err(EFAULT)); }
            checked_to = (va & !0xFFF) + PAGE_SIZE;
        }
        let b = unsafe { core::ptr::read_volatile(va as *const u8) };
        if b == 0 {
            return String::from_utf8(buf).map_err(|_| err(EINVAL));
        }
        buf.push(b);
        va = match va.checked_add(1) { Some(v) => v, None => return Err(err(EFAULT)) };
    }
    Err(err(EINVAL))
}

// Read a user-supplied argv: an array of `argc` user pointers, each
// pointing to a NUL-terminated string. Strings are copied into kernel-
// owned memory so the caller can swap address spaces safely afterwards.
// Caps applied: at most MAX_ARGV_ENTRIES entries, each at most
// MAX_ARG_BYTES long. Returns Ok(empty) when argc == 0 or argv_ptr == 0
pub fn read_user_argv(argv_ptr: u64, argc: u64) -> Result<Vec<String>, u64> {
    if argc == 0 || argv_ptr == 0 { return Ok(Vec::new()); }
    if argc > MAX_ARGV_ENTRIES { return Err(err(EINVAL)); }

    let bytes = argc.checked_mul(8).ok_or_else(|| err(EINVAL))?;
    let cr3   = current_cr3();
    if !user_ptr_mapped(cr3, argv_ptr, bytes) { return Err(err(EFAULT)); }

    // Snapshot the pointer array first so an unmap race can't change it
    // mid-iteration
    let mut ptrs = vec![0u64; argc as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(
            argv_ptr as *const u64,
            ptrs.as_mut_ptr(),
            argc as usize,
        );
    }

    let mut out: Vec<String> = Vec::with_capacity(argc as usize);
    for &p in ptrs.iter() {
        if p == 0 {
            // POSIX allows trailing NULL but we stop early - well-behaved
            // argv ends with one NULL terminator
            break;
        }
        out.push(read_user_cstr(p, MAX_ARG_BYTES)?);
    }
    Ok(out)
}
