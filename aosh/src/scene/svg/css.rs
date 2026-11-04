// SVG element style from style sheets
//
// Cascade, weakest first: presentation attributes (`fill="#fff"`), `<style>`
// rules by specificity then order, the `style` attribute. Selectors are
// compound (element, classes, id, `*`) and lists; combinators match nothing.
// KDE's color scheme uses `.ColorScheme-Text { color: … }` and
// `fill: currentColor`.

use super::xml::Element;

/// The properties an attribute of the same name gives
pub const PROPERTIES: &[&str] = &[
    "fill", "fill-opacity", "fill-rule", "stroke", "stroke-width", "stroke-opacity", "stroke-linecap",
    "stroke-linejoin", "stroke-miterlimit", "stroke-dasharray", "stroke-dashoffset", "opacity", "color",
    "stop-color", "stop-opacity", "display", "visibility",
];

struct Rule {
    sel: Compound,
    /// Ids, classes, element names
    spec: (u32, u32, u32),
    decls: Vec<(String, String)>,
}

#[derive(Default)]
struct Compound {
    name: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Default)]
pub struct Sheet {
    rules: Vec<Rule>,
}

impl Sheet {
    /// Every `<style>` under `root`, in document order
    pub fn of(root: &Element) -> Sheet {
        let mut sheet = Sheet::default();
        collect(root, &mut sheet);
        sheet
    }

    fn add(&mut self, text: &str) {
        let text = strip_comments(text);
        let mut rest = text.as_str();
        while let Some(open) = rest.find('{') {
            let sels = rest[..open].trim();
            let Some(close) = rest[open..].find('}') else { break };
            let body = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            // At-rules (`@media`, `@font-face`) carry nothing an icon draws
            if sels.starts_with('@') {
                continue;
            }
            let decls = declarations(body);
            for s in sels.split(',') {
                if let Some((sel, spec)) = compound(s.trim()) {
                    self.rules.push(Rule { sel, spec, decls: decls.clone() });
                }
            }
        }
    }

    /// Declarations for `el`: presentation attributes, matching rules from
    /// weakest, then `style`. Later entries override
    pub fn cascade(&self, el: &Element) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = el.attrs.iter()
            .filter(|(k, _)| PROPERTIES.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.trim().to_string()))
            .collect();
        let mut hits: Vec<(&Rule, usize)> = self.rules.iter().enumerate()
            .filter(|(_, r)| matches(&r.sel, el))
            .map(|(i, r)| (r, i))
            .collect();
        hits.sort_by_key(|(r, i)| (r.spec, *i));
        for (r, _) in hits {
            out.extend(r.decls.iter().cloned());
        }
        if let Some(s) = el.attr("style") {
            out.extend(declarations(s));
        }
        out
    }
}

fn collect(el: &Element, sheet: &mut Sheet) {
    if el.name == "style" {
        sheet.add(&el.text());
    }
    for c in el.elements() {
        collect(c, sheet);
    }
}

/// `a: b; c: d`, names lowercased, `!important` dropped
pub fn declarations(s: &str) -> Vec<(String, String)> {
    strip_comments(s).split(';').filter_map(|d| {
        let (k, v) = d.split_once(':')?;
        let v = v.trim().trim_end_matches("!important").trim();
        let k = k.trim().to_ascii_lowercase();
        (!k.is_empty()).then(|| (k, v.to_string()))
    }).collect()
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `rect.a.b#c`, `*`, `.a`: one compound selector and its specificity
fn compound(s: &str) -> Option<(Compound, (u32, u32, u32))> {
    if s.is_empty() || s.contains(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~' | '[' | ':')) {
        return None;
    }
    let mut c = Compound::default();
    let mut spec = (0, 0, 0);
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        let kind = b[i];
        let start = if matches!(kind, b'.' | b'#') { i + 1 } else { i };
        let mut end = start;
        while end < b.len() && !matches!(b[end], b'.' | b'#') {
            end += 1;
        }
        let part = &s[start..end];
        match kind {
            b'.' => {
                c.classes.push(part.to_string());
                spec.1 += 1;
            }
            b'#' => {
                c.id = Some(part.to_string());
                spec.0 += 1;
            }
            _ if part == "*" => {}
            _ => {
                c.name = Some(part.to_string());
                spec.2 += 1;
            }
        }
        i = end.max(i + 1);
    }
    Some((c, spec))
}

fn matches(c: &Compound, el: &Element) -> bool {
    if c.name.as_ref().is_some_and(|n| *n != el.name) {
        return false;
    }
    if c.id.as_ref().is_some_and(|id| el.attr("id") != Some(id.as_str())) {
        return false;
    }
    let classes: Vec<&str> = el.attr("class").map(|v| v.split_whitespace().collect()).unwrap_or_default();
    c.classes.iter().all(|k| classes.contains(&k.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(src: &str) -> Element {
        super::super::xml::parse(src).unwrap()
    }

    #[test]
    fn the_cascade_is_attribute_then_rules_by_weight_then_style() {
        let doc = el(r#"<svg><style>
            /* a comment */
            rect { fill: red; stroke: blue }
            .a { fill: green }
            #x.a { fill: navy }
            .a { stroke: gray }
            g rect { fill: pink }
        </style><rect id="x" class="a b" fill="black" style="stroke-width: 2"/></svg>"#);
        let sheet = Sheet::of(&doc);
        let rect = doc.elements().find(|e| e.name == "rect").unwrap();
        let got = sheet.cascade(rect);
        let last = |k: &str| got.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(last("fill"), Some("navy"));
        assert_eq!(last("stroke"), Some("gray"));
        assert_eq!(last("stroke-width"), Some("2"));
    }
}
