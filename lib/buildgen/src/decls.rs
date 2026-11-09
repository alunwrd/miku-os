//! The `extern` blocks of `lib/sdk/miku`, generated from the definitions

use std::fmt::Write;
use std::path::Path;

use crate::{exports, libs};

/// A signature's type, as the `miku` crate names it
fn type_here(t: &str) -> String {
    let t = t.replace("file::", "").replace("crate::ctime::", "").replace("crate::", "");
    let mut out = String::new();
    let mut word = String::new();
    for ch in t.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
            continue;
        }
        out.push_str(match word.as_str() {
            // pthread.rs's internals, which callers see as the C types
            "Mutex" => "pthread_mutex_t",
            "Cond" => "pthread_cond_t",
            "RwLock" => "pthread_rwlock_t",
            w => w,
        });
        word.clear();
        out.push(ch);
    }
    out.trim_end().to_string()
}

fn block(out: &mut String, indent: &str, list: &[exports::Export]) {
    writeln!(out, "{indent}#[allow(improper_ctypes, clashing_extern_declarations)]").unwrap();
    writeln!(out, "{indent}extern \"C\" {{").unwrap();
    for e in list {
        // `va_list` functions have no use from Rust here
        if e.args.iter().any(|(_, t)| t.contains("VaList")) {
            continue;
        }
        for d in &e.docs {
            writeln!(out, "{indent}    {d}").unwrap();
        }
        let args: Vec<String> = e.args.iter()
            .map(|(n, t)| if t == "..." { "...".to_string() } else { format!("{n}: {}", type_here(t)) })
            .collect();
        let ret = if e.ret.is_empty() { String::new() } else { format!(" -> {}", type_here(&e.ret)) };
        writeln!(out, "{indent}    pub fn {}({}){ret};", e.name, args.join(", ")).unwrap();
    }
    writeln!(out, "{indent}}}").unwrap();
}

/// Every library's functions at the top, `libc_miku`'s in `pub mod libc`
pub fn generate(root: &Path, list: &[libs::Lib]) -> String {
    let mut out = String::from("// generated from the definitions by lib/buildgen/src/decls.rs\n\n");
    for l in list {
        let mut all = Vec::new();
        for m in libs::modules(root, &l.name) {
            all.extend(exports::scan(&m));
        }
        all.sort_by(|a, b| a.name.cmp(&b.name));
        if l.name == "libc_miku" {
            out.push_str("pub mod libc {\n    use super::*;\n\n");
            block(&mut out, "    ", &all);
            out.push_str("}\n\n");
        } else {
            writeln!(out, "// {}", l.name).unwrap();
            block(&mut out, "", &all);
            out.push('\n');
        }
    }
    out
}
