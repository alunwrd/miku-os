#![forbid(unsafe_op_in_unsafe_fn)]

cfg_select! {
    all(unix, not(target_os = "miku")) => {
        mod unix;
        pub use unix::{Pipe, pipe};
    }
    windows => {
        mod windows;
        pub use windows::{Pipe, pipe};
    }
    target_os = "motor" => {
        mod motor;
        pub use motor::{Pipe, pipe};
    }
    target_os = "miku" => {
        mod miku;
        pub use miku::{Pipe, pipe};
    }
    _ => {
        mod unsupported;
        pub use unsupported::{Pipe, pipe};
    }
}
