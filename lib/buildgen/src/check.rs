//! Hand-written declarations against the definitions
//!
//! `lib/sdk/rust-std`'s `abi.rs` and `lib/sdk/rust-libc` declare functions
//! themselves: `safe fn`, and a format `libc` owns. A wrong one is memory
//! corruption nobody sees, so the builder refuses it.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::{exports, libs};

/// How an argument travels, as far as source can say
fn class(t: &str) -> Option<&'static str> {
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let last = t.rsplit("::").next().unwrap_or(&t);
    if t.starts_with('*') || t.starts_with('&') || t.contains("fn(") || t.starts_with("Option<") {
        return Some("8");
    }
    Some(match last {
        "u8" | "i8" | "bool" | "c_char" | "c_uchar" | "c_schar" | "cc_t" => "1",
        "u16" | "i16" | "c_short" | "c_ushort" | "sa_family_t" | "in_port_t" => "2",
        "u32" | "i32" | "c_int" | "c_uint" | "pid_t" | "uid_t" | "gid_t" | "mode_t" | "socklen_t"
        | "clockid_t" | "id_t" | "tcflag_t" | "speed_t" | "in_addr_t" => "4",
        "u64" | "i64" | "usize" | "isize" | "c_long" | "c_ulong" | "c_longlong" | "c_ulonglong"
        | "size_t" | "ssize_t" | "off_t" | "time_t" | "dev_t" | "ino_t" | "nfds_t" | "rlim_t"
        | "_Opcode" | "sighandler_t" | "ThreadFn" | "SignalHandler" | "CmpFn" | "ForkFn"
        | "pthread_t" => "8",
        "f32" => "f4",
        "f64" => "f8",
        // A struct, or a name not listed: nothing to compare
        _ => return None,
    })
}

/// `(name, args, ret)` of each function in an `extern` block of `text`
fn declared(text: &str) -> Vec<exports::Export> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("extern \"C\" {") {
        let body = &rest[i + "extern \"C\" {".len()..];
        let mut depth = 1;
        let mut end = 0;
        for (j, c) in body.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                end = j;
                break;
            }
        }
        let stripped: String = body[..end]
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for item in stripped.split(';') {
            let s = item.trim();
            if s.contains("fn ") {
                out.push(exports::parse_signature(&s.split_whitespace().collect::<Vec<_>>().join(" "), Vec::new()));
            }
        }
        rest = &body[end..];
    }
    out
}

fn compare(what: &str, d: &exports::Export, e: &exports::Export, errors: &mut Vec<String>) {
    if d.is_variadic() || e.is_variadic() {
        return;
    }
    if d.args.len() != e.args.len() {
        errors.push(format!("{what}: {} takes {} arguments, the library's {}", d.name, d.args.len(), e.args.len()));
        return;
    }
    for (n, (a, b)) in d.args.iter().zip(e.args.iter()).enumerate() {
        if let (Some(x), Some(y)) = (class(&a.1), class(&b.1)) {
            if x != y {
                errors.push(format!("{what}: {} argument {n} is `{}`, the library's `{}`", d.name, a.1, b.1));
            }
        }
    }
    if let (Some(x), Some(y)) = (class(&d.ret), class(&e.ret)) {
        if x != y {
            errors.push(format!("{what}: {} returns `{}`, the library's `{}`", d.name, d.ret, e.ret));
        }
    }
}

/// Checks `files` against the libraries under `root`. Errors, one per line
pub fn declarations(root: &Path, files: &[&Path]) -> Result<(), String> {
    let list = libs::read(root);
    let mut defined = HashMap::new();
    for l in &list {
        for m in libs::modules(root, &l.name) {
            for e in exports::scan(&m) {
                defined.insert(e.name.clone(), e);
            }
        }
    }
    let mut errors = Vec::new();
    for f in files {
        let text = fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        for d in declared(&text) {
            match defined.get(&d.name) {
                Some(e) => compare(&f.display().to_string(), &d, e, &mut errors),
                // A C name no library defines is `check_declarations`' finding for
                // headers; a `.global` label has no signature here
                None => {}
            }
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
}
