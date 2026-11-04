//! Builds or checks Zed for MikuOS
//!
//!     cargo run --release -- zed build -p zed
//!     cargo run --release -- zed check -p remote_server
//!     cargo run --release -- zed tree -i getrandom
//!
//! Works on a copy, `target/zed-miku/src`, refreshed from
//! `refsrc/miku_programs/zed` on every run. `compile` supplies the target,
//! `-Z build-std`, and the C toolchain; overlay crates are patched with
//! `--config` flags, since the `Cargo.toml` is Zed's. Not part of an image
//! build; the builder packages the binary if present (`EXTERNAL_BINS`).

use std::fs;
use std::path::Path;
use std::process::Command;

use super::fsx::mirror;
use super::sources::{read_lock, semver_line};
use super::{arms, compile, overlay, sources};

pub fn run(root: &Path, cmd: &str, args: &[String]) -> Result<(), String> {
    if !matches!(cmd, "build" | "check" | "tree") {
        return Err("usage: zed build|check|tree [cargo args...]".into());
    }
    let clone = root.join("refsrc/miku_programs/zed");
    if !clone.join("Cargo.toml").exists() {
        return Err(format!("{} is not a Zed checkout", clone.display()));
    }
    let work = root.join("target/zed-miku/src");
    mirror(&clone, &work, true, &["target", ".git"]).map_err(|e| format!("{}: {e}", work.display()))?;

    // Zed-specific changes: one patch per concern in `zed-patches/`, applied to
    // the fresh copy, failing if upstream moved. `patch` rather than
    // `git apply`, which inside this repository silently skips paths outside
    // the current directory
    let mut patches: Vec<_> = fs::read_dir(root.join("lib/rust-std/zed-patches"))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "patch")).collect())
        .unwrap_or_default();
    patches.sort();
    for p in &patches {
        let ok = Command::new("patch").args(["-s", "-p1", "--fuzz=0", "--forward", "-d"]).arg(&work)
            .arg("-i").arg(p).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return Err(format!("zed: {} does not apply", p.file_name().unwrap().to_string_lossy()));
        }
    }
    // New files, such as gpui's Mayland backend: `zed-files/` mirrors the tree
    let files = root.join("lib/rust-std/zed-files");
    if files.is_dir() {
        mirror(&files, &work, false, &[]).map_err(|e| format!("{}: {e}", work.display()))?;
    }
    // Where Zed's code lists systems, MikuOS is added by anchor
    arms::beside("zed", &work)?;

    // Assembles overlays from the copy's pinned versions, fetching sources if
    // cargo cleaned them
    let lock = work.join("Cargo.lock");
    let mut names: Vec<&str> = vec!["libc", "rustix"];
    names.extend(arms::BESIDE_CRATES);
    let missing = sources::ensure(&[lock.clone()], &names, true);
    for m in &missing {
        eprintln!("[!] zed: no source of {m} on this machine - it is built unpatched");
    }
    let ov = root.join("target/zed-miku/rust-src");
    overlay::assemble(root, &ov, Some(&lock))?;

    // Each overlay as a `[patch.crates-io]` entry. The key is the directory
    // name, so an older line (`getrandom-0.3-miku`) has its own, with `package`
    // naming the crate
    let mut pflags: Vec<String> = Vec::new();
    let mut dirs: Vec<_> = fs::read_dir(&ov).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    dirs.sort();
    let mut overlaid: Vec<(String, String)> = Vec::new();
    for dir in &dirs {
        let dname = dir.file_name().unwrap().to_string_lossy().into_owned();
        let Some(stem) = dname.strip_suffix("-miku") else { continue };
        let Ok(manifest) = fs::read_to_string(dir.join("Cargo.toml")) else { continue };
        let first = |k: &str| manifest.lines().find_map(|l| {
            l.strip_prefix(k)?.strip_prefix(" = \"")?.strip_suffix('"').map(String::from)
        });
        let (Some(name), Some(ver)) = (first("name"), first("version")) else { continue };
        let key = stem.replace('.', "_");
        pflags.push("--config".into());
        pflags.push(format!("patch.crates-io.{key}.path=\"{}\"", dir.display()));
        if key != name {
            pflags.push("--config".into());
            pflags.push(format!("patch.crates-io.{key}.package=\"{name}\""));
        }
        overlaid.push((name, ver));
    }

    // A patch only replaces an equal locked version, so the lock moves to the
    // overlaid ones
    let locked = read_lock(&lock);
    for (name, ver) in &overlaid {
        for p in locked.iter().filter(|p| &p.name == name) {
            if p.version != *ver && semver_line(&p.version) == semver_line(ver) {
                let ok = Command::new("cargo").current_dir(&work)
                    .args(["+nightly", "update"]).args(&pflags)
                    .args(["-p", &format!("{name}@{}", p.version), "--precise", ver, "-q"])
                    .status().map(|s| s.success()).unwrap_or(false);
                if !ok { eprintln!("zed: could not move {name} {} to {ver}", p.version); }
            }
        }
    }

    let spec = root.join("lib/rust-std/x86_64-unknown-miku.json");
    if cmd == "tree" {
        let ok = Command::new("cargo").current_dir(&work)
            .args(["+nightly", "tree", "-Z", "json-target-spec", "--target"]).arg(&spec)
            .args(&pflags).args(args)
            .status().map(|s| s.success()).unwrap_or(false);
        return if ok { Ok(()) } else { Err("cargo tree failed".into()) };
    }

    // Profile for a 16 GB machine: 6 jobs (`ZED_JOBS`), 16 codegen units, no
    // LTO, no debug info. `ZED_FULL_PROFILE=1` builds upstream's profile
    let mut extra = pflags;
    if std::env::var_os("ZED_FULL_PROFILE").is_none() {
        let jobs = std::env::var("ZED_JOBS").unwrap_or_else(|_| "6".into());
        extra.extend(["-j".into(), jobs,
            "--config".into(), "profile.release.codegen-units=16".into(),
            "--config".into(), "profile.release.lto=false".into(),
            "--config".into(), "profile.release.debug=false".into()]);
    }
    extra.extend(args.iter().cloned());
    compile::run(&compile::Build {
        root,
        dir: &work,
        overlay: &ov,
        cargo: if cmd == "check" { "check" } else { "build" },
        target_dir: Some(root.join("target/zed-miku/target")),
        args: extra,
    })
}
