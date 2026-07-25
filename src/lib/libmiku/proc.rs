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

/// File-backed mmap. 'flags' is MAP_SHARED (1) or MAP_PRIVATE (2). The six
/// parameters travel through a struct in our own stack memory, since the
/// 4-argument syscall ABI can't carry them all
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_mmap_file(
    addr: u64, len: usize, prot: u64, flags: u64, fd: i64, offset: u64,
) -> *mut u8 {
    let args: [u64; 6] = [addr, len as u64, prot, flags, fd as u64, offset];
    let r = unsafe { sc1(SYS_MMAP_FILE, args.as_ptr() as u64) };
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_msync(addr: *mut u8, len: usize) -> i64 {
    unsafe { sc2(SYS_MSYNC, addr as u64, len as u64) }
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

// ---------------------------------------------------------------------------
// Process control: fork / wait / kill / exec
// ---------------------------------------------------------------------------

/// fork(): 0 in the child, child pid in the parent, negative errno on failure
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_fork() -> i64 {
    unsafe { sc0(SYS_FORK) }
}

/// waitpid(pid, *status): blocks until the child exits. pid 0 = any child.
/// status (if non-null) receives the raw exit code. Returns the reaped pid
/// or negative errno
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_waitpid(pid: u64, status: *mut i64) -> i64 {
    unsafe { sc3(SYS_WAIT4, pid, status as u64, 0) }
}

/// kill(pid, sig). sig 9/15 terminate; see kernel sys_kill for the rest
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_kill(pid: u64, sig: u64) -> i64 {
    unsafe { sc2(SYS_KILL, pid, sig) }
}

/// exec(path, argv, argc): replace the current process image. argv is an
/// array of pointers to NUL-terminated strings (argv[0] = program name).
/// The kernel supplies a default environment. Does not return on success
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_exec(path: *const u8, argv: *const *const u8, argc: usize) -> i64 {
    let len = crate::string::miku_strlen(path);
    unsafe { sc4(SYS_EXEC, path as u64, len as u64, argv as u64, argc as u64) }
}

/// execve(path, argv, argc, envp, envc): exec with an explicit environment.
/// envp entries are "KEY=value" C strings, same layout as argv. The six
/// parameters travel through a u64[6] struct (4-arg syscall ABI)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_execve(
    path: *const u8,
    argv: *const *const u8,
    argc: usize,
    envp: *const *const u8,
    envc: usize,
) -> i64 {
    let len = crate::string::miku_strlen(path);
    let args: [u64; 6] = [
        path as u64, len as u64,
        argv as u64, argc as u64,
        envp as u64, envc as u64,
    ];
    unsafe { sc1(SYS_EXECVE, args.as_ptr() as u64) }
}
