# lib/rust-std — Rust's `std` for MikuOS

The `x86_64-unknown-miku` target and the patch that makes `library/std` build
for it. Everything here is either a target spec, a file that does not exist
upstream, or a `cfg_select!` arm that tells upstream where MikuOS's version of
a concern lives.

## Why it is a patch and not a fork

Upstream stays a clean clone in `refsrc/miku_programs/rust`, pinned to the
commit of the toolchain in use, so it can be pulled forward rather than merged.
What MikuOS adds lives here, in the repository, where it is versioned with the
kernel it calls. The builder (`builder/src/stdport/overlay.rs`) puts the two
together into `target/rust-src-miku/` at build time; nothing is edited in
either source tree.

`std` internals are not a stable interface. The pin is deliberate: reading a
`library/` from a different nightly than the one that compiles it is how a port
acquires a bug that belongs to nobody.

## Building

```bash
cd builder
cargo run --release -- std stdapps      # or any crate directory below the root
cargo run --release -- std aosh --check # cargo check instead of build
cargo run --release -- zed build -p zed # Zed, from a patched copy of its clone
cargo run --release -- overlay          # only assemble target/rust-src-miku
```

which is what the image build does (`builder/src/stdport/`), and what anything
else should run too. It used to be `build-std.sh`, `overlay.sh` and `zed.sh`
and five Python scripts beside them; they are the builder's own code
now. Before anything else it brings back crate sources that cargo's automatic
cache clean has removed (`cargo fetch`, offline first). Besides
the overlays and `-Z build-std`, it passes the linker script and the link stubs
(`lib/miku/linker.ld`, `lib/miku/stubs/`) by absolute path. The target spec
used to carry them as `../lib/miku/...`, which the linker resolves against the
workspace being built; that tied every program to sitting one level below the
repository root, and Zed's copy (`target/zed-miku/src`) is three.

## What is in the patch

| Path | What it is |
|---|---|
| `x86_64-unknown-miku.json` | the target. `"os": "miku"` is the whole point of it - without it `target_os = "miku"` does not exist and nothing below can be written |
| `library/std/src/sys/pal/miku/abi.rs` | **the seam.** Every `libmiku` symbol the port calls, declared once |
| `library/std/src/sys/pal/miku/mod.rs` | `_start`, `init`, `abort_internal`, errno decoding |
| `library/std/src/sys/{alloc,args,io/error,random,time,thread,fd,fs,pipe}/miku.rs` | one file per concern |
| `library/std/src/sys/pal/miku/futex.rs` | thirty lines, and the reason `std::sync` needs none |
| `library/std/src/os/miku/` | `std::os::miku` - byte `OsStr` extensions, and deliberately not `std::os::unix` |
| `library/std/build.rs` | one line, so `std` is not marked `restricted_std` for this target |
| `library/std/src/sys/*/mod.rs` | one `cfg_select!` arm each |

## The seam

**The PAL calls `libmiku`, not the syscalls.** The ten `*_miku.so` libraries
already wrap all 113 of them, with one errno translation and one path
discipline; a second set of wrappers inside `std` would be two places that know
a syscall number. `dev/std-architecture.md` has the argument, and upstream's
own `target_os = "motor"` is the same shape.

The symbols are resolved from the link stubs at `lib/miku/stubs/`, which the
target spec names the way it names `linker.ld`. A program written against this
`std` therefore has an ordinary `Cargo.toml` with no MikuOS dependency in it,
which is what having a `std` is for.

## What works, and what does not

Built: `println!`, the allocator, `format!`, `String`/`Vec`, `std::env::args`
and `vars`, entropy, `std::process::exit`, panics. And `std::thread` with a
real `join`, `Mutex`, `Condvar`, `RwLock`, `Once`, the thread parker,
`thread_local!`, `Instant` and `SystemTime`.

`std::sync` is upstream's own `futex.rs` compiled unchanged - there is no
MikuOS synchronisation code anywhere in this directory, which is what syscall
110 was for.

And `std::fs` in full, `std::io::pipe`, `std::os::fd`,
`std::process::Command`, and `std::net` - IPv4 TCP and UDP.

**Nothing falls back to `unsupported`.** What is refused is refused by name and
for a reason: IPv6 (the stack has none), `bind` to port 0 (`bind` does not
choose a port and there is no `getsockname`), `set_nonblocking` on a socket
(sockets are not in the descriptor table, so `fcntl` cannot reach them),
`lookup_host` (DNS is `lib/net` in ring 3), file locking, `canonicalize`, and
`set_times`. Each returns an error rather than pretending.

## Two things that bite

**The link stubs are not rebuilt by anything you can see.** `lib/miku/stubs/`
is named by the target spec and written by `lib/miku`'s build script, and a
`std` program does not depend on that crate. The builder's `std` step builds
it first for exactly this reason. A `miku_*` function added to `libmiku` and missing from
the stub produced a *successful* link with the call absent from the binary when
LTO was on, which is why `stdapps` has LTO off.

**TLS is local-exec, and the spec says so.** Without it LLVM picks
general-dynamic for a PIE and the first `thread_local!` leaves
`__tls_get_addr` undefined - which `ld-miku` does not define and would refuse
to start the program over.
