// pwd.h and grp.h, and the calls carrying a group list into the kernel
//
// The accounts are `/etc/passwd` and `/etc/group`, world-readable, with `#`
// comments: `name:x:uid:gid:gecos:home:shell` and
// `name:x:gid:member,member,...`. `lib/admin/mikuauth` reads the same files.
//
// `_r` forms only: strings go in the caller's buffer, `ERANGE` when too small,
// and an absent name is success with a null result, per POSIX.
//
// Supplementary groups are the kernel's (`getgroups`(89), `setgroups`(88),
// sixteen at most, 16-bit ids); `initgroups` reads `/etc/group` and calls
// `setgroups`, needing `CAP_SETGID`.

use crate::libc::{close, fail, open, read, strlen, __errno_location, EINVAL, ERANGE};

use miku_abi::cred::MAX_GROUPS;

/// Reads a whole account file into `text`, returning its length
fn read_db(path: &[u8], text: &mut [u8]) -> Result<usize, i32> {
    let fd = open(path.as_ptr(), 0x0001, 0);
    if fd < 0 { return Err(unsafe { *__errno_location() }); }
    let mut n = 0;
    while n < text.len() {
        let r = read(fd, unsafe { text.as_mut_ptr().add(n) }, text.len() - n);
        if r <= 0 { break; }
        n += r as usize;
    }
    close(fd);
    Ok(n)
}

/// Each non-comment line split at `:` into exactly `N` fields; fewer is
/// skipped. The last field keeps further colons
fn records<const N: usize>(text: &[u8]) -> impl Iterator<Item = [&[u8]; N]> {
    text.split(|&b| b == b'\n').filter_map(|line| {
        if line.first() == Some(&b'#') { return None; }
        let mut it = line.splitn(N, |&b| b == b':');
        let mut f = [&b""[..]; N];
        for slot in f.iter_mut() {
            *slot = it.next()?;
        }
        Some(f)
    })
}

fn num(x: &[u8]) -> Option<u32> {
    if x.is_empty() || !x.iter().all(u8::is_ascii_digit) { return None; }
    Some(x.iter().fold(0u32, |a, &d| a.wrapping_mul(10).wrapping_add((d - b'0') as u32)))
}

/// Copies NUL-terminated strings into the caller's buffer, one after another
struct Packer {
    buf: *mut u8,
    at: usize,
}

impl Packer {
    fn put(&mut self, x: &[u8]) -> *mut u8 {
        let p = unsafe { self.buf.add(self.at) };
        unsafe {
            core::ptr::copy_nonoverlapping(x.as_ptr(), p, x.len());
            *p.add(x.len()) = 0;
        }
        self.at += x.len() + 1;
        p
    }
}

fn c_str<'a>(p: *const u8) -> &'a [u8] {
    unsafe { core::slice::from_raw_parts(p, strlen(p)) }
}

// pwd.h //

pub use miku_abi::pwd::passwd;

/// The first `/etc/passwd` line `want` accepts, filled into `pwd`
fn passwd_lookup(
    want: &dyn Fn(&[u8], u32) -> bool,
    pwd: *mut passwd,
    buf: *mut u8,
    buflen: usize,
    result: *mut *mut passwd,
) -> i32 {
    if pwd.is_null() || result.is_null() || (buf.is_null() && buflen > 0) { return EINVAL; }
    unsafe { *result = core::ptr::null_mut() };
    let mut text = [0u8; 8192];
    let n = match read_db(b"/etc/passwd\0", &mut text) {
        Ok(n) => n,
        Err(e) => return e,
    };
    for f in records::<7>(&text[..n]) {
        let (Some(uid), Some(gid)) = (num(f[2]), num(f[3])) else { continue };
        if !want(f[0], uid) { continue; }
        let need: usize = [f[0], f[1], f[4], f[5], f[6]].iter().map(|x| x.len() + 1).sum();
        if need > buflen { return ERANGE; }
        let mut pk = Packer { buf, at: 0 };
        unsafe {
            pwd.write(passwd {
                pw_name: pk.put(f[0]).cast(),
                pw_passwd: pk.put(f[1]).cast(),
                pw_uid: uid,
                pw_gid: gid,
                pw_gecos: pk.put(f[4]).cast(),
                pw_dir: pk.put(f[5]).cast(),
                pw_shell: pk.put(f[6]).cast(),
            });
            *result = pwd;
        }
        return 0;
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpwuid_r(uid: u32, pwd: *mut passwd, buf: *mut u8, buflen: usize, result: *mut *mut passwd) -> i32 {
    passwd_lookup(&|_, u| u == uid, pwd, buf, buflen, result)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpwnam_r(name: *const u8, pwd: *mut passwd, buf: *mut u8, buflen: usize, result: *mut *mut passwd) -> i32 {
    if name.is_null() { return EINVAL; }
    let want = c_str(name);
    passwd_lookup(&|n, _| n == want, pwd, buf, buflen, result)
}

/// `getpwuid_r` into a shared buffer. POSIX lets this one be unsafe across
/// threads; use `getpwuid_r`
#[no_mangle]
#[inline(never)]
pub extern "C" fn getpwuid(uid: u32) -> *mut passwd {
    static mut PW: passwd = passwd {
        pw_name: core::ptr::null_mut(),
        pw_passwd: core::ptr::null_mut(),
        pw_uid: 0,
        pw_gid: 0,
        pw_gecos: core::ptr::null_mut(),
        pw_dir: core::ptr::null_mut(),
        pw_shell: core::ptr::null_mut(),
    };
    static mut BUF: [u8; 1024] = [0; 1024];
    let mut out: *mut passwd = core::ptr::null_mut();
    let e = unsafe {
        getpwuid_r(
            uid,
            core::ptr::addr_of_mut!(PW),
            core::ptr::addr_of_mut!(BUF) as *mut u8,
            1024,
            &mut out,
        )
    };
    if e != 0 { fail(e); }
    out
}

// grp.h //

pub use miku_abi::posix::group;

/// First `/etc/group` line `want` accepts, filled into `grp`. The member list
/// is a null-terminated pointer array at the front of `buf`, aligned for
/// pointers, with the strings after
fn group_lookup(
    want: &dyn Fn(&[u8], u32) -> bool,
    grp: *mut group,
    buf: *mut u8,
    buflen: usize,
    result: *mut *mut group,
) -> i32 {
    if grp.is_null() || result.is_null() || (buf.is_null() && buflen > 0) { return EINVAL; }
    unsafe { *result = core::ptr::null_mut() };
    let mut text = [0u8; 8192];
    let n = match read_db(b"/etc/group\0", &mut text) {
        Ok(n) => n,
        Err(e) => return e,
    };
    for f in records::<4>(&text[..n]) {
        let Some(gid) = num(f[2]) else { continue };
        if !want(f[0], gid) { continue; }
        let members = || f[3].split(|&b| b == b',').filter(|m| !m.is_empty());
        let count = members().count();
        let ptr = core::mem::size_of::<*mut u8>();
        let pad = (ptr - (buf as usize) % ptr) % ptr;
        let table = pad + (count + 1) * ptr;
        let strings: usize = f[0].len() + 1 + f[1].len() + 1 + members().map(|m| m.len() + 1).sum::<usize>();
        if table + strings > buflen { return ERANGE; }
        let mem = unsafe { buf.add(pad) } as *mut *mut u8;
        let mut pk = Packer { buf, at: table };
        for (i, m) in members().enumerate() {
            unsafe { *mem.add(i) = pk.put(m) };
        }
        unsafe {
            *mem.add(count) = core::ptr::null_mut();
            grp.write(group { gr_name: pk.put(f[0]).cast(), gr_passwd: pk.put(f[1]).cast(), gr_gid: gid, gr_mem: mem.cast() });
            *result = grp;
        }
        return 0;
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getgrgid_r(gid: u32, grp: *mut group, buf: *mut u8, buflen: usize, result: *mut *mut group) -> i32 {
    group_lookup(&|_, g| g == gid, grp, buf, buflen, result)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getgrnam_r(name: *const u8, grp: *mut group, buf: *mut u8, buflen: usize, result: *mut *mut group) -> i32 {
    if name.is_null() { return EINVAL; }
    let want = c_str(name);
    group_lookup(&|n, _| n == want, grp, buf, buflen, result)
}

/// `user`'s groups: `group` first, then every `/etc/group` line naming them.
/// `*ngroups` in is the room in `groups`, out the number; -1 if they did not
/// all fit
#[no_mangle]
#[inline(never)]
pub extern "C" fn getgrouplist(user: *const u8, group: u32, groups: *mut u32, ngroups: *mut i32) -> i32 {
    if user.is_null() || ngroups.is_null() { return fail(EINVAL); }
    let room = unsafe { *ngroups }.max(0) as usize;
    let mut found = 0usize;
    let mut add = |g: u32| {
        if found < room && !groups.is_null() {
            unsafe { *groups.add(found) = g };
        }
        found += 1;
    };
    add(group);
    let user = c_str(user);
    let mut text = [0u8; 8192];
    if let Ok(n) = read_db(b"/etc/group\0", &mut text) {
        for f in records::<4>(&text[..n]) {
            let Some(gid) = num(f[2]) else { continue };
            if gid != group && f[3].split(|&b| b == b',').any(|m| m == user) {
                add(gid);
            }
        }
    }
    unsafe { *ngroups = found as i32 };
    if found > room { -1 } else { found as i32 }
}

// unistd.h: the kernel's group list

/// The kernel keeps groups as 16 bits; POSIX passes `gid_t`
#[no_mangle]
#[inline(never)]
pub extern "C" fn getgroups(size: i32, list: *mut u32) -> i32 {
    if size < 0 { return fail(EINVAL); }
    let mut k = [0u16; MAX_GROUPS];
    let r = unsafe { crate::sys::sc2(crate::sys::SYS_GETGROUPS, k.as_mut_ptr() as u64, MAX_GROUPS as u64) };
    if r < 0 { return fail(-r as i32); }
    let n = r as usize;
    if size == 0 { return n as i32; }
    if (size as usize) < n || list.is_null() { return fail(EINVAL); }
    for (i, &g) in k[..n].iter().enumerate() {
        unsafe { *list.add(i) = g as u32 };
    }
    n as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn setgroups(size: usize, list: *const u32) -> i32 {
    if size > MAX_GROUPS || (size > 0 && list.is_null()) { return fail(EINVAL); }
    let mut k = [0u16; MAX_GROUPS];
    for i in 0..size {
        let g = unsafe { *list.add(i) };
        if g > u16::MAX as u32 { return fail(EINVAL); }
        k[i] = g as u16;
    }
    let r = unsafe { crate::sys::sc2(crate::sys::SYS_SETGROUPS, k.as_ptr() as u64, size as u64) };
    if r < 0 { fail(-r as i32) } else { 0 }
}

/// `user`'s groups from `/etc/group` plus `group`, as the supplementary list
#[no_mangle]
#[inline(never)]
pub extern "C" fn initgroups(user: *const u8, group: u32) -> i32 {
    let mut list = [0u32; MAX_GROUPS];
    let mut n = MAX_GROUPS as i32;
    // Cut to what the kernel holds, as glibc does with NGROUPS_MAX
    getgrouplist(user, group, list.as_mut_ptr(), &mut n);
    if n < 0 { return -1; }
    setgroups((n as usize).min(MAX_GROUPS), list.as_ptr())
}
