//! Builds one crate against MikuOS's `std`: points cargo at the patched
//! `library/` (`overlay`) and at the JSON target spec whose `"os": "miku"`
//! defines `target_os = "miku"`

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{cheaders, fsx};

pub struct Build<'a> {
    pub root: &'a Path,
    /// The crate, or the workspace, to run cargo in
    pub dir: &'a Path,
    /// `overlay::assemble`'s output
    pub overlay: &'a Path,
    /// `build`, or `check` for a graph too large to build while it is ported
    pub cargo: &'a str,
    /// Cargo target directory, default `<dir>/target`
    pub target_dir: Option<PathBuf>,
    pub args: Vec<String>,
}

/// Builds the link stubs first. `lib/sdk/miku/stubs/` comes from `lib/sdk/miku`'s build
/// script, a crate `std` programs do not depend on. With LTO a missing stub
/// links successfully and drops the call
fn link_stubs(root: &Path) -> Result<(), String> {
    let _ = Command::new("cargo")
        .current_dir(root.join("lib/sdk/miku"))
        .args(["+nightly", "build", "--release", "--target"])
        .arg(root.join("lib/sdk/miku/x86_64-miku-app.json"))
        .args(["-Z", "json-target-spec", "-Z", "build-std=core,alloc",
               "-Z", "build-std-features=compiler-builtins-mem"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !root.join("lib/sdk/miku/stubs").is_dir() {
        return Err("lib/sdk/miku/stubs/ was not generated - the link would fail".into());
    }
    Ok(())
}

pub fn run(b: &Build) -> Result<(), String> {
    let root = b.root;
    link_stubs(root)?;

    // Cargo fingerprints `-Z build-std` sysroot sources by path, not content.
    // So the patch trees and spec are hashed, and a change removes the target
    // directory. The stamp is written only after a successful build
    let spec = root.join("lib/sdk/rust-std/x86_64-unknown-miku.json");
    let base = b.target_dir.clone().unwrap_or_else(|| b.dir.join("target"));
    let tdir = base.join("x86_64-unknown-miku");
    let stamp = base.join(".miku-std-patch");
    let mut patch_files = fsx::files_sorted(&root.join("lib/sdk/rust-std/library"));
    patch_files.extend(fsx::files_sorted(&root.join("lib/abi/src")));
    patch_files.extend(fsx::files_sorted(&root.join("lib/sdk/rust-libc")));
    patch_files.push(spec.clone());
    let hash = fsx::hash_files(&patch_files);
    if fs::read_to_string(&stamp).ok().as_deref().map(str::trim) != Some(hash.as_str()) {
        if tdir.exists() { println!("std patch changed - rebuilding it"); }
        let _ = fs::remove_dir_all(&tdir);
    }

    // C toolchain for crates with C (SQLite, zstd, tree-sitter, FreeType,
    // QuickJS), read per target by `cc`. Headers are generated and checked
    // against the stubs first. `cc` fingerprints only its environment, so the
    // header hash goes into CFLAGS
    cheaders::generate(root)?;
    cheaders::check_declarations(root)?;
    cheaders::check_signatures(root)?;
    let mut c_files = fsx::files_sorted(&root.join("lib/sdk/c"));
    c_files.push(root.join("lib/sdk/rust-libc/libc/src/unix/miku/mod.rs"));
    let c_hash: String = fsx::hash_files(&c_files).chars().take(16).collect();

    // Absolute linker script and stub paths, since relative ones resolve
    // against the workspace being built. `--check-cfg` declares
    // `target_os = "miku"`, which rustc would otherwise flag as an error in
    // crates that deny it. Also for the host: with `--target`, build scripts
    // and proc-macros take only `host.rustflags`
    let check_cfg = r#""--check-cfg=cfg(target_os,values(\"miku\"))""#;
    let target_flags = format!(
        r#"target.x86_64-unknown-miku.rustflags=["-Clink-arg=-T{}", "-Clink-arg=-L{}", {check_cfg}]"#,
        root.join("lib/sdk/miku/linker.ld").display(), root.join("lib/sdk/miku/stubs").display());

    let mut cmd = Command::new("cargo");
    cmd.current_dir(b.dir)
        .env("__CARGO_TESTS_ONLY_SRC_ROOT", b.overlay.join("library"))
        .env("CC_x86_64_unknown_miku", root.join("lib/sdk/c/miku-cc"))
        .env("AR_x86_64_unknown_miku", "llvm-ar")
        // `MDB_USE_ROBUST=0`: LMDB without robust mutexes. `NO_TM_GMTOFF`:
        // QuickJS without `tm_gmtoff`, since MikuOS's `struct tm` has the nine
        // POSIX fields (`lib/sdk/c/check/layout.c`)
        .env("CFLAGS_x86_64_unknown_miku",
             format!("-DMIKU_C_HEADERS=0x{c_hash} -DMDB_USE_ROBUST=0 -DNO_TM_GMTOFF"))
        .args(["+nightly", b.cargo, "--release", "--target"])
        .arg(&spec)
        .args(["-Z", "json-target-spec", "-Z", "build-std=std,panic_abort",
               "-Z", "build-std-features=compiler-builtins-mem"])
        .arg("--config").arg(target_flags)
        .args(["-Z", "host-config", "-Z", "target-applies-to-host",
               "--config", "target-applies-to-host=false"])
        .arg("--config").arg(format!("host.rustflags=[{check_cfg}]"))
        .args(&b.args);
    if let Some(t) = &b.target_dir {
        cmd.env("CARGO_TARGET_DIR", t);
    }
    let ok = cmd.status().map_err(|e| format!("cannot run cargo: {e}"))?.success();
    if !ok {
        return Err(format!("cargo {} failed in {}", b.cargo, b.dir.display()));
    }
    let _ = fs::create_dir_all(&base);
    fs::write(&stamp, format!("{hash}\n")).map_err(|e| format!("{}: {e}", stamp.display()))?;
    Ok(())
}
