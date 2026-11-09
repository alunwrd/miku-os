//! Builds programs against MikuOS's Rust `std`
//!
//! | module     | does |
//! |------------|------|
//! | `sources`  | refetches crate sources cargo cleaned, through `cargo fetch` |
//! | `overlay`  | the patched `library/` and patched copies of crates.io crates |
//! | `arms`     | MikuOS's name in those crates' target lists, by anchor |
//! | `cheaders` | generated C header constants, and a check of their
//! declarations |
//! | `compile`  | one crate built against the overlay, with the C toolchain |
//! | `zed`      | Zed from a patched copy of its clone |
//!
//! Used by `main` for the image (always aosh, `stdapps` on request) and by the
//! subcommands `std <crate>`, `zed build|check|tree`, and `overlay`.
//!
//! `lib/sdk/c/miku-cc` stays outside: `cc` invokes a compiler by path, so it must
//! be an executable.

mod arms;
mod cheaders;
mod compile;
mod fsx;
mod overlay;
mod sources;
mod zed;

use std::path::{Path, PathBuf};

/// The overlay `stdapps` and aosh name in their `[patch.crates-io]`
pub fn default_overlay(root: &Path) -> PathBuf {
    root.join("target/rust-src-miku")
}

/// Every crates.io crate the overlay patches
fn overlay_crates() -> Vec<&'static str> {
    let mut v = vec!["libc", "rustix"];
    v.extend(arms::BESIDE_CRATES);
    v
}

/// Prepares the overlay for `consumers` (crate directories with a
/// `Cargo.lock`): fetches the pinned sources if cargo cleaned them (over the
/// network when `online`) and assembles the trees. A source still missing is
/// reported here and fails the consumer's build
pub fn prepare(root: &Path, consumers: &[&str], online: bool) -> Result<PathBuf, String> {
    let locks: Vec<PathBuf> = consumers.iter().map(|c| root.join(c).join("Cargo.lock")).collect();
    let missing = sources::ensure(&locks, &overlay_crates(), online);
    if !missing.is_empty() {
        println!("[!] still no source for: {} - {}", missing.join(", "),
            if online { "the download failed" } else { "not downloaded" });
    }
    overlay::assemble(root, &default_overlay(root), None)
}

/// Build (or `check`) the crate in `dir` against an overlay `prepare` made
pub fn build(root: &Path, dir: &Path, overlay: &Path, check: bool, args: Vec<String>) -> Result<(), String> {
    compile::run(&compile::Build {
        root,
        dir,
        overlay,
        cargo: if check { "check" } else { "build" },
        target_dir: None,
        args,
    })
}

/// `zed build|check|tree [cargo args]`
pub fn zed(root: &Path, cmd: &str, args: &[String]) -> Result<(), String> {
    zed::run(root, cmd, args)
}
