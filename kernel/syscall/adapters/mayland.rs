// Shared-memory pool syscalls
//
// A client gets at a few megabytes of pixels the compositor can read without a
// copy. Wayland passes a file descriptor over the socket and mmaps it on both
// sides; MikuOS's compositor is the kernel, so a pool is named by a handle
//
//      may_pool(size, out)   allocate a pool, map it, write {handle, addr}
//      may_map(handle, out)  map a pool made for this process, write {addr}
//      may_unpool(handle)    unmap and free it
//
// The handle goes in a may_shm.create_pool request; the address is where the
// client draws. Ownership is by pid and checked here, so a process cannot free
// another's, and in the server, so a connection cannot name a pool its process
// does not own

use crate::syscall::errno::*;
use crate::syscall::usercopy::{current_cr3, current_pid, user_ptr_writable};
use crate::mayland::server::shm::{self, ShmError};

#[inline]
fn e(code: i64) -> u64 { code as u64 }

fn shm_err(err: ShmError) -> u64 {
    e(match err {
        ShmError::BadSize       => EINVAL,
        ShmError::NoMemory      => ENOMEM,
        ShmError::Full          => EMFILE,
        ShmError::OwnerFull     => EMFILE,
        ShmError::BadHandle     => EINVAL,
        ShmError::NotOwner      => EACCES,
        ShmError::AlreadyMapped => EBUSY,
        // Video memory where only system memory will do
        ShmError::Video         => EINVAL,
    })
}

/// may_pool(size, out_ptr) -> 0 | errno
///
/// 'out_ptr' receives two u64s: the handle, then the address the pool is mapped
/// at
pub fn sys_may_pool(size: u64, out_ptr: u64) -> u64 {
    if size == 0 || size > shm::MAX_POOL_BYTES as u64 {
        return e(EINVAL);
    }
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, out_ptr, 16) {
        Some(g) => g,
        None => return e(EFAULT),
    };
    let pid = current_pid();

    let handle = match shm::create(pid, size as usize) {
        Ok(h) => h,
        Err(err) => return shm_err(err),
    };
    let addr = match shm::map_into(handle, pid, cr3) {
        Ok(a) => a,
        Err(err) => {
            let _ = shm::destroy(handle);
            return shm_err(err);
        }
    };

    // Safe: the 16-byte range was validated as writable above
    unsafe {
        core::ptr::write_unaligned(out_ptr as *mut u64, handle as u64);
        core::ptr::write_unaligned((out_ptr + 8) as *mut u64, addr);
    }
    0
}

/// may_map(handle, out_ptr) -> 0 | errno
///
/// A pool the compositor made for this process, such as the keymap, whose
/// layout is the compositor's to decide. Ownership is checked by pid
pub fn sys_may_map(handle: u64, out_ptr: u64) -> u64 {
    if handle == 0 || handle > u32::MAX as u64 {
        return e(EINVAL);
    }
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, out_ptr, 8) {
        Some(g) => g,
        None => return e(EFAULT),
    };
    let pid = current_pid();
    let handle = handle as u32;
    match shm::map_into(handle, pid, cr3) {
        Ok(addr) => {
            // Safe: the 8-byte range was validated as writable above
            unsafe { core::ptr::write_unaligned(out_ptr as *mut u64, addr); }
            0
        }
        Err(err) => shm_err(err),
    }
}

/// may_unpool(handle) -> 0 | errno
pub fn sys_may_unpool(handle: u64) -> u64 {
    if handle == 0 || handle > u32::MAX as u64 {
        return e(EINVAL);
    }
    let handle = handle as u32;
    // Ownership and destruction in one call, under one acquisition of the pool
    // table, so the handle allowed to be freed and the handle freed are the
    // same.
    //
    // Its GPU binding first, if it has one: the slot is the process's to reuse,
    // and the mapping must not outlive the memory
    if shm::claim(handle, current_pid()).is_ok() {
        super::gpu::forget_pool(current_pid(), handle);
    }
    match shm::destroy_owned(handle, current_pid()) {
        Ok(()) => 0,
        Err(err) => shm_err(err),
    }
}
