//! Sources of overlay crates, refetched when cargo cleans them
//!
//! The overlay copies crates from `~/.cargo/registry/src` and programs build
//! the copies by path, so cargo never counts the originals as used. Since 1.88
//! it deletes extracted sources unused for a month and `.crate` files unused
//! for three (`cargo gc`).
//!
//! `cargo fetch` on a throwaway manifest of the versions the lock pins restores
//! them: offline first, then over the network if allowed. Cargo records the use
//! either way.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cargo_home() -> PathBuf {
    env::var_os("CARGO_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".cargo"))
}

/// Where cargo keeps the checkouts of git dependencies
pub fn cargo_git_checkouts() -> PathBuf {
    cargo_home().join("git/checkouts")
}

/// `1.2.3` -> `1`, `0.2.191` -> `0.2`: one semver line
pub fn semver_line(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.first() == Some(&"0") && parts.len() > 1 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        parts[0].to_string()
    }
}

/// Key for `sort -V`: numbers compare as numbers
fn version_key(v: &str) -> Vec<(u64, String)> {
    v.split(|c: char| c == '.' || c == '-' || c == '+')
        .map(|p| {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            (digits.parse().unwrap_or(0), p[digits.len()..].to_string())
        })
        .collect()
}

/// Unpacked copies of `name` whose version starts with `prefix` ("" for all),
/// oldest first
pub fn crate_dirs(name: &str, prefix: &str) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(indexes) = fs::read_dir(cargo_home().join("registry/src")) else { return out };
    let lead = format!("{name}-");
    for idx in indexes.flatten() {
        let Ok(rd) = fs::read_dir(idx.path()) else { continue };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(ver) = fname.strip_prefix(&lead) else { continue };
            // `wasmtime-` must not match `wasmtime-wasi-48.0.2`
            if !ver.starts_with(|c: char| c.is_ascii_digit()) || !ver.starts_with(prefix) { continue; }
            if e.path().is_dir() { out.push((ver.to_string(), e.path())); }
        }
    }
    out.sort_by(|a, b| version_key(&a.0).cmp(&version_key(&b.0)));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// One `[[package]]` of a `Cargo.lock`
pub struct Locked {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
}

pub fn read_lock(path: &Path) -> Vec<Locked> {
    let Ok(text) = fs::read_to_string(path) else { return Vec::new() };
    let mut out: Vec<Locked> = Vec::new();
    let mut cur: Option<Locked> = None;
    let field = |l: &str, k: &str| -> Option<String> {
        l.strip_prefix(k)?.strip_prefix(" = \"")?.strip_suffix('"').map(String::from)
    };
    for line in text.lines() {
        if line == "[[package]]" {
            if let Some(p) = cur.take() { out.push(p); }
            cur = Some(Locked { name: String::new(), version: String::new(), source: None });
        } else if let Some(p) = cur.as_mut() {
            if let Some(v) = field(line, "name") { p.name = v; }
            else if let Some(v) = field(line, "version") { p.version = v; }
            else if let Some(v) = field(line, "source") { p.source = Some(v); }
        }
    }
    if let Some(p) = cur { out.push(p); }
    out
}

/// Pinned versions of `names` that are not unpacked
fn missing(locks: &[PathBuf], names: &[&str]) -> BTreeSet<(String, String)> {
    let mut want = BTreeSet::new();
    for lock in locks {
        for p in read_lock(lock) {
            // Skips git sources (Zed's `calloop`)
            if p.source.as_deref().is_some_and(|s| s.starts_with("git+")) { continue; }
            if names.contains(&p.name.as_str()) && !p.version.is_empty() {
                want.insert((p.name, p.version));
            }
        }
    }
    want.retain(|(n, v)| !crate_dirs(n, "").iter().any(|(have, _)| have == v));
    want
}

/// Restores the pinned sources of `names`. `online` allows downloads. Returns
/// what is still missing, as `name version`
pub fn ensure(locks: &[PathBuf], names: &[&str], online: bool) -> Vec<String> {
    let want = missing(locks, names);
    if want.is_empty() { return Vec::new(); }
    println!("[*] crate sources cargo has cleaned away: {}",
        want.iter().map(|(n, v)| format!("{n} {v}")).collect::<Vec<_>>().join(", "));

    // One manifest cannot hold two versions of a semver line, so they spread
    // over several
    let mut groups: Vec<Vec<(String, String)>> = Vec::new();
    for (n, v) in &want {
        let line = semver_line(v);
        match groups.iter_mut().find(|g| !g.iter().any(|(gn, gv)| gn == n && semver_line(gv) == line)) {
            Some(g) => g.push((n.clone(), v.clone())),
            None => groups.push(vec![(n.clone(), v.clone())]),
        }
    }

    // Outside the repository, whose `.cargo/config.toml` sets the kernel's
    // target and `build-std`
    let base = env::temp_dir().join(format!("miku-stdport-fetch-{}", std::process::id()));
    for (i, g) in groups.iter().enumerate() {
        let dir = base.join(i.to_string());
        let _ = fs::create_dir_all(dir.join("src"));
        let mut toml = String::from(
            "[package]\nname = \"miku-stdport-fetch\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
             # Its own workspace, so no directory above can claim it\n[workspace]\n\n[dependencies]\n");
        for (n, v) in g {
            let key = format!("{}_{}", n.replace('-', "_"), v.replace(['.', '-', '+'], "_"));
            toml.push_str(&format!("{key} = {{ package = \"{n}\", version = \"={v}\" }}\n"));
        }
        let _ = fs::write(dir.join("Cargo.toml"), toml);
        let _ = fs::write(dir.join("src/lib.rs"), "");
        let fetch = |offline: bool| {
            let mut c = Command::new("cargo");
            // Host platform only; otherwise `cargo fetch` pulls every
            // platform's dependencies
            c.current_dir(&dir).args(["+nightly", "fetch", "--target", host_triple().as_str()]);
            if offline { c.arg("--offline"); }
            c.stdout(std::process::Stdio::null());
            if offline { c.stderr(std::process::Stdio::null()); }
            c.status().map(|s| s.success()).unwrap_or(false)
        };
        if !fetch(true) && online {
            println!("[*] downloading {}", g.iter().map(|(n, v)| format!("{n} {v}")).collect::<Vec<_>>().join(", "));
            fetch(false);
        }
    }
    let _ = fs::remove_dir_all(&base);

    missing(locks, names).into_iter().map(|(n, v)| format!("{n} {v}")).collect()
}

fn host_triple() -> String {
    Command::new("rustc").args(["+nightly", "-vV"]).output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("host: ").map(String::from)))
        .unwrap_or_else(|| "x86_64-unknown-linux-gnu".to_string())
}
