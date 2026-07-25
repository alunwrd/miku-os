// Build support for the split miku libraries.
//
// Each [[bin]] here becomes one shared library (<name>.so) on MikuOS.
// Libraries call each other only through C-ABI `miku_*` symbols (see
// src/lib/mikulibs/shim/), resolved at load time by ld-miku.
//
// This script:
//   1. Parses the library manifest (src/lib/mikulibs/libs.list) and
//      validates it: every library has a root .rs, every root is listed,
//      all dependencies exist, and the graph is acyclic.
//   2. Regenerates the shim modules from the module sources, so shim
//      signatures can never drift from the real exports. Text below the
//      manual-section marker in each shim is preserved.
//   3. Generates an empty stub .so per library (soname <name>.so) with
//      every exported symbol, and links each bin against the stubs of
//      its dependencies so the static link succeeds and DT_NEEDED
//      entries are recorded.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MANUAL_MARKER: &str =
    "// ===== manual additions below (preserved by the generator) =====";

// shims that are written by hand and never regenerated
const SHIM_SKIP: &[&str] = &["sync"];

// ---------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------

fn parse_libs_list(path: &Path) -> Vec<(String, Vec<String>)> {
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, deps) = line
            .split_once(':')
            .unwrap_or_else(|| panic!("libs.list: malformed line '{line}'"));
        out.push((
            name.trim().to_string(),
            deps.split_whitespace().map(str::to_string).collect(),
        ));
    }
    out
}

fn validate(libs: &[(String, Vec<String>)], roots: &Path) {
    // every library has a root file
    for (lib, deps) in libs {
        let root = roots.join(format!("{lib}.rs"));
        if !root.exists() {
            panic!("libs.list: '{lib}' has no root at {}", root.display());
        }
        for dep in deps {
            if !libs.iter().any(|(n, _)| n == dep) {
                panic!("libs.list: '{lib}' depends on unknown library '{dep}'");
            }
        }
    }

    // every root file is listed
    for entry in fs::read_dir(roots).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if let Some(stem) = name.strip_suffix(".rs") {
            if stem.ends_with("_miku") && !libs.iter().any(|(n, _)| n == stem) {
                panic!("root {name} exists but '{stem}' is missing from libs.list");
            }
        }
    }

    // the dependency graph must be acyclic
    let index: BTreeMap<&str, usize> =
        libs.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    let mut state = vec![0u8; libs.len()]; // 0 unvisited, 1 in stack, 2 done
    fn dfs(
        i: usize,
        libs: &[(String, Vec<String>)],
        index: &BTreeMap<&str, usize>,
        state: &mut [u8],
        stack: &mut Vec<String>,
    ) {
        state[i] = 1;
        stack.push(libs[i].0.clone());
        for dep in &libs[i].1 {
            let j = index[dep.as_str()];
            match state[j] {
                1 => panic!(
                    "libs.list: dependency cycle: {} -> {dep}",
                    stack.join(" -> ")
                ),
                0 => dfs(j, libs, index, state, stack),
                _ => {}
            }
        }
        stack.pop();
        state[i] = 2;
    }
    for i in 0..libs.len() {
        if state[i] == 0 {
            dfs(i, libs, &index, &mut state, &mut Vec::new());
        }
    }
}

// ---------------------------------------------------------------------------
// module source parsing
// ---------------------------------------------------------------------------

fn ident_end(s: &str) -> usize {
    s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len())
}

// collect names of #[no_mangle] fns and ".global" asm labels from a module
fn exported_syms(path: &Path, out: &mut Vec<String>) {
    let src = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some(pos) = lines[i].find(".global ") {
            let rest = lines[i][pos + ".global ".len()..].trim();
            let end = ident_end(rest);
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
                    out.push(rest[..ident_end(rest)].to_string());
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
    include_paths(root_rs, "#[path = \"../libmiku/")
}

// shim modules referenced by a root = #[path = "shim/X.rs"] includes
fn shim_modules(root_rs: &Path) -> Vec<String> {
    include_paths(root_rs, "#[path = \"shim/")
        .iter()
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
        .collect()
}

fn include_paths(root_rs: &Path, prefix: &str) -> Vec<PathBuf> {
    let src = fs::read_to_string(root_rs)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", root_rs.display()));
    let dir = root_rs.parent().unwrap();
    let mut mods = Vec::new();
    for line in src.lines() {
        if let Some(start) = line.find(prefix) {
            let rest = &line[start + "#[path = \"".len()..];
            if let Some(end) = rest.find('"') {
                mods.push(dir.join(&rest[..end]));
            }
        }
    }
    mods
}

// ---------------------------------------------------------------------------
// shim generation
// ---------------------------------------------------------------------------

struct ModApi {
    consts: Vec<String>,
    structs: Vec<String>,
    statics: Vec<String>,
    fns: Vec<String>, // full signatures without trailing '{'
}

fn parse_module_api(path: &Path) -> ModApi {
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let lines: Vec<&str> = src.lines().collect();
    let mut api = ModApi { consts: vec![], structs: vec![], statics: vec![], fns: vec![] };

    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];

        // top-level pub const / pub static (copy whole item, may span lines;
        // the item ends at a ';' outside any braces)
        let is_const = l.starts_with("pub const ");
        let is_static = l.starts_with("pub static ") && !l.starts_with("pub static mut");
        if is_const || is_static {
            let mut blk = vec![l.to_string()];
            let mut depth = brace_delta(l);
            let mut j = i;
            while depth > 0 || !ends_item(lines[j]) {
                j += 1;
                blk.push(lines[j].to_string());
                depth += brace_delta(lines[j]);
            }
            if is_const {
                api.consts.push(blk.join("\n"));
            } else {
                api.statics.push(blk.join("\n"));
            }
            i = j + 1;
            continue;
        }

        // #[repr(C)] structs, incl. following attributes
        if l.starts_with("#[repr(C)]") {
            let mut j = i + 1;
            while j < lines.len() && lines[j].starts_with("#[") {
                j += 1;
            }
            if j < lines.len()
                && (lines[j].starts_with("pub struct")
                    || lines[j].starts_with("pub union")
                    || lines[j].starts_with("pub enum"))
            {
                let mut blk: Vec<String> = lines[i..=j].iter().map(|s| s.to_string()).collect();
                if lines[j].contains('{') {
                    let mut depth = brace_delta(lines[j]);
                    let mut k = j;
                    while depth > 0 {
                        k += 1;
                        blk.push(lines[k].to_string());
                        depth += brace_delta(lines[k]);
                    }
                    i = k + 1;
                } else {
                    i = j + 1;
                }
                api.structs.push(blk.join("\n"));
                continue;
            }
        }

        // #[no_mangle] extern "C" fn
        if l.trim() == "#[no_mangle]" {
            let mut j = i + 1;
            while j < lines.len()
                && (lines[j].trim().starts_with("#[") || lines[j].trim().is_empty())
            {
                j += 1;
            }
            if j < lines.len() && lines[j].contains("extern \"C\" fn") {
                let mut sig = lines[j].to_string();
                let mut k = j;
                while !sig.contains('{') && !sig.contains(';') {
                    k += 1;
                    sig.push(' ');
                    sig.push_str(lines[k].trim());
                }
                let sig = sig.split('{').next().unwrap().trim().to_string();
                api.fns.push(sig);
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    api
}

// a const/static item line terminates the item if it ends with ';'
fn ends_item(l: &str) -> bool {
    l.trim_end().ends_with(';')
}

fn brace_delta(l: &str) -> i32 {
    l.chars().map(|c| match c {
        '{' => 1,
        '}' => -1,
        _ => 0,
    }).sum()
}

// split "a: T, b: fn(X, Y) -> Z" into top-level arguments
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in args.chars() {
        match ch {
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => {
                if "<([".contains(ch) { depth += 1; }
                if ">)]".contains(ch) { depth -= 1; }
                cur.push(ch);
            }
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn generate_shim(module: &str, src_path: &Path, shim_path: &Path) {
    let api = parse_module_api(src_path);

    // preserve the manual tail, if any
    let manual_tail = fs::read_to_string(shim_path)
        .ok()
        .and_then(|old| old.find(MANUAL_MARKER).map(|p| old[p..].to_string()));

    let mut o = String::new();
    o.push_str(&format!(
        "// shim for `{module}`: generated by mikulibs/build.rs from\n\
         // src/lib/libmiku/{module}.rs. DO NOT EDIT above the manual marker.\n\
         //\n\
         // Forwards to the C symbols exported by the library that owns the\n\
         // module, resolved at load time by ld-miku. Shims must stay\n\
         // stateless: all state lives in the owning library (see README.md).\n\n"
    ));

    for c in &api.consts {
        o.push_str(c);
        o.push('\n');
    }
    if !api.consts.is_empty() { o.push('\n'); }
    for s in &api.structs {
        o.push_str(s);
        o.push('\n');
    }
    if !api.structs.is_empty() { o.push('\n'); }
    for s in &api.statics {
        o.push_str(s);
        o.push('\n');
    }
    if !api.statics.is_empty() { o.push('\n'); }

    let mut decls = String::new();
    let mut wraps = String::new();
    for sig in &api.fns {
        // pub [unsafe] extern "C" fn name(args) [-> ret]
        let is_unsafe = sig.contains("unsafe extern");
        let after_fn = &sig[sig.find("fn ").unwrap() + 3..];
        let name = &after_fn[..ident_end(after_fn)];
        let open = after_fn.find('(').unwrap();
        let close = matching_paren(after_fn, open);
        let args = &after_fn[open + 1..close];
        let ret = after_fn[close + 1..].trim(); // "" or "-> T"

        let cleaned: Vec<String> = split_args(args)
            .iter()
            .map(|a| a.strip_prefix("mut ").unwrap_or(a).to_string())
            .collect();
        let names: Vec<String> =
            cleaned.iter().map(|a| a.split(':').next().unwrap().trim().to_string()).collect();
        let arglist = cleaned.join(", ");
        let namelist = names.join(", ");

        decls.push_str(&format!("        pub fn {name}({arglist}) {ret};\n"));
        if is_unsafe {
            wraps.push_str(&format!(
                "#[inline]\npub unsafe fn {name}({arglist}) {ret} {{ ffi::{name}({namelist}) }}\n"
            ));
        } else {
            wraps.push_str(&format!(
                "#[inline]\npub fn {name}({arglist}) {ret} {{ unsafe {{ ffi::{name}({namelist}) }} }}\n"
            ));
        }
    }

    o.push_str("mod ffi {\n");
    o.push_str("    #[allow(unused_imports)] use super::*;\n");
    o.push_str("    #[allow(improper_ctypes)]\n");
    o.push_str("    extern \"C\" {\n");
    o.push_str(&decls);
    o.push_str("    }\n}\n\n");
    o.push_str(&wraps);
    o.push('\n');
    match manual_tail {
        Some(tail) => o.push_str(&tail),
        None => {
            o.push_str(MANUAL_MARKER);
            o.push('\n');
        }
    }

    // avoid mtime churn when nothing changed
    if fs::read_to_string(shim_path).ok().as_deref() != Some(o.as_str()) {
        fs::write(shim_path, o)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", shim_path.display()));
    }
}

fn matching_paren(s: &str, open: usize) -> usize {
    let mut depth = 0;
    for (i, ch) in s.char_indices().skip(open) {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced parens in signature: {s}");
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let roots = manifest.join("../src/lib/mikulibs");
    let modules_dir = manifest.join("../src/lib/libmiku");

    println!("cargo:rerun-if-changed={}", roots.display());
    println!("cargo:rerun-if-changed={}", modules_dir.display());

    let libs = parse_libs_list(&roots.join("libs.list"));
    validate(&libs, &roots);

    // regenerate shims for every module some root includes from shim/
    for (lib, _) in &libs {
        for shim in shim_modules(&roots.join(format!("{lib}.rs"))) {
            if SHIM_SKIP.contains(&shim.as_str()) {
                continue;
            }
            generate_shim(
                &shim,
                &modules_dir.join(format!("{shim}.rs")),
                &roots.join("shim").join(format!("{shim}.rs")),
            );
        }
    }

    // build a stub .so per library
    for (lib, _) in &libs {
        let mut syms = Vec::new();
        for m in lib_modules(&roots.join(format!("{lib}.rs"))) {
            exported_syms(&m, &mut syms);
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
    for (lib, deps) in &libs {
        for dep in deps {
            println!("cargo:rustc-link-arg-bin={lib}=-l{dep}");
        }
        // libc_miku exports POSIX names (strlen, malloc, printf, ...), so it
        // keeps full --export-dynamic; everything else only exposes the
        // miku_* C ABI, which lets --gc-sections drop unused core machinery.
        if lib == "libc_miku" {
            println!("cargo:rustc-link-arg-bin={lib}=--export-dynamic");
        } else {
            println!("cargo:rustc-link-arg-bin={lib}=--export-dynamic-symbol=miku_*");
            println!("cargo:rustc-link-arg-bin={lib}=--export-dynamic-symbol=__miku_*");
            println!("cargo:rustc-link-arg-bin={lib}=--gc-sections");
        }
    }
}
