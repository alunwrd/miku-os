//! MikuOS's `std::os::unix`.
//!
//! The target declares `target_family = "unix"`, so crates written for a Unix
//! compile and expect this module.
//!
//! `ffi` (an `OsStr` is bytes), `io` (descriptors are `RawFd`s), `fs`
//! (`symlink`, `PermissionsExt`, and the extension traits `DirEntryExt`,
//! `FileExt`, `FileTypeExt`, `MetadataExt`, `OpenOptionsExt`), `net`
//! (`os/miku/net.rs`), `process` (`CommandExt`, `ExitStatusExt`), `thread`
//! (`JoinHandleExt`).
//!
//! `MetadataExt` is a translation: MikuOS's `stat` is 64 bytes of its own
//! shape, and the fields it lacks (`dev`, `blksize`, every `*_nsec`) are
//! documented on the trait.

#![stable(feature = "rust1", since = "1.0.0")]
// See the note in `os/miku/fs.rs`: `std::os::unix` is stable everywhere else,
// and a crate on stable Rust cannot name an unstable module.

#[stable(feature = "rust1", since = "1.0.0")]
pub mod ffi {
    //! Unix-specific extensions to primitives in [`std::ffi`].
    //!
    //! [`std::ffi`]: crate::ffi
    //!
    //! An `OsStr` on MikuOS is bytes: a filesystem name is up to 255 bytes and
    //! need not be UTF-8.
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use crate::os::miku::ffi::{OsStrExt, OsStringExt};
}

#[stable(feature = "rust1", since = "1.0.0")]
pub mod io {
    //! Unix-specific extensions to general I/O primitives.
    //!
    //! The descriptors `std::os::fd` describes: small integers in two ranges,
    //! files from 0 and sockets from `SOCK_FD_BASE`. The `os::fd` traits cover
    //! both.
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use crate::os::fd::*;
}

#[stable(feature = "rust1", since = "1.0.0")]
pub mod fs {
    //! Unix-specific extensions to primitives in [`std::fs`].
    //!
    //! [`std::fs`]: crate::fs
    //!
    //! The extension traits `rustix` asks for, and through it `polling`,
    //! `async-io` and `smol`; `DirBuilderExt` is for `async-fs`. Where MikuOS
    //! has nothing, the doc comment on the trait says so.
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use crate::os::miku::fs::{
        DirBuilderExt, DirEntryExt, FileExt, FileTypeExt, MetadataExt, OpenOptionsExt,
        PermissionsExt, symlink,
    };
}

#[stable(feature = "unix_socket", since = "1.10.0")]
pub mod net {
    //! Local sockets. MikuOS's `AF_UNIX` is a stream named by a path; see
    //! `std::os::miku::net` for what that leaves out.
    #[stable(feature = "unix_socket", since = "1.10.0")]
    pub use crate::os::miku::net::*;
}

#[stable(feature = "rust1", since = "1.0.0")]
pub mod process {
    //! Unix-specific extensions to primitives in [`std::process`].
    //!
    //! [`std::process`]: crate::process
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use crate::os::miku::process::{CommandExt, ExitStatusExt};
}

#[stable(feature = "thread_extensions", since = "1.9.0")]
pub mod thread {
    //! Unix-specific extensions to primitives in [`std::thread`]. The raw
    //! handle is the kernel thread id; see `std::os::miku::thread`.
    //!
    //! [`std::thread`]: crate::thread
    #[stable(feature = "thread_extensions", since = "1.9.0")]
    pub use crate::os::miku::thread::{JoinHandleExt, RawPthread};
}

#[stable(feature = "rust1", since = "1.0.0")]
pub mod prelude {
    //! A prelude for conveniently writing platform-specific code.
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::ffi::{OsStrExt, OsStringExt};
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::fs::{
        DirEntryExt, FileExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    #[doc(no_inline)]
    #[stable(feature = "rust1", since = "1.0.0")]
    pub use super::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
}
