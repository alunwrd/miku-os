//! `libs.list`: `<library>: <dependency> <dependency> ...`

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Lib {
    pub name: String,
    pub deps: Vec<String>,
}

/// Parses `<root>/libs.list`
pub fn read(root: &Path) -> Vec<Lib> {
    let path = root.join("libs.list");
    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut out = Vec::new();
    for line in src.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, deps) = line
            .split_once(':')
            .unwrap_or_else(|| panic!("libs.list: malformed line '{line}'"));
        out.push(Lib {
            name: name.trim().to_string(),
            deps: deps.split_whitespace().map(str::to_string).collect(),
        });
    }
    out
}

/// `core_miku` lives in `core/`
pub fn dir(root: &Path, lib: &str) -> PathBuf {
    root.join(lib.strip_suffix("_miku").unwrap_or(lib))
}

/// The files beside a library's `main.rs`
pub fn modules(root: &Path, lib: &str) -> Vec<PathBuf> {
    let d = dir(root, lib);
    let mut mods: Vec<PathBuf> = fs::read_dir(&d)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", d.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs") && !p.ends_with("main.rs"))
        .collect();
    mods.sort();
    mods
}

/// The library that owns a module
pub fn module_source(libs: &[Lib], root: &Path, module: &str) -> PathBuf {
    libs.iter()
        .map(|l| dir(root, &l.name).join(format!("{module}.rs")))
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("no library has a module '{module}'"))
}

/// Every library has a directory, every directory with a `main.rs` is listed,
/// dependencies exist, and the graph is acyclic
pub fn validate(libs: &[Lib], root: &Path) {
    for l in libs {
        let main = dir(root, &l.name).join("main.rs");
        if !main.exists() {
            panic!("libs.list: '{}' has no root at {}", l.name, main.display());
        }
        for dep in &l.deps {
            if !libs.iter().any(|o| o.name == *dep) {
                panic!("libs.list: '{}' depends on unknown library '{dep}'", l.name);
            }
        }
    }
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.join("main.rs").exists() {
            let stem = path.file_name().unwrap().to_str().unwrap();
            if !libs.iter().any(|l| l.name.strip_suffix("_miku") == Some(stem)) {
                panic!("{} has a main.rs but is missing from libs.list", path.display());
            }
        }
    }

    let index: BTreeMap<&str, usize> =
        libs.iter().enumerate().map(|(i, l)| (l.name.as_str(), i)).collect();
    fn visit(i: usize, libs: &[Lib], index: &BTreeMap<&str, usize>, state: &mut [u8], stack: &mut Vec<String>) {
        state[i] = 1;
        stack.push(libs[i].name.clone());
        for dep in &libs[i].deps {
            let j = index[dep.as_str()];
            match state[j] {
                1 => panic!("libs.list: dependency cycle: {} -> {dep}", stack.join(" -> ")),
                0 => visit(j, libs, index, state, stack),
                _ => {}
            }
        }
        stack.pop();
        state[i] = 2;
    }
    let mut state = vec![0u8; libs.len()];
    for i in 0..libs.len() {
        if state[i] == 0 {
            visit(i, libs, &index, &mut state, &mut Vec::new());
        }
    }
}
