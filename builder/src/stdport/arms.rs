//! Adds MikuOS to target lists in third-party crates
//!
//! `libc`, `rustix`, and a dozen crates under tokio, Zed, and aosh choose
//! platform code from `cfg` chains listing `target_os` names, with no unix
//! default. Applied by anchor against the local version instead of whole-file
//! patches. Edits are idempotent; a missing anchor is an error naming the crate
//! and file.

use std::fs;
use std::path::Path;

const MIKU: &str = "target_os = \"miku\"";

type R = Result<(), String>;

fn read(root: &Path, rel: &str) -> Result<String, String> {
    fs::read_to_string(root.join(rel)).map_err(|e| format!("{}: {e}", root.join(rel).display()))
}

fn write(root: &Path, rel: &str, s: &str) -> R {
    fs::write(root.join(rel), s).map_err(|e| format!("{}: {e}", root.join(rel).display()))
}

/// `(file, anchor, text inserted before the anchor)`, each applied once
fn insert_before(what: &str, root: &Path, edits: &[(&str, &str, &str)]) -> R {
    for (rel, anchor, insert) in edits {
        let s = read(root, rel)?;
        if s.contains(insert) { continue; }
        if !s.contains(anchor) {
            let head: String = anchor.chars().take(70).collect();
            return Err(format!("{what} {rel}: anchor not found - the crate has changed shape:\n{head}"));
        }
        write(root, rel, &s.replacen(anchor, &format!("{insert}{anchor}"), 1))?;
    }
    eprintln!("{what} arms applied");
    Ok(())
}

/// The three lines in `libc`'s dispatch files selecting `src/unix/miku/` and
/// `src/new/miku/`. The modules are in `lib/sdk/rust-libc/libc/`; these go in
/// `cfg_if!` chains that vary by version
pub fn libc(root: &Path) -> R {
    insert_before("libc", root, &[
        (
            "src/unix/mod.rs",
            "    } else if #[cfg(any(target_os = \"nto\", target_os = \"qnx\"))] {\n        mod nto;\n        pub use self::nto::*;",
            "    } else if #[cfg(target_os = \"miku\")] {\n        mod miku;\n        pub use self::miku::*;\n",
        ),
        (
            // Link block, anchored on the aix arm that links (`aix` appears in
            // several chains)
            "src/unix/mod.rs",
            "    } else if #[cfg(target_os = \"aix\")] {\n        #[link(name = \"c\")]\n        #[link(name = \"m\")]\n        #[link(name = \"bsd\")]\n        #[link(name = \"pthread\")]",
            "    } else if #[cfg(target_os = \"miku\")] {\n\
             \x20       // Nothing to link. MikuOS has no `libc.so`, no `libm`, no\n\
             \x20       // `librt` and no `libpthread`: its C library is the ten\n\
             \x20       // `*_miku.so` that `lib/runtime/libmiku/libs.list` names, and the\n\
             \x20       // target spec's pre-link args already put every one of them\n\
             \x20       // on the line. Asking for `-lc` here would ask the linker for\n\
             \x20       // a file that does not exist on this system and never will\n\
             \x20       extern \"C\" {}\n",
        ),
        (
            "src/new/mod.rs",
            "    } else if #[cfg(any(target_os = \"nto\", target_os = \"qnx\"))] {",
            "    } else if #[cfg(target_os = \"miku\")] {\n        mod miku;\n        pub(crate) use miku::*;\n",
        ),
    ])
}

/// `rustix`'s two lists with no fallback arm
pub fn rustix(root: &Path) -> R {
    insert_before("rustix", root, &[
        (
            // `ioctl` request type. The chain has no default. MikuOS's
            // `ioctl`(71) takes `u64` (`c_ulong`), as on the BSDs and glibc
            // Linux
            "src/ioctl/mod.rs",
            "// Windows has `ioctlsocket`, which uses `i32`.\n#[cfg(windows)]\ntype _Opcode = i32;",
            "// MikuOS: `ioctl`(71) takes the request as a `u64`.\n#[cfg(target_os = \"miku\")]\ntype _Opcode = c::c_ulong;\n\n",
        ),
        (
            // `Termios` has `c_line` only on listed systems. MikuOS's
            // `struct termios` is Linux's, with it, at 60 bytes;
            // `rustix-openpty` asserts the size. Inserted before the last
            // entry, Redox
            "src/termios/types.rs",
            "        target_os = \"redox\"\n    ))]\n    pub line_discipline: u8,",
            "        target_os = \"miku\",\n",
        ),
    ])
}

/// How a row puts MikuOS into a file
#[derive(Clone, Copy)]
enum Near {
    /// Beside every `target_os = "x"` entry (or the first `n`)
    Os(&'static str),
    /// Beside a `target_arch = "x"` arm
    Arch(&'static str),
    /// Every line naming the enum variant gets a twin naming `Miku`
    Dup(&'static str),
    /// `#[cfg(unix)]` on the item starting with this text becomes
    /// `all(unix, not(target_os = "miku"))`
    UnixBut(&'static str),
    /// `"miku",` after `"x",` in a list of bare names a build script keeps
    Str(&'static str),
    /// A copy of this file, under the row's name, if the crate has none
    Copy(&'static str),
}

/// `(file, how, entry count; None is all)`. A file starting with `?` is
/// optional, since semver lines differ
type Row = (&'static str, Near, Option<usize>);

use Near::*;

/// MikuOS goes beside GNU Hurd, the closest known to `mio` and `socket2`: unix
/// with no `epoll`, a `poll(2)` reactor, a pipe waker, `pipe2`, `accept4`,
/// `SOCK_NONBLOCK | SOCK_CLOEXEC`, `int` `msg_iovlen`, and no `IP_RECVTOS` or
/// source-specific multicast. Other placements say why
fn table(krate: &str) -> Option<&'static [Row]> {
    Some(match krate {
        "mio" => &[
            // Paths for epoll and kqueue only, excluded for poll(2) targets
            ("src/poll.rs", Os("hurd"), None),
            // The selector, the waker and the pipe module
            ("src/sys/unix/mod.rs", Os("hurd"), None),
            // `pipe2` with O_CLOEXEC | O_NONBLOCK
            ("src/sys/unix/pipe.rs", Os("hurd"), None),
            // `accept4` with SOCK_CLOEXEC | SOCK_NONBLOCK
            ("src/sys/unix/tcp.rs", Os("hurd"), None),
            // First is `new_socket` with flags in the type; the other two are
            // `sin_len` fields MikuOS lacks
            ("src/sys/unix/net.rs", Os("hurd"), Some(1)),
        ],
        "socket2" => &[
            // `SOCK_CLOEXEC` in the type, and no separate `FD_CLOEXEC` after it
            ("src/socket.rs", Os("hurd"), None),
            // `IovLen = c_int`, the options Hurd lacks, the keepalive interval
            ("src/sys/unix.rs", Os("hurd"), None),
        ],
        // tokio `peer_cred`: Hurd's `impl_noproc` reports uid and gid 0. MikuOS
        // has `peercred`(83) through `getsockopt(SO_PEERCRED)`, so it goes
        // beside Cygwin, in the Linux one
        "tokio" => &[
            ("src/net/unix/ucred.rs", Os("cygwin"), None),
        ],
        // `fd_guard.rs` uses `kcmp(KCMP_FILE)` and has a `compile_error!` for
        // unknown systems. Same path as FreeBSD (`libc::kcmp`)
        "inotify" => &[
            ("src/fd_guard.rs", Os("freebsd"), None),
        ],
        // Entropy through `libc::getrandom` and `errno` through
        // `__errno_location`, as on Hurd. Versions 0.3 and 0.4
        "getrandom" => &[
            // Its `libc` dependency is itself behind a list of targets
            ("?Cargo.toml", Os("hurd"), None),
            // Backend: `src/backends.rs` from 0.3, `src/lib.rs` in 0.2
            ("?src/backends.rs", Os("hurd"), None),
            ("?src/lib.rs", Os("hurd"), None),
            // The errno reader moved between the two lines
            ("?src/util_libc.rs", Os("hurd"), None),
            ("?src/utils/get_errno.rs", Os("hurd"), None),
        ],
        // Versions 0.30 and 0.28, whose files differ
        "nix" => &[
            ("?src/dir.rs", Os("hurd"), None),
            // Not Hurd: MikuOS's errno values are Linux's (ENOTSUP ==
            // EOPNOTSUPP), as for Fuchsia
            ("?src/errno.rs", Os("fuchsia"), None),
            ("?src/fcntl.rs", Os("hurd"), None),
            ("?src/features.rs", Os("hurd"), None),
            ("?src/sys/mman.rs", Os("hurd"), None),
            // Not Hurd: a `c_int` resource, as on Android
            ("?src/sys/resource.rs", Os("android"), None),
            // Not Hurd: MikuOS's signal set is Linux's, with SIGSTKFLT and
            // SIGPWR, as on Fuchsia
            ("?src/sys/signal.rs", Os("fuchsia"), None),
            ("?src/sys/socket/addr.rs", Os("hurd"), None),
            ("?src/sys/socket/mod.rs", Os("hurd"), None),
            ("?src/sys/socket/sockopt.rs", Os("hurd"), None),
            ("?src/sys/statvfs.rs", Os("hurd"), None),
            ("?src/time.rs", Os("hurd"), None),
            ("?src/unistd.rs", Os("hurd"), None),
        ],
        // Signal machine context. Illumos appears only in the x86_64 arms
        // indexing Linux's `gregs`, MikuOS's `ucontext_t` layout
        // (dev/signal-architecture.md)
        "wasmtime" => &[
            ("src/runtime/vm/sys/unix/signals.rs", Os("illumos"), None),
            // Registers JIT unwind tables with libgcc or libunwind. MikuOS has
            // neither and uses panic=abort; stubbed as for 32-bit ARM
            ("src/runtime/vm/sys/unix/unwind.rs", Arch("arm"), Some(1)),
        ],
        // `wasmtime-wasi` filesystem. No `O_PATH`, so beside systems opening
        // directories for reading (Solaris), and no `O_RSYNC` (beside Fuchsia)
        "cap-primitives" => &[
            ("src/rustix/fs/dir_utils.rs", Os("solaris"), None),
            ("src/rustix/fs/oflags.rs", Os("fuchsia"), None),
        ],
        // `O_RSYNC` in `wasmtime-wasi`'s open flags: beside FreeBSD
        "wasmtime-wasi" => &[
            ("src/filesystem/unix.rs", Os("freebsd"), None),
        ],
        // Systems where `SystemRandom` uses `getrandom`: beside Redox
        "ring" => &[
            ("src/rand.rs", Os("redox"), None),
            // Assembly: `build.rs` generates ELF perlasm for systems in
            // `LINUX_ABI`. MikuOS is SysV x86_64 ELF
            ("build.rs", Str("redox"), None),
        ],
        // Enum `OperatingSystem`: name, parse, and calling convention.
        // cranelift's build script panics on an unknown OS
        "target-lexicon" => &[
            ("src/targets.rs", Dup("Hurd"), None),
            ("src/triple.rs", Dup("Hurd"), None),
        ],
        // `calloop` exposes its poller's descriptor on all unixes, but
        // `polling`'s `poll(2)` backend has none
        "calloop" => &[
            ("src/loop_logic.rs", UnixBut("impl<Data> AsRawFd for EventLoop"), None),
            ("src/loop_logic.rs", UnixBut("impl<Data> AsFd for EventLoop"), None),
        ],
        // aosh's QuickJS. Bindings are named by target triple and identical on
        // Linux glibc and musl, so MikuOS uses the x86_64 Linux ones
        "rquickjs-sys" => &[
            ("src/bindings/x86_64-unknown-miku.rs", Copy("src/bindings/x86_64-unknown-linux-gnu.rs"), None),
        ],
        // Constructors via `.init_array`, called by `ld-miku`. Unknown systems
        // silently get no section
        "inventory" => &[
            ("src/lib.rs", Os("freebsd"), None),
        ],
        "ctor" => &[
            ("src/lib.rs", Os("freebsd"), None),
        ],
        // `backtrace` uses libunwind on unix; MikuOS has none, so the `noop`
        // backend, as for Emscripten
        "backtrace" => &[
            ("src/backtrace/mod.rs", Os("emscripten"), Some(1)),
        ],
        // Zed's own crates, from the copy's root. Beside FreeBSD where it means
        // the Linux desktop, not FreeBSD downloads or names
        "zed" => &[
            ("crates/gpui/Cargo.toml", Os("freebsd"), None),
            ("crates/gpui/src/app.rs", Os("freebsd"), None),
            // The dispatcher's priority queue, listed for Linux alone
            ("crates/gpui/src/gpui.rs", Os("linux"), None),
            ("crates/gpui/src/keymap/context.rs", Os("freebsd"), None),
            ("crates/gpui/src/platform.rs", Os("freebsd"), None),
            ("crates/gpui/src/platform/keystroke.rs", Os("freebsd"), None),
            ("crates/gpui/src/platform/test/platform.rs", Os("freebsd"), None),
            ("crates/gpui/src/scene.rs", Os("freebsd"), None),
            ("crates/gpui/src/svg_renderer.rs", Os("freebsd"), None),
            ("crates/gpui/src/window.rs", Os("freebsd"), None),
            // Calls: `livekit_client` is its mock with no WebRTC build
            ("crates/livekit_client/src/lib.rs", Os("freebsd"), None),
            ("crates/call/src/call_impl/diagnostics.rs", Os("freebsd"), None),
            // `audio`'s echo canceller is libwebrtc's, left out where calls are
            ("crates/audio/Cargo.toml", Os("freebsd"), None),
            ("crates/audio/src/audio_pipeline/echo_canceller.rs", Os("freebsd"), None),
            ("crates/markdown/src/markdown.rs", Os("freebsd"), None),
            ("crates/paths/src/paths.rs", Os("freebsd"), None),
            ("crates/settings_content/src/title_bar.rs", Os("freebsd"), None),
            ("crates/system_specs/Cargo.toml", Os("freebsd"), None),
            ("crates/system_specs/src/system_specs.rs", Os("freebsd"), None),
            ("crates/terminal/src/terminal.rs", Os("freebsd"), None),
            ("crates/ui/src/styles/platform.rs", Os("freebsd"), None),
            ("crates/util/src/paths.rs", Os("freebsd"), None),
            ("crates/util/src/util.rs", Os("freebsd"), None),
            ("crates/zlog/src/filter.rs", Os("freebsd"), None),
            // inotify watcher and window icon. Not `main.rs`, whose FreeBSD
            // arms use ashpd and the datagram instance check
            // (`zed-patches/zed.patch`)
            ("crates/zed/src/zed.rs", Os("freebsd"), None),
            // VS Code settings: a non-macOS unix reads the `linux` keys
            ("crates/settings/src/vscode_import.rs", Os("linux"), None),
        ],
        _ => return None,
    })
}

/// crates.io crates with `table` rows, in overlay order. `calloop` is a git
/// fork and `zed` is Zed itself, both handled elsewhere
pub const BESIDE_CRATES: &[&str] = &[
    "mio", "socket2", "tokio", "inotify", "getrandom", "target-lexicon", "nix",
    "wasmtime", "cap-primitives", "wasmtime-wasi", "ring", "inventory", "ctor",
    "backtrace", "rquickjs-sys",
];

/// Put MikuOS beside its neighbours in every file `krate`'s rows name
pub fn beside(krate: &str, root: &Path) -> R {
    let rows = table(krate).ok_or_else(|| format!("no arms for {krate}"))?;
    for &(rel, near, count) in rows {
        let (rel, optional) = match rel.strip_prefix('?') {
            Some(r) => (r, true),
            None => (rel, false),
        };
        if optional && !root.join(rel).exists() { continue; }
        match near {
            Dup(v) => dup_variant(krate, root, rel, v)?,
            UnixBut(a) => unix_but(krate, root, rel, a)?,
            Str(n) => in_str_list(krate, root, rel, n)?,
            Copy(src) => {
                let s = root.join(src);
                if !s.exists() {
                    return Err(format!("{krate} {src}: not there - the crate has changed shape"));
                }
                if !root.join(rel).exists() {
                    fs::copy(&s, root.join(rel)).map_err(|e| format!("{krate} {rel}: {e}"))?;
                }
            }
            Os(_) | Arch(_) => in_cfg_lists(krate, root, rel, near, count, optional)?,
        }
    }
    eprintln!("{krate}: MikuOS arms applied");
    Ok(())
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

fn in_cfg_lists(krate: &str, root: &Path, rel: &str, near: Near, count: Option<usize>, optional: bool) -> R {
    let s = read(root, rel)?;
    if s.contains(MIKU) { return Ok(()); }
    let entry = match near {
        Arch(a) => format!("target_arch = \"{a}\""),
        Os(o) => format!("target_os = \"{o}\""),
        _ => unreachable!(),
    };
    let room = |done: usize| count.map_or(true, |c| done < c);
    let mut out: Vec<String> = Vec::new();
    let mut done = 0;
    for line in s.split('\n') {
        let t = line.trim();
        // Both forms of a list entry: `target_os = "x",` and, last in a list,
        // `target_os = "x"`, which then needs a comma
        if t.trim_end_matches(',') == entry && room(done) {
            let indent = indent_of(line);
            if !t.ends_with(',') {
                out.push(format!("{line},"));
                out.push(format!("{indent}{MIKU}"));
            } else {
                out.push(line.to_string());
                out.push(format!("{indent}{MIKU},"));
            }
            done += 1;
        } else if line.contains(&entry) && room(done) {
            // One-line forms: a whole `cfg`, or `not(target_os = "x")` in a
            // longer one. Inside `any(...)` MikuOS joins the list, otherwise
            // the condition becomes one
            if line.contains("any(") {
                out.push(line.replacen(&entry, &format!("{entry}, {MIKU}"), 1));
            } else {
                out.push(line.replacen(&entry, &format!("any({entry}, {MIKU})"), 1));
            }
            done += 1;
        } else {
            out.push(line.to_string());
        }
    }
    if done == 0 {
        // An optional file may not name the target
        if optional { return Ok(()); }
        return Err(format!("{krate} {rel}: no `{entry}` found - the crate has changed shape"));
    }
    write(root, rel, &out.join("\n"))
}

fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' }

/// Replaces `word` as a whole word with `with`; `None` if absent
fn replace_word(line: &str, word: &str, with: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = line;
    let mut hit = false;
    let mut prev: Option<char> = None;
    while let Some(i) = rest.find(word) {
        let before = rest[..i].chars().last().or(prev);
        let after = rest[i + word.len()..].chars().next();
        out.push_str(&rest[..i]);
        if before.map_or(true, |c| !is_word(c)) && after.map_or(true, |c| !is_word(c)) {
            out.push_str(with);
            hit = true;
        } else {
            out.push_str(word);
        }
        prev = word.chars().last();
        rest = &rest[i + word.len()..];
    }
    out.push_str(rest);
    hit.then_some(out)
}

/// Each line naming `variant` as a whole word gets a twin naming `Miku`,
/// lowercase included
fn dup_variant(krate: &str, root: &Path, rel: &str, variant: &str) -> R {
    let s = read(root, rel)?;
    if s.split('\n').any(|l| replace_word(l, "Miku", "Miku").is_some()) { return Ok(()); }
    let lower = format!("\"{}\"", variant.to_lowercase());
    let mut out: Vec<String> = Vec::new();
    let mut done = 0;
    for line in s.split('\n') {
        out.push(line.to_string());
        if let Some(twin) = replace_word(line, variant, "Miku") {
            out.push(twin.replace(&lower, "\"miku\""));
            done += 1;
        }
    }
    if done == 0 {
        return Err(format!("{krate} {rel}: no `{variant}` found - the crate has changed shape"));
    }
    write(root, rel, &out.join("\n"))
}

/// `#[cfg(unix)]` over the item starting with `anchor` (skipping docs and
/// attributes) becomes `#[cfg(all(unix, not(target_os = "miku")))]`
fn unix_but(krate: &str, root: &Path, rel: &str, anchor: &str) -> R {
    let s = read(root, rel)?;
    let mut lines: Vec<String> = s.split('\n').map(String::from).collect();
    let at = lines.iter().position(|l| l.trim().starts_with(anchor))
        .ok_or_else(|| format!("{krate} {rel}: no `{anchor}` found - the crate has changed shape"))?;
    let not_under = || format!("{krate} {rel}: `{anchor}` is not under `#[cfg(unix)]`");
    let mut i = at;
    loop {
        if i == 0 { return Err(not_under()); }
        i -= 1;
        let t = lines[i].trim();
        if t == "#[cfg(unix)]" { break; }
        if t.contains(MIKU) { return Ok(()); }
        if !(t.starts_with("///") || t.starts_with("#[")) { return Err(not_under()); }
    }
    lines[i] = lines[i].replace("#[cfg(unix)]", &format!("#[cfg(all(unix, not({MIKU})))]"));
    write(root, rel, &lines.join("\n"))
}

/// A build script's bare target list: `"miku",` goes after `name`
fn in_str_list(krate: &str, root: &Path, rel: &str, name: &str) -> R {
    let s = read(root, rel)?;
    let lines: Vec<&str> = s.split('\n').collect();
    if lines.iter().any(|l| l.trim().trim_end_matches(',') == "\"miku\"") { return Ok(()); }
    let quoted = format!("\"{name}\"");
    let mut out: Vec<String> = Vec::new();
    let mut done = 0;
    for line in lines {
        out.push(line.to_string());
        if line.trim().trim_end_matches(',') == quoted {
            let indent = indent_of(line).to_string();
            if !line.trim_end().ends_with(',') {
                *out.last_mut().unwrap() = format!("{},", line.trim_end());
            }
            out.push(format!("{indent}\"miku\","));
            done += 1;
        }
    }
    if done == 0 {
        return Err(format!("{krate} {rel}: no `\"{name}\"` found - the crate has changed shape"));
    }
    write(root, rel, &out.join("\n"))
}
