# lib/sdk/rust-libc — the `libc` crate for MikuOS

MikuOS declares `target_family = "unix"`, which is a statement about its
**C-level interface** and not about the kernel's design: `fork`/`execve`/
`wait4`, `open`/`read`/`write`/`lseek`, `poll`, `fcntl` and BSD sockets are all
there, and `libc_miku.so` exports them under their POSIX names on purpose. The
argument, both ways, is in `dev/std-architecture.md` under "The wall after
`std`".

Declaring it means the `libc` crate has to have something to say about this
target, because everything under `cfg(unix)` calls it - `rustix`, and through
it `polling`, `async-io` and `smol`.

Same shape as `lib/sdk/rust-std`: upstream stays a clean clone
(`refsrc/miku_programs/libc`), the patch lives here, and
the builder (`builder/src/stdport/overlay.rs`) assembles the two into
`target/rust-src-miku/libc-miku/` at build time; the three lines that select
the new modules are applied by anchor in `builder/src/stdport/arms.rs`. `library/Cargo.toml` in the overlaid tree points `[patch.crates-io]` at
it, which is how a `-Z build-std` build is told to use a `libc` other than the
registry's.

**`std` itself does not use this.** Every `cfg_select!` in `library/std/src/sys`
has MikuOS excluded from its unix arm, so the PAL in `lib/sdk/rust-std` is what
`std` compiles - see the `not(target_os = "miku")` guards there. This crate
exists for everything *else* that says `cfg(unix)`.

Types and constants are not here: they are `lib/abi`, which the overlay copies to
`src/unix/miku/abi/`. `miku/mod.rs` re-exports it and declares the functions, and
the builder checks those declarations against the definitions.
