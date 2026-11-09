//! `<errno.h>`, Linux's numbers. `EWOULDBLOCK` is `EAGAIN`, and `ENOTSUP` is `EOPNOTSUPP`
//!
//! A name here is no promise that the kernel returns it: crates name constants
//! the system never produces, and `rustix`'s `Errno` lists the whole table.

use super::prelude::*;

/// The table twice: the positive `c_int` C reads from `errno`, and in `neg` the
/// negative `i64` a system call returns
macro_rules! table {
    ($($name:ident = $value:expr;)*) => {
        $(pub const $name: c_int = $value;)*

        /// As a system call returns them
        pub mod neg {
            $(pub const $name: i64 = -(super::$name as i64);)*
        }
    };
}

table! {
    // The errno numbers
    //
    // Linux's values, the same as `kernel/syscall/errno.rs`. `EWOULDBLOCK` is
    // `EAGAIN`, and `ENOTSUP` is `EOPNOTSUPP`.
    EPERM = 1;
    ENOENT = 2;
    ESRCH = 3;
    EINTR = 4;
    EIO = 5;
    ENXIO = 6;
    E2BIG = 7;
    ENOEXEC = 8;
    EBADF = 9;
    ECHILD = 10;
    EAGAIN = 11;
    EWOULDBLOCK = EAGAIN;
    ENOMEM = 12;
    EACCES = 13;
    EFAULT = 14;
    EBUSY = 16;
    EEXIST = 17;
    EXDEV = 18;
    ENODEV = 19;
    ENOTDIR = 20;
    EISDIR = 21;
    EINVAL = 22;
    ENFILE = 23;
    EMFILE = 24;
    ENOTTY = 25;
    EFBIG = 27;
    ENOSPC = 28;
    ESPIPE = 29;
    EROFS = 30;
    EMLINK = 31;
    EPIPE = 32;
    EDOM = 33;
    ERANGE = 34;
    ENAMETOOLONG = 36;
    ENOSYS = 38;
    ENOTEMPTY = 39;
    ELOOP = 40;
    EOPNOTSUPP = 95;
    ENOTSUP = EOPNOTSUPP;
    EADDRINUSE = 98;
    EADDRNOTAVAIL = 99;
    ECONNABORTED = 103;
    ECONNRESET = 104;
    ENOTCONN = 107;
    ETIMEDOUT = 110;
    ECONNREFUSED = 111;
    // The rest of the errno table
    //
    // Linux's numbers, the same space as `kernel/syscall/errno.rs`. Declared in
    // full because crates name constants the system never produces: `rustix`'s
    // `Errno` enumerates the whole table in one `match`.
    //
    // A name here is no promise that the kernel returns it; see
    // `kernel/syscall/errno.rs`.
    ENOTBLK = 15;
    ETXTBSY = 26;
    EDEADLK = 35;
    EDEADLOCK = EDEADLK;
    ENOLCK = 37;
    ENOMSG = 42;
    EIDRM = 43;
    ECHRNG = 44;
    EL2NSYNC = 45;
    EL3HLT = 46;
    EL3RST = 47;
    ELNRNG = 48;
    EUNATCH = 49;
    ENOCSI = 50;
    EL2HLT = 51;
    EBADE = 52;
    EBADR = 53;
    EXFULL = 54;
    ENOANO = 55;
    EBADRQC = 56;
    EBADSLT = 57;
    EBFONT = 59;
    ENOSTR = 60;
    ENODATA = 61;
    ETIME = 62;
    ENOSR = 63;
    ENONET = 64;
    ENOPKG = 65;
    EREMOTE = 66;
    ENOLINK = 67;
    EADV = 68;
    ESRMNT = 69;
    ECOMM = 70;
    EPROTO = 71;
    EMULTIHOP = 72;
    EDOTDOT = 73;
    EBADMSG = 74;
    EOVERFLOW = 75;
    ENOTUNIQ = 76;
    EBADFD = 77;
    EREMCHG = 78;
    ELIBACC = 79;
    ELIBBAD = 80;
    ELIBSCN = 81;
    ELIBMAX = 82;
    ELIBEXEC = 83;
    EILSEQ = 84;
    ERESTART = 85;
    ESTRPIPE = 86;
    EUSERS = 87;
    ENOTSOCK = 88;
    EDESTADDRREQ = 89;
    EMSGSIZE = 90;
    EPROTOTYPE = 91;
    ENOPROTOOPT = 92;
    EPROTONOSUPPORT = 93;
    ESOCKTNOSUPPORT = 94;
    EPFNOSUPPORT = 96;
    EAFNOSUPPORT = 97;
    ENETDOWN = 100;
    ENETUNREACH = 101;
    ENETRESET = 102;
    ENOBUFS = 105;
    EISCONN = 106;
    ESHUTDOWN = 108;
    ETOOMANYREFS = 109;
    EALREADY = 114;
    EINPROGRESS = 115;
    ESTALE = 116;
    EUCLEAN = 117;
    ENOTNAM = 118;
    ENAVAIL = 119;
    EISNAM = 120;
    EREMOTEIO = 121;
    EDQUOT = 122;
    ENOMEDIUM = 123;
    EMEDIUMTYPE = 124;
    ECANCELED = 125;
    ENOKEY = 126;
    EKEYEXPIRED = 127;
    EKEYREVOKED = 128;
    EKEYREJECTED = 129;
    EOWNERDEAD = 130;
    ENOTRECOVERABLE = 131;
    ERFKILL = 132;
    EHWPOISON = 133;
    // BSD's and Mach's errno names, which Hurd inherits and `nix` lists for it;
    // MikuOS sits beside Hurd there. The kernel never returns them, and they are
    // numbered above anything it produces
    EAUTH = 1001;
    EBACKGROUND = 1002;
    EBADRPC = 1003;
    EDIED = 1004;
    EFTYPE = 1005;
    EGRATUITOUS = 1006;
    EGREGIOUS = 1007;
    EIEIO = 1008;
    ENEEDAUTH = 1009;
    EPROCLIM = 1010;
    EPROCUNAVAIL = 1011;
    EPROGMISMATCH = 1012;
    EPROGUNAVAIL = 1013;
    ERPCMISMATCH = 1014;
    EHOSTDOWN = 112;
    EHOSTUNREACH = 113;
}
