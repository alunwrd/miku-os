// Memory-management syscalls: mmap, munmap, mprotect, brk, TLS, map_lib, getcwd

extern crate alloc;

use x86_64::VirtAddr;
use x86_64::registers::model_specific::FsBase;

use super::errno::{err, EFAULT, EINVAL};
use super::user_mem::{current_cr3, user_ptr_mapped, user_ptr_writable, USER_MAX};
use crate::mmap;

// 3  mmap(addr, len, prot, flags) -> addr_or_errno
pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64) -> u64 {
    if len == 0 { return err(EINVAL); }
    let cr3 = current_cr3();
    let mflags = (flags as u32) | 0x20;
    let result = mmap::sys_mmap(cr3, addr, len, prot as u32, mflags, -1, 0);
    if result < 0 { err(result as i64) } else { result as u64 }
}

// 60  mmap_file(args_ptr) -> addr_or_errno
//
// The 4-argument syscall ABI can't carry mmap's six parameters, so file
// mappings pass them through a struct in user memory (mmap2 style):
//   struct MmapFileArgs { addr, length, prot, flags, fd, offset } (u64 x6)
pub fn sys_mmap_file(args_ptr: u64) -> u64 {
    const N: usize = 6 * 8;
    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, args_ptr, N as u64) {
        return err(EFAULT);
    }
    let mut raw = [0u8; N];
    unsafe {
        core::ptr::copy_nonoverlapping(args_ptr as *const u8, raw.as_mut_ptr(), N);
    }
    let g = |i: usize| u64::from_le_bytes(raw[i * 8..i * 8 + 8].try_into().unwrap());
    let addr   = g(0);
    let length = g(1);
    let prot   = g(2) as u32;
    let flags  = g(3) as u32;
    let fd     = g(4) as usize;
    let offset = g(5);

    if length == 0 { return err(EINVAL); }

    // Resolve the fd to its backing vnode + size (mapping pins the vnode,
    // not the fd, so it survives close)
    let Some((vnode_id, file_size)) = crate::vfs::core::fd_backing(fd) else {
        return err(EINVAL);
    };

    // MAP_SHARED = 0x01, MAP_PRIVATE = 0x02 (one is required)
    let shared = flags & 0x01 != 0;
    let mflags = flags | 0x20; // force the address-validation path

    let result = mmap::sys_mmap_file(
        cr3, addr, length, prot, mflags, vnode_id, offset, file_size, shared,
    );
    if result < 0 { err(result as i64) } else { result as u64 }
}

// 61  msync(addr, len) -> 0 / errno
pub fn sys_msync(addr: u64, len: u64) -> u64 {
    if addr & 0xFFF != 0 { return err(EINVAL); }
    let result = mmap::sys_msync(current_cr3(), addr, len);
    if result < 0 { err(result as i64) } else { 0 }
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

// 8  getcwd(buf, size) -> buf
//
// Walks the parent chain from the calling process's cwd (Process::cwd,
// accessed via scheduler::current_cwd) up to the root vnode. Returns
// -EINVAL if the path doesn't fit in 'size'
pub fn sys_getcwd(buf: u64, size: u64) -> u64 {
    if size < 2 { return err(EINVAL); }
    let cr3 = current_cr3();
    // The kernel writes through buf - PTE_WRITABLE required so the user
    // can't trick us into store-through-a-read-only-mapping (which works
    // for CPL=0 regardless of WP) or hit a page they should not be able
    // to clobber
    if !user_ptr_writable(cr3, buf, size) { return err(EFAULT); }

    let cwd_id = crate::scheduler::current_cwd() as usize;
    let path = crate::vfs::core::with_vfs(|vfs| {
        let mut cur = cwd_id;
        let root    = vfs.ctx.root as usize;

        // Collect names walking up to root. Cap iterations so a corrupt
        // parent cycle can't loop forever; in practice depth is bounded
        // by the path length
        let mut parts: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
        let mut steps = 0usize;
        while cur != root && steps < 1024 {
            if !vfs.valid_vnode(cur) { return alloc::string::String::from("/"); }
            let n = vfs.nodes[cur].name.as_str();
            parts.push(alloc::string::String::from(n));
            let parent = vfs.nodes[cur].parent as usize;
            if parent == cur { break; } // self-parent guard
            cur = parent;
            steps += 1;
        }
        if parts.is_empty() {
            return alloc::string::String::from("/");
        }
        let mut s = alloc::string::String::new();
        for name in parts.iter().rev() {
            s.push('/');
            s.push_str(name);
        }
        s
    });

    let bytes = path.as_bytes();
    if (bytes.len() as u64) + 1 > size { return err(EINVAL); }
    unsafe {
        let p = buf as *mut u8;
        for (i, &b) in bytes.iter().enumerate() {
            p.add(i).write(b);
        }
        p.add(bytes.len()).write(0);
    }
    buf
}

// 9  set_tls(addr) -> 0
// Userspace can only set FS.base to a canonical user-half address.
// A non-canonical or kernel-half value would either #GP wrmsr (kernel
// DoS) or let a later kernel-side FS-prefixed access read the wrong
// half of the virtual space. VirtAddr::try_new performs the canonical
// check for us; we additionally reject anything above USER_MAX
pub fn sys_set_tls(addr: u64) -> u64 {
    if addr > USER_MAX { return err(EINVAL); }
    let va = match VirtAddr::try_new(addr) {
        Ok(v)  => v,
        Err(_) => return err(EINVAL),
    };
    FsBase::write(va);
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
    // Copy the name into a kernel-owned buffer before parsing; the
    // user could otherwise rewrite the bytes between validation and the
    // solib lookup that yields on disk I/O
    let mut name_buf = [0u8; 256];
    unsafe {
        core::ptr::copy_nonoverlapping(
            name_ptr as *const u8,
            name_buf.as_mut_ptr(),
            name_len as usize,
        );
    }
    let mut effective = &name_buf[..name_len as usize];
    while let [rest @ .., 0] = effective { effective = rest; }
    let soname = match core::str::from_utf8(effective) {
        Ok(s)  => s,
        Err(_) => return err(EINVAL),
    };
    match crate::solib::map_into_process(soname, cr3) {
        Ok(base) => base,
        Err(e)   => e as u64,
    }
}
