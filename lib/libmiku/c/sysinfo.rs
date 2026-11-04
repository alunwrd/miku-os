// sys/utsname.h, sys/resource.h's getrusage, unistd.h's pathconf
//
// `uname`: the release is the kernel's, read from `/proc/version` ("MikuOS
// v1.0.0 (x86_64)"). The node name is `/etc/hostname`, else "miku".
//
// `getrusage`: CPU time per task in 4 ms ticks, from `procinfo`(103) for the
// process's first thread, is `ru_utime`. No user/system split, and nothing kept
// for reaped children, so `RUSAGE_CHILDREN` is zeros.
//
// `pathconf`: the limits the kernel enforces, the same for each filesystem.

use crate::libc::{close, fail, getpid, open, read, EINVAL, ENOSYS};

const FIELD: usize = 65;

/// Linux's `struct utsname`, six 65-byte fields
#[repr(C)]
pub struct utsname {
    pub sysname: [u8; FIELD],
    pub nodename: [u8; FIELD],
    pub release: [u8; FIELD],
    pub version: [u8; FIELD],
    pub machine: [u8; FIELD],
    pub domainname: [u8; FIELD],
}

fn read_small(path: &[u8], buf: &mut [u8]) -> usize {
    let fd = open(path.as_ptr(), 0x0001, 0);
    if fd < 0 { return 0; }
    let r = read(fd, buf.as_mut_ptr(), buf.len());
    close(fd);
    r.max(0) as usize
}

fn set(field: &mut [u8; FIELD], s: &[u8]) {
    let n = s.len().min(FIELD - 1);
    field[..n].copy_from_slice(&s[..n]);
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
    // "MikuOS v1.0.0 (x86_64)": the word after the name, without `v`
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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// Linux's `struct rusage`: two times, then fourteen counters
#[repr(C)]
pub struct rusage {
    pub ru_utime: timeval,
    pub ru_stime: timeval,
    pub counters: [i64; 14],
}

const RUSAGE_SELF: i32 = 0;
const RUSAGE_CHILDREN: i32 = -1;
const RUSAGE_THREAD: i32 = 1;
const TICK_US: i64 = 4_000;
const PROCINFO_SIZE: usize = 64;

/// The CPU ticks `procinfo` reports for `pid`
fn cpu_ticks(pid: u64) -> Option<u64> {
    let mut table = [0u8; PROCINFO_SIZE * 128];
    let n = unsafe { crate::sys::sc2(crate::sys::SYS_PROCINFO, table.as_mut_ptr() as u64, 128) };
    if n < 0 { return None; }
    table.chunks_exact(PROCINFO_SIZE).take(n as usize).find_map(|r| {
        let at = |o: usize| u64::from_le_bytes(r[o..o + 8].try_into().unwrap());
        (at(0) == pid).then(|| at(24))
    })
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn getrusage(who: i32, usage: *mut rusage) -> i32 {
    if usage.is_null() { return fail(EINVAL); }
    let zero = timeval { tv_sec: 0, tv_usec: 0 };
    let mut r = rusage { ru_utime: zero, ru_stime: zero, counters: [0; 14] };
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

const _PC_LINK_MAX: i32 = 0;
const _PC_MAX_CANON: i32 = 1;
const _PC_MAX_INPUT: i32 = 2;
const _PC_NAME_MAX: i32 = 3;
const _PC_PATH_MAX: i32 = 4;
const _PC_PIPE_BUF: i32 = 5;
const _PC_CHOWN_RESTRICTED: i32 = 6;
const _PC_NO_TRUNC: i32 = 7;
const _PC_VDISABLE: i32 = 8;

fn path_limit(name: i32) -> i64 {
    match name {
        // ext's ceiling, the most any filesystem here allows
        _PC_LINK_MAX => 65000,
        // The line discipline's buffer (`kernel/io/tty/ldisc.rs`)
        _PC_MAX_CANON | _PC_MAX_INPUT => 256,
        // `vfs::types::NAME_LEN` and the kernel's path copy limit
        _PC_NAME_MAX => 255,
        _PC_PATH_MAX => 4096,
        _PC_PIPE_BUF => 65536,
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
