// Parses a QML file into its tree
//
// JavaScript is kept as the source slice it occupies. QML has no semicolons, so
// a value runs to the end of its line, unless:
// - a bracket opened on it is still open;
// - the line ends on something that cannot end an expression (`a +`);
//  - the next line begins with something that can only continue one (`? b`).

use super::ast::*;
use super::lexer::{lex, Tok, Token};

#[derive(Debug)]
pub struct ParseError {
    pub loc: Loc,
    pub what: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.loc.line, self.loc.col, self.what)
    }
}

pub fn parse(src: &str) -> Result<Document, ParseError> {
    let toks = lex(src).map_err(|e| ParseError { loc: e.loc, what: e.what })?;
    let mut p = Parser { src, toks, i: 0, enums: Vec::new() };
    p.document()
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    i: usize,
    enums: Vec<(String, Vec<(String, i64)>)>,
}

fn is_type_name(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

impl<'a> Parser<'a> {
    fn at(&self, k: usize) -> Option<&Token> {
        self.toks.get(self.i + k)
    }

    fn here(&self) -> Loc {
        self.at(0).map(|t| t.loc).unwrap_or_else(|| self.toks.last().map(|t| t.loc).unwrap_or_default())
    }

    fn err<T>(&self, what: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError { loc: self.here(), what: what.into() })
    }

    fn is_punct(&self, k: usize, p: &str) -> bool {
        matches!(self.at(k), Some(Token { tok: Tok::Punct(q), .. }) if *q == p)
    }

    fn ident(&self, k: usize) -> Option<&str> {
        match self.at(k) {
            Some(Token { tok: Tok::Ident(s), .. }) => Some(s.as_str()),
            _ => None,
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), ParseError> {
        if self.is_punct(0, p) {
            self.i += 1;
            Ok(())
        } else {
            self.err(format!("expected '{p}' here"))
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.ident(0) {
            Some(s) => {
                let s = s.to_string();
                self.i += 1;
                Ok(s)
            }
            None => self.err("expected a name here"),
        }
    }

    /// `A.B.C`, as one string
    fn dotted(&mut self) -> Result<String, ParseError> {
        let mut name = self.expect_ident()?;
        while self.is_punct(0, ".") && self.ident(1).is_some() {
            self.i += 1;
            name.push('.');
            name.push_str(&self.expect_ident()?);
        }
        Ok(name)
    }

    fn skip_semis(&mut self) {
        while self.is_punct(0, ";") {
            self.i += 1;
        }
    }

    fn document(&mut self) -> Result<Document, ParseError> {
        let mut doc = Document::default();
        loop {
            self.skip_semis();
            match self.ident(0) {
                Some("pragma") => {
                    let line = self.at(0).unwrap().loc.line;
                    self.i += 1;
                    let start = self.at(0).map(|t| t.start).unwrap_or(0);
                    let mut end = start;
                    while let Some(t) = self.at(0) {
                        if t.loc.line != line {
                            break;
                        }
                        end = t.end;
                        self.i += 1;
                    }
                    doc.pragmas.push(self.src[start..end].trim().to_string());
                }
                Some("import") => {
                    let loc = self.here();
                    self.i += 1;
                    let (what, is_path) = match self.at(0).map(|t| t.tok.clone()) {
                        Some(Tok::Str(s)) => {
                            self.i += 1;
                            (s, true)
                        }
                        Some(Tok::Ident(_)) => (self.dotted()?, false),
                        _ => return self.err("expected a module or a quoted path after 'import'"),
                    };
                    let mut version = None;
                    if let Some(Tok::Number(_)) = self.at(0).map(|t| t.tok.clone()) {
                        let t = self.at(0).unwrap();
                        version = Some(self.src[t.start..t.end].to_string());
                        self.i += 1;
                        // `2.15` lexes as one number; `2.15.1` as a number and
                        // `.1`
                        while let Some(Tok::Number(_)) = self.at(0).map(|t| t.tok.clone()) {
                            if self.at(0).unwrap().newline_before {
                                break;
                            }
                            let t = self.at(0).unwrap();
                            version.as_mut().unwrap().push_str(&self.src[t.start..t.end]);
                            self.i += 1;
                        }
                    }
                    let mut alias = None;
                    if self.ident(0) == Some("as") {
                        self.i += 1;
                        alias = Some(self.expect_ident()?);
                    }
                    doc.imports.push(Import { what, is_path, version, alias, loc });
                }
                _ => break,
            }
        }
        doc.root = self.object(&mut doc.inline)?;
        doc.enums = std::mem::take(&mut self.enums);
        self.skip_semis();
        if self.at(0).is_some() {
            return self.err("something after the root object - a file has one");
        }
        Ok(doc)
    }

    /// `Type { members }`, with the type at the cursor
    fn object(&mut self, inline: &mut Vec<(String, ObjDef)>) -> Result<ObjDef, ParseError> {
        let loc = self.here();
        let ty = self.dotted()?;
        self.expect_punct("{")?;
        let mut obj = ObjDef { ty, id: None, members: Vec::new(), loc };
        self.members(&mut obj, "", inline)?;
        Ok(obj)
    }

    /// Members up to and including the closing brace. `prefix` is the enclosing
    /// group (`anchors.` inside `anchors { }`)
    fn members(&mut self, obj: &mut ObjDef, prefix: &str, inline: &mut Vec<(String, ObjDef)>)
        -> Result<(), ParseError>
    {
        loop {
            self.skip_semis();
            if self.is_punct(0, "}") {
                self.i += 1;
                return Ok(());
            }
            if self.at(0).is_none() {
                return self.err(format!("'{}' at line {} never closes", obj.ty, obj.loc.line));
            }
            let loc = self.here();
            let Some(word) = self.ident(0).map(str::to_string) else {
                return self.err("expected a property, an object or a declaration");
            };

            // `id: name`
            if prefix.is_empty() && word == "id" && self.is_punct(1, ":") {
                self.i += 2;
                obj.id = Some(self.expect_ident()?);
                continue;
            }

            // Declarations
            if prefix.is_empty() && matches!(word.as_str(), "property" | "readonly" | "required" | "default") {
                if self.declaration(obj, inline)? {
                    continue;
                }
            }
            if prefix.is_empty() && word == "signal" && self.ident(1).is_some() {
                self.i += 1;
                let name = self.expect_ident()?;
                let params = if self.is_punct(0, "(") { self.params()?.0 } else { Vec::new() };
                obj.members.push(Member::Signal { name, params, loc });
                continue;
            }
            if prefix.is_empty() && word == "function" && self.ident(1).is_some() {
                self.i += 1;
                let name = self.expect_ident()?;
                let (params, types) = self.params()?;
                // `function f(): int {`: a return type, used by IPC calls
                let mut ret = String::new();
                if self.is_punct(0, ":") {
                    self.i += 1;
                    ret = self.dotted()?;
                }
                let body = self.braced()?;
                obj.members.push(Member::Function { name, params, types, ret, body, loc });
                continue;
            }
            if prefix.is_empty() && word == "enum" && self.ident(1).is_some() && self.is_punct(2, "{") {
                self.i += 1;
                let name = self.expect_ident()?;
                let values = self.enum_values()?;
                self.enums.push((name, values));
                continue;
            }
            if prefix.is_empty() && word == "component" && self.ident(1).is_some() && self.is_punct(2, ":") {
                self.i += 1;
                let name = self.expect_ident()?;
                self.expect_punct(":")?;
                let def = self.object(inline)?;
                inline.push((name, def));
                continue;
            }

            // A possibly dotted name, then `:`, `{`, or `on`
            let start = self.i;
            let name = self.dotted()?;
            let full = format!("{prefix}{name}");

            if self.ident(0) == Some("on") && is_type_name(&name) {
                // `Behavior on width { ... }`. Inside a group it applies to the
                // group's property: `anchors { Behavior on bottomMargin {} }`
                self.i += 1;
                let prop = format!("{prefix}{}", self.dotted()?);
                self.expect_punct("{")?;
                let mut def = ObjDef { ty: name, id: None, members: Vec::new(), loc };
                self.members(&mut def, "", inline)?;
                obj.members.push(Member::On { prop, obj: def, loc });
                continue;
            }
            if self.is_punct(0, "{") {
                if is_type_name(&name) && prefix.is_empty() && !name.contains('.') {
                    self.i = start;
                    let child = self.object(inline)?;
                    obj.members.push(Member::Child(child));
                } else {
                    // A group: `anchors { left: x.right }`
                    self.i += 1;
                    self.members(obj, &format!("{full}."), inline)?;
                }
                continue;
            }
            if self.is_punct(0, ":") {
                self.i += 1;
                let value = self.value()?;
                obj.members.push(Member::Assign { name: full, value, loc });
                continue;
            }
            return self.err(format!("'{name}' is followed by neither ':' nor '{{'"));
        }
    }

    /// `[default|required|readonly] property <type> <name> [: value]`. False,
    /// with the cursor restored, if the words are not a declaration
    fn declaration(&mut self, obj: &mut ObjDef, _inline: &mut Vec<(String, ObjDef)>) -> Result<bool, ParseError> {
        let save = self.i;
        let loc = self.here();
        let (mut default, mut readonly, mut required) = (false, false, false);
        loop {
            match self.ident(0) {
                Some("default") => default = true,
                Some("readonly") => readonly = true,
                Some("required") => required = true,
                _ => break,
            }
            self.i += 1;
        }
        if self.ident(0) != Some("property") || self.is_punct(1, ":") {
            // `required name` alone marks a type's property as required;
            // recorded with no type
            if required && self.ident(0).is_some() && !self.is_punct(1, ":") {
                let name = self.expect_ident()?;
                obj.members.push(Member::Property {
                    name, ty: String::new(), value: None, default, readonly, required, loc,
                });
                return Ok(true);
            }
            self.i = save;
            return Ok(false);
        }
        self.i += 1;
        // The type: `int`, `var`, `QtObject`, `list<Item>`
        let mut ty = self.dotted()?;
        if self.is_punct(0, "<") {
            self.i += 1;
            let inner = self.dotted()?;
            self.expect_punct(">")?;
            ty = format!("{ty}<{inner}>");
        }
        let name = self.expect_ident()?;
        if ty == "alias" {
            self.expect_punct(":")?;
            let Value::Js { src, .. } = self.value()? else {
                return self.err("an alias names a property, not an object");
            };
            obj.members.push(Member::Alias { name, target: src.trim().to_string(), default, loc });
            return Ok(true);
        }
        let value = if self.is_punct(0, ":") {
            self.i += 1;
            Some(self.value()?)
        } else {
            None
        };
        obj.members.push(Member::Property { name, ty, value, default, readonly, required, loc });
        Ok(true)
    }

    /// `(a, b)`, `(int a, string b)`, `(a: int, b: string)`: names and declared
    /// types (`""` if none)
    fn params(&mut self) -> Result<(Vec<String>, Vec<String>), ParseError> {
        self.expect_punct("(")?;
        let mut out = Vec::new();
        let mut types = Vec::new();
        let mut group: Vec<String> = Vec::new();
        let mut typed_after = false;
        // `...args`: remaining arguments as an array
        let mut rest = false;
        loop {
            match self.at(0).map(|t| t.tok.clone()) {
                Some(Tok::Punct(")")) => {
                    self.i += 1;
                    break;
                }
                Some(Tok::Punct(",")) => {
                    self.i += 1;
                    if let Some((n, t)) = pick(&group, typed_after) {
                        out.push(if rest { format!("...{n}") } else { n });
                        types.push(t);
                    }
                    group.clear();
                    typed_after = false;
                    rest = false;
                }
                Some(Tok::Punct("...")) => {
                    rest = true;
                    self.i += 1;
                }
                Some(Tok::Punct(":")) => {
                    typed_after = true;
                    self.i += 1;
                }
                Some(Tok::Punct("=")) => {
                    // Default value, skipped to the next comma at this depth
                    self.i += 1;
                    let mut depth = 0;
                    while let Some(t) = self.at(0) {
                        match &t.tok {
                            Tok::Punct("(" | "[" | "{") => depth += 1,
                            Tok::Punct(")" | "]" | "}") if depth > 0 => depth -= 1,
                            Tok::Punct(")" | ",") if depth == 0 => break,
                            _ => {}
                        }
                        self.i += 1;
                    }
                }
                Some(Tok::Ident(s)) => {
                    group.push(s);
                    self.i += 1;
                }
                Some(_) => {
                    self.i += 1;
                }
                None => return self.err("a parameter list that never closes"),
            }
        }
        if let Some((n, t)) = pick(&group, typed_after) {
            out.push(if rest { format!("...{n}") } else { n });
            types.push(t);
        }
        return Ok((out, types));

        // `int a` and `a: int` both name a: the first word when a colon
        // follows, otherwise the last
        fn pick(group: &[String], typed_after: bool) -> Option<(String, String)> {
            let name = if typed_after { group.first() } else { group.last() }?.clone();
            let ty = match group.len() {
                0 | 1 => String::new(),
                _ if typed_after => group.last().cloned().unwrap_or_default(),
                _ => group.first().cloned().unwrap_or_default(),
            };
            Some((name, ty))
        }
    }

    /// `{ A, B = 5, C }`, numbered as QML numbers them
    fn enum_values(&mut self) -> Result<Vec<(String, i64)>, ParseError> {
        self.expect_punct("{")?;
        let mut out = Vec::new();
        let mut next = 0i64;
        loop {
            if self.is_punct(0, "}") {
                self.i += 1;
                return Ok(out);
            }
            let name = self.expect_ident()?;
            if self.is_punct(0, "=") {
                self.i += 1;
                let neg = self.is_punct(0, "-");
                if neg {
                    self.i += 1;
                }
                let Some(t) = self.at(0) else { return self.err("an enum that never closes") };
                let text = &self.src[t.start..t.end];
                let v = if let Some(h) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                    i64::from_str_radix(h, 16).ok()
                } else {
                    text.parse::<i64>().ok()
                };
                let Some(v) = v else { return self.err("an enum value is a whole number") };
                self.i += 1;
                next = if neg { -v } else { v };
            }
            out.push((name, next));
            next += 1;
            if self.is_punct(0, ",") {
                self.i += 1;
            }
        }
    }

    /// `{ ... }` as source, braces included, with the cursor after it
    fn braced(&mut self) -> Result<String, ParseError> {
        if !self.is_punct(0, "{") {
            return self.err("expected '{' here");
        }
        let start = self.at(0).unwrap().start;
        let mut depth = 0;
        loop {
            let Some(t) = self.at(0) else { return self.err("a block that never closes") };
            match &t.tok {
                Tok::Punct("{") => depth += 1,
                Tok::Punct("}") => {
                    depth -= 1;
                    if depth == 0 {
                        let end = t.end;
                        self.i += 1;
                        return Ok(self.src[start..end].to_string());
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
    }

    /// What follows `name:`: an object, a list of objects, or JavaScript
    fn value(&mut self) -> Result<Value, ParseError> {
        // `Type {`
        if let Some(w) = self.ident(0) {
            if is_type_name(w) {
                let mut k = 1;
                while self.is_punct(k, ".") && self.ident(k + 1).is_some() {
                    k += 2;
                }
                if self.is_punct(k, "{") {
                    let mut inline = Vec::new();
                    return Ok(Value::Object(self.object(&mut inline)?));
                }
            }
        }
        // `[ Type { }, Type { } ]`
        if self.is_punct(0, "[") && self.ident(1).is_some_and(is_type_name) && self.is_punct(2, "{") {
            self.i += 1;
            let mut list = Vec::new();
            let mut inline = Vec::new();
            loop {
                list.push(self.object(&mut inline)?);
                if self.is_punct(0, ",") {
                    self.i += 1;
                    continue;
                }
                self.expect_punct("]")?;
                break;
            }
            return Ok(Value::Objects(list));
        }
        self.js()
    }

    /// One JavaScript value, to where it ends (the rule at the top)
    fn js(&mut self) -> Result<Value, ParseError> {
        let Some(first) = self.at(0) else { return self.err("expected a value here") };
        let loc = first.loc;
        let start = first.start;
        let mut end = start;
        // Open brackets with their locations, so a mismatch is reported at the
        // unclosed bracket
        let mut open: Vec<(&'static str, Loc)> = Vec::new();
        let mut n = 0;
        // A statement value whose body follows its head on the next line:
        // `onExited: if (hovered)`
        let is_if = matches!(&first.tok, Tok::Ident(w) if w == "if");
        let mut awaiting_body = false;
        // Position of the last top-level `if`/`for`/`while` whose condition is
        // still open
        let mut keyword: Option<usize> = None;
        while let Some(t) = self.at(0) {
            let depth = open.len();
            if depth == 0 && n > 0 {
                if matches!(t.tok, Tok::Punct(";")) {
                    // `if (a) x; else y` is one statement
                    if is_if && matches!(self.at(1).map(|t| &t.tok), Some(Tok::Ident(w)) if w == "else") {
                        end = t.end;
                        self.i += 1;
                        n += 1;
                        continue;
                    }
                    self.i += 1;
                    break;
                }
                if matches!(t.tok, Tok::Punct("}") | Tok::Punct("]") | Tok::Punct(")")) {
                    break;
                }
                let else_follows = is_if && matches!(&t.tok, Tok::Ident(w) if w == "else");
                if t.newline_before && !awaiting_body && !else_follows
                    && ends_value(&self.toks[self.i - 1]) && !continues(t)
                {
                    break;
                }
                awaiting_body = false;
            }
            match t.tok {
                Tok::Punct(p @ ("(" | "[" | "{")) => open.push((p, t.loc)),
                Tok::Punct(p @ (")" | "]" | "}")) => {
                    let Some((o, at)) = open.pop() else { break };
                    let want = match o { "(" => ")", "[" => "]", _ => "}" };
                    if p != want {
                        return Err(ParseError { loc: at, what: format!("this '{o}' is closed by '{p}'") });
                    }
                    // Condition closed; a body follows
                    if p == ")" && open.is_empty() {
                        if let Some(k) = keyword.take() {
                            awaiting_body = paren_closes_head(&self.toks, k, self.i);
                        }
                    }
                }
                Tok::Ident(ref w) if depth == 0 && matches!(w.as_str(), "if" | "for" | "while") => {
                    keyword = Some(self.i);
                }
                Tok::Ident(ref w) if w == "else" && depth == 0 => awaiting_body = true,
                _ => {}
            }
            end = t.end;
            self.i += 1;
            n += 1;
        }
        if n == 0 {
            return self.err("expected a value here");
        }
        if let Some((o, at)) = open.last() {
            return Err(ParseError { loc: *at, what: format!("this '{o}' never closes") });
        }
        Ok(Value::Js { src: self.src[start..end].to_string(), loc })
    }
}

/// Whether the `)` at `close` ends the condition of the keyword at `start`
fn paren_closes_head(toks: &[Token], start: usize, close: usize) -> bool {
    if !matches!(toks.get(start + 1).map(|t| &t.tok), Some(Tok::Punct("("))) {
        return false;
    }
    let mut depth = 0;
    for (k, t) in toks.iter().enumerate().take(close + 1).skip(start + 1) {
        match t.tok {
            Tok::Punct("(") => depth += 1,
            Tok::Punct(")") => {
                depth -= 1;
                if depth == 0 {
                    return k == close;
                }
            }
            _ => {}
        }
    }
    false
}

/// Can an expression end with this token?
fn ends_value(t: &Token) -> bool {
    match &t.tok {
        Tok::Ident(_) | Tok::Number(_) | Tok::Str(_) | Tok::Template | Tok::Regex => true,
        Tok::Punct(p) => matches!(*p, ")" | "]" | "}" | "++" | "--"),
    }
}

/// Does a line beginning with this token carry on the line before?
fn continues(t: &Token) -> bool {
    match &t.tok {
        Tok::Punct(p) => !matches!(*p, "}" | "{" | "(" | "[" | ";" | "!" | "~" | "++" | "--" | "@"),
        Tok::Ident(w) => matches!(w.as_str(), "instanceof" | "in"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn js(v: &Value) -> &str {
        match v {
            Value::Js { src, .. } => src,
            other => panic!("not JavaScript: {other:?}"),
        }
    }

    fn assign<'d>(o: &'d ObjDef, name: &str) -> &'d Value {
        o.members.iter().find_map(|m| match m {
            Member::Assign { name: n, value, .. } if n == name => Some(value),
            _ => None,
        }).unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn imports_pragmas_and_a_tree() {
        let d = parse(r#"
            pragma Singleton
            import QtQuick 2.15
            import Quickshell.Io
            import "widgets" as W
            Item {
                id: root
                width: 100; height: width * 2
                Rectangle { color: "red" }
            }
        "#).unwrap();
        assert_eq!(d.pragmas, vec!["Singleton"]);
        assert_eq!(d.imports.len(), 3);
        assert_eq!(d.imports[0].version.as_deref(), Some("2.15"));
        assert_eq!(d.imports[1].what, "Quickshell.Io");
        assert!(d.imports[2].is_path);
        assert_eq!(d.imports[2].alias.as_deref(), Some("W"));
        assert_eq!(d.root.ty, "Item");
        assert_eq!(d.root.id.as_deref(), Some("root"));
        assert_eq!(js(assign(&d.root, "width")), "100");
        assert_eq!(js(assign(&d.root, "height")), "width * 2");
        assert!(matches!(&d.root.members[2], Member::Child(c) if c.ty == "Rectangle"));
    }

    #[test]
    fn where_a_value_ends() {
        let d = parse(r#"
            Item {
                a: cond
                   ? 1
                   : 2
                b: [1,
                    2]
                c: x +
                   y
                d: { if (x) return 1; return "}"; }
                e: `t ${ {a: 1}.a } }`
                f: s.replace(/[}]/g, "")
                t: `a '${x.replace(/'/g, "'\\''")}' b`
                g: foo(); h: 3
            }
        "#).unwrap();
        let o = &d.root;
        assert!(js(assign(o, "a")).contains(": 2"));
        assert!(js(assign(o, "b")).ends_with("2]"));
        assert!(js(assign(o, "c")).ends_with("y"));
        assert!(js(assign(o, "d")).ends_with("}"));
        assert!(js(assign(o, "e")).ends_with('`'));
        assert_eq!(js(assign(o, "f")), r#"s.replace(/[}]/g, "")"#);
        assert!(js(assign(o, "t")).ends_with("b`"));
        assert_eq!(js(assign(o, "g")), "foo()");
        assert_eq!(js(assign(o, "h")), "3");
    }

    #[test]
    fn declarations() {
        let d = parse(r#"
            Item {
                property int count: 0
                readonly property string name: "x"
                required property var model
                property list<Item> items
                property alias label: text.text
                signal picked(int index, string what)
                signal plain
                function add(a, b: int): int { return a + b }
                default property alias content: box.data
            }
        "#).unwrap();
        let m = &d.root.members;
        assert!(matches!(&m[0], Member::Property { name, ty, .. } if name == "count" && ty == "int"));
        assert!(matches!(&m[1], Member::Property { readonly: true, .. }));
        assert!(matches!(&m[2], Member::Property { required: true, value: None, .. }));
        assert!(matches!(&m[3], Member::Property { ty, .. } if ty == "list<Item>"));
        assert!(matches!(&m[4], Member::Alias { target, .. } if target == "text.text"));
        assert!(matches!(&m[5], Member::Signal { params, .. } if params == &["index", "what"]));
        assert!(matches!(&m[6], Member::Signal { params, .. } if params.is_empty()));
        assert!(matches!(&m[7], Member::Function { params, body, .. }
            if params == &["a", "b"] && body.starts_with('{') && body.ends_with('}')));
        assert!(matches!(&m[8], Member::Alias { name, .. } if name == "content"));
    }

    #[test]
    fn groups_attached_and_behaviors() {
        let d = parse(r#"
            Item {
                anchors { left: parent.left; margins: 4 }
                anchors.top: parent.top
                font.pixelSize: 12
                Layout.fillWidth: true
                Component.onCompleted: console.log("up")
                Behavior on opacity { NumberAnimation { duration: 200 } }
                onClicked: (mouse) => { count++ }
                model: [ Item {}, Item {} ]
            }
        "#).unwrap();
        let o = &d.root;
        assert_eq!(js(assign(o, "anchors.left")), "parent.left");
        assert_eq!(js(assign(o, "anchors.margins")), "4");
        assert_eq!(js(assign(o, "anchors.top")), "parent.top");
        assert_eq!(js(assign(o, "font.pixelSize")), "12");
        assert_eq!(js(assign(o, "Layout.fillWidth")), "true");
        assert!(js(assign(o, "Component.onCompleted")).starts_with("console.log"));
        assert!(o.members.iter().any(|m| matches!(m, Member::On { prop, obj, .. }
            if prop == "opacity" && obj.ty == "Behavior")));
        assert!(js(assign(o, "onClicked")).starts_with("(mouse) =>"));
        assert!(matches!(assign(o, "model"), Value::Objects(v) if v.len() == 2));
    }

    #[test]
    fn statement_values() {
        // A switch binding, an `if` with its body on the next line, an if/else
        // across lines, and a Behavior in a group
        let d = parse(r#"
            Item {
                shape: switch (sides) {
                    case 4: return a;
                    default: return b
                }
                onExited: if (root.hovered === index)
                            root.hovered = -1
                onClicked: if (x) a()
                           else b()
                w: if (wide) 200; else 100
                next: 1
                anchors {
                    bottomMargin: 20
                    Behavior on bottomMargin { NumberAnimation { duration: 200 } }
                }
            }
        "#).unwrap();
        let o = &d.root;
        assert!(js(assign(o, "shape")).ends_with('}'));
        assert!(js(assign(o, "onExited")).ends_with("= -1"), "{}", js(assign(o, "onExited")));
        assert!(js(assign(o, "onClicked")).ends_with("b()"), "{}", js(assign(o, "onClicked")));
        assert_eq!(js(assign(o, "w")), "if (wide) 200; else 100");
        assert_eq!(js(assign(o, "next")), "1");
        assert!(o.members.iter().any(|m| matches!(m, Member::On { prop, .. } if prop == "anchors.bottomMargin")));
    }

    #[test]
    fn errors_say_where() {
        let e = parse("Item {\n  width: (1 + \n}").unwrap_err();
        assert_eq!(e.loc.line, 2);
        let e = parse("Item {\n  width 3\n}").unwrap_err();
        assert!(e.what.contains("width"), "{e}");
        assert!(parse("Item { } Item { }").is_err());
    }
}
