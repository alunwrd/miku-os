//! `<sys/socket.h>`: the BSD calls, over syscalls 56-66, 77, and 80

use crate::libc::*;

// sys/socket.h: the BSD calls, over syscalls 56-66, 77, and 80
//
// The kernel's socket calls take BSD's arguments, a `sockaddr` and a length,
// told apart by the family in the first two bytes. Added here: the `SOCK_*`
// flags in the type word, the `MSG_*` flags, unchecked address lengths, and
// `sendmsg`/`recvmsg`, which are not syscalls.
pub(crate) use crate::sys::{sc1, sc2, sc3, sc4};

pub(crate) use crate::sys::{
    SYS_ACCEPT, SYS_BIND, SYS_CONNECT, SYS_LISTEN, SYS_RECV, SYS_RECVFROM, SYS_SEND,
    SYS_GETSOCKOPT, SYS_SENDTO, SYS_SETSOCKOPT, SYS_SOCKET, SYS_SOCKETPAIR,
};

pub(crate) use miku_abi::socket::AF_UNIX;

pub(crate) use miku_abi::socket::AF_INET;

pub(crate) use miku_abi::socket::SOCK_STREAM;

pub(crate) const SOCK_TYPE_FLAGS: i32 = O_NONBLOCK | O_CLOEXEC; // SOCK_NONBLOCK | SOCK_CLOEXEC

pub(crate) const IPPROTO_IP: i32 = 0;

pub(crate) use miku_abi::inet::IP_TTL;

pub(crate) use miku_abi::socket::MSG_PEEK;

pub(crate) use miku_abi::socket::MSG_DONTWAIT;

pub(crate) use miku_abi::socket::MSG_WAITALL;

pub(crate) use miku_abi::socket::MSG_NOSIGNAL;

pub(crate) use miku_abi::socket::MSG_CMSG_CLOEXEC;

pub(crate) use miku_abi::errno::EAFNOSUPPORT;

pub(crate) use miku_abi::errno::EPROTONOSUPPORT;

pub(crate) use miku_abi::errno::ENOPROTOOPT;

pub(crate) use miku_abi::errno::EOPNOTSUPP;

pub(crate) use miku_abi::errno::EMSGSIZE;

/// The 16-byte `sockaddr_in` the kernel reads and writes for `AF_INET`
pub(crate) const SOCKADDR_IN_LEN: u32 = 16;

/// `struct msghdr` as `libc` declares it for MikuOS, with POSIX's field types
pub use miku_abi::socket::msghdr;

#[no_mangle]
#[inline(never)]
pub extern "C" fn socket(domain: i32, ty: i32, protocol: i32) -> i32 {
    // The kernel matches the type exactly, so the two flags `socket` may carry
    // come off here and are applied as `fcntl` would: `O_NONBLOCK` and
    // `FD_CLOEXEC` on the socket's slot
    let flags = ty & SOCK_TYPE_FLAGS;
    let base = ty & !SOCK_TYPE_FLAGS;
    let fd = unsafe { sc3(SYS_SOCKET, domain as u64, base as u64, protocol as u64) };
    if fd < 0 { return fail(-fd as i32); }
    let r = apply_fd_flags(fd, flags);
    if r < 0 {
        file::miku_close(fd);
        return fail(-r as i32);
    }
    fd as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn connect(fd: i32, addr: *const u8, len: u32) -> i32 {
    // Non-blocking: `EINPROGRESS`, then `poll` for writable and `SO_ERROR` for
    // the outcome, as on any Unix. A local socket is writable once the listener
    // accepts. See `docs/MikuOS_ABI.md`
    ret(unsafe { sc3(SYS_CONNECT, fd as u64, addr as u64, len as u64) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn bind(fd: i32, addr: *const u8, len: u32) -> i32 {
    ret(unsafe { sc3(SYS_BIND, fd as u64, addr as u64, len as u64) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn listen(fd: i32, backlog: i32) -> i32 {
    ret(unsafe { sc2(SYS_LISTEN, fd as u64, backlog.max(0) as u64) }) as i32
}

/// The kernel writes a whole `sockaddr_in` for a TCP peer and nothing for a
/// local one, reading neither the caller's length nor its buffer size. So the
/// address lands in a local buffer, marked so "nothing written" is detectable,
/// and is copied out within the caller's room
pub(crate) unsafe fn accept_into(fd: i32, addr: *mut u8, lenp: *mut u32) -> i64 {
    const UNTOUCHED: u16 = 0xffff;
    let mut buf = [0u8; SOCKADDR_IN_LEN as usize];
    buf[0..2].copy_from_slice(&UNTOUCHED.to_le_bytes());
    let r = unsafe { sc3(SYS_ACCEPT, fd as u64, buf.as_mut_ptr() as u64, SOCKADDR_IN_LEN as u64) };
    if r < 0 { return r; }
    if !addr.is_null() && !lenp.is_null() {
        let room = unsafe { *lenp } as usize;
        let family = u16::from_le_bytes([buf[0], buf[1]]);
        let (src, len): (&[u8], u32) = if family == UNTOUCHED {
            // A local socket: unnamed, a bare family
            (&(AF_UNIX as u16).to_le_bytes()[..], 2)
        } else {
            (&buf[..], SOCKADDR_IN_LEN)
        };
        let n = room.min(src.len());
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), addr, n);
            *lenp = len;
        }
    }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn accept(fd: i32, addr: *mut u8, lenp: *mut u32) -> i32 {
    ret(unsafe { accept_into(fd, addr, lenp) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn accept4(fd: i32, addr: *mut u8, lenp: *mut u32, flags: i32) -> i32 {
    if flags & !SOCK_TYPE_FLAGS != 0 { return fail(EINVAL); }
    let r = unsafe { accept_into(fd, addr, lenp) };
    if r < 0 { return fail(-r as i32); }
    let f = apply_fd_flags(r, flags);
    if f < 0 {
        file::miku_close(r);
        return fail(-f as i32);
    }
    r as i32
}

/// `EOPNOTSUPP` both: no `getsockname` or `getpeername` syscall
#[no_mangle]
#[inline(never)]
pub extern "C" fn getsockname(_fd: i32, _addr: *mut u8, _lenp: *mut u32) -> i32 {
    fail(EOPNOTSUPP)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn getpeername(_fd: i32, _addr: *mut u8, _lenp: *mut u32) -> i32 {
    fail(EOPNOTSUPP)
}

/// `IP_TTL`, the one option of `setsockopt`(80), and `ENOPROTOOPT` for every
/// other
#[no_mangle]
#[inline(never)]
pub extern "C" fn setsockopt(fd: i32, level: i32, name: i32, val: *const u8, len: u32) -> i32 {
    if level == IPPROTO_IP && name == IP_TTL {
        if val.is_null() || len < 4 { return fail(EINVAL); }
        let ttl = unsafe { (val as *const i32).read_unaligned() };
        return ret(unsafe { sc4(SYS_SETSOCKOPT, fd as u64, level as u64, name as u64, ttl as u64) }) as i32;
    }
    // No Nagle in the kernel's TCP: every socket is already TCP_NODELAY, so
    // turning it off cannot be done
    if level == IPPROTO_TCP && name == TCP_NODELAY {
        if val.is_null() || len < 4 { return fail(EINVAL); }
        let on = unsafe { (val as *const i32).read_unaligned() } != 0;
        return if on { 0 } else { fail(ENOPROTOOPT) };
    }
    // Keepalive: the kernel's TCP probes a silent connection. One integer each,
    // by value, as `setsockopt`(80) takes them
    use miku_abi::socket::SO_KEEPALIVE;
    use miku_abi::inet::TCP_KEEPIDLE;
    use miku_abi::inet::TCP_KEEPCNT;
    if (level == SOL_SOCKET && name == SO_KEEPALIVE)
        || (level == IPPROTO_TCP && (TCP_KEEPIDLE..=TCP_KEEPCNT).contains(&name))
    {
        if val.is_null() || len < 4 { return fail(EINVAL); }
        let v = unsafe { (val as *const i32).read_unaligned() };
        if v < 0 { return fail(EINVAL); }
        return ret(unsafe { sc4(SYS_SETSOCKOPT, fd as u64, level as u64, name as u64, v as u64) }) as i32;
    }
    fail(ENOPROTOOPT)
}

pub(crate) const IPPROTO_TCP: i32 = 6;

pub(crate) use miku_abi::inet::TCP_NODELAY;

/// Through `getsockopt`(114), which answers with the value, since every option
/// it knows is one integer. `SO_ERROR` reads and clears the error of a
/// non-blocking connect, as `mio` and `async-io` use
#[no_mangle]
#[inline(never)]
pub extern "C" fn getsockopt(fd: i32, level: i32, name: i32, val: *mut u8, lenp: *mut u32) -> i32 {
    if val.is_null() || lenp.is_null() || unsafe { *lenp } < 4 { return fail(EINVAL); }
    if level == SOL_SOCKET && name == SO_PEERCRED {
        return peercred_opt(fd, val, lenp);
    }
    if level == IPPROTO_TCP && name == TCP_NODELAY {
        unsafe {
            (val as *mut i32).write_unaligned(1);
            *lenp = 4;
        }
        return 0;
    }
    let r = unsafe { sc3(SYS_GETSOCKOPT, fd as u64, level as u64, name as u64) };
    if r < 0 { return fail(-r as i32); }
    unsafe {
        (val as *mut i32).write_unaligned(r as i32);
        *lenp = 4;
    }
    0
}

pub(crate) use miku_abi::socket::SOL_SOCKET;

pub(crate) use miku_abi::socket::SO_PEERCRED;

/// `SO_PEERCRED`, from `peercred`(83): pid, then the peer's effective uid and
/// gid at connect, as Linux reports. The kernel's record also has real ids and
/// a start tick
pub(crate) fn peercred_opt(fd: i32, val: *mut u8, lenp: *mut u32) -> i32 {
    if unsafe { *lenp } < 12 { return fail(EINVAL); }
    let mut rec = miku_abi::cred::MikuPeerCred::default();
    let r = proc::miku_peercred(fd as u64, &mut rec as *mut _ as *mut u8);
    if r < 0 { return fail(-r as i32); }
    unsafe {
        (val as *mut i32).write_unaligned(rec.pid as i32);
        (val.add(4) as *mut u32).write_unaligned(rec.euid);
        (val.add(8) as *mut u32).write_unaligned(rec.egid);
        *lenp = 12;
    }
    0
}

/// `EOPNOTSUPP`: half-close is not in the syscall table
#[no_mangle]
#[inline(never)]
pub extern "C" fn shutdown(_fd: i32, _how: i32) -> i32 {
    fail(EOPNOTSUPP)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn socketpair(domain: i32, ty: i32, protocol: i32, sv: *mut i32) -> i32 {
    if sv.is_null() { return fail(EFAULT); }
    if domain != AF_UNIX { return fail(EAFNOSUPPORT); }
    let flags = ty & SOCK_TYPE_FLAGS;
    // `socketpair`(77) makes local streams and nothing else
    if ty & !SOCK_TYPE_FLAGS != SOCK_STREAM || protocol != 0 { return fail(EPROTONOSUPPORT); }
    let mut fds = [0u64; 2];
    let r = unsafe { sc1(SYS_SOCKETPAIR, fds.as_mut_ptr() as u64) };
    if r < 0 { return fail(-r as i32); }
    for &fd in &fds {
        let f = apply_fd_flags(fd as i64, flags);
        if f < 0 {
            file::miku_close(fds[0] as i64);
            file::miku_close(fds[1] as i64);
            return fail(-f as i32);
        }
    }
    unsafe {
        *sv = fds[0] as i32;
        *sv.add(1) = fds[1] as i32;
    }
    0
}

/// `MSG_*` flags `send`(58) and `recv`(59) do not read, handled here or
/// refused. `MSG_NOSIGNAL` is already true: only a pipe raises `SIGPIPE`, and a
/// socket answers `EPIPE`. `MSG_DONTWAIT` makes the descriptor non-blocking for
/// one call, which another thread on the socket can observe
pub(crate) fn with_msg_flags(fd: i32, flags: i32, allowed: i32, f: impl FnOnce() -> i64) -> i64 {
    if flags & !allowed != 0 { return -(EOPNOTSUPP as i64); }
    if flags & MSG_DONTWAIT == 0 { return f(); }
    let cur = file::miku_fcntl(fd as i64, F_GETFL as u64, 0);
    if cur < 0 { return cur; }
    if cur & O_NONBLOCK as i64 != 0 { return f(); }
    let s = file::miku_set_nonblocking(fd as i64, true);
    if s < 0 { return s; }
    let r = f();
    file::miku_set_nonblocking(fd as i64, false);
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn send(fd: i32, buf: *const u8, len: usize, flags: i32) -> i64 {
    let allowed = MSG_DONTWAIT | MSG_NOSIGNAL;
    ret(with_msg_flags(fd, flags, allowed, || unsafe {
        sc4(SYS_SEND, fd as u64, buf as u64, len as u64, 0)
    }))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> i64 {
    // `MSG_PEEK` is refused: there is no way to read without taking
    let allowed = MSG_DONTWAIT | MSG_WAITALL;
    ret(with_msg_flags(fd, flags, allowed, || {
        if flags & MSG_WAITALL == 0 {
            return unsafe { sc4(SYS_RECV, fd as u64, buf as u64, len as u64, 0) };
        }
        // All of it, or end-of-stream, or an error after nothing
        let mut got = 0usize;
        while got < len {
            let r = unsafe { sc4(SYS_RECV, fd as u64, buf.add(got) as u64, (len - got) as u64, 0) };
            if r < 0 { return if got > 0 { got as i64 } else { r }; }
            if r == 0 { break; }
            got += r as usize;
        }
        got as i64
    }))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sendto(
    fd: i32, buf: *const u8, len: usize, flags: i32, addr: *const u8, alen: u32,
) -> i64 {
    if addr.is_null() { return send(fd, buf, len, flags); }
    // `sendto`(65) reads a fixed 16-byte `sockaddr_in` with no length, so the
    // length and family are checked here
    if alen < SOCKADDR_IN_LEN { return fail(EINVAL) as i64; }
    let family = unsafe { (addr as *const u16).read_unaligned() } as i32;
    if family != AF_INET { return fail(EAFNOSUPPORT) as i64; }
    let allowed = MSG_DONTWAIT | MSG_NOSIGNAL;
    ret(with_msg_flags(fd, flags, allowed, || unsafe {
        sc4(SYS_SENDTO, fd as u64, buf as u64, len as u64, addr as u64)
    }))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn recvfrom(
    fd: i32, buf: *mut u8, len: usize, flags: i32, addr: *mut u8, lenp: *mut u32,
) -> i64 {
    // The kernel writes a whole `sockaddr_in` given somewhere to put it; here,
    // then as much as the caller has room for
    let mut sa = [0u8; SOCKADDR_IN_LEN as usize];
    let allowed = MSG_DONTWAIT;
    let r = with_msg_flags(fd, flags, allowed, || unsafe {
        sc4(SYS_RECVFROM, fd as u64, buf as u64, len as u64, sa.as_mut_ptr() as u64)
    });
    if r < 0 { return ret(r); }
    if !addr.is_null() && !lenp.is_null() {
        let n = (unsafe { *lenp } as usize).min(sa.len());
        unsafe {
            core::ptr::copy_nonoverlapping(sa.as_ptr(), addr, n);
            *lenp = SOCKADDR_IN_LEN;
        }
    }
    r
}

/// The iovecs of a `msghdr`, as a slice
pub(crate) unsafe fn msg_iovs<'a>(m: &msghdr) -> Result<&'a [iovec], i32> {
    if m.msg_iovlen < 0 || m.msg_iovlen > 1024 { return Err(EINVAL); }
    if m.msg_iovlen == 0 { return Ok(&[]); }
    if m.msg_iov.is_null() { return Err(EFAULT); }
    Ok(unsafe { core::slice::from_raw_parts(m.msg_iov, m.msg_iovlen as usize) })
}

/// One `send` of all segments gathered in one buffer, so a datagram stays one
/// datagram. Ancillary data is refused: descriptors pass with `sendfd`(84), one
/// per call, not `SCM_RIGHTS`
#[no_mangle]
#[inline(never)]
pub extern "C" fn sendmsg(fd: i32, msg: *const msghdr, flags: i32) -> i64 {
    if msg.is_null() { return fail(EFAULT) as i64; }
    let m = unsafe { &*msg };
    if !m.msg_control.is_null() && m.msg_controllen > 0 { return fail(EOPNOTSUPP) as i64; }
    let iovs = match unsafe { msg_iovs(m) } {
        Ok(v) => v,
        Err(e) => return fail(e) as i64,
    };
    let total: usize = iovs.iter().map(|v| v.iov_len).sum();
    if total > 1 << 20 { return fail(EMSGSIZE) as i64; }
    let one = iovs.iter().filter(|v| v.iov_len > 0).count() <= 1;
    let (ptr, owned) = if one {
        let p = iovs.iter().find(|v| v.iov_len > 0).map(|v| v.iov_base.cast::<u8>()).unwrap_or(core::ptr::null_mut());
        (p, false)
    } else {
        let p = heap::miku_malloc(total.max(1));
        if p.is_null() { return fail(ENOMEM) as i64; }
        let mut off = 0;
        for v in iovs {
            unsafe { core::ptr::copy_nonoverlapping(v.iov_base.cast::<u8>(), p.add(off), v.iov_len) };
            off += v.iov_len;
        }
        (p, true)
    };
    let r = if m.msg_name.is_null() {
        send(fd, ptr, total, flags)
    } else {
        sendto(fd, ptr, total, flags, m.msg_name.cast(), m.msg_namelen)
    };
    if owned { heap::miku_free(ptr); }
    r
}

/// One `recv` into the segments, scattered from one buffer. No ancillary data
/// arrives, so a control buffer comes back empty
#[no_mangle]
#[inline(never)]
pub extern "C" fn recvmsg(fd: i32, msg: *mut msghdr, flags: i32) -> i64 {
    if msg.is_null() { return fail(EFAULT) as i64; }
    let m = unsafe { &mut *msg };
    let flags = flags & !MSG_CMSG_CLOEXEC;
    let iovs = match unsafe { msg_iovs(m) } {
        Ok(v) => v,
        Err(e) => return fail(e) as i64,
    };
    let total: usize = iovs.iter().map(|v| v.iov_len).sum();
    let nonempty = iovs.iter().filter(|v| v.iov_len > 0).count();
    let (ptr, owned) = if nonempty <= 1 {
        let p = iovs.iter().find(|v| v.iov_len > 0).map(|v| v.iov_base.cast::<u8>()).unwrap_or(core::ptr::null_mut());
        (p, false)
    } else {
        let p = heap::miku_malloc(total.max(1));
        if p.is_null() { return fail(ENOMEM) as i64; }
        (p, true)
    };
    let r = if m.msg_name.is_null() {
        recv(fd, ptr, total, flags)
    } else {
        recvfrom(fd, ptr, total, flags, m.msg_name.cast(), &mut m.msg_namelen)
    };
    if owned {
        if r > 0 {
            let mut left = r as usize;
            let mut off = 0;
            for v in iovs {
                let n = left.min(v.iov_len);
                unsafe { core::ptr::copy_nonoverlapping(ptr.add(off), v.iov_base.cast::<u8>(), n) };
                off += n;
                left -= n;
                if left == 0 { break; }
            }
        }
        heap::miku_free(ptr);
    }
    if r >= 0 {
        m.msg_controllen = 0;
        m.msg_flags = 0;
    }
    r
}
