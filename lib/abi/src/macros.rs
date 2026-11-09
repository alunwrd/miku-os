//! How a C struct is declared, the same as `libc`'s `s!`. Full paths, because
//! `libc` under `std` is `no_core`

macro_rules! c_structs {
    ($($(#[$attr:meta])* pub struct $name:ident { $($body:tt)* })*) => {
        $(
            #[repr(C)]
            #[::core::prelude::v1::derive(::core::clone::Clone, ::core::marker::Copy, ::core::fmt::Debug)]
            #[cfg_attr(
                feature = "extra_traits",
                ::core::prelude::v1::derive(::core::cmp::PartialEq, ::core::cmp::Eq, ::core::hash::Hash)
            )]
            $(#[$attr])*
            pub struct $name { $($body)* }
        )*
    };
}
