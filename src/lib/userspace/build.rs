// Link stubs for MikuOS apps.
//
// libmiku was split into per-domain libraries (see ../mikulibs/). For each
// of them this script generates a stub .so whose soname matches the runtime
// library, so apps link statically against the stubs and get one DT_NEEDED
// entry per library; ld-miku resolves the real symbols at load time.
//
// Stub symbol lists are derived from the library roots in ../mikulibs/
// (#[path] module includes) and the module sources in ../libmiku/
// (#[no_mangle] functions and global_asm ".global" labels).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// library names come from the shared manifest ../mikulibs/libs.list
fn miku_libs(roots: &Path) -> Vec<String> {
    let list = roots.join("libs.list");
    let src = fs::read_to_string(&list)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", list.display()));
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').next().unwrap().trim().to_string())
        .collect()
}

// collect names of #[no_mangle] fns and ".global" asm labels from a module
fn exported_syms(path: &Path, out: &mut Vec<String>) {
    let src = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some(pos) = lines[i].find(".global ") {
            let rest = lines[i][pos + ".global ".len()..].trim();
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if end > 0 {
                out.push(rest[..end].to_string());
            }
            i += 1;
            continue;
        }
        if lines[i].trim() == "#[no_mangle]" {
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j].trim();
                if l.starts_with("#[") || l.is_empty() {
                    j += 1;
                    continue;
                }
                if let Some(pos) = l.find("fn ") {
                    let rest = &l[pos + 3..];
                    let end = rest
                        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .unwrap_or(rest.len());
                    out.push(rest[..end].to_string());
                }
                break;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
}

// modules of a library = #[path = "../libmiku/X.rs"] includes in its root
fn lib_modules(root_rs: &Path) -> Vec<PathBuf> {
    let src = fs::read_to_string(root_rs)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", root_rs.display()));
    let dir = root_rs.parent().unwrap();
    let mut mods = Vec::new();
    for line in src.lines() {
        if let Some(start) = line.find("#[path = \"../libmiku/") {
            let rest = &line[start + "#[path = \"".len()..];
            if let Some(end) = rest.find('"') {
                mods.push(dir.join(&rest[..end]));
            }
        }
    }
    mods
}

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let roots = manifest.join("../mikulibs");

    println!("cargo:rerun-if-changed={}", roots.display());
    println!("cargo:rerun-if-changed={}", manifest.join("../libmiku").display());

    let libs = miku_libs(&roots);
    for lib in &libs {
        let mut syms = Vec::new();
        for m in lib_modules(&roots.join(format!("{lib}.rs"))) {
            exported_syms(&m, &mut syms);
        }
        if lib == "core_miku" {
            // compiler/runtime hooks apps may reference
            syms.push("__stack_chk_fail".to_string());
        }
        syms.sort();
        syms.dedup();

        let mut c = String::from("/* auto-generated link stub */\n");
        for s in &syms {
            c.push_str(&format!("void {s}(void) {{}}\n"));
        }
        let c_path = out_dir.join(format!("{lib}_stub.c"));
        let so_path = out_dir.join(format!("lib{lib}.so"));
        fs::write(&c_path, c).unwrap();

        let status = Command::new("gcc")
            .args([
                "-shared",
                "-nostdlib",
                "-fPIC",
                &format!("-Wl,-soname,{lib}.so"),
                "-o",
                so_path.to_str().unwrap(),
                c_path.to_str().unwrap(),
            ])
            .status()
            .expect("gcc failed");
        assert!(status.success(), "failed to build stub for {lib}");
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    for lib in &libs {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}
