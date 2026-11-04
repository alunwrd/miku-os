// JavaScript imports: `import "shapes/offset.js" as Offset` in QML and
// `.import "cubic.js" as Cubic` in a script
//
// The alias names the script's top-level declarations. Each file is evaluated
// once and its namespace is shared, as with `.pragma library`.
//
// Namespace members are getters onto the script's variables, so later changes
// are visible. Cyclic imports get each other's namespace before it is filled; a
// name read at top level before the other script runs is `undefined`, as in Qt.

use std::path::{Path, PathBuf};

use rquickjs::{Ctx, Function, Object, Persistent};

use crate::object::Core;
use crate::qml::lexer::{lex, Tok};

/// Namespace of `path`'s declarations, evaluated on first request
pub fn namespace<'js>(ctx: &Ctx<'js>, core: &Core, path: &Path) -> rquickjs::Result<Object<'js>> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let cached = core.0.borrow().libs.get(&path).cloned();
    if let Some(ns) = cached {
        return ns.restore(ctx);
    }
    let ns = Object::new(ctx.clone())?;
    core.0.borrow_mut().libs.insert(path.clone(), Persistent::save(ctx, ns.clone()));
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            core.log(format!("{}: {e}", path.display()));
            return Ok(ns);
        }
    };
    let script = Script::read(&src, path.parent().unwrap_or(Path::new(".")));
    let mut args = vec![ns.clone().into_value()];
    for (_, dep) in &script.imports {
        args.push(namespace(ctx, core, dep)?.into_value());
    }
    let mut opts = rquickjs::context::EvalOptions::default();
    opts.strict = false;
    let run = ctx.eval_with_options::<Function, _>(script.wrapped(), opts)
        .and_then(|f| f.call::<_, ()>((rquickjs::function::Rest(args),)));
    if let Err(e) = run {
        core.log(format!("{}: {}", path.display(), crate::js::describe(ctx, e)));
    }
    Ok(ns)
}

/// A script with QML directives removed, its imports, and its declared names
struct Script {
    body: String,
    imports: Vec<(String, PathBuf)>,
    names: Vec<String>,
}

impl Script {
    fn read(src: &str, dir: &Path) -> Script {
        // `.pragma library` and `.import "x.js" as X` become empty lines,
        // keeping QuickJS's line numbers
        let mut imports = Vec::new();
        let mut body = String::with_capacity(src.len());
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with(".pragma") {
                body.push('\n');
                continue;
            }
            if let Some(rest) = t.strip_prefix(".import") {
                if let Some((file, alias)) = parse_import(rest) {
                    imports.push((alias, dir.join(file)));
                }
                body.push('\n');
                continue;
            }
            body.push_str(line);
            body.push('\n');
        }
        let names = top_level_names(&body);
        Script { body, imports, names }
    }

    /// `(function(__ns, A, B) { <body>; <getters> })`
    fn wrapped(&self) -> String {
        let mut params = vec!["__ns".to_string()];
        params.extend(self.imports.iter().map(|(a, _)| a.clone()));
        let mut out = format!("(function({}) {{\n", params.join(", "));
        out.push_str(&self.body);
        out.push_str("\n;");
        for n in &self.names {
            out.push_str(&format!(
                "Object.defineProperty(__ns, {n:?}, {{ get: () => {n}, set: (v) => {{ {n} = v; }}, \
                 enumerable: true, configurable: true }});"
            ));
        }
        out.push_str("\n})");
        out
    }
}

/// ` "cubic.js" as Cubic`
fn parse_import(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim();
    let q = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = rest[1..].find(q)? + 1;
    let file = rest[1..end].to_string();
    let after = rest[end + 1..].trim();
    let alias = after.strip_prefix("as")?.trim().trim_end_matches(';').trim();
    (!alias.is_empty()).then(|| (file, alias.to_string()))
}

/// Top-level declarations: `function f`, `class C`, `var x`, `let a, b`,
/// `const k`. A script that fails to lex declares nothing
fn top_level_names(body: &str) -> Vec<String> {
    let Ok(toks) = lex(body) else { return Vec::new() };
    let mut names = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    let ident = |k: usize| match toks.get(k).map(|t| &t.tok) {
        Some(Tok::Ident(s)) => Some(s.clone()),
        _ => None,
    };
    while i < toks.len() {
        match &toks[i].tok {
            Tok::Punct("{" | "(" | "[") => depth += 1,
            Tok::Punct("}" | ")" | "]") => depth -= 1,
            Tok::Ident(w) if depth == 0 => match w.as_str() {
                "function" => {
                    let k = if matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("*"))) { i + 2 } else { i + 1 };
                    if let Some(n) = ident(k) {
                        names.push(n);
                    }
                }
                "class" => {
                    if let Some(n) = ident(i + 1) {
                        names.push(n);
                    }
                }
                "var" | "let" | "const" => {
                    // First name, then any after a comma at this depth
                    if let Some(n) = ident(i + 1) {
                        names.push(n);
                    }
                    let mut d = 0i32;
                    let mut k = i + 2;
                    while k < toks.len() {
                        match &toks[k].tok {
                            Tok::Punct("{" | "(" | "[") => d += 1,
                            Tok::Punct("}" | ")" | "]") => d -= 1,
                            Tok::Punct(";") if d == 0 => break,
                            Tok::Punct(",") if d == 0 => {
                                if let Some(n) = ident(k + 1) {
                                    names.push(n);
                                }
                            }
                            _ if d == 0 && toks[k].newline_before
                                && !matches!(toks[k - 1].tok, Tok::Punct(",")) => break,
                            _ => {}
                        }
                        if d < 0 {
                            break;
                        }
                        k += 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_script_declares() {
        let n = top_level_names(r#"
            var _circle = null, _square = null
            let a = 1
            const k = { x: 1, y: [2, 3] }, j = 2;
            function createOffset(x, y) { var inner = 1; return new Offset(x, y); }
            class Offset { constructor(x, y) { this.x = x; } }
            const rotate30 = (p) => { let hidden = 2; return p; }
        "#);
        assert_eq!(n, vec!["Offset", "_circle", "_square", "a", "createOffset", "j", "k", "rotate30"]);
    }

    #[test]
    fn directives_out_imports_in() {
        let s = Script::read(".pragma library\n\n.import \"cubic.js\" as Cubic\n.import '../geometry/offset.js' as Offset\nvar x = 1\n",
                             Path::new("/a/b"));
        assert_eq!(s.imports, vec![("Cubic".into(), PathBuf::from("/a/b/cubic.js")),
                                   ("Offset".into(), PathBuf::from("/a/b/../geometry/offset.js"))]);
        assert_eq!(s.names, vec!["x"]);
        // Line numbers are preserved
        assert_eq!(s.body.lines().position(|l| l.contains("var x")), Some(4));
    }
}
