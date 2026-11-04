pub const SYS_EXIT:     u64 = 0;
pub const SYS_WRITE:    u64 = 1;
pub const SYS_READ:     u64 = 2;
pub const SYS_MMAP:     u64 = 3;
pub const SYS_MAP_LIB:  u64 = 15;
pub const SYS_MUNMAP:   u64 = 4;
pub const SYS_MPROTECT: u64 = 5;
pub const SYS_SET_TLS:  u64 = 9;
pub const SYS_OPEN:     u64 = 11;
pub const SYS_CLOSE:    u64 = 12;
pub const SYS_SEEK:     u64 = 13;
pub const SYS_FSIZE:    u64 = 14;
/// Kernel log
pub const SYS_KLOG:     u64 = 75;

pub const PROT_READ:  u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC:  u64 = 4;

#[inline(always)]
unsafe fn sc1(nr: u64, a1: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr, in("rdi") a1,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack)
    );
    ret
}

#[inline(always)]
unsafe fn sc2(nr: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr, in("rdi") a1, in("rsi") a2,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack)
    );
    ret
}

#[inline(always)]
unsafe fn sc3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack)
    );
    ret
}

#[inline(always)]
unsafe fn sc4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4,
        lateout("rax") ret,
        out("rcx") _, out("r11") _,
        options(nostack)
    );
    ret
}

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
