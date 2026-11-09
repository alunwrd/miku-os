//! Three directories, in order of authority, the same as
//! `lib/gfx/mayland/src/manifest.rs`.
//!
//! ```text
//! /etc/mikupkg/default.d/*.conf   what the build ships and rewrites every image
//! /etc/mikupkg/mikupkg.conf       this machine
//! /etc/mikupkg/conf.d/*.conf      yours; nothing is ever installed here
//! ```
//!
//! Later wins; within a directory, name order (`10-`, `50-`, `99-`). Only
//! `*.conf` is read.
//!
//! A file in `conf.d` with the name of one in `default.d` replaces it, so an
//! empty file switches a default off. A package may own files under `default.d`
//! and never under `conf.d`, so a package file and a user file never share a
//! path.

use alloc::string::String;
use alloc::vec::Vec;

use super::grammar;
use super::note::Note;
use super::Config;

/// Source of configuration, as a trait so the layering can be tested against a
/// table
pub trait Files {
    /// Names inside a directory, without the path, in any order
    fn list(&self, dir: &str) -> Vec<String>;
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

pub const DEFAULT_D: &str = "default.d";
pub const CONF_D: &str = "conf.d";
pub const MAIN: &str = "mikupkg.conf";

/// The files that make up the configuration, in the order they are applied.
pub fn files_in_order<F: Files + ?Sized>(files: &F, etc: &str) -> Vec<String> {
    let confs = |dir: &str| -> Vec<String> {
        let mut v: Vec<String> = files
            .list(dir)
            .into_iter()
            .filter(|n| n.ends_with(".conf"))
            .collect();
        v.sort();
        v
    };

    let default_dir = alloc::format!("{etc}/{DEFAULT_D}");
    let conf_dir = alloc::format!("{etc}/{CONF_D}");
    let mine = confs(&conf_dir);

    let mut out = Vec::new();
    for name in confs(&default_dir) {
        // Replaced, not added to, so an empty file in `conf.d` is an off switch
        if mine.contains(&name) {
            continue;
        }
        out.push(alloc::format!("{default_dir}/{name}"));
    }
    let main = alloc::format!("{etc}/{MAIN}");
    if files.read(&main).is_some() {
        out.push(main);
    }
    for name in mine {
        out.push(alloc::format!("{conf_dir}/{name}"));
    }
    out
}

/// Reads everything. Returns the configuration and every complaint. A fatal
/// note means the configuration must not be used
pub fn load<F: Files + ?Sized>(files: &F, etc: &str) -> (Config, Vec<Note>) {
    let mut cfg = Config::default();
    let mut notes = Vec::new();
    for path in files_in_order(files, etc) {
        if let Some(bytes) = files.read(&path) {
            grammar::parse_into(&path, &bytes, &mut cfg, &mut notes);
        }
    }
    (cfg, notes)
}
