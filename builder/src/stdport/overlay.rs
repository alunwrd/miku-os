//! Assembles the patched trees for `std` programs: Rust's `library/` with
//! `lib/sdk/rust-std/library/` over it, and crates.io crates patched by `arms`
//!
//! `lib/sdk/rust-std/library/` mirrors upstream paths and holds only MikuOS's
//! changes. Upstream is the toolchain's `rust-src`, the exact version that will
//! compile it. `refsrc/miku_programs/rust` is pinned to the same commit.
//!
//! One output directory per consumer pinning its own versions:
//! `target/rust-src-miku` for `stdapps` and aosh, and Zed's own (`zed.rs`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::arms;
use super::fsx::mirror;
use super::sources::{crate_dirs, read_lock, semver_line};

fn io<T>(r: std::io::Result<T>, what: &Path) -> Result<T, String> {
    r.map_err(|e| format!("{}: {e}", what.display()))
}

/// The toolchain's `library/`
pub fn rust_src() -> Result<PathBuf, String> {
    let out = Command::new("rustc").args(["+nightly", "--print", "sysroot"]).output()
        .map_err(|e| format!("cannot run rustc: {e}"))?;
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let src = Path::new(&sysroot).join("lib/rustlib/src/rust/library");
    if !src.is_dir() {
        return Err("rust-src is not installed: rustup component add rust-src --toolchain nightly".into());
    }
    Ok(src)
}

/// Builds the trees in `out`. With `lock`, each crate at its pinned version if
/// present; otherwise the newest of each semver line
pub fn assemble(root: &Path, out: &Path, lock: Option<&Path>) -> Result<PathBuf, String> {
    let src = rust_src()?;
    let lib = out.join("library");
    // Deleting, so a file removed from the patch stops applying
    io(mirror(&src, &lib, true, &[]), &lib)?;
    let patch = root.join("lib/sdk/rust-std/library");
    if patch.is_dir() {
        io(mirror(&patch, &lib, false, &[]), &lib)?;
    }
    let _ = fs::remove_file(lib.join("Cargo.lock"));
    abi_module(root, &lib.join("std/src/sys/pal/miku/defs"), &["kstat"])?;

    libc(root, out)?;
    rustix(out)?;
    beside_crates(out, lock)?;
    calloop(out, lock)?;
    Ok(out.to_path_buf())
}

/// The kernel's own shapes in `lib/abi`, which C's view has no use for
const KERNEL_SHAPES: &[&str] =
    &["auxv", "cred", "elf", "futex", "kproc", "ksignal", "kstat", "ktermios", "libtypes", "sys", "tcb", "vvar"];

/// `lib/abi/src` as a module at `dst`, because `-Z build-std` takes no path
/// dependency: `lib.rs` becomes `mod.rs` without its crate attributes. `keep`
/// names the kernel-shape files this tree uses
fn abi_module(root: &Path, dst: &Path, keep: &[&str]) -> Result<(), String> {
    let src = root.join("lib/abi/src");
    io(mirror(&src, dst, true, &[]), dst)?;
    let dropped = |m: &str| KERNEL_SHAPES.contains(&m) && !keep.contains(&m);
    let text = io(fs::read_to_string(src.join("lib.rs")), &src)?;
    let module: Vec<&str> = text.lines()
        .filter(|l| !l.starts_with("//!") && !l.starts_with("#!["))
        .filter(|l| !l.strip_prefix("pub mod ").and_then(|m| m.strip_suffix(';')).is_some_and(dropped))
        .collect();
    let path = dst.join("mod.rs");
    io(fs::write(&path, module.join("\n") + "\n"), &path)?;
    let _ = fs::remove_file(dst.join("lib.rs"));
    for m in KERNEL_SHAPES.iter().filter(|m| dropped(m)) {
        let _ = fs::remove_file(dst.join(format!("{m}.rs")));
    }
    Ok(())
}

/// The `libc` crate. MikuOS is `target_family = "unix"`, so everything under
/// `cfg(unix)` calls it. `std` does not use it but depends on it, through
/// `library/Cargo.toml`'s `[patch.crates-io]`.
///
/// The newest present, since a patch must satisfy both `std`'s `^0.2.185` and
/// what programs pull in
fn libc(root: &Path, out: &Path) -> Result<(), String> {
    let Some((_, src)) = crate_dirs("libc", "0.2.").pop() else { return Ok(()) };
    let dst = out.join("libc-miku");
    io(mirror(&src, &dst, true, &[]), &dst)?;
    let patch = root.join("lib/sdk/rust-libc/libc");
    if patch.is_dir() {
        io(mirror(&patch, &dst, false, &[]), &dst)?;
    }
    let abi = dst.join("src/unix/miku/abi");
    abi_module(root, &abi, &[])?;
    // Upstream owns these, and `libc::msghdr` must hold a `libc::iovec`
    let posix = abi.join("posix.rs");
    io(fs::write(&posix, "pub use crate::unix::{\n    group, in6_addr, iovec, pollfd, rlimit, rusage, timespec, timeval, winsize,\n};\n"), &posix)?;
    arms::libc(&dst)?;
    // Adds the patch entry once, here since the path is only known now
    let manifest = out.join("library/Cargo.toml");
    let text = io(fs::read_to_string(&manifest), &manifest)?;
    if !text.lines().any(|l| l.starts_with("libc = ")) {
        let mut t = text;
        t.push_str("\n# MikuOS: see lib/sdk/rust-libc/README.md\nlibc = { path = \"../libc-miku\" }\n");
        io(fs::write(&manifest, t), &manifest)?;
    }
    Ok(())
}

/// `rustix`: under `polling`, `async-io`, and `smol` on every unix, with `cfg`
/// chains naming targets and no fallback
fn rustix(out: &Path) -> Result<(), String> {
    let Some((_, src)) = crate_dirs("rustix", "1.").pop() else { return Ok(()) };
    let dst = out.join("rustix-miku");
    io(mirror(&src, &dst, true, &[]), &dst)?;
    arms::rustix(&dst)
}

/// Every crate in `arms`, at every semver line present, since Zed's graph holds
/// `getrandom` 0.3 and 0.4 and a patch only replaces a satisfied line. The
/// newest goes to `<crate>-miku`, older ones to `<crate>-<line>-miku`
fn beside_crates(out: &Path, lock: Option<&Path>) -> Result<(), String> {
    let locked = lock.map(read_lock).unwrap_or_default();
    for krate in arms::BESIDE_CRATES {
        let all = crate_dirs(krate, "");
        let Some((newest_ver, _)) = all.last() else { continue };
        let newest_line = semver_line(newest_ver);
        let mut lines: Vec<String> = all.iter().map(|(v, _)| semver_line(v)).collect();
        lines.dedup();
        for line in lines {
            let in_line: Vec<&(String, PathBuf)> = all.iter().filter(|(v, _)| semver_line(v) == line).collect();
            let newest_src = in_line.last().unwrap().1.clone();
            // With a lock, the pinned version in this line if present. A newer
            // one would move the lock, and crates pinning siblings exactly
            // would pull in second copies
            let mut src = newest_src.clone();
            for p in locked.iter().filter(|p| p.name == *krate) {
                if let Some((_, d)) = in_line.iter().find(|(v, _)| *v == p.version) {
                    src = d.clone();
                }
            }
            if line == newest_line {
                let dst = out.join(format!("{krate}-miku"));
                io(mirror(&src, &dst, true, &[]), &dst)?;
                if src != newest_src {
                    // The newest is the fallback where the pinned version
                    // predates the arms
                    if arms::beside(krate, &dst).is_ok() { continue; }
                    io(mirror(&newest_src, &dst, true, &[]), &dst)?;
                }
                arms::beside(krate, &dst)?;
            } else {
                // An older line with a different layout stays unpatched, with a
                // message
                let dst = out.join(format!("{krate}-{line}-miku"));
                io(mirror(&src, &dst, true, &[]), &dst)?;
                if let Err(e) = arms::beside(krate, &dst) {
                    eprintln!("{e}");
                    eprintln!("overlay: {krate} {line} does not take the arms - left unpatched");
                    let _ = fs::remove_dir_all(&dst);
                }
            }
        }
    }
    Ok(())
}

/// A git crate: Zed's `calloop` fork, replacing crates.io's by
/// `[patch.crates-io]`. The checkout is the locked commit, or the newest
/// present; patched under the crates.io name
fn calloop(out: &Path, lock: Option<&Path>) -> Result<(), String> {
    let checkouts = super::sources::cargo_git_checkouts();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = fs::read_dir(&checkouts) {
        for e in rd.flatten() {
            if !e.file_name().to_string_lossy().starts_with("calloop-") { continue; }
            if let Ok(revs) = fs::read_dir(e.path()) {
                candidates.extend(revs.flatten().map(|r| r.path()).filter(|p| p.is_dir()));
            }
        }
    }
    let src = match lock {
        Some(l) => {
            let rev = read_lock(l).into_iter()
                .find(|p| p.name == "calloop")
                .and_then(|p| p.source)
                .and_then(|s| s.split_once('#').map(|(_, r)| r.chars().take(7).collect::<String>()));
            rev.and_then(|r| candidates.into_iter().find(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == r)))
        }
        None => candidates.into_iter()
            .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok()),
    };
    let Some(src) = src else { return Ok(()) };
    let dst = out.join("calloop-miku");
    io(mirror(&src, &dst, true, &[".git"]), &dst)?;
    arms::beside("calloop", &dst)
}
