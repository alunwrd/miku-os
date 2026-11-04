//! Platform-dependent file descriptor abstraction.

#![forbid(unsafe_op_in_unsafe_fn)]

cfg_select! {
    any(all(target_family = "unix", not(target_os = "miku")), target_os = "wasi") => {
        mod unix;
        pub use unix::*;
    }
    target_os = "hermit" => {
        mod hermit;
        pub use hermit::*;
    }
    target_os = "motor" => {
        mod motor;
        pub use motor::*;
    }
    target_os = "miku" => {
        mod miku;
        pub use miku::*;
    }
    all(target_vendor = "fortanix", target_env = "sgx") => {
        mod sgx;
        pub use sgx::*;
    }
    _ => {}
}
