cfg_select! {
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
    target_os = "solid_asp3" => {
        mod solid;
        pub use solid::*;
    }
    target_os = "teeos" => {
        mod teeos;
        pub use teeos::*;
    }
    target_os = "uefi" => {
        mod uefi;
        pub use uefi::*;
    }
    all(target_family = "unix", not(target_os = "miku")) => {
        mod unix;
        pub use unix::*;
    }
    target_os = "wasi" => {
        mod wasi;
        pub use wasi::*;
    }
    target_os = "windows" => {
        mod windows;
        pub use windows::*;
    }
    target_os = "xous" => {
        mod xous;
        pub use xous::*;
    }
    any(
        target_os = "vexos",
        target_family = "wasm",
        target_os = "zkvm",
        target_os = "trusty",
    ) => {
        mod generic;
        pub use generic::*;
    }
}
