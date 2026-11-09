//! `<ucontext.h>`: what a `SA_SIGINFO` handler gets

#![allow(unused_imports)]

use super::prelude::*;

pub type greg_t = i64;
pub type gregset_t = [greg_t; 23];
pub const REG_R8: c_int = 0;
pub const REG_R9: c_int = 1;
pub const REG_R10: c_int = 2;
pub const REG_R11: c_int = 3;
pub const REG_R12: c_int = 4;
pub const REG_R13: c_int = 5;
pub const REG_R14: c_int = 6;
pub const REG_R15: c_int = 7;
pub const REG_RDI: c_int = 8;
pub const REG_RSI: c_int = 9;
pub const REG_RBP: c_int = 10;
pub const REG_RBX: c_int = 11;
pub const REG_RDX: c_int = 12;
pub const REG_RAX: c_int = 13;
pub const REG_RCX: c_int = 14;
pub const REG_RSP: c_int = 15;
pub const REG_RIP: c_int = 16;
pub const REG_EFL: c_int = 17;
pub const REG_CSGSFS: c_int = 18;
pub const REG_ERR: c_int = 19;
pub const REG_TRAPNO: c_int = 20;
pub const REG_OLDMASK: c_int = 21;
pub const REG_CR2: c_int = 22;
c_structs! {
    /// The machine context a signal handler gets, Linux's layout: the general
    /// registers indexed by `REG_*`, then a pointer to the FPU image.
    /// `sigreturn` restores `gregs` as written, RIP and RSP included
    /// (`dev/signal-architecture.md`)
    pub struct mcontext_t {
        pub gregs: gregset_t,
        pub fpregs: *mut c_void,
        __reserved1: [c_ulonglong; 8],
    }

    /// The third argument of a `SA_SIGINFO` handler, byte for byte Linux's up
    /// to the end of `uc_sigmask`. `libmiku`'s entry stub saves and restores
    /// the FPU image `uc_mcontext.fpregs` points at
    pub struct ucontext_t {
        pub uc_flags: c_ulong,
        pub uc_link: *mut ucontext_t,
        pub uc_stack: stack_t,
        pub uc_mcontext: mcontext_t,
        pub uc_sigmask: sigset_t,
    }
}
