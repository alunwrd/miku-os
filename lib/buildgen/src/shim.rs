//! The modules one library calls another through
//!
//! A library that needs `crate::heap::miku_malloc` and does not own `heap.rs`
//! includes a shim generated from it. The shim forwards to the C symbol, which
//! `ld-miku` resolves through the library's `DT_NEEDED`, so a signature cannot
//! diverge from the export. See `shim/README.md`.

use std::fs;
use std::path::Path;

use crate::exports;

struct ModApi {
    reexports: Vec<String>,
    consts: Vec<String>,
    structs: Vec<String>,
    statics: Vec<String>,
}

fn brace_delta(l: &str) -> i32 {
    l.chars().map(|c| match c { '{' => 1, '}' => -1, _ => 0 }).sum()
}

fn parse_items(src: &str) -> ModApi {
    let lines: Vec<&str> = src.lines().collect();
    let mut api = ModApi { reexports: vec![], consts: vec![], structs: vec![], statics: vec![] };
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.starts_with("pub use miku_abi::") {
            api.reexports.push(l.to_string());
            i += 1;
            continue;
        }
        let is_const = l.starts_with("pub const ");
        let is_static = l.starts_with("pub static ") && !l.starts_with("pub static mut");
        if is_const || is_static {
            let mut blk = vec![l.to_string()];
            let mut depth = brace_delta(l);
            let mut j = i;
            while depth > 0 || !lines[j].trim_end().ends_with(';') {
                j += 1;
                blk.push(lines[j].to_string());
                depth += brace_delta(lines[j]);
            }
            (if is_const { &mut api.consts } else { &mut api.statics }).push(blk.join("\n"));
            i = j + 1;
            continue;
        }
        if l.starts_with("#[repr(C)]") {
            let mut j = i + 1;
            while j < lines.len() && lines[j].starts_with("#[") {
                j += 1;
            }
            if j < lines.len() && ["pub struct", "pub union", "pub enum"].iter().any(|p| lines[j].starts_with(p)) {
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
        i += 1;
    }
    api
}

/// The shim of `module`: its re-exports and data, then a forwarding function per
/// export, then `manual` if any
pub fn generate(module: &str, source: &Path, manual: Option<&str>) -> String {
    let src = fs::read_to_string(source)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", source.display()));
    let api = parse_items(&src);
    let mut o = format!("// shim for `{module}`, generated from {}\n\n", source.display());
    for group in [&api.reexports, &api.consts, &api.structs, &api.statics] {
        for item in group {
            o.push_str(item);
            o.push('\n');
        }
        if !group.is_empty() {
            o.push('\n');
        }
    }
    let mut decls = String::new();
    let mut wraps = String::new();
    for e in exports::scan(source) {
        let arglist = e.args.iter().map(|(n, t)| format!("{n}: {t}")).collect::<Vec<_>>().join(", ");
        let names = e.args.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
        let ret = if e.ret.is_empty() { String::new() } else { format!(" -> {}", e.ret) };
        let name = &e.name;
        decls.push_str(&format!("        pub fn {name}({arglist}){ret};\n"));
        if e.is_unsafe {
            wraps.push_str(&format!("#[inline]\npub unsafe fn {name}({arglist}){ret} {{ ffi::{name}({names}) }}\n"));
        } else {
            wraps.push_str(&format!("#[inline]\npub fn {name}({arglist}){ret} {{ unsafe {{ ffi::{name}({names}) }} }}\n"));
        }
    }
    o.push_str("mod ffi {\n    #[allow(unused_imports)] use super::*;\n    #[allow(improper_ctypes)]\n    extern \"C\" {\n");
    o.push_str(&decls);
    o.push_str("    }\n}\n\n");
    o.push_str(&wraps);
    if let Some(m) = manual {
        o.push('\n');
        o.push_str(m);
    }
    o
}
