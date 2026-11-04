# Writing programs for MikuOS

The developer's guide to ring 3: the three ways to write a MikuOS program
(no_std Rust over libmiku, Rust `std`, and C through `libc_miku`), a working
example of each, what the libraries offer, how dynamic linking works from a
program's side, how a program is built and gets into `/bin`, which C libraries
are already ported, how Zed was brought up, how to call a system call no
library wraps, and how to debug a program. The kernel interface underneath is
[the ABI reference](../MikuOS_ABI.md); processes, signals and threads as a
kernel subsystem are [processes.md](processes.md).

## Contents

- [The three ways](#the-three-ways)
- [1. no_std Rust with libmiku](#1-no_std-rust-with-libmiku)
- [2. Rust std](#2-rust-std)
- [3. C through libc_miku](#3-c-through-libc_miku)
- [The miku libraries](#the-miku-libraries)
- [libmiku by module](#libmiku-by-module)
- [libc_miku](#libc_miku)
- [Dynamic linking](#dynamic-linking)
- [Building and installing a program](#building-and-installing-a-program)
- [Ported C dependencies](#ported-c-dependencies)
- [Zed: the big example](#zed-the-big-example)
- [Calling a system call directly](#calling-a-system-call-directly)
- [Conventions](#conventions)
- [Debugging a program](#debugging-a-program)
- [The lib/ crate map](#the-lib-crate-map)

---

## The three ways

| | no_std + libmiku | Rust `std` | C |
|---|---|---|---|
| Crates | `system/`, `utils/`, `apps/` | `stdapps/`, `aosh/`, Zed | C files inside a `std` crate (`stdapps/c/`) |
| Target | `lib/miku/x86_64-miku-app.json` (`os: none`, static relocation, `ET_EXEC`) | `lib/rust-std/x86_64-unknown-miku.json` (`os: miku`, `target_family = "unix"`, PIE) | the same as `std`; compiled by `lib/c/miku-cc` |
| Built with | `cargo +nightly build -Z build-std=core,alloc` | `cd builder && cargo run --release -- std <crate>` | `cc` crate from a `build.rs` |
| Runtime | `miku` crate: `_start`, allocator, bindings to the ten libraries | `std` (patched upstream) over libmiku and `libc_miku` | `libc_miku.so` |
| Panics | your `#[panic_handler]` | `panic = "abort"` | - |
| What it is good for | small system programs and commands | anything using crates.io (tokio, smol, rustix) | C dependencies of Rust programs |

Every program, whichever way it was written, is a dynamically linked ELF whose
interpreter is `/lib/ld-miku.so` and whose `DT_NEEDED` entries are miku
libraries. There is no static libc and no second libc: one `malloc`, one
`errno`, one stdio per process.

---

## 1. no_std Rust with libmiku

A no_std program depends on the `miku` crate (`lib/miku`), which declares
every function the ten shared libraries export, provides a `_start` that
initialises the environment and calls your `_start_main`, sets the global
allocator to libmiku's heap (so `alloc::Vec`, `String` and `Box` work), and
writes the link stubs its build script generates.

### A complete program

`apps/src/hello.rs`, as it is in the tree:

```rust
#![no_std]
#![no_main]

pub use miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("Hello from Rust on MikuOS!");
    miku::print("pid = ");
    miku::print_int(miku::getpid() as i64);
    miku::println("");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::println("panic!");
    miku::exit(1);
}
```

With arguments, as every command in `utils/` does (`utils/src/echo.rs`):

```rust
#![no_std]
#![no_main]
extern crate alloc;
pub use miku;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);            // Vec<&'static str>
    for a in args.iter().skip(1) {
        miku::print(a);
        miku::print(" ");
    }
    miku::println("");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

`_start_main` may take `()`, `(argc, argv)` or `(argc, argv, envp)`: the
`_start` in `lib/miku/src/lib.rs` passes all three in `rdi`, `rsi`, `rdx`
after calling `miku_env_init(envp)`, so `miku_getenv` sees the environment the
process was given.

### The crate

```toml
# apps/Cargo.toml (abridged)
[package]
name    = "miku-apps"
edition = "2021"

[workspace]                       # each program crate is its own workspace

[dependencies]
miku = { path = "../lib/miku" }

[profile.release]
opt-level = 1
lto = false

[[bin]]
name = "hello"
path = "src/hello.rs"
```

The crate must sit **one level below the repository root**: the target spec
passes `-T../lib/miku/linker.ld`, which the linker resolves against the crate
being built. Build it the way the builder does (`build_program_crate` in
`builder/src/main.rs`):

```bash
cd apps
cargo +nightly build --release --target ../lib/miku/x86_64-miku-app.json \
  -Z json-target-spec -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem --bin hello
# -> apps/target/x86_64-miku-app/release/hello
```

A plain `cargo build` at the repository root builds the **kernel's** target
(`.cargo/config.toml` forces `x86_64-unknown-none` with the kernel's linker
flags) and produces something that is not a MikuOS program.

### What `miku` gives you

The `miku` crate has three layers:

- **`extern "C"` declarations** of every `miku_*` function (I/O, strings,
  memory, files, processes, threads, time, sockets, data structures, codecs,
  parsers), linked against `core_miku` and the other libraries;
- **safe wrappers** at the top level: `print`, `println`, `eprint`,
  `eprintln`, `print_int`, `print_hex`, `exit`, `args`, `getpid`, `gettid`,
  `open`, `close`, `read`, `write`, `read_file`, `seek`, `fsize`, `sleep`,
  `sleep_ms`, `uptime_ms`, `yield_now`, `thread_spawn`, `futex_wait`,
  `futex_wake`, `getrandom`, `realtime`, `malloc`/`free`, the hash, base64,
  UTF-8 and path helpers, and `read_dir`;
- **modules** for the system's own concepts: credentials (`getuid` ..
  `capset`, `peercred`, `sendfd`, `recvfd`), terminals (`open_pty`, `attach`,
  `foreground`, `termios`, `set_termios`), processes and jobs (`setpgid`,
  `try_wait_any`, `exited`, `exit_code`, `signalled`, `term_sig`, `stopped`,
  `stop_sig`, `wait_status_code`, `wait_job`, `killpg`, `procinfo`,
  `path_of`, `sigdisp`), sessions (`open`, `close`, `list`, `current`), the
  administrative channel (`request`), `klog`, and file helpers
  (`read_file_string`, `write_file`); `miku::time` reads the clock through the
  vDSO (`clock_gettime`, `monotonic_ms`, `realtime_secs`, `getcpu`).

`cstr!("text")` makes a NUL-terminated pointer for the C-convention calls.

---

## 2. Rust std

Since 2026-09-22 MikuOS has a Rust `std`: the target
`lib/rust-std/x86_64-unknown-miku.json` (`"os": "miku"`, `target_family =
"unix"`) and a patch to upstream's `library/` in `lib/rust-std/library/`. A
`std` program has an **ordinary `Cargo.toml` with no MikuOS dependency**.

### A complete program

`stdapps/src/stdhello.rs`:

```rust
fn main() {
    println!("hello from std on MikuOS");

    let mut names: Vec<String> = Vec::new();
    for (i, a) in std::env::args().enumerate() {
        names.push(format!("argv[{i}] = {a}"));
    }
    println!("{} argument(s)", names.len());
    for n in &names {
        println!("  {n}");
    }

    match std::env::var("PATH") {
        Ok(v) => println!("PATH = {v}"),
        Err(e) => println!("PATH unavailable: {e}"),
    }
    eprintln!("and this goes to stderr");
}
```

A threaded, file-using one is no different from Linux:

```rust
use std::{fs, sync::{Arc, Mutex}, thread};

fn main() -> std::io::Result<()> {
    fs::write("/tmp/hello.txt", b"from std\n")?;
    let counter = Arc::new(Mutex::new(0));
    let handles: Vec<_> = (0..4).map(|_| {
        let c = Arc::clone(&counter);
        thread::spawn(move || { for _ in 0..1000 { *c.lock().unwrap() += 1; } })
    }).collect();
    for h in handles { h.join().unwrap(); }
    println!("{} {}", fs::read_to_string("/tmp/hello.txt")?.trim(), counter.lock().unwrap());
    Ok(())
}
```

### Building

```bash
cd builder && cargo run --release -- std stdapps   # any crate directory one level below the root
```

The builder's `std` step (`builder/src/stdport/`) assembles the overlays (it
puts the patch on the toolchain's own `rust-src`, which
`refsrc/miku_programs/rust` is pinned to, into `target/rust-src-miku/`, and
first fetches back any crate source cargo's cache clean removed), passes `-Z
build-std=std,panic_abort`, the linker script and the link stubs, exports the
C toolchain (`CC_x86_64_unknown_miku=lib/c/miku-cc`, `llvm-ar`), and hashes
`lib/rust-std/library` and the target spec so a change to the patch is not
silently ignored (`-Z build-std` fingerprints the sysroot by path). The crate
needs:

```toml
[profile.release]
panic = "abort"
lto = false                # with LTO a stale link stub can drop a call silently

[patch.crates-io]           # only for crates that name targets in lists
libc   = { path = "../target/rust-src-miku/libc-miku" }
rustix = { path = "../target/rust-src-miku/rustix-miku" }
mio    = { path = "../target/rust-src-miku/mio-miku" }
socket2 = { path = "../target/rust-src-miku/socket2-miku" }
tokio  = { path = "../target/rust-src-miku/tokio-miku" }
inotify = { path = "../target/rust-src-miku/inotify-miku" }
```

The target links every program against all ten miku libraries
(`-lcore_miku ... -llibc_miku`), with `--dynamic-linker=/lib/ld-miku.so`, PIE,
no red zone, and `tls-model = local-exec` (without it the first
`thread_local!` needs `__tls_get_addr`, which ld-miku refuses).

### What works

| Module | State |
|---|---|
| `println!`, `format!`, the allocator, `String`/`Vec`, panics | working |
| `std::env` (`args`, `vars`, `var`, `current_dir`, `current_exe`) | working (`current_exe` is `proc_path`) |
| `std::thread` (`spawn`, `join`, `thread_local!`, the parker, `sleep`) | working; stacks are owned by the kernel and freed when a thread ends |
| `std::sync` (`Mutex`, `Condvar`, `RwLock`, `Once`, `mpsc`) | upstream's futex implementations, unchanged |
| `std::time` (`Instant`, `SystemTime`) | working, through the vDSO |
| `std::fs` | `File`, `OpenOptions`, `read_dir`, `metadata`, `symlink_metadata`, symlinks, `rename`, `remove_dir_all`, `set_permissions`, `File::set_times`, `canonicalize` (the kernel's `realpath`, 119) |
| `std::fs::File::lock` | **refused**: the kernel has POSIX record locks, not `flock`'s whole-file per-open-file locks |
| `std::io::pipe`, `std::os::fd` | working |
| `std::process::Command` | `fork` + `execve` + `wait4`, with the POSIX status word; an `execve` failure is reported over a close-on-exec pipe |
| `std::net` | IPv4 TCP and UDP, `lookup_host` through `libc_miku`'s `getaddrinfo` |
| `std::net` refused | IPv6, `bind` to port 0, `local_addr`/`peer_addr` (no `getsockname`), `shutdown`, `peek`, socket timeouts, linger, multicast, broadcast |
| `std::os::unix` | MikuOS's own (`library/std/src/os/miku/unix/`), because upstream's is written over `libc::stat` and `sockaddr_un` |

What is refused is refused by name with an error; nothing silently falls
back to "unsupported".

### Crates verified on the machine

| Crate | Proof | Since |
|---|---|---|
| `polling`, `rustix` (one overlay arm) | `stdpoll`, 8 cases | 2026-09-22 |
| `smol` (`async-io`, `async-executor`, `async-channel`, `async-process`, `async-fs`, `async-signal`), unmodified | `stdasync`, 9 cases (7-9 need `python3 -m http.server 8000` on the host) | 2026-09-22 |
| `tokio` (multi-thread runtime, timers, sync, fs, Unix and TCP sockets, process), `mio`, `socket2` | `stdtokio`, 11 cases | 2026-09-23 |
| `libsqlite3-sys` (bundled), `zstd`, `tree-sitter` + `tree-sitter-json` | `stdc` | 2026-09-23 |
| `inotify` (and `mio` driving it as `notify` does) | `stdwatch` | 2026-09-23 |
| `rustix-openpty`, `rustix` termios | `stdpty` | |
| DNS | `stddns` (needs `dhcp` first) | 2026-09-23 |
| signals as `wasmtime` uses them (`SA_SIGINFO`, rewriting the `ucontext_t`, `sigaltstack`) | `stdsignal` | 2026-09-23 |

### How the port is wired

```
your program
   |  std (lib/rust-std/library/std/src/sys/*/miku.rs, the PAL)
   |     calls libmiku through one seam: sys/pal/miku/abi.rs
   |  crates.io code under cfg(unix)
   |     libc crate      -> lib/rust-libc  (a `miku` module: types, constants,
   |                                         ~30 C functions it declares)
   |     rustix          -> two arms, applied by anchor
   |     mio, socket2, tokio, inotify -> MikuOS added beside `hurd` in their
   |                                     target lists
   |     (both: builder/src/stdport/arms.rs)
   v
libc_miku.so + core_miku.so ... (lib/libmiku)  ->  syscalls
```

- **The PAL calls libmiku, not the system calls.** Every `miku_*` symbol it
  uses is declared once in `sys/pal/miku/abi.rs`; the ten libraries are the
  one place that knows a system call number. Upstream's `target_os = "motor"`
  is the same shape.
- **`std` does not use the `libc` crate.** Every `cfg_select!` in
  `library/std/src/sys` has `not(target_os = "miku")` on its unix arm and a
  MikuOS arm of its own. A guard without an arm silently turns a working call
  into a trap (an `unsupported` fallback, or `intrinsics::abort`), so a new
  `sys/` module is checked by what it does on a boot, never by whether it
  compiles.
- **The `libc` crate is for everything else** that says `cfg(unix)`
  (`lib/rust-libc/libc/src/unix/miku/mod.rs`, 2100 lines): Linux-shaped
  `stat`, `termios`, `sigaction`, `ucontext_t`, `pollfd`, `sockaddr_*`, and
  MikuOS's own constant values (its `O_*` differ from Linux's). The C side of
  each function is in `libc_miku.so`, which must speak the **C convention**
  (`-1` and `errno`), because `rustix` reads any other negative number as a
  byte count.
- Upstream sources stay clean clones in `refsrc/miku_programs/` (rust, libc,
  zed, and the rest); every overlay is assembled into `target/` at build time
  and nothing is edited in place.

The design and history are in [dev/std-architecture.md](../../dev/std-architecture.md).

---

## 3. C through libc_miku

C reaches a MikuOS program as **static archives inside a Rust program**. There
is no C-only program and no C linker path: `lib/c/miku-cc` only compiles
(`-c`), and rustc links the objects with `rust-lld` like everything else.

### A complete example

`stdapps/` does exactly this. `stdapps/build.rs`:

```rust
fn main() {
    println!("cargo:rerun-if-changed=c/stdc.c");
    cc::Build::new().file("c/stdc.c").opt_level(2).warnings(true).compile("stdc");
}
```

`stdapps/c/greet.c` (modelled on `stdapps/c/stdc.c`):

```c
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <pthread.h>

static void *worker(void *arg) {
    printf("thread %s, pid %d\n", (const char *)arg, getpid());
    return NULL;
}

int greet(const char *who) {
    pthread_t t;
    if (pthread_create(&t, NULL, worker, (void *)who) != 0) return -1;
    pthread_join(t, NULL);
    return (int)strlen(who);
}
```

and the Rust side:

```rust
extern "C" { fn greet(who: *const core::ffi::c_char) -> i32; }

fn main() {
    let n = unsafe { greet(c"miku".as_ptr()) };
    println!("greet returned {n}");
}
```

Add `[build-dependencies] cc = "1"` to the crate and build it with the
builder's `std` step; `cc` finds the compiler through `CC_x86_64_unknown_miku`.

### miku-cc

`lib/c/miku-cc` runs the host's **clang** with MikuOS's code model
(`x86_64-unknown-none-elf`, PIC, no red zone), `-nostdlibinc` so the host's
`/usr/include` is never seen (a Linux header that compiles is a Linux layout in
a MikuOS program), clang's own freestanding headers, then `lib/c/include` and
the generated `target/miku-c/include`. It defines `__miku__` and `__unix__`
and the `HAVE_*` facts portable C checks for. A `--target` from the build
system is replaced, and a link request is refused. The constants it includes
are generated by the builder before any C compiles (`builder/src/stdport/cheaders.rs`,
from the Rust `libc` module, so `O_CREAT` and the rest are MikuOS's values);
when a header changed, it compiles `lib/c/check/layout.c` first.

### The rules

- **Headers declare only what `libc_miku.so` defines.** A C program that uses
  something MikuOS does not have fails to compile, which is the cheapest place
  to learn it. The builder checks it before every `std` build, against the
  link stubs with `nm` (`builder/src/stdport/cheaders.rs`).
- **Layouts are pinned** to the Rust `libc` module's definitions by
  `_Static_assert`s on sizes and offsets in `lib/c/check/layout.c`, compiled on
  every C build, and checked again at run time by `stdc`. The C structs are
  Linux's shapes where the kernel's are smaller: `struct stat` is 144 bytes in
  C over the kernel's 64 (`to_c_stat` in `lib/libmiku/libc.rs`), `struct
  termios` 60 over the kernel's 14.
- Data symbols cannot cross the link stubs, so `stdout` is
  `__miku_stdfile(1)` behind a macro.
- Something that cannot work answers `ENOSYS` or `EOPNOTSUPP`, never a fake
  success.

Design: [dev/c-architecture.md](../../dev/c-architecture.md).

---

## The miku libraries

libmiku's sources (`lib/libmiku/`) are built into ten shared libraries
(`lib/mikulibs/`), declared with their dependency graph in
`lib/mikulibs/libs.list`, which drives the library build, the kernel's preload
table, the builder and the link stubs:

| Library | Depends on | Modules (from `lib/libmiku/`) |
|---|---|---|
| `core_miku.so` | - | sys, errno, sync, panic, mem, heap_core, heap, io, proc, thread, string, ctype, convert, num, stdio, time, auxv |
| `sys_miku.so` | core | signal, env, event, timer, log, test, datetime |
| `text_miku.so` | core | utf8, strbuf, format, fmt, glob, regex |
| `ds_miku.so` | core | vec, list, queue, hashmap, treemap, trie, ringbuf, ringbuf2, bitset, heap_queue, channel, arena, slab, pool |
| `algo_miku.so` | core | sort, hash, bitops, math, random, endian |
| `codec_miku.so` | core, algo | base64, hex, checksum, sha256, lz, uuid |
| `fs_miku.so` | core | file, bufio, dir, path |
| `net_miku.so` | - | net |
| `parse_miku.so` | core, text | json, csv, ini, getopt, args |
| `libc_miku.so` | core, fs, sys, algo, text | libc, cmath, and `c/`: printf, strtod, ctime, pthread, netdb, termios, scanf, pty, accounts, sysinfo, at, setjmp |

About **1440 functions** are exported in all (`#[no_mangle]`), about 900 of
them `miku_*` and about 540 in `libc_miku.so`. Libraries call each other only
through the C ABI; a module used by another library is reached through a thin
forwarding shim (`lib/mikulibs/shim/`), so a module's source is the same
whether its dependency is in its own library or another. A `#[no_mangle]`
function must have `#[no_mangle]` directly above `fn` (attributes only, no doc
comment between): the stub generator (`lib/mikulibs/build.rs`) reads it that
way.

The libraries are built with the target `lib/mikulibs/x86_64-miku-lib.json`
and **embedded in the kernel image** (the kernel's `build.rs` generates the
table from `libs.list` with `include_bytes!`), as is `ld-miku.so`. At boot they
appear in `/lib` as read-only vnodes and in the kernel's shared-object cache,
so `map_lib` maps their pages from memory and a disk never holds them. A
package therefore cannot ship a shared library yet
([system.md](system.md#mikupkg)).

---

## libmiku by module

The C-convention functions return `-errno` (MikuOS's convention) unless
noted; `libc_miku`'s POSIX functions return `-1` and set `errno`.

| Area | Module (library) | What it provides |
|---|---|---|
| system calls | `sys` | the numbers (`SYS_*`) and the inline `sc0`..`sc4` stubs; not exported |
| errors | `errno` (core) | the codes, `miku_strerror`, `miku_perror`, the thread's `errno` at `tp + 40` |
| I/O | `io` (core) | `miku_write`, `miku_read`, `miku_poll`, `miku_ioctl`, `miku_klog` |
| stdio | `stdio` (core), `fmt` (text) | `miku_print`, `miku_println`, `miku_printf`, `miku_snprintf`, `miku_dprintf`, `miku_getchar`, `miku_readline` |
| strings | `string`, `ctype`, `convert`, `num` (core), `utf8`, `strbuf`, `format`, `glob`, `regex` (text) | C string functions, classification, `strtol`, number formatting, UTF-8, growable strings, globbing, a small regex |
| memory | `mem` (core) | `miku_memcpy`, `memset`, `memmove`, `memcmp` (word-at-a-time) |
| heap | `heap`, `heap_core` (core) | `miku_malloc`, `free`, `realloc`, `calloc`, `memalign` (below) |
| processes | `proc` (core) | `fork`, `exec`, `execve`, `wait4`/`waitpid`/`try_wait`, `kill`/`killpg`, `getpid`, `brk`, `mmap`/`munmap`/`mprotect`/`mmap_file`/`msync`, credentials, sessions, `tty`, `procinfo`, `proc_path`, `kcmd`, `power`, `peercred`, `sendfd`/`recvfd`, `map_lib` |
| threads | `thread` (core) | `miku_thread_spawn`, `miku_thread_spawn_owned`, `miku_thread_exit`, `miku_gettid`, `miku_futex_wait`, `miku_futex_wake` |
| sync | `sync` (core) | `miku_mutex_*` (spins 100 times, then sleeps on a futex), atomics, `miku_once_*` |
| time | `time` (core), `timer`, `datetime` (sys) | `miku_sleep` (4 ms ticks), `miku_sleep_ms`, `miku_sleep_ns`, `miku_uptime_ms`, `miku_clock_gettime` (vDSO first), `miku_realtime`, stopwatches, one-shot timers, calendar formatting |
| auxv | `auxv` (core) | `miku_getauxval`, `miku_set_auxv` (called by ld-miku), the vDSO lookup |
| signals | `signal` (sys) | `miku_signal`, `miku_sigaction`, the handler table, the blocked mask, the entry stub |
| environment | `env` (sys) | `miku_env_init`, `miku_getenv`, `miku_setenv`, `miku_unsetenv`, `miku_environ` (a heap table, no size limits) |
| files | `file`, `bufio`, `dir`, `path` (fs) | open/close/seek/stat/lstat/fstat/getdents/mkdir/rename/link/symlink/chmod/chown/utimensat/realpath and the `*at` forms, buffered readers and writers, directory iteration and walks, `miku_mkdir_p`, path manipulation |
| network | `net` (net) | TCP, UDP, ICMP and AF_UNIX sockets, `miku_set_ttl` |
| data structures | ds | vectors, lists, queues, hash maps, AVL tree maps, tries, ring buffers, bitsets, priority queues, channels, arenas, slabs, pools |
| algorithms | algo | sort and search, hashes (FNV, djb2, CRC32, SipHash, Murmur3), bit operations, integer maths, the PRNG (`miku_rand`, seeded from uptime and pid: **not** for secrets; use `miku_getrandom`), endian conversion |
| codecs | codec | base64, hex, checksums, SHA-256 and HMAC, an LZ compressor, UUIDs |
| parsing | parse | JSON (reader and writer), CSV, INI, `getopt`, a flag parser |
| logging, tests, events | `log`, `test`, `event` (sys) | leveled logging, a test framework, callbacks |

### Heap

`lib/libmiku/heap.rs` is every program's `malloc` (Rust programs import
`malloc`/`free`/`realloc` from `core_miku.so`). Since 2026-10-20:

- **segregated bins**: one per 16-byte size below 1 KiB, then four per power
  of two, with a bitmap of non-empty bins, so finding a block is constant time
  (16-22 ns an operation in the host replay, against a best-fit walk that grew
  to 776 ns);
- **boundary tags**: a `FLAG_PREV_FREE` bit and a footer word in each free
  block, so a free merges with both neighbours and no two free blocks are ever
  adjacent;
- requests of 32 KiB and more go straight to `mmap`;
- since 2026-10-27, **per-thread caches** (80 size classes up to 16 KiB, at most
  256 KiB per thread, the pointer at TCB `tp + 56`) in front of a
  `sync::Mutex`, which brought multi-threaded allocation close to Linux's;
  `miku_heap_thread_done` empties a thread's cache when it ends; a double free
  is detected.

The allocator itself is `heap_core.rs`, shared with nothing else in ring 3
(the kernel has its own slab heap since 2026-10-27). Host tests: `cd
lib/libmiku/hosttest && cargo test` (compiles `heap.rs` by `#[path]` over a
stand-in `mmap`; `heap::check` verifies tiling, tags, bins and bitmap). Not done
yet: growing `realloc` in place, returning empty slabs.

### fork safety

`miku_fork` (`lib/libmiku/proc.rs`) holds libmiku's locks across the system
call so the child never inherits one held by a sibling thread: hooks' outer
locks (the environment), the heap lock, hooks' inner locks, the syscall, then
the parent and child hooks in reverse, then the heap unlock. The child
force-unlocks the two signal tables instead of holding them, because a
`SIGCHLD` can be delivered at the tail of the `fork` on the forking thread and
its dispatcher takes them. A new lock a forked child can reach must be added to
these hooks (`miku_fork_hooks`). `libc_miku`'s `fork` adds
`pthread_atfork` handlers.

### Time: the tick is 4 ms

The kernel timer runs at 250 Hz. `miku_sleep(ticks)` and `miku_uptime()` count
4 ms ticks (libmiku assumed 10 ms until 2026-09-22, which made every ring-3
sleep 40 % of what it asked). For real durations use `miku_sleep_ms`,
`miku_sleep_ns` (syscall 122, kept to microseconds), `nanosleep`, or
`std::thread::sleep`.

---

## libc_miku

`libc_miku.so` is the C library: about 540 functions, written in Rust in
`lib/libmiku/libc.rs`, `lib/libmiku/cmath.rs` and `lib/libmiku/c/`.

### The headers

47 headers in `lib/c/include` (plus the generated `bits/` constants):

| Header | What it declares |
|---|---|
| `stdio.h` | `FILE` streams (`fopen`, `fdopen`, `fread`, `fwrite`, `fgets`, `fputs`, `fseek`, `ftell`, `setvbuf`, `fflush`...), `printf` family with full conversions and floats, `scanf` family, `perror`, `remove`, `rename` |
| `stdlib.h` | `malloc` family, `aligned_alloc`, `posix_memalign`, `exit`/`_Exit`/`atexit`/`abort`, `getenv`/`setenv`/`putenv`/`unsetenv`, `strtol`..`strtod`, `qsort`, `bsearch`, `rand`, `abs`/`div` |
| `string.h`, `strings.h` | string and memory functions, `strerror`, `strsignal`, `strcasecmp` |
| `ctype.h`, `wctype.h`, `wchar.h` | character classes, wide classes |
| `math.h` | about 125 functions (`cmath.rs`) |
| `unistd.h` | `read`, `write`, `close`, `lseek`, `pread`, `pwrite`, `dup`..`dup3`, `pipe`, `pipe2`, `fork`, `execve`, `execvpe`, `fexecve`, `_exit`, `getpid`, `getppid`, ids, `setpgid`, `setsid`, `getpgrp`, `chdir`, `fchdir`, `getcwd`, `access`, `faccessat`, `link`, `symlink`, `readlink`, `unlink`, `rmdir`, `truncate`, `ftruncate`, `fsync`, `sync`, `sleep`, `usleep`, `isatty`, `ttyname_r`, `sysconf`, `pathconf`, `getpagesize` |
| `fcntl.h` | `open`, `openat`, `creat`, `fcntl` (including `F_GETPATH`), `posix_fadvise`, `posix_fallocate`, `O_*` |
| `sys/stat.h` | `stat`, `fstat`, `lstat`, `fstatat`, `mkdir`, `mkdirat`, `chmod`, `fchmod`, `fchmodat`, `umask`, `mkfifo`, `mknodat`, `utimensat`, `futimens` |
| `dirent.h` | `opendir`, `fdopendir`, `readdir`, `closedir`, `rewinddir`, `seekdir`, `telldir`, `dirfd` |
| `sys/mman.h` | `mmap`, `munmap`, `mprotect`, `msync`, `madvise`, `mlock`.. (the locks succeed without effect) |
| `pthread.h` | threads, mutexes (normal, recursive, process-shared on tmpfs), condvars (with `pthread_condattr_setclock`), rwlocks, keys, `pthread_once`, `pthread_atfork`, attributes |
| `signal.h` | `sigaction`, `signal`, `kill`, `killpg`, `raise`, `sigprocmask`, `pthread_sigmask`, sets, `sigaltstack`, `sigsuspend`, `sigwait` |
| `setjmp.h` | `setjmp`, `longjmp` |
| `time.h`, `sys/time.h` | `clock_gettime`, `clock_getres`, `clock_settime`, `nanosleep`, `time`, `gettimeofday`, `gmtime`, `localtime` (and `_r`), `mktime`, `timegm`, `strftime`, `difftime`, `clock`, `utimes`, `lutimes` |
| `termios.h`, `pty.h`, `sys/ioctl.h` | `tcgetattr`, `tcsetattr`, `cfmakeraw`, speeds, `tcsetpgrp`.., `ioctl`, `openpty`, `login_tty`; `posix_openpt`, `grantpt`, `unlockpt`, `ptsname` in `stdlib.h` |
| `sys/socket.h`, `netinet/in.h`, `netinet/tcp.h`, `arpa/inet.h`, `sys/un.h` | sockets, `sendmsg`/`recvmsg` (no control messages), `inet_pton`, `inet_ntop` |
| `netdb.h` | `getaddrinfo`, `freeaddrinfo`, `gai_strerror` |
| `poll.h`, `sys/select.h` | `poll`, and `select` built on it |
| `sys/wait.h` | `waitpid`, `waitid`, the `W*` macros |
| `sys/uio.h` | `readv`, `writev`, `preadv`, `pwritev` (loops of single calls) |
| `sys/resource.h` | `getrlimit`, `setrlimit`, `getrusage`, `getpriority`, `setpriority` |
| `sys/random.h` | `getrandom` |
| `sys/auxv.h` | `getauxval` |
| `dlfcn.h` | `dlopen`, `dlsym`, `dlclose`, `dlerror` (they fail: below) |
| `locale.h` | `setlocale` (only `"C"`) |
| `sched.h` | `sched_yield` |
| `sys/file.h` | `flock` (on record locks) |
| others | `assert.h`, `errno.h`, `limits.h`, `inttypes.h`, `endian.h`, `alloca.h`, `sys/types.h`, `sys/param.h` |

### Notes by area

- **pthreads** (`c/pthread.rs`): the pthread block lives at TCB `tp + 48`,
  made on first use, so threads started by Rust or libmiku also get one.
  Mutexes and condvars are futex-based; process-shared ones work in a tmpfs
  `MAP_SHARED` mapping.
- **stdio**: streams are registered so `exit` and `fflush(NULL)` flush them
  all; `atexit` handlers run at `exit`.
- **malloc** is libmiku's heap.
- **termios and pty**: Linux's 60-byte `struct termios` translated to the
  kernel's 14 bytes (`c/termios.rs`); speeds are stored and have no effect;
  `openpty` and `login_tty` are `open("/dev/ptmx")`, `TIOCGPTN`,
  `/dev/ptsN` and `TIOCSCTTY` (`c/pty.rs`).
- **getaddrinfo** (`c/netdb.rs`): a numeric address, `localhost`,
  `/etc/hosts`, then one DNS query to the DHCP lease's resolver; IPv4 only.
  The wire format is shared with `mikunet` (`lib/net/src/dns_wire.rs`).
- **select** is built on `poll` (MikuOS has no `select` of its own). `fd_set`
  is 1024 bits as everywhere, so **sockets, numbered from 4096, cannot be put in
  one**: use `poll` for anything that waits on a socket. The exception set is
  never marked.
- **pwd and grp** (`c/accounts.rs`): `getpwnam_r`, `getpwuid`, `getpwuid_r`,
  `getgrnam_r`, `getgrgid_r`, `getgrouplist`, `initgroups`, `setgroups`,
  `getgroups`, reading `/etc/passwd` and `/etc/group`.
- **sysinfo** (`c/sysinfo.rs`): `uname`, `sysconf` numbers and CPU tick
  counts for system monitors.
- **time**: `localtime` is UTC (there is no zone database).
- **`pread`/`pwrite`** are `seek`, transfer, `seek` back: not atomic against
  another thread on the same descriptor.

### What is missing

- **`dlopen`**, `dlsym`, `dlclose` fail: `dlerror()` says "dynamic loading is
  not supported on MikuOS". Every C library in a program is linked in.
- **`getsockname`**, `getpeername` answer `EOPNOTSUPP`; `SCM_RIGHTS` control
  messages are `EOPNOTSUPP` (use `sendfd`/`recvfd`).
- **`posix_spawn`**, `vfork`, `ptrace`, `prctl`, `epoll`, `eventfd`,
  `signalfd`, `timerfd` do not exist.
- No locales beyond `"C"`, no time zones, no wide-character stdio, no C++.
- `setuid` and friends are not exported under their POSIX names (use the
  `miku_*` functions).
- `getrlimit` is library-answered ([processes.md](processes.md#resource-limits)).

### The layout static asserts

`lib/c/check/layout.c` holds `_Static_assert`s on `sizeof` and `offsetof`
for every struct the headers define (`stat`, `dirent`, `tm`, `timespec`,
`timeval`, `pollfd`, `flock`, the pthread types, `sockaddr_*`, `termios` and
the rest), against the values in the Rust `libc` module. `miku-cc` compiles it
whenever a header or a generated constant changes, so a drift is a build
error. `stdc` re-checks the same sizes and constants at run time against the
Rust side (`stdc_sizeof`, `stdc_const` in `stdapps/c/stdc.c`).

---

## Dynamic linking

Every MikuOS program has `PT_INTERP` = `/lib/ld-miku.so` (`lib/ld-miku/`). The
kernel loads the program and ld-miku, and jumps to ld-miku
([processes.md](processes.md#the-elf-loader)), which:

1. walks the auxiliary vector for the program's headers;
2. for each `DT_NEEDED` entry, recursively: asks the kernel with `map_lib`
   (15), which maps the library from its shared-object cache (the ten miku
   libraries are preloaded at boot) or loads it from `/lib` or `/usr/lib`;
   failing that, ld-miku opens `/lib/<name>`, `/usr/lib/<name>` or
   `/usr/local/lib/<name>` itself and maps it (only the ten embedded libraries
   are in `/lib` today);
3. resolves symbols and applies relocations (`RELATIVE`, `IRELATIVE`, `64`,
   `GLOB_DAT`, `JUMP_SLOT`, `COPY`, `PC32`, `TPOFF64`, `DTPMOD64`,
   `DTPOFF64`; a weak undefined symbol resolves to zero);
4. applies `PT_GNU_RELRO`;
5. builds the first thread's TLS block and the 64-byte TCB;
6. hands the auxiliary vector to libmiku (`miku_set_auxv`) before any
   constructor runs, then runs the initialisers (`DT_INIT`, then
   `DT_INIT_ARRAY`) and jumps to the entry. A library it cannot find is
   reported as `[ld-miku] not found: <name>`.

**The search path is fixed**: `/lib`, `/usr/lib`, `/usr/local/lib`. ld-miku
reads no `LD_LIBRARY_PATH`, and the `-rpath /lib` the targets pass is not
consulted. A library of your own goes in `/lib`.

**Cross-module TLS is refused**: only the program's own `PT_TLS` (static,
local-exec) exists. A library with a `PT_TLS` of its own, or a program needing
`__tls_get_addr`, fails to start.

**`dlopen` is not supported** ([above](#what-is-missing)).

**Building a dynamically linked program** is the default: both targets set
`--dynamic-linker=/lib/ld-miku.so` and link against the stubs in
`lib/miku/stubs/` (stub `.so` files with the runtime sonames, generated by
`lib/miku/build.rs` from `libs.list` and the `#[no_mangle]` functions). The
no_std target links only what the program uses; the `std` target names all ten
libraries. Check a binary:

```bash
readelf -l prog | grep INTERP        # /lib/ld-miku.so
readelf -d prog | grep NEEDED        # core_miku.so, ...
readelf -n prog | grep 'Build ID'    # present
```

The kernel also starts programs with no `PT_INTERP` (the kernel's own embedded
images); such a program gets its TCB from the kernel and cannot load
`libc_miku`.

---

## Building and installing a program

The whole system image is built by the builder (`cd builder && cargo run`),
which is described with QEMU and the boot media in [building.md](building.md).
For a program:

### Adding a program

1. Write `src/<name>.rs` in the right crate: `utils/` for commands, `system/`
   for what the system is made of (shells, login, the window manager),
   `apps/` for applications and tests, `stdapps/` (or a new crate one level
   below the root) for `std` programs.
2. Add a `[[bin]]` to the crate's `Cargo.toml`.
3. Add `("<name>", "<package>")` to the crate's list in `builder/src/main.rs`:
   `SYSTEM_BINS`, `UTIL_BINS`, `APP_BINS` or `STD_BINS` (`AOSH_BINS` for
   aosh). The package is one of `miku-base`, `miku-system`, `miku-desktop`,
   `miku-net`, `miku-demos` ([system.md](system.md#mikupkg)). One list drives
   both the build and the staging into `/bin`; a program built but not listed
   is not on the image.
4. Rebuild with the builder.

### How it reaches /bin

`/bin`, `/etc` and `/lib/firmware` come from **`boot/firmware.img`**, an
ext2 image the builder produces by installing the packages into it, and GRUB
loads as a multiboot2 module (the ten libraries in `/lib` are inside the
kernel image itself). The kernel exposes the module as a read-only RAM block
device and mounts it at boot. The root disk (`disk.img`, ext4) is where a
running machine writes, and `mikupkg` installs there; attaching another disk
does **not** override the module's `/bin`.

### Testing a ring-3 binary

- The quickest loop is a full builder run and a QEMU boot
  ([building.md](building.md), [testing.md](testing.md)).
- To swap one binary into an existing ISO tree without a full rebuild, write it
  into the module with `debugfs` (`chmod u+w isoroot/boot/firmware.img`, then
  `debugfs -w` with `rm /bin/<name>` and `write <file> <name>` in `/bin`) and
  rebuild the ISO with `grub-mkrescue`.
- Before believing a run, confirm the image has your build: `strings` the
  staged binary for a string you just added. A stale binary looks exactly like
  a broken program.
- Test programs in `apps/` (`abitest`, `threadtest`, `polltest`, `sigtest`,
  `crashtest`, `mmaptest`, `envtest`, `test_full`) and `stdapps/` (`std*`) are
  on the image; many are also run headless with `run=` on the boot line.

---

## Ported C dependencies

| Library | Through | Proved by |
|---|---|---|
| SQLite 3.46 | `libsqlite3-sys` with `bundled`, unmodified | `stdc`: in memory and on disk, reopened from another thread, record locks between two processes |
| zstd | `zstd` / `zstd-sys`, unmodified | `stdc`: round trips |
| tree-sitter | `tree-sitter` 0.25 + `tree-sitter-json`, unmodified | `stdc`: parses JSON |
| FreeType 2.14.3 | vendored in `lib/freetype` (for aosh's text) | aosh |

Adding one:

1. Add the crate to a `std` crate's dependencies and build with the
   builder's `std` step.
2. A compile error for a missing function or header: declare it in the right
   `lib/c/include/*.h` **and** define it in `libc_miku.so`
   (`lib/libmiku/libc.rs`, or a file under `lib/libmiku/c/` for an area).
3. A new struct: its Rust definition in `lib/rust-libc/libc/src/unix/miku/`
   first, then the header, then a line in `lib/c/check/layout.c`.
4. A compiler flag goes in `lib/c/miku-cc` and nowhere else.
5. If the C code needs what MikuOS cannot do, return `ENOSYS`, never success.

Next on this path, per the design note: `aws-lc-sys` and `libmimalloc-sys`.

---

## Zed: the big example

The Zed editor is the program the `std` port was built for
([dev/std-architecture.md](../../dev/std-architecture.md), "Where Zed comes
in"). `cd builder && cargo run --release -- zed build -p zed` builds it from a copy of a clean
upstream clone (`refsrc/miku_programs/zed` copied to `target/zed-miku/src` on
every run), with the same target and overlays as `stdapps`, the crates that
name targets patched through `--config` flags, and one patch per concern in
`lib/rust-std/zed-patches/` (`client`, `crashes`, `fs`, `gpui`, `languages`,
`workspace`, `zed`). The builder ships the result as `/bin/zed`
(`EXTERNAL_BINS`, package `zed`, stripped).

What it took, in the order it was found:

- **Kernel**: threads and futexes (106-110), per-thread TLS, the FPU in the task,
  `getdents` with a cursor and 255-byte names, `lstat`, `fcntl` and a real
  `O_NONBLOCK`, POSIX record locks, inotify (116-118), `realpath` and the path
  word for every `*at` call, `utimensat` by path, `pipe2`, `getsockopt` and
  non-blocking `connect`, sockets owned by the process instead of the thread,
  descriptors per process raised to 512, pids recycled, exec reading ELF images
  by range (a 100+ MB binary), the syscall entry aligning the stack, faults as
  signals, TLB shootdowns, and many fixes found by running it (fork storms,
  lost wakes, a COW page writable before the flush).
- **Libraries**: libmiku's heap merging freed blocks into neighbouring
  mappings (the big blocker), then the binned heap; the fork-safe locks; the
  environment on the heap; `getaddrinfo`.
- **std and crates**: the whole PAL, `target_family = "unix"` and its
  `libc`/`rustix` overlays, mio/socket2/tokio target arms, C through
  `miku-cc` (SQLite, zstd, tree-sitter).
- **Pixels**: gpui gets a Mayland client and a CPU renderer
  (`lib/rust-std/zed-files/`, wired by `zed-patches/gpui.patch`); MikuOS sits
  beside FreeBSD in `gpui_linux`.

Milestones: `remote_server` ran and served a ping over Unix sockets on
2026-09-24; the windowed editor ran the same week with text, clicks, wheel and
keys; a real editing session ran on 2026-09-25/26. `Ctrl+W` is bound to
`/bin/zed` in the default desktop configuration
(`etc/mayland/default.d/50-keys.conf`).

---

## Calling a system call directly

From no_std Rust, copy the stub shape of `lib/libmiku/sys.rs`:

```rust
use core::arch::asm;

/// syscall with four arguments; returns the raw value (negative = -errno)
#[inline(always)]
unsafe fn sc4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let r: i64;
    asm!(
        "syscall",
        in("rax") nr, inlateout("rdi") a1 => _, inlateout("rsi") a2 => _,
        inlateout("rdx") a3 => _, inlateout("r10") a4 => _,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    r
}

// poll_ns (123) has no library wrapper: wait up to 1.5 ms on one descriptor
#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }

fn wait_readable(fd: i32) -> i64 {
    let mut p = PollFd { fd, events: 0x001, revents: 0 };
    unsafe { sc4(123, &mut p as *mut PollFd as u64, 1, 1_500_000, 0) }
}
```

The kernel preserves every register but `rcx` and `r11`; libmiku declares the
argument registers clobbered anyway, which is always safe. From `std` code use
the same `asm!` (there is no `libc::syscall` on MikuOS). The number, argument
order and error codes of every call are in the
[ABI index](../MikuOS_ABI.md#3-index-of-all-system-calls). Remember:

- paths are `(ptr, len)` and the length is a **path word**: the high 32 bits
  name a base directory descriptor plus one ([ABI §2.4](../MikuOS_ABI.md#24-paths-and-the-path-word));
- flags use MikuOS's values, not Linux's (`O_CREATE` is 0x8);
- more than four arguments travel in a block of `u64`s behind one pointer.

---

## Conventions

| Topic | Rule |
|---|---|
| errno | the kernel and the `miku_*` functions return `-errno`; `libc_miku`'s POSIX functions return `-1` and set the thread's `errno` (`%fs:0 + 40`, `__errno_location`) |
| paths | absolute or relative to the working directory; there is no `~` expansion anywhere below the shell (and `msh` has none either); names up to 255 bytes, paths up to 4096 |
| environment | given by the parent through `execve`; the kernel's own default (for `exec` (46) and the kernel console) is `PATH=/bin HOME=/ TERM=miku OS=MikuOS`; what a login session starts with is in [system.md](system.md) |
| argv | `argv[0]` is what the parent passed (the kernel uses the path if `argv` is empty) |
| auxv | read with `getauxval`/`miku_getauxval`; `AT_SECURE` is 1 when the effective ids differ from the real ones; the vDSO is `AT_SYSINFO_EHDR` |
| stdio | fds 0-2 are inherited; a program on a terminal sees its pty slave or its controlling terminal there |
| time | `CLOCK_REALTIME` fails until the wall clock is set; prefer `CLOCK_MONOTONIC` for intervals |
| randomness | `getrandom` for anything secret; `miku_rand` is a seeded xorshift |
| logging | a service logs with `miku_klog` (`dmesg` shows it), not to stdout, which is somebody's terminal |
| exit status | `exit(code)`; a death by signal is reported as such, not as `128 + sig` |

---

## Debugging a program

| Problem | Where to look |
|---|---|
| it crashed | the `[crash]` report on serial / in `mikudo dmesg`, with unbiased addresses and symbols; `/tmp/core.<name>.<pid>` for `gdb`; `elf addr /bin/<prog> <hex>` in the kernel console ([diagnostics.md](diagnostics.md#ring-3-crash-reports)) |
| it will not start | `[exec] ELF load failed: <reason>` on serial; `readelf -l` for `INTERP` and W+X segments; ld-miku prints which library or symbol it could not find |
| a call fails | the negative return is an errno ([ABI §2.2](../MikuOS_ABI.md#22-return-values-and-errno)); a C or `std` caller gets `errno` / `io::Error::raw_os_error` |
| a handler does not run | signals from another process arrive only at a system call boundary; install with `sigaction`/`miku_signal` (which registers the entry point), not `sigdisp` alone |
| it hangs | `ps` for its state; `mikudo dmesg -f` while reproducing; a wait that ends after exactly 100 ms is a lost wake; [diagnostics.md](diagnostics.md#how-to-debug-x) |
| progress logging | `miku_klog("...")` and `mikudo dmesg -m user` (or `-g`) |
| a std program misbehaves only on MikuOS | a `cfg_select!` arm falling back to `unsupported`; check the PAL in `lib/rust-std/library/std/src/sys/*/miku.rs` |
| a C library misbehaves | a layout mismatch is caught at build time by `layout.c`; a missing function at compile time by the headers |

Host-side checks that need no boot: `cd lib/libmiku/hosttest && cargo test`
(the heap), `cd lib/net && cargo test` (`mikunet`), `cd lib/mikuauth && cargo
test`, `cd lib/mayland && cargo test-host` (libmay); see [testing.md](testing.md).

---

## The lib/ crate map

| Crate | What it is | Runs in |
|---|---|---|
| `lib/libmiku` | the sources of the ten miku libraries: system call stubs, heap, threads, signals, files, strings, data structures, and `libc_miku`; `hosttest/` tests the heap on the build host | ring 3 (host for `hosttest`) |
| `lib/mikulibs` | the build of the ten `.so` files from libmiku's modules, `libs.list`, the cross-library shims, the library target spec | build; ring 3 |
| `lib/miku` | the crate no_std programs depend on: declarations, safe wrappers, `_start`, the global allocator, link stubs, the app target spec and linker script | ring 3 |
| `lib/ld-miku` | the dynamic linker `/lib/ld-miku.so`; also embedded in the kernel | ring 3 |
| `lib/vdso` | the vDSO image and `abi.rs`, the vvar layout shared with the kernel | ring 3 (the kernel includes `abi.rs`) |
| `lib/rust-std` | the `x86_64-unknown-miku` target, the `std` patch, Zed's patches and files | build; ring 3 |
| `lib/rust-libc` | the `libc` crate's `miku` module, applied to a clean clone | build; ring 3 |
| `lib/c` | C headers, `miku-cc`, the layout check | build |
| `builder/src/stdport` | how all of the above is assembled and built: crate sources, the overlay, the arms for crates.io crates, the generated C constants and the declaration check, the `std` build, Zed | build |
| `lib/mayland` | `libmay`, the Mayland display protocol: encoder, client connection, pools, GPU helpers, shapes, keymaps; linked by the kernel's server and by every client | kernel and ring 3 (host tests) |
| `lib/net` | `mikunet`: DNS, NTP, TLS 1.2/1.3 with X.509, HTTP/1.1 and HTTP/2, and the L2-L4 stack the kernel links | kernel (stack) and ring 3 (protocols); host tests |
| `lib/extfs` | the ext2/ext3/ext4 on-disk format, journal, mkfs | kernel; host tests |
| `lib/btrfs` | the btrfs on-disk format, read-only | kernel; host tests |
| `lib/fshost` | the block-device trait and test double filesystems are written against | kernel; host |
| `lib/mikuauth` | passwords (Argon2id), `/etc/passwd`, `/etc/group`, `/etc/shadow`, the `/etc/mikudo.conf` grammar; 102 host tests | ring 3 (`mikudod`, `login`, `passwd`); host |
| `lib/mikupkg` | the package format, trust, the image description, install logic | ring 3 (`mikupkg`); builder; host tests |
| `lib/freetype` | vendored FreeType 2.14.3 with a shim, for aosh's text | ring 3 (aosh); host |
| `lib/buildgen` | build-time generators: images, QOI, shaders, the cubin reader shared with the kernel's `gpu_program` | build (the kernel's `build.rs`) |
