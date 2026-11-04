// User-space memory access helpers: pointer validation and string copy-in
//
// Rules enforced here:
//   - Range must lie entirely within the canonical user half
//   - Every page in the range must be PRESENT and USER_ACCESSIBLE (otherwise
//     the
//     kernel could be tricked into reading or writing a supervisor-only mapping
//     in the user VA range)
//   - Paths are copied into kernel-owned memory, never borrowed from user
//     pages:
//     the user can unmap or rewrite the bytes after validation (TOCTOU)

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::errno::{err, EFAULT, EINVAL};
use crate::mm::vmm::AddressSpace;

pub const PAGE_SIZE: u64 = 4096;
/// Re-exported so the boundary has one definition (mm::vmm)
pub use crate::mm::vmm::USER_MAX;

const PTE_PRESENT:  u64 = 1 << 0;
const PTE_WRITABLE: u64 = 1 << 1;
const PTE_USER:     u64 = 1 << 2;

// Largest path length we'll accept from userspace. Larger requests are
// almost certainly malicious or buggy
const MAX_PATH_LEN: u64 = 4096;

// Caps on exec()-time argv. The elf_loader enforces its own byte budget
// (ARG_MAX_STRING_BYTES); these bound the kernel-heap copies made here
const MAX_ARGV_ENTRIES:     u64 = 256;
const MAX_ARG_BYTES:        u64 = 32 * 1024;
const MAX_ARGV_TOTAL_BYTES: u64 = 128 * 1024;

/// Permission to touch user memory from ring 0, for as long as this value is
/// alive.
///
/// With SMAP on, a kernel read or write of a user page faults unless RFLAGS.AC
/// is set. The two validators below hand back one of these, and Drop closes it,
/// so the check and the window are one thing and no `clac` can be missed.
///
/// Windows nest. Each one remembers whether AC was set when it opened, and
/// closing it clears AC only if it set it: a helper opening and closing its own
/// window (`read_sockaddr_in` inside `sendto`) must not shut its caller's
#[must_use = "the SMAP window closes as soon as this is dropped"]
pub struct UserAccess {
    /// AC was set before this window opened, so closing it leaves it set
    outer: bool,
}

impl UserAccess {
    /// Opens the window without a range check, for places that validated their
    /// pointer earlier, or touch user memory they mapped themselves
    #[inline]
    pub fn assume_checked() -> Self {
        let rflags: u64;
        unsafe { core::arch::asm!("pushfq", "pop {}", out(reg) rflags, options(nomem, preserves_flags)) };
        crate::arch::x86_64::cpu::stac();
        UserAccess { outer: rflags & (1 << 18) != 0 }
    }
}

impl Drop for UserAccess {
    #[inline]
    fn drop(&mut self) {
        if !self.outer {
            crate::arch::x86_64::cpu::clac();
        }
    }
}

#[inline]
pub fn current_cr3() -> u64 {
    let (frame, _) = x86_64::registers::control::Cr3::read();
    frame.start_address().as_u64()
}

#[inline]
pub fn current_pid() -> u64 {
    crate::sched::current_pid()
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
    // Lazy VMA pages (demand-zero stack/mmap, file-backed mmap) are valid
    // user memory that simply has not faulted in yet; populate them before
    // the PTE walk so they don't spuriously EFAULT. Callers run this at
    // syscall entry, before any VFS lock is taken
    crate::mm::mmap::fault_in_user_range(cr3, ptr, len);
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

// Some(UserAccess) if [ptr, ptr+len) is fully mapped and accessible to the
// owning userspace process: PRESENT + USER on every page.
//
// Returns the access token rather than a bool, so the SMAP window and the check
// justifying it are one thing
#[must_use]
pub fn user_ptr_mapped(cr3: u64, ptr: u64, len: u64) -> Option<UserAccess> {
    if check_user_range(cr3, ptr, len, PTE_USER) {
        Some(UserAccess::assume_checked())
    } else {
        None
    }
}

// Same as user_ptr_mapped but also requires every page to be writable from
// userspace (PTE_WRITABLE). For when the kernel is about to store through the
// pointer: read-only mappings would bypass the user's intent, since the kernel
// ignores W^X at CPL=0
#[must_use]
pub fn user_ptr_writable(cr3: u64, ptr: u64, len: u64) -> Option<UserAccess> {
    if ptr == 0 || len == 0 {
        return None;
    }
    // After fork, user pages are COW = mapped read-only; a kernel-side
    // write (syscall out-parameter) never triggers the #PF COW path the
    // way a user write would (CR0.WP makes it a kernel fault instead).
    // Break COW eagerly for every page in the range, then verify
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return None,
    };
    // break_cow_page fails on not-present pages, so lazy VMA pages must be
    // populated before the COW walk
    crate::mm::mmap::fault_in_user_range(cr3, ptr, len);
    let mut va = ptr & !0xFFF;
    while va < end {
        if !crate::mm::vmm::break_cow_page(cr3, va) {
            return None;
        }
        va += PAGE_SIZE;
    }
    // Writable, or copy-on-write, which is writable to the program, and what a
    // sibling thread's `fork` makes a page again right after the break above.
    // Requiring the bit itself would fail that race with EFAULT. The kernel's
    // own write through a page that is COW again faults and is broken like a
    // user's (`resolve_page_fault`)
    let Some((start_page, end_page)) = check_range(ptr, len) else { return None; };
    let aspace = AddressSpace::from_raw(cr3);
    let mut va = start_page;
    let mut ok = true;
    while va < end_page {
        match aspace.read_pte_raw(va) {
            Some(pte) if pte & (PTE_PRESENT | PTE_USER) == PTE_PRESENT | PTE_USER
                && pte & (PTE_WRITABLE | crate::mm::vmm::PTE_COW) != 0 => {}
            _ => { ok = false; break; }
        }
        va += PAGE_SIZE;
    }
    let _ = aspace.into_raw();
    if ok { Some(UserAccess::assume_checked()) } else { None }
}

// Copies a user-supplied path string into kernel-owned memory, so the owner
// cannot unmap or rewrite the bytes before a later VFS call that may yield
pub fn read_user_path(ptr: u64, len: u64) -> Result<String, u64> {
    if len == 0 || len > MAX_PATH_LEN { return Err(err(EINVAL)); }
    let cr3 = current_cr3();
    let _ua = match user_ptr_mapped(cr3, ptr, len) { Some(g) => g, None => return Err(err(EFAULT)) };

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

/// Where a relative path starts: the high half of the word carrying its length
/// (docs/MikuOS_ABI.md, 3.2e). Zero there (a length never reaches bit 12) is
/// the working directory; `n` is the directory open on descriptor `n - 1`
///
/// A length of zero with a descriptor names the descriptor's own vnode, which
/// need not be a directory: `AT_EMPTY_PATH`, how `fstatvfs` on a file asks
/// about its filesystem
#[derive(Clone, Copy)]
pub enum PathBase {
    Cwd,
    Fd(usize),
    Itself(usize),
}

impl PathBase {
    /// The vnode a relative path is resolved from. Called inside `with_vfs`,
    /// so the descriptor cannot close between being looked at and being used
    pub fn start(self, vfs: &crate::fs::vfs::core::MikuVFS) -> Result<usize, u64> {
        let (fd, dir_only) = match self {
            PathBase::Cwd => return Ok(crate::sched::current_cwd() as usize),
            PathBase::Fd(fd) => (fd, true),
            PathBase::Itself(fd) => (fd, false),
        };
        let vid = match vfs.fds().get(fd) {
            Ok(f) => f.vnode_id as usize,
            Err(e) => return Err(super::errno::vfs_err(e)),
        };
        if !vfs.valid_vnode(vid) { return Err(err(super::errno::EBADF)); }
        if dir_only && !vfs.nodes[vid].is_dir() { return Err(err(super::errno::ENOTDIR)); }
        Ok(vid)
    }
}

/// A path, and where it starts if relative: `read_user_path` for the low half
/// of `word`, `PathBase` for the high one. The empty path is only a path with a
/// descriptor under it; alone it is `EINVAL`
pub fn read_user_path_at(ptr: u64, word: u64) -> Result<(String, PathBase), u64> {
    let len = word & 0xFFFF_FFFF;
    match word >> 32 {
        0 => Ok((read_user_path(ptr, len)?, PathBase::Cwd)),
        n if len == 0 => Ok((String::new(), PathBase::Itself((n - 1) as usize))),
        n => Ok((read_user_path(ptr, len)?, PathBase::Fd((n - 1) as usize))),
    }
}

// Copy a NUL-terminated C string from user space into kernel memory.
// Reads byte by byte (after page-level mapping check) up to 'max' bytes,
// stopping at the first NUL. Returns EINVAL on missing terminator and
// EFAULT on unmapped pages
pub fn read_user_cstr(ptr: u64, max: u64) -> Result<String, u64> {
    if ptr == 0 || max == 0 { return Err(err(EINVAL)); }
    let max = max.min(MAX_ARG_BYTES);
    let cr3 = current_cr3();

    let mut buf: Vec<u8> = Vec::new();
    let mut va = ptr;
    let mut checked_to: u64 = 0;
    // The access window has to outlive the `if` that opens it, so it is held
    // here and replaced page by page. Dropped explicitly before the next is
    // taken, so the clac of the old cannot land after the stac of the new
    let mut win: Option<UserAccess> = None;
    while (buf.len() as u64) < max {
        // Validates one page at a time so strings spanning into a page ending
        // before max are not rejected
        if va >= checked_to {
            win = None;
            win = Some(match user_ptr_mapped(cr3, va, 1) {
                Some(g) => g,
                None    => return Err(err(EFAULT)),
            });
            checked_to = (va & !0xFFF) + PAGE_SIZE;
        }
        let b = unsafe { core::ptr::read_volatile(va as *const u8) };
        if b == 0 {
            return String::from_utf8(buf).map_err(|_| err(EINVAL));
        }
        buf.push(b);
        va = match va.checked_add(1) { Some(v) => v, None => return Err(err(EFAULT)) };
    }
    drop(win);
    Err(err(EINVAL))
}

// Read a user-supplied argv: an array of 'argc' user pointers, each
// pointing to a NUL-terminated string. Strings are copied into kernel-
// owned memory so the caller can swap address spaces safely afterwards.
// Caps applied: at most MAX_ARGV_ENTRIES entries, each at most
// MAX_ARG_BYTES long. Returns Ok(empty) when argc == 0 or argv_ptr == 0
pub fn read_user_argv(argv_ptr: u64, argc: u64) -> Result<Vec<String>, u64> {
    if argc == 0 || argv_ptr == 0 { return Ok(Vec::new()); }
    if argc > MAX_ARGV_ENTRIES { return Err(err(EINVAL)); }

    let bytes = argc.checked_mul(8).ok_or_else(|| err(EINVAL))?;
    let cr3   = current_cr3();
    let _ua = match user_ptr_mapped(cr3, argv_ptr, bytes) { Some(g) => g, None => return Err(err(EFAULT)) };

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
    let mut total: u64 = 0;
    for &p in ptrs.iter() {
        if p == 0 {
            // POSIX allows a trailing NULL; stops early, as well-behaved argv
            // ends with one NULL
            break;
        }
        let s = read_user_cstr(p, MAX_ARG_BYTES)?;
        // Total budget bounds the kernel-heap copy of the whole vector,
        // matching the elf_loader's stack-side string budget
        total = total.saturating_add(s.len() as u64 + 1);
        if total > MAX_ARGV_TOTAL_BYTES {
            return Err(err(EINVAL));
        }
        out.push(s);
    }
    Ok(out)
}
