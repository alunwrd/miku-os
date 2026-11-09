// sys/utsname.h, sys/resource.h's getrusage, unistd.h's pathconf
//
// `uname`: the release is the kernel's, read from `/proc/version` ("MikuOS
// v1.0.1 (x86_64)"). The node name is `/etc/hostname`, else "miku".
//
// `getrusage`: CPU time per task in 4 ms ticks, from `procinfo`(103) for the
// process's first thread, is `ru_utime`. No user/system split, and nothing kept
// for reaped children, so `RUSAGE_CHILDREN` is zeros.
//
// `pathconf`: the limits the kernel enforces, the same for each filesystem.

use crate::libc::{close, fail, getpid, open, read, EINVAL, ENOSYS};

const FIELD: usize = 65;

/// Linux's `struct utsname`, six 65-byte fields
pub use miku_abi::utsname::utsname;

fn read_small(path: &[u8], buf: &mut [u8]) -> usize {
    let fd = open(path.as_ptr(), 0x0001, 0);
    if fd < 0 { return 0; }
    let r = read(fd, buf.as_mut_ptr(), buf.len());
    close(fd);
    r.max(0) as usize
}

fn set(field: &mut [core::ffi::c_char; FIELD], s: &[u8]) {
    let n = s.len().min(FIELD - 1);
    for (to, from) in field.iter_mut().zip(s[..n].iter()) {
        *to = *from as core::ffi::c_char;
    }
    field[n..].fill(0);
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn uname(u: *mut utsname) -> i32 {
    if u.is_null() { return fail(EINVAL); }
    let u = unsafe { &mut *u };
    set(&mut u.sysname, b"MikuOS");
    set(&mut u.machine, b"x86_64");
    set(&mut u.domainname, b"(none)");

    let mut v = [0u8; 128];
    let n = read_small(b"/proc/version\0", &mut v);
    let first = v[..n].split(|&b| b == b'\n').next().unwrap_or(b"");
    // "MikuOS v1.0.1 (x86_64)": the word after the name, without `v`
    let release = first
        .split(|&b| b == b' ')
        .nth(1)
        .map(|r| r.strip_prefix(b"v").unwrap_or(r))
        .unwrap_or(b"unknown");
    set(&mut u.release, release);
    set(&mut u.version, first);

    let mut h = [0u8; FIELD];
    let n = read_small(b"/etc/hostname\0", &mut h);
    let host = h[..n].split(|&b| b == b'\n' || b == b' ').next().unwrap_or(b"");
    set(&mut u.nodename, if host.is_empty() { b"miku" } else { host });
    0
}

pub use miku_abi::posix::timeval;

/// Linux's `struct rusage`: two times, then fourteen counters
pub use miku_abi::posix::rusage;

use miku_abi::resource::RUSAGE_SELF;
use miku_abi::resource::RUSAGE_CHILDREN;
use miku_abi::resource::RUSAGE_THREAD;
const TICK_US: i64 = 4_000;

/// The CPU ticks `procinfo` reports for `pid`. None if it is not among the first
/// 128 processes
fn cpu_ticks(pid: u64) -> Option<u64> {
    let mut table = [miku_abi::kproc::ProcInfo::default(); 128];
    let n = unsafe { crate::sys::sc2(crate::sys::SYS_PROCINFO, table.as_mut_ptr() as u64, table.len() as u64) };
    if n < 0 { return None; }
    table.iter().take(n as usize).find(|p| p.pid == pid).map(|p| p.cpu_time)
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn getrusage(who: i32, usage: *mut rusage) -> i32 {
    if usage.is_null() { return fail(EINVAL); }
    let zero = timeval { tv_sec: 0, tv_usec: 0 };
    let mut r: rusage = unsafe { core::mem::zeroed() };
    r.ru_utime = zero;
    r.ru_stime = zero;
    match who {
        RUSAGE_SELF | RUSAGE_THREAD => {
            let us = cpu_ticks(getpid() as u64).unwrap_or(0) as i64 * TICK_US;
            r.ru_utime = timeval { tv_sec: us / 1_000_000, tv_usec: us % 1_000_000 };
        }
        RUSAGE_CHILDREN => {}
        _ => return fail(EINVAL),
    }
    unsafe { usage.write(r) };
    0
}

use miku_abi::unistd::_PC_LINK_MAX;
use miku_abi::unistd::_PC_MAX_CANON;
use miku_abi::unistd::_PC_MAX_INPUT;
use miku_abi::unistd::_PC_NAME_MAX;
use miku_abi::unistd::_PC_PATH_MAX;
use miku_abi::unistd::_PC_PIPE_BUF;
use miku_abi::unistd::_PC_CHOWN_RESTRICTED;
use miku_abi::unistd::_PC_NO_TRUNC;
use miku_abi::unistd::_PC_VDISABLE;

fn path_limit(name: i32) -> i64 {
    match name {
        // ext's ceiling, the most any filesystem here allows
        _PC_LINK_MAX => 65000,
        // The line discipline's buffer (`kernel/io/tty/ldisc.rs`)
        _PC_MAX_CANON | _PC_MAX_INPUT => 256,
        // `vfs::types::NAME_LEN` and the kernel's path copy limit
        _PC_NAME_MAX => miku_abi::limits::NAME_MAX as i64,
        _PC_PATH_MAX => miku_abi::limits::PATH_MAX as i64,
        _PC_PIPE_BUF => miku_abi::limits::PIPE_BUF as i64,
        // Only root changes an owner, and a long name is refused, not cut
        _PC_CHOWN_RESTRICTED | _PC_NO_TRUNC => 1,
        _PC_VDISABLE => 0,
        _ => { fail(EINVAL); -1 }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pathconf(path: *const u8, name: i32) -> i64 {
    if path.is_null() { return fail(EINVAL) as i64; }
    path_limit(name)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fpathconf(fd: i32, name: i32) -> i64 {
    if fd < 0 { return fail(crate::libc::EBADF) as i64; }
    path_limit(name)
}

/// Named pipes: the kernel's pipes are anonymous vnodes, and no filesystem here
/// stores a FIFO node
#[no_mangle]
#[inline(never)]
pub extern "C" fn mkfifo(_path: *const u8, _mode: u32) -> i32 {
    fail(ENOSYS)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn mkfifoat(_dirfd: i32, _path: *const u8, _mode: u32) -> i32 {
    fail(ENOSYS)
}

/// No call flushes one filesystem; `fsync` on a descriptor is real
#[no_mangle]
#[inline(never)]
pub extern "C" fn syncfs(_fd: i32) -> i32 {
    fail(ENOSYS)
}
