// `aosh check <file or directory>...`: parses every QML file and compiles its
// JavaScript without running it. Reports where a parse or compile fails

use std::path::{Path, PathBuf};

use crate::js::Js;
use crate::object::build::{binding_body, binding_fallback, handler_body};
use crate::qml::ast::{Member, ObjDef, Value};

pub fn run(args: &[String]) -> i32 {
    let mut files = Vec::new();
    for a in args {
        collect(Path::new(a), &mut files);
    }
    let js = match Js::new() {
        Ok(js) => js,
        Err(e) => {
            eprintln!("aosh: {e}");
            return 1;
        }
    };
    let (mut bad_files, mut snippets, mut bad_snippets) = (0, 0, 0);
    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                println!("{}: {e}", f.display());
                bad_files += 1;
                continue;
            }
        };
        let doc = match crate::qml::parse(&src) {
            Ok(d) => d,
            Err(e) => {
                println!("{}:{e}", f.display());
                bad_files += 1;
                continue;
            }
        };
        let mut found = Vec::new();
        walk(&doc.root, &mut found);
        for (_, def) in &doc.inline {
            walk(def, &mut found);
        }
        js.with(|ctx| {
            for (line, params, body, alt) in &found {
                snippets += 1;
                let r = crate::js::check_syntax(&ctx, params, body)
                    .or_else(|e| alt.as_ref().map_or(Err(e), |a| crate::js::check_syntax(&ctx, params, a)));
                if let Err(e) = r {
                    println!("{}:{line}: {e}", f.display());
                    bad_snippets += 1;
                }
            }
        });
    }
    println!(
        "{} file(s): {} parsed; {} piece(s) of JavaScript, {} did not compile",
        files.len(), files.len() - bad_files, snippets, bad_snippets
    );
    if bad_files + bad_snippets == 0 { 0 } else { 1 }
}

/// JavaScript in a tree as (line, parameters, body, fallback body if `body`
/// does not compile)
type Found = Vec<(u32, Vec<String>, String, Option<String>)>;

fn walk(o: &ObjDef, out: &mut Found) {
    for m in &o.members {
        match m {
            Member::Assign { name, value, .. } => {
                let handler = name.rsplit('.').next().is_some_and(|n| {
                    n.strip_prefix("on").is_some_and(|r| r.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
                });
                value_js(value, handler, out);
            }
            Member::Property { value: Some(v), .. } => value_js(v, false, out),
            Member::Function { params, body, loc, .. } => out.push((loc.line, params.clone(), body.clone(), None)),
            Member::Child(c) => walk(c, out),
            Member::On { obj, .. } => walk(obj, out),
            _ => {}
        }
    }
}

fn value_js(v: &Value, handler: bool, out: &mut Found) {
    match v {
        Value::Js { src, loc } => {
            let (body, alt) = if handler {
                (handler_body(src), None)
            } else {
                (binding_body(src), binding_fallback(src))
            };
            out.push((loc.line, Vec::new(), body, alt));
        }
        Value::Object(o) => walk(o, out),
        Value::Objects(list) => list.iter().for_each(|o| walk(o, out)),
    }
}

/// Every `.qml` file at or under `p`, in a stable order
fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if p.extension().is_some_and(|e| e == "qml") {
        out.push(p.to_path_buf());
    }
}
