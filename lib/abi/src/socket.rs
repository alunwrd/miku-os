//! `<sys/socket.h>`

#![allow(unused_imports)]

use super::prelude::*;

pub const AF_UNIX: c_int = 1;
pub const AF_INET: c_int = 2;
pub const SOCK_STREAM: c_int = 1;
pub const SOCK_DGRAM: c_int = 2;
// Sockets, as `rustix::net` enumerates them
//
// Linux's numbers; `rustix` names every family and option in one `match`. The
// kernel's `net/socket.rs` implements `AF_INET` stream and datagram, raw ICMP,
// and `AF_UNIX` stream; one option to set (`IP_TTL`) and four to read
// (`SO_ERROR`, `SO_TYPE`, `SO_ACCEPTCONN`, `IP_TTL`, through
// `getsockopt`(114)). The rest answer `EAFNOSUPPORT`, `EPROTONOSUPPORT` or
// `ENOPROTOOPT`.
pub const AF_UNSPEC: c_int = 0;
pub const AF_AX25: c_int = 3;
pub const AF_IPX: c_int = 4;
pub const AF_APPLETALK: c_int = 5;
pub const AF_NETROM: c_int = 6;
pub const AF_BRIDGE: c_int = 7;
pub const AF_ATMPVC: c_int = 8;
pub const AF_X25: c_int = 9;
pub const AF_INET6: c_int = 10;
pub const AF_ROSE: c_int = 11;
pub const AF_NETBEUI: c_int = 13;
pub const AF_SECURITY: c_int = 14;
pub const AF_KEY: c_int = 15;
pub const AF_NETLINK: c_int = 16;
pub const AF_PACKET: c_int = 17;
pub const AF_ASH: c_int = 18;
pub const AF_ECONET: c_int = 19;
pub const AF_ATMSVC: c_int = 20;
pub const AF_RDS: c_int = 21;
pub const AF_SNA: c_int = 22;
pub const AF_IRDA: c_int = 23;
pub const AF_PPPOX: c_int = 24;
pub const AF_WANPIPE: c_int = 25;
pub const AF_LLC: c_int = 26;
pub const AF_CAN: c_int = 29;
pub const AF_TIPC: c_int = 30;
pub const AF_BLUETOOTH: c_int = 31;
pub const AF_IUCV: c_int = 32;
pub const AF_RXRPC: c_int = 33;
pub const AF_ISDN: c_int = 34;
pub const AF_PHONET: c_int = 35;
pub const AF_IEEE802154: c_int = 36;
pub const SOCK_RAW: c_int = 3;
pub const SOCK_RDM: c_int = 4;
pub const SOCK_SEQPACKET: c_int = 5;
/// The two flags `socket` and `accept4` take in the type word: `O_NONBLOCK`'s
/// and `O_CLOEXEC`'s values, as on Linux. `libc_miku.so` strips them and
/// applies them with `fcntl`, since `socket`(56) matches the type exactly
pub const SOCK_NONBLOCK: c_int = O_NONBLOCK;
pub const SOCK_CLOEXEC: c_int = O_CLOEXEC;
pub const SOL_SOCKET: c_int = 1;
pub const SO_REUSEADDR: c_int = 2;
pub const SO_TYPE: c_int = 3;
pub const SO_ERROR: c_int = 4;
pub const SO_BROADCAST: c_int = 6;
pub const SO_SNDBUF: c_int = 7;
pub const SO_RCVBUF: c_int = 8;
pub const SO_KEEPALIVE: c_int = 9;
pub const SO_OOBINLINE: c_int = 10;
pub const SO_LINGER: c_int = 13;
/// Who is at the other end of a local socket, from `peercred`(83), recorded
/// when the connection was made
pub const SO_PEERCRED: c_int = 17;
pub const SO_REUSEPORT: c_int = 15;
pub const SO_RCVTIMEO: c_int = 20;
pub const SO_SNDTIMEO: c_int = 21;
pub const SO_ACCEPTCONN: c_int = 30;
pub const SO_DOMAIN: c_int = 39;
/// `send`/`recv` flags. `send`(58) and `recv`(59) read none of their flags
/// word; `libc_miku.so` handles `MSG_DONTWAIT` and `MSG_NOSIGNAL` and answers
/// `EOPNOTSUPP` for the rest
pub const MSG_OOB: c_int = 0x1;
pub const MSG_PEEK: c_int = 0x2;
pub const MSG_DONTROUTE: c_int = 0x4;
pub const MSG_CTRUNC: c_int = 0x8;
pub const MSG_TRUNC: c_int = 0x20;
pub const MSG_DONTWAIT: c_int = 0x40;
pub const MSG_EOR: c_int = 0x80;
pub const MSG_WAITALL: c_int = 0x100;
pub const MSG_CONFIRM: c_int = 0x800;
pub const MSG_ERRQUEUE: c_int = 0x2000;
pub const MSG_NOSIGNAL: c_int = 0x4000;
pub const MSG_MORE: c_int = 0x8000;
pub const MSG_CMSG_CLOEXEC: c_int = 0x4000_0000;
/// What `listen` is told for "as many as the system allows". The kernel reads
/// no backlog
pub const SOMAXCONN: c_int = 128;
pub const SHUT_RD: c_int = 0;
pub const SHUT_WR: c_int = 1;
pub const SHUT_RDWR: c_int = 2;
/// Descriptor passing is `sendfd`(84)/`recvfd`(85), one descriptor per call
/// over an `AF_UNIX` socket, not ancillary data, so `sendmsg`/`recvmsg` with a
/// control buffer answer `EOPNOTSUPP`
pub const SCM_RIGHTS: c_int = 1;
// Below: for Rust only, not in the C headers
/// The display protocol's transport. Not a network family: a connection to the
/// compositor is a descriptor with an owner and a lifetime tied to the process
pub const AF_MAYLAND: c_int = 42;
pub type socklen_t = u32;
pub type sa_family_t = u16;
pub const AF_DECnet: c_int = 12;
const fn CMSG_ALIGN(len: usize) -> usize {
    (len + core::mem::size_of::<usize>() - 1) & !(core::mem::size_of::<usize>() - 1)
}
/// The control-message walkers, as arithmetic, matching Linux's layout above.
/// MikuOS never fills a control buffer
pub unsafe fn CMSG_FIRSTHDR(mhdr: *const msghdr) -> *mut cmsghdr {
    if (*mhdr).msg_controllen as usize >= core::mem::size_of::<cmsghdr>() {
        (*mhdr).msg_control.cast()
    } else {
        core::ptr::null_mut()
    }
}
pub unsafe fn CMSG_NXTHDR(mhdr: *const msghdr, cmsg: *const cmsghdr) -> *mut cmsghdr {
    if ((*cmsg).cmsg_len as usize) < core::mem::size_of::<cmsghdr>() {
        return core::ptr::null_mut();
    }
    let next = (cmsg as usize + CMSG_ALIGN((*cmsg).cmsg_len as usize)) as *mut cmsghdr;
    let max = (*mhdr).msg_control as usize + (*mhdr).msg_controllen as usize;
    if (next.wrapping_offset(1)) as usize > max
        || next as usize + CMSG_ALIGN((*next).cmsg_len as usize) > max
    {
        core::ptr::null_mut()
    } else {
        next
    }
}
pub unsafe fn CMSG_DATA(cmsg: *const cmsghdr) -> *mut c_uchar {
    cmsg.offset(1) as *mut c_uchar
}
pub const unsafe fn CMSG_SPACE(length: c_uint) -> c_uint {
    (CMSG_ALIGN(length as usize) + CMSG_ALIGN(core::mem::size_of::<cmsghdr>())) as c_uint
}
pub const unsafe fn CMSG_LEN(length: c_uint) -> c_uint {
    CMSG_ALIGN(core::mem::size_of::<cmsghdr>()) as c_uint + length
}
c_structs! {
    pub struct sockaddr {
        pub sa_family: sa_family_t,
        pub sa_data: [c_char; 14],
    }

    /// The form `socket`(56)'s `AF_UNIX` takes, byte for byte. See
    /// `docs/MikuOS_ABI.md` §3.4
    pub struct sockaddr_un {
        pub sun_family: sa_family_t,
        pub sun_path: [c_char; 108],
    }

    /// Room for any address, 128 bytes aligned to eight: Linux's size
    pub struct sockaddr_storage {
        pub ss_family: sa_family_t,
        __ss_pad2: [u8; 118],
        __ss_align: usize,
    }

    /// `SO_PEERCRED`'s answer, in Linux's shape: the peer's pid and its
    /// effective ids at the moment it connected, as `peercred`(83) keeps them
    pub struct ucred {
        pub pid: pid_t,
        pub uid: uid_t,
        pub gid: gid_t,
    }

    /// POSIX's field types: `msg_iovlen` is an `int`, as in musl and the BSDs,
    /// not glibc's `size_t`. `rustix` assumes this for a system it does not
    /// list
    pub struct msghdr {
        pub msg_name: *mut c_void,
        pub msg_namelen: socklen_t,
        pub msg_iov: *mut iovec,
        pub msg_iovlen: c_int,
        pub msg_control: *mut c_void,
        pub msg_controllen: size_t,
        pub msg_flags: c_int,
    }

    pub struct cmsghdr {
        pub cmsg_len: size_t,
        pub cmsg_level: c_int,
        pub cmsg_type: c_int,
    }
}
