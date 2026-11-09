//! A library's exports, read from source: `#[no_mangle]` functions and
//! `.global` labels in `global_asm!`

use std::fs;
use std::path::Path;

/// One `#[no_mangle] extern "C" fn`
pub struct Export {
    pub name: String,
    /// `///` lines above it, as written
    pub docs: Vec<String>,
    pub is_unsafe: bool,
    /// `name: type`, without `mut`. A variadic's last parameter is `...`
    pub args: Vec<(String, String)>,
    /// `""` or the type after `->`
    pub ret: String,
}

impl Export {
    pub fn is_variadic(&self) -> bool {
        self.args.last().is_some_and(|(_, t)| t == "...")
    }
}

fn ident_end(s: &str) -> usize {
    s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len())
}

/// Top-level commas only: `a: T, b: fn(X, Y) -> Z` is two
pub fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut prev = ' ';
    for ch in args.chars() {
        match ch {
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => {
                if "<([".contains(ch) {
                    depth += 1;
                }
                if ">)]".contains(ch) && !(ch == '>' && prev == '-') {
                    depth -= 1;
                }
                cur.push(ch);
            }
        }
        prev = ch;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

pub fn matching_paren(s: &str, open: usize) -> usize {
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

/// `pub unsafe extern "C" fn name(args) -> ret`, without the body
pub fn parse_signature(sig: &str, docs: Vec<String>) -> Export {
    let is_unsafe = sig.contains("unsafe ");
    let after_fn = &sig[sig.find("fn ").unwrap() + 3..];
    let name = after_fn[..ident_end(after_fn)].to_string();
    let open = after_fn.find('(').unwrap();
    let close = matching_paren(after_fn, open);
    let ret = after_fn[close + 1..].trim().trim_start_matches("->").trim().to_string();
    let args = split_args(&after_fn[open + 1..close])
        .iter()
        .map(|a| {
            let a = a.strip_prefix("mut ").unwrap_or(a);
            match a.split_once(':') {
                Some((n, t)) => (n.trim().to_string(), t.trim().to_string()),
                // `...` on its own
                None => (String::new(), a.to_string()),
            }
        })
        .collect();
    Export { name, docs, is_unsafe, args, ret }
}

/// Where a function's body opens, outside the parameter list
fn body_start(sig: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in sig.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// The `#[no_mangle]` functions of one module
pub fn scan(path: &Path) -> Vec<Export> {
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "#[no_mangle]" {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < lines.len() && (lines[j].trim().starts_with("#[") || lines[j].trim().is_empty()) {
            j += 1;
        }
        if j >= lines.len() || !lines[j].contains("fn ") {
            i = j;
            continue;
        }
        let mut sig = lines[j].to_string();
        let mut k = j;
        while body_start(&sig).is_none() {
            k += 1;
            sig.push(' ');
            sig.push_str(lines[k].trim());
        }
        let sig = sig[..body_start(&sig).unwrap()].trim().to_string();
        let mut d = i;
        let mut docs = Vec::new();
        while d > 0 && (lines[d - 1].trim_start().starts_with("///") || lines[d - 1].trim_start().starts_with("#[")) {
            d -= 1;
            if lines[d].trim_start().starts_with("///") {
                docs.insert(0, lines[d].trim().to_string());
            }
        }
        out.push(parse_signature(&sig, docs));
        i = k + 1;
    }
    out
}

/// Every symbol a module exports, `global_asm!` labels included
pub fn symbols(path: &Path, out: &mut Vec<String>) {
    let src = fs::read_to_string(path).unwrap_or_default();
    for line in src.lines() {
        if let Some(pos) = line.find(".global ") {
            let rest = line[pos + ".global ".len()..].trim();
            let end = ident_end(rest);
            if end > 0 {
                out.push(rest[..end].to_string());
            }
        }
    }
    out.extend(scan(path).into_iter().map(|e| e.name));
}
