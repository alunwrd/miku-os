//! An empty `.so` with a library's soname and exports, for linking against.
//! `ld-miku` resolves the real symbols at load time

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{exports, libs};

/// Writes `lib<name>.so` for every library into `out_dir`
pub fn build_all(root: &Path, list: &[libs::Lib], out_dir: &Path) {
    for l in list {
        let mut syms = Vec::new();
        for m in libs::modules(root, &l.name) {
            exports::symbols(&m, &mut syms);
        }
        // A compiler hook programs may reference
        if l.name == "core_miku" {
            syms.push("__stack_chk_fail".to_string());
        }
        syms.sort();
        syms.dedup();
        build(&l.name, &syms, out_dir);
    }
}

fn build(lib: &str, syms: &[String], out_dir: &Path) -> PathBuf {
    let mut c = String::from("/* generated link stub */\n");
    for s in syms {
        c.push_str(&format!("void {s}(void) {{}}\n"));
    }
    let c_path = out_dir.join(format!("{lib}_stub.c"));
    let so_path = out_dir.join(format!("lib{lib}.so"));
    fs::write(&c_path, c).unwrap();
    let status = Command::new("gcc")
        .args(["-shared", "-nostdlib", "-fPIC", &format!("-Wl,-soname,{lib}.so"), "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("gcc failed");
    assert!(status.success(), "failed to build stub for {lib}");
    so_path
}
